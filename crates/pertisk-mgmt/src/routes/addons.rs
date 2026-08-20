//! Cluster add-on catalog, config check, and install jobs.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::addons;
use crate::auth::audit;
use crate::error::{ApiResult, AppError};
use crate::jobs;
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clusters/{id}/addons", get(list))
        .route("/clusters/{id}/addons/{name}", get(get_one).post(check))
        .route("/clusters/{id}/addons/{name}/check", post(check))
        .route("/clusters/{id}/addons/{name}/install", post(install))
}

async fn cluster_exists(state: &AppState, id: &str) -> ApiResult<()> {
    let found: Option<String> = sqlx::query_scalar("SELECT id FROM clusters WHERE id = ?")
        .bind(id)
        .fetch_optional(state.pool())
        .await?;
    found.map(|_| ()).ok_or(AppError::NotFound)
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    cluster_exists(&state, &id).await?;
    let data = addons::list_addons(&state, &id).await?;
    Ok(Json(json!({ "data": data })))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, name)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    cluster_exists(&state, &id).await?;
    let addon = addons::parse_addon_id(&name)?;
    let data = addons::summarize_one(&state, &id, &addon, None, true).await?;
    Ok(Json(data))
}

async fn check(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, name)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    cluster_exists(&state, &id).await?;
    let addon = addons::parse_addon_id(&name)?;
    let data = addons::summarize_one(&state, &id, &addon, Some(body), true).await?;
    Ok(Json(data))
}

async fn install(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, name)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    require_mutate(&user)?;
    cluster_exists(&state, &id).await?;
    let addon = addons::parse_addon_id(&name)?;

    let existing: Option<String> = sqlx::query_scalar(
        r#"SELECT id FROM jobs
           WHERE cluster_id = ?
             AND kind = 'install_addon'
             AND status IN ('queued', 'running')
             AND json_extract(payload_json, '$.addon') = ?
           LIMIT 1"#,
    )
    .bind(&id)
    .bind(&addon)
    .fetch_optional(state.pool())
    .await?;
    if let Some(job_id) = existing {
        return Err(AppError::Conflict(format!(
            "addon {addon} install already in progress ({job_id})"
        )));
    }

    let (_, _, public) = addons::upsert_install(&state, &id, &addon, body).await?;
    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "install_addon",
        json!({ "addon": addon }),
    )
    .await
    .map_err(AppError::Anyhow)?;

    audit(
        state.pool(),
        Some(&user.id),
        "cluster.addon.install",
        Some(&id),
        Some(&format!("{addon} {public}")),
    )
    .await;

    Ok(Json(json!({
        "ok": true,
        "job_id": job_id,
        "addon": addon,
        "config": public,
    })))
}
