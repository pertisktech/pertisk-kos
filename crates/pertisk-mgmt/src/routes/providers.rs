use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::audit;
use crate::crypto;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::proxmox::ProxmoxClient;
use crate::rbac::{require_admin, require_mutate};
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/providers", get(list).post(create))
        .route("/providers/{id}", get(get_one).put(update).delete(delete))
        .route("/providers/{id}/test", post(test))
        .route("/providers/{id}/storage", get(storage))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ProviderOut {
    id: String,
    name: String,
    kind: String,
    url: String,
    token_id: String,
    node: String,
    storage: String,
    bridge: String,
    insecure: i64,
    defaults_json: String,
    created_at: String,
    updated_at: String,
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
    #[serde(default = "default_defaults")]
    defaults: serde_json::Value,
}

fn default_bridge() -> String {
    "vmbr0".into()
}

fn default_defaults() -> serde_json::Value {
    serde_json::json!({})
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
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<ProviderOut>>> {
    let rows = sqlx::query_as::<_, ProviderOut>(
        r#"SELECT id, name, kind, url, token_id, node, storage, bridge, insecure, defaults_json, created_at, updated_at
           FROM providers ORDER BY name"#,
    )
    .fetch_all(state.pool())
    .await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<ProviderOut>> {
    let row = sqlx::query_as::<_, ProviderOut>(
        r#"SELECT id, name, kind, url, token_id, node, storage, bridge, insecure, defaults_json, created_at, updated_at
           FROM providers WHERE id = ?"#,
    )
    .bind(&id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(row))
}

async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<ProviderIn>,
) -> ApiResult<Json<ProviderOut>> {
    require_mutate(&user)?;
    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    let enc = crypto::encrypt(&state.cfg().secret_key, &body.token_secret)
        .map_err(AppError::Anyhow)?;
    let defaults = body.defaults.to_string();
    sqlx::query(
        r#"INSERT INTO providers
           (id, name, kind, url, token_id, token_secret_enc, node, storage, bridge, insecure, defaults_json, created_at, updated_at)
           VALUES (?, ?, 'proxmox', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(&body.name)
    .bind(&body.url)
    .bind(&body.token_id)
    .bind(&enc)
    .bind(&body.node)
    .bind(&body.storage)
    .bind(&body.bridge)
    .bind(if body.insecure { 1 } else { 0 })
    .bind(&defaults)
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
    let existing = sqlx::query_as::<_, ProviderOut>(
        r#"SELECT id, name, kind, url, token_id, node, storage, bridge, insecure, defaults_json, created_at, updated_at
           FROM providers WHERE id = ?"#,
    )
    .bind(&id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;

    let name = body.name.unwrap_or(existing.name);
    let url = body.url.unwrap_or(existing.url);
    let token_id = body.token_id.unwrap_or(existing.token_id);
    let node = body.node.unwrap_or(existing.node);
    let storage = body.storage.unwrap_or(existing.storage);
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

    if let Some(secret) = body.token_secret {
        let enc = crypto::encrypt(&state.cfg().secret_key, &secret).map_err(AppError::Anyhow)?;
        sqlx::query(
            r#"UPDATE providers SET name=?, url=?, token_id=?, token_secret_enc=?, node=?, storage=?, bridge=?, insecure=?, defaults_json=?, updated_at=? WHERE id=?"#,
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
        .bind(&now)
        .bind(&id)
        .execute(state.pool())
        .await?;
    } else {
        sqlx::query(
            r#"UPDATE providers SET name=?, url=?, token_id=?, node=?, storage=?, bridge=?, insecure=?, defaults_json=?, updated_at=? WHERE id=?"#,
        )
        .bind(&name)
        .bind(&url)
        .bind(&token_id)
        .bind(&node)
        .bind(&storage)
        .bind(&bridge)
        .bind(insecure)
        .bind(&defaults)
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

async fn load_client(state: &AppState, id: &str) -> ApiResult<(ProxmoxClient, String)> {
    let row = sqlx::query_as::<_, (String, String, String, String, i64, String)>(
        "SELECT url, token_id, token_secret_enc, node, insecure, storage FROM providers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;
    let secret = crypto::decrypt(&state.cfg().secret_key, &row.2).map_err(AppError::Anyhow)?;
    Ok((
        ProxmoxClient {
            url: row.0,
            token_id: row.1,
            token_secret: secret,
            insecure: row.4 != 0,
        },
        row.3,
    ))
}

async fn test(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::proxmox::TestResult>> {
    require_mutate(&user)?;
    let (client, _) = load_client(&state, &id).await?;
    let result = client.test_connection().await?;
    Ok(Json(result))
}

async fn storage(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<crate::proxmox::ProxmoxStorage>>> {
    let (client, node) = load_client(&state, &id).await?;
    let list = client.list_storage(&node).await?;
    Ok(Json(list))
}
