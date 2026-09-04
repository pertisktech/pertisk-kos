//! Live node reachability (Machine API `:50000`) — separate from lifecycle `status`.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::routes::nodes::NodeOut;

const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
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
    format!("{ip}:50000")
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

/// Bare IPv4/IPv6 for a TCP probe (strip CIDR, skip empties).
pub(crate) fn normalize_probe_ip(ip: &str) -> Option<String> {
    let s = ip.trim().split('/').next().unwrap_or("").trim();
    if s.is_empty() {
        return None;
    }
    Some(s.to_string())
}

fn skip_probe(status: &str) -> bool {
    // Probe whenever an IP exists — including provisioning/error — so a live
    // guest after a failed lab-up is not stuck as "unknown".
    matches!(status, "deleting")
}

fn spawn_refresh(ip: Option<String>, ip6: Option<String>, status: String) {
    if skip_probe(&status) {
        return;
    }
    let Some(key_ip) = ip
        .as_deref()
        .and_then(normalize_probe_ip)
        .or_else(|| ip6.as_deref().and_then(normalize_probe_ip))
    else {
        return;
    };
    let key = key_for(&key_ip);
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
        let _ = probe_addrs(ip.as_deref(), ip6.as_deref(), &status).await;
        if let Ok(mut g) = inflight().lock() {
            g.remove(&key);
        }
    });
}

/// `online` — TCP `:50000` accepts
/// `offline` — has an address but API unreachable
/// `unknown` — no IP yet / node is deleting
pub async fn probe(ip: Option<&str>, status: &str) -> String {
    probe_addrs(ip, None, status).await
}

pub async fn probe_addrs(ip: Option<&str>, ip6: Option<&str>, status: &str) -> String {
    if skip_probe(status) {
        return "unknown".into();
    }
    let v4 = ip.and_then(normalize_probe_ip);
    let v6 = ip6.and_then(normalize_probe_ip);
    let Some(primary) = v4.clone().or_else(|| v6.clone()) else {
        return "unknown".into();
    };

    let key = key_for(&primary);
    if let Some(avail) = lookup(&key, FRESH_TTL) {
        return avail;
    }

    let mut open = tcp_open(&primary, 50000).await;
    if !open {
        if let Some(alt) = v6.filter(|a| Some(a.as_str()) != Some(primary.as_str())) {
            open = tcp_open(&alt, 50000).await;
        }
    }
    let result: String = if open { "online".into() } else { "offline".into() };
    store(&key, result.clone());
    result
}

/// Health RPC succeeded — Machine API is up even if the TCP probe raced.
pub fn mark_online(ip: Option<&str>) {
    let Some(ip) = ip.and_then(normalize_probe_ip) else {
        return;
    };
    store(&key_for(&ip), "online".into());
}

pub async fn probe_node(node: &NodeOut) -> String {
    probe_addrs(node.ip.as_deref(), node.ip6.as_deref(), &node.status).await
}

/// Fill `availability` from a live probe (cached 15s).
pub async fn fill(nodes: &mut [NodeOut]) {
    let futs: Vec<_> = nodes
        .iter()
        .map(|n| probe_addrs(n.ip.as_deref(), n.ip6.as_deref(), &n.status))
        .collect();
    let avails = futures::future::join_all(futs).await;
    for (n, a) in nodes.iter_mut().zip(avails) {
        n.availability = a;
        spawn_refresh(n.ip.clone(), n.ip6.clone(), n.status.clone());
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
    let Some(ip) = normalize_probe_ip(ip) else {
        return false;
    };
    let addr = if ip.contains(':') && !ip.starts_with('[') {
        format!("[{ip}]:{port}")
    } else {
        format!("{ip}:{port}")
    };
    let Ok(addr) = addr.parse::<SocketAddr>() else {
        return false;
    };
    matches!(
        timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

#[cfg(test)]
mod tests {
    use super::normalize_probe_ip;

    #[test]
    fn strips_cidr_and_space() {
        assert_eq!(normalize_probe_ip(" 10.1.1.245/24 ").as_deref(), Some("10.1.1.245"));
        assert_eq!(normalize_probe_ip("10.1.1.245").as_deref(), Some("10.1.1.245"));
        assert_eq!(
            normalize_probe_ip("fd00:1::1/64").as_deref(),
            Some("fd00:1::1")
        );
        assert_eq!(normalize_probe_ip("  ").as_deref(), None);
    }
}
