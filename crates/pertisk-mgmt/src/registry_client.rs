//! Docker Registry HTTP API V2 helpers (Harbor, distribution, Pertisk common registry).
//!
//! Follows the Bearer token challenge from `WWW-Authenticate` so anonymous and
//! basic-auth pulls work the same way `docker` / containerd do.

use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, WWW_AUTHENTICATE};
use serde_json::Value;

const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json";

#[derive(Debug, Clone)]
struct BearerChallenge {
    realm: String,
    service: Option<String>,
    scope: Option<String>,
}

fn http_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?)
}

fn upgrade_realm(realm: &str) -> String {
    realm
        .trim()
        .replacen("http://", "https://", 1)
        .trim_end_matches('/')
        .to_string()
}

fn parse_quoted_param(params: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = params.find(&needle)? + needle.len();
    let end = params[start..].find('"')? + start;
    Some(params[start..end].to_string())
}

fn parse_bearer_challenge(header: &str) -> Option<BearerChallenge> {
    let header = header.trim();
    if !header.to_ascii_lowercase().starts_with("bearer ") {
        return None;
    }
    let params = &header[7..];
    Some(BearerChallenge {
        realm: upgrade_realm(&parse_quoted_param(params, "realm")?),
        service: parse_quoted_param(params, "service"),
        scope: parse_quoted_param(params, "scope"),
    })
}

fn pull_scope(repository: &str, challenge_scope: Option<&str>) -> String {
    // Prefer pull-only even when the registry advertises pull,push.
    if let Some(scope) = challenge_scope {
        if scope.contains(&format!("repository:{repository}:")) {
            return format!("repository:{repository}:pull");
        }
    }
    format!("repository:{repository}:pull")
}

/// Exchange credentials (or anonymous) for a registry Bearer token.
pub async fn fetch_bearer_token(
    registry: &str,
    repository: &str,
    reference: &str,
    user: &str,
    password: &str,
) -> anyhow::Result<Option<String>> {
    let registry = registry.trim().trim_end_matches('/');
    let repository = repository.trim().trim_start_matches('/');
    let reference = if reference.trim().is_empty() {
        "latest"
    } else {
        reference.trim()
    };
    if registry.is_empty() || repository.is_empty() {
        bail!("registry and repository are required");
    }

    let client = http_client()?;
    let probe = format!("https://{registry}/v2/{repository}/manifests/{reference}");
    let probe_res = client
        .get(&probe)
        .header("Accept", MANIFEST_ACCEPT)
        .send()
        .await
        .with_context(|| format!("probe registry {registry}"))?;

    if probe_res.status().is_success() {
        // Some registries allow unauthenticated pulls with no token.
        return Ok(None);
    }

    let www = probe_res
        .headers()
        .get(WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if www.is_empty() {
        if probe_res.status().as_u16() != 401 {
            // Not an auth challenge (e.g. 404) — caller will surface the miss.
            return Ok(None);
        }
        bail!(
            "registry {registry} returned 401 without a Bearer challenge"
        );
    }
    let challenge = parse_bearer_challenge(&www).ok_or_else(|| {
        anyhow!(
            "registry {registry} returned {} with an unparsable Bearer challenge",
            probe_res.status()
        )
    })?;

    let mut url = reqwest::Url::parse(&challenge.realm)
        .with_context(|| format!("invalid token realm {}", challenge.realm))?;
    {
        let mut q = url.query_pairs_mut();
        if let Some(svc) = &challenge.service {
            q.append_pair("service", svc);
        }
        q.append_pair(
            "scope",
            &pull_scope(repository, challenge.scope.as_deref()),
        );
    }

    let mut req = client.get(url);
    let user = user.trim();
    if !user.is_empty() {
        req = req.basic_auth(user, Some(password));
    }

    let token_res = req
        .send()
        .await
        .with_context(|| format!("token request for {registry}"))?;
    if !token_res.status().is_success() {
        let status = token_res.status();
        let body = token_res.text().await.unwrap_or_default();
        if user.is_empty() {
            bail!(
                "registry {registry} does not allow anonymous pull ({status}). \
                 Set addon Registry user/password or MGMT_IMAGE_REGISTRY_USER / MGMT_IMAGE_REGISTRY_PASSWORD"
            );
        }
        bail!("registry {registry} auth failed ({status}): {body}");
    }

    let body: Value = token_res.json().await.context("parse registry token JSON")?;
    let token = body
        .get("token")
        .or_else(|| body.get("access_token"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("registry token response missing token"))?;
    Ok(Some(token))
}

fn auth_headers(token: Option<&str>, user: &str, password: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::ACCEPT, HeaderValue::from_static(MANIFEST_ACCEPT));
    if let Some(token) = token {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(AUTHORIZATION, v);
        }
    } else if !user.trim().is_empty() {
        let encoded = base64_auth(user.trim(), password);
        if let Ok(v) = HeaderValue::from_str(&format!("Basic {encoded}")) {
            headers.insert(AUTHORIZATION, v);
        }
    }
    headers
}

fn base64_auth(user: &str, password: &str) -> String {
    B64.encode(format!("{user}:{password}"))
}

/// Fetch an image manifest / index for `repository:reference` (tag or digest).
pub async fn fetch_manifest(
    registry: &str,
    repository: &str,
    reference: &str,
    user: &str,
    password: &str,
) -> anyhow::Result<Value> {
    let registry = registry.trim().trim_end_matches('/');
    let repository = repository.trim().trim_start_matches('/');
    let reference = reference.trim();
    if reference.is_empty() {
        bail!("image reference is empty");
    }

    let token = fetch_bearer_token(registry, repository, reference, user, password).await?;
    let client = http_client()?;
    let url = format!("https://{registry}/v2/{repository}/manifests/{reference}");
    let res = client
        .get(&url)
        .headers(auth_headers(token.as_deref(), user, password))
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        bail!("manifest {registry}/{repository}:{reference} → {status}: {body}");
    }
    Ok(res.json().await.context("parse manifest JSON")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bearer_upgrades_http_realm_and_keeps_fields() {
        let c = parse_bearer_challenge(
            r#"Bearer realm="http://registry.example/v2/token",service="registry.example",scope="repository:foo/bar:pull,push""#,
        )
        .unwrap();
        assert_eq!(c.realm, "https://registry.example/v2/token");
        assert_eq!(c.service.as_deref(), Some("registry.example"));
        assert_eq!(
            c.scope.as_deref(),
            Some("repository:foo/bar:pull,push")
        );
        assert_eq!(
            pull_scope("foo/bar", c.scope.as_deref()),
            "repository:foo/bar:pull"
        );
    }
}
