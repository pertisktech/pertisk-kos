//! Plain-text fallback mirroring node / network / resources / services / logs.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use pertisk_api::SharedState;
use pertisk_config::MachineConfig;

use crate::dashboard::snapshot::{format_bytes, format_kib, StatusSnapshot};
use crate::log_ring::LogRing;

fn pct(used: u64, total: u64) -> u16 {
    if total == 0 {
        return 0;
    }
    ((used.saturating_mul(100)) / total).min(100) as u16
}

/// Run a ~2s status loop until `stop` is set.
pub fn run_banner_loop(
    stop: Arc<AtomicBool>,
    cfg: Option<MachineConfig>,
    state: SharedState,
    state_root: PathBuf,
    logs: LogRing,
) {
    while !stop.load(Ordering::SeqCst) {
        let snap = StatusSnapshot::collect(cfg.as_ref(), &state, &state_root);
        let recent = logs.tail(16);
        let mut out = String::from("\x1b[2J\x1b[H");

        out.push_str("==== node ====\r\n");
        out.push_str(&format!(
            "{}  v{}  {}  {}\r\n",
            snap.hostname,
            snap.version,
            snap.machine_type,
            if snap.ready { "ready" } else { "not-ready" },
        ));
        if let Some(url) = crate::dashboard::mgmt_public_url() {
            out.push_str(&format!("mgmt {url}\r\n"));
        }

        out.push_str("==== network ====\r\n");
        if snap.node_iface.is_empty() && snap.node_ip == "-" {
            out.push_str("(no node ip)\r\n");
        } else if snap.node_iface.is_empty() {
            out.push_str(&format!("node {}\r\n", snap.node_ip));
        } else {
            out.push_str(&format!("node {} {}\r\n", snap.node_iface, snap.node_ip));
        }
        for row in &snap.net_rows {
            let name = row.split_whitespace().next().unwrap_or("");
            if name == snap.node_iface {
                continue;
            }
            out.push_str(row);
            out.push_str("\r\n");
        }
        out.push_str(&format!(
            "cluster {}\r\nKubernetes {}  cni {}  pod {}\r\n",
            snap.cluster_endpoint, snap.kubernetes_version, snap.cni, snap.pod_cidr
        ));

        out.push_str("==== resources ====\r\n");
        out.push_str(&format!(
            "cpu     [{}%]  {}c  load {:.2}\r\n",
            snap.cpu_usage_pct, snap.cpu_cores, snap.load_1m
        ));
        let mem_pct = pct(snap.mem_used_kb(), snap.mem_total_kb);
        out.push_str(&format!(
            "memory  [{}%]  {}/{}\r\n",
            mem_pct,
            format_kib(snap.mem_used_kb()),
            format_kib(snap.mem_total_kb)
        ));
        for (i, d) in snap.disks.iter().enumerate() {
            let name = if i == 0 { "disk" } else { d.label.as_str() };
            out.push_str(&format!(
                "{:<8}[{}%]  {}/{}  {}\r\n",
                name,
                pct(d.used_bytes, d.total_bytes),
                format_bytes(d.used_bytes),
                format_bytes(d.total_bytes),
                d.label
            ));
        }

        out.push_str("==== services ====\r\n");
        out.push_str(&format!("containerd  {}\r\n", snap.containerd));
        out.push_str(&format!("kubelet     {}\r\n", snap.kubelet));

        out.push_str("==== logs ====\r\n");
        if recent.is_empty() {
            out.push_str("(no logs)\r\n");
        } else {
            for line in &recent {
                out.push_str(line);
                out.push_str("\r\n");
            }
        }

        let _ = io::stderr().write_all(out.as_bytes());
        let _ = io::stderr().flush();
        for _ in 0..20 {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}
