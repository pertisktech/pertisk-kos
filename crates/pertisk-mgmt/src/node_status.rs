//! Live node status: Machine Health, Prometheus scrape, kubectl top.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

use crate::config::Config;
use crate::k8s::{kubeconfig_tls_error, refresh_kubeconfig_from_guest, resolve_cluster_kubeconfig};
use crate::routes::nodes::NodeOut;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct NodeStatusOut {
    pub node: NodeOut,
    pub health: HealthOut,
    pub metrics: MetricsOut,
    pub resources: ResourcesOut,
}

#[derive(Debug, Serialize)]
pub struct HealthOut {
    pub ready: Option<bool>,
    pub containerd: Option<String>,
    pub kubelet: Option<String>,
    pub message: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MetricsOut {
    pub gauges: HashMap<String, f64>,
    pub api: ApiMetricsOut,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiMetricsOut {
    pub requests_total: u64,
    pub duration_count: u64,
    pub duration_sum_seconds: f64,
    pub by_method: HashMap<String, u64>,
}

#[derive(Debug, Serialize)]
pub struct ResourcesOut {
    pub cpu: Option<String>,
    pub cpu_percent: Option<f64>,
    pub memory: Option<String>,
    pub memory_percent: Option<f64>,
    pub error: Option<String>,
}

impl HealthOut {
    fn err(msg: impl Into<String>) -> Self {
        Self {
            ready: None,
            containerd: None,
            kubelet: None,
            message: None,
            error: Some(msg.into()),
        }
    }
}

impl MetricsOut {
    fn err(msg: impl Into<String>) -> Self {
        Self {
            gauges: HashMap::new(),
            api: ApiMetricsOut {
                requests_total: 0,
                duration_count: 0,
                duration_sum_seconds: 0.0,
                by_method: HashMap::new(),
            },
            error: Some(msg.into()),
        }
    }
}

impl ResourcesOut {
    fn err(msg: impl Into<String>) -> Self {
        Self {
            cpu: None,
            cpu_percent: None,
            memory: None,
            memory_percent: None,
            error: Some(msg.into()),
        }
    }
}

/// Gather live status for a node (never fails hard — sections carry `error`).
pub async fn gather(state: &AppState, node: NodeOut, cluster_id: &str) -> NodeStatusOut {
    let Some(ip) = node.ip.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return NodeStatusOut {
            node,
            health: HealthOut::err("node has no IPv4 address yet"),
            metrics: MetricsOut::err("node has no IPv4 address yet"),
            resources: ResourcesOut::err("node has no IPv4 address yet"),
        };
    };

    let health_fut = fetch_health(state.cfg(), ip);
    let metrics_fut = scrape_metrics(&state.inner.metrics_http, ip, state.cfg());
    let resources_fut = fetch_resources(state, cluster_id, &node.name);

    let (health, metrics, resources) = tokio::join!(health_fut, metrics_fut, resources_fut);

    NodeStatusOut {
        node,
        health,
        metrics,
        resources,
    }
}

async fn fetch_health(cfg: &Config, ip: &str) -> HealthOut {
    if !cfg.pertiskctl.exists() {
        return HealthOut::err(format!(
            "pertiskctl not found at {}",
            cfg.pertiskctl.display()
        ));
    }
    let out = Command::new(&cfg.pertiskctl)
        .args(["-e", &format!("{ip}:50000"), "health"])
        .output()
        .await;
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            if !o.status.success() {
                let msg = stderr.trim();
                return HealthOut::err(if msg.is_empty() {
                    format!("pertiskctl health failed (exit {})", o.status)
                } else {
                    msg.to_string()
                });
            }
            parse_health_line(stdout.trim())
        }
        Err(e) => HealthOut::err(format!("pertiskctl: {e}")),
    }
}

/// Parse `ready=true containerd=up kubelet=up — message`.
fn parse_health_line(line: &str) -> HealthOut {
    let (main, message) = match line.split_once(" — ") {
        Some((a, b)) => (a, Some(b.to_string())),
        None => match line.split_once(" - ") {
            Some((a, b)) => (a, Some(b.to_string())),
            None => (line, None),
        },
    };
    let mut ready = None;
    let mut containerd = None;
    let mut kubelet = None;
    for part in main.split_whitespace() {
        if let Some((k, v)) = part.split_once('=') {
            match k {
                "ready" => ready = Some(v == "true" || v == "1"),
                "containerd" => containerd = Some(v.to_string()),
                "kubelet" => kubelet = Some(v.to_string()),
                _ => {}
            }
        }
    }
    if ready.is_none() && containerd.is_none() && kubelet.is_none() {
        return HealthOut::err(format!("unrecognized health output: {line}"));
    }
    HealthOut {
        ready,
        containerd,
        kubelet,
        message,
        error: None,
    }
}

async fn scrape_metrics(http: &reqwest::Client, ip: &str, cfg: &Config) -> MetricsOut {
    let scheme = if cfg.metrics_tls.is_some() {
        "https"
    } else {
        "http"
    };
    let url = format!("{scheme}://{ip}:50001/metrics");
    let mut req = http.get(&url).timeout(Duration::from_secs(2));
    if let Some(tok) = cfg.metrics_token.as_deref() {
        req = req.bearer_auth(tok);
    }
    match req.send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return MetricsOut::err(format!("metrics HTTP {}", resp.status()));
            }
            match resp.text().await {
                Ok(body) => parse_prometheus(&body),
                Err(e) => MetricsOut::err(format!("read metrics body: {e}")),
            }
        }
        Err(e) => MetricsOut::err(format!("scrape {url}: {e}")),
    }
}

fn parse_prometheus(body: &str) -> MetricsOut {
    let mut gauges = HashMap::new();
    let mut by_method = HashMap::new();
    let mut duration_sum_seconds = 0.0_f64;
    let mut duration_count = 0_u64;

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name_labels, value_s)) = split_metric_line(line) else {
            continue;
        };
        let Ok(value) = value_s.parse::<f64>() else {
            continue;
        };
        let (name, labels) = split_name_labels(name_labels);

        match name {
            "pertisk_api_requests_total" => {
                if let Some(method) = label_value(labels, "method") {
                    if !method.is_empty() {
                        by_method.insert(method, value as u64);
                    }
                }
            }
            "pertisk_api_request_duration_seconds_sum" => duration_sum_seconds = value,
            "pertisk_api_request_duration_seconds_count" => duration_count = value as u64,
            "pertisk_node_ready"
            | "pertisk_containerd_up"
            | "pertisk_kubelet_up"
            | "pertisk_boot_ok"
            | "pertisk_boot_attempts"
            | "pertisk_active_slot"
            | "pertisk_metrics_scrapes_total" => {
                gauges.insert(name.to_string(), value);
            }
            _ => {}
        }
    }

    let requests_total = by_method.values().copied().sum();
    MetricsOut {
        gauges,
        api: ApiMetricsOut {
            requests_total,
            duration_count,
            duration_sum_seconds,
            by_method,
        },
        error: None,
    }
}

fn split_metric_line(line: &str) -> Option<(&str, &str)> {
    let mut parts = line.rsplitn(2, char::is_whitespace);
    let value = parts.next()?.trim();
    let name_labels = parts.next()?.trim();
    if name_labels.is_empty() || value.is_empty() {
        return None;
    }
    Some((name_labels, value))
}

fn split_name_labels(s: &str) -> (&str, &str) {
    if let Some(i) = s.find('{') {
        (&s[..i], &s[i..])
    } else {
        (s, "")
    }
}

fn label_value(labels: &str, key: &str) -> Option<String> {
    // {method="Health"} or {method="Health",other="x"}
    let needle = format!("{key}=\"");
    let start = labels.find(&needle)? + needle.len();
    let rest = &labels[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

async fn fetch_resources(state: &AppState, cluster_id: &str, node_name: &str) -> ResourcesOut {
    let (kc, cluster_name) = match resolve_cluster_kubeconfig(state, cluster_id).await {
        Ok(v) => v,
        Err(e) => return ResourcesOut::err(e.to_string()),
    };
    let out = kubectl_top_node(&kc, node_name).await;
    match out {
        Ok(o) => parse_top_output(o, node_name),
        Err(e) if kubeconfig_tls_error(&e) => {
            let ip: Option<String> = sqlx::query_scalar::<_, Option<String>>(
                "SELECT ip FROM nodes WHERE cluster_id = ? AND role = 'controlplane' \
                 ORDER BY name LIMIT 1",
            )
            .bind(cluster_id)
            .fetch_optional(state.pool())
            .await
            .ok()
            .flatten()
            .flatten()
            .filter(|s| !s.is_empty());
            if let Some(ip) = ip {
                if refresh_kubeconfig_from_guest(&state.cfg().pertiskctl, &ip, &kc, &cluster_name)
                    .await
                    .is_ok()
                {
                    match kubectl_top_node(&kc, node_name).await {
                        Ok(o) => return parse_top_output(o, node_name),
                        Err(e2) => return ResourcesOut::err(e2),
                    }
                }
            }
            ResourcesOut::err(format!(
                "{e} — leftover kubeconfig CA from a previous cluster of this name?"
            ))
        }
        Err(e) => ResourcesOut::err(e),
    }
}

async fn kubectl_top_node(
    kc: &std::path::Path,
    node_name: &str,
) -> Result<std::process::Output, String> {
    let o = Command::new("kubectl")
        .args([
            "--kubeconfig",
            &kc.to_string_lossy(),
            "top",
            "node",
            node_name,
            "--no-headers",
        ])
        .output()
        .await
        .map_err(|e| format!("kubectl: {e}"))?;
    if !o.status.success() {
        let msg = String::from_utf8_lossy(&o.stderr);
        let msg = msg.trim();
        return Err(if msg.is_empty() {
            "kubectl top node failed (is metrics-server installed?)".into()
        } else {
            msg.to_string()
        });
    }
    Ok(o)
}

fn parse_top_output(o: std::process::Output, _node_name: &str) -> ResourcesOut {
    let stdout = String::from_utf8_lossy(&o.stdout);
    parse_kubectl_top(stdout.trim())
}

/// Parse `NAME CPU(cores) CPU% MEMORY(bytes) MEMORY%`.
fn parse_kubectl_top(line: &str) -> ResourcesOut {
    let parts: Vec<&str> = line.split_whitespace().collect();
    // name cpu cpu% mem mem%
    if parts.len() < 5 {
        return ResourcesOut::err(format!("unexpected kubectl top output: {line}"));
    }
    let cpu = parts[1].to_string();
    let cpu_percent = parse_percent(parts[2]);
    let memory = parts[3].to_string();
    let memory_percent = parse_percent(parts[4]);
    ResourcesOut {
        cpu: Some(cpu),
        cpu_percent,
        memory: Some(memory),
        memory_percent,
        error: None,
    }
}

fn parse_percent(s: &str) -> Option<f64> {
    s.trim_end_matches('%').parse().ok()
}

#[derive(Debug, Serialize)]
pub struct NodeLogsOut {
    pub service: String,
    pub source: Option<String>,
    pub lines: Vec<String>,
    pub error: Option<String>,
}

const ALLOWED_LOG_SERVICES: &[&str] = &["pertiskd", "containerd", "kubelet", "dmesg"];

/// Tail guest logs via `pertiskctl logs`.
pub async fn fetch_logs(state: &AppState, node: &NodeOut, service: &str, tail: u32) -> NodeLogsOut {
    let service = service.trim().to_ascii_lowercase();
    if !ALLOWED_LOG_SERVICES.contains(&service.as_str()) {
        return NodeLogsOut {
            service,
            source: None,
            lines: vec![],
            error: Some(format!(
                "unknown service (want {})",
                ALLOWED_LOG_SERVICES.join("|")
            )),
        };
    }
    let tail = tail.clamp(1, 2000);
    let Some(ip) = node.ip.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return NodeLogsOut {
            service,
            source: None,
            lines: vec![],
            error: Some("node has no IPv4 address yet".into()),
        };
    };
    let cfg = state.cfg();
    if !cfg.pertiskctl.exists() {
        return NodeLogsOut {
            service,
            source: None,
            lines: vec![],
            error: Some(format!(
                "pertiskctl not found at {}",
                cfg.pertiskctl.display()
            )),
        };
    }

    let out = Command::new(&cfg.pertiskctl)
        .args([
            "-e",
            &format!("{ip}:50000"),
            "logs",
            &service,
            "-n",
            &tail.to_string(),
        ])
        .output()
        .await;

    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            if !o.status.success() {
                let msg = stderr.trim();
                return NodeLogsOut {
                    service,
                    source: None,
                    lines: vec![],
                    error: Some(if msg.is_empty() {
                        format!("pertiskctl logs failed (exit {})", o.status)
                    } else {
                        msg.to_string()
                    }),
                };
            }
            // stderr has "# service from source"; stdout is the lines.
            let source = stderr.lines().find_map(|l| {
                let t = l.trim().trim_start_matches('#').trim();
                t.strip_prefix(&format!("{service} from "))
                    .map(|s| s.trim().to_string())
                    .or_else(|| {
                        if t.contains(" from ") {
                            Some(t.to_string())
                        } else {
                            None
                        }
                    })
            });
            let lines: Vec<String> = stdout
                .lines()
                .map(|l| l.to_string())
                .filter(|l| !l.is_empty())
                .collect();
            NodeLogsOut {
                service,
                source,
                lines,
                error: None,
            }
        }
        Err(e) => NodeLogsOut {
            service,
            source: None,
            lines: vec![],
            error: Some(format!("pertiskctl: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_health() {
        let h = parse_health_line("ready=true containerd=up kubelet=up — all good");
        assert_eq!(h.ready, Some(true));
        assert_eq!(h.containerd.as_deref(), Some("up"));
        assert_eq!(h.kubelet.as_deref(), Some("up"));
        assert_eq!(h.message.as_deref(), Some("all good"));
        assert!(h.error.is_none());
    }

    #[test]
    fn parses_prometheus_api() {
        let body = r#"
# HELP pertisk_node_ready x
# TYPE pertisk_node_ready gauge
pertisk_node_ready 1
pertisk_containerd_up 1
pertisk_api_requests_total{method="Health"} 10
pertisk_api_requests_total{method="Version"} 2
pertisk_api_request_duration_seconds_sum 0.05
pertisk_api_request_duration_seconds_count 12
"#;
        let m = parse_prometheus(body);
        assert!(m.error.is_none());
        assert_eq!(m.gauges.get("pertisk_node_ready"), Some(&1.0));
        assert_eq!(m.api.requests_total, 12);
        assert_eq!(m.api.by_method.get("Health"), Some(&10));
        assert!((m.api.duration_sum_seconds - 0.05).abs() < 1e-9);
        assert_eq!(m.api.duration_count, 12);
    }

    #[test]
    fn parses_top() {
        let r = parse_kubectl_top("lab-cp-1   250m   12%   1024Mi   25%");
        assert_eq!(r.cpu.as_deref(), Some("250m"));
        assert_eq!(r.cpu_percent, Some(12.0));
        assert_eq!(r.memory.as_deref(), Some("1024Mi"));
        assert_eq!(r.memory_percent, Some(25.0));
    }
}
