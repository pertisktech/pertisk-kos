//! Live cluster reachability (apiserver /readyz) — separate from lifecycle `status`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::time::timeout;

use crate::k8s::resolve_cluster_kubeconfig;
use crate::state::AppState;

const READYZ_TIMEOUT: Duration = Duration::from_secs(1);
/// Serve this without re-probing (list/dashboard poll interval is 15s).
const FRESH_TTL: Duration = Duration::from_secs(15);
/// Last-known value is good enough to paint the UI while a refresh runs.
const STALE_TTL: Duration = Duration::from_secs(120);

type AvailCache = HashMap<String, (Instant, String)>;

fn avail_cache() -> &'static Mutex<AvailCache> {
    static CACHE: OnceLock<Mutex<AvailCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn inflight() -> &'static Mutex<HashSet<String>> {
    static C: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashSet::new()))
}

fn store(cluster_id: &str, avail: String) {
    if let Ok(mut cache) = avail_cache().lock() {
        cache.insert(cluster_id.to_string(), (Instant::now(), avail));
    }
}

fn lookup(cluster_id: &str, max_age: Duration) -> Option<String> {
    let cache = avail_cache().lock().ok()?;
    let (at, avail) = cache.get(cluster_id)?;
    if at.elapsed() <= max_age {
        Some(avail.clone())
    } else {
        None
    }
}

/// Last-known reachability for a ready cluster (`unknown` if none / not ready).
pub fn cached_or(cluster_id: &str, lifecycle_status: &str) -> String {
    if lifecycle_status != "ready" {
        return "unknown".into();
    }
    lookup(cluster_id, STALE_TTL).unwrap_or_else(|| "unknown".into())
}

/// Refresh in the background unless a probe is already fresh or in flight.
pub fn spawn_refresh(state: AppState, cluster_id: String, lifecycle_status: String) {
    if lifecycle_status != "ready" {
        return;
    }
    if lookup(&cluster_id, FRESH_TTL).is_some() {
        return;
    }
    {
        let Ok(mut g) = inflight().lock() else {
            return;
        };
        if !g.insert(cluster_id.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        let _ = probe(&state, &cluster_id, &lifecycle_status).await;
        if let Ok(mut g) = inflight().lock() {
            g.remove(&cluster_id);
        }
    });
}

/// `online` — apiserver answered /readyz  
/// `offline` — provisioned (ready) but API unreachable  
/// `unknown` — not ready yet / no kubeconfig / mid-job
pub async fn probe(state: &AppState, cluster_id: &str, lifecycle_status: &str) -> String {
    if lifecycle_status != "ready" {
        return "unknown".into();
    }

    if let Some(avail) = lookup(cluster_id, FRESH_TTL) {
        return avail;
    }

    let result = probe_uncached(state, cluster_id).await;
    store(cluster_id, result.clone());
    result
}

async fn probe_uncached(state: &AppState, cluster_id: &str) -> String {
    let Ok((kc, _)) = resolve_cluster_kubeconfig(state, cluster_id).await else {
        return "offline".into();
    };

    // VIP (kubeconfig server) + CP IPs in parallel — first success wins.
    let cps: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT ip FROM nodes WHERE cluster_id = ? AND role = 'controlplane' ORDER BY name",
    )
    .bind(cluster_id)
    .fetch_all(state.pool())
    .await
    .unwrap_or_default();

    let mut servers: Vec<Option<String>> = vec![None]; // kubeconfig default (VIP)
    for (ip,) in cps {
        let Some(ip) = ip.filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        servers.push(Some(format!("https://{}:6443", ip.trim())));
    }

    let futs: Vec<_> = servers
        .into_iter()
        .map(|server| {
            let kc = kc.clone();
            async move { probe_readyz(&kc, server.as_deref()).await }
        })
        .collect();

    let results = futures::future::join_all(futs).await;
    if results.into_iter().any(|ok| ok) {
        spawn_kubelet_heal(state.clone(), cluster_id.to_string());
        return "online".into();
    }

    // Nutanix/vSphere host reboot often hands guests a new DHCP/IPAM address.
    // Refresh node IPs, then retry /readyz against the new control-plane IPs.
    let _ = crate::node_sync::rediscover_cluster_ips(state, cluster_id).await;
    let cps: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT ip FROM nodes WHERE cluster_id = ? AND role = 'controlplane' ORDER BY name",
    )
    .bind(cluster_id)
    .fetch_all(state.pool())
    .await
    .unwrap_or_default();
    let mut servers: Vec<Option<String>> = vec![None];
    for (ip,) in cps {
        let Some(ip) = ip.filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        servers.push(Some(format!("https://{}:6443", ip.trim())));
    }
    let futs: Vec<_> = servers
        .into_iter()
        .map(|server| {
            let kc = kc.clone();
            async move { probe_readyz(&kc, server.as_deref()).await }
        })
        .collect();
    if futures::future::join_all(futs)
        .await
        .into_iter()
        .any(|ok| ok)
    {
        spawn_kubelet_heal(state.clone(), cluster_id.to_string());
        return "online".into();
    }
    "offline".into()
}

fn spawn_kubelet_heal(state: AppState, cluster_id: String) {
    tokio::spawn(async move {
        let _ = crate::node_sync::heal_absent_kubelets(&state, &cluster_id).await;
    });
}

pub async fn probe_readyz(kc: &Path, server: Option<&str>) -> bool {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kc);
    if let Some(s) = server {
        cmd.arg("--server").arg(s);
    }
    // Match tokio timeout so kubectl does not hang past the deadline.
    cmd.args(["--request-timeout=1s", "get", "--raw", "/readyz"]);
    match timeout(READYZ_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => out.status.success(),
        _ => false,
    }
}

/// `/readyz` or a namespace get — kube-vip can be down while a CP `:6443` still serves.
pub async fn probe_api(kc: &Path, server: Option<&str>) -> bool {
    if probe_readyz(kc, server).await {
        return true;
    }
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kc);
    if let Some(s) = server {
        cmd.arg("--server").arg(s);
    }
    cmd.args(["--request-timeout=2s", "get", "ns", "kube-system"]);
    match timeout(Duration::from_secs(3), cmd.output()).await {
        Ok(Ok(out)) => out.status.success(),
        _ => false,
    }
}

/// Parallel /readyz against kubeconfig server + optional overrides; returns first working server override
/// (`None` means kubeconfig default worked).
pub async fn first_reachable_server(kc: &Path, extra_servers: &[String]) -> Option<Option<String>> {
    let mut candidates: Vec<Option<String>> = vec![None];
    for s in extra_servers {
        let t = s.trim();
        if !t.is_empty() {
            candidates.push(Some(t.to_string()));
        }
    }

    let kc_buf: PathBuf = kc.to_path_buf();
    let futs: Vec<_> = candidates
        .into_iter()
        .map(|server| {
            let kc = kc_buf.clone();
            async move {
                let ok = probe_readyz(&kc, server.as_deref()).await;
                (ok, server)
            }
        })
        .collect();

    for (ok, server) in futures::future::join_all(futs).await {
        if ok {
            return Some(server);
        }
    }
    None
}

/// Like [`first_reachable_server`] but also accepts an API that answers `get ns` when `/readyz` is false.
pub async fn first_usable_server(kc: &Path, extra_servers: &[String]) -> Option<Option<String>> {
    let mut candidates: Vec<Option<String>> = vec![None];
    for s in extra_servers {
        let t = s.trim();
        if !t.is_empty() {
            candidates.push(Some(t.to_string()));
        }
    }

    let kc_buf: PathBuf = kc.to_path_buf();
    let futs: Vec<_> = candidates
        .into_iter()
        .map(|server| {
            let kc = kc_buf.clone();
            async move {
                let ok = probe_api(&kc, server.as_deref()).await;
                (ok, server)
            }
        })
        .collect();

    for (ok, server) in futures::future::join_all(futs).await {
        if ok {
            return Some(server);
        }
    }
    None
}
