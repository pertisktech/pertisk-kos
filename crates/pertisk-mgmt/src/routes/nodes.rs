use axum::extract::{Path, State};
use axum::routing::{get, post, put};
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
        .route("/clusters/{id}/nodes/bulk-delete", axum::routing::post(bulk_delete))
        .route("/clusters/{id}/nodes/bulk-reboot", post(bulk_reboot))
        .route(
            "/clusters/{cid}/nodes/{nid}",
            get(get_one).delete(remove),
        )
        .route("/clusters/{cid}/nodes/{nid}/status", get(status))
        .route("/clusters/{cid}/nodes/{nid}/reboot", post(reboot))
        .route(
            "/clusters/{cid}/nodes/{nid}/hardware",
            put(update_hardware),
        )
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
    pub memory: Option<i64>,
    pub cores: Option<i64>,
    pub disk_gb: Option<i64>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

pub const NODE_SELECT: &str = r#"SELECT id, cluster_id, name, role, vmid, ip, ip6, k8s_version,
       memory, cores, disk_gb, status, created_at, updated_at
       FROM nodes"#;

#[derive(Deserialize)]
struct AddNode {
    /// controlplane | worker
    role: String,
    /// How many nodes to add (default 1).
    #[serde(default = "default_count")]
    count: i64,
    /// Optional hardware overrides applied to each new node (and cluster role defaults).
    #[serde(default)]
    memory: Option<i64>,
    #[serde(default)]
    cores: Option<i64>,
    #[serde(default)]
    disk_gb: Option<i64>,
}

fn default_count() -> i64 {
    1
}

#[derive(Deserialize)]
struct BulkDelete {
    node_ids: Vec<String>,
}

#[derive(Deserialize)]
struct BulkReboot {
    node_ids: Vec<String>,
}

#[derive(Deserialize)]
struct HardwareUpdate {
    #[serde(default)]
    memory: Option<i64>,
    #[serde(default)]
    cores: Option<i64>,
    #[serde(default)]
    disk_gb: Option<i64>,
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<NodeOut>>> {
    let rows = sqlx::query_as::<_, NodeOut>(&format!(
        "{NODE_SELECT} WHERE cluster_id = ? ORDER BY role, name"
    ))
    .bind(&id)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
) -> ApiResult<Json<NodeOut>> {
    let row = sqlx::query_as::<_, NodeOut>(&format!(
        "{NODE_SELECT} WHERE id = ? AND cluster_id = ?"
    ))
    .bind(&nid)
    .bind(&cid)
    .fetch_optional(state.pool())
    .await?;
    let Some(node) = row else {
        return Err(AppError::NotFound);
    };
    Ok(Json(node))
}

async fn status(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
) -> ApiResult<Json<crate::node_status::NodeStatusOut>> {
    let row = sqlx::query_as::<_, NodeOut>(&format!(
        "{NODE_SELECT} WHERE id = ? AND cluster_id = ?"
    ))
    .bind(&nid)
    .bind(&cid)
    .fetch_optional(state.pool())
    .await?;
    let Some(node) = row else {
        return Err(AppError::NotFound);
    };
    let out = crate::node_status::gather(&state, node, &cid).await;
    Ok(Json(out))
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
    if body.count < 1 || body.count > 16 {
        return Err(AppError::bad("count must be 1..=16"));
    }
    if let Some(m) = body.memory {
        if m < 512 {
            return Err(AppError::bad("memory must be >= 512 MB"));
        }
    }
    if let Some(c) = body.cores {
        if c < 1 {
            return Err(AppError::bad("cores must be >= 1"));
        }
    }
    if let Some(d) = body.disk_gb {
        if d < 10 {
            return Err(AppError::bad("disk_gb must be >= 10"));
        }
    }
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM clusters WHERE id = ?")
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
        serde_json::json!({
            "role": body.role,
            "count": body.count,
            "memory": body.memory,
            "cores": body.cores,
            "disk_gb": body.disk_gb,
        }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.add",
        Some(&id),
        Some(&format!("{} x{}", body.role, body.count)),
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
        serde_json::json!({ "node_ids": [nid] }),
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

async fn bulk_delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<BulkDelete>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    if body.node_ids.is_empty() {
        return Err(AppError::bad("node_ids required"));
    }
    if body.node_ids.len() > 32 {
        return Err(AppError::bad("too many nodes (max 32)"));
    }
    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "remove_node",
        serde_json::json!({ "node_ids": body.node_ids }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.bulk_remove",
        Some(&id),
        Some(&format!("{} nodes", body.node_ids.len())),
    )
    .await;
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}

async fn reboot(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let job_id = jobs::enqueue(
        &state,
        Some(&cid),
        "reboot_node",
        serde_json::json!({ "node_id": nid }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.reboot",
        Some(&nid),
        None,
    )
    .await;
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}

async fn bulk_reboot(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<BulkReboot>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    if body.node_ids.is_empty() {
        return Err(AppError::bad("node_ids required"));
    }
    if body.node_ids.len() > 32 {
        return Err(AppError::bad("too many nodes (max 32)"));
    }
    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "reboot_node",
        serde_json::json!({ "node_ids": body.node_ids }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.bulk_reboot",
        Some(&id),
        Some(&format!("{} nodes", body.node_ids.len())),
    )
    .await;
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}

async fn update_hardware(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
    Json(body): Json<HardwareUpdate>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    if body.memory.is_none() && body.cores.is_none() && body.disk_gb.is_none() {
        return Err(AppError::bad("provide memory, cores, and/or disk_gb"));
    }
    if let Some(m) = body.memory {
        if m < 512 {
            return Err(AppError::bad("memory must be >= 512 MB"));
        }
    }
    if let Some(c) = body.cores {
        if c < 1 {
            return Err(AppError::bad("cores must be >= 1"));
        }
    }
    if let Some(d) = body.disk_gb {
        if d < 10 {
            return Err(AppError::bad("disk_gb must be >= 10"));
        }
    }
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT disk_gb FROM nodes WHERE id = ? AND cluster_id = ?")
            .bind(&nid)
            .bind(&cid)
            .fetch_optional(state.pool())
            .await?;
    let Some((cur_disk,)) = row else {
        return Err(AppError::NotFound);
    };
    if let Some(d) = body.disk_gb {
        if let Some(cur) = cur_disk {
            if d < cur {
                return Err(AppError::bad(format!(
                    "disk can only grow (have {cur} GiB, asked {d} GiB)"
                )));
            }
        }
    }
    let job_id = jobs::enqueue(
        &state,
        Some(&cid),
        "resize_node",
        serde_json::json!({
            "node_id": nid,
            "memory": body.memory,
            "cores": body.cores,
            "disk_gb": body.disk_gb,
        }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.hardware",
        Some(&nid),
        None,
    )
    .await;
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}
