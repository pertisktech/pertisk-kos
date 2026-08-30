//! Live node reachability (Machine API `:50000`) — separate from lifecycle `status`.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::routes::nodes::NodeOut;

const PROBE_TIMEOUT: Duration = Duration::from_millis(800);
const FRESH_TTL: Duration = Duration::from_secs(15);
const STALE_TTL: Duration = Duration::from_secs(60);

type Cache = HashMap<String, (Instant, String)>;

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn inflight() -> &'static Mutex<HashSet<String>> {
    static C: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashSet::new()))
}

fn key_for(ip: &str) -> String {
    format!("{}:50000", ip.trim())
}

fn store(key: &str, avail: String) {
    if let Ok(mut c) = cache().lock() {
        c.insert(key.to_string(), (Instant::now(), avail));
    }
}

fn lookup(key: &str, max_age: Duration) -> Option<String> {
    let c = cache().lock().ok()?;
    let (at, avail) = c.get(key)?;
    if at.elapsed() <= max_age {
        Some(avail.clone())
    } else {
        None
    }
}

fn cached_or(ip: Option<&str>, status: &str) -> String {
    let ip = ip.map(str::trim).filter(|s| !s.is_empty());
    let Some(ip) = ip else {
        return "unknown".into();
    };
    if matches!(status, "pending" | "provisioning" | "deleting") {
        return "unknown".into();
    }
    lookup(&key_for(ip), STALE_TTL).unwrap_or_else(|| "unknown".into())
}

fn spawn_refresh(ip: Option<String>, status: String) {
    let Some(ip) = ip.filter(|s| !s.trim().is_empty()) else {
        return;
    };
    if matches!(status.as_str(), "pending" | "provisioning" | "deleting") {
        return;
    }
    let key = key_for(&ip);
    if lookup(&key, FRESH_TTL).is_some() {
        return;
    }
    {
        let Ok(mut g) = inflight().lock() else {
            return;
        };
        if !g.insert(key.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        let _ = probe(Some(&ip), &status).await;
        if let Ok(mut g) = inflight().lock() {
            g.remove(&key);
        }
    });
}

/// `online` — TCP `:50000` accepts  
/// `offline` — has IPv4 but API unreachable  
/// `unknown` — no IP yet / still provisioning
pub async fn probe(ip: Option<&str>, status: &str) -> String {
    let ip = ip.map(str::trim).filter(|s| !s.is_empty());
    let Some(ip) = ip else {
        return "unknown".into();
    };
    if matches!(status, "pending" | "provisioning" | "deleting") {
        return "unknown".into();
    }

    let key = key_for(ip);
    if let Some(avail) = lookup(&key, FRESH_TTL) {
        return avail;
    }

    let result: String = if tcp_open(ip, 50000).await {
        "online".into()
    } else {
        "offline".into()
    };
    store(&key, result.clone());
    result
}

pub async fn probe_node(node: &NodeOut) -> String {
    probe(node.ip.as_deref(), &node.status).await
}

/// Fill `availability` from cache and refresh in the background.
pub async fn fill(nodes: &mut [NodeOut]) {
    for n in nodes.iter_mut() {
        n.availability = cached_or(n.ip.as_deref(), &n.status);
        spawn_refresh(n.ip.clone(), n.status.clone());
    }
}

/// A node can go stale (mgmt has the pre-reboot DHCP/IPAM IP) even while the
/// rest of an HA cluster answers `/readyz` fine, so `cluster_availability`'s
/// rediscovery never fires. Kick it here too, keyed off individual node
/// unreachability; `node_sync::rediscover_cluster_ips` has its own 60s throttle.
pub fn spawn_rediscover_if_offline(state: &crate::state::AppState, cluster_id: &str, nodes: &[NodeOut]) {
    let stale = nodes
        .iter()
        .any(|n| n.availability == "offline" && n.ip.as_deref().is_some_and(|s| !s.trim().is_empty()));
    if !stale {
        return;
    }
    let state = state.clone();
    let cluster_id = cluster_id.to_string();
    tokio::spawn(async move {
        let _ = crate::node_sync::rediscover_cluster_ips(&state, &cluster_id).await;
    });
}

async fn tcp_open(ip: &str, port: u16) -> bool {
    let Ok(addr) = format!("{ip}:{port}").parse::<SocketAddr>() else {
        return false;
    };
    matches!(
        timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}
