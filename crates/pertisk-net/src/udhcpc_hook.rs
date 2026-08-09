//! Apply BusyBox `udhcpc` lease events without a shell script.
//!
//! Production images ship `udhcpc` but no `/bin/sh`, so the stock
//! `/usr/share/udhcpc/default.script` cannot run. This hook is invoked via
//! `udhcpc -s /usr/lib/pertisk/udhcpc-hook`.
//!
//! Address/route install is **ioctl-only** (no tokio/netlink) so a lease still
//! lands when rtnetlink is unhappy — a common cause of IPv6-LL-only boots.

use crate::apply::NetError;

#[cfg(target_os = "linux")]
use std::net::Ipv4Addr;
#[cfg(target_os = "linux")]
use std::str::FromStr;

/// Entry point for the `pertisk-udhcpc-hook` binary (and tests).
#[cfg(target_os = "linux")]
pub fn run_from_env(args: &[String]) -> Result<(), NetError> {
    let action = args
        .get(1)
        .map(String::as_str)
        .ok_or_else(|| NetError::Msg("udhcpc hook: missing action".into()))?;

    let iface = std::env::var("interface")
        .map_err(|_| NetError::Msg("udhcpc hook: missing $interface".into()))?;

    match action {
        "deconfig" => {
            // Best-effort; kernel will replace on the next bound.
            let _ = crate::link::del_ipv4_ioctl(&iface);
            Ok(())
        }
        "bound" | "renew" => {
            let ip = std::env::var("ip")
                .map_err(|_| NetError::Msg("udhcpc hook: missing $ip".into()))?;
            let ip = Ipv4Addr::from_str(&ip)
                .map_err(|e| NetError::Msg(format!("udhcpc bad $ip {ip}: {e}")))?;
            let prefix = lease_prefix_len()?;
            eprintln!("pertisk-udhcpc-hook: {action} {iface} {ip}/{prefix}");
            let mut routers = Vec::new();
            if let Ok(router) = std::env::var("router") {
                for gw in router.split_whitespace() {
                    if let Ok(gw_ip) = Ipv4Addr::from_str(gw) {
                        routers.push(gw_ip);
                    }
                }
            }
            crate::link::apply_dhcp_v4_lease(&iface, ip, prefix, &routers)?;
            if let Ok(dns) = std::env::var("dns") {
                let servers: Vec<String> = dns
                    .split_whitespace()
                    .map(str::to_string)
                    .filter(|s| !s.is_empty())
                    .collect();
                if !servers.is_empty() {
                    let _ = crate::dns::write_resolv_conf(&servers);
                }
            }
            Ok(())
        }
        "leasefail" | "nak" => {
            eprintln!("pertisk-udhcpc-hook: {action} on {iface}");
            Ok(())
        }
        other => Err(NetError::Msg(format!(
            "udhcpc hook: unknown action {other}"
        ))),
    }
}

#[cfg(target_os = "linux")]
fn lease_prefix_len() -> Result<u8, NetError> {
    // BusyBox may export bit-count `mask` and/or dotted `subnet`.
    if let Ok(mask) = std::env::var("mask") {
        if let Ok(bits) = mask.parse::<u8>() {
            if bits <= 32 {
                return Ok(bits);
            }
        }
        if let Ok(bits) = dotted_mask_to_prefix(&mask) {
            return Ok(bits);
        }
    }
    if let Ok(subnet) = std::env::var("subnet") {
        return dotted_mask_to_prefix(&subnet);
    }
    Err(NetError::Msg(
        "udhcpc hook: missing usable $mask / $subnet".into(),
    ))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn dotted_mask_to_prefix(mask: &str) -> Result<u8, NetError> {
    let mut parts = mask.split('.');
    let mut bits: u32 = 0;
    let mut seen_zero = false;
    for _ in 0..4 {
        let octet: u8 = parts
            .next()
            .ok_or_else(|| NetError::Msg(format!("bad netmask {mask}")))?
            .parse()
            .map_err(|_| NetError::Msg(format!("bad netmask {mask}")))?;
        if seen_zero && octet != 0 {
            return Err(NetError::Msg(format!("non-contiguous netmask {mask}")));
        }
        match octet {
            255 => bits += 8,
            0 => seen_zero = true,
            other => {
                let leading = other.leading_ones();
                if other.trailing_zeros() != 8 - leading {
                    return Err(NetError::Msg(format!("non-contiguous netmask {mask}")));
                }
                bits += leading;
                seen_zero = true;
            }
        }
    }
    if parts.next().is_some() {
        return Err(NetError::Msg(format!("bad netmask {mask}")));
    }
    Ok(bits as u8)
}

#[cfg(test)]
mod tests {
    use super::dotted_mask_to_prefix;

    #[test]
    fn netmask_to_prefix() {
        assert_eq!(dotted_mask_to_prefix("255.255.255.0").unwrap(), 24);
        assert_eq!(dotted_mask_to_prefix("255.255.0.0").unwrap(), 16);
        assert_eq!(dotted_mask_to_prefix("255.255.255.252").unwrap(), 30);
        assert_eq!(dotted_mask_to_prefix("255.255.255.255").unwrap(), 32);
    }
}
