//! Provider-injected network config (AHV IPAM disk, etc.).
//!
//! Nutanix Prism assigns an address at NIC create. That reservation is not a
//! guest DHCP lease — lab-up attaches a tiny extra disk whose first 4KiB is:
//!
//! ```text
//! PERTISK-NET
//! IPV4=10.1.1.124/24
//! GATEWAY=10.1.1.1
//! INTERFACE=eth0
//! DUAL_STACK=1
//! ```
//!
//! `DUAL_STACK=1` (or `IPV6=slaac`) lets the guest enable SLAAC before cluster
//! YAML is applied — required for static-IP labs where early boot is IPv4-only.

use pertisk_config::{Interface, Network};

/// Parsed provider netcfg disk.
#[derive(Debug, Clone)]
pub struct ProviderNetcfg {
    #[allow(dead_code)] // read on Linux apply path / unit tests
    pub network: Network,
    /// Enable IPv6 SLAAC (+ ULA fallback) alongside the static IPv4.
    #[allow(dead_code)]
    pub dual_stack: bool,
}

/// Parse a `PERTISK-NET` blob. Ignores trailing NUL / padding.
#[allow(dead_code)]
pub fn parse_pertisk_net(bytes: &[u8]) -> Option<ProviderNetcfg> {
    const MAGIC: &[u8] = b"PERTISK-NET";
    let window = &bytes[..bytes.len().min(65536)];
    let start = window
        .windows(MAGIC.len())
        .position(|w| w.eq_ignore_ascii_case(MAGIC))
        .unwrap_or(0);
    let slice = &window[start..];
    let end = slice
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(slice.len())
        .min(4096);
    let text = std::str::from_utf8(&slice[..end]).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let magic = lines.next()?;
    if !magic.eq_ignore_ascii_case("PERTISK-NET") {
        return None;
    }
    let mut ipv4 = None;
    let mut gateway = None;
    let mut iface = "eth0".to_string();
    let mut nameservers = Vec::new();
    let mut dual_stack = false;
    for line in lines {
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        match k.trim().to_ascii_uppercase().as_str() {
            "IPV4" | "ADDRESS" | "IP" => ipv4 = Some(v.to_string()),
            "GATEWAY" | "GW" => gateway = Some(v.to_string()),
            "INTERFACE" | "IFACE" => iface = v.to_string(),
            "NAMESERVER" | "DNS" => nameservers.push(v.to_string()),
            "DUAL_STACK" | "DUALSTACK" => {
                dual_stack = matches!(
                    v.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            "IPV6" => {
                // IPV6=slaac|auto|dual → enable dual-stack; static v6 later if needed.
                let lower = v.to_ascii_lowercase();
                if matches!(lower.as_str(), "slaac" | "auto" | "dual" | "1" | "true") {
                    dual_stack = true;
                }
            }
            _ => {}
        }
    }
    let ipv4 = ipv4?;
    if !ipv4.contains('.') {
        return None;
    }
    let cidr = if ipv4.contains('/') {
        ipv4
    } else {
        format!("{ipv4}/24")
    };
    Some(ProviderNetcfg {
        network: Network {
            hostname: None,
            interfaces: vec![Interface {
                interface: iface,
                dhcp: false,
                addresses: vec![cidr],
                gateway,
            }],
            nameservers,
        },
        dual_stack,
    })
}

/// Apply a provider netcfg disk if present. Returns `true` when an address was configured.
pub fn apply_provider_netcfg() -> Result<bool, super::NetError> {
    #[cfg(target_os = "linux")]
    {
        linux::apply()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(false)
    }
}

/// Single scan (no 15s wait). Used by the supervise loop to replace DHCP later.
pub fn try_apply_provider_netcfg() -> Result<bool, super::NetError> {
    #[cfg(target_os = "linux")]
    {
        linux::apply_once()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(false)
    }
}

/// True when a PERTISK-NET disk asks for dual-stack (SLAAC) before cluster YAML.
///
/// Call **before** [`crate::set_ipv6_enabled`] / sysctl policy so early boot does
/// not disable IPv6 when the static netcfg disk already opted in.
pub fn provider_netcfg_wants_dual_stack() -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::wants_dual_stack()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::apply::apply_network;
    use tracing::{debug, info};

    const CANDIDATES: &[&str] = &[
        "/dev/sr0",
        "/dev/sr1",
        "/dev/vdb",
        "/dev/vdc",
        "/dev/sdb",  // Proxmox attaches netcfg as scsi1 → /dev/sdb
        "/dev/sdc",
        "/dev/xvdb",
        "/dev/nvme0n2",
        "/dev/disk/by-label/PERTISK-NET",
    ];

    pub fn apply() -> Result<bool, crate::NetError> {
        apply_attempts(30)
    }

    pub fn apply_once() -> Result<bool, crate::NetError> {
        apply_attempts(1)
    }

    pub fn wants_dual_stack() -> bool {
        // Short scan — disk is either present early or not (no 15s wait).
        load(4).map(|c| c.dual_stack).unwrap_or(false)
    }

    fn apply_attempts(attempts: u32) -> Result<bool, crate::NetError> {
        let Some(cfg) = load(attempts) else {
            if attempts > 1 {
                info!("no provider netcfg disk found after scanning all candidates");
            } else {
                debug!("no provider netcfg disk on this pass");
            }
            return Ok(false);
        };
        let addr = cfg
            .network
            .interfaces
            .first()
            .and_then(|i| i.addresses.first())
            .cloned()
            .unwrap_or_default();
        info!(
            addr = %addr,
            dual_stack = cfg.dual_stack,
            "applying provider netcfg (AHV IPAM disk)"
        );
        if cfg.dual_stack {
            crate::link::set_ipv6_enabled(true);
        }
        apply_network(&cfg.network)?;
        // Netcfg is static. A DHCP maintainer left from boot would later expire the
        // lease, rediscover a *different* LAN address, and rebase etcd peer URLs
        // under every CP at once — overnight HA death after power-on.
        for iface in &cfg.network.interfaces {
            crate::dhcp::stop_maintainer(&iface.interface);
            crate::dhcp::clear_persisted_lease(&iface.interface);
        }
        Ok(true)
    }

    fn load(attempts: u32) -> Option<ProviderNetcfg> {
        let _ = std::process::Command::new("modprobe")
            .args(["sr_mod"])
            .status();
        let attempts = attempts.max(1);
        for attempt in 1..=attempts {
            if attempts > 1 && (attempt == 1 || attempt % 5 == 0) {
                debug!(attempt, "scanning for provider netcfg disk (attempt {attempt}/{attempts})");
            }
            if let Some(net) = scan() {
                info!(
                    attempt,
                    dual_stack = net.dual_stack,
                    "provider netcfg disk found"
                );
                return Some(net);
            }
            if attempt < attempts {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
        if attempts > 1 {
            info!("provider netcfg disk not found after {attempts} attempts");
        }
        None
    }

    fn skip_block(name: &str) -> bool {
        name.starts_with("loop")
            || name.starts_with("ram")
            || name.starts_with("zram")
            || name.starts_with("dm-")
            || name.starts_with("fd")
    }

    fn scan() -> Option<ProviderNetcfg> {
        debug!("scanning candidates for provider netcfg disk");
        for path in CANDIDATES {
            if let Some(net) = read_dev(path) {
                return Some(net);
            }
        }
        // Also scan all block devices in /sys/block
        let rd = match std::fs::read_dir("/sys/block") {
            Ok(rd) => rd,
            Err(e) => {
                debug!(error = %e, "cannot read /sys/block");
                return None;
            }
        };
        let mut scanned: Vec<String> = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if skip_block(&name) {
                continue;
            }
            let path = format!("/dev/{name}");
            if CANDIDATES.iter().any(|c| *c == path.as_str()) {
                continue;
            }
            scanned.push(path.clone());
            if let Some(net) = read_dev(&path) {
                return Some(net);
            }
        }
        debug!(
            extras = scanned.len(),
            "no provider netcfg disk among candidates / extras"
        );
        None
    }

    fn read_dev(path: &str) -> Option<ProviderNetcfg> {
        use std::fs::File;
        use std::io::Read;
        let mut f = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                // Missing candidates (sr0, vdb, …) are normal on Proxmox / DHCP-only labs.
                debug!(path, error = %e, "cannot open device");
                return None;
            }
        };
        let mut buf = [0u8; 65536];
        let n = match f.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                debug!(path, error = %e, "cannot read device");
                return None;
            }
        };
        if n == 0 {
            debug!(path, "read 0 bytes from device");
            return None;
        }
        match parse_pertisk_net(&buf[..n]) {
            Some(net) => {
                info!(device = path, "provider netcfg disk found");
                Some(net)
            }
            None => {
                debug!(path, bytes = n, "no PERTISK-NET header");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_pertisk_net;

    #[test]
    fn parses_padded_blob() {
        let mut raw = vec![0u8; 1024];
        let body = b"PERTISK-NET\nIPV4=10.1.1.124/24\nGATEWAY=10.1.1.1\n";
        raw[..body.len()].copy_from_slice(body);
        let cfg = parse_pertisk_net(&raw).unwrap();
        assert_eq!(cfg.network.interfaces[0].addresses, vec!["10.1.1.124/24"]);
        assert_eq!(
            cfg.network.interfaces[0].gateway.as_deref(),
            Some("10.1.1.1")
        );
        assert!(!cfg.network.interfaces[0].dhcp);
        assert!(!cfg.dual_stack);
    }

    #[test]
    fn rejects_gpt() {
        assert!(parse_pertisk_net(b"EFI PART....").is_none());
    }

    #[test]
    fn adds_slash24_when_missing() {
        let cfg = parse_pertisk_net(b"PERTISK-NET\nIP=10.1.1.10\n").unwrap();
        assert_eq!(cfg.network.interfaces[0].addresses, vec!["10.1.1.10/24"]);
    }

    #[test]
    fn parses_nameserver() {
        let cfg = parse_pertisk_net(
            b"PERTISK-NET\nIPV4=10.1.1.129/24\nGATEWAY=10.1.1.10\nNAMESERVER=10.1.1.10\n",
        )
        .unwrap();
        assert_eq!(cfg.network.nameservers, vec!["10.1.1.10"]);
        assert_eq!(
            cfg.network.interfaces[0].gateway.as_deref(),
            Some("10.1.1.10")
        );
    }

    #[test]
    fn finds_magic_after_iso_system_area() {
        let mut raw = vec![0u8; 4096];
        raw[0] = 1;
        raw[1..6].copy_from_slice(b"CD001");
        let body = b"PERTISK-NET\nIPV4=10.1.1.19/24\nGATEWAY=10.1.1.10\n";
        raw[2048..2048 + body.len()].copy_from_slice(body);
        let cfg = parse_pertisk_net(&raw).unwrap();
        assert_eq!(cfg.network.interfaces[0].addresses, vec!["10.1.1.19/24"]);
    }

    #[test]
    fn parses_dual_stack_flag() {
        let cfg = parse_pertisk_net(
            b"PERTISK-NET\nIPV4=10.1.1.252/24\nGATEWAY=10.1.1.10\nDUAL_STACK=1\n",
        )
        .unwrap();
        assert!(cfg.dual_stack);
        let cfg2 =
            parse_pertisk_net(b"PERTISK-NET\nIPV4=10.1.1.1/24\nIPV6=slaac\n").unwrap();
        assert!(cfg2.dual_stack);
    }
}
