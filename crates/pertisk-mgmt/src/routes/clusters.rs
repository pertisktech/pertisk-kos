use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::audit;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::jobs;
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clusters", get(list).post(create))
        .route(
            "/clusters/{id}",
            get(get_one).delete(delete),
        )
        .route("/clusters/{id}/kubeconfig", get(kubeconfig))
        .route("/clusters/{id}/jobs", get(list_jobs))
        .route("/clusters/{id}/upgrade", axum::routing::post(upgrade))
        .route("/clusters/{id}/config", axum::routing::post(update_config))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/log", get(job_log))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ClusterOut {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub status: String,
    pub controlplanes: i64,
    pub workers: i64,
    pub vip: Option<String>,
    pub vip6: Option<String>,
    pub cni: String,
    pub k8s_version: String,
    pub cp_memory: i64,
    pub cp_cores: i64,
    pub cp_disk_gb: i64,
    pub worker_memory: i64,
    pub worker_cores: i64,
    pub worker_disk_gb: i64,
    pub cp_vmid: Option<i64>,
    pub endpoint: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
struct CreateCluster {
    name: String,
    provider_id: String,
    #[serde(default = "one")]
    controlplanes: i64,
    #[serde(default = "one")]
    workers: i64,
    vip: Option<String>,
    vip6: Option<String>,
    #[serde(default = "default_cni")]
    cni: String,
    #[serde(default = "default_k8s")]
    k8s_version: String,
    #[serde(default = "default_cp_mem")]
    cp_memory: i64,
    #[serde(default = "two")]
    cp_cores: i64,
    #[serde(default = "default_cp_disk")]
    cp_disk_gb: i64,
    #[serde(default = "default_wk_mem")]
    worker_memory: i64,
    #[serde(default = "four")]
    worker_cores: i64,
    #[serde(default = "default_wk_disk")]
    worker_disk_gb: i64,
    #[serde(default = "default_vmid")]
    cp_vmid: i64,
}

fn one() -> i64 {
    1
}
fn two() -> i64 {
    2
}
fn four() -> i64 {
    4
}
fn default_cni() -> String {
    "cilium".into()
}
fn default_k8s() -> String {
    "v1.36.3".into()
}
fn default_cp_mem() -> i64 {
    4096
}
fn default_cp_disk() -> i64 {
    50
}
fn default_wk_mem() -> i64 {
    8192
}
fn default_wk_disk() -> i64 {
    75
}
fn default_vmid() -> i64 {
    210
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<ClusterOut>>> {
    let rows = sqlx::query_as::<_, ClusterOut>(
        r#"SELECT id, name, provider_id, status, controlplanes, workers, vip, vip6, cni, k8s_version,
                  cp_memory, cp_cores, cp_disk_gb, worker_memory, worker_cores, worker_disk_gb,
                  cp_vmid, endpoint, error, created_at, updated_at
           FROM clusters ORDER BY created_at DESC"#,
    )
    .fetch_all(state.pool())
    .await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let cluster = sqlx::query_as::<_, ClusterOut>(
        r#"SELECT id, name, provider_id, status, controlplanes, workers, vip, vip6, cni, k8s_version,
                  cp_memory, cp_cores, cp_disk_gb, worker_memory, worker_cores, worker_disk_gb,
                  cp_vmid, endpoint, error, created_at, updated_at
           FROM clusters WHERE id = ?"#,
    )
    .bind(&id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;

    let nodes = sqlx::query_as::<_, crate::routes::nodes::NodeOut>(
        r#"SELECT id, cluster_id, name, role, vmid, ip, status, created_at, updated_at
           FROM nodes WHERE cluster_id = ? ORDER BY role, name"#,
    )
    .bind(&id)
    .fetch_all(state.pool())
    .await?;

    Ok(Json(serde_json::json!({
        "cluster": cluster,
        "nodes": nodes,
    })))
}

async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<CreateCluster>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    if body.controlplanes < 1 {
        return Err(AppError::bad("controlplanes must be >= 1"));
    }
    if body.controlplanes > 1 {
        let vip = body.vip.as_deref().unwrap_or("").trim();
        if vip.is_empty() {
            return Err(AppError::bad("vip required when controlplanes > 1"));
        }
    }
    if body.workers < 0 {
        return Err(AppError::bad("workers must be >= 0"));
    }

    // Ensure provider exists
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM providers WHERE id = ?")
            .bind(&body.provider_id)
            .fetch_optional(state.pool())
            .await?;
    if exists.is_none() {
        return Err(AppError::bad("provider not found"));
    }

    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    sqlx::query(
        r#"INSERT INTO clusters
           (id, name, provider_id, status, controlplanes, workers, vip, vip6, cni, k8s_version,
            cp_memory, cp_cores, cp_disk_gb, worker_memory, worker_cores, worker_disk_gb, cp_vmid,
            created_at, updated_at)
           VALUES (?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(&body.name)
    .bind(&body.provider_id)
    .bind(body.controlplanes)
    .bind(body.workers)
    .bind(&body.vip)
    .bind(&body.vip6)
    .bind(&body.cni)
    .bind(&body.k8s_version)
    .bind(body.cp_memory)
    .bind(body.cp_cores)
    .bind(body.cp_disk_gb)
    .bind(body.worker_memory)
    .bind(body.worker_cores)
    .bind(body.worker_disk_gb)
    .bind(body.cp_vmid)
    .bind(&now)
    .bind(&now)
    .execute(state.pool())
    .await?;

    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "create_cluster",
        serde_json::json!({ "cp_vmid": body.cp_vmid }),
    )
    .await
    .map_err(AppError::Anyhow)?;

    audit(
        state.pool(),
        Some(&user.id),
        "cluster.create",
        Some(&id),
        Some(&body.name),
    )
    .await;

    Ok(Json(serde_json::json!({
        "id": id,
        "job_id": job_id,
        "status": "pending",
    })))
}

async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM clusters WHERE id = ?")
            .bind(&id)
            .fetch_optional(state.pool())
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    let now = db::now_rfc3339();
    sqlx::query("UPDATE clusters SET status = 'deleting', updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&id)
        .execute(state.pool())
        .await?;

    let job_id = jobs::enqueue(&state, Some(&id), "delete_cluster", serde_json::json!({}))
        .await
        .map_err(AppError::Anyhow)?;

    audit(
        state.pool(),
        Some(&user.id),
        "cluster.delete",
        Some(&id),
        None,
    )
    .await;

    Ok(Json(serde_json::json!({ "job_id": job_id })))
}

async fn kubeconfig(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<impl axum::response::IntoResponse> {
    let row = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT kubeconfig_path, name FROM clusters WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;

    let (stored, name) = row;
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(p) = stored {
        candidates.push(std::path::PathBuf::from(&p));
    }
    candidates.push(state.cfg().kubeconfigs_dir().join(&name).join("admin.conf"));
    candidates.push(std::path::PathBuf::from("out/cluster/admin.conf"));
    candidates.push(std::path::PathBuf::from("./out/cluster/admin.conf"));

    let mut last_err = String::from("kubeconfig not available yet");
    for path in &candidates {
        match std::fs::read_to_string(path) {
            Ok(content) if content.contains("apiVersion") || content.contains("clusters:") => {
                // Persist resolved path for next time
                let now = crate::db::now_rfc3339();
                let _ = sqlx::query(
                    "UPDATE clusters SET kubeconfig_path = ?, updated_at = ? WHERE id = ?",
                )
                .bind(path.to_string_lossy().as_ref())
                .bind(&now)
                .bind(&id)
                .execute(state.pool())
                .await;
                return Ok((
                    [
                        (
                            axum::http::header::CONTENT_TYPE,
                            "application/yaml",
                        ),
                        (
                            axum::http::header::CONTENT_DISPOSITION,
                            "attachment; filename=\"admin.conf\"",
                        ),
                    ],
                    content,
                ));
            }
            Ok(_) => {
                last_err = format!("{} exists but is not a valid kubeconfig", path.display());
            }
            Err(e) => {
                last_err = format!("{}: {e}", path.display());
            }
        }
    }
    Err(AppError::bad(format!(
        "kubeconfig not found for cluster {name} ({last_err})"
    )))
}

#[derive(Serialize, sqlx::FromRow)]
struct JobOut {
    id: String,
    cluster_id: Option<String>,
    kind: String,
    status: String,
    error: Option<String>,
    created_at: String,
    updated_at: String,
    finished_at: Option<String>,
}

async fn list_jobs(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<JobOut>>> {
    let rows = sqlx::query_as::<_, JobOut>(
        r#"SELECT id, cluster_id, kind, status, error, created_at, updated_at, finished_at
           FROM jobs WHERE cluster_id = ? ORDER BY created_at DESC LIMIT 50"#,
    )
    .bind(&id)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(rows))
}

async fn get_job(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<JobOut>> {
    let row = sqlx::query_as::<_, JobOut>(
        r#"SELECT id, cluster_id, kind, status, error, created_at, updated_at, finished_at
           FROM jobs WHERE id = ?"#,
    )
    .bind(&id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(row))
}

async fn job_log(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<String> {
    let path: Option<String> = sqlx::query_scalar("SELECT log_path FROM jobs WHERE id = ?")
        .bind(&id)
        .fetch_optional(state.pool())
        .await?
        .flatten();
    let Some(path) = path else {
        return Err(AppError::NotFound);
    };
    Ok(std::fs::read_to_string(&path).unwrap_or_default())
}

#[derive(Deserialize)]
struct UpgradeReq {
    version: String,
}

async fn upgrade(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<UpgradeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "upgrade_cluster",
        serde_json::json!({ "version": body.version }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "cluster.upgrade",
        Some(&id),
        Some(&body.version),
    )
    .await;
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}

#[derive(Deserialize)]
struct ConfigReq {
    config_yaml: String,
    node_id: Option<String>,
}

async fn update_config(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<ConfigReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "update_config",
        serde_json::json!({
            "config_yaml": body.config_yaml,
            "node_id": body.node_id,
        }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "cluster.config",
        Some(&id),
        None,
    )
    .await;
    Ok(Json(serde_json::json!({ "job_id": job_id })))
}
