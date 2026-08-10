//! Live node reachability (Machine API `:50000`) — separate from lifecycle `status`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::routes::nodes::NodeOut;

const PROBE_TIMEOUT: Duration = Duration::from_millis(800);
/// Skip re-probing offline nodes briefly (cluster detail poll).
const OFFLINE_CACHE_TTL: Duration = Duration::from_secs(15);

type Cache = HashMap<String, (Instant, String)>;

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `online` — TCP `:50000` accepts  
/// `offline` — has IPv4 but API unreachable  
/// `unknown` — no IP yet / still provisioning
pub async fn probe_node(node: &NodeOut) -> String {
    let ip = node
        .ip
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(ip) = ip else {
        return "unknown".into();
    };
    if matches!(
        node.status.as_str(),
        "pending" | "provisioning" | "deleting"
    ) {
        return "unknown".into();
    }

    let key = format!("{ip}:50000");
    if let Ok(cache) = cache().lock() {
        if let Some((at, avail)) = cache.get(&key) {
            if avail == "offline" && at.elapsed() < OFFLINE_CACHE_TTL {
                return avail.clone();
            }
        }
    }

    let result: String = if tcp_open(ip, 50000).await {
        "online".into()
    } else {
        "offline".into()
    };

    if result == "offline" {
        if let Ok(mut c) = cache().lock() {
            c.insert(key, (Instant::now(), result.clone()));
        }
    } else if let Ok(mut c) = cache().lock() {
        c.remove(&key);
    }

    result
}

/// Fill `availability` on each node (parallel probes).
pub async fn fill(nodes: &mut [NodeOut]) {
    let futs: Vec<_> = nodes.iter().map(probe_node).collect();
    let avails = futures::future::join_all(futs).await;
    for (n, a) in nodes.iter_mut().zip(avails) {
        n.availability = a;
    }
}

async fn tcp_open(ip: &str, port: u16) -> bool {
    let Ok(addr) = format!("{ip}:{port}").parse::<SocketAddr>() else {
        return false;
    };
    matches!(timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await, Ok(Ok(_)))
}
