//! Cluster-level CPU / memory / disk summaries for the management Dashboard.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;
use tokio::time::timeout;

use crate::k8s::{kubectl_json, resolve_cluster_kubeconfig};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct ClusterResourceSummary {
    pub cluster_id: String,
    pub cluster_name: String,
    pub status: String,
    pub k8s_version: String,
    pub node_count: i64,
    pub cpu: ResourceMetric,
    pub memory: ResourceMetric,
    pub disk: ResourceMetric,
    /// Soft error for live metrics (capacity may still be present).
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResourceMetric {
    pub used: Option<f64>,
    pub total: Option<f64>,
    pub percent: Option<f64>,
    pub unit: String,
    pub display_used: Option<String>,
    pub display_total: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ClusterRow {
    id: String,
    name: String,
    status: String,
    k8s_version: String,
}

#[derive(Debug, sqlx::FromRow)]
struct NodeCap {
    name: String,
    cores: Option<i64>,
    memory: Option<i64>,
    disk_gb: Option<i64>,
}

/// Summaries for every cluster (live metrics when ready + kubeconfig present).
pub async fn gather_all(state: &AppState) -> Vec<ClusterResourceSummary> {
    let clusters: Vec<ClusterRow> =
        match sqlx::query_as(
            "SELECT id, name, status, k8s_version FROM clusters ORDER BY created_at DESC",
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
            async move { gather_one(&state, c).await }
        })
        .collect();
    futures::future::join_all(futs).await
}

async fn gather_one(state: &AppState, cluster: ClusterRow) -> ClusterResourceSummary {
    let nodes: Vec<NodeCap> = sqlx::query_as(
        "SELECT name, cores, memory, disk_gb FROM nodes WHERE cluster_id = ? ORDER BY role, name",
    )
    .bind(&cluster.id)
    .fetch_all(state.pool())
    .await
    .unwrap_or_default();

    let node_count = nodes.len() as i64;
    let cap_cores: f64 = nodes.iter().filter_map(|n| n.cores).sum::<i64>() as f64;
    let cap_mem_mib: f64 = nodes.iter().filter_map(|n| n.memory).sum::<i64>() as f64;
    let cap_disk_gib: f64 = nodes.iter().filter_map(|n| n.disk_gb).sum::<i64>() as f64;

    let mut cpu = ResourceMetric {
        used: None,
        total: if cap_cores > 0.0 { Some(cap_cores) } else { None },
        percent: None,
        unit: "cores".into(),
        display_used: None,
        display_total: if cap_cores > 0.0 {
            Some(format_cores(cap_cores))
        } else {
            None
        },
        error: None,
    };
    let mut memory = ResourceMetric {
        used: None,
        total: if cap_mem_mib > 0.0 {
            Some(cap_mem_mib / 1024.0)
        } else {
            None
        },
        percent: None,
        unit: "GiB".into(),
        display_used: None,
        display_total: if cap_mem_mib > 0.0 {
            Some(format!("{:.1} GiB", cap_mem_mib / 1024.0))
        } else {
            None
        },
        error: None,
    };
    let mut disk = ResourceMetric {
        used: None,
        total: if cap_disk_gib > 0.0 {
            Some(cap_disk_gib)
        } else {
            None
        },
        percent: None,
        unit: "GiB".into(),
        display_used: None,
        display_total: if cap_disk_gib > 0.0 {
            Some(format!("{cap_disk_gib:.0} GiB"))
        } else {
            None
        },
        error: None,
    };

    if cluster.status != "ready" {
        let status = cluster.status.clone();
        return ClusterResourceSummary {
            cluster_id: cluster.id,
            cluster_name: cluster.name,
            status: cluster.status,
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
                k8s_version: cluster.k8s_version,
                node_count,
                cpu,
                memory,
                disk,
                error: Some(e.to_string()),
            };
        }
    };

    let top_fut = fetch_kubectl_top_all(&kc);
    let disk_fut = fetch_disk_from_stats(&kc, &nodes);
    let (top_res, disk_res) = tokio::join!(top_fut, disk_fut);

    let mut soft_err: Option<String> = None;

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
            soft_err = Some("kubectl top returned no nodes (is metrics-server installed?)".into());
            cpu.error = soft_err.clone();
            memory.error = soft_err.clone();
        }
        Err(e) => {
            soft_err = Some(e);
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
        k8s_version: cluster.k8s_version,
        node_count,
        cpu,
        memory,
        disk,
        error: soft_err,
    }
}

#[derive(Debug)]
struct TopRow {
    cpu_cores: f64,
    cpu_percent: Option<f64>,
    memory_mib: f64,
    memory_percent: Option<f64>,
}

async fn fetch_kubectl_top_all(kc: &Path) -> Result<Vec<TopRow>, String> {
    let out = timeout(
        Duration::from_secs(12),
        Command::new("kubectl")
            .args([
                "--kubeconfig",
                &kc.to_string_lossy(),
                "top",
                "nodes",
                "--no-headers",
            ])
            .output(),
    )
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
) -> Result<Option<(u64, u64)>, String> {
    if nodes.is_empty() {
        return Ok(None);
    }
    let mut used = 0_u64;
    let mut cap = 0_u64;
    let mut any = false;
    let mut last_err: Option<String> = None;

    // Cap concurrency — one stats call per node with short timeout.
    for n in nodes {
        match timeout(Duration::from_secs(4), fetch_one_node_fs(kc, &n.name)).await {
            Ok(Ok(Some((u, c)))) => {
                used = used.saturating_add(u);
                cap = cap.saturating_add(c);
                any = true;
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => last_err = Some(e),
            Err(_) => last_err = Some(format!("stats timeout for {}", n.name)),
        }
    }

    if any {
        return Ok(Some((used, cap)));
    }
    Err(last_err.unwrap_or_else(|| "no filesystem stats".into()))
}

async fn fetch_one_node_fs(kc: &Path, name: &str) -> Result<Option<(u64, u64)>, String> {
    let path = format!("/api/v1/nodes/{name}/proxy/stats/summary");
    let doc = kubectl_json(kc, &["get", "--raw", &path])
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
}
