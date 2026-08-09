//! Live cluster reachability (apiserver /readyz) — separate from lifecycle `status`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::time::timeout;

use crate::k8s::resolve_cluster_kubeconfig;
use crate::state::AppState;

const READYZ_TIMEOUT: Duration = Duration::from_secs(1);
/// Skip re-probing offline clusters briefly (dashboard / list poll).
const OFFLINE_CACHE_TTL: Duration = Duration::from_secs(30);

type AvailCache = HashMap<String, (Instant, String)>;

fn avail_cache() -> &'static Mutex<AvailCache> {
    static CACHE: OnceLock<Mutex<AvailCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `online` — apiserver answered /readyz  
/// `offline` — provisioned (ready) but API unreachable  
/// `unknown` — not ready yet / no kubeconfig / mid-job
pub async fn probe(state: &AppState, cluster_id: &str, lifecycle_status: &str) -> String {
    if lifecycle_status != "ready" {
        return "unknown".into();
    }

    if let Ok(cache) = avail_cache().lock() {
        if let Some((at, avail)) = cache.get(cluster_id) {
            if avail == "offline" && at.elapsed() < OFFLINE_CACHE_TTL {
                return avail.clone();
            }
        }
    }

    let result = probe_uncached(state, cluster_id).await;

    if result == "offline" {
        if let Ok(mut cache) = avail_cache().lock() {
            cache.insert(cluster_id.to_string(), (Instant::now(), result.clone()));
        }
    } else if let Ok(mut cache) = avail_cache().lock() {
        cache.remove(cluster_id);
    }

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

    let kc = kc;
    let futs: Vec<_> = servers
        .into_iter()
        .map(|server| {
            let kc = kc.clone();
            async move { probe_readyz(&kc, server.as_deref()).await }
        })
        .collect();

    let results = futures::future::join_all(futs).await;
    if results.into_iter().any(|ok| ok) {
        "online".into()
    } else {
        "offline".into()
    }
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

/// Parallel /readyz against kubeconfig server + optional overrides; returns first working server override
/// (`None` means kubeconfig default worked).
pub async fn first_reachable_server(
    kc: &Path,
    extra_servers: &[String],
) -> Option<Option<String>> {
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
