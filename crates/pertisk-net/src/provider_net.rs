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
    use tracing::info;

    const CANDIDATES: &[&str] = &[
        "/dev/sr0",
        "/dev/sr1",
        "/dev/vdb",
        "/dev/vdc",
        "/dev/sdb",
        "/dev/sdc",
        "/dev/xvdb",
        "/dev/nvme0n2",
    ];

    pub fn apply() -> Result<bool, crate::NetError> {
        let Some(net) = load() else {
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
        for _ in 0..3 {
            if let Some(net) = scan() {
                return Some(net);
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
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
        for path in CANDIDATES {
            if let Some(net) = read_dev(path) {
                return Some(net);
            }
        }
        let Ok(rd) = std::fs::read_dir("/sys/block") else {
            return None;
        };
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
            if let Some(net) = read_dev(&path) {
                return Some(net);
            }
        }
        None
    }

    fn read_dev(path: &str) -> Option<Network> {
        use std::fs::File;
        use std::io::Read;
        let mut f = File::open(path).ok()?;
        let mut buf = [0u8; 4096];
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        match parse_pertisk_net(&buf[..n]) {
            Some(net) => {
                info!(device = path, "provider netcfg disk found");
                Some(net)
            }
            None => None,
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
