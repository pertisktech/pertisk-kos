//! Collect a one-shot status snapshot for the console dashboard.

use std::path::Path;

use pertisk_api::SharedState;
use pertisk_config::{Cluster, MachineConfig};
use pertisk_update::BootMeta;

#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub hostname: String,
    pub version: String,
    pub ready: bool,
    pub message: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    /// 0–100 from `/proc/stat` idle delta between collects; 0 until second sample.
    pub cpu_usage_pct: u16,
    /// 1-minute load average from `/proc/loadavg`.
    pub load_1m: f32,
    pub uptime_secs: u64,
    pub process_count: usize,
    pub mem_total_kb: u64,
    pub mem_available_kb: u64,
    pub disks: Vec<DiskUsage>,
    pub interfaces: Vec<IfaceAddrs>,
    /// Host NIC rows only (CNI/veth filtered). Format: `eth0  up  10.1.1.192/24`.
    pub net_rows: Vec<String>,
    /// Primary host iface name (e.g. `eth0`), or empty.
    pub node_iface: String,
    /// First IPv4 CIDR on the primary host iface (e.g. `10.1.1.198/24`), or `-`.
    pub node_ip: String,
    pub machine_type: String,
    pub cluster_endpoint: String,
    pub cni: String,
    /// Cluster-wide pod network CIDR from `cluster.podSubnet` (e.g. `10.244.0.0/16`).
    pub pod_cidr: String,
    /// Cluster service CIDR from `cluster.serviceSubnet` (e.g. `10.96.0.0/12`).
    pub service_subnet: String,
    /// From `cluster.kubernetesVersion`, or `-`.
    pub kubernetes_version: String,
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
    pub fn collect(cfg: Option<&MachineConfig>, state: &SharedState, state_root: &Path) -> Self {
        let mut snap = Self::default();

        let (root, config_path) = if let Ok(st) = state.lock() {
            snap.version = st.version.clone();
            snap.ready = st.ready;
            snap.message = st.message.clone();
            snap.containerd = st.containerd.clone();
            snap.containerd_pid = st.containerd_pid;
            snap.kubelet = st.kubelet.clone();
            snap.kubelet_pid = st.kubelet_pid;
            (st.state_root.clone(), st.config_path.clone())
        } else {
            (state_root.to_path_buf(), state_root.join("config.yaml"))
        };

        // Prefer live STATE config so an early-started dashboard picks up apply/bootstrap.
        let disk_cfg = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|y| MachineConfig::from_yaml(&y).ok());
        let effective = disk_cfg.as_ref().or(cfg);

        let mut configured_ifaces: Vec<String> = Vec::new();
        if let Some(cfg) = effective {
            snap.hostname = cfg
                .machine
                .network
                .hostname
                .clone()
                .unwrap_or_else(|| "pertisk".into());
            snap.machine_type = format!("{:?}", cfg.machine.machine_type).to_lowercase();
            configured_ifaces = cfg
                .machine
                .network
                .interfaces
                .iter()
                .map(|i| i.interface.clone())
                .collect();
            if let Some(ref cluster) = cfg.cluster {
                snap.cluster_endpoint = cluster.endpoint.clone();
                (snap.cni, snap.pod_cidr, snap.service_subnet) = cluster_network_display(cluster);
                snap.kubernetes_version = cluster
                    .kubernetes_version
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "-".into());
            } else {
                snap.cluster_endpoint = "(none)".into();
                snap.cni = "-".into();
                snap.pod_cidr = "-".into();
                snap.service_subnet = "-".into();
                snap.kubernetes_version = "-".into();
            }
        } else {
            snap.hostname = read_hostname().unwrap_or_else(|| "pertisk".into());
            snap.machine_type = "-".into();
            snap.cluster_endpoint = "(no config)".into();
            snap.cni = "-".into();
            snap.pod_cidr = "-".into();
            snap.service_subnet = "-".into();
            snap.kubernetes_version = "-".into();
        }

        // Always discover live NICs; drop CNI/veth so the panel keeps the host IP.
        let (ifaces, rows) = collect_network();
        let prioritized = prioritize_host_network(&ifaces, &rows, &configured_ifaces);
        snap.interfaces = prioritized.interfaces;
        snap.net_rows = prioritized.net_rows;
        snap.node_iface = prioritized.node_iface;
        snap.node_ip = prioritized.node_ip;

        let (model, cores) = parse_cpuinfo(&read_to_string("/proc/cpuinfo").unwrap_or_default());
        snap.cpu_model = model;
        snap.cpu_cores = cores;
        snap.cpu_usage_pct = sample_cpu_usage_pct();
        snap.load_1m = parse_loadavg(&read_to_string("/proc/loadavg").unwrap_or_default());
        snap.uptime_secs = parse_uptime(&read_to_string("/proc/uptime").unwrap_or_default());
        snap.process_count = process_count();

        let (total, avail) = parse_meminfo(&read_to_string("/proc/meminfo").unwrap_or_default());
        snap.mem_total_kb = total;
        snap.mem_available_kb = avail;

        push_disk_unique(&mut snap.disks, disk_usage("STATE", &root));
        // EPHEMERAL is bound over /var after boot — show usable container space.
        push_disk_unique(&mut snap.disks, disk_usage("EPHEMERAL", Path::new("/var")));
        push_disk_unique(&mut snap.disks, disk_usage("root", Path::new("/")));

        if let Ok(meta) = BootMeta::load(&root) {
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

fn cluster_network_display(cluster: &Cluster) -> (String, String, String) {
    let cni = match cluster.cni.as_str() {
        "none" => "external".into(),
        configured => configured.to_string(),
    };
    // Keep dual-stack CIDRs visible (v4,v6) — do not strip IPv6.
    let pod_subnet = display_cidrs(&cluster.cluster_cidr());
    let service_subnet = display_cidrs(&cluster.effective_service_subnets().join(","));
    (cni, pod_subnet, service_subnet)
}

/// Join CIDR list for the dashboard; `-` if empty.
fn display_cidrs(raw: &str) -> String {
    let parts: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty() && *c != "-")
        .collect();
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(",")
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
            let addrs: Vec<_> = parts
                .filter(|a| {
                    // Hide IPv6 link-local so fe80:: does not look like "online".
                    !(a.starts_with("fe80:") || a.starts_with("fe80::"))
                })
                .map(|a| display_addr(a))
                .collect();
            let has_v4 = addrs.iter().any(|a| a.contains('.'));
            if addrs.is_empty() {
                rows.push(format!("{iface}  {state}  (no ipv4)"));
            } else if !has_v4 {
                rows.push(format!("{iface}  {state}  (no ipv4) {}", addrs.join("  ")));
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
            let addrs: Vec<String> = i.addresses.iter().map(|a| display_addr(a)).collect();
            let has_v4 = addrs.iter().any(|a| a.contains('.'));
            if addrs.is_empty() {
                format!("{}  {}  (no ipv4)", i.name, state)
            } else if !has_v4 {
                format!(
                    "{}  {}  (no ipv4) {}",
                    i.name,
                    state,
                    addrs.join("  ")
                )
            } else {
                format!("{}  {}  {}", i.name, state, addrs.join("  "))
            }
        })
        .collect()
}

/// IPv4 keeps `addr/prefix`; IPv6 is address-only (no subnet on the dashboard).
fn display_addr(addr: &str) -> String {
    let ip = addr.split('/').next().unwrap_or(addr);
    if ip.contains(':') {
        ip.to_string()
    } else {
        addr.to_string()
    }
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
                // Skip IPv6 link-local — fe80:: alone looks like "has IP" but
                // management/API need DHCPv4.
                let sin6 = &*(entry.ifa_addr as *const libc::sockaddr_in6);
                let octets = sin6.sin6_addr.s6_addr;
                let ip = Ipv6Addr::from(octets);
                if ip.is_unspecified() || ip.is_unicast_link_local() || ip.is_multicast() {
                    None
                } else {
                    let prefix = if !entry.ifa_netmask.is_null() {
                        let mask = &*(entry.ifa_netmask as *const libc::sockaddr_in6);
                        mask.sin6_addr
                            .s6_addr
                            .iter()
                            .map(|b| b.count_ones())
                            .sum::<u32>() as u8
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

/// CNI / container virtual interfaces that crowd out the host NIC on the dashboard.
pub fn is_cni_iface(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "cilium",
        "lxc",
        "veth",
        "cali",
        "flannel",
        "cni",
        "weave",
        "docker",
        "br-",
        "tunl",
        "vxlan",
        "nodelocaldns",
        "kube-",
    ];
    PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Physical / virtio NICs commonly used as the node management interface.
pub fn looks_like_host_iface(name: &str) -> bool {
    name.starts_with("eth")
        || name.starts_with("ens")
        || name.starts_with("enp")
        || name.starts_with("eno")
}

fn row_iface_name(row: &str) -> &str {
    row.split_whitespace().next().unwrap_or("")
}

fn first_ipv4(addresses: &[String]) -> Option<&str> {
    addresses.iter().find_map(|a| {
        let ip = a.split('/').next().unwrap_or(a.as_str());
        if ip.contains('.') {
            Some(a.as_str())
        } else {
            None
        }
    })
}

fn first_global_ipv6(addresses: &[String]) -> Option<&str> {
    // Prefer SLAAC GUA over synthetic fd00:: ULA (same rule as kubelet --node-ip).
    // Dashboard shows IPv6 as bare address (no /prefix); IPv4 keeps its CIDR.
    pertisk_net::prefer_global_ipv6(addresses.iter().map(|s| s.as_str())).map(|a| {
        a.split('/').next().unwrap_or(a)
    })
}

fn row_has_ipv4(row: &str) -> bool {
    row.split_whitespace().any(|p| p.contains('.') && p.contains('/'))
}

#[derive(Debug, Default)]
struct PrioritizedNet {
    interfaces: Vec<IfaceAddrs>,
    net_rows: Vec<String>,
    node_iface: String,
    node_ip: String,
}

/// Keep host NICs only; pick primary by config names, then eth/ens/…, then first with IPv4.
fn prioritize_host_network(
    ifaces: &[IfaceAddrs],
    rows: &[String],
    configured: &[String],
) -> PrioritizedNet {
    let host_ifaces: Vec<IfaceAddrs> = ifaces
        .iter()
        .filter(|i| !is_cni_iface(&i.name))
        .cloned()
        .collect();
    let mut host_rows: Vec<String> = rows
        .iter()
        .filter(|r| !is_cni_iface(row_iface_name(r)))
        .cloned()
        .collect();

    // Prefer configured → looks_like_host → has IPv4 → name order.
    host_rows.sort_by(|a, b| {
        let an = row_iface_name(a);
        let bn = row_iface_name(b);
        host_iface_rank(an, configured, row_has_ipv4(a))
            .cmp(&host_iface_rank(bn, configured, row_has_ipv4(b)))
            .then_with(|| an.cmp(bn))
    });

    let (node_iface, node_ip) = pick_primary_node_ip(&host_ifaces, configured);

    PrioritizedNet {
        interfaces: host_ifaces,
        net_rows: host_rows,
        node_iface,
        node_ip,
    }
}

fn host_iface_rank(name: &str, configured: &[String], has_v4: bool) -> u8 {
    if configured.iter().any(|c| c == name) {
        0
    } else if looks_like_host_iface(name) && has_v4 {
        1
    } else if looks_like_host_iface(name) {
        2
    } else if has_v4 {
        3
    } else {
        4
    }
}

fn pick_primary_node_ip(host_ifaces: &[IfaceAddrs], configured: &[String]) -> (String, String) {
    let mut ordered: Vec<&IfaceAddrs> = host_ifaces.iter().collect();
    ordered.sort_by(|a, b| {
        host_iface_rank(&a.name, configured, first_ipv4(&a.addresses).is_some())
            .cmp(&host_iface_rank(
                &b.name,
                configured,
                first_ipv4(&b.addresses).is_some(),
            ))
            .then_with(|| a.name.cmp(&b.name))
    });
    for iface in &ordered {
        if let Some(v4) = first_ipv4(&iface.addresses) {
            let mut display = v4.to_string();
            for v6 in all_global_ipv6(&iface.addresses) {
                display.push(' ');
                display.push_str(&v6);
            }
            return (iface.name.clone(), display);
        }
    }
    for iface in &ordered {
        let v6s = all_global_ipv6(&iface.addresses);
        if !v6s.is_empty() {
            return (iface.name.clone(), format!("(no ipv4) {}", v6s.join(" ")));
        }
    }
    if let Some(iface) = host_ifaces.first() {
        return (iface.name.clone(), "-".into());
    }
    (String::new(), "-".into())
}

/// All non-link-local IPv6 addresses (GUA first, then others), address-only.
fn all_global_ipv6(addresses: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(best) = first_global_ipv6(addresses) {
        out.push(best.to_string());
    }
    for a in addresses {
        let ip = a.split('/').next().unwrap_or(a);
        if !ip.contains(':') {
            continue;
        }
        if ip.starts_with("fe80:") || ip.starts_with("fe80::") {
            continue;
        }
        if out.iter().any(|e| e == ip) {
            continue;
        }
        out.push(ip.to_string());
    }
    out
}

fn push_disk_unique(disks: &mut Vec<DiskUsage>, next: Option<DiskUsage>) {
    let Some(d) = next else {
        return;
    };
    let dup = disks.iter().any(|x| {
        x.total_bytes == d.total_bytes && x.used_bytes == d.used_bytes
    });
    if !dup {
        disks.push(d);
    }
}

/// Parse `/proc/loadavg` — returns the 1-minute average (0.0 if missing).
pub fn parse_loadavg(text: &str) -> f32 {
    text.split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

fn parse_uptime(text: &str) -> u64 {
    text.split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .map(|seconds| seconds.max(0.0) as u64)
        .unwrap_or(0)
}

fn process_count() -> usize {
    std::fs::read_dir("/proc")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        })
        .count()
}

/// Aggregate CPU ticks from the first `cpu ` line of `/proc/stat`.
/// Returns `(idle_all, total)` where idle_all includes iowait.
pub fn parse_proc_stat_cpu(text: &str) -> Option<(u64, u64)> {
    for line in text.lines() {
        let rest = line.strip_prefix("cpu ")?;
        let mut vals = rest.split_whitespace().filter_map(|v| v.parse::<u64>().ok());
        // user nice system idle iowait irq softirq steal …
        let user = vals.next()?;
        let nice = vals.next()?;
        let system = vals.next()?;
        let idle = vals.next()?;
        let iowait = vals.next().unwrap_or(0);
        let irq = vals.next().unwrap_or(0);
        let softirq = vals.next().unwrap_or(0);
        let steal = vals.next().unwrap_or(0);
        let idle_all = idle.saturating_add(iowait);
        let total = user
            .saturating_add(nice)
            .saturating_add(system)
            .saturating_add(idle)
            .saturating_add(iowait)
            .saturating_add(irq)
            .saturating_add(softirq)
            .saturating_add(steal);
        return Some((idle_all, total));
    }
    None
}

/// Busy % since the previous call (0 on the first sample).
fn sample_cpu_usage_pct() -> u16 {
    use std::sync::Mutex;
    static PREV: Mutex<Option<(u64, u64)>> = Mutex::new(None);

    let Some(text) = read_to_string("/proc/stat") else {
        return 0;
    };
    let Some((idle, total)) = parse_proc_stat_cpu(&text) else {
        return 0;
    };
    let Ok(mut guard) = PREV.lock() else {
        return 0;
    };
    let pct = if let Some((prev_idle, prev_total)) = *guard {
        let di = idle.saturating_sub(prev_idle);
        let dt = total.saturating_sub(prev_total);
        if dt == 0 {
            0
        } else {
            let busy = dt.saturating_sub(di);
            ((busy.saturating_mul(100)) / dt).min(100) as u16
        }
    } else {
        0
    };
    *guard = Some((idle, total));
    pct
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
    fn external_cni_uses_cluster_pod_subnet() {
        let cfg = MachineConfig::from_yaml(
            "version: v1alpha1\nmachine:\n  type: controlplane\ncluster:\n  endpoint: https://10.0.0.1:6443\n  cni: none\n  podSubnet: 10.244.0.0/16\n  serviceSubnet: 10.96.0.0/12\n",
        )
        .expect("config");
        let cluster = cfg.cluster.as_ref().expect("cluster");

        assert_eq!(
            cluster_network_display(cluster),
            (
                "external".into(),
                "10.244.0.0/16".into(),
                "10.96.0.0/12".into()
            )
        );
    }

    #[test]
    fn pod_subnet_preferred_over_node_pod_cidr() {
        let cfg = MachineConfig::from_yaml(
            "version: v1alpha1\nmachine:\n  type: controlplane\ncluster:\n  endpoint: https://10.0.0.1:6443\n  podSubnet: 10.244.0.0/16\n  podCidr: 10.244.1.0/24\n  serviceSubnet: 10.96.0.0/12\n",
        )
        .expect("config");
        let cluster = cfg.cluster.as_ref().expect("cluster");
        assert_eq!(
            cluster_network_display(cluster),
            (
                "bridge".into(),
                "10.244.0.0/16".into(),
                "10.96.0.0/12".into()
            )
        );
    }

    #[test]
    fn dual_stack_dashboard_shows_both_family_cidrs() {
        let cfg = MachineConfig::from_yaml(
            r#"
version: v1alpha1
machine:
  type: controlplane
cluster:
  endpoint: https://10.0.0.1:6443
  networkMode: dual-stack
  network:
    podSubnets:
      - 10.244.0.0/16
      - 2001:db8:10:0::/56
    serviceSubnets:
      - 10.96.0.0/12
      - 2001:db8:96:1::/112
"#,
        )
        .expect("config");
        let cluster = cfg.cluster.as_ref().expect("cluster");
        assert!(cluster.is_dual_stack());
        assert_eq!(
            cluster.cluster_cidr(),
            "10.244.0.0/16,2001:db8:10:0::/56"
        );
        assert_eq!(
            cluster_network_display(cluster),
            (
                "bridge".into(),
                "10.244.0.0/16,2001:db8:10:0::/56".into(),
                "10.96.0.0/12,2001:db8:96:1::/112".into()
            )
        );
    }

    #[test]
    fn display_cidrs_keeps_dual_stack() {
        assert_eq!(
            display_cidrs("10.244.0.0/16,2001:db8:10:0::/56"),
            "10.244.0.0/16,2001:db8:10:0::/56"
        );
        assert_eq!(display_cidrs("10.96.0.0/12"), "10.96.0.0/12");
        assert_eq!(display_cidrs("-"), "-");
    }

    #[test]
    fn display_addr_keeps_ipv4_subnet_strips_ipv6() {
        assert_eq!(display_addr("10.1.1.173/24"), "10.1.1.173/24");
        assert_eq!(
            display_addr("2405:9800:b901:194c:be24:11ff:fe91:e066/64"),
            "2405:9800:b901:194c:be24:11ff:fe91:e066"
        );
        assert_eq!(display_addr("fd00:a:1:1::ad/64"), "fd00:a:1:1::ad");
    }

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

    #[test]
    fn loadavg_parses_first_field() {
        assert!((parse_loadavg("0.42 0.35 0.28 1/234 99") - 0.42).abs() < f32::EPSILON);
        assert_eq!(parse_loadavg(""), 0.0);
    }

    #[test]
    fn uptime_parses_fractional_seconds() {
        assert_eq!(parse_uptime("93784.23 1234.00"), 93_784);
        assert_eq!(parse_uptime(""), 0);
    }

    #[test]
    fn proc_stat_cpu_sums_ticks() {
        let sample = "cpu  100 20 30 400 10 1 2 3 0 0\ncpu0 50 10 15 200 5 0 1 1 0 0\n";
        let (idle_all, total) = parse_proc_stat_cpu(sample).unwrap();
        assert_eq!(idle_all, 410); // idle + iowait
        assert_eq!(total, 100 + 20 + 30 + 400 + 10 + 1 + 2 + 3);
    }

    #[test]
    fn cni_iface_detection() {
        assert!(is_cni_iface("cilium_host"));
        assert!(is_cni_iface("cilium_vxlan"));
        assert!(is_cni_iface("lxc123abc"));
        assert!(is_cni_iface("veth0a1b"));
        assert!(is_cni_iface("cali123"));
        assert!(is_cni_iface("flannel.1"));
        assert!(is_cni_iface("br-abc"));
        assert!(!is_cni_iface("eth0"));
        assert!(!is_cni_iface("ens18"));
        assert!(!is_cni_iface("enp0s3"));
    }

    #[test]
    fn prioritize_hides_cilium_keeps_host_ip() {
        let ifaces = vec![
            IfaceAddrs {
                name: "cilium_host".into(),
                addresses: vec!["10.0.0.1/32".into()],
            },
            IfaceAddrs {
                name: "lxcabc".into(),
                addresses: vec!["10.244.0.5/24".into()],
            },
            IfaceAddrs {
                name: "eth0".into(),
                addresses: vec!["10.1.1.198/24".into()],
            },
            IfaceAddrs {
                name: "vethxyz".into(),
                addresses: vec!["169.254.1.1/32".into()],
            },
        ];
        let rows = vec![
            "cilium_host  UP  10.0.0.1/32".into(),
            "lxcabc  UP  10.244.0.5/24".into(),
            "eth0  UP  10.1.1.198/24".into(),
            "vethxyz  UP  169.254.1.1/32".into(),
        ];
        let p = prioritize_host_network(&ifaces, &rows, &[]);
        assert_eq!(p.node_iface, "eth0");
        assert_eq!(p.node_ip, "10.1.1.198/24");
        assert_eq!(p.net_rows.len(), 1);
        assert!(p.net_rows[0].starts_with("eth0"));
        assert!(p.interfaces.iter().all(|i| !is_cni_iface(&i.name)));
    }

    #[test]
    fn prioritize_prefers_configured_iface() {
        let ifaces = vec![
            IfaceAddrs {
                name: "eth0".into(),
                addresses: vec!["10.0.0.1/24".into()],
            },
            IfaceAddrs {
                name: "ens18".into(),
                addresses: vec!["10.1.1.50/24".into()],
            },
        ];
        let rows = vec![
            "eth0  UP  10.0.0.1/24".into(),
            "ens18  UP  10.1.1.50/24".into(),
        ];
        let p = prioritize_host_network(&ifaces, &rows, &["ens18".into()]);
        assert_eq!(p.node_iface, "ens18");
        assert_eq!(p.node_ip, "10.1.1.50/24");
        assert_eq!(row_iface_name(&p.net_rows[0]), "ens18");
    }

    #[test]
    fn prefer_gua_over_ula_for_node_ip() {
        let ifaces = vec![IfaceAddrs {
            name: "eth0".into(),
            addresses: vec![
                "10.1.1.173/24".into(),
                "fd00:a:1:1::ad/64".into(),
                "2405:9800:b901:194c:be24:11ff:fe91:e066/64".into(),
            ],
        }];
        let rows = vec![
            "eth0  UP  10.1.1.173/24  fd00:a:1:1::ad/64  2405:9800:b901:194c:be24:11ff:fe91:e066/64"
                .into(),
        ];
        let p = prioritize_host_network(&ifaces, &rows, &[]);
        assert_eq!(p.node_iface, "eth0");
        // IPv4 keeps subnet; IPv6 is address-only (no /64).
        assert_eq!(
            p.node_ip,
            "10.1.1.173/24 2405:9800:b901:194c:be24:11ff:fe91:e066 fd00:a:1:1::ad"
        );
        assert!(!p.node_ip.contains("/64"), "ipv6 must not include subnet: {}", p.node_ip);
    }
}
