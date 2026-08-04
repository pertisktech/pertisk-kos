use axum::extract::{Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::audit;
use crate::error::{ApiResult, AppError};
use crate::jobs;
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clusters/{id}/nodes", get(list).post(add))
        .route("/clusters/{cid}/nodes/{nid}", delete(remove))
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct NodeOut {
    pub id: String,
    pub cluster_id: String,
    pub name: String,
    pub role: String,
    pub vmid: Option<i64>,
    pub ip: Option<String>,
    pub ip6: Option<String>,
    pub k8s_version: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
struct AddNode {
    /// controlplane | worker
    role: String,
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<NodeOut>>> {
    let rows = sqlx::query_as::<_, NodeOut>(
        r#"SELECT id, cluster_id, name, role, vmid, ip, ip6, k8s_version, status, created_at, updated_at
           FROM nodes WHERE cluster_id = ? ORDER BY role, name"#,
    )
    .bind(&id)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(rows))
}

async fn add(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<AddNode>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    if body.role != "controlplane" && body.role != "worker" {
        return Err(AppError::bad("role must be controlplane or worker"));
    }
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM clusters WHERE id = ?")
            .bind(&id)
            .fetch_optional(state.pool())
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "add_node",
        serde_json::json!({ "role": body.role }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.add",
        Some(&id),
        Some(&body.role),
    )
    .await;
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}

async fn remove(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let job_id = jobs::enqueue(
        &state,
        Some(&cid),
        "remove_node",
        serde_json::json!({ "node_id": nid }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.remove",
        Some(&nid),
        None,
    )
    .await;
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}
