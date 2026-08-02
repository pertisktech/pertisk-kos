//! Apply machine network configuration.

use pertisk_config::Network;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum NetError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Msg(String),
}

/// Configure host interfaces from machine config.
pub fn apply_network(network: &Network) -> Result<(), NetError> {
    #[cfg(target_os = "linux")]
    {
        linux::apply(network)
    }
    #[cfg(not(target_os = "linux"))]
    {
        for iface in &network.interfaces {
            info!(
                interface = %iface.interface,
                dhcp = iface.dhcp,
                addresses = ?iface.addresses,
                "network apply (dev log only)"
            );
        }
        if !network.nameservers.is_empty() {
            info!(nameservers = ?network.nameservers, "DNS (dev log only)");
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use tracing::warn;

    use crate::dns::write_resolv_conf;
    use crate::link;

    pub fn apply(network: &Network) -> Result<(), NetError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| NetError::Msg(e.to_string()))?;

        for iface in &network.interfaces {
            rt.block_on(link::set_link_up(&iface.interface))?;
            if iface.dhcp {
                match link::run_dhcp(&iface.interface) {
                    Ok(()) => {
                        let addrs = rt
                            .block_on(link::list_addresses(&iface.interface))
                            .unwrap_or_default();
                        info!(
                            interface = %iface.interface,
                            addresses = ?addrs,
                            "DHCP configured"
                        );
                    }
                    Err(err) => warn!(
                        interface = %iface.interface,
                        error = %err,
                        "DHCP failed; continuing"
                    ),
                }
            } else {
                for addr in &iface.addresses {
                    rt.block_on(link::add_address(&iface.interface, addr))?;
                }
                if let Some(gw) = &iface.gateway {
                    rt.block_on(link::add_default_route(gw))?;
                }
                info!(
                    interface = %iface.interface,
                    addresses = ?iface.addresses,
                    gateway = ?iface.gateway,
                    "static network configured"
                );
            }
        }

        if !network.nameservers.is_empty() {
            write_resolv_conf(&network.nameservers)?;
        }

        Ok(())
    }
}
