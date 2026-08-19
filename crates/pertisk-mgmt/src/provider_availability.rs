//! Live hypervisor reachability — separate from stored provider rows.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::crypto;
use crate::nutanix::NutanixClient;
use crate::proxmox::ProxmoxClient;
use crate::routes::providers::ProviderOut;
use crate::state::AppState;
use crate::vsphere::VsphereClient;

const FRESH_TTL: Duration = Duration::from_secs(15);
const STALE_TTL: Duration = Duration::from_secs(120);

type Cache = HashMap<String, (Instant, String)>;

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn inflight() -> &'static Mutex<HashSet<String>> {
    static C: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashSet::new()))
}

fn store(provider_id: &str, avail: String) {
    if let Ok(mut c) = cache().lock() {
        c.insert(provider_id.to_string(), (Instant::now(), avail));
    }
}

fn lookup(provider_id: &str, max_age: Duration) -> Option<String> {
    let c = cache().lock().ok()?;
    let (at, avail) = c.get(provider_id)?;
    if at.elapsed() <= max_age {
        Some(avail.clone())
    } else {
        None
    }
}

/// Last-known hypervisor reachability (`unknown` if never probed).
pub fn cached_or(provider_id: &str) -> String {
    if provider_id.trim().is_empty() {
        return "unknown".into();
    }
    lookup(provider_id, STALE_TTL).unwrap_or_else(|| "unknown".into())
}

/// Refresh in the background unless a probe is already fresh or in flight.
pub fn spawn_refresh(state: AppState, provider_id: String) {
    if provider_id.trim().is_empty() {
        return;
    }
    if lookup(&provider_id, FRESH_TTL).is_some() {
        return;
    }
    {
        let Ok(mut g) = inflight().lock() else {
            return;
        };
        if !g.insert(provider_id.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        let _ = probe(&state, &provider_id).await;
        if let Ok(mut g) = inflight().lock() {
            g.remove(&provider_id);
        }
    });
}

/// `online` — hypervisor API accepted stored credentials  
/// `offline` — timeout / unreachable / auth failed  
/// `unknown` — missing provider row
pub async fn probe(state: &AppState, provider_id: &str) -> String {
    if provider_id.trim().is_empty() {
        return "unknown".into();
    }
    if let Some(avail) = lookup(provider_id, FRESH_TTL) {
        return avail;
    }

    let result = probe_uncached(state, provider_id).await;
    store(provider_id, result.clone());
    result
}

async fn probe_uncached(state: &AppState, provider_id: &str) -> String {
    let row = sqlx::query_as::<_, (String, String, String, String, i64)>(
        "SELECT kind, url, token_id, token_secret_enc, insecure FROM providers WHERE id = ?",
    )
    .bind(provider_id)
    .fetch_optional(state.pool())
    .await
    .ok()
    .flatten();
    let Some((kind, url, token_id, secret_enc, insecure)) = row else {
        return "unknown".into();
    };
    let Ok(secret) = crypto::decrypt(&state.cfg().secret_key, &secret_enc) else {
        return "offline".into();
    };
    let insecure = insecure != 0;
    let ok = match kind.as_str() {
        "vsphere" => {
            VsphereClient::new(url, token_id, secret, insecure)
                .ping()
                .await
        }
        "nutanix" => {
            NutanixClient::new(url, token_id, secret, insecure)
                .ping()
                .await
        }
        _ => {
            ProxmoxClient {
                url,
                token_id,
                token_secret: secret,
                insecure,
            }
            .ping()
            .await
        }
    };
    if ok {
        "online".into()
    } else {
        "offline".into()
    }
}

pub async fn fill(state: &AppState, providers: &mut [ProviderOut]) {
    let mut seen: HashSet<String> = HashSet::new();
    for p in providers.iter_mut() {
        p.availability = cached_or(&p.id);
        if seen.insert(p.id.clone()) {
            spawn_refresh(state.clone(), p.id.clone());
        }
    }
}
