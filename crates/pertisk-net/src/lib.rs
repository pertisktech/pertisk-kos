//! Host networking for Pertisk KOS (Phase 1 / M2).
//!
//! Brings links up, applies static addressing via netlink/ioctl, or requests
//! DHCPv4 via the **in-process** client (`dhcp::run_dhcp`). BusyBox `udhcpc`
//! (with `pertisk-udhcpc-hook`) remains a fallback only.

mod apply;
#[cfg(target_os = "linux")]
mod dhcp;
mod dns;
mod link;
pub mod udhcpc_hook;

pub use apply::{apply_network, NetError};
pub use link::{
    ipv6_enabled, is_ula_ipv6, is_usable_global_ipv6, prefer_global_ipv6, set_ipv6_enabled,
};

/// Comma-separated `v4,v6` for kubelet `--node-ip` when dual-stack (required by k8s).
/// Prefers SLAAC GUA over synthetic ULA so the node InternalIP matches eth0.
pub fn dual_stack_node_ip(iface: &str) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let addrs = list_addresses(iface).ok()?;
        let v4 = addrs.iter().find_map(|a| {
            let ip = a.split('/').next().unwrap_or(a.as_str());
            if ip.contains('.') && !ip.starts_with("127.") {
                Some(ip.to_string())
            } else {
                None
            }
        })?;
        let v6 = prefer_global_ipv6(addrs.iter().map(|s| s.as_str()))?.to_string();
        Some(format!("{v4},{v6}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = iface;
        None
    }
}

/// List non-link-local addresses on an interface (Linux). Empty elsewhere.
pub fn list_addresses(iface: &str) -> Result<Vec<String>, NetError> {
    #[cfg(target_os = "linux")]
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| NetError::Msg(e.to_string()))?;
        rt.block_on(link::list_addresses(iface))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = iface;
        Ok(Vec::new())
    }
}

/// Non-loopback interface names (Linux). Empty elsewhere.
pub fn list_interfaces() -> Result<Vec<String>, NetError> {
    #[cfg(target_os = "linux")]
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| NetError::Msg(e.to_string()))?;
        rt.block_on(link::list_interfaces())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(Vec::new())
    }
}
