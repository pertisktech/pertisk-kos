use axum::extract::{Path, Query, State};
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
        .route("/clusters/{id}/nodes/adopt", post(adopt))
        .route(
            "/clusters/{id}/nodes/bulk-delete",
            axum::routing::post(bulk_delete),
        )
        .route("/clusters/{id}/nodes/bulk-reboot", post(bulk_reboot))
        .route("/clusters/{cid}/nodes/{nid}", get(get_one).delete(remove))
        .route("/clusters/{cid}/nodes/{nid}/status", get(status))
        .route("/clusters/{cid}/nodes/{nid}/logs", get(logs))
        .route("/clusters/{cid}/nodes/{nid}/reboot", post(reboot))
        .route(
            "/clusters/{cid}/nodes/{nid}/attestation",
            get(attestation_status),
        )
        .route(
            "/clusters/{cid}/nodes/{nid}/attestation/enroll",
            post(attestation_enroll),
        )
        .route(
            "/clusters/{cid}/nodes/{nid}/attestation/verify",
            post(attestation_verify),
        )
        .route("/clusters/{cid}/nodes/{nid}/hardware", put(update_hardware))
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
    /// Active A/B OS bundle version (`pertiskctl upgrade-status`).
    pub os_version: Option<String>,
    /// Kernel from kubelet `nodeInfo.kernelVersion`.
    pub kernel_version: Option<String>,
    /// containerd (or other CRI) version from `nodeInfo.containerRuntimeVersion`.
    pub container_runtime: Option<String>,
    pub memory: Option<i64>,
    pub cores: Option<i64>,
    pub disk_gb: Option<i64>,
    #[sqlx(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<crate::cluster_resources::ResourceMetric>,
    #[sqlx(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_usage: Option<crate::cluster_resources::ResourceMetric>,
    #[sqlx(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_usage: Option<crate::cluster_resources::ResourceMetric>,
    /// Stored AK public (TPM2B_PUBLIC bytes, base64). Not serialized to API clients.
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    pub ak_public_b64: Option<String>,
    pub ak_enrolled_at: Option<String>,
    /// proxmox | vsphere | nutanix | adopted | baremetal
    pub source: String,
    pub status: String,
    /// Live Machine API reachability: `online` | `offline` | `unknown` (not stored).
    #[sqlx(skip)]
    #[serde(default = "default_availability")]
    pub availability: String,
    pub created_at: String,
    pub updated_at: String,
}

fn default_availability() -> String {
    "unknown".into()
}

pub const NODE_SELECT: &str = r#"SELECT id, cluster_id, name, role, vmid, ip, ip6, k8s_version,
       os_version, kernel_version, container_runtime, memory, cores, disk_gb,
       ak_public_b64, ak_enrolled_at,
       CASE
         WHEN COALESCE(source, '') IN ('adopted', 'baremetal') THEN source
         ELSE COALESCE(
           (SELECT CASE
              WHEN lower(p.kind) IN ('nutanix', 'ahv', 'prism') THEN 'nutanix'
              WHEN lower(p.kind) IN ('vsphere', 'esxi', 'vmware') THEN 'vsphere'
              ELSE 'proxmox'
            END
            FROM clusters c
            JOIN providers p ON p.id = c.provider_id
            WHERE c.id = nodes.cluster_id),
           NULLIF(source, ''),
           'proxmox'
         )
       END AS source, status, created_at, updated_at
       FROM nodes"#;

pub fn attach_resource_metrics(cluster_id: &str, nodes: &mut [NodeOut]) {
    let Some(s) = crate::cluster_resources::cached_summary(cluster_id) else {
        return;
    };
    let by: std::collections::HashMap<_, _> = s.nodes.iter().map(|n| (n.name.clone(), n)).collect();
    for n in nodes {
        if let Some(nr) = by.get(&n.name) {
            n.cpu = Some(nr.cpu.clone());
            n.memory_usage = Some(nr.memory.clone());
            n.disk_usage = Some(nr.disk.clone());
        }
    }
}

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
struct AdoptNode {
    /// controlplane | worker
    role: String,
    /// Existing node Machine API IPv4 (reachable from mgmt).
    ip: String,
    /// Optional hostname; default `{cluster}-wk-N` / `{cluster}-cp-N`.
    #[serde(default)]
    name: Option<String>,
    /// adopted | baremetal (default adopted).
    #[serde(default = "default_adopt_source")]
    source: String,
}

fn default_adopt_source() -> String {
    "adopted".into()
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
    let mut rows = sqlx::query_as::<_, NodeOut>(&format!(
        "{NODE_SELECT} WHERE cluster_id = ? ORDER BY role, name"
    ))
    .bind(&id)
    .fetch_all(state.pool())
    .await?;
    crate::node_availability::fill(&mut rows).await;
    let _ = crate::cluster_resources::gather_one_cached(&state, &id).await;
    attach_resource_metrics(&id, &mut rows);
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
) -> ApiResult<Json<NodeOut>> {
    let row =
        sqlx::query_as::<_, NodeOut>(&format!("{NODE_SELECT} WHERE id = ? AND cluster_id = ?"))
            .bind(&nid)
            .bind(&cid)
            .fetch_optional(state.pool())
            .await?;
    let Some(mut node) = row else {
        return Err(AppError::NotFound);
    };
    node.availability = crate::node_availability::probe_node(&node).await;
    let _ = crate::cluster_resources::gather_one_cached(&state, &cid).await;
    attach_resource_metrics(&cid, std::slice::from_mut(&mut node));
    Ok(Json(node))
}

async fn status(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
) -> ApiResult<Json<crate::node_status::NodeStatusOut>> {
    let row =
        sqlx::query_as::<_, NodeOut>(&format!("{NODE_SELECT} WHERE id = ? AND cluster_id = ?"))
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

#[derive(Debug, Deserialize)]
struct LogsQuery {
    #[serde(default = "default_log_service")]
    service: String,
    #[serde(default = "default_log_tail")]
    tail: u32,
}

fn default_log_service() -> String {
    "pertiskd".into()
}

fn default_log_tail() -> u32 {
    200
}

async fn logs(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Json<crate::node_status::NodeLogsOut>> {
    let row =
        sqlx::query_as::<_, NodeOut>(&format!("{NODE_SELECT} WHERE id = ? AND cluster_id = ?"))
            .bind(&nid)
            .bind(&cid)
            .fetch_optional(state.pool())
            .await?;
    let Some(node) = row else {
        return Err(AppError::NotFound);
    };
    Ok(Json(
        crate::node_status::fetch_logs(&state, &node, &q.service, q.tail).await,
    ))
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

async fn adopt(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<AdoptNode>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    if body.role != "controlplane" && body.role != "worker" {
        return Err(AppError::bad("role must be controlplane or worker"));
    }
    let ip = body.ip.trim();
    if ip.is_empty() {
        return Err(AppError::bad("ip is required"));
    }
    // Basic IPv4 sanity (hostname also ok if it has a label).
    if ip.contains(' ') || ip.contains('/') {
        return Err(AppError::bad("ip must be a host address (no CIDR)"));
    }
    let source = match body.source.trim().to_ascii_lowercase().as_str() {
        "adopted" | "" => "adopted",
        "baremetal" | "bare-metal" | "metal" => "baremetal",
        other => {
            return Err(AppError::bad(format!(
                "source must be adopted|baremetal (got {other})"
            )));
        }
    };
    if let Some(n) = body.name.as_deref() {
        let n = n.trim();
        if n.is_empty() {
            return Err(AppError::bad("name must not be empty when set"));
        }
        if n.len() > 63
            || !n
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        {
            return Err(AppError::bad(
                "name must be a DNS-ish hostname (alnum, -, ., max 63)",
            ));
        }
    }
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM clusters WHERE id = ?")
        .bind(&id)
        .fetch_optional(state.pool())
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    let taken: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM nodes WHERE cluster_id = ? AND ip = ? AND status != 'error'",
    )
    .bind(&id)
    .bind(ip)
    .fetch_optional(state.pool())
    .await?;
    if taken.is_some() {
        return Err(AppError::bad(format!(
            "a node with ip {ip} is already registered on this cluster"
        )));
    }
    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "adopt_node",
        serde_json::json!({
            "role": body.role,
            "ip": ip,
            "name": body.name.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            "source": source,
        }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.adopt",
        Some(&id),
        Some(&format!("{} @ {}", body.role, ip)),
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

async fn load_node_ip(state: &AppState, cid: &str, nid: &str) -> ApiResult<(NodeOut, String)> {
    let row =
        sqlx::query_as::<_, NodeOut>(&format!("{NODE_SELECT} WHERE id = ? AND cluster_id = ?"))
            .bind(nid)
            .bind(cid)
            .fetch_optional(state.pool())
            .await?;
    let Some(node) = row else {
        return Err(AppError::NotFound);
    };
    let ip = node
        .ip
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("node has no IPv4 yet"))?;
    Ok((node, ip))
}

async fn attestation_status(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
) -> ApiResult<Json<crate::node_attestation::AttestationOut>> {
    let exists: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM nodes WHERE id = ? AND cluster_id = ?")
            .bind(&nid)
            .bind(&cid)
            .fetch_optional(state.pool())
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    let out = crate::node_attestation::status(state.pool(), &nid).await?;
    Ok(Json(out))
}

async fn attestation_enroll(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
) -> ApiResult<Json<crate::node_attestation::AttestationOut>> {
    require_mutate(&user)?;
    let (_node, ip) = load_node_ip(&state, &cid, &nid).await?;
    let out = crate::node_attestation::enroll(state.cfg(), state.pool(), &nid, &ip).await?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.attestation.enroll",
        Some(&nid),
        None,
    )
    .await;
    Ok(Json(out))
}

async fn attestation_verify(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((cid, nid)): Path<(String, String)>,
) -> ApiResult<Json<crate::node_attestation::AttestationOut>> {
    require_mutate(&user)?;
    let (_node, ip) = load_node_ip(&state, &cid, &nid).await?;
    let out = crate::node_attestation::verify(state.cfg(), state.pool(), &nid, &ip).await?;
    audit(
        state.pool(),
        Some(&user.id),
        "node.attestation.verify",
        Some(&nid),
        None,
    )
    .await;
    Ok(Json(out))
}
