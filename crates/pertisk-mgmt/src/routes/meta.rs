//! Cluster metadata endpoints (K8s versions, etc.).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::Value;

use crate::error::{ApiResult, AppError};
use crate::state::AppState;

use super::CurrentUser;

const CACHE_TTL: Duration = Duration::from_secs(3600);
const KEEP: usize = 10;

struct VersionCache {
    fetched_at: Instant,
    versions: Vec<String>,
    source: &'static str,
}

static CACHE: Mutex<Option<VersionCache>> = Mutex::new(None);

#[derive(Serialize)]
struct K8sVersionsOut {
    versions: Vec<String>,
    latest: String,
    /// Kubelet pin currently baked into `out/runtime` (if present).
    image: Option<String>,
    source: String,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/meta/k8s-versions", get(k8s_versions))
}

async fn k8s_versions(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<K8sVersionsOut>> {
    let image = read_image_k8s_ver();
    let (mut versions, source) = cached_or_fetch(&state).await?;

    if let Some(ref img) = image {
        if !versions.iter().any(|v| v == img) {
            versions.push(img.clone());
            sort_versions_desc(&mut versions);
        }
    }

    if versions.is_empty() {
        versions = fallback_versions(image.as_deref());
    }

    let latest = versions
        .first()
        .cloned()
        .unwrap_or_else(|| "v1.36.3".into());

    Ok(Json(K8sVersionsOut {
        versions,
        latest,
        image,
        source: source.into(),
    }))
}

async fn cached_or_fetch(state: &AppState) -> Result<(Vec<String>, &'static str), AppError> {
    if let Ok(guard) = CACHE.lock() {
        if let Some(c) = guard.as_ref() {
            if c.fetched_at.elapsed() < CACHE_TTL && !c.versions.is_empty() {
                return Ok((c.versions.clone(), c.source));
            }
        }
    }

    match fetch_github_versions(&state.inner.http).await {
        Ok(versions) if !versions.is_empty() => {
            if let Ok(mut guard) = CACHE.lock() {
                *guard = Some(VersionCache {
                    fetched_at: Instant::now(),
                    versions: versions.clone(),
                    source: "github",
                });
            }
            Ok((versions, "github"))
        }
        Ok(_) | Err(_) => {
            let image = read_image_k8s_ver();
            let versions = fallback_versions(image.as_deref());
            Ok((versions, "fallback"))
        }
    }
}

async fn fetch_github_versions(http: &reqwest::Client) -> anyhow::Result<Vec<String>> {
    let url = "https://api.github.com/repos/kubernetes/kubernetes/releases?per_page=40";
    let resp = http
        .get(url)
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
        if item
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(tag) = item.get("tag_name").and_then(|v| v.as_str()) else {
            continue;
        };
        if is_stable_tag(tag) {
            versions.push(tag.to_string());
        }
    }

    sort_versions_desc(&mut versions);
    versions.dedup();
    versions.truncate(KEEP);
    Ok(versions)
}

fn is_stable_tag(tag: &str) -> bool {
    let Some(rest) = tag.strip_prefix('v') else {
        return false;
    };
    let parts: Vec<&str> = rest.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn parse_ver(tag: &str) -> Option<(u64, u64, u64)> {
    let rest = tag.strip_prefix('v').unwrap_or(tag);
    let mut it = rest.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn sort_versions_desc(versions: &mut [String]) {
    versions.sort_by(|a, b| {
        let pa = parse_ver(a).unwrap_or((0, 0, 0));
        let pb = parse_ver(b).unwrap_or((0, 0, 0));
        pb.cmp(&pa)
    });
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

fn fallback_versions(image: Option<&str>) -> Vec<String> {
    let mut versions = vec![
        "v1.36.3".into(),
        "v1.36.2".into(),
        "v1.36.1".into(),
        "v1.36.0".into(),
        "v1.35.7".into(),
        "v1.35.6".into(),
        "v1.34.10".into(),
        "v1.34.9".into(),
        "v1.33.13".into(),
        "v1.33.12".into(),
    ];
    if let Some(img) = image {
        if !versions.iter().any(|v| v == img) {
            versions.insert(0, img.to_string());
        }
    }
    sort_versions_desc(&mut versions);
    versions.dedup();
    versions.truncate(KEEP);
    versions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_tag_filter() {
        assert!(is_stable_tag("v1.36.3"));
        assert!(!is_stable_tag("v1.37.0-beta.0"));
        assert!(!is_stable_tag("v1.36.0-rc.1"));
        assert!(!is_stable_tag("1.36.3"));
    }

    #[test]
    fn sort_desc() {
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
}
