//! Hypervisor CPU / memory / disk for the provider dashboard.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::time::timeout;

use crate::cluster_resources::ResourceMetric;
use crate::crypto;
use crate::error::{ApiResult, AppError};
use crate::nutanix::NutanixClient;
use crate::pertisk_vms::PertiskVmsClient;
use crate::proxmox::{HypervisorCapacity, ProxmoxClient};
use crate::state::AppState;
use crate::vsphere::VsphereClient;

const LIVE_TTL: Duration = Duration::from_secs(20);
const FETCH_SECS: u64 = 8;

#[derive(Debug, Clone, Serialize)]
pub struct ProviderResourceSummary {
    pub provider_id: String,
    pub provider_name: String,
    pub kind: String,
    pub url: String,
    pub node: String,
    pub storage: String,
    pub availability: String,
    pub cpu: ResourceMetric,
    pub memory: ResourceMetric,
    pub disk: ResourceMetric,
    pub error: Option<String>,
}

type LiveCache = HashMap<String, (Instant, ProviderResourceSummary)>;

fn live_cache() -> &'static Mutex<LiveCache> {
    static C: OnceLock<Mutex<LiveCache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn inflight() -> &'static Mutex<HashSet<String>> {
    static C: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashSet::new()))
}

fn remember(summary: &ProviderResourceSummary) {
    if let Ok(mut c) = live_cache().lock() {
        c.insert(
            summary.provider_id.clone(),
            (Instant::now(), summary.clone()),
        );
    }
}

fn cached(id: &str, max_age: Option<Duration>) -> Option<ProviderResourceSummary> {
    let c = live_cache().lock().ok()?;
    let (at, s) = c.get(id)?;
    if max_age.is_none_or(|ttl| at.elapsed() <= ttl) {
        Some(s.clone())
    } else {
        None
    }
}

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

fn cores_metric(used: Option<f64>, total: Option<f64>) -> ResourceMetric {
    metric(used, None, total, "cores", |v| {
        if v < 10.0 {
            format!("{v:.2}")
        } else {
            format!("{v:.1}")
        }
    })
}

fn bytes_metric(used: Option<f64>, avail: Option<f64>, total: Option<f64>) -> ResourceMetric {
    let to_gib = |v: f64| v / GIB;
    metric(
        used.map(to_gib),
        avail.map(to_gib),
        total.map(to_gib),
        "GiB",
        |v| format!("{v:.1} GiB"),
    )
}

fn metric(
    used: Option<f64>,
    available: Option<f64>,
    total: Option<f64>,
    unit: &str,
    fmt: fn(f64) -> String,
) -> ResourceMetric {
    let available = available.or_else(|| match (used, total) {
        (Some(u), Some(t)) => Some((t - u).max(0.0)),
        _ => None,
    });
    let percent = match (used, total) {
        (Some(u), Some(t)) if t > 0.0 => Some(((u / t) * 100.0).clamp(0.0, 100.0)),
        _ => None,
    };
    ResourceMetric {
        used,
        available,
        total,
        percent,
        unit: unit.into(),
        display_used: used.map(fmt),
        display_available: available.map(fmt),
        display_total: total.map(fmt),
        error: None,
    }
}

fn empty_metrics() -> (ResourceMetric, ResourceMetric, ResourceMetric) {
    (
        cores_metric(None, None),
        bytes_metric(None, None, None),
        bytes_metric(None, None, None),
    )
}

fn from_capacity(cap: &HypervisorCapacity) -> (ResourceMetric, ResourceMetric, ResourceMetric) {
    (
        cores_metric(cap.cpu_used, cap.cpu_total),
        bytes_metric(cap.mem_used_bytes, None, cap.mem_total_bytes),
        bytes_metric(
            cap.disk_used_bytes,
            cap.disk_avail_bytes,
            cap.disk_total_bytes,
        ),
    )
}

fn placeholder(
    id: &str,
    name: &str,
    kind: &str,
    url: &str,
    node: &str,
    storage: &str,
) -> ProviderResourceSummary {
    let (cpu, memory, disk) = empty_metrics();
    ProviderResourceSummary {
        provider_id: id.into(),
        provider_name: name.into(),
        kind: kind.into(),
        url: url.into(),
        node: node.into(),
        storage: storage.into(),
        availability: crate::provider_availability::cached_or(id),
        cpu,
        memory,
        disk,
        error: None,
    }
}

async fn fetch_capacity(
    kind: &str,
    url: &str,
    token_id: &str,
    secret: &str,
    node: &str,
    storage: &str,
    insecure: bool,
) -> Result<HypervisorCapacity, String> {
    match kind {
        "vsphere" => {
            let c = VsphereClient::new(
                url.to_string(),
                token_id.to_string(),
                secret.to_string(),
                insecure,
            );
            c.host_capacity(node, storage)
                .await
                .map_err(|e| e.to_string())
        }
        "nutanix" => {
            let c = NutanixClient::new(
                url.to_string(),
                token_id.to_string(),
                secret.to_string(),
                insecure,
            );
            c.host_capacity(node, storage)
                .await
                .map_err(|e| e.to_string())
        }
        "pertisk-vms" => {
            let c = PertiskVmsClient::new(
                url.to_string(),
                token_id.to_string(),
                secret.to_string(),
                insecure,
            );
            c.host_capacity(node, storage)
                .await
                .map_err(|e| e.to_string())
        }
        _ => {
            let c = ProxmoxClient {
                url: url.to_string(),
                token_id: token_id.to_string(),
                token_secret: secret.to_string(),
                insecure,
            };
            c.host_capacity(node, storage)
                .await
                .map_err(|e| e.to_string())
        }
    }
}

async fn gather_live(state: &AppState, id: &str) -> ProviderResourceSummary {
    let row: Option<(
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
    )> = sqlx::query_as(
        "SELECT id, name, kind, url, token_id, token_secret_enc, node, storage, insecure \
             FROM providers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(state.pool())
    .await
    .ok()
    .flatten();
    let Some((id, name, kind, url, token_id, secret_enc, node, storage, insecure)) = row else {
        let (cpu, memory, disk) = empty_metrics();
        return ProviderResourceSummary {
            provider_id: id.to_string(),
            provider_name: String::new(),
            kind: String::new(),
            url: String::new(),
            node: String::new(),
            storage: String::new(),
            availability: "unknown".into(),
            cpu,
            memory,
            disk,
            error: Some("provider not found".into()),
        };
    };
    let mut summary = placeholder(&id, &name, &kind, &url, &node, &storage);
    let secret = match crypto::decrypt(&state.cfg().secret_key, &secret_enc) {
        Ok(s) => s,
        Err(e) => {
            summary.error = Some(format!("decrypt: {e}"));
            return summary;
        }
    };
    match timeout(
        Duration::from_secs(FETCH_SECS),
        fetch_capacity(
            &kind,
            &url,
            &token_id,
            &secret,
            &node,
            &storage,
            insecure != 0,
        ),
    )
    .await
    {
        Ok(Ok(cap)) => {
            let (cpu, memory, disk) = from_capacity(&cap);
            summary.cpu = cpu;
            summary.memory = memory;
            summary.disk = disk;
            if !cap.node.is_empty() {
                summary.node = cap.node;
            }
            if !cap.storage.is_empty() {
                summary.storage = cap.storage;
            }
            summary.availability = "online".into();
        }
        Ok(Err(e)) => {
            summary.availability = "offline".into();
            summary.error = Some(e);
        }
        Err(_) => {
            summary.availability = crate::provider_availability::cached_or(&id);
            summary.error = Some("timed out reading hypervisor stats".into());
        }
    }
    summary
}

fn spawn_live(state: AppState, id: String) {
    {
        let Ok(mut g) = inflight().lock() else {
            return;
        };
        if !g.insert(id.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        let summary = gather_live(&state, &id).await;
        remember(&summary);
        if let Ok(mut g) = inflight().lock() {
            g.remove(&id);
        }
    });
}

pub async fn gather_all(state: &AppState) -> Vec<ProviderResourceSummary> {
    let rows: Vec<(String, String, String, String, String, String)> = match sqlx::query_as(
        "SELECT id, name, kind, url, node, storage FROM providers ORDER BY name",
    )
    .fetch_all(state.pool())
    .await
    {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let mut out = Vec::with_capacity(rows.len());
    for (id, name, kind, url, node, storage) in rows {
        if let Some(s) = cached(&id, Some(LIVE_TTL)) {
            out.push(s);
            continue;
        }
        spawn_live(state.clone(), id.clone());
        if let Some(s) = cached(&id, None) {
            out.push(s);
        } else {
            out.push(placeholder(&id, &name, &kind, &url, &node, &storage));
        }
    }
    out
}

pub async fn gather_one(state: &AppState, id: &str) -> ApiResult<ProviderResourceSummary> {
    let exists: Option<String> = sqlx::query_scalar("SELECT id FROM providers WHERE id = ?")
        .bind(id)
        .fetch_optional(state.pool())
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    if let Some(s) = cached(id, Some(LIVE_TTL)) {
        spawn_live(state.clone(), id.to_string());
        return Ok(s);
    }
    let summary = gather_live(state, id).await;
    remember(&summary);
    Ok(summary)
}
