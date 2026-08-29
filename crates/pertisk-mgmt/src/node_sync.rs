//! Sync node IP / K8s version from kubectl or job logs.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use sqlx::SqlitePool;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::timeout;

use crate::crypto;
use crate::db;
use crate::kubeconfig;
use crate::nutanix::NutanixClient;
use crate::proxmox::ProxmoxClient;
use crate::state::AppState;
use crate::vsphere::VsphereClient;

#[derive(Debug, Clone, Default)]
pub struct NodeSnapshot {
    pub ip: Option<String>,
    pub ip6: Option<String>,
    pub k8s_version: Option<String>,
    pub kernel_version: Option<String>,
    pub container_runtime: Option<String>,
    pub kubelet_status: Option<String>,
    pub os_version: Option<String>,
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
             os_version = CASE
               WHEN os_version IS NULL OR os_version = '' OR os_version IN ('0.1.0', 'v0.1.0')
               THEN COALESCE(?, os_version)
               ELSE os_version
             END,
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
             os_version = CASE
               WHEN os_version IS NULL OR os_version = '' OR os_version IN ('0.1.0', 'v0.1.0')
               THEN COALESCE(?, os_version)
               ELSE os_version
             END,
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

/// Cloud-image seed / Cargo workspace default — not the running A/B slot.
#[allow(dead_code)]
pub fn is_placeholder_os_version(v: Option<&str>) -> bool {
    matches!(
        v.map(str::trim),
        None | Some("") | Some("0.1.0") | Some("v0.1.0")
    )
}

/// Guest `pertiskctl version` — running initramfs. Refresh whenever the node answers
/// so the dashboard matches the VM (not a leftover create-time or catalog pin).
pub async fn sync_os_versions_from_machine_api(
    pool: &SqlitePool,
    cluster_id: &str,
    pertiskctl: &Path,
) -> anyhow::Result<usize> {
    if !pertiskctl.is_file() {
        return Ok(0);
    }
    let rows: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, ip, os_version FROM nodes WHERE cluster_id = ?")
            .bind(cluster_id)
            .fetch_all(pool)
            .await?;

    let mut futs = Vec::new();
    for (id, ip, current) in rows {
        let Some(ip) = ip.filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        let bin = pertiskctl.to_path_buf();
        futs.push(async move {
            let ver = fetch_node_os_version(&bin, ip.trim()).await;
            (id, current, ver)
        });
    }

    let results = futures::future::join_all(futs).await;
    let now = db::now_rfc3339();
    let mut updated = 0usize;
    for (id, current, ver) in results {
        let Some(ver) = ver.filter(|v| !v.is_empty()) else {
            continue;
        };
        if current.as_deref() == Some(ver.as_str()) {
            continue;
        }
        let r = sqlx::query("UPDATE nodes SET os_version = ?, updated_at = ? WHERE id = ?")
            .bind(&ver)
            .bind(&now)
            .bind(&id)
            .execute(pool)
            .await?;
        if r.rows_affected() > 0 {
            updated += 1;
        }
    }
    Ok(updated)
}

async fn fetch_node_os_version(pertiskctl: &Path, ip: &str) -> Option<String> {
    let addr = format!("{ip}:50000");
    let out = tokio::time::timeout(
        Duration::from_secs(4),
        Command::new(pertiskctl)
            .args(["-e", &addr, "version"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_pertiskctl_node_version(&stdout)
}

/// `pertiskctl version` prints the CLI version first, then `node <ver> …`.
pub fn parse_pertiskctl_node_version(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("node ") else {
            continue;
        };
        if rest.starts_with("unreachable") {
            continue;
        }
        let ver = rest.split_whitespace().next().unwrap_or("");
        if ver.is_empty() {
            continue;
        }
        return Some(ver.to_string());
    }
    None
}

/// `node 0.2.89 hostname=lab-ha-cp-1 (api v1alpha1 / linux)`
pub fn parse_pertiskctl_hostname(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("node ") else {
            continue;
        };
        for part in rest.split_whitespace() {
            if let Some(h) = part.strip_prefix("hostname=") {
                let host = h.trim_matches(|c| c == '(' || c == ')');
                if !host.is_empty() {
                    return Some(host.to_string());
                }
            }
        }
    }
    None
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
        anyhow::bail!("kubectl failed: {}", String::from_utf8_lossy(&out.stderr));
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

        if let Some(cond) = item
            .pointer("/status/conditions")
            .and_then(|v| v.as_array())
        {
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
            if want_ip6 && last.ip.is_some() && last.ip6.is_none() {
                anyhow::bail!(
                    "{node_name} has IPv4 ({}) but no IPv6 InternalIP after dual-stack wait",
                    last.ip.as_deref().unwrap_or("?")
                );
            }
            if last.ip.is_some() || last.ip6.is_some() {
                return Ok(last);
            }
            anyhow::bail!("timed out waiting for addresses on {node_name}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

const REDISCOVER_TTL: Duration = Duration::from_secs(60);

fn rediscover_at() -> &'static Mutex<HashMap<String, Instant>> {
    static C: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// After Nutanix/vSphere host reboot, guests often get a new DHCP/IPAM address.
/// Refresh `nodes.ip` from the hypervisor (or a :50000 scan) and fix kubeconfig
/// when the cluster endpoint was the old control-plane IP.
pub async fn rediscover_cluster_ips(state: &AppState, cluster_id: &str) -> anyhow::Result<usize> {
    rediscover_cluster_ips_inner(state, cluster_id, false).await
}

/// Same as [`rediscover_cluster_ips`] but ignores the 60s TTL (OS upgrade / reboot).
pub async fn rediscover_cluster_ips_now(
    state: &AppState,
    cluster_id: &str,
) -> anyhow::Result<usize> {
    rediscover_cluster_ips_inner(state, cluster_id, true).await
}

async fn rediscover_cluster_ips_inner(
    state: &AppState,
    cluster_id: &str,
    force: bool,
) -> anyhow::Result<usize> {
    {
        let mut g = rediscover_at().lock().unwrap_or_else(|p| p.into_inner());
        if !force {
            if let Some(at) = g.get(cluster_id) {
                if at.elapsed() < REDISCOVER_TTL {
                    return Ok(0);
                }
            }
        }
        g.insert(cluster_id.to_string(), Instant::now());
    }

    let row = sqlx::query_as::<_, (String, Option<String>, Option<String>, i64)>(
        "SELECT provider_id, kubeconfig_path, vip, controlplanes FROM clusters WHERE id = ?",
    )
    .bind(cluster_id)
    .fetch_optional(state.pool())
    .await?;
    let Some((provider_id, kc_path, vip, controlplanes)) = row else {
        return Ok(0);
    };
    let vip = vip.filter(|s| !s.trim().is_empty());

    let nodes: Vec<(String, String, String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT id, name, role, ip, vmid FROM nodes WHERE cluster_id = ?")
            .bind(cluster_id)
            .fetch_all(state.pool())
            .await?;
    if nodes.is_empty() {
        return Ok(0);
    }

    let mut found: HashMap<String, String> = HashMap::new();
    if let Ok(from_hv) = hypervisor_guest_ips(state, &provider_id, &nodes).await {
        for (id, ip) in from_hv {
            if vip.as_deref() == Some(ip.as_str()) {
                continue;
            }
            found.insert(id, ip);
        }
    }

    let mut scan_needed = found.len() < nodes.len();
    for (id, _, _, ip, _) in &nodes {
        let Some(cur) = ip.as_deref().filter(|s| !s.is_empty()) else {
            scan_needed = true;
            continue;
        };
        if found.get(id).map(String::as_str).unwrap_or(cur) == cur && !tcp_open(cur, 50000).await {
            scan_needed = true;
        }
    }
    if scan_needed {
        if let Ok(scanned) = scan_subnet_for_hostnames(state, &nodes, vip.as_deref()).await {
            for (id, ip) in scanned {
                found.insert(id, ip);
            }
        }
    }

    let now = db::now_rfc3339();
    let mut updated = 0usize;
    let mut old_cp: Option<String> = None;
    let mut new_cp: Option<String> = None;
    for (id, _name, role, ip, _) in &nodes {
        let Some(next) = found.get(id) else {
            continue;
        };
        let prev = ip.as_deref().unwrap_or("");
        if prev == next {
            continue;
        }
        if role == "controlplane" && old_cp.is_none() {
            old_cp = Some(prev.to_string());
            new_cp = Some(next.clone());
        }
        let r = sqlx::query("UPDATE nodes SET ip = ?, updated_at = ? WHERE id = ?")
            .bind(next)
            .bind(&now)
            .bind(id)
            .execute(state.pool())
            .await?;
        if r.rows_affected() > 0 {
            updated += 1;
        }
    }

    if let (Some(new_ip), Some(kc)) = (
        new_cp.as_deref(),
        kc_path.as_deref().filter(|p| !p.is_empty()),
    ) {
        maybe_rewrite_cluster_endpoint(
            state,
            cluster_id,
            Path::new(kc),
            &vip,
            old_cp.as_deref(),
            new_ip,
            controlplanes,
        )
        .await;
    }

    Ok(updated)
}

const KUBELET_HEAL_TTL: Duration = Duration::from_secs(90);

fn kubelet_heal_at() -> &'static Mutex<HashMap<String, Instant>> {
    static C: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 0.3.x guests can leave `kubelet=absent` after VM power-on (CRI race). Re-apply
/// the node's machine config so pertiskd starts kubelet once containerd is up.
pub async fn heal_absent_kubelets(state: &AppState, cluster_id: &str) -> anyhow::Result<usize> {
    let pertiskctl = &state.cfg().pertiskctl;
    if !pertiskctl.is_file() {
        return Ok(0);
    }
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT name, kubeconfig_path FROM clusters WHERE id = ?",
    )
    .bind(cluster_id)
    .fetch_optional(state.pool())
    .await?;
    let Some((cluster_name, kc_path)) = row else {
        return Ok(0);
    };
    let Some(kc) = kc_path.filter(|p| !p.is_empty()) else {
        return Ok(0);
    };
    let kc = PathBuf::from(kc);
    if !kc.is_file() {
        return Ok(0);
    }

    let rows = fetch_from_kubectl(&kc).await.unwrap_or_default();
    let mut healed = 0usize;
    for (name, snap) in rows {
        if snap.kubelet_status.as_deref() != Some("not_ready") {
            continue;
        }
        let Some(ip) = snap.ip.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        {
            let mut g = kubelet_heal_at().lock().unwrap_or_else(|p| p.into_inner());
            if let Some(at) = g.get(ip) {
                if at.elapsed() < KUBELET_HEAL_TTL {
                    continue;
                }
            }
            g.insert(ip.to_string(), Instant::now());
        }
        if !tcp_open(ip, 50000).await {
            continue;
        }
        let health = fetch_machine_health(pertiskctl, ip).await;
        if health.as_deref() != Some("absent") {
            continue;
        }
        let Some(cfg_path) =
            machine_config_for_node(&state.cfg().kubeconfigs_dir(), &cluster_name, &name)
        else {
            tracing::debug!(node = %name, "kubelet heal skipped; no machine yaml");
            continue;
        };
        let Ok(yaml) = std::fs::read_to_string(&cfg_path) else {
            continue;
        };
        let mut yaml = patch_yaml_hostname(&yaml, &name);
        if let Some(ver) = snap.k8s_version.as_deref() {
            yaml = patch_yaml_kubernetes_version(&yaml, ver);
        }
        let tmp =
            state
                .cfg()
                .data_dir
                .join(format!("kubelet-heal-{}-{}.yaml", cluster_id, uuid_stamp()));
        if std::fs::write(&tmp, &yaml).is_err() {
            continue;
        }
        tracing::info!(node = %name, ip, "re-applying machine config to start kubelet");
        let apply = timeout(
            Duration::from_secs(20),
            Command::new(pertiskctl)
                .args([
                    "-e",
                    &format!("{ip}:50000"),
                    "apply",
                    "-f",
                    &tmp.to_string_lossy(),
                ])
                .output(),
        )
        .await;
        let _ = std::fs::remove_file(&tmp);
        match apply {
            Ok(Ok(out)) if out.status.success() => healed += 1,
            Ok(Ok(out)) => tracing::warn!(
                node = %name,
                status = %out.status,
                stderr = %String::from_utf8_lossy(&out.stderr),
                "kubelet heal apply failed"
            ),
            Ok(Err(err)) => tracing::warn!(node = %name, error = %err, "kubelet heal apply failed"),
            Err(_) => tracing::warn!(node = %name, "kubelet heal apply timed out"),
        }
    }
    Ok(healed)
}

fn uuid_stamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "x".into())
}

async fn fetch_machine_health(pertiskctl: &Path, ip: &str) -> Option<String> {
    let out = timeout(
        Duration::from_secs(4),
        Command::new(pertiskctl)
            .args(["-e", &format!("{ip}:50000"), "health"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_health_kubelet(&String::from_utf8_lossy(&out.stdout))
}

fn parse_health_kubelet(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let main = line.split(" — ").next().unwrap_or(line);
        let main = main.split(" - ").next().unwrap_or(main);
        for part in main.split_whitespace() {
            if let Some(v) = part.strip_prefix("kubelet=") {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn machine_config_for_node(
    kubeconfigs_dir: &Path,
    cluster_name: &str,
    node_name: &str,
) -> Option<PathBuf> {
    let dir = kubeconfigs_dir.join(cluster_name);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(suffix) = node_name.strip_prefix(&format!("{cluster_name}-")) {
        if let Some(rest) = suffix.strip_prefix("cp-") {
            if rest == "1" {
                candidates.push(dir.join("controlplane.yaml"));
            } else {
                candidates.push(dir.join(format!("controlplane-{rest}.yaml")));
            }
        } else if let Some(rest) = suffix.strip_prefix("wk-") {
            candidates.push(dir.join(format!("worker-{rest}.yaml")));
            candidates.push(dir.join("worker.yaml"));
        }
    }
    candidates.push(dir.join(format!("{node_name}.yaml")));
    candidates.into_iter().find(|p| p.is_file())
}

fn patch_yaml_hostname(yaml: &str, hostname: &str) -> String {
    let mut out = String::with_capacity(yaml.len() + 16);
    let mut patched = false;
    for line in yaml.lines() {
        let trimmed = line.trim_start();
        if !patched && trimmed.starts_with("hostname:") {
            let indent = line.len() - trimmed.len();
            out.push_str(&" ".repeat(indent));
            out.push_str("hostname: ");
            out.push_str(hostname);
            out.push('\n');
            patched = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn patch_yaml_kubernetes_version(yaml: &str, version: &str) -> String {
    let mut out = String::with_capacity(yaml.len() + 32);
    for line in yaml.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("kubernetesVersion:") {
            let indent = line.len() - trimmed.len();
            out.push_str(&" ".repeat(indent));
            out.push_str("kubernetesVersion: ");
            out.push_str(version);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

async fn maybe_rewrite_cluster_endpoint(
    state: &AppState,
    cluster_id: &str,
    kc: &Path,
    vip: &Option<String>,
    old_cp: Option<&str>,
    new_ip: &str,
    controlplanes: i64,
) {
    let Ok(raw) = std::fs::read_to_string(kc) else {
        return;
    };
    let host = kubeconfig::kubeconfig_server_host(&raw).unwrap_or_default();
    let points_at_vip = vip.as_deref().is_some_and(|v| v == host);
    let points_at_old = old_cp.is_some_and(|o| !o.is_empty() && o == host);
    if points_at_vip && controlplanes > 1 {
        return;
    }
    if !(points_at_old || (controlplanes <= 1 && !points_at_vip)) {
        return;
    }
    let url = format!("https://{new_ip}:6443");
    let next = kubeconfig::rewrite_kubeconfig_server_url(&raw, &url);
    if next != raw {
        let _ = std::fs::write(kc, next);
    }
    let _ = sqlx::query("UPDATE clusters SET endpoint = ?, updated_at = ? WHERE id = ?")
        .bind(new_ip)
        .bind(db::now_rfc3339())
        .bind(cluster_id)
        .execute(state.pool())
        .await;
}

async fn hypervisor_guest_ips(
    state: &AppState,
    provider_id: &str,
    nodes: &[(String, String, String, Option<String>, Option<i64>)],
) -> anyhow::Result<Vec<(String, String)>> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, i64)>(
        "SELECT kind, url, token_id, token_secret_enc, node, insecure FROM providers WHERE id = ?",
    )
    .bind(provider_id)
    .fetch_optional(state.pool())
    .await?;
    let Some((kind, url, token_id, secret_enc, pve_node, insecure)) = row else {
        return Ok(Vec::new());
    };
    let secret = crypto::decrypt(&state.cfg().secret_key, &secret_enc)?;
    let insecure = insecure != 0;
    let mut out = Vec::new();
    for (id, name, _, _, vmid) in nodes {
        let ip = match kind.as_str() {
            "nutanix" => {
                NutanixClient::new(url.clone(), token_id.clone(), secret.clone(), insecure)
                    .vm_ipv4(name)
                    .await
                    .ok()
                    .flatten()
            }
            "vsphere" => {
                VsphereClient::new(url.clone(), token_id.clone(), secret.clone(), insecure)
                    .vm_guest_ipv4(name)
                    .await
                    .ok()
                    .flatten()
            }
            _ => {
                let Some(vmid) = *vmid else {
                    continue;
                };
                let client = ProxmoxClient {
                    url: url.clone(),
                    token_id: token_id.clone(),
                    token_secret: secret.clone(),
                    insecure,
                };
                client.vm_guest_ipv4(&pve_node, vmid).await.ok().flatten()
            }
        };
        if let Some(ip) = ip.filter(|s| usable_lan_ipv4(s)) {
            out.push((id.clone(), ip));
        }
    }
    Ok(out)
}

async fn scan_subnet_for_hostnames(
    state: &AppState,
    nodes: &[(String, String, String, Option<String>, Option<i64>)],
    vip: Option<&str>,
) -> anyhow::Result<Vec<(String, String)>> {
    let pertiskctl = &state.cfg().pertiskctl;
    if !pertiskctl.is_file() {
        return Ok(Vec::new());
    }
    let Some(base) = subnet_base(nodes) else {
        return Ok(Vec::new());
    };
    let mut skip: HashSet<String> = HashSet::new();
    if let Some(v) = vip {
        skip.insert(v.to_string());
    }
    let by_name: HashMap<String, String> = nodes
        .iter()
        .map(|(id, name, _, _, _)| (name.clone(), id.clone()))
        .collect();

    let mut futs = Vec::new();
    for host in 1u8..=254 {
        let ip = Ipv4Addr::new(base[0], base[1], base[2], host);
        let ip_s = ip.to_string();
        if skip.contains(&ip_s) {
            continue;
        }
        let bin = pertiskctl.clone();
        futs.push(async move {
            if !tcp_open(&ip_s, 50000).await {
                return None;
            }
            let out = timeout(
                Duration::from_secs(3),
                Command::new(&bin)
                    .args(["-e", &format!("{ip_s}:50000"), "version"])
                    .output(),
            )
            .await
            .ok()?
            .ok()?;
            if !out.status.success() {
                return None;
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            let host = parse_pertiskctl_hostname(&stdout)?;
            Some((host, ip_s))
        });
    }
    let mut out = Vec::new();
    for (hostname, ip) in futures::future::join_all(futs).await.into_iter().flatten() {
        if let Some(id) = by_name.get(&hostname) {
            out.push((id.clone(), ip));
        }
    }
    Ok(out)
}

fn subnet_base(nodes: &[(String, String, String, Option<String>, Option<i64>)]) -> Option<[u8; 4]> {
    for (_, _, _, ip, _) in nodes {
        let Some(ip) = ip.as_deref().filter(|s| usable_lan_ipv4(s)) else {
            continue;
        };
        let Ok(v4) = ip.parse::<Ipv4Addr>() else {
            continue;
        };
        let o = v4.octets();
        return Some([o[0], o[1], o[2], 0]);
    }
    None
}

fn usable_lan_ipv4(ip: &str) -> bool {
    let Ok(addr) = ip.trim().parse::<Ipv4Addr>() else {
        return false;
    };
    !(addr.is_loopback()
        || addr.is_unspecified()
        || addr.is_broadcast()
        || addr.is_link_local()
        || addr.is_multicast())
}

async fn tcp_open(ip: &str, port: u16) -> bool {
    let Ok(addr) = format!("{ip}:{port}").parse::<SocketAddr>() else {
        return false;
    };
    matches!(
        timeout(Duration::from_millis(400), TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
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
    fn parses_pertiskctl_node_version() {
        let out =
            "pertiskctl 0.1.0\nnode 0.2.89 hostname=lab-ha-285h-cp-3 (api v1alpha1 / linux)\n";
        assert_eq!(
            parse_pertiskctl_node_version(out).as_deref(),
            Some("0.2.89")
        );
        assert_eq!(
            parse_pertiskctl_node_version("pertiskctl 0.1.0\nnode: unreachable (timeout)\n")
                .as_deref(),
            None
        );
        assert!(is_placeholder_os_version(Some("0.1.0")));
        assert!(!is_placeholder_os_version(Some("0.2.89")));
        assert_eq!(
            parse_pertiskctl_hostname(out).as_deref(),
            Some("lab-ha-285h-cp-3")
        );
    }

    #[test]
    fn parses_os_image() {
        assert_eq!(
            parse_os_image("pertisk-kos 0.2.87").as_deref(),
            Some("0.2.87")
        );
        assert_eq!(parse_os_image("pertisk-kos").as_deref(), None);
        assert_eq!(parse_os_image("  ").as_deref(), None);
        assert_eq!(
            parse_os_image("Alpine Linux v3.20").as_deref(),
            Some("Alpine Linux v3.20")
        );
    }

    #[test]
    fn parses_containerd_runtime() {
        assert_eq!(
            parse_container_runtime("containerd://2.3.4").as_deref(),
            Some("2.3.4")
        );
        assert_eq!(
            parse_container_runtime("cri-o://1.32.0").as_deref(),
            Some("cri-o://1.32.0")
        );
        assert_eq!(parse_container_runtime("").as_deref(), None);
    }

    #[test]
    fn parses_health_kubelet() {
        assert_eq!(
            parse_health_kubelet(
                "ready=true containerd=up kubelet=absent — containerd=up kubelet=absent"
            )
            .as_deref(),
            Some("absent")
        );
        assert_eq!(
            parse_health_kubelet("ready=true containerd=up kubelet=up — all good").as_deref(),
            Some("up")
        );
    }

    #[test]
    fn patches_hostname_and_k8s_version() {
        let yaml = "machine:\n  type: worker\n  network:\n    hostname: old-wk-1\ncluster:\n  kubernetesVersion: v1.36.1\n";
        let y = patch_yaml_hostname(yaml, "lab-ha-nutanix-wk-1");
        assert!(y.contains("hostname: lab-ha-nutanix-wk-1"));
        let y = patch_yaml_kubernetes_version(&y, "v1.36.3");
        assert!(y.contains("kubernetesVersion: v1.36.3"));
        assert!(!y.contains("v1.36.1"));
    }
}
