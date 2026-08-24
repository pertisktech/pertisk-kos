//! Cluster-level CPU / memory / disk summaries for the management Dashboard.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::process::Command;
use tokio::time::timeout;

use crate::cluster_availability;
use crate::k8s::{
    kubeconfig_tls_error, kubectl_json, refresh_kubeconfig_from_guest, resolve_cluster_kubeconfig,
};
use crate::state::AppState;

const LIVE_TTL: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize)]
pub struct ClusterResourceSummary {
    pub cluster_id: String,
    pub cluster_name: String,
    pub status: String,
    /// Live reachability: `online` | `offline` | `unknown`.
    pub availability: String,
    pub k8s_version: String,
    pub node_count: i64,
    pub cpu: ResourceMetric,
    pub memory: ResourceMetric,
    pub disk: ResourceMetric,
    /// Soft error for live metrics (capacity may still be present).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceMetric {
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<f64>,
    pub total: Option<f64>,
    pub percent: Option<f64>,
    pub unit: String,
    pub display_used: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_available: Option<String>,
    pub display_total: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ClusterRow {
    id: String,
    name: String,
    status: String,
    k8s_version: String,
    controlplanes: i64,
    vip: Option<String>,
    vip6: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct NodeCap {
    name: String,
    role: String,
    ip: Option<String>,
    cores: Option<i64>,
    memory: Option<i64>,
    disk_gb: Option<i64>,
}

type LiveCache = HashMap<String, (Instant, ClusterResourceSummary)>;

fn live_cache() -> &'static Mutex<LiveCache> {
    static C: OnceLock<Mutex<LiveCache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn inflight() -> &'static Mutex<HashSet<String>> {
    static C: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashSet::new()))
}

fn remember(summary: &ClusterResourceSummary) {
    if let Ok(mut c) = live_cache().lock() {
        c.insert(
            summary.cluster_id.clone(),
            (Instant::now(), summary.clone()),
        );
    }
}

fn cached_live(id: &str, max_age: Option<Duration>) -> Option<ClusterResourceSummary> {
    let c = live_cache().lock().ok()?;
    let (at, s) = c.get(id)?;
    if max_age.is_none_or(|ttl| at.elapsed() <= ttl) {
        Some(s.clone())
    } else {
        None
    }
}

fn spawn_live(state: AppState, cluster: ClusterRow) {
    let id = cluster.id.clone();
    {
        let Ok(mut g) = inflight().lock() else {
            return;
        };
        if !g.insert(id.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        let summary =
            match timeout(Duration::from_secs(5), gather_one(&state, cluster.clone())).await {
                Ok(summary) => summary,
                Err(_) => timeout_summary_with_capacity(&state, &cluster).await,
            };
        remember(&summary);
        if let Ok(mut g) = inflight().lock() {
            g.remove(&id);
        }
    });
}

/// Summaries for every cluster. DB capacity returns immediately; kubectl usage
/// is refreshed in the background and served from cache on the next poll.
pub async fn gather_all(state: &AppState) -> Vec<ClusterResourceSummary> {
    let clusters: Vec<ClusterRow> = match sqlx::query_as(
        "SELECT id, name, status, k8s_version, controlplanes, vip, vip6 \
             FROM clusters ORDER BY created_at DESC",
    )
    .fetch_all(state.pool())
    .await
    {
        Ok(rows) => rows,
        Err(_) => return vec![],
    };

    let futs: Vec<_> = clusters
        .into_iter()
        .map(|c| {
            let state = state.clone();
            async move {
                if let Some(s) = cached_live(&c.id, Some(LIVE_TTL)) {
                    if s.status != c.status {
                        spawn_live(state, c);
                    }
                    return s;
                }
                spawn_live(state.clone(), c.clone());
                if let Some(s) = cached_live(&c.id, None).filter(|s| s.status == c.status) {
                    return s;
                }
                capacity_now(&state, &c).await
            }
        })
        .collect();
    futures::future::join_all(futs).await
}

async fn capacity_now(state: &AppState, cluster: &ClusterRow) -> ClusterResourceSummary {
    let nodes: Vec<NodeCap> = sqlx::query_as(
        "SELECT name, role, ip, cores, memory, disk_gb \
         FROM nodes WHERE cluster_id = ? ORDER BY role, name",
    )
    .bind(&cluster.id)
    .fetch_all(state.pool())
    .await
    .unwrap_or_default();

    let (cpu, memory, disk) = capacity_metrics(&nodes);
    ClusterResourceSummary {
        cluster_id: cluster.id.clone(),
        cluster_name: cluster.name.clone(),
        status: cluster.status.clone(),
        availability: cluster_availability::cached_or(&cluster.id, &cluster.status),
        k8s_version: cluster.k8s_version.clone(),
        node_count: nodes.len() as i64,
        cpu,
        memory,
        disk,
        error: None,
    }
}

/// Like gather_one capacity path, used when the live probe hits the outer deadline.
async fn timeout_summary_with_capacity(
    state: &AppState,
    cluster: &ClusterRow,
) -> ClusterResourceSummary {
    let nodes: Vec<NodeCap> = sqlx::query_as(
        "SELECT name, role, ip, cores, memory, disk_gb \
         FROM nodes WHERE cluster_id = ? ORDER BY role, name",
    )
    .bind(&cluster.id)
    .fetch_all(state.pool())
    .await
    .unwrap_or_default();

    let (cpu, memory, disk) = capacity_metrics(&nodes);
    let err = Some("resource probe timed out (API unreachable?)".to_string());
    ClusterResourceSummary {
        cluster_id: cluster.id.clone(),
        cluster_name: cluster.name.clone(),
        status: cluster.status.clone(),
        availability: if cluster.status == "ready" {
            "offline".into()
        } else {
            "unknown".into()
        },
        k8s_version: cluster.k8s_version.clone(),
        node_count: nodes.len() as i64,
        cpu: with_metric_error(cpu, err.clone()),
        memory: with_metric_error(memory, err.clone()),
        disk: with_metric_error(disk, err.clone()),
        error: err,
    }
}

fn capacity_metrics(nodes: &[NodeCap]) -> (ResourceMetric, ResourceMetric, ResourceMetric) {
    let cap_cores: f64 = nodes.iter().filter_map(|n| n.cores).sum::<i64>() as f64;
    let cap_mem_mib: f64 = nodes.iter().filter_map(|n| n.memory).sum::<i64>() as f64;
    let cap_disk_gib: f64 = nodes.iter().filter_map(|n| n.disk_gb).sum::<i64>() as f64;

    let cpu = ResourceMetric {
        used: None,
        available: None,
        total: if cap_cores > 0.0 {
            Some(cap_cores)
        } else {
            None
        },
        percent: None,
        unit: "cores".into(),
        display_used: None,
        display_available: None,
        display_total: if cap_cores > 0.0 {
            Some(format_cores(cap_cores))
        } else {
            None
        },
        error: None,
    };
    let memory = ResourceMetric {
        used: None,
        available: None,
        total: if cap_mem_mib > 0.0 {
            Some(cap_mem_mib / 1024.0)
        } else {
            None
        },
        percent: None,
        unit: "GiB".into(),
        display_used: None,
        display_available: None,
        display_total: if cap_mem_mib > 0.0 {
            Some(format!("{:.1} GiB", cap_mem_mib / 1024.0))
        } else {
            None
        },
        error: None,
    };
    let disk = ResourceMetric {
        used: None,
        available: None,
        total: if cap_disk_gib > 0.0 {
            Some(cap_disk_gib)
        } else {
            None
        },
        percent: None,
        unit: "GiB".into(),
        display_used: None,
        display_available: None,
        display_total: if cap_disk_gib > 0.0 {
            Some(format!("{cap_disk_gib:.0} GiB"))
        } else {
            None
        },
        error: None,
    };
    (cpu, memory, disk)
}

fn with_metric_error(mut m: ResourceMetric, err: Option<String>) -> ResourceMetric {
    m.error = err;
    m
}

async fn gather_one(state: &AppState, cluster: ClusterRow) -> ClusterResourceSummary {
    let nodes: Vec<NodeCap> = sqlx::query_as(
        "SELECT name, role, ip, cores, memory, disk_gb \
         FROM nodes WHERE cluster_id = ? ORDER BY role, name",
    )
    .bind(&cluster.id)
    .fetch_all(state.pool())
    .await
    .unwrap_or_default();

    let node_count = nodes.len() as i64;
    let (mut cpu, mut memory, mut disk) = capacity_metrics(&nodes);
    let cap_cores = cpu.total.unwrap_or(0.0);
    let cap_mem_mib = memory.total.map(|g| g * 1024.0).unwrap_or(0.0);

    if cluster.status != "ready" {
        let status = cluster.status.clone();
        return ClusterResourceSummary {
            cluster_id: cluster.id,
            cluster_name: cluster.name,
            status: cluster.status,
            availability: "unknown".into(),
            k8s_version: cluster.k8s_version,
            node_count,
            cpu,
            memory,
            disk,
            error: Some(format!("live usage when status is ready (now {status})")),
        };
    }

    let kc = match resolve_cluster_kubeconfig(state, &cluster.id).await {
        Ok((p, _)) => p,
        Err(e) => {
            return ClusterResourceSummary {
                cluster_id: cluster.id,
                cluster_name: cluster.name,
                status: cluster.status,
                availability: "offline".into(),
                k8s_version: cluster.k8s_version,
                node_count,
                cpu,
                memory,
                disk,
                error: Some(e.to_string()),
            };
        }
    };

    // After reboot, kube-vip can lag while apiserver is already up on a CP.
    // Prefer kubeconfig server; on connect failure, fall back to CP node IPs.
    let (server_override, mut soft_err) = resolve_api_server(&kc, &nodes, &cluster).await;

    // Cluster VMs powered off / VIP dead — skip kubectl top & stats (would just timeout).
    if soft_err.is_some() && server_override.is_none() {
        cpu.error = soft_err.clone();
        memory.error = soft_err.clone();
        disk.error = soft_err.clone();
        return ClusterResourceSummary {
            cluster_id: cluster.id,
            cluster_name: cluster.name,
            status: cluster.status,
            availability: "offline".into(),
            k8s_version: cluster.k8s_version,
            node_count,
            cpu,
            memory,
            disk,
            error: soft_err,
        };
    }

    let top_fut = fetch_kubectl_top_all(&kc, server_override.as_deref());
    let disk_fut = fetch_disk_from_stats(&kc, &nodes, server_override.as_deref());
    let (top_res, disk_res) = tokio::join!(top_fut, disk_fut);

    let top_res = match top_res {
        Err(e) if kubeconfig_tls_error(&e) => {
            if let Some(ip) = first_cp_ip(&nodes) {
                if refresh_kubeconfig_from_guest(&state.cfg().pertiskctl, &ip, &kc, &cluster.name)
                    .await
                    .is_ok()
                {
                    fetch_kubectl_top_all(&kc, server_override.as_deref()).await
                } else {
                    Err(format!(
                        "{e} — leftover kubeconfig CA from a previous cluster? Delete leftover admin.conf or recreate."
                    ))
                }
            } else {
                Err(e)
            }
        }
        other => other,
    };

    match top_res {
        Ok(rows) if !rows.is_empty() => {
            let mut used_cores = 0.0_f64;
            let mut used_mem_mib = 0.0_f64;
            let mut cpu_pct_sum = 0.0_f64;
            let mut mem_pct_sum = 0.0_f64;
            let mut n = 0_usize;
            for r in &rows {
                used_cores += r.cpu_cores;
                used_mem_mib += r.memory_mib;
                if let Some(p) = r.cpu_percent {
                    cpu_pct_sum += p;
                    n += 1;
                }
                if let Some(p) = r.memory_percent {
                    mem_pct_sum += p;
                }
            }
            let n_f = n.max(1) as f64;

            cpu.used = Some(used_cores);
            cpu.display_used = Some(format_cores(used_cores));
            cpu.percent = if cap_cores > 0.0 {
                Some(((used_cores / cap_cores) * 100.0).clamp(0.0, 100.0))
            } else if n > 0 {
                Some((cpu_pct_sum / n_f).clamp(0.0, 100.0))
            } else {
                None
            };

            let used_gib = used_mem_mib / 1024.0;
            memory.used = Some(used_gib);
            memory.display_used = Some(format!("{used_gib:.1} GiB"));
            memory.percent = if cap_mem_mib > 0.0 {
                Some(((used_mem_mib / cap_mem_mib) * 100.0).clamp(0.0, 100.0))
            } else if n > 0 {
                Some((mem_pct_sum / n_f).clamp(0.0, 100.0))
            } else {
                None
            };
        }
        Ok(_) => {
            soft_err = soft_err.or(Some(
                "kubectl top returned no nodes (is metrics-server installed?)".into(),
            ));
            cpu.error = soft_err.clone();
            memory.error = soft_err.clone();
        }
        Err(e) => {
            soft_err = soft_err.or(Some(e));
            cpu.error = soft_err.clone();
            memory.error = soft_err.clone();
        }
    }

    match disk_res {
        Ok(Some((used_b, cap_b))) if cap_b > 0 => {
            let used_gib = used_b as f64 / (1024.0 * 1024.0 * 1024.0);
            let total_gib = cap_b as f64 / (1024.0 * 1024.0 * 1024.0);
            disk.used = Some(used_gib);
            disk.total = Some(total_gib);
            disk.percent = Some(((used_b as f64 / cap_b as f64) * 100.0).clamp(0.0, 100.0));
            disk.display_used = Some(format!("{used_gib:.1} GiB"));
            disk.display_total = Some(format!("{total_gib:.0} GiB"));
            disk.error = None;
        }
        Ok(None) | Ok(Some(_)) => {
            if disk.total.is_none() {
                disk.error = Some("disk capacity unknown".into());
            }
            // Keep provisioned capacity; percent stays unset without live FS stats.
        }
        Err(e) => {
            if disk.total.is_some() {
                disk.error = Some(format!("live disk: {e}"));
            } else {
                disk.error = Some(e);
            }
        }
    }

    ClusterResourceSummary {
        cluster_id: cluster.id,
        cluster_name: cluster.name,
        status: cluster.status,
        availability: "online".into(),
        k8s_version: cluster.k8s_version,
        node_count,
        cpu,
        memory,
        disk,
        error: soft_err,
    }
}

/// Pick a working API server override when the kubeconfig endpoint (often a VIP)
/// is unreachable after reboot / kube-vip election.
async fn resolve_api_server(
    kc: &Path,
    nodes: &[NodeCap],
    cluster: &ClusterRow,
) -> (Option<String>, Option<String>) {
    let cp_servers: Vec<String> = nodes
        .iter()
        .filter(|n| n.role == "controlplane")
        .filter_map(|n| n.ip.as_ref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|ip| format!("https://{ip}:6443"))
        .collect();

    match cluster_availability::first_reachable_server(kc, &cp_servers).await {
        Some(None) => {
            // Kubeconfig default (VIP) worked.
            if cluster.controlplanes <= 1 {
                if let Some(cp) = first_cp_ip(nodes) {
                    if kubeconfig_points_at_vip(kc, cluster) {
                        let _ = rewrite_kubeconfig_server(kc, &format!("https://{cp}:6443"));
                    }
                }
            }
            (None, None)
        }
        Some(Some(server)) => {
            if cluster.controlplanes <= 1 {
                let _ = rewrite_kubeconfig_server(kc, &server);
                (None, None)
            } else {
                // HA: keep kubeconfig on VIP; override for this poll only.
                (Some(server), None)
            }
        }
        None => {
            let tip = cluster
                .vip
                .as_deref()
                .filter(|v| !v.is_empty())
                .or_else(|| cluster.vip6.as_deref().filter(|v| !v.is_empty()))
                .map(|v| format!("VIP {v}"))
                .unwrap_or_else(|| "API endpoint".into());
            (
                None,
                Some(format!(
                    "cannot connect to {tip}; no reachable control plane after reboot?"
                )),
            )
        }
    }
}

fn first_cp_ip(nodes: &[NodeCap]) -> Option<String> {
    nodes
        .iter()
        .filter(|n| n.role == "controlplane")
        .filter_map(|n| n.ip.as_ref())
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
}

fn kubeconfig_points_at_vip(kc: &Path, cluster: &ClusterRow) -> bool {
    let Some(server) = read_kubeconfig_server(kc) else {
        return false;
    };
    let host = server_host(&server);
    for vip in [cluster.vip.as_deref(), cluster.vip6.as_deref()]
        .into_iter()
        .flatten()
    {
        let v = vip.trim();
        if !v.is_empty() && host == v {
            return true;
        }
    }
    false
}

fn read_kubeconfig_server(kc: &Path) -> Option<String> {
    let text = std::fs::read_to_string(kc).ok()?;
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("server:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn server_host(server: &str) -> String {
    let s = server
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    // [ipv6]:port or host:port
    if let Some(rest) = s.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest).to_string();
    }
    s.split(':').next().unwrap_or(s).to_string()
}

fn rewrite_kubeconfig_server(kc: &Path, url: &str) -> std::io::Result<()> {
    let text = std::fs::read_to_string(kc)?;
    let mut out = String::with_capacity(text.len() + 16);
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("server:") {
            let pad = line.len() - t.len();
            out.push_str(&" ".repeat(pad));
            out.push_str("server: ");
            out.push_str(url);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(kc, out)
}

#[derive(Debug)]
struct TopRow {
    cpu_cores: f64,
    cpu_percent: Option<f64>,
    memory_mib: f64,
    memory_percent: Option<f64>,
}

async fn fetch_kubectl_top_all(kc: &Path, server: Option<&str>) -> Result<Vec<TopRow>, String> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kc);
    if let Some(s) = server {
        cmd.arg("--server").arg(s);
    }
    cmd.args(["top", "nodes", "--no-headers"]);

    let out = timeout(Duration::from_secs(3), cmd.output())
        .await
        .map_err(|_| "kubectl top nodes timed out".to_string())?
        .map_err(|e| format!("kubectl: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = stderr.trim();
        return Err(if msg.is_empty() {
            "kubectl top nodes failed (is metrics-server installed?)".into()
        } else {
            msg.to_string()
        });
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(parse_top_line)
        .collect())
}

fn parse_top_line(line: &str) -> Option<TopRow> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    Some(TopRow {
        cpu_cores: parse_cpu_cores(parts[1]),
        cpu_percent: parse_percent(parts[2]),
        memory_mib: parse_memory_mib(parts[3]),
        memory_percent: parse_percent(parts[4]),
    })
}

fn parse_cpu_cores(s: &str) -> f64 {
    let t = s.trim();
    if let Some(m) = t.strip_suffix('m') {
        return m.parse::<f64>().unwrap_or(0.0) / 1000.0;
    }
    t.parse().unwrap_or(0.0)
}

fn parse_memory_mib(s: &str) -> f64 {
    let t = s.trim();
    if let Some(v) = t.strip_suffix("Ki") {
        return v.parse::<f64>().unwrap_or(0.0) / 1024.0;
    }
    if let Some(v) = t.strip_suffix("Mi") {
        return v.parse().unwrap_or(0.0);
    }
    if let Some(v) = t.strip_suffix("Gi") {
        return v.parse::<f64>().unwrap_or(0.0) * 1024.0;
    }
    if let Some(v) = t.strip_suffix('K') {
        return v.parse::<f64>().unwrap_or(0.0) / 1024.0;
    }
    if let Some(v) = t.strip_suffix('M') {
        return v.parse().unwrap_or(0.0);
    }
    if let Some(v) = t.strip_suffix('G') {
        return v.parse::<f64>().unwrap_or(0.0) * 1024.0;
    }
    // bare bytes
    t.parse::<f64>().unwrap_or(0.0) / (1024.0 * 1024.0)
}

fn parse_percent(s: &str) -> Option<f64> {
    s.trim_end_matches('%').parse().ok()
}

fn format_cores(v: f64) -> String {
    if v < 10.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.1}")
    }
}

/// Sum node filesystem usage via kubelet stats summary (admin kubeconfig).
async fn fetch_disk_from_stats(
    kc: &Path,
    nodes: &[NodeCap],
    server: Option<&str>,
) -> Result<Option<(u64, u64)>, String> {
    if nodes.is_empty() {
        return Ok(None);
    }

    let kc_buf = kc.to_path_buf();
    let server = server.map(str::to_string);
    let futs: Vec<_> = nodes
        .iter()
        .map(|n| {
            let name = n.name.clone();
            let kc = kc_buf.clone();
            let server = server.clone();
            async move {
                match timeout(
                    Duration::from_secs(2),
                    fetch_one_node_fs(&kc, &name, server.as_deref()),
                )
                .await
                {
                    Ok(Ok(Some((u, c)))) => Ok(Some((u, c))),
                    Ok(Ok(None)) => Ok(None),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(format!("stats timeout for {name}")),
                }
            }
        })
        .collect();

    let mut used = 0_u64;
    let mut cap = 0_u64;
    let mut any = false;
    let mut last_err: Option<String> = None;
    for res in futures::future::join_all(futs).await {
        match res {
            Ok(Some((u, c))) => {
                used = used.saturating_add(u);
                cap = cap.saturating_add(c);
                any = true;
            }
            Ok(None) => {}
            Err(e) => last_err = Some(e),
        }
    }

    if any {
        return Ok(Some((used, cap)));
    }
    Err(last_err.unwrap_or_else(|| "no filesystem stats".into()))
}

async fn fetch_one_node_fs(
    kc: &Path,
    name: &str,
    server: Option<&str>,
) -> Result<Option<(u64, u64)>, String> {
    let path = format!("/api/v1/nodes/{name}/proxy/stats/summary");
    let doc = kubectl_json_server(kc, server, &["get", "--raw", &path])
        .await
        .map_err(|e| e.to_string())?;
    let used = doc
        .pointer("/node/fs/usedBytes")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            doc.pointer("/node/fs/usedBytes")
                .and_then(|v| v.as_i64())
                .map(|i| i as u64)
        });
    let capacity = doc
        .pointer("/node/fs/capacityBytes")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            doc.pointer("/node/fs/capacityBytes")
                .and_then(|v| v.as_i64())
                .map(|i| i as u64)
        });
    match (used, capacity) {
        (Some(u), Some(c)) if c > 0 => Ok(Some((u, c))),
        _ => Ok(None),
    }
}

async fn kubectl_json_server(
    kubeconfig: &Path,
    server: Option<&str>,
    args: &[&str],
) -> Result<serde_json::Value, String> {
    if server.is_none() {
        return kubectl_json(kubeconfig, args)
            .await
            .map_err(|e| e.to_string());
    }
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kubeconfig);
    if let Some(s) = server {
        cmd.arg("--server").arg(s);
    }
    cmd.args(args);
    let out = cmd.output().await.map_err(|e| format!("kubectl: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = stderr.trim();
        return Err(if msg.is_empty() {
            format!("kubectl {args:?} failed")
        } else {
            msg.to_string()
        });
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).map_err(|e| format!("kubectl json parse: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_line() {
        let r = parse_top_line("lab-cp-1   250m   12%   1024Mi   25%").unwrap();
        assert!((r.cpu_cores - 0.25).abs() < 1e-9);
        assert_eq!(r.cpu_percent, Some(12.0));
        assert!((r.memory_mib - 1024.0).abs() < 1e-9);
        assert_eq!(r.memory_percent, Some(25.0));
    }

    #[test]
    fn parses_memory_units() {
        assert!((parse_memory_mib("2Gi") - 2048.0).abs() < 1e-6);
        assert!((parse_memory_mib("512Mi") - 512.0).abs() < 1e-6);
    }

    #[test]
    fn server_host_ipv4_and_v6() {
        assert_eq!(server_host("https://10.1.1.200:6443"), "10.1.1.200");
        assert_eq!(server_host("https://[fd00:1::200]:6443"), "fd00:1::200");
    }
}
