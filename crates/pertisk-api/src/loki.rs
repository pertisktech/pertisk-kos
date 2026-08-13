//! Optional Loki push from node log files (same sources as `pertiskctl logs`).
//!
//! Disabled unless `PERTISK_LOKI_URL` or `machine.observability.lokiUrl` is set.
//! POSTs `/loki/api/v1/push` JSON (Grafana Alloy `loki.source.api` accepts this).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pertisk_config::MachineConfig;
use tracing::{info, warn};

use crate::logs::log_file_path;

const SERVICES: &[&str] = &["pertiskd", "containerd", "kubelet", "dmesg"];
const FLUSH_EVERY: Duration = Duration::from_secs(2);
const MAX_BATCH: usize = 200;
const MAX_LINE: usize = 8192;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

static CLI: OnceLock<LokiCli> = OnceLock::new();
static HANDLE: Mutex<Option<LokiPushHandle>> = Mutex::new(None);

#[derive(Debug, Clone, Default)]
struct LokiCli {
    url: Option<String>,
    token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LokiSettings {
    pub url: String,
    pub token: Option<String>,
    pub hostname: String,
    pub cluster: String,
    pub extra_labels: BTreeMap<String, String>,
}

impl LokiSettings {
    pub fn resolve(
        cfg: Option<&MachineConfig>,
        cli_url: Option<&str>,
        cli_token: Option<&str>,
    ) -> Option<Self> {
        let yaml = cfg.and_then(|c| c.machine.observability.as_ref());
        let url = first_nonempty([
            cli_url.map(str::trim),
            yaml.and_then(|o| o.loki_url.as_deref().map(str::trim)),
        ])?;
        let token = first_nonempty([
            cli_token.map(str::trim),
            yaml.and_then(|o| o.loki_token.as_deref().map(str::trim)),
        ]);
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
            url: url.to_string(),
            token: token.map(str::to_string),
            hostname,
            cluster,
            extra_labels: yaml.map(|o| o.extra_labels.clone()).unwrap_or_default(),
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

/// Remember CLI/env overrides for later config reloads.
pub fn init_loki_cli(url: Option<String>, token: Option<String>) {
    let _ = CLI.set(LokiCli { url, token });
}

/// Start, stop, or restart the pusher from current machine config + CLI.
pub fn apply_loki_push(cfg: Option<&MachineConfig>, state_root: &Path) {
    let cli = CLI.get().cloned().unwrap_or_default();
    let settings = LokiSettings::resolve(
        cfg,
        cli.url.as_deref(),
        cli.token.as_deref(),
    );
    let mut slot = HANDLE.lock().unwrap_or_else(|e| e.into_inner());
    match (settings, slot.as_ref().map(|h| h.settings.clone())) {
        (None, None) => {}
        (None, Some(_)) => {
            if let Some(h) = slot.take() {
                h.stop();
            }
            info!("loki push stopped");
        }
        (Some(next), prev) if prev.as_ref() == Some(&next) => {}
        (Some(next), _) => {
            if let Some(h) = slot.take() {
                h.stop();
            }
            info!(url = %next.url, hostname = %next.hostname, cluster = %next.cluster, "loki push starting");
            *slot = Some(LokiPushHandle::spawn(next, state_root.to_path_buf()));
        }
    }
}

struct LokiPushHandle {
    settings: LokiSettings,
    cancel: std::sync::Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl LokiPushHandle {
    fn spawn(settings: LokiSettings, state_root: PathBuf) -> Self {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let cancel_t = cancel.clone();
        let settings_t = settings.clone();
        let join = thread::Builder::new()
            .name("pertisk-loki".into())
            .spawn(move || run_loop(settings_t, state_root, cancel_t))
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

struct FileTail {
    path: PathBuf,
    pos: u64,
    partial: String,
}

fn run_loop(settings: LokiSettings, state_root: PathBuf, cancel: std::sync::Arc<AtomicBool>) {
    let mut files: Vec<(&str, FileTail)> = SERVICES
        .iter()
        .filter_map(|svc| {
            let path = log_file_path(&state_root, svc)?;
            Some((
                *svc,
                FileTail {
                    path,
                    pos: 0,
                    partial: String::new(),
                },
            ))
        })
        .collect();
    for (_, tail) in &mut files {
        tail.pos = File::open(&tail.path)
            .and_then(|f| f.metadata())
            .map(|m| m.len())
            .unwrap_or(0);
    }

    let mut pending: BTreeMap<String, Vec<(u128, String)>> = BTreeMap::new();
    let mut dmesg_last: Vec<String> = Vec::new();
    let mut dmesg_primed = false;
    let mut backoff = Duration::from_secs(5);
    let mut last_flush = Instant::now();

    while !cancel.load(Ordering::Relaxed) {
        for (svc, tail) in &mut files {
            for line in read_new_lines(tail) {
                pending.entry((*svc).into()).or_default().push(stamp_line(line));
            }
        }
        match poll_dmesg() {
            Some(snap) if !dmesg_primed => {
                dmesg_last = snap;
                dmesg_primed = true;
            }
            Some(snap) => {
                if snap.len() > dmesg_last.len() && snap[..dmesg_last.len()] == *dmesg_last {
                    for line in snap[dmesg_last.len()..].iter().cloned() {
                        pending.entry("dmesg".into()).or_default().push(stamp_line(line));
                    }
                }
                dmesg_last = snap;
            }
            None => {}
        }

        let n: usize = pending.values().map(Vec::len).sum();
        let due = last_flush.elapsed() >= FLUSH_EVERY || n >= MAX_BATCH;
        if due && n > 0 {
            let body = encode_push(&settings, &pending);
            match post_loki(&settings.url, settings.token.as_deref(), &body) {
                Ok(()) => {
                    pending.clear();
                    last_flush = Instant::now();
                    backoff = Duration::from_secs(5);
                }
                Err(err) => {
                    warn!(error = %err, "loki push failed");
                    thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                    last_flush = Instant::now();
                }
            }
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn stamp_line(line: String) -> (u128, String) {
    let mut line = line;
    if line.len() > MAX_LINE {
        line.truncate(MAX_LINE);
    }
    (now_ns(), line)
}

fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn read_new_lines(tail: &mut FileTail) -> Vec<String> {
    let mut f = match File::open(&tail.path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let meta_len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if meta_len < tail.pos {
        tail.pos = 0;
        tail.partial.clear();
    }
    if f.seek(SeekFrom::Start(tail.pos)).is_err() {
        return Vec::new();
    }
    let mut buf = String::new();
    let n = f.read_to_string(&mut buf).unwrap_or(0);
    if n == 0 {
        return Vec::new();
    }
    tail.pos += n as u64;
    tail.partial.push_str(&buf);
    let mut lines = Vec::new();
    while let Some(idx) = tail.partial.find('\n') {
        let mut line = tail.partial[..idx].to_string();
        if line.ends_with('\r') {
            line.pop();
        }
        if !line.is_empty() {
            lines.push(line);
        }
        tail.partial = tail.partial[idx + 1..].to_string();
    }
    lines
}

fn poll_dmesg() -> Option<Vec<String>> {
    let out = Command::new("dmesg").arg("-T").output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

pub fn encode_push(
    settings: &LokiSettings,
    pending: &BTreeMap<String, Vec<(u128, String)>>,
) -> serde_json::Value {
    let streams: Vec<serde_json::Value> = pending
        .iter()
        .filter(|(_, lines)| !lines.is_empty())
        .map(|(service, lines)| {
            let mut labels = BTreeMap::new();
            labels.insert("job".into(), "pertisk".into());
            labels.insert("service".into(), service.clone());
            labels.insert("hostname".into(), settings.hostname.clone());
            labels.insert("cluster".into(), settings.cluster.clone());
            for (k, v) in &settings.extra_labels {
                labels.insert(k.clone(), v.clone());
            }
            let values: Vec<serde_json::Value> = lines
                .iter()
                .map(|(ns, line)| serde_json::json!([ns.to_string(), line]))
                .collect();
            serde_json::json!({ "stream": labels, "values": values })
        })
        .collect();
    serde_json::json!({ "streams": streams })
}

fn post_loki(url: &str, token: Option<&str>, body: &serde_json::Value) -> Result<(), String> {
    let mut req = ureq::post(url)
        .set("Content-Type", "application/json")
        .timeout(HTTP_TIMEOUT);
    if let Some(t) = token.filter(|s| !s.is_empty()) {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    match req.send_json(body.clone()) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("HTTP {code}: {text}"))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pertisk_config::MachineConfig;

    #[test]
    fn resolve_prefers_cli_over_yaml() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
  network:
    hostname: node-1
  observability:
    lokiUrl: http://yaml:3100/loki/api/v1/push
cluster:
  name: lab
  endpoint: https://10.0.0.1:6443
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        let s = LokiSettings::resolve(Some(&cfg), Some("http://cli:3500/loki/api/v1/push"), None)
            .unwrap();
        assert_eq!(s.url, "http://cli:3500/loki/api/v1/push");
        assert_eq!(s.hostname, "node-1");
        assert_eq!(s.cluster, "lab");
    }

    #[test]
    fn resolve_disabled_when_empty() {
        let yaml = r#"
version: v1alpha1
machine:
  type: worker
"#;
        let cfg = MachineConfig::from_yaml(yaml).unwrap();
        assert!(LokiSettings::resolve(Some(&cfg), None, None).is_none());
        assert!(LokiSettings::resolve(Some(&cfg), Some("  "), None).is_none());
    }

    #[test]
    fn encode_push_json_shape() {
        let settings = LokiSettings {
            url: "http://loki/loki/api/v1/push".into(),
            token: None,
            hostname: "n1".into(),
            cluster: "lab".into(),
            extra_labels: BTreeMap::from([("env".into(), "dev".into())]),
        };
        let mut pending = BTreeMap::new();
        pending.insert("pertiskd".into(), vec![(1_700_000_000_000_000_000, "hello".into())]);
        let body = encode_push(&settings, &pending);
        let stream = &body["streams"][0];
        assert_eq!(stream["stream"]["job"], "pertisk");
        assert_eq!(stream["stream"]["service"], "pertiskd");
        assert_eq!(stream["stream"]["hostname"], "n1");
        assert_eq!(stream["stream"]["cluster"], "lab");
        assert_eq!(stream["stream"]["env"], "dev");
        assert_eq!(stream["values"][0][0], "1700000000000000000");
        assert_eq!(stream["values"][0][1], "hello");
    }
}
