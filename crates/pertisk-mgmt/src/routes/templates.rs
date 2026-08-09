//! Machine-config templates / blueprints (Phase D1).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::audit;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/templates", get(list).post(create))
        .route(
            "/templates/{id}",
            get(get_one).put(update).delete(delete),
        )
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct TemplateOut {
    id: String,
    name: String,
    description: String,
    /// controlplane | worker | any
    role: String,
    yaml: String,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct TemplateIn {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_role")]
    role: String,
    yaml: String,
}

fn default_role() -> String {
    "any".into()
}

fn normalize_role(role: &str) -> ApiResult<String> {
    match role.trim().to_ascii_lowercase().as_str() {
        "controlplane" | "cp" => Ok("controlplane".into()),
        "worker" | "wk" => Ok("worker".into()),
        "any" | "" => Ok("any".into()),
        other => Err(AppError::bad(format!(
            "role must be controlplane|worker|any (got {other})"
        ))),
    }
}

fn validate_yaml(yaml: &str) -> ApiResult<()> {
    let trimmed = yaml.trim();
    if trimmed.is_empty() {
        return Err(AppError::bad("yaml must not be empty"));
    }
    pertisk_config::MachineConfig::from_yaml(trimmed)
        .map_err(|e| AppError::bad(format!("invalid machine config: {e}")))?;
    Ok(())
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<TemplateOut>>> {
    let rows = sqlx::query_as::<_, TemplateOut>(
        "SELECT id, name, description, role, yaml, created_at, updated_at \
         FROM config_templates ORDER BY name",
    )
    .fetch_all(state.pool())
    .await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<TemplateOut>> {
    let row = sqlx::query_as::<_, TemplateOut>(
        "SELECT id, name, description, role, yaml, created_at, updated_at \
         FROM config_templates WHERE id = ?",
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
    Json(body): Json<TemplateIn>,
) -> ApiResult<Json<TemplateOut>> {
    require_mutate(&user)?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::bad("name is required"));
    }
    let role = normalize_role(&body.role)?;
    validate_yaml(&body.yaml)?;
    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    sqlx::query(
        "INSERT INTO config_templates (id, name, description, role, yaml, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(body.description.trim())
    .bind(&role)
    .bind(body.yaml.trim())
    .bind(&now)
    .bind(&now)
    .execute(state.pool())
    .await
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            AppError::Conflict(format!("template name already exists: {name}"))
        } else {
            AppError::from(e)
        }
    })?;

    audit(
        state.pool(),
        Some(&user.id),
        "template.create",
        Some(&id),
        Some(name),
    )
    .await;

    get_one(State(state), CurrentUser(user), Path(id)).await
}

async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<TemplateIn>,
) -> ApiResult<Json<TemplateOut>> {
    require_mutate(&user)?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::bad("name is required"));
    }
    let role = normalize_role(&body.role)?;
    validate_yaml(&body.yaml)?;
    let now = db::now_rfc3339();
    let res = sqlx::query(
        "UPDATE config_templates SET name = ?, description = ?, role = ?, yaml = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(name)
    .bind(body.description.trim())
    .bind(&role)
    .bind(body.yaml.trim())
    .bind(&now)
    .bind(&id)
    .execute(state.pool())
    .await
    .map_err(|e| {
        if e.to_string().contains("UNIQUE") {
            AppError::Conflict(format!("template name already exists: {name}"))
        } else {
            AppError::from(e)
        }
    })?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    audit(
        state.pool(),
        Some(&user.id),
        "template.update",
        Some(&id),
        Some(name),
    )
    .await;
    get_one(State(state), CurrentUser(user), Path(id)).await
}

async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let res = sqlx::query("DELETE FROM config_templates WHERE id = ?")
        .bind(&id)
        .execute(state.pool())
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    audit(
        state.pool(),
        Some(&user.id),
        "template.delete",
        Some(&id),
        None,
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}
