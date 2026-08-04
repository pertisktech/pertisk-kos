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

        // containerd/kubelet/etcd bind 127.0.0.1 — lo must be up with an address.
        if let Err(err) = rt.block_on(ensure_loopback()) {
            warn!(error = %err, "loopback setup failed");
        }

        for iface in &network.interfaces {
            let name = match rt.block_on(link::resolve_iface(&iface.interface)) {
                Ok(n) => n,
                Err(err) => {
                    warn!(
                        configured = %iface.interface,
                        error = %err,
                        "skip interface"
                    );
                    continue;
                }
            };
            rt.block_on(link::set_link_up(&name))?;
            if iface.dhcp {
                link::relax_rp_filter(&name);
                let existing = rt.block_on(link::list_addresses(&name)).unwrap_or_default();
                // IPv6 SLAAC/link-local often appears before DHCPv4 — only skip DHCP
                // when we already have an IPv4 address.
                let has_v4 = existing.iter().any(|a| a.contains('.'));
                if has_v4 {
                    info!(
                        interface = %name,
                        addresses = ?existing,
                        "DHCP already configured (IPv4 present)"
                    );
                } else {
                    match link::run_dhcp(&name) {
                        Ok(()) => {
                            let addrs = rt.block_on(link::list_addresses(&name)).unwrap_or_default();
                            info!(
                                interface = %name,
                                configured = %iface.interface,
                                addresses = ?addrs,
                                "DHCP configured"
                            );
                        }
                        Err(err) => warn!(
                            interface = %name,
                            configured = %iface.interface,
                            error = %err,
                            "DHCP failed; continuing"
                        ),
                    }
                }
            } else {
                for addr in &iface.addresses {
                    rt.block_on(link::add_address(&name, addr))?;
                }
                if let Some(gw) = &iface.gateway {
                    rt.block_on(link::add_default_route(gw))?;
                }
                info!(
                    interface = %name,
                    addresses = ?iface.addresses,
                    gateway = ?iface.gateway,
                    "static network configured"
                );
            }

            // DHCP + static extras (typical: IPv6 ULA alongside DHCPv4).
            // Previously skipped when IPv4 was already present — dual-stack never landed.
            if iface.dhcp {
                let existing = rt.block_on(link::list_addresses(&name)).unwrap_or_default();
                for addr in &iface.addresses {
                    let ip = addr.split('/').next().unwrap_or(addr.as_str());
                    let already = existing.iter().any(|a| a.split('/').next() == Some(ip));
                    if already {
                        continue;
                    }
                    match rt.block_on(link::add_address(&name, addr)) {
                        Ok(()) => info!(interface = %name, addr, "static address added (with DHCP)"),
                        Err(err) => warn!(
                            interface = %name,
                            addr,
                            error = %err,
                            "static address add failed"
                        ),
                    }
                }
            }

            // Lab LANs often lack IPv6 RA — synthesize a stable ULA when dual-stack.
            if let Err(err) = rt.block_on(link::ensure_stable_ula(&name)) {
                warn!(interface = %name, error = %err, "dual-stack ULA ensure failed");
            }
        }

        if !network.nameservers.is_empty() {
            write_resolv_conf(&network.nameservers)?;
        }

        Ok(())
    }

    async fn ensure_loopback() -> Result<(), NetError> {
        link::set_link_up("lo").await?;
        let addrs = link::list_addresses("lo").await.unwrap_or_default();
        let has_v4 = addrs.iter().any(|a| a.starts_with("127."));
        if !has_v4 {
            match link::add_address("lo", "127.0.0.1/8").await {
                Ok(()) => {}
                Err(err) => {
                    // Race / already present.
                    let addrs = link::list_addresses("lo").await.unwrap_or_default();
                    if !addrs.iter().any(|a| a.starts_with("127.")) {
                        return Err(err);
                    }
                }
            }
        }
        info!(addresses = ?link::list_addresses("lo").await.unwrap_or_default(), "loopback ready");
        Ok(())
    }
}
