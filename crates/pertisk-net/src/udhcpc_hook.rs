//! Apply BusyBox `udhcpc` lease events without a shell script.
//!
//! Production images ship `udhcpc` but no `/bin/sh`, so the stock
//! `/usr/share/udhcpc/default.script` cannot run. This hook is invoked via
//! `udhcpc -s /usr/lib/pertisk/udhcpc-hook`.

use crate::apply::NetError;

/// Entry point for the `pertisk-udhcpc-hook` binary (and tests).
#[cfg(target_os = "linux")]
pub fn run_from_env(args: &[String]) -> Result<(), NetError> {
    let action = args
        .get(1)
        .map(String::as_str)
        .ok_or_else(|| NetError::Msg("udhcpc hook: missing action".into()))?;

    let iface = std::env::var("interface")
        .map_err(|_| NetError::Msg("udhcpc hook: missing $interface".into()))?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| NetError::Msg(e.to_string()))?;

    match action {
        "deconfig" => rt.block_on(crate::link::flush_addresses(&iface)),
        "bound" | "renew" => {
            let ip = std::env::var("ip")
                .map_err(|_| NetError::Msg("udhcpc hook: missing $ip".into()))?;
            let prefix = lease_prefix_len()?;
            let cidr = format!("{ip}/{prefix}");
            // Surface lease details on serial even if tracing isn't configured.
            eprintln!("pertisk-udhcpc-hook: {action} {iface} {cidr}");
            rt.block_on(async {
                crate::link::flush_addresses(&iface).await?;
                crate::link::add_address(&iface, &cidr).await?;
                if let Ok(routers) = std::env::var("router") {
                    for gw in routers.split_whitespace() {
                        // Best-effort: replace default route with the lease gateway.
                        let _ = crate::link::add_default_route(gw).await;
                    }
                }
                Ok::<(), NetError>(())
            })?;
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
            tracing::warn!(interface = %iface, action, "DHCP lease failed");
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
