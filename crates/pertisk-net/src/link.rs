//! Linux netlink link/address helpers + DHCP via system clients.

#[cfg(target_os = "linux")]
use crate::apply::NetError;

#[cfg(target_os = "linux")]
use futures::stream::TryStreamExt;

#[cfg(target_os = "linux")]
pub async fn set_link_up(iface: &str) -> Result<(), NetError> {
    use rtnetlink::new_connection;

    let (connection, handle, _) = new_connection().map_err(|e| NetError::Msg(e.to_string()))?;
    tokio::spawn(connection);

    let mut links = handle.link().get().match_name(iface.to_string()).execute();
    let link = links
        .try_next()
        .await
        .map_err(|e| NetError::Msg(e.to_string()))?
        .ok_or_else(|| NetError::Msg(format!("interface {iface} not found")))?;

    handle
        .link()
        .set(link.header.index)
        .up()
        .execute()
        .await
        .map_err(|e| NetError::Msg(e.to_string()))?;

    tracing::info!(interface = iface, "link up");
    Ok(())
}

#[cfg(target_os = "linux")]
pub async fn add_address(iface: &str, cidr: &str) -> Result<(), NetError> {
    use std::str::FromStr;

    use ipnet::IpNet;
    use rtnetlink::new_connection;

    let net = IpNet::from_str(cidr).map_err(|e| NetError::Msg(format!("bad CIDR {cidr}: {e}")))?;
    let (connection, handle, _) = new_connection().map_err(|e| NetError::Msg(e.to_string()))?;
    tokio::spawn(connection);

    let mut links = handle.link().get().match_name(iface.to_string()).execute();
    let link = links
        .try_next()
        .await
        .map_err(|e| NetError::Msg(e.to_string()))?
        .ok_or_else(|| NetError::Msg(format!("interface {iface} not found")))?;

    let index = link.header.index;
    match net {
        IpNet::V4(v4) => {
            handle
                .address()
                .add(index, std::net::IpAddr::V4(v4.addr()), v4.prefix_len())
                .execute()
                .await
                .map_err(|e| NetError::Msg(e.to_string()))?;
        }
        IpNet::V6(v6) => {
            handle
                .address()
                .add(index, std::net::IpAddr::V6(v6.addr()), v6.prefix_len())
                .execute()
                .await
                .map_err(|e| NetError::Msg(e.to_string()))?;
        }
    }
    tracing::info!(interface = iface, cidr, "address added");
    Ok(())
}

/// Remove IPv4/IPv6 addresses from an interface (used by DHCP deconfig/renew).
#[cfg(target_os = "linux")]
pub async fn flush_addresses(iface: &str) -> Result<(), NetError> {
    use rtnetlink::new_connection;

    let (connection, handle, _) = new_connection().map_err(|e| NetError::Msg(e.to_string()))?;
    tokio::spawn(connection);

    let mut links = handle.link().get().match_name(iface.to_string()).execute();
    let link = links
        .try_next()
        .await
        .map_err(|e| NetError::Msg(e.to_string()))?
        .ok_or_else(|| NetError::Msg(format!("interface {iface} not found")))?;
    let index = link.header.index;

    let mut addrs = handle
        .address()
        .get()
        .set_link_index_filter(index)
        .execute();
    while let Some(addr) = addrs
        .try_next()
        .await
        .map_err(|e| NetError::Msg(e.to_string()))?
    {
        handle
            .address()
            .del(addr)
            .execute()
            .await
            .map_err(|e| NetError::Msg(e.to_string()))?;
    }
    tracing::info!(interface = iface, "addresses flushed");
    Ok(())
}

/// List non-link-local addresses currently on an interface (for boot logs).
#[cfg(target_os = "linux")]
pub async fn list_addresses(iface: &str) -> Result<Vec<String>, NetError> {
    use std::net::IpAddr;

    use netlink_packet_route::address::AddressAttribute;
    use rtnetlink::new_connection;

    let (connection, handle, _) = new_connection().map_err(|e| NetError::Msg(e.to_string()))?;
    tokio::spawn(connection);

    let mut links = handle.link().get().match_name(iface.to_string()).execute();
    let link = links
        .try_next()
        .await
        .map_err(|e| NetError::Msg(e.to_string()))?
        .ok_or_else(|| NetError::Msg(format!("interface {iface} not found")))?;
    let index = link.header.index;

    let mut out = Vec::new();
    let mut addrs = handle
        .address()
        .get()
        .set_link_index_filter(index)
        .execute();
    while let Some(addr) = addrs
        .try_next()
        .await
        .map_err(|e| NetError::Msg(e.to_string()))?
    {
        for attr in &addr.attributes {
            if let AddressAttribute::Address(ip) = attr {
                let keep = match ip {
                    IpAddr::V4(v4) => !v4.is_link_local() && !v4.is_unspecified(),
                    IpAddr::V6(v6) => !v6.is_unicast_link_local() && !v6.is_unspecified(),
                };
                if keep {
                    out.push(format!("{ip}/{}", addr.header.prefix_len));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(target_os = "linux")]
pub async fn add_default_route(gateway: &str) -> Result<(), NetError> {
    use std::net::IpAddr;
    use std::str::FromStr;

    use rtnetlink::new_connection;

    let gw = IpAddr::from_str(gateway)
        .map_err(|e| NetError::Msg(format!("bad gateway {gateway}: {e}")))?;
    let (connection, handle, _) = new_connection().map_err(|e| NetError::Msg(e.to_string()))?;
    tokio::spawn(connection);

    match gw {
        IpAddr::V4(v4) => {
            handle
                .route()
                .add()
                .v4()
                .destination_prefix(std::net::Ipv4Addr::UNSPECIFIED, 0)
                .gateway(v4)
                .execute()
                .await
                .map_err(|e| NetError::Msg(e.to_string()))?;
        }
        IpAddr::V6(v6) => {
            handle
                .route()
                .add()
                .v6()
                .destination_prefix(std::net::Ipv6Addr::UNSPECIFIED, 0)
                .gateway(v6)
                .execute()
                .await
                .map_err(|e| NetError::Msg(e.to_string()))?;
        }
    }
    tracing::info!(gateway, "default route added");
    Ok(())
}

/// Run a DHCP client if available (`udhcpc`, then `dhclient`).
///
/// Prefer `udhcpc -s /usr/lib/pertisk/udhcpc-hook` so leases apply without `/bin/sh`.
#[cfg(target_os = "linux")]
pub fn run_dhcp(iface: &str) -> Result<(), NetError> {
    use std::process::Command;

    const UDHCPC_HOOK: &str = "/usr/lib/pertisk/udhcpc-hook";

    let hook = if std::path::Path::new(UDHCPC_HOOK).is_file() {
        Some(UDHCPC_HOOK)
    } else {
        None
    };

    let mut args = vec!["-i", iface, "-n", "-q", "-t", "5"];
    if let Some(script) = hook {
        args.extend_from_slice(&["-s", script]);
    }

    let udhcpc = Command::new("udhcpc").args(&args).output();
    if let Ok(out) = udhcpc {
        if out.status.success() {
            return Ok(());
        }
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        tracing::warn!(
            interface = iface,
            status = %out.status,
            stderr = %detail,
            hook = hook.unwrap_or("(default.script)"),
            "udhcpc failed"
        );
    }

    let dhclient = Command::new("dhclient").args(["-1", "-v", iface]).output();
    match dhclient {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(NetError::Msg(format!(
                "DHCP clients failed for {iface}: {detail}"
            )))
        }
        Err(_) => Err(NetError::Msg(format!(
            "no DHCP client found (tried udhcpc, dhclient) for {iface}"
        ))),
    }
}
