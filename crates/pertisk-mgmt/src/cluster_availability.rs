//! Live cluster reachability (apiserver /readyz) — separate from lifecycle `status`.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

use crate::k8s::resolve_cluster_kubeconfig;
use crate::state::AppState;

/// `online` — apiserver answered /readyz  
/// `offline` — provisioned (ready) but API unreachable  
/// `unknown` — not ready yet / no kubeconfig / mid-job
pub async fn probe(state: &AppState, cluster_id: &str, lifecycle_status: &str) -> String {
    if lifecycle_status != "ready" {
        return "unknown".into();
    }
    let Ok((kc, _)) = resolve_cluster_kubeconfig(state, cluster_id).await else {
        return "offline".into();
    };
    if probe_readyz(&kc, None).await {
        return "online".into();
    }

    // VIP may be down while a CP still answers — try control-plane node IPs.
    let cps: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT ip FROM nodes WHERE cluster_id = ? AND role = 'controlplane' ORDER BY name",
    )
    .bind(cluster_id)
    .fetch_all(state.pool())
    .await
    .unwrap_or_default();

    for (ip,) in cps {
        let Some(ip) = ip.filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        let server = format!("https://{}:6443", ip.trim());
        if probe_readyz(&kc, Some(&server)).await {
            return "online".into();
        }
    }
    "offline".into()
}

async fn probe_readyz(kc: &Path, server: Option<&str>) -> bool {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kc);
    if let Some(s) = server {
        cmd.arg("--server").arg(s);
    }
    cmd.args(["get", "--raw", "/readyz"]);
    // Keep short — list/dashboard probe many clusters.
    match timeout(Duration::from_secs(3), cmd.output()).await {
        Ok(Ok(out)) => out.status.success(),
        _ => false,
    }
}
