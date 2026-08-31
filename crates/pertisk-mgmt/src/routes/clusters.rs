use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
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
        .route("/clusters/suggest-vmid", axum::routing::post(suggest_vmid))
        .route("/clusters/check-vip", axum::routing::post(check_vip))
        .route("/clusters/scan-ips", axum::routing::post(scan_ips))
        .route("/clusters/{id}", get(get_one).delete(delete))
        .route("/clusters/{id}/resources", get(resources))
        .route("/clusters/{id}/kubeconfig", get(kubeconfig))
        .route("/clusters/{id}/versions", get(versions))
        .route("/clusters/{id}/config-bundle", get(config_bundle))
        .route("/clusters/{id}/jobs", get(list_jobs))
        .route("/clusters/{id}/delete-check", get(delete_check))
        .route("/clusters/{id}/upgrade", axum::routing::post(upgrade))
        .merge(
            Router::new()
                .route("/clusters/{id}/os-upgrade", axum::routing::post(os_upgrade))
                .layer(DefaultBodyLimit::max(512 * 1024 * 1024)),
        )
        .route(
            "/clusters/{id}/os-upgrade/package",
            axum::routing::post(os_upgrade_package),
        )
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
    /// `proxmox` | `vsphere` | `nutanix` (from providers.kind).
    pub provider_kind: Option<String>,
    pub provider_url: Option<String>,
    pub provider_node: Option<String>,
    pub status: String,
    /// Live reachability: `online` | `offline` | `unknown` (not stored in DB).
    #[sqlx(skip)]
    #[serde(default)]
    pub availability: String,
    /// Live hypervisor API for this cluster's provider.
    #[sqlx(skip)]
    #[serde(default)]
    pub provider_availability: String,
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
    pub arch: String,
    pub pod_subnet: String,
    pub service_subnet: String,
    pub pod_subnet_ipv6: Option<String>,
    pub service_subnet_ipv6: Option<String>,
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
    /// Guest CPU arch for cloud image + Proxmox VM (amd64|arm64).
    /// When omitted, uses the provider's default arch.
    #[serde(default)]
    arch: Option<String>,
    #[serde(default = "default_pod_subnet")]
    pod_subnet: String,
    #[serde(default = "default_service_subnet")]
    service_subnet: String,
    #[serde(default)]
    pod_subnet_ipv6: Option<String>,
    #[serde(default)]
    service_subnet_ipv6: Option<String>,
    /// Copy saved add-on configs (same cluster name, or `addon_preset`) and reinstall after create.
    #[serde(default = "default_true")]
    reuse_addons: bool,
    /// Cluster name to copy add-on configs from. Defaults to this cluster's name.
    #[serde(default)]
    addon_preset: Option<String>,
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
fn default_pod_subnet() -> String {
    "10.244.0.0/16".into()
}
fn default_service_subnet() -> String {
    "10.96.0.0/12".into()
}
fn default_pod_subnet_ipv6() -> String {
    "2001:db8:10:0::/56".into()
}
fn default_service_subnet_ipv6() -> String {
    "2001:db8:96:1::/112".into()
}

fn default_true() -> bool {
    true
}

fn map_cluster_insert_err(e: sqlx::Error, name: &str) -> AppError {
    let msg = e.to_string();
    if msg.contains("UNIQUE") {
        return AppError::Conflict(format!("cluster name already exists: {name}"));
    }
    if msg.contains("FOREIGN KEY") {
        return AppError::bad("provider not found");
    }
    if msg.contains("no such column") {
        tracing::error!(error = %e, "cluster insert schema mismatch");
        return AppError::Anyhow(anyhow::anyhow!(
            "database schema is out of date; restart pertisk-mgmt to migrate"
        ));
    }
    AppError::from(e)
}

const CLUSTER_SELECT: &str = r#"
SELECT c.id, c.name, c.provider_id,
       p.name as provider_name,
       COALESCE(p.kind, 'proxmox') as provider_kind,
       p.url as provider_url, p.node as provider_node,
       c.status, c.controlplanes, c.workers, c.vip, c.vip6, c.cni, c.k8s_version,
       c.cp_memory, c.cp_cores, c.cp_disk_gb, c.worker_memory, c.worker_cores, c.worker_disk_gb,
       c.cp_vmid, c.endpoint, c.error, COALESCE(c.network_mode, 'ipv4') as network_mode,
       COALESCE(c.max_pods, 250) as max_pods,
       COALESCE(c.arch, 'amd64') as arch,
       COALESCE(c.pod_subnet, '10.244.0.0/16') as pod_subnet,
       COALESCE(c.service_subnet, '10.96.0.0/12') as service_subnet,
       c.pod_subnet_ipv6,
       c.service_subnet_ipv6,
       c.created_at, c.updated_at
FROM clusters c
LEFT JOIN providers p ON p.id = c.provider_id
"#;

fn apply_cached_availability(state: &AppState, rows: &mut [ClusterOut]) {
    let mut provider_ids: Vec<String> = Vec::new();
    for c in rows.iter_mut() {
        c.availability = crate::cluster_availability::cached_or(&c.id, &c.status);
        crate::cluster_availability::spawn_refresh(state.clone(), c.id.clone(), c.status.clone());
        c.provider_availability = crate::provider_availability::cached_or(&c.provider_id);
        if !c.provider_id.is_empty() && !provider_ids.iter().any(|x| x == &c.provider_id) {
            provider_ids.push(c.provider_id.clone());
        }
    }
    for id in provider_ids {
        crate::provider_availability::spawn_refresh(state.clone(), id);
    }
}

fn spawn_cluster_node_sync(state: &AppState, cluster_id: &str) {
    let state = state.clone();
    let cluster_id = cluster_id.to_string();
    tokio::spawn(async move {
        let kc: Option<String> =
            sqlx::query_scalar("SELECT kubeconfig_path FROM clusters WHERE id = ?")
                .bind(&cluster_id)
                .fetch_optional(state.pool())
                .await
                .ok()
                .flatten();
        if let Some(kc) = kc.filter(|s| !s.is_empty()) {
            let log_path: Option<String> = sqlx::query_scalar(
                "SELECT log_path FROM jobs WHERE cluster_id = ? AND kind IN ('create_cluster', 'upgrade_cluster', 'upgrade_os') ORDER BY updated_at DESC LIMIT 1",
            )
            .bind(&cluster_id)
            .fetch_optional(state.pool())
            .await
            .ok()
            .flatten();
            let kc_path = std::path::Path::new(&kc);
            let _ = crate::node_sync::sync_cluster_nodes(
                state.pool(),
                &cluster_id,
                Some(kc_path),
                log_path.as_deref(),
            )
            .await;
            let _ = crate::k8s::approve_pending_kubelet_serving_csrs_throttled(kc_path).await;
        }
        let _ = crate::node_sync::sync_os_versions_from_machine_api(
            state.pool(),
            &cluster_id,
            &state.cfg().pertiskctl,
        )
        .await;
    });
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<ClusterOut>>> {
    let mut rows =
        sqlx::query_as::<_, ClusterOut>(&format!("{CLUSTER_SELECT} ORDER BY c.created_at DESC"))
            .fetch_all(state.pool())
            .await?;
    apply_cached_availability(&state, &mut rows);
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut cluster = sqlx::query_as::<_, ClusterOut>(&format!("{CLUSTER_SELECT} WHERE c.id = ?"))
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
    if heal && (cluster.status == "error" || cluster.error.as_ref().is_some_and(|e| !e.is_empty()))
    {
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

    // Node IP / version sync talks to the cluster — do not block the first paint.
    if cluster.status == "ready" {
        spawn_cluster_node_sync(&state, &id);
    }

    let mut nodes = sqlx::query_as::<_, crate::routes::nodes::NodeOut>(&format!(
        "{} WHERE cluster_id = ? ORDER BY role, name",
        crate::routes::nodes::NODE_SELECT
    ))
    .bind(&id)
    .fetch_all(state.pool())
    .await?;

    crate::node_availability::fill(&mut nodes).await;
    crate::node_availability::spawn_rediscover_if_offline(&state, &id, &nodes);
    let _ = crate::cluster_resources::gather_one_cached(&state, &id).await;
    crate::routes::nodes::attach_resource_metrics(&id, &mut nodes);

    cluster.availability = crate::cluster_availability::cached_or(&id, &cluster.status);
    crate::cluster_availability::spawn_refresh(state.clone(), id.clone(), cluster.status.clone());
    cluster.provider_availability = crate::provider_availability::cached_or(&cluster.provider_id);
    crate::provider_availability::spawn_refresh(state.clone(), cluster.provider_id.clone());

    let versions = cluster_versions(state.pool(), &cluster, &nodes).await;

    Ok(Json(serde_json::json!({
        "cluster": cluster,
        "nodes": nodes,
        "versions": versions,
    })))
}

async fn resources(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::cluster_resources::ClusterResourceSummary>> {
    crate::cluster_resources::gather_one_cached(&state, &id)
        .await
        .ok_or(AppError::NotFound)
        .map(Json)
}

async fn versions(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let cluster = sqlx::query_as::<_, ClusterOut>(&format!("{CLUSTER_SELECT} WHERE c.id = ?"))
        .bind(&id)
        .fetch_optional(state.pool())
        .await?
        .ok_or(AppError::NotFound)?;

    let nodes = sqlx::query_as::<_, crate::routes::nodes::NodeOut>(&format!(
        "{} WHERE cluster_id = ? ORDER BY role, name",
        crate::routes::nodes::NODE_SELECT
    ))
    .bind(&id)
    .fetch_all(state.pool())
    .await?;

    Ok(Json(serde_json::json!({
        "versions": cluster_versions(state.pool(), &cluster, &nodes).await,
    })))
}

async fn cluster_versions(
    pool: &sqlx::SqlitePool,
    cluster: &ClusterOut,
    nodes: &[crate::routes::nodes::NodeOut],
) -> Vec<crate::cluster_versions::ComponentVersion> {
    let catalog_os: Option<String> = sqlx::query_scalar(
        r#"SELECT json_extract(payload, '$.version') FROM jobs
           WHERE cluster_id = ? AND kind = 'upgrade_os' AND status = 'succeeded'
             AND json_extract(payload, '$.version') IS NOT NULL
             AND json_extract(payload, '$.version') != ''
             AND json_extract(payload, '$.version') != 'unknown'
           ORDER BY updated_at DESC LIMIT 1"#,
    )
    .bind(&cluster.id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let has_vip = cluster.vip.as_deref().is_some_and(|s| !s.trim().is_empty())
        || cluster
            .vip6
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
    crate::cluster_versions::summarize(
        &crate::cluster_versions::ClusterVersionCtx {
            k8s_version: &cluster.k8s_version,
            cni: &cluster.cni,
            catalog_os: catalog_os.as_deref(),
            has_vip,
        },
        nodes,
    )
}

async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<CreateCluster>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::bad("name is required"));
    }
    if let Err(msg) = validate_cluster_name(name) {
        return Err(AppError::bad(msg));
    }
    let existing: Option<(String, String)> =
        sqlx::query_as("SELECT id, status FROM clusters WHERE name = ?")
            .bind(name)
            .fetch_optional(state.pool())
            .await?;
    if let Some((eid, status)) = existing {
        return Err(AppError::Conflict(format!(
            "cluster name already exists: {name} (id={eid} status={status})"
        )));
    }
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
            return Err(AppError::bad(
                "vip required when controlplanes > 1 (ipv4/dual-stack)",
            ));
        }
        if matches!(mode.as_str(), "ipv6" | "dual-stack") && vip6.is_empty() {
            return Err(AppError::bad(
                "vip6 required when controlplanes > 1 (ipv6/dual-stack)",
            ));
        }
        let vip_check = validate_vips(
            &state,
            if matches!(mode.as_str(), "ipv4" | "dual-stack") {
                Some(vip)
            } else {
                None
            },
            if matches!(mode.as_str(), "ipv6" | "dual-stack") {
                Some(vip6)
            } else {
                None
            },
            None,
        )
        .await?;
        if !vip_check.ok {
            return Err(AppError::bad(vip_check.message));
        }
    }
    if body.workers < 0 {
        return Err(AppError::bad("workers must be >= 0"));
    }
    if body.max_pods < 1 || body.max_pods > 1000 {
        return Err(AppError::bad("max_pods must be between 1 and 1000"));
    }
    let pod_subnet = body.pod_subnet.trim().to_string();
    let service_subnet = body.service_subnet.trim().to_string();
    if pod_subnet.is_empty() {
        return Err(AppError::bad("pod_subnet is required"));
    }
    if service_subnet.is_empty() {
        return Err(AppError::bad("service_subnet is required"));
    }
    if !looks_like_ipv4_cidr(&pod_subnet) {
        return Err(AppError::bad(
            "pod_subnet must be an IPv4 CIDR (e.g. 10.244.0.0/16)",
        ));
    }
    if !looks_like_ipv4_cidr(&service_subnet) {
        return Err(AppError::bad(
            "service_subnet must be an IPv4 CIDR (e.g. 10.96.0.0/12)",
        ));
    }
    let wants_v6 = matches!(mode.as_str(), "dual-stack" | "ipv6");
    let pod_subnet_ipv6 = body
        .pod_subnet_ipv6
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| wants_v6.then(default_pod_subnet_ipv6));
    let service_subnet_ipv6 = body
        .service_subnet_ipv6
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| wants_v6.then(default_service_subnet_ipv6));
    if let Some(ref v6) = pod_subnet_ipv6 {
        if !looks_like_ipv6_cidr(v6) {
            return Err(AppError::bad(
                "pod_subnet_ipv6 must be an IPv6 CIDR (e.g. 2001:db8:10:0::/56)",
            ));
        }
    }
    if let Some(ref v6) = service_subnet_ipv6 {
        if !looks_like_ipv6_cidr(v6) {
            return Err(AppError::bad(
                "service_subnet_ipv6 must be an IPv6 CIDR (e.g. 2001:db8:96:1::/112)",
            ));
        }
    }
    let (pod_subnet_ipv6, service_subnet_ipv6) = if wants_v6 {
        (pod_subnet_ipv6, service_subnet_ipv6)
    } else {
        (None, None)
    };

    // Ensure provider exists (+ default guest arch).
    let provider: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, COALESCE(arch, 'amd64'), kind FROM providers WHERE id = ?")
            .bind(&body.provider_id)
            .fetch_optional(state.pool())
            .await?;
    let Some((_, provider_arch, provider_kind)) = provider else {
        return Err(AppError::bad("provider not found"));
    };
    let arch = match body
        .arch
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(a) => match a.to_ascii_lowercase().as_str() {
            "amd64" | "x86_64" | "x64" => "amd64".to_string(),
            "arm64" | "aarch64" => "arm64".to_string(),
            _ => return Err(AppError::bad("arch must be amd64|arm64")),
        },
        None => match provider_arch.to_ascii_lowercase().as_str() {
            "arm64" | "aarch64" => "arm64".to_string(),
            _ => "amd64".to_string(),
        },
    };
    let kind = provider_kind.to_ascii_lowercase();
    if arch == "arm64" && matches!(kind.as_str(), "vsphere" | "nutanix") {
        return Err(AppError::bad(
            "arm64 guests are supported on Proxmox; vSphere and Nutanix use amd64",
        ));
    }
    if crate::cloud_images::find_for_arch(&state.cfg().images_dir, &arch).is_none() {
        return Err(AppError::bad(crate::cloud_images::missing_message(&arch)));
    }

    // Reject if any planned VMIDs already exist on the provider node.
    let vm_count = body.controlplanes + body.workers;
    if vm_count > 0 {
        let check = provider_check_vmids(&state, &body.provider_id, body.cp_vmid, vm_count).await?;
        if !check.ok {
            return Err(AppError::bad(check.message));
        }
    }

    let vip = if body.controlplanes <= 1 || mode == "ipv6" {
        None
    } else {
        body.vip.clone()
    };
    let vip6 = if body.controlplanes <= 1 || mode == "ipv4" {
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
            network_mode, max_pods, arch, pod_subnet, service_subnet, pod_subnet_ipv6, service_subnet_ipv6,
            created_at, updated_at)
           VALUES (?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(name)
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
    .bind(&arch)
    .bind(&pod_subnet)
    .bind(&service_subnet)
    .bind(&pod_subnet_ipv6)
    .bind(&service_subnet_ipv6)
    .bind(&now)
    .bind(&now)
    .execute(state.pool())
    .await
    .map_err(|e| map_cluster_insert_err(e, name))?;

    if body.reuse_addons {
        let from = body
            .addon_preset
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(name);
        match crate::addons::restore_presets(&state, &id, from, &body.cni).await {
            Ok(restored) if !restored.is_empty() => {
                tracing::info!(
                    cluster = %id,
                    name,
                    from,
                    addons = %restored.join(","),
                    "restored add-on config for recreate"
                );
            }
            Err(e) => tracing::warn!(cluster = %id, error = %e, "failed to restore add-on config"),
            _ => {}
        }
    }

    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "create_cluster",
        serde_json::json!({ "cp_vmid": body.cp_vmid, "network_mode": mode, "arch": arch }),
    )
    .await
    .map_err(AppError::Anyhow)?;

    audit(
        state.pool(),
        Some(&user.id),
        "cluster.create",
        Some(&id),
        Some(name),
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
    let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE cluster_id = ?")
        .bind(&cid)
        .fetch_one(state.pool())
        .await
        .unwrap_or(0);

    let provider = sqlx::query_as::<_, (String, String, String, String, String, String, i64)>(
        "SELECT id, name, kind, url, token_id, node, insecure FROM providers WHERE id = ?",
    )
    .bind(&provider_id)
    .fetch_optional(state.pool())
    .await?;

    let mut provider_json = serde_json::json!({
        "exists": false,
        "id": provider_id,
        "reachable": false,
    });

    if let Some((pid, pname, kind, url, token_id, node, insecure)) = provider {
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
                    let test = if kind == "vsphere" {
                        let client = crate::vsphere::VsphereClient::new(
                            url.clone(),
                            token_id.clone(),
                            secret,
                            insecure != 0,
                        );
                        client.test_connection().await
                    } else if kind == "nutanix" {
                        let client = crate::nutanix::NutanixClient::new(
                            url.clone(),
                            token_id.clone(),
                            secret,
                            insecure != 0,
                        );
                        client.test_connection().await
                    } else {
                        let client = crate::proxmox::ProxmoxClient {
                            url: url.clone(),
                            token_id: token_id.clone(),
                            token_secret: secret,
                            insecure: insecure != 0,
                        };
                        client.test_connection().await
                    };
                    match test {
                        Ok(r) => {
                            reachable = true;
                            version = Some(r.version);
                        }
                        Err(e) => {
                            check_error = Some(match &e {
                                AppError::BadRequest(m) | AppError::Conflict(m) => m.clone(),
                                other => other.to_string(),
                            })
                        }
                    }
                }
                Err(e) => check_error = Some(format!("decrypt secret: {e}")),
            }
        }

        provider_json = serde_json::json!({
            "exists": true,
            "id": pid,
            "name": pname,
            "kind": kind,
            "url": url,
            "token_id": token_id,
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
    state.emit_cluster(&id, "deleting");

    let _ = jobs::cancel_cluster_jobs(&state, &id, None).await;

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
                let filename = kubeconfig_download_name(&name);
                let mut headers = axum::http::HeaderMap::new();
                headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    axum::http::HeaderValue::from_static("application/x-yaml; charset=utf-8"),
                );
                headers.insert(
                    axum::http::header::CONTENT_DISPOSITION,
                    axum::http::HeaderValue::from_str(&format!(
                        "attachment; filename=\"{filename}\""
                    ))
                    .unwrap_or_else(|_| {
                        axum::http::HeaderValue::from_static(
                            "attachment; filename=\"kubeconfig.yaml\"",
                        )
                    }),
                );
                return Ok((headers, content));
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

/// Safe download basename: `{cluster}.yaml` (kubectl / Lens / etc. accept YAML).
fn kubeconfig_download_name(cluster_name: &str) -> String {
    let mut s = sanitize_download_stem(cluster_name);
    if s.is_empty() {
        s = "kubeconfig".into();
    }
    if !s.ends_with(".yaml") && !s.ends_with(".yml") {
        s.push_str(".yaml");
    }
    s
}

/// Safe ZIP basename: `{cluster}-config.zip`.
fn config_bundle_download_name(cluster_name: &str) -> String {
    let mut s = sanitize_download_stem(cluster_name);
    if s.is_empty() {
        s = "cluster".into();
    }
    // Drop a trailing .yaml/.yml if the stem somehow includes it.
    if let Some(stripped) = s.strip_suffix(".yaml").or_else(|| s.strip_suffix(".yml")) {
        s = stripped.trim_end_matches('-').to_string();
        if s.is_empty() {
            s = "cluster".into();
        }
    }
    format!("{s}-config.zip")
}

fn sanitize_download_stem(name: &str) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("..") {
        s = s.replace("..", ".");
    }
    s.trim_matches('.').trim_matches('-').to_string()
}

/// ZIP of `{kubeconfigs_dir}/{name}/` (admin.conf, worker.yaml, role YAMLs).
async fn config_bundle(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<impl axum::response::IntoResponse> {
    let name = sqlx::query_as::<_, (String,)>("SELECT name FROM clusters WHERE id = ?")
        .bind(&id)
        .fetch_optional(state.pool())
        .await?
        .ok_or(AppError::NotFound)?
        .0;

    let dir = state.cfg().kubeconfigs_dir().join(&name);
    if !dir.is_dir() {
        return Err(AppError::bad(format!(
            "cluster config directory not found for {name} ({})",
            dir.display()
        )));
    }

    let entries = std::fs::read_dir(&dir).map_err(|e| {
        AppError::bad(format!(
            "cannot read cluster config dir {}: {e}",
            dir.display()
        ))
    })?;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| AppError::bad(format!("read_dir: {e}")))?;
        let path = entry.path();
        let meta = entry
            .metadata()
            .map_err(|e| AppError::bad(format!("{}: {e}", path.display())))?;
        if !meta.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if file_name.starts_with('.') {
            continue;
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| AppError::bad(format!("read {}: {e}", path.display())))?;
        let bytes = if file_name == "admin.conf" {
            let text = String::from_utf8_lossy(&bytes);
            crate::kubeconfig::rename_kubeconfig_context(&text, &name).into_bytes()
        } else {
            bytes
        };
        files.push((file_name.to_string(), bytes));
    }

    if files.is_empty() {
        return Err(AppError::bad(format!(
            "cluster config directory is empty for {name} ({})",
            dir.display()
        )));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in &files {
            zip.start_file(name, opts)
                .map_err(|e| AppError::bad(format!("zip start_file {name}: {e}")))?;
            use std::io::Write;
            zip.write_all(data)
                .map_err(|e| AppError::bad(format!("zip write {name}: {e}")))?;
        }
        zip.finish()
            .map_err(|e| AppError::bad(format!("zip finish: {e}")))?;
    }
    let body = cursor.into_inner();
    let filename = config_bundle_download_name(&name);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/zip"),
    );
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .unwrap_or_else(|_| {
                axum::http::HeaderValue::from_static("attachment; filename=\"cluster-config.zip\"")
            }),
    );
    Ok((headers, body))
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

async fn os_upgrade(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let exists: Option<String> = sqlx::query_scalar("SELECT id FROM clusters WHERE id = ?")
        .bind(&id)
        .fetch_optional(state.pool())
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }

    let bundle_id = Uuid::new_v4().to_string();
    let dest = state.cfg().os_bundles_dir().join(&id).join(&bundle_id);
    std::fs::create_dir_all(&dest).map_err(anyhow::Error::from)?;

    let mut reboot = true;
    let mut node_ids: Option<Vec<String>> = None;
    let mut zip_bytes: Option<Vec<u8>> = None;
    let mut zip_name = String::new();
    let mut arch_hint: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::bad(format!("multipart: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        let file_name = field.file_name().unwrap_or("").to_string();
        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::bad(format!("read field {name}: {e}")))?;

        match name.as_str() {
            "reboot" => {
                let v = String::from_utf8_lossy(&data).trim().to_ascii_lowercase();
                reboot = v != "0" && v != "false" && v != "no";
            }
            "arch" => {
                let raw = String::from_utf8_lossy(&data);
                if !raw.trim().is_empty() {
                    arch_hint = Some(
                        crate::os_upgrade::normalize_arch(raw.trim())
                            .map_err(|e| AppError::bad(e.to_string()))?,
                    );
                }
            }
            "node_ids" => {
                let raw = String::from_utf8_lossy(&data);
                let parsed: Vec<String> = serde_json::from_str(raw.trim())
                    .map_err(|e| AppError::bad(format!("node_ids: {e}")))?;
                if !parsed.is_empty() {
                    node_ids = Some(parsed);
                }
            }
            "bundle" | "archive" | "zip" => {
                if !file_name.is_empty() {
                    zip_name = file_name.clone();
                }
                zip_bytes = Some(data.to_vec());
            }
            _ => {
                let orig = if file_name.is_empty() {
                    name.clone()
                } else {
                    file_name.clone()
                };
                if orig.to_ascii_lowercase().ends_with(".zip") {
                    zip_name = orig;
                    zip_bytes = Some(data.to_vec());
                } else if crate::os_upgrade::canonical_bundle_name(
                    std::path::Path::new(&orig)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&orig),
                )
                .is_some()
                {
                    crate::os_upgrade::write_bundle_file(&dest, &orig, &data)
                        .map_err(|e| AppError::bad(e.to_string()))?;
                } else if !name.is_empty() && name != "version" {
                    tracing::debug!(field = %name, file = %orig, "ignored os-upgrade field");
                }
            }
        }
    }

    let version = if let Some(bytes) = zip_bytes {
        let zip_path = dest.join("_upload.zip");
        std::fs::write(&zip_path, &bytes).map_err(anyhow::Error::from)?;
        let v = crate::os_upgrade::extract_bundle_zip(&zip_path, &dest)
            .map_err(|e| AppError::bad(e.to_string()))?;
        let _ = std::fs::remove_file(&zip_path);
        v
    } else {
        crate::os_upgrade::validate_bundle_dir(&dest).map_err(|e| AppError::bad(e.to_string()))?
    };

    let cluster_arch: String = sqlx::query_scalar("SELECT arch FROM clusters WHERE id = ?")
        .bind(&id)
        .fetch_one(state.pool())
        .await
        .unwrap_or_else(|_| "amd64".into());
    let arch = arch_hint
        .or_else(|| crate::os_upgrade::infer_arch_from_name(&zip_name))
        .unwrap_or(cluster_arch);
    let arch =
        crate::os_upgrade::normalize_arch(&arch).map_err(|e| AppError::bad(e.to_string()))?;

    let pkg = crate::routes::os_packages::upsert_package(&state, &dest, &version, &arch)
        .await
        .ok();
    let (bundle_dir, package_id) = if let Some(ref pkg) = pkg {
        (pkg.path.clone(), Some(pkg.id.clone()))
    } else {
        (dest.display().to_string(), None)
    };

    let job_id = jobs::enqueue(
        &state,
        Some(&id),
        "upgrade_os",
        serde_json::json!({
            "bundle_dir": bundle_dir,
            "version": version,
            "reboot": reboot,
            "node_ids": node_ids,
            "package_id": package_id,
        }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(&user.id),
        "cluster.os_upgrade",
        Some(&id),
        Some(&version),
    )
    .await;
    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "version": version,
        "package_id": package_id,
    })))
}

#[derive(Deserialize)]
struct OsUpgradePackageReq {
    package_id: String,
    #[serde(default = "os_upgrade_reboot_default")]
    reboot: bool,
    #[serde(default)]
    node_ids: Option<Vec<String>>,
}

fn os_upgrade_reboot_default() -> bool {
    true
}

async fn os_upgrade_package(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<OsUpgradePackageReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let package_id = body.package_id.trim();
    if package_id.is_empty() {
        return Err(AppError::bad("package_id is required"));
    }
    let (job_id, version) = crate::routes::os_packages::enqueue_from_package_id(
        &state,
        &user.id,
        &id,
        package_id,
        body.reboot,
        body.node_ids,
    )
    .await?;
    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "version": version,
        "package_id": package_id,
    })))
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

#[derive(Serialize)]
struct SuggestVmidOut {
    cp_vmid: i64,
    range_start: i64,
    range_end: i64,
    node: String,
    message: String,
    /// True when the suggested range is free on the provider.
    ok: bool,
}

/// Next base VMID: after all mgmt cluster ranges, then bump until free on provider.
async fn suggest_vmid(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Json(body): Json<CheckVmidsIn>,
) -> ApiResult<Json<SuggestVmidOut>> {
    let count = body.controlplanes + body.workers;
    if count < 1 {
        return Err(AppError::bad("controlplanes + workers must be >= 1"));
    }

    // Start after every known cluster's VMID span (all providers — shared LAN).
    let rows = sqlx::query_as::<_, (Option<i64>, i64, i64)>(
        "SELECT cp_vmid, controlplanes, workers FROM clusters WHERE status != 'deleting'",
    )
    .fetch_all(state.pool())
    .await?;

    let mut next = 210i64;
    for (cp_vmid, cps, workers) in rows {
        let base = cp_vmid.unwrap_or(210);
        let span = cps + workers;
        if span < 1 {
            continue;
        }
        let end = base + span - 1;
        if end + 1 > next {
            next = end + 1;
        }
    }
    // Align to 10s for readable lab ranges (210, 220, …).
    if next > 210 {
        next = ((next + 9) / 10) * 10;
    }
    if body.cp_vmid > next {
        // Caller already chose a higher base — still validate / bump from there.
        next = body.cp_vmid;
    }

    let mut last_msg = String::new();
    let mut last_node = String::new();
    for _ in 0..50 {
        let check = provider_check_vmids(&state, &body.provider_id, next, count).await?;
        last_msg = check.message.clone();
        last_node = check.node.clone();
        if check.ok {
            let node = check.node.clone();
            return Ok(Json(SuggestVmidOut {
                cp_vmid: next,
                range_start: check.range_start,
                range_end: check.range_end,
                node: check.node,
                message: format!(
                    "suggested base VMID {next} (range {}–{} free on {node})",
                    check.range_start, check.range_end
                ),
                ok: true,
            }));
        }
        // Skip past the first conflict.
        let bump = check.conflicts.iter().map(|c| c.vmid).max().unwrap_or(next) + 1;
        next = ((bump + 9) / 10) * 10;
        if next < bump {
            next = bump;
        }
    }

    Ok(Json(SuggestVmidOut {
        cp_vmid: next,
        range_start: next,
        range_end: next + count - 1,
        node: last_node,
        message: format!("could not find a free VMID range near {next}: {last_msg}"),
        ok: false,
    }))
}

#[derive(Deserialize)]
struct CheckVipIn {
    #[serde(default)]
    vip: Option<String>,
    #[serde(default)]
    vip6: Option<String>,
    /// Optional cluster id to exclude (re-check while editing an existing cluster).
    #[serde(default)]
    exclude_cluster_id: Option<String>,
}

#[derive(Serialize)]
struct VipConflict {
    address: String,
    reason: String,
    cluster_id: Option<String>,
    cluster_name: Option<String>,
}

#[derive(Serialize)]
struct VipCheck {
    ok: bool,
    message: String,
    conflicts: Vec<VipConflict>,
}

async fn check_vip(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Json(body): Json<CheckVipIn>,
) -> ApiResult<Json<VipCheck>> {
    let vip = body.vip.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let vip6 = body
        .vip6
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if vip.is_none() && vip6.is_none() {
        return Ok(Json(VipCheck {
            ok: true,
            message: "no VIP to check".into(),
            conflicts: vec![],
        }));
    }
    let check = validate_vips(&state, vip, vip6, body.exclude_cluster_id.as_deref()).await?;
    Ok(Json(check))
}

async fn validate_vips(
    state: &AppState,
    vip: Option<&str>,
    vip6: Option<&str>,
    exclude_cluster_id: Option<&str>,
) -> ApiResult<VipCheck> {
    let mut conflicts = Vec::new();

    for addr in [vip, vip6].into_iter().flatten() {
        if addr.parse::<std::net::IpAddr>().is_err() {
            return Ok(VipCheck {
                ok: false,
                message: format!(
                    "VIP not available — {addr}: not a valid IP address (IPv4 octets must be 0–255)"
                ),
                conflicts: vec![VipConflict {
                    address: addr.to_string(),
                    reason: "invalid IP address".into(),
                    cluster_id: None,
                    cluster_name: None,
                }],
            });
        }
        // Another Pertisk cluster already claims this VIP.
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            r#"SELECT id, name, status FROM clusters
               WHERE (vip = ? OR vip6 = ?)
                 AND status NOT IN ('deleting', 'deleted')
                 AND (? IS NULL OR id != ?)"#,
        )
        .bind(addr)
        .bind(addr)
        .bind(exclude_cluster_id)
        .bind(exclude_cluster_id)
        .fetch_all(state.pool())
        .await?;
        for (id, name, status) in rows {
            conflicts.push(VipConflict {
                address: addr.to_string(),
                reason: format!("claimed by cluster {name} ({status})"),
                cluster_id: Some(id),
                cluster_name: Some(name),
            });
        }

        if address_answers_ping(addr).await {
            conflicts.push(VipConflict {
                address: addr.to_string(),
                reason: "answers ICMP ping on the LAN".into(),
                cluster_id: None,
                cluster_name: None,
            });
        } else if address_has_apiserver(addr).await {
            conflicts.push(VipConflict {
                address: addr.to_string(),
                reason: "HTTPS :6443 already responds (apiserver/kube-vip in use)".into(),
                cluster_id: None,
                cluster_name: None,
            });
        }
    }

    let ok = conflicts.is_empty();
    let message = if ok {
        let mut parts = Vec::new();
        if let Some(v) = vip {
            parts.push(format!("{v} free"));
        }
        if let Some(v) = vip6 {
            parts.push(format!("{v} free"));
        }
        format!("VIP OK ({})", parts.join(", "))
    } else {
        let detail = conflicts
            .iter()
            .map(|c| format!("{}: {}", c.address, c.reason))
            .collect::<Vec<_>>()
            .join("; ");
        format!("VIP not available — {detail}")
    };
    Ok(VipCheck {
        ok,
        message,
        conflicts,
    })
}

async fn address_answers_ping(addr: &str) -> bool {
    let is_v6 = addr.contains(':');
    let mut cmd = tokio::process::Command::new("ping");
    cmd.arg("-c").arg("1").arg("-W").arg("1");
    if is_v6 {
        cmd.arg("-6");
    }
    cmd.arg(addr)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.status().await {
        Ok(st) => st.success(),
        Err(_) => false,
    }
}

async fn address_has_apiserver(addr: &str) -> bool {
    let host = if addr.contains(':') {
        format!("[{addr}]")
    } else {
        addr.to_string()
    };
    let url = format!("https://{host}:6443/readyz");
    let client = match reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(2))
        .connect_timeout(std::time::Duration::from_secs(1))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    matches!(client.get(&url).send().await, Ok(resp) if resp.status().is_success())
}

// --- IP scan endpoint ---

#[derive(Debug, Deserialize)]
struct ScanIpsIn {
    provider_id: String,
    #[serde(default = "one")]
    controlplanes: i64,
    #[serde(default = "one")]
    workers: i64,
    /// Optional VIP to exclude from scan results.
    vip: Option<String>,
    /// Optional IPv6 VIP to exclude.
    vip6: Option<String>,
}

#[derive(Debug, Serialize)]
struct IpScanResult {
    ip: String,
    in_use: bool,
    status: String,
}

#[derive(Debug, Serialize)]
struct ScanIpsOut {
    ok: bool,
    subnet: String,
    gateway: String,
    /// All probed gateway IPs (`.1`, `.254`, routing-table hops).
    #[serde(default)]
    gateway_candidates: Vec<String>,
    /// IPs that will be assigned (free IPs found by TCP scan).
    assigned: Vec<String>,
    /// All scanned IPs with their status.
    scanned: Vec<IpScanResult>,
    message: String,
}

/// Scan provider's subnet for available IPs (TCP-based, not ICMP).
async fn scan_ips(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Json(body): Json<ScanIpsIn>,
) -> ApiResult<Json<ScanIpsOut>> {
    use crate::jobs::{
        detect_guest_gateway, ip_from_provider_url, scan_subnet_for_free_ips,
        subnet_from_provider_ip,
    };

    let need_count = body.controlplanes + body.workers;
    if need_count < 1 {
        return Err(AppError::bad("controlplanes + workers must be >= 1"));
    }

    // Get provider URL
    let row = sqlx::query_as::<_, (String,)>("SELECT url FROM providers WHERE id = ?")
        .bind(&body.provider_id)
        .fetch_optional(state.pool())
        .await?
        .ok_or_else(|| AppError::bad("provider not found"))?;

    let provider_ip = ip_from_provider_url(&row.0)
        .ok_or_else(|| AppError::bad("cannot extract IP from provider URL"))?;
    let subnet = subnet_from_provider_ip(&provider_ip)
        .ok_or_else(|| AppError::bad("cannot infer subnet from provider IP"))?;

    // Build exclusion list: all providers + existing nodes + VIPs
    let mut exclude_ips: Vec<String> = Vec::new();

    let urls: Vec<String> = sqlx::query_scalar("SELECT url FROM providers")
        .fetch_all(state.pool())
        .await
        .unwrap_or_default();
    exclude_ips.extend(urls.iter().filter_map(|u| ip_from_provider_url(u)));

    let node_ips: Vec<Option<String>> =
        sqlx::query_scalar("SELECT ip FROM nodes WHERE ip IS NOT NULL")
            .fetch_all(state.pool())
            .await
            .unwrap_or_default();
    exclude_ips.extend(node_ips.into_iter().flatten());

    // Exclude VIPs if provided
    if let Some(ref vip) = body.vip {
        if !vip.trim().is_empty() {
            exclude_ips.push(vip.trim().to_string());
        }
    }
    if let Some(ref vip6) = body.vip6 {
        if !vip6.trim().is_empty() {
            exclude_ips.push(vip6.trim().to_string());
        }
    }

    exclude_ips.sort();
    exclude_ips.dedup();

    // Probe `.1`, `.254`, and routing-table hops; pick the LAN router (not mgmt default).
    let gw_probe = detect_guest_gateway(
        &subnet,
        &[
            "PROXMOX_STATIC_GATEWAY",
            "NUTANIX_STATIC_GATEWAY",
            "VSPHERE_STATIC_GATEWAY",
            "LAB_GATEWAY",
        ],
    )
    .await;
    let gateway = gw_probe.chosen.clone();

    exclude_ips.push(gateway.clone());
    exclude_ips.sort();
    exclude_ips.dedup();

    // Scan for free IPs
    let scan_result = scan_subnet_for_free_ips(&subnet, need_count, &exclude_ips).await;

    match scan_result {
        Ok(free_ips) if !free_ips.is_empty() => {
            let assigned: Vec<String> = free_ips
                .iter()
                .take(need_count as usize)
                .map(|ip| ip.split('/').next().unwrap_or(ip).to_string())
                .collect();

            let scanned: Vec<IpScanResult> = assigned
                .iter()
                .map(|ip| IpScanResult {
                    ip: ip.clone(),
                    in_use: false,
                    status: "free".to_string(),
                })
                .collect();

            let message = format!(
                "Found {} free IPs in {} (gateway {})",
                assigned.len(),
                subnet,
                gw_probe.summary
            );

            Ok(Json(ScanIpsOut {
                ok: true,
                subnet,
                gateway,
                gateway_candidates: gw_probe.candidates.clone(),
                assigned,
                scanned,
                message,
            }))
        }
        _ => Ok(Json(ScanIpsOut {
            ok: false,
            subnet: subnet.clone(),
            gateway,
            gateway_candidates: gw_probe.candidates,
            assigned: vec![],
            scanned: vec![],
            message: format!("No free IPs found in {} (all scanned hosts responded to TCP)", subnet),
        })),
    }
}

async fn provider_check_vmids(
    state: &AppState,
    provider_id: &str,
    cp_vmid: i64,
    count: i64,
) -> ApiResult<crate::proxmox::VmIdCheck> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, i64)>(
        "SELECT kind, url, token_id, token_secret_enc, node, insecure FROM providers WHERE id = ?",
    )
    .bind(provider_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or_else(|| AppError::bad("provider not found"))?;
    let secret = crypto::decrypt(&state.cfg().secret_key, &row.3).map_err(AppError::Anyhow)?;
    if row.0 == "vsphere" {
        let client = crate::vsphere::VsphereClient::new(row.1, row.2, secret, row.5 != 0);
        // Prefix unknown at check time (cluster name not chosen yet). Match bare
        // `{vmid}`, legacy `{prefix}-{vmid}`, and any inventory name ending in `-{vmid}`.
        // Create uses `{cluster}-cp-N` / `{cluster}-wk-N` (same as Proxmox).
        client.check_vmids(&row.4, cp_vmid, count, None).await
    } else if row.0 == "nutanix" {
        let client = crate::nutanix::NutanixClient::new(row.1, row.2, secret, row.5 != 0);
        client.check_vmids(&row.4, cp_vmid, count, None).await
    } else {
        let client = ProxmoxClient {
            url: row.1,
            token_id: row.2,
            token_secret: secret,
            insecure: row.5 != 0,
        };
        client.check_vmids(&row.4, cp_vmid, count).await
    }
}

/// Cluster name is the Proxmox/K8s hostname prefix (`{name}-cp-1`). RFC 1123
/// labels only: letters, digits, hyphen. `lab-ha+orion` is rejected (`+`).
fn validate_cluster_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("name is required".into());
    }
    if n.len() > 50 {
        return Err("name must be at most 50 characters (VM names are {name}-cp-N)".into());
    }
    let ok = n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
        && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && n.chars().last().is_some_and(|c| c.is_ascii_alphanumeric());
    if !ok {
        return Err(
            "name must be a DNS hostname (letters, digits, hyphen). Use lab-ha-orion, not lab-ha+orion"
                .into(),
        );
    }
    Ok(())
}

/// Basic IPv4 CIDR check (e.g. `10.244.0.0/16`) — rejects IPv6 / empty.
fn looks_like_ipv4_cidr(s: &str) -> bool {
    let Some((ip, prefix)) = s.split_once('/') else {
        return false;
    };
    if ip.contains(':') || !ip.contains('.') {
        return false;
    }
    let Ok(pfx) = prefix.parse::<u8>() else {
        return false;
    };
    if pfx > 32 {
        return false;
    }
    let parts: Vec<&str> = ip.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok())
}

/// Basic IPv6 CIDR check (e.g. `2001:db8:10:0::/56`).
fn looks_like_ipv6_cidr(s: &str) -> bool {
    let Some((ip, prefix)) = s.split_once('/') else {
        return false;
    };
    if !ip.contains(':') {
        return false;
    }
    let Ok(pfx) = prefix.parse::<u8>() else {
        return false;
    };
    pfx <= 128
}

#[cfg(test)]
mod tests {
    use super::validate_cluster_name;

    #[test]
    fn cluster_name_allows_hyphenated_dns() {
        assert!(validate_cluster_name("lab-ha").is_ok());
        assert!(validate_cluster_name("lab-ha-orion").is_ok());
        assert!(validate_cluster_name("a").is_ok());
    }

    #[test]
    fn cluster_name_rejects_plus() {
        let err = validate_cluster_name("lab-ha+orion").unwrap_err();
        assert!(err.contains("DNS hostname"), "{err}");
    }

    #[test]
    fn cluster_name_rejects_underscore_and_space() {
        assert!(validate_cluster_name("lab_ha").is_err());
        assert!(validate_cluster_name("lab ha").is_err());
        assert!(validate_cluster_name("-lab").is_err());
        assert!(validate_cluster_name("lab-").is_err());
    }
}
