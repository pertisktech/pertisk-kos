use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::audit;
use crate::crypto;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::nutanix::NutanixClient;
use crate::proxmox::{ProbeResult, ProxmoxClient, ProxmoxStorage};
use crate::rbac::{require_admin, require_mutate};
use crate::routes::CurrentUser;
use crate::state::AppState;
use crate::vsphere::VsphereClient;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/providers", get(list).post(create))
        .route("/providers/probe", post(probe))
        .route("/providers/{id}", get(get_one).put(update).delete(delete))
        .route("/providers/{id}/test", post(test))
        .route("/providers/{id}/storage", get(storage))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProviderOut {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub token_id: String,
    pub node: String,
    pub storage: String,
    pub bridge: String,
    pub insecure: i64,
    pub defaults_json: String,
    /// Default guest arch for clusters on this provider (amd64|arm64).
    pub arch: String,
    pub created_at: String,
    pub updated_at: String,
    /// Live hypervisor API: `online` | `offline` | `unknown` (not stored).
    #[sqlx(skip)]
    #[serde(default)]
    pub availability: String,
}

#[derive(Deserialize)]
struct ProviderIn {
    name: String,
    url: String,
    token_id: String,
    token_secret: String,
    node: String,
    storage: String,
    #[serde(default = "default_bridge")]
    bridge: String,
    #[serde(default)]
    insecure: bool,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default = "default_defaults")]
    defaults: serde_json::Value,
    #[serde(default = "default_arch")]
    arch: String,
}

fn default_bridge() -> String {
    "vmbr0".into()
}

fn default_kind() -> String {
    "proxmox".into()
}

fn default_defaults() -> serde_json::Value {
    serde_json::json!({})
}

fn default_arch() -> String {
    "auto".into()
}

fn normalize_arch(arch: &str) -> ApiResult<String> {
    match arch.trim().to_ascii_lowercase().as_str() {
        "amd64" | "x86_64" | "x64" => Ok("amd64".into()),
        "arm64" | "aarch64" => Ok("arm64".into()),
        "auto" | "" => Ok("auto".into()),
        other => Err(AppError::bad(format!(
            "unsupported arch `{other}` (use amd64|arm64|auto)"
        ))),
    }
}

/// Resolve guest arch: explicit amd64/arm64, or auto from probe.
fn resolve_provider_arch(requested: &str, detected: Option<&str>) -> String {
    match requested {
        "amd64" | "arm64" => requested.into(),
        _ => match detected.map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("arm64" | "aarch64") => "arm64".into(),
            _ => "amd64".into(),
        },
    }
}

fn normalize_kind(kind: &str) -> ApiResult<String> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "proxmox" | "" => Ok("proxmox".into()),
        "vsphere" | "esxi" | "vmware" => Ok("vsphere".into()),
        "nutanix" | "ahv" | "prism" => Ok("nutanix".into()),
        other => Err(AppError::bad(format!(
            "unsupported provider kind `{other}` (use proxmox|vsphere|nutanix)"
        ))),
    }
}

#[derive(Deserialize)]
struct ProviderPatch {
    name: Option<String>,
    url: Option<String>,
    token_id: Option<String>,
    token_secret: Option<String>,
    node: Option<String>,
    storage: Option<String>,
    bridge: Option<String>,
    insecure: Option<bool>,
    defaults: Option<serde_json::Value>,
    arch: Option<String>,
}

const PROVIDER_SELECT: &str = r#"SELECT id, name, kind, url, token_id, node, storage, bridge, insecure,
       defaults_json, COALESCE(arch, 'amd64') as arch, created_at, updated_at
       FROM providers"#;

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<ProviderOut>>> {
    let mut rows = sqlx::query_as::<_, ProviderOut>(&format!("{PROVIDER_SELECT} ORDER BY name"))
        .fetch_all(state.pool())
        .await?;
    crate::provider_availability::fill(&state, &mut rows).await;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<ProviderOut>> {
    let mut row = sqlx::query_as::<_, ProviderOut>(&format!("{PROVIDER_SELECT} WHERE id = ?"))
        .bind(&id)
        .fetch_optional(state.pool())
        .await?
        .ok_or(AppError::NotFound)?;
    row.availability = crate::provider_availability::probe(&state, &id).await;
    Ok(Json(row))
}

async fn probe_provider(
    kind: &str,
    url: &str,
    token_id: &str,
    token_secret: &str,
    node: &str,
    storage: &str,
    bridge: &str,
    insecure: bool,
) -> ApiResult<ProbeResult> {
    match kind {
        "vsphere" => {
            let client = VsphereClient::new(
                url.to_string(),
                token_id.to_string(),
                token_secret.to_string(),
                insecure,
            );
            client.probe(Some(node), Some(storage), Some(bridge)).await
        }
        "nutanix" => {
            let client = NutanixClient::new(
                url.to_string(),
                token_id.to_string(),
                token_secret.to_string(),
                insecure,
            );
            client.probe(Some(node), Some(storage), Some(bridge)).await
        }
        _ => {
            let client = ProxmoxClient {
                url: url.to_string(),
                token_id: token_id.to_string(),
                token_secret: token_secret.to_string(),
                insecure,
            };
            client.probe(Some(node), Some(storage)).await
        }
    }
}

async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<ProviderIn>,
) -> ApiResult<Json<ProviderOut>> {
    require_mutate(&user)?;
    let kind = normalize_kind(&body.kind)?;
    let arch_req = normalize_arch(&body.arch)?;
    let probe = probe_provider(
        &kind,
        &body.url,
        &body.token_id,
        &body.token_secret,
        &body.node,
        &body.storage,
        &body.bridge,
        body.insecure,
    )
    .await?;
    if !probe.ok {
        let msg = probe
            .storage
            .as_ref()
            .filter(|s| !s.ok)
            .map(|s| s.message.clone())
            .unwrap_or_else(|| {
                if !probe.node_ok {
                    probe.node_message.clone()
                } else {
                    format!("{kind} probe failed")
                }
            });
        return Err(AppError::bad(msg));
    }
    let arch = resolve_provider_arch(&arch_req, probe.arch.as_deref());

    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    let enc =
        crypto::encrypt(&state.cfg().secret_key, &body.token_secret).map_err(AppError::Anyhow)?;
    let defaults = body.defaults.to_string();
    sqlx::query(
        r#"INSERT INTO providers
           (id, name, kind, url, token_id, token_secret_enc, node, storage, bridge, insecure, defaults_json, arch, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(&body.name)
    .bind(&kind)
    .bind(&body.url)
    .bind(&body.token_id)
    .bind(&enc)
    .bind(&body.node)
    .bind(&body.storage)
    .bind(&body.bridge)
    .bind(if body.insecure { 1 } else { 0 })
    .bind(&defaults)
    .bind(&arch)
    .bind(&now)
    .bind(&now)
    .execute(state.pool())
    .await?;
    audit(
        state.pool(),
        Some(&user.id),
        "provider.create",
        Some(&id),
        Some(&body.name),
    )
    .await;
    get_one(State(state), CurrentUser(user), Path(id)).await
}

async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<ProviderPatch>,
) -> ApiResult<Json<ProviderOut>> {
    require_mutate(&user)?;
    let existing = sqlx::query_as::<_, ProviderOut>(&format!("{PROVIDER_SELECT} WHERE id = ?"))
        .bind(&id)
        .fetch_optional(state.pool())
        .await?
        .ok_or(AppError::NotFound)?;

    let kind = existing.kind.clone();
    let name = body.name.unwrap_or(existing.name);
    let url = body.url.clone().unwrap_or(existing.url.clone());
    let token_id = body.token_id.clone().unwrap_or(existing.token_id.clone());
    let node = body.node.clone().unwrap_or(existing.node.clone());
    let storage = body.storage.clone().unwrap_or(existing.storage.clone());
    let bridge = body.bridge.unwrap_or(existing.bridge);
    let insecure = body
        .insecure
        .map(|b| if b { 1 } else { 0 })
        .unwrap_or(existing.insecure);
    let defaults = body
        .defaults
        .map(|d| d.to_string())
        .unwrap_or(existing.defaults_json);
    let now = db::now_rfc3339();

    let secret = if let Some(ref s) = body.token_secret {
        s.clone()
    } else {
        let enc: String = sqlx::query_scalar("SELECT token_secret_enc FROM providers WHERE id = ?")
            .bind(&id)
            .fetch_one(state.pool())
            .await?;
        crypto::decrypt(&state.cfg().secret_key, &enc).map_err(AppError::Anyhow)?
    };
    let probe = probe_provider(
        &kind,
        &url,
        &token_id,
        &secret,
        &node,
        &storage,
        &bridge,
        insecure != 0,
    )
    .await?;
    if !probe.ok {
        let msg = probe
            .storage
            .as_ref()
            .filter(|s| !s.ok)
            .map(|s| s.message.clone())
            .unwrap_or_else(|| {
                if !probe.node_ok {
                    probe.node_message.clone()
                } else {
                    format!("{kind} probe failed")
                }
            });
        return Err(AppError::bad(msg));
    }
    let arch = if let Some(a) = body.arch.as_deref() {
        let req = normalize_arch(a)?;
        resolve_provider_arch(&req, probe.arch.as_deref())
    } else {
        existing.arch
    };

    if body.token_secret.is_some() {
        let enc = crypto::encrypt(&state.cfg().secret_key, &secret).map_err(AppError::Anyhow)?;
        sqlx::query(
            r#"UPDATE providers SET name=?, url=?, token_id=?, token_secret_enc=?, node=?, storage=?, bridge=?, insecure=?, defaults_json=?, arch=?, updated_at=? WHERE id=?"#,
        )
        .bind(&name)
        .bind(&url)
        .bind(&token_id)
        .bind(&enc)
        .bind(&node)
        .bind(&storage)
        .bind(&bridge)
        .bind(insecure)
        .bind(&defaults)
        .bind(&arch)
        .bind(&now)
        .bind(&id)
        .execute(state.pool())
        .await?;
    } else {
        sqlx::query(
            r#"UPDATE providers SET name=?, url=?, token_id=?, node=?, storage=?, bridge=?, insecure=?, defaults_json=?, arch=?, updated_at=? WHERE id=?"#,
        )
        .bind(&name)
        .bind(&url)
        .bind(&token_id)
        .bind(&node)
        .bind(&storage)
        .bind(&bridge)
        .bind(insecure)
        .bind(&defaults)
        .bind(&arch)
        .bind(&now)
        .bind(&id)
        .execute(state.pool())
        .await?;
    }
    audit(
        state.pool(),
        Some(&user.id),
        "provider.update",
        Some(&id),
        None,
    )
    .await;
    get_one(State(state), CurrentUser(user), Path(id)).await
}

async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    let res = sqlx::query("DELETE FROM providers WHERE id = ?")
        .bind(&id)
        .execute(state.pool())
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    audit(
        state.pool(),
        Some(&user.id),
        "provider.delete",
        Some(&id),
        None,
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

struct LoadedProvider {
    kind: String,
    url: String,
    token_id: String,
    secret: String,
    node: String,
    storage: String,
    bridge: String,
    insecure: bool,
}

async fn load_provider(state: &AppState, id: &str) -> ApiResult<LoadedProvider> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, String, String, i64)>(
        "SELECT kind, url, token_id, token_secret_enc, node, storage, bridge, insecure FROM providers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;
    let secret = crypto::decrypt(&state.cfg().secret_key, &row.3).map_err(AppError::Anyhow)?;
    Ok(LoadedProvider {
        kind: row.0,
        url: row.1,
        token_id: row.2,
        secret,
        node: row.4,
        storage: row.5,
        bridge: row.6,
        insecure: row.7 != 0,
    })
}

#[derive(Deserialize)]
struct ProbeIn {
    url: String,
    token_id: String,
    token_secret: String,
    node: String,
    storage: String,
    #[serde(default = "default_bridge")]
    bridge: String,
    #[serde(default)]
    insecure: bool,
    #[serde(default = "default_kind")]
    kind: String,
}

/// Probe an unsaved (or draft) provider: connection + node + storage.
async fn probe(
    State(_state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<ProbeIn>,
) -> ApiResult<Json<ProbeResult>> {
    require_mutate(&user)?;
    if body.token_secret.is_empty() {
        return Err(AppError::bad("token_secret is required to probe"));
    }
    let kind = normalize_kind(&body.kind)?;
    let result = probe_provider(
        &kind,
        &body.url,
        &body.token_id,
        &body.token_secret,
        &body.node,
        &body.storage,
        &body.bridge,
        body.insecure,
    )
    .await?;
    Ok(Json(result))
}

async fn test(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<TestOverrides>,
) -> ApiResult<Json<ProbeResult>> {
    require_mutate(&user)?;
    let p = load_provider(&state, &id).await?;
    let node = body
        .node
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&p.node);
    let storage = body
        .storage
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&p.storage);
    let bridge = body
        .bridge
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&p.bridge);
    let result = probe_provider(
        &p.kind,
        &p.url,
        &p.token_id,
        &p.secret,
        node,
        storage,
        bridge,
        p.insecure,
    )
    .await?;
    Ok(Json(result))
}

#[derive(Deserialize, Default)]
struct TestOverrides {
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    storage: Option<String>,
    #[serde(default)]
    bridge: Option<String>,
}

async fn storage(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<ProxmoxStorage>>> {
    let p = load_provider(&state, &id).await?;
    if p.kind == "vsphere" {
        let client = VsphereClient::new(p.url, p.token_id, p.secret, p.insecure);
        let list = client.list_storage(&p.node).await?;
        Ok(Json(list))
    } else if p.kind == "nutanix" {
        let client = NutanixClient::new(p.url, p.token_id, p.secret, p.insecure);
        let list = client.list_storage(&p.node).await?;
        Ok(Json(list))
    } else {
        let client = ProxmoxClient {
            url: p.url,
            token_id: p.token_id,
            token_secret: p.secret,
            insecure: p.insecure,
        };
        let list = client.list_storage(&p.node).await?;
        Ok(Json(list))
    }
}
