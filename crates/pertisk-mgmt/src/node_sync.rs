//! Sync node IP / K8s version from kubectl or job logs.

use std::net::IpAddr;
use std::path::Path;

use serde_json::Value;
use sqlx::SqlitePool;
use tokio::process::Command;

use crate::db;

#[derive(Debug, Clone, Default)]
pub struct NodeSnapshot {
    pub ip: Option<String>,
    pub ip6: Option<String>,
    pub k8s_version: Option<String>,
    pub os_version: Option<String>,
    pub kernel_version: Option<String>,
    pub container_runtime: Option<String>,
    pub kubelet_status: Option<String>,
}

/// Refresh node rows from kubeconfig (preferred) or job log (fallback).
pub async fn sync_cluster_nodes(
    pool: &SqlitePool,
    cluster_id: &str,
    kubeconfig: Option<&Path>,
    log_path: Option<&str>,
) -> anyhow::Result<usize> {
    let mut snapshots = if let Some(kc) = kubeconfig.filter(|p| p.is_file()) {
        fetch_from_kubectl(kc).await.unwrap_or_default()
    } else {
        Vec::new()
    };

    if snapshots.is_empty() {
        if let Some(log) = log_path {
            snapshots = parse_kubectl_wide_from_log(log);
        }
    }

    if snapshots.is_empty() {
        return Ok(0);
    }

    let mut updated = 0usize;
    for (name, snap) in snapshots {
        let n = persist_snapshot_by_name(pool, cluster_id, &name, &snap).await?;
        if n > 0 {
            updated += 1;
        }
    }
    Ok(updated)
}

pub async fn persist_snapshot_by_name(
    pool: &SqlitePool,
    cluster_id: &str,
    name: &str,
    snap: &NodeSnapshot,
) -> anyhow::Result<u64> {
    let now = db::now_rfc3339();
    let r = sqlx::query(
        r#"UPDATE nodes SET
             ip = COALESCE(?, ip),
             ip6 = COALESCE(?, ip6),
             k8s_version = COALESCE(?, k8s_version),
             os_version = COALESCE(?, os_version),
             kernel_version = COALESCE(?, kernel_version),
             container_runtime = COALESCE(?, container_runtime),
             updated_at = ?
           WHERE cluster_id = ? AND name = ?"#,
    )
    .bind(&snap.ip)
    .bind(&snap.ip6)
    .bind(&snap.k8s_version)
    .bind(&snap.os_version)
    .bind(&snap.kernel_version)
    .bind(&snap.container_runtime)
    .bind(&now)
    .bind(cluster_id)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

pub async fn persist_snapshot_by_id(
    pool: &SqlitePool,
    node_id: &str,
    snap: &NodeSnapshot,
) -> anyhow::Result<u64> {
    let now = db::now_rfc3339();
    let r = sqlx::query(
        r#"UPDATE nodes SET
             ip = COALESCE(?, ip),
             ip6 = COALESCE(?, ip6),
             k8s_version = COALESCE(?, k8s_version),
             os_version = COALESCE(?, os_version),
             kernel_version = COALESCE(?, kernel_version),
             container_runtime = COALESCE(?, container_runtime),
             updated_at = ?
           WHERE id = ?"#,
    )
    .bind(&snap.ip)
    .bind(&snap.ip6)
    .bind(&snap.k8s_version)
    .bind(&snap.os_version)
    .bind(&snap.kernel_version)
    .bind(&snap.container_runtime)
    .bind(&now)
    .bind(node_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

async fn fetch_from_kubectl(kubeconfig: &Path) -> anyhow::Result<Vec<(String, NodeSnapshot)>> {
    let out = Command::new("kubectl")
        .args([
            "--kubeconfig",
            &kubeconfig.to_string_lossy(),
            "get",
            "nodes",
            "-o",
            "json",
        ])
        .output()
        .await?;

    if !out.status.success() {
        anyhow::bail!(
            "kubectl failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let body: Value = serde_json::from_slice(&out.stdout)?;
    let items = body
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("kubectl nodes json missing items"))?;

    let mut rows = Vec::new();
    for item in items {
        let name = item
            .pointer("/metadata/name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }

        let mut snap = NodeSnapshot::default();
        if let Some(addrs) = item.pointer("/status/addresses").and_then(|v| v.as_array()) {
            for addr in addrs {
                let ty = addr.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let ip = addr.get("address").and_then(|v| v.as_str()).unwrap_or("");
                if ty != "InternalIP" || ip.is_empty() {
                    continue;
                }
                if ip.parse::<IpAddr>().is_ok() {
                    if ip.contains(':') {
                        snap.ip6.get_or_insert_with(|| ip.to_string());
                    } else {
                        snap.ip.get_or_insert_with(|| ip.to_string());
                    }
                }
            }
        }

        if let Some(ver) = item
            .pointer("/status/nodeInfo/kubeletVersion")
            .and_then(|v| v.as_str())
        {
            snap.k8s_version = nonempty(ver);
        }
        snap.os_version = item
            .pointer("/status/nodeInfo/osImage")
            .and_then(|v| v.as_str())
            .and_then(parse_os_image);
        snap.kernel_version = item
            .pointer("/status/nodeInfo/kernelVersion")
            .and_then(|v| v.as_str())
            .and_then(nonempty);
        snap.container_runtime = item
            .pointer("/status/nodeInfo/containerRuntimeVersion")
            .and_then(|v| v.as_str())
            .and_then(parse_container_runtime);

        if let Some(cond) = item.pointer("/status/conditions").and_then(|v| v.as_array()) {
            for c in cond {
                if c.get("type").and_then(|v| v.as_str()) == Some("Ready") {
                    let st = c.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    snap.kubelet_status = Some(if st == "True" {
                        "ready".into()
                    } else {
                        "not_ready".into()
                    });
                }
            }
        }

        rows.push((name, snap));
    }
    Ok(rows)
}

/// `pertisk-kos 0.2.87` → `0.2.87`. Bare `pertisk-kos` is ignored.
pub fn parse_os_image(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix("pertisk-kos") {
        let v = rest.trim().trim_start_matches('/').trim();
        if v.is_empty() {
            return None;
        }
        return Some(v.to_string());
    }
    Some(s.to_string())
}

/// `containerd://2.3.4` → `2.3.4`.
pub fn parse_container_runtime(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(v) = s.strip_prefix("containerd://") {
        return nonempty(v);
    }
    Some(s.to_string())
}

fn nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Parse `kubectl get nodes -o wide` table lines from a job log.
pub fn parse_kubectl_wide_from_log(log: &str) -> Vec<(String, NodeSnapshot)> {
    let mut rows = Vec::new();
    let mut in_table = false;

    for line in log.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("NAME ") && trimmed.contains("INTERNAL-IP") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with("==>") || trimmed.starts_with("kubectl ") {
            in_table = false;
            continue;
        }
        if let Some((name, snap)) = parse_wide_line(trimmed) {
            rows.push((name, snap));
        }
    }
    rows
}

fn parse_wide_line(line: &str) -> Option<(String, NodeSnapshot)> {
    // NAME STATUS ROLES AGE VERSION INTERNAL-IP ...
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 6 {
        return None;
    }
    let name = parts[0].to_string();
    if !name.contains('-') {
        return None;
    }
    let status = parts.get(1).map(|s| s.to_ascii_lowercase());
    let version = parts
        .iter()
        .find(|p| p.starts_with('v') && p.contains('.'))
        .map(|s| (*s).to_string());
    let ip = parts
        .iter()
        .find(|p| p.parse::<std::net::Ipv4Addr>().is_ok())
        .map(|s| (*s).to_string());
    let ip6 = parts
        .iter()
        .find(|p| {
            // kubectl wide may show bare IPv6; reject placeholders.
            **p != "<none>" && p.parse::<std::net::Ipv6Addr>().is_ok()
        })
        .map(|s| (*s).to_string());

    Some((
        name,
        NodeSnapshot {
            ip,
            ip6,
            k8s_version: version,
            kubelet_status: status,
            ..Default::default()
        },
    ))
}

/// Wait until kubectl reports the node (and optionally an IPv6 InternalIP).
pub async fn wait_node_addresses(
    kubeconfig: &Path,
    node_name: &str,
    want_ip6: bool,
    timeout: std::time::Duration,
) -> anyhow::Result<NodeSnapshot> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = NodeSnapshot::default();
    loop {
        if let Ok(rows) = fetch_from_kubectl(kubeconfig).await {
            if let Some((_, snap)) = rows.into_iter().find(|(n, _)| n == node_name) {
                last = snap.clone();
                let has_v4 = snap.ip.as_deref().is_some_and(|s| !s.is_empty());
                let has_v6 = snap.ip6.as_deref().is_some_and(|s| !s.is_empty());
                if (has_v4 || has_v6) && (!want_ip6 || has_v6) {
                    return Ok(snap);
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            if last.ip.is_some() || last.ip6.is_some() {
                return Ok(last);
            }
            anyhow::bail!("timed out waiting for addresses on {node_name}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wide_lines() {
        let log = r#"
  kubectl get nodes -o wide
NAME                          STATUS   ROLES           AGE   VERSION   INTERNAL-IP   EXTERNAL-IP
lab-ha-cp-1   Ready    control-plane   75s   v1.36.3   10.1.1.31     <none>
lab-ha-wk-1   Ready    <none>          42s   v1.36.2   10.1.1.134    <none>
"#;
        let rows = parse_kubectl_wide_from_log(log);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "lab-ha-cp-1");
        assert_eq!(rows[0].1.ip.as_deref(), Some("10.1.1.31"));
        assert_eq!(rows[0].1.k8s_version.as_deref(), Some("v1.36.3"));
    }

    #[test]
    fn parses_os_image() {
        assert_eq!(parse_os_image("pertisk-kos 0.2.87").as_deref(), Some("0.2.87"));
        assert_eq!(parse_os_image("pertisk-kos").as_deref(), None);
        assert_eq!(parse_os_image("  ").as_deref(), None);
        assert_eq!(parse_os_image("Alpine Linux v3.20").as_deref(), Some("Alpine Linux v3.20"));
    }

    #[test]
    fn parses_containerd_runtime() {
        assert_eq!(parse_container_runtime("containerd://2.3.4").as_deref(), Some("2.3.4"));
        assert_eq!(parse_container_runtime("cri-o://1.32.0").as_deref(), Some("cri-o://1.32.0"));
        assert_eq!(parse_container_runtime("").as_deref(), None);
    }
}
