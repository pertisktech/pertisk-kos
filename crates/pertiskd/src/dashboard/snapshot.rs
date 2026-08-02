//! Collect a one-shot status snapshot for the console dashboard.

use std::path::Path;

use pertisk_api::SharedState;
use pertisk_config::MachineConfig;
use pertisk_update::BootMeta;

#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub hostname: String,
    pub version: String,
    pub ready: bool,
    pub message: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub mem_total_kb: u64,
    pub mem_available_kb: u64,
    pub disks: Vec<DiskUsage>,
    pub interfaces: Vec<IfaceAddrs>,
    pub machine_type: String,
    pub cluster_endpoint: String,
    pub cni: String,
    pub pod_cidr: String,
    pub containerd: String,
    pub containerd_pid: u32,
    pub kubelet: String,
    pub kubelet_pid: u32,
    pub boot_slot: String,
    pub boot_ok: bool,
    pub boot_attempts: u32,
}

#[derive(Debug, Clone, Default)]
pub struct DiskUsage {
    pub label: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct IfaceAddrs {
    pub name: String,
    pub addresses: Vec<String>,
}

impl StatusSnapshot {
    pub fn collect(
        cfg: Option<&MachineConfig>,
        state: &SharedState,
        state_root: &Path,
    ) -> Self {
        let mut snap = Self::default();

        if let Ok(st) = state.lock() {
            snap.version = st.version.clone();
            snap.ready = st.ready;
            snap.message = st.message.clone();
            snap.containerd = st.containerd.clone();
            snap.containerd_pid = st.containerd_pid;
            snap.kubelet = st.kubelet.clone();
            snap.kubelet_pid = st.kubelet_pid;
        }

        if let Some(cfg) = cfg {
            snap.hostname = cfg
                .machine
                .network
                .hostname
                .clone()
                .unwrap_or_else(|| "pertisk".into());
            snap.machine_type = format!("{:?}", cfg.machine.machine_type).to_lowercase();
            if let Some(ref cluster) = cfg.cluster {
                snap.cluster_endpoint = cluster.endpoint.clone();
                snap.cni = cluster.cni.as_str().to_string();
                snap.pod_cidr = cluster
                    .pod_cidr
                    .clone()
                    .unwrap_or_else(|| "-".into());
            } else {
                snap.cluster_endpoint = "(none)".into();
                snap.cni = "-".into();
                snap.pod_cidr = "-".into();
            }
            for iface in &cfg.machine.network.interfaces {
                let addrs = pertisk_net::list_addresses(&iface.interface).unwrap_or_default();
                snap.interfaces.push(IfaceAddrs {
                    name: iface.interface.clone(),
                    addresses: addrs,
                });
            }
        } else {
            snap.hostname = read_hostname().unwrap_or_else(|| "pertisk".into());
            snap.machine_type = "-".into();
            snap.cluster_endpoint = "(no config)".into();
        }

        if snap.interfaces.is_empty() {
            // Show whatever NICs exist (eth0 / ens18 / …) even without config.
            if let Ok(names) = pertisk_net::list_interfaces() {
                for name in names {
                    let addrs = pertisk_net::list_addresses(&name).unwrap_or_default();
                    snap.interfaces.push(IfaceAddrs { name, addresses: addrs });
                }
            }
        }

        let (model, cores) = parse_cpuinfo(&read_to_string("/proc/cpuinfo").unwrap_or_default());
        snap.cpu_model = model;
        snap.cpu_cores = cores;

        let (total, avail) = parse_meminfo(&read_to_string("/proc/meminfo").unwrap_or_default());
        snap.mem_total_kb = total;
        snap.mem_available_kb = avail;

        if let Some(d) = disk_usage("STATE", state_root) {
            snap.disks.push(d);
        }
        if let Some(d) = disk_usage("root", Path::new("/")) {
            if snap
                .disks
                .first()
                .map(|x| x.total_bytes != d.total_bytes || x.used_bytes != d.used_bytes)
                .unwrap_or(true)
            {
                snap.disks.push(d);
            }
        }

        if let Ok(meta) = BootMeta::load(state_root) {
            snap.boot_slot = meta.active.to_string();
            snap.boot_ok = meta.boot_ok;
            snap.boot_attempts = meta.boot_attempts;
        }

        snap
    }

    pub fn mem_used_kb(&self) -> u64 {
        self.mem_total_kb.saturating_sub(self.mem_available_kb)
    }
}

fn read_to_string(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read_hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Parse `/proc/cpuinfo` — exposed for unit tests.
pub fn parse_cpuinfo(text: &str) -> (String, u32) {
    let mut model = String::from("unknown");
    let mut cores = 0u32;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("model name") {
            if let Some(v) = rest.split(':').nth(1) {
                model = v.trim().to_string();
            }
        } else if line.starts_with("processor") {
            cores += 1;
        }
        // ARM virt
        if model == "unknown" {
            if let Some(rest) = line.strip_prefix("Hardware") {
                if let Some(v) = rest.split(':').nth(1) {
                    model = v.trim().to_string();
                }
            }
        }
    }
    if cores == 0 {
        cores = 1;
    }
    (model, cores)
}

/// Parse `/proc/meminfo` — exposed for unit tests.
pub fn parse_meminfo(text: &str) -> (u64, u64) {
    let mut total = 0u64;
    let mut available = 0u64;
    let mut free = 0u64;
    let mut buffers = 0u64;
    let mut cached = 0u64;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or("");
        let val: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "MemTotal:" => total = val,
            "MemAvailable:" => available = val,
            "MemFree:" => free = val,
            "Buffers:" => buffers = val,
            "Cached:" => cached = val,
            _ => {}
        }
    }
    if available == 0 {
        available = free.saturating_add(buffers).saturating_add(cached);
    }
    (total, available)
}

fn disk_usage(label: &str, path: &Path) -> Option<DiskUsage> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        let cpath = CString::new(path.to_str()?).ok()?;
        // SAFETY: path is a valid CString; statvfs fills the struct.
        unsafe {
            let mut s: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(cpath.as_ptr(), &mut s) != 0 {
                return None;
            }
            let frsize = s.f_frsize as u64;
            let total = (s.f_blocks as u64).saturating_mul(frsize);
            let free = (s.f_bavail as u64).saturating_mul(frsize);
            let used = total.saturating_sub(free);
            Some(DiskUsage {
                label: label.into(),
                total_bytes: total,
                used_bytes: used,
            })
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (label, path);
        None
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_kib(kib: u64) -> String {
    format_bytes(kib.saturating_mul(1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpuinfo_x86() {
        let sample = "\
processor\t: 0
model name\t: Intel(R) Core(TM)
processor\t: 1
model name\t: Intel(R) Core(TM)
";
        let (model, cores) = parse_cpuinfo(sample);
        assert!(model.contains("Intel"));
        assert_eq!(cores, 2);
    }

    #[test]
    fn meminfo_basic() {
        let sample = "\
MemTotal:       16384000 kB
MemFree:         2048000 kB
MemAvailable:    8192000 kB
Buffers:          102400 kB
Cached:          2048000 kB
";
        let (total, avail) = parse_meminfo(sample);
        assert_eq!(total, 16384000);
        assert_eq!(avail, 8192000);
    }
}
