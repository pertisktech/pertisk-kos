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
//! ```

use pertisk_config::{Interface, Network};

/// Parse a `PERTISK-NET` blob. Ignores trailing NUL / padding.
#[allow(dead_code)]
pub fn parse_pertisk_net(bytes: &[u8]) -> Option<Network> {
    let end = bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(bytes.len())
        .min(4096);
    let text = std::str::from_utf8(&bytes[..end]).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let magic = lines.next()?;
    if !magic.eq_ignore_ascii_case("PERTISK-NET") {
        return None;
    }
    let mut ipv4 = None;
    let mut gateway = None;
    let mut iface = "eth0".to_string();
    let mut nameservers = Vec::new();
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
    Some(Network {
        hostname: None,
        interfaces: vec![Interface {
            interface: iface,
            dhcp: false,
            addresses: vec![cidr],
            gateway,
        }],
        nameservers,
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
        let Some(net) = load() else {
            info!("no provider netcfg disk found after scanning all candidates");
            return Ok(false);
        };
        let addr = net
            .interfaces
            .first()
            .and_then(|i| i.addresses.first())
            .cloned()
            .unwrap_or_default();
        info!(addr = %addr, "applying provider netcfg (AHV IPAM disk)");
        apply_network(&net)?;
        Ok(true)
    }

    fn load() -> Option<Network> {
        // Retry up to 30 times (15 seconds total) - virtio disk may take time to appear + udev
        for attempt in 1..=30 {
            info!(attempt, "scanning for provider netcfg disk (attempt {}/30)", attempt);
            if let Some(net) = scan() {
                info!("provider netcfg disk found on attempt {}", attempt);
                return Some(net);
            }
            if attempt % 5 == 0 {
                info!("still scanning for provider netcfg disk (attempt {})", attempt);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        info!("provider netcfg disk not found after 30 attempts (15 seconds)");
        None
    }

    fn skip_block(name: &str) -> bool {
        name.starts_with("loop")
            || name.starts_with("ram")
            || name.starts_with("zram")
            || name.starts_with("dm-")
            || name.starts_with("fd")
            || name == "vda"
            || name == "sda"
            || name == "nvme0n1"
            || name == "xvda"
    }

    fn scan() -> Option<Network> {
        info!("scanning CANDIDATES for provider netcfg disk");
        for path in CANDIDATES {
            info!("checking candidate device: {}", path);
            if let Some(net) = read_dev(path) {
                return Some(net);
            }
        }
        // Also scan all block devices in /sys/block
        let rd = match std::fs::read_dir("/sys/block") {
            Ok(rd) => rd,
            Err(e) => {
                info!(error = %e, "cannot read /sys/block");
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
            info!("checking dynamic block device: {}", path);
            if let Some(net) = read_dev(&path) {
                return Some(net);
            }
        }
        if !scanned.is_empty() {
            info!(count = scanned.len(), "scanned {} extra block devices (no netcfg found)", scanned.len());
        } else {
            info!("no extra block devices found in /sys/block");
        }
        None
    }

    fn read_dev(path: &str) -> Option<Network> {
        use std::fs::File;
        use std::io::Read;
        let mut f = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                info!(path, error = %e, "cannot open device");
                return None;
            }
        };
        let mut buf = [0u8; 4096];
        let n = match f.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                info!(path, error = %e, "cannot read device");
                return None;
            }
        };
        if n == 0 {
            info!(path, "read 0 bytes from device");
            return None;
        }
        match parse_pertisk_net(&buf[..n]) {
            Some(net) => {
                info!(device = path, "provider netcfg disk found");
                Some(net)
            }
            None => {
                info!(path, bytes = n, "device read but no PERTISK-NET header found");
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
        let net = parse_pertisk_net(&raw).unwrap();
        assert_eq!(net.interfaces[0].addresses, vec!["10.1.1.124/24"]);
        assert_eq!(net.interfaces[0].gateway.as_deref(), Some("10.1.1.1"));
        assert!(!net.interfaces[0].dhcp);
    }

    #[test]
    fn rejects_gpt() {
        assert!(parse_pertisk_net(b"EFI PART....").is_none());
    }

    #[test]
    fn adds_slash24_when_missing() {
        let net = parse_pertisk_net(b"PERTISK-NET\nIP=10.1.1.10\n").unwrap();
        assert_eq!(net.interfaces[0].addresses, vec!["10.1.1.10/24"]);
    }

    #[test]
    fn parses_nameserver() {
        let net = parse_pertisk_net(
            b"PERTISK-NET\nIPV4=10.1.1.129/24\nGATEWAY=10.1.1.10\nNAMESERVER=10.1.1.10\n",
        )
        .unwrap();
        assert_eq!(net.nameservers, vec!["10.1.1.10"]);
        assert_eq!(net.interfaces[0].gateway.as_deref(), Some("10.1.1.10"));
    }
}
