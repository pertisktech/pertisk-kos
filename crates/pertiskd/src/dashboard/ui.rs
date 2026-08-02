//! Console status banner (serial-safe).
//!
//! Avoids ratatui alternate-screen clears, which left Proxmox Serial blank when
//! the TTY reported 0×0 or draw failed.

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

/// Handle that stops the status thread.
pub struct DashboardHandle {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl DashboardHandle {
    #[allow(dead_code)]
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for DashboardHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub fn should_enable_dashboard(no_dashboard: bool, smoke: bool) -> bool {
    !no_dashboard && !smoke
}

/// Start a ~2s text status banner on stderr (serial).
pub fn start_dashboard(
    cfg: Option<MachineConfig>,
    state: SharedState,
    state_root: PathBuf,
    logs: LogRing,
) -> Option<DashboardHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = stop.clone();
    let join = thread::Builder::new()
        .name("pertisk-dashboard".into())
        .spawn(move || {
            // Never silence stderr — serial logs must keep flowing.
            while !stop_t.load(Ordering::SeqCst) {
                let snap = StatusSnapshot::collect(cfg.as_ref(), &state, &state_root);
                let recent = logs.tail(8);
                print_banner(&snap, &recent);
                for _ in 0..20 {
                    if stop_t.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        })
        .ok()?;
    Some(DashboardHandle {
        stop,
        join: Some(join),
    })
}

fn print_banner(snap: &StatusSnapshot, recent: &[String]) {
    let mut out = String::new();
    out.push_str("\r\n");
    out.push_str("======== PERTISK ========\r\n");
    out.push_str(&format!(
        "node     {}  v{}  {}\r\n",
        snap.hostname, snap.version, snap.message
    ));
    out.push_str(&format!(
        "cpu      {} ({} cores)\r\n",
        snap.cpu_model, snap.cpu_cores
    ));
    out.push_str(&format!(
        "mem      {} / {} (avail {})\r\n",
        format_kib(snap.mem_used_kb()),
        format_kib(snap.mem_total_kb),
        format_kib(snap.mem_available_kb)
    ));
    if snap.disks.is_empty() {
        out.push_str("disk     (n/a)\r\n");
    } else {
        for d in &snap.disks {
            out.push_str(&format!(
                "disk     {} {} / {}\r\n",
                d.label,
                format_bytes(d.used_bytes),
                format_bytes(d.total_bytes)
            ));
        }
    }
    if snap.interfaces.is_empty() {
        out.push_str("net      (no addresses)\r\n");
    } else {
        for iface in &snap.interfaces {
            if iface.addresses.is_empty() {
                out.push_str(&format!("net      {} (no IP)\r\n", iface.name));
            } else {
                for a in &iface.addresses {
                    out.push_str(&format!("net      {} {}\r\n", iface.name, a));
                }
            }
        }
    }
    out.push_str(&format!(
        "cluster  {}  cni={}  pod={}\r\n",
        snap.cluster_endpoint, snap.cni, snap.pod_cidr
    ));
    out.push_str(&format!(
        "runtime  containerd={} pid={}  kubelet={} pid={}\r\n",
        snap.containerd, snap.containerd_pid, snap.kubelet, snap.kubelet_pid
    ));
    out.push_str(&format!(
        "boot     slot={} ok={} attempts={}\r\n",
        snap.boot_slot, snap.boot_ok, snap.boot_attempts
    ));
    if !recent.is_empty() {
        out.push_str("---- recent logs ----\r\n");
        for line in recent {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("========================\r\n");
    let _ = io::stderr().write_all(out.as_bytes());
    let _ = io::stderr().flush();
}
