//! Live hypervisor reachability — separate from stored provider rows.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::crypto;
use crate::nutanix::NutanixClient;
use crate::proxmox::ProxmoxClient;
use crate::routes::providers::ProviderOut;
use crate::state::AppState;
use crate::vsphere::VsphereClient;

const OFFLINE_CACHE_TTL: Duration = Duration::from_secs(20);

type Cache = HashMap<String, (Instant, String)>;

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `online` — hypervisor API accepted stored credentials  
/// `offline` — timeout / unreachable / auth failed  
/// `unknown` — missing provider row
pub async fn probe(state: &AppState, provider_id: &str) -> String {
    if provider_id.trim().is_empty() {
        return "unknown".into();
    }
    if let Ok(cache) = cache().lock() {
        if let Some((at, avail)) = cache.get(provider_id) {
            if avail == "offline" && at.elapsed() < OFFLINE_CACHE_TTL {
                return avail.clone();
            }
        }
    }

    let result = probe_uncached(state, provider_id).await;

    if result == "offline" {
        if let Ok(mut c) = cache().lock() {
            c.insert(provider_id.to_string(), (Instant::now(), result.clone()));
        }
    } else if let Ok(mut c) = cache().lock() {
        c.remove(provider_id);
    }

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
    let futs: Vec<_> = providers
        .iter()
        .map(|p| {
            let state = state.clone();
            let id = p.id.clone();
            async move { probe(&state, &id).await }
        })
        .collect();
    let avails = futures::future::join_all(futs).await;
    for (p, a) in providers.iter_mut().zip(avails) {
        p.availability = a;
    }
}
