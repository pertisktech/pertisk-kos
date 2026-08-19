//! Optional Prometheus Pushgateway POST from `pertiskd` (same host metrics as `:50001`).
//!
//! Logs already *push* to Loki; the Grafana node dashboard *pulls* Prometheus.
//! Without this, compose `file_sd` placeholders leave **Pertisk node** empty while
//! Loki still has data. Disabled unless `prometheusPushUrl` / `PERTISK_PROM_PUSH_URL`
//! is set, or `lokiUrl` uses compose Alloy `:3500` (then push to `:9091`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pertisk_config::MachineConfig;
use tracing::{info, warn};

use crate::metrics::render_metrics;
use crate::state::SharedState;

const PUSH_EVERY: Duration = Duration::from_secs(15);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

static CLI: OnceLock<PromCli> = OnceLock::new();
static HANDLE: Mutex<Option<PromPushHandle>> = Mutex::new(None);

#[derive(Debug, Clone, Default)]
struct PromCli {
    push_url: Option<String>,
    loki_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromPushSettings {
    pub url: String,
    pub hostname: String,
    pub cluster: String,
}

impl PromPushSettings {
    pub fn resolve(
        cfg: Option<&MachineConfig>,
        cli_push: Option<&str>,
        cli_loki: Option<&str>,
    ) -> Option<Self> {
        let yaml = cfg.and_then(|c| c.machine.observability.as_ref());
        let explicit = first_nonempty([
            cli_push.map(str::trim),
            yaml.and_then(|o| o.prometheus_push_url.as_deref().map(str::trim)),
        ]);
        let url = match explicit {
            Some(u) => u.to_string(),
            None => derive_from_loki(first_nonempty([
                cli_loki.map(str::trim),
                yaml.and_then(|o| o.loki_url.as_deref().map(str::trim)),
            ]))?,
        };
        let hostname = cfg
            .and_then(|c| c.machine.network.hostname.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(kernel_hostname);
        let cluster = cfg
            .and_then(|c| c.cluster.as_ref())
            .and_then(|c| c.name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "pertisk".into());
        Some(Self {
            url,
            hostname,
            cluster,
        })
    }
}

fn first_nonempty<'a>(cands: [Option<&'a str>; 2]) -> Option<&'a str> {
    cands.into_iter().flatten().find(|s| !s.is_empty())
}

fn kernel_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// Compose Alloy Loki ingest is `:3500`; Pushgateway is `:9091` on the same host.
pub fn derive_from_loki(loki_url: Option<&str>) -> Option<String> {
    let raw = loki_url.map(str::trim).filter(|s| !s.is_empty())?;
    let (scheme, rest) = if let Some(r) = raw.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = raw.strip_prefix("http://") {
        ("http", r)
    } else {
        return None;
    };
    let hostport = rest.split('/').next().unwrap_or(rest);
    if hostport.starts_with('[') {
        return None;
    }
    let (host, port) = hostport.rsplit_once(':')?;
    if port != "3500" || host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}:9091"))
}

fn grouping_url(base: &str, hostname: &str, cluster: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.contains("/metrics/") {
        return base.to_string();
    }
    let inst = sanitize_label(hostname);
    let cl = sanitize_label(cluster);
    format!("{base}/metrics/job/pertisk/instance/{inst}/cluster/{cl}")
}

fn sanitize_label(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if t.is_empty() {
        "unknown".into()
    } else {
        t
    }
}

/// Remember CLI/env override for later config reloads.
pub fn init_prom_push_cli(push_url: Option<String>, loki_url: Option<String>) {
    let _ = CLI.set(PromCli {
        push_url: push_url.filter(|s| !s.trim().is_empty()),
        loki_url: loki_url.filter(|s| !s.trim().is_empty()),
    });
}

/// Start, stop, or restart the pusher from current machine config + CLI.
pub fn apply_prom_push(cfg: Option<&MachineConfig>, state: SharedState) {
    let cli = CLI.get().cloned().unwrap_or_default();
    let settings = PromPushSettings::resolve(cfg, cli.push_url.as_deref(), cli.loki_url.as_deref());
    let mut slot = HANDLE.lock().unwrap_or_else(|e| e.into_inner());
    match (settings, slot.as_ref().map(|h| h.settings.clone())) {
        (None, None) => {}
        (None, Some(_)) => {
            if let Some(h) = slot.take() {
                h.stop();
            }
            info!("prometheus push stopped");
        }
        (Some(next), prev) if prev.as_ref() == Some(&next) => {}
        (Some(next), _) => {
            if let Some(h) = slot.take() {
                h.stop();
            }
            info!(
                url = %next.url,
                hostname = %next.hostname,
                cluster = %next.cluster,
                "prometheus push starting"
            );
            *slot = Some(PromPushHandle::spawn(next, state));
        }
    }
}

struct PromPushHandle {
    settings: PromPushSettings,
    cancel: std::sync::Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl PromPushHandle {
    fn spawn(settings: PromPushSettings, state: SharedState) -> Self {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let cancel_t = cancel.clone();
        let settings_t = settings.clone();
        let join = thread::Builder::new()
            .name("pertisk-prom".into())
            .spawn(move || run_loop(settings_t, state, cancel_t))
            .ok();
        Self {
            settings,
            cancel,
            join,
        }
    }

    fn stop(mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn run_loop(settings: PromPushSettings, state: SharedState, cancel: std::sync::Arc<AtomicBool>) {
    let url = grouping_url(&settings.url, &settings.hostname, &settings.cluster);
    let mut backoff = Duration::from_secs(5);
    while !cancel.load(Ordering::Relaxed) {
        let body = render_metrics(&state);
        match push_once(&url, &body) {
            Ok(()) => backoff = Duration::from_secs(5),
            Err(err) => {
                warn!(error = %err, url = %url, "prometheus push failed");
                if sleep_or_cancel(&cancel, backoff) {
                    break;
                }
                backoff = (backoff * 2).min(Duration::from_secs(60));
                continue;
            }
        }
        if sleep_or_cancel(&cancel, PUSH_EVERY) {
            break;
        }
    }
}

fn push_once(url: &str, body: &str) -> Result<(), String> {
    let resp = ureq::put(url)
        .timeout(HTTP_TIMEOUT)
        .set("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
        .send_string(body)
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("HTTP {status}"))
    }
}

fn sleep_or_cancel(cancel: &AtomicBool, total: Duration) -> bool {
    let step = Duration::from_millis(200);
    let mut left = total;
    while left > Duration::ZERO {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        let chunk = left.min(step);
        thread::sleep(chunk);
        left -= chunk;
    }
    cancel.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_compose_alloy_loki_to_pushgateway() {
        assert_eq!(
            derive_from_loki(Some("http://10.1.1.150:3500/loki/api/v1/push")),
            Some("http://10.1.1.150:9091".into())
        );
        assert_eq!(
            derive_from_loki(Some("http://10.1.1.150:3100/loki/api/v1/push")),
            None
        );
        assert_eq!(derive_from_loki(Some("")), None);
    }

    #[test]
    fn grouping_url_appends_job_instance_cluster() {
        assert_eq!(
            grouping_url("http://10.1.1.150:9091/", "lab-cp-1", "lab"),
            "http://10.1.1.150:9091/metrics/job/pertisk/instance/lab-cp-1/cluster/lab"
        );
    }
}
