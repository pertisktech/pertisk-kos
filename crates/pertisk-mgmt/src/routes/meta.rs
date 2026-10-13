//! Cluster metadata endpoints (K8s versions, etc.).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ApiResult, AppError};
use crate::state::AppState;

use super::CurrentUser;

const CACHE_TTL: Duration = Duration::from_secs(3600);
const KEEP_STABLE: usize = 10;
const KEEP_ALL: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Channel {
    Alpha = 0,
    Beta = 1,
    Rc = 2,
    Stable = 3,
}

impl Channel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Alpha => "alpha",
            Self::Beta => "beta",
            Self::Rc => "rc",
            Self::Stable => "stable",
        }
    }
}

/// Parsed Kubernetes release tag for sorting / filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedTag {
    major: u64,
    minor: u64,
    patch: u64,
    channel: Channel,
    pre_n: u64,
}

impl PartialOrd for ParsedTag {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ParsedTag {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Descending: higher version first; within same X.Y.Z, stable > rc > beta > alpha.
        other
            .major
            .cmp(&self.major)
            .then(other.minor.cmp(&self.minor))
            .then(other.patch.cmp(&self.patch))
            .then(other.channel.cmp(&self.channel))
            .then(other.pre_n.cmp(&self.pre_n))
    }
}

struct VersionCache {
    fetched_at: Instant,
    versions: Vec<String>,
    channels: BTreeMap<String, String>,
    latest_stable: String,
    source: &'static str,
}

struct DualCache {
    stable: Option<VersionCache>,
    all: Option<VersionCache>,
}

static CACHE: Mutex<DualCache> = Mutex::new(DualCache {
    stable: None,
    all: None,
});

#[derive(Deserialize)]
struct K8sVersionsQuery {
    #[serde(default)]
    include_prerelease: bool,
}

#[derive(Serialize)]
struct K8sVersionsOut {
    versions: Vec<String>,
    /// Newest **stable** tag (never a prerelease).
    latest: String,
    /// Kubelet pin currently baked into `out/runtime` (if present).
    image: Option<String>,
    source: String,
    /// Map of tag → `stable` | `alpha` | `beta` | `rc`.
    channels: BTreeMap<String, String>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/meta/k8s-versions", get(k8s_versions))
}

async fn k8s_versions(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Query(q): Query<K8sVersionsQuery>,
) -> ApiResult<Json<K8sVersionsOut>> {
    let image = read_image_k8s_ver();
    let (mut versions, mut channels, latest_stable, source) =
        cached_or_fetch(&state, q.include_prerelease).await?;

    if let Some(ref img) = image {
        if !versions.iter().any(|v| v == img) {
            versions.push(img.clone());
            if let Some(ch) = parse_tag(img) {
                channels.insert(img.clone(), ch.channel.as_str().into());
            }
            sort_versions_desc(&mut versions);
        }
    }

    if versions.is_empty() {
        versions = fallback_versions(image.as_deref(), q.include_prerelease);
        channels = channels_for(&versions);
    }

    let latest = if !latest_stable.is_empty() {
        latest_stable
    } else {
        versions
            .iter()
            .find(|v| parse_tag(v).is_some_and(|p| p.channel == Channel::Stable))
            .cloned()
            .or_else(|| versions.first().cloned())
            .unwrap_or_else(|| "v1.37.1".into())
    };

    Ok(Json(K8sVersionsOut {
        versions,
        latest,
        image,
        source: source.into(),
        channels,
    }))
}

async fn cached_or_fetch(
    state: &AppState,
    include_prerelease: bool,
) -> Result<(Vec<String>, BTreeMap<String, String>, String, &'static str), AppError> {
    if let Ok(guard) = CACHE.lock() {
        let slot = if include_prerelease {
            guard.all.as_ref()
        } else {
            guard.stable.as_ref()
        };
        if let Some(c) = slot {
            if c.fetched_at.elapsed() < CACHE_TTL && !c.versions.is_empty() {
                return Ok((
                    c.versions.clone(),
                    c.channels.clone(),
                    c.latest_stable.clone(),
                    c.source,
                ));
            }
        }
    }

    match fetch_github_versions(&state.inner.http, include_prerelease).await {
        Ok((versions, channels, latest_stable)) if !versions.is_empty() => {
            if let Ok(mut guard) = CACHE.lock() {
                let entry = VersionCache {
                    fetched_at: Instant::now(),
                    versions: versions.clone(),
                    channels: channels.clone(),
                    latest_stable: latest_stable.clone(),
                    source: "github",
                };
                if include_prerelease {
                    // Also refresh stable cache from the same payload.
                    let stable_versions: Vec<String> = versions
                        .iter()
                        .filter(|v| {
                            parse_tag(v).is_some_and(|p| p.channel == Channel::Stable)
                        })
                        .cloned()
                        .take(KEEP_STABLE)
                        .collect();
                    let stable_channels = channels_for(&stable_versions);
                    guard.all = Some(entry);
                    guard.stable = Some(VersionCache {
                        fetched_at: Instant::now(),
                        versions: stable_versions,
                        channels: stable_channels,
                        latest_stable: latest_stable.clone(),
                        source: "github",
                    });
                } else {
                    guard.stable = Some(entry);
                }
            }
            Ok((versions, channels, latest_stable, "github"))
        }
        Ok(_) | Err(_) => {
            let image = read_image_k8s_ver();
            let versions = fallback_versions(image.as_deref(), include_prerelease);
            let channels = channels_for(&versions);
            let latest_stable = versions
                .iter()
                .find(|v| parse_tag(v).is_some_and(|p| p.channel == Channel::Stable))
                .cloned()
                .unwrap_or_else(|| "v1.37.1".into());
            Ok((versions, channels, latest_stable, "fallback"))
        }
    }
}

async fn fetch_github_versions(
    http: &reqwest::Client,
    include_prerelease: bool,
) -> anyhow::Result<(Vec<String>, BTreeMap<String, String>, String)> {
    let per_page = if include_prerelease { 100 } else { 40 };
    let url = format!(
        "https://api.github.com/repos/kubernetes/kubernetes/releases?per_page={per_page}"
    );
    let resp = http
        .get(&url)
        .header("User-Agent", "pertisk-mgmt")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?;
    let body: Value = resp.json().await?;
    let arr = body
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("github releases not an array"))?;

    let mut versions: Vec<String> = Vec::new();
    for item in arr {
        if item.get("draft").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let is_pre = item
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if is_pre && !include_prerelease {
            continue;
        }
        let Some(tag) = item.get("tag_name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(parsed) = parse_tag(tag) else {
            continue;
        };
        // Stable always; alpha/beta/RC only when include_prerelease.
        if !allowed_channel(parsed.channel, include_prerelease) {
            continue;
        }
        versions.push(tag.to_string());
    }

    sort_versions_desc(&mut versions);
    versions.dedup();
    let keep = if include_prerelease {
        KEEP_ALL
    } else {
        KEEP_STABLE
    };
    versions.truncate(keep);

    let channels = channels_for(&versions);
    let latest_stable = versions
        .iter()
        .find(|v| parse_tag(v).is_some_and(|p| p.channel == Channel::Stable))
        .cloned()
        .unwrap_or_default();

    Ok((versions, channels, latest_stable))
}

fn allowed_channel(channel: Channel, include_prerelease: bool) -> bool {
    match channel {
        Channel::Stable => true,
        Channel::Alpha | Channel::Beta | Channel::Rc => include_prerelease,
    }
}

/// Accept `vMAJOR.MINOR.PATCH` optionally followed by `-alpha.N`, `-beta.N`, or `-rc.N`.
fn parse_tag(tag: &str) -> Option<ParsedTag> {
    let rest = tag.strip_prefix('v')?;
    let (core, pre) = match rest.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (rest, None),
    };
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    let (channel, pre_n) = match pre {
        None => (Channel::Stable, 0),
        Some(p) => {
            let (kind, n) = p.split_once('.')?;
            let channel = match kind {
                "alpha" => Channel::Alpha,
                "beta" => Channel::Beta,
                "rc" => Channel::Rc,
                _ => return None,
            };
            if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let pre_n: u64 = n.parse().ok()?;
            (channel, pre_n)
        }
    };
    Some(ParsedTag {
        major,
        minor,
        patch,
        channel,
        pre_n,
    })
}

fn sort_versions_desc(versions: &mut [String]) {
    versions.sort_by(|a, b| {
        let pa = parse_tag(a);
        let pb = parse_tag(b);
        match (pa, pb) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.cmp(a),
        }
    });
}

fn channels_for(versions: &[String]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for v in versions {
        if let Some(p) = parse_tag(v) {
            map.insert(v.clone(), p.channel.as_str().into());
        }
    }
    map
}

fn read_image_k8s_ver() -> Option<String> {
    let candidates = [
        std::path::PathBuf::from("out/runtime/versions.txt"),
        std::path::PathBuf::from("./out/runtime/versions.txt"),
    ];
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("K8S_VER=") {
                    let v = v.trim();
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

fn fallback_versions(image: Option<&str>, include_prerelease: bool) -> Vec<String> {
    let mut versions = vec![
        "v1.37.1".into(),
        "v1.36.5".into(),
        "v1.35.9".into(),
        "v1.34.12".into(),
        "v1.37.0".into(),
        "v1.36.4".into(),
        "v1.35.8".into(),
        "v1.34.11".into(),
        "v1.36.3".into(),
        "v1.35.7".into(),
    ];
    if include_prerelease {
        versions.extend([
            "v1.38.0-alpha.2".into(),
            "v1.38.0-alpha.1".into(),
            "v1.37.0-rc.1".into(),
            "v1.37.0-beta.0".into(),
        ]);
    }
    if let Some(img) = image {
        if !versions.iter().any(|v| v == img) {
            versions.insert(0, img.to_string());
        }
    }
    sort_versions_desc(&mut versions);
    versions.dedup();
    let keep = if include_prerelease {
        KEEP_ALL
    } else {
        KEEP_STABLE
    };
    versions.truncate(keep);
    versions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_stable_tag(tag: &str) -> bool {
        parse_tag(tag).is_some_and(|p| p.channel == Channel::Stable)
    }

    fn is_release_tag(tag: &str) -> bool {
        parse_tag(tag).is_some()
    }

    #[test]
    fn stable_tag_filter() {
        assert!(is_stable_tag("v1.36.3"));
        assert!(!is_stable_tag("v1.37.0-beta.0"));
        assert!(!is_stable_tag("v1.36.0-rc.1"));
        assert!(!is_stable_tag("1.36.3"));
    }

    #[test]
    fn release_tag_accepts_prereleases() {
        assert!(is_release_tag("v1.38.0-alpha.2"));
        assert!(is_release_tag("v1.37.0-beta.0"));
        assert!(is_release_tag("v1.37.0-rc.1"));
        assert!(is_release_tag("v1.37.1"));
        assert!(!is_release_tag("v1.37.0-alpha"));
        assert!(!is_release_tag("v1.37.0-nightly.1"));
        assert!(!is_release_tag("1.36.3"));
    }

    #[test]
    fn list_includes_prereleases_when_enabled() {
        assert!(allowed_channel(Channel::Stable, false));
        assert!(!allowed_channel(Channel::Rc, false));
        assert!(!allowed_channel(Channel::Alpha, false));
        assert!(!allowed_channel(Channel::Beta, false));

        assert!(allowed_channel(Channel::Stable, true));
        assert!(allowed_channel(Channel::Rc, true));
        assert!(allowed_channel(Channel::Alpha, true));
        assert!(allowed_channel(Channel::Beta, true));
    }

    #[test]
    fn parse_channels() {
        assert_eq!(parse_tag("v1.37.1").unwrap().channel, Channel::Stable);
        assert_eq!(parse_tag("v1.38.0-alpha.2").unwrap().pre_n, 2);
        assert_eq!(parse_tag("v1.37.0-beta.0").unwrap().channel, Channel::Beta);
        assert_eq!(parse_tag("v1.37.0-rc.1").unwrap().channel, Channel::Rc);
    }

    #[test]
    fn sort_desc_stable() {
        let mut v = vec![
            "v1.35.7".into(),
            "v1.36.1".into(),
            "v1.36.3".into(),
            "v1.34.10".into(),
        ];
        sort_versions_desc(&mut v);
        assert_eq!(v[0], "v1.36.3");
        assert_eq!(v[1], "v1.36.1");
    }

    #[test]
    fn sort_prerelease_order() {
        let mut v = vec![
            "v1.38.0-alpha.2".into(),
            "v1.37.1".into(),
            "v1.37.0-rc.1".into(),
            "v1.37.0-beta.0".into(),
            "v1.37.0".into(),
            "v1.37.0-alpha.1".into(),
            "v1.37.0-rc.0".into(),
        ];
        sort_versions_desc(&mut v);
        assert_eq!(
            v,
            vec![
                "v1.38.0-alpha.2",
                "v1.37.1",
                "v1.37.0",
                "v1.37.0-rc.1",
                "v1.37.0-rc.0",
                "v1.37.0-beta.0",
                "v1.37.0-alpha.1",
            ]
        );
    }

    #[test]
    fn channels_map() {
        let map = channels_for(&[
            "v1.37.1".into(),
            "v1.38.0-alpha.2".into(),
            "v1.37.0-beta.0".into(),
            "v1.37.0-rc.1".into(),
        ]);
        assert_eq!(map.get("v1.37.1").map(String::as_str), Some("stable"));
        assert_eq!(map.get("v1.38.0-alpha.2").map(String::as_str), Some("alpha"));
        assert_eq!(map.get("v1.37.0-beta.0").map(String::as_str), Some("beta"));
        assert_eq!(map.get("v1.37.0-rc.1").map(String::as_str), Some("rc"));
    }
}
