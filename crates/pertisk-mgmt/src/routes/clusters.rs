use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::audit;
use crate::crypto;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::jobs;
use crate::proxmox::ProxmoxClient;
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clusters", get(list).post(create))
        .route("/clusters/check-vmids", axum::routing::post(check_vmids))
        .route(
            "/clusters/{id}",
            get(get_one).delete(delete),
        )
        .route("/clusters/{id}/kubeconfig", get(kubeconfig))
        .route("/clusters/{id}/jobs", get(list_jobs))
        .route("/clusters/{id}/delete-check", get(delete_check))
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
    pub provider_name: Option<String>,
    pub provider_url: Option<String>,
    pub provider_node: Option<String>,
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
    pub network_mode: String,
    pub max_pods: i64,
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
    #[serde(default = "default_net_mode")]
    network_mode: String,
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
    #[serde(default = "default_max_pods")]
    max_pods: i64,
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
fn default_max_pods() -> i64 {
    250
}
fn default_net_mode() -> String {
    "ipv4".into()
}

const CLUSTER_SELECT: &str = r#"
SELECT c.id, c.name, c.provider_id,
       p.name as provider_name, p.url as provider_url, p.node as provider_node,
       c.status, c.controlplanes, c.workers, c.vip, c.vip6, c.cni, c.k8s_version,
       c.cp_memory, c.cp_cores, c.cp_disk_gb, c.worker_memory, c.worker_cores, c.worker_disk_gb,
       c.cp_vmid, c.endpoint, c.error, COALESCE(c.network_mode, 'ipv4') as network_mode,
       COALESCE(c.max_pods, 250) as max_pods,
       c.created_at, c.updated_at
FROM clusters c
LEFT JOIN providers p ON p.id = c.provider_id
"#;

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<ClusterOut>>> {
    let rows = sqlx::query_as::<_, ClusterOut>(&format!(
        "{CLUSTER_SELECT} ORDER BY c.created_at DESC"
    ))
    .fetch_all(state.pool())
    .await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut cluster = sqlx::query_as::<_, ClusterOut>(&format!(
        "{CLUSTER_SELECT} WHERE c.id = ?"
    ))
    .bind(&id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;

    // Self-heal sticky errors: a later successful job (or ready status with a
    // leftover error column) must not keep poisoning the UI banner.
    let latest_job: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT status, error FROM jobs WHERE cluster_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&id)
    .fetch_optional(state.pool())
    .await?;
    let heal = match latest_job.as_ref() {
        Some((st, _)) if st == "succeeded" || st == "running" || st == "queued" => true,
        _ => cluster.status == "ready" && cluster.error.as_ref().is_some_and(|e| !e.is_empty()),
    };
    if heal && (cluster.status == "error" || cluster.error.as_ref().is_some_and(|e| !e.is_empty())) {
        let now = db::now_rfc3339();
        let _ = sqlx::query(
            "UPDATE clusters SET status = 'ready', error = NULL, updated_at = ? WHERE id = ? AND status != 'deleting'",
        )
        .bind(&now)
        .bind(&id)
        .execute(state.pool())
        .await;
        cluster.status = "ready".into();
        cluster.error = None;
    }

    // Refresh node IP / K8s version when ready (missing IPs or active upgrade).
    if cluster.status == "ready" {
        let upgrading: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE cluster_id = ? AND kind = 'upgrade_cluster' AND status = 'running'",
        )
        .bind(&id)
        .fetch_one(state.pool())
        .await
        .unwrap_or(0);
        let missing_ip: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM nodes WHERE cluster_id = ? AND (ip IS NULL OR ip = '')",
        )
        .bind(&id)
        .fetch_one(state.pool())
        .await
        .unwrap_or(0);
        let mode = cluster.network_mode.to_ascii_lowercase();
        let wants_ip6 = mode == "dual-stack" || mode == "ipv6";
        let missing_ip6: i64 = if wants_ip6 {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM nodes WHERE cluster_id = ? AND (ip6 IS NULL OR ip6 = '')",
            )
            .bind(&id)
            .fetch_one(state.pool())
            .await
            .unwrap_or(0)
        } else {
            0
        };
        if upgrading > 0 || missing_ip > 0 || missing_ip6 > 0 {
            let kc: Option<String> = sqlx::query_scalar(
                "SELECT kubeconfig_path FROM clusters WHERE id = ?",
            )
            .bind(&id)
            .fetch_optional(state.pool())
            .await?;
            if let Some(kc) = kc.filter(|s| !s.is_empty()) {
                let log_path: Option<String> = sqlx::query_scalar(
                    "SELECT log_path FROM jobs WHERE cluster_id = ? AND kind IN ('create_cluster', 'upgrade_cluster') ORDER BY updated_at DESC LIMIT 1",
                )
                .bind(&id)
                .fetch_optional(state.pool())
                .await?;
                let _ = crate::node_sync::sync_cluster_nodes(
                    state.pool(),
                    &id,
                    Some(std::path::Path::new(&kc)),
                    log_path.as_deref(),
                )
                .await;
            }
        }
    }

    let nodes = sqlx::query_as::<_, crate::routes::nodes::NodeOut>(&format!(
        "{} WHERE cluster_id = ? ORDER BY role, name",
        crate::routes::nodes::NODE_SELECT
    ))
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
    let mode = body.network_mode.to_ascii_lowercase();
    if !matches!(mode.as_str(), "ipv4" | "ipv6" | "dual-stack") {
        return Err(AppError::bad("network_mode must be ipv4|ipv6|dual-stack"));
    }
    if body.controlplanes < 1 {
        return Err(AppError::bad("controlplanes must be >= 1"));
    }
    if body.controlplanes > 1 {
        let vip = body.vip.as_deref().unwrap_or("").trim();
        let vip6 = body.vip6.as_deref().unwrap_or("").trim();
        if matches!(mode.as_str(), "ipv4" | "dual-stack") && vip.is_empty() {
            return Err(AppError::bad("vip required when controlplanes > 1 (ipv4/dual-stack)"));
        }
        if matches!(mode.as_str(), "ipv6" | "dual-stack") && vip6.is_empty() {
            return Err(AppError::bad("vip6 required when controlplanes > 1 (ipv6/dual-stack)"));
        }
    }
    if body.workers < 0 {
        return Err(AppError::bad("workers must be >= 0"));
    }
    if body.max_pods < 1 || body.max_pods > 1000 {
        return Err(AppError::bad("max_pods must be between 1 and 1000"));
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

    // Reject if any planned VMIDs already exist on the provider node.
    let vm_count = body.controlplanes + body.workers;
    if vm_count > 0 {
        let check = provider_check_vmids(&state, &body.provider_id, body.cp_vmid, vm_count).await?;
        if !check.ok {
            return Err(AppError::bad(check.message));
        }
    }

    let vip = if mode == "ipv6" {
        None
    } else {
        body.vip.clone()
    };
    let vip6 = if mode == "ipv4" {
        None
    } else {
        body.vip6.clone()
    };

    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    sqlx::query(
        r#"INSERT INTO clusters
           (id, name, provider_id, status, controlplanes, workers, vip, vip6, cni, k8s_version,
            cp_memory, cp_cores, cp_disk_gb, worker_memory, worker_cores, worker_disk_gb, cp_vmid,
            network_mode, max_pods, created_at, updated_at)
           VALUES (?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(&body.name)
    .bind(&body.provider_id)
    .bind(body.controlplanes)
    .bind(body.workers)
    .bind(&vip)
    .bind(&vip6)
    .bind(&body.cni)
    .bind(&body.k8s_version)
    .bind(body.cp_memory)
    .bind(body.cp_cores)
    .bind(body.cp_disk_gb)
    .bind(body.worker_memory)
    .bind(body.worker_cores)
    .bind(body.worker_disk_gb)
    .bind(body.cp_vmid)
    .bind(&mode)
    .bind(body.max_pods)
    .bind(&now)
    .bind(&now)
    .execute(state.pool())
    .await?;

    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "create_cluster",
        serde_json::json!({ "cp_vmid": body.cp_vmid, "network_mode": mode }),
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

async fn delete_check(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let row = sqlx::query_as::<_, (String, String, String, Option<i64>, i64, i64)>(
        "SELECT id, name, provider_id, cp_vmid, controlplanes, workers FROM clusters WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;

    let (cid, name, provider_id, cp_vmid, cps, workers) = row;
    let node_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE cluster_id = ?")
            .bind(&cid)
            .fetch_one(state.pool())
            .await
            .unwrap_or(0);

    let provider = sqlx::query_as::<_, (String, String, String, String, String, i64)>(
        "SELECT id, name, url, token_id, node, insecure FROM providers WHERE id = ?",
    )
    .bind(&provider_id)
    .fetch_optional(state.pool())
    .await?;

    let mut provider_json = serde_json::json!({
        "exists": false,
        "id": provider_id,
        "reachable": false,
    });

    if let Some((pid, pname, url, token_id, node, insecure)) = provider {
        let secret_row: Option<String> =
            sqlx::query_scalar("SELECT token_secret_enc FROM providers WHERE id = ?")
                .bind(&pid)
                .fetch_optional(state.pool())
                .await?;
        let mut reachable = false;
        let mut version: Option<String> = None;
        let mut check_error: Option<String> = None;

        if let Some(enc) = secret_row {
            match crate::crypto::decrypt(&state.cfg().secret_key, &enc) {
                Ok(secret) => {
                    let client = crate::proxmox::ProxmoxClient {
                        url: url.clone(),
                        token_id: token_id.clone(),
                        token_secret: secret,
                        insecure: insecure != 0,
                    };
                    match client.test_connection().await {
                        Ok(r) => {
                            reachable = true;
                            version = Some(r.version);
                        }
                        Err(e) => check_error = Some(match &e {
                            AppError::BadRequest(m) | AppError::Conflict(m) => m.clone(),
                            other => other.to_string(),
                        }),
                    }
                }
                Err(e) => check_error = Some(format!("decrypt secret: {e}")),
            }
        }

        provider_json = serde_json::json!({
            "exists": true,
            "id": pid,
            "name": pname,
            "url": url,
            "node": node,
            "insecure": insecure != 0,
            "reachable": reachable,
            "version": version,
            "error": check_error,
        });
    }

    Ok(Json(serde_json::json!({
        "cluster_id": cid,
        "cluster_name": name,
        "cp_vmid": cp_vmid,
        "controlplanes": cps,
        "workers": workers,
        "recorded_nodes": node_count,
        "planned_vms": cps + workers,
        "provider": provider_json,
        "can_delete": true,
        "warning": if !provider_json["exists"].as_bool().unwrap_or(false) {
            Some("Provider is missing — only the DB record will be removed; Proxmox VMs may remain.")
        } else if !provider_json["reachable"].as_bool().unwrap_or(false) {
            Some("Provider is unreachable — delete will still remove the DB record; VM cleanup may fail.")
        } else {
            None
        },
    })))
}

async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, status, provider_id FROM clusters WHERE id = ?")
            .bind(&id)
            .fetch_optional(state.pool())
            .await?;
    let Some((_cid, status, provider_id)) = row else {
        return Err(AppError::NotFound);
    };

    // Require provider to exist for ready clusters (so VM cleanup target is known).
    // Failed/pending may still delete without provider (DB-only).
    let provider_exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM providers WHERE id = ?")
            .bind(&provider_id)
            .fetch_optional(state.pool())
            .await?;
    let immediate = matches!(
        status.as_str(),
        "error" | "pending" | "deleting" | "provisioning"
    );
    if provider_exists.is_none() && !immediate {
        return Err(AppError::bad(
            "provider missing — restore/recreate the Proxmox provider before deleting a ready cluster, or the VMs cannot be cleaned up safely",
        ));
    }

    let now = db::now_rfc3339();
    sqlx::query("UPDATE clusters SET status = 'deleting', updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&id)
        .execute(state.pool())
        .await?;

    // Cancel queued work for this cluster so delete is not blocked.
    let _ = sqlx::query(
        r#"UPDATE jobs SET status = 'cancelled', error = 'superseded by delete', updated_at = ?, finished_at = ?
           WHERE cluster_id = ? AND status IN ('queued')"#,
    )
    .bind(&now)
    .bind(&now)
    .bind(&id)
    .execute(state.pool())
    .await;

    if immediate {
        jobs::force_delete_cluster(&state, &id)
            .await
            .map_err(AppError::Anyhow)?;
        audit(
            state.pool(),
            Some(&user.id),
            "cluster.delete",
            Some(&id),
            Some("force"),
        )
        .await;
        return Ok(Json(serde_json::json!({
            "ok": true,
            "mode": "immediate",
            "provider_id": provider_id,
        })));
    }

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

    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "mode": "async",
        "provider_id": provider_id,
    })))
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
                let content = crate::kubeconfig::rename_kubeconfig_context(&content, &name);
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

#[derive(Deserialize)]
struct CheckVmidsIn {
    provider_id: String,
    #[serde(default = "default_vmid")]
    cp_vmid: i64,
    #[serde(default = "one")]
    controlplanes: i64,
    #[serde(default = "one")]
    workers: i64,
}

async fn check_vmids(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Json(body): Json<CheckVmidsIn>,
) -> ApiResult<Json<crate::proxmox::VmIdCheck>> {
    let count = body.controlplanes + body.workers;
    if count < 1 {
        return Err(AppError::bad("controlplanes + workers must be >= 1"));
    }
    let check = provider_check_vmids(&state, &body.provider_id, body.cp_vmid, count).await?;
    Ok(Json(check))
}

async fn provider_check_vmids(
    state: &AppState,
    provider_id: &str,
    cp_vmid: i64,
    count: i64,
) -> ApiResult<crate::proxmox::VmIdCheck> {
    let row = sqlx::query_as::<_, (String, String, String, String, i64)>(
        "SELECT url, token_id, token_secret_enc, node, insecure FROM providers WHERE id = ?",
    )
    .bind(provider_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or_else(|| AppError::bad("provider not found"))?;
    let secret = crypto::decrypt(&state.cfg().secret_key, &row.2).map_err(AppError::Anyhow)?;
    let client = ProxmoxClient {
        url: row.0,
        token_id: row.1,
        token_secret: secret,
        insecure: row.4 != 0,
    };
    client.check_vmids(&row.3, cp_vmid, count).await
}
