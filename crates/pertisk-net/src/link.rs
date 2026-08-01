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
#[cfg(target_os = "linux")]
pub fn run_dhcp(iface: &str) -> Result<(), NetError> {
    use std::process::Command;

    let udhcpc = Command::new("udhcpc")
        .args(["-i", iface, "-n", "-q", "-t", "5"])
        .output();
    if let Ok(out) = udhcpc {
        if out.status.success() {
            return Ok(());
        }
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
