//! Host networking for Pertisk KOS (Phase 1 / M2).
//!
//! Brings links up, applies static addressing via netlink, or requests DHCP
//! through `udhcpc` / `dhclient` when present.
//!
//! Production images have no shell, so DHCP leases are applied by the
//! `pertisk-udhcpc-hook` binary (`udhcpc -s /usr/lib/pertisk/udhcpc-hook`).

mod apply;
#[cfg(target_os = "linux")]
mod dhcp;
mod dns;
mod link;
pub mod udhcpc_hook;

pub use apply::{apply_network, NetError};

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
