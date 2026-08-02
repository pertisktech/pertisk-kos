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
    /// Display rows like ptkube: `eth0  up  10.1.1.192/24`.
    pub net_rows: Vec<String>,
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
        } else {
            snap.hostname = read_hostname().unwrap_or_else(|| "pertisk".into());
            snap.machine_type = "-".into();
            snap.cluster_endpoint = "(no config)".into();
        }

        // Always discover live NICs (config names alone miss ens18 vs eth0).
        let (ifaces, rows) = collect_network();
        snap.interfaces = ifaces;
        snap.net_rows = rows;

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

/// Collect interfaces + display rows (ptkube-style). Prefer `ip -br addr`, else getifaddrs.
fn collect_network() -> (Vec<IfaceAddrs>, Vec<String>) {
    if let Some(rows) = read_addresses_ip_br() {
        let ifaces = rows_to_ifaces(&rows);
        return (ifaces, rows);
    }
    let ifaces = read_addresses_getifaddrs();
    let ifaces = if ifaces.is_empty() {
        read_addresses_netlink()
    } else {
        ifaces
    };
    let rows = ifaces_to_rows(&ifaces);
    (ifaces, rows)
}

/// Same approach as ptkube-dashboard `read_addresses`.
fn read_addresses_ip_br() -> Option<Vec<String>> {
    use std::process::Command;
    for bin in ["/sbin/ip", "/usr/sbin/ip", "ip"] {
        let Ok(out) = Command::new(bin).args(["-br", "addr"]).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let mut rows = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(iface) = parts.next() else {
                continue;
            };
            if iface == "lo" {
                continue;
            }
            let state = parts.next().unwrap_or("?");
            let addrs: Vec<_> = parts.collect();
            if addrs.is_empty() {
                rows.push(format!("{iface}  {state}  (no ip)"));
            } else {
                rows.push(format!("{iface}  {state}  {}", addrs.join("  ")));
            }
        }
        if !rows.is_empty() {
            return Some(rows);
        }
    }
    None
}

fn rows_to_ifaces(rows: &[String]) -> Vec<IfaceAddrs> {
    let mut out = Vec::new();
    for row in rows {
        let mut parts = row.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let _state = parts.next();
        let addresses: Vec<String> = parts
            .filter(|p| *p != "(no" && *p != "ip)")
            .map(|s| s.to_string())
            .collect();
        out.push(IfaceAddrs {
            name: name.to_string(),
            addresses,
        });
    }
    out
}

fn ifaces_to_rows(ifaces: &[IfaceAddrs]) -> Vec<String> {
    if ifaces.is_empty() {
        return vec!["(no interfaces)".into()];
    }
    ifaces
        .iter()
        .map(|i| {
            let state = operstate(&i.name);
            if i.addresses.is_empty() {
                format!("{}  {}  (no ip)", i.name, state)
            } else {
                format!("{}  {}  {}", i.name, state, i.addresses.join("  "))
            }
        })
        .collect()
}

fn operstate(iface: &str) -> String {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/operstate"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into())
}

fn read_addresses_netlink() -> Vec<IfaceAddrs> {
    let Ok(names) = pertisk_net::list_interfaces() else {
        return Vec::new();
    };
    names
        .into_iter()
        .map(|name| {
            let addresses = pertisk_net::list_addresses(&name).unwrap_or_default();
            IfaceAddrs { name, addresses }
        })
        .collect()
}

/// libc getifaddrs — works without iproute2 / netlink runtime.
#[cfg(target_os = "linux")]
fn read_addresses_getifaddrs() -> Vec<IfaceAddrs> {
    use std::collections::BTreeMap;
    use std::ffi::CStr;
    use std::net::{Ipv4Addr, Ipv6Addr};

    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name != "lo" {
                map.entry(name).or_default();
            }
        }
    }

    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return map
                .into_iter()
                .map(|(name, addresses)| IfaceAddrs { name, addresses })
                .collect();
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let entry = &*cur;
            if entry.ifa_addr.is_null() || entry.ifa_name.is_null() {
                cur = entry.ifa_next;
                continue;
            }
            let name = CStr::from_ptr(entry.ifa_name)
                .to_string_lossy()
                .into_owned();
            if name == "lo" {
                cur = entry.ifa_next;
                continue;
            }
            let family = i32::from((*entry.ifa_addr).sa_family);
            let addr = if family == libc::AF_INET {
                let sin = &*(entry.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if ip.is_unspecified() {
                    None
                } else {
                    let prefix = if !entry.ifa_netmask.is_null() {
                        let mask = &*(entry.ifa_netmask as *const libc::sockaddr_in);
                        u32::from_be(mask.sin_addr.s_addr).count_ones() as u8
                    } else {
                        32
                    };
                    Some(format!("{ip}/{prefix}"))
                }
            } else if family == libc::AF_INET6 {
                let sin6 = &*(entry.ifa_addr as *const libc::sockaddr_in6);
                let octets = sin6.sin6_addr.s6_addr;
                let ip = Ipv6Addr::from(octets);
                if ip.is_unspecified() {
                    None
                } else {
                    let prefix = if !entry.ifa_netmask.is_null() {
                        let mask = &*(entry.ifa_netmask as *const libc::sockaddr_in6);
                        mask.sin6_addr.s6_addr.iter().map(|b| b.count_ones()).sum::<u32>() as u8
                    } else {
                        128
                    };
                    Some(format!("{ip}/{prefix}"))
                }
            } else {
                None
            };
            if let Some(a) = addr {
                let list = map.entry(name).or_default();
                if !list.contains(&a) {
                    list.push(a);
                }
            }
            cur = entry.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }

    // Prefer IPv4 first within each iface.
    for addrs in map.values_mut() {
        addrs.sort_by(|a, b| {
            let a4 = a.contains('.');
            let b4 = b.contains('.');
            b4.cmp(&a4)
        });
    }

    map.into_iter()
        .map(|(name, addresses)| IfaceAddrs { name, addresses })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn read_addresses_getifaddrs() -> Vec<IfaceAddrs> {
    Vec::new()
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
