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

/// Non-loopback interface names (kernel order).
#[cfg(target_os = "linux")]
pub async fn list_interfaces() -> Result<Vec<String>, NetError> {
    use netlink_packet_route::link::LinkAttribute;
    use rtnetlink::new_connection;

    let (connection, handle, _) = new_connection().map_err(|e| NetError::Msg(e.to_string()))?;
    tokio::spawn(connection);

    let mut out = Vec::new();
    let mut links = handle.link().get().execute();
    while let Some(link) = links
        .try_next()
        .await
        .map_err(|e| NetError::Msg(e.to_string()))?
    {
        let mut name = None;
        for attr in &link.attributes {
            if let LinkAttribute::IfName(n) = attr {
                name = Some(n.clone());
                break;
            }
        }
        if let Some(n) = name {
            if n != "lo" {
                out.push(n);
            }
        }
    }
    Ok(out)
}

/// Wait until the link reports carrier (or timeout). Virtio often needs a beat after UP.
#[cfg(target_os = "linux")]
pub fn wait_carrier(iface: &str, timeout: std::time::Duration) -> Result<(), NetError> {
    use std::thread;
    use std::time::{Duration, Instant};

    let path = format!("/sys/class/net/{iface}/carrier");
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(s) = std::fs::read_to_string(&path) {
            if s.trim() == "1" {
                tracing::info!(interface = iface, "carrier up");
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            tracing::warn!(
                interface = iface,
                "carrier wait timed out; trying DHCP anyway"
            );
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// Resolve a configured iface name, or the first non-loopback NIC if missing.
#[cfg(target_os = "linux")]
pub async fn resolve_iface(configured: &str) -> Result<String, NetError> {
    let names = list_interfaces().await?;
    if names.iter().any(|n| n == configured) {
        return Ok(configured.to_string());
    }
    if let Some(first) = names.first() {
        tracing::warn!(
            configured,
            using = %first,
            available = ?names,
            "configured interface missing; using first NIC"
        );
        return Ok(first.clone());
    }
    Err(NetError::Msg(format!(
        "interface {configured} not found and no other NICs present"
    )))
}

/// Funnel a child's captured output into tracing, one line per record.
fn log_child_output(bin: &str, stdout: &[u8], stderr: &[u8]) {
    for (stream, bytes) in [("stdout", stdout), ("stderr", stderr)] {
        for line in String::from_utf8_lossy(bytes).lines() {
            let line = line.trim();
            if !line.is_empty() {
                tracing::info!(bin, stream, "{line}");
            }
        }
    }
}

fn dhcp_bin(candidates: &[&'static str]) -> Option<&'static str> {
    for c in candidates {
        if std::path::Path::new(c).is_file() {
            return Some(*c);
        }
    }
    None
}

/// Run a DHCP client if available (`udhcpc`, then `dhclient`).
///
/// Prefer `udhcpc -s /usr/lib/pertisk/udhcpc-hook` so leases apply without `/bin/sh`.
/// Uses absolute paths — PID 1 often has an empty `PATH`.
#[cfg(target_os = "linux")]
pub fn run_dhcp(iface: &str) -> Result<(), NetError> {
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    const UDHCPC_HOOK: &str = "/usr/lib/pertisk/udhcpc-hook";
    const UDHCPC_BINS: &[&str] = &["/usr/sbin/udhcpc", "/sbin/udhcpc", "udhcpc"];
    const DHCLIENT_BINS: &[&str] = &["/usr/sbin/dhclient", "/sbin/dhclient", "dhclient"];

    let hook = if std::path::Path::new(UDHCPC_HOOK).is_file() {
        Some(UDHCPC_HOOK)
    } else {
        tracing::warn!("udhcpc hook missing at {UDHCPC_HOOK}; lease may not apply");
        None
    };

    wait_carrier(iface, Duration::from_secs(10))?;

    // Prefer in-process DHCP — BusyBox udhcpc daemonizes without `-f`, and
    // `Command::output()` then returns before the lease/hook runs.
    match crate::dhcp::run_dhcp(iface) {
        Ok(()) => return Ok(()),
        Err(err) => {
            tracing::warn!(interface = iface, error = %err, "builtin DHCP failed; trying udhcpc");
        }
    }

    let mut last_err = String::new();
    for attempt in 1..=4 {
        if let Some(bin) = dhcp_bin(UDHCPC_BINS) {
            // `-f` keeps udhcpc in the foreground so we can wait for the lease.
            let mut args = vec!["-i", iface, "-f", "-n", "-q", "-t", "8"];
            if let Some(script) = hook {
                args.extend_from_slice(&["-s", script]);
            }
            use std::process::Stdio;
            // Capture rather than inherit: udhcpc and its hook would otherwise
            // write straight onto the serial console, scribbling over the
            // dashboard and parking the cursor mid-screen.
            match Command::new(bin)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .map(|out| {
                    log_child_output(bin, &out.stdout, &out.stderr);
                    out.status
                }) {
                Ok(status) if status.success() => {
                    // Confirm the hook actually installed an address.
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| NetError::Msg(e.to_string()))?;
                    let addrs = rt.block_on(list_addresses(iface)).unwrap_or_default();
                    if !addrs.is_empty() {
                        tracing::info!(
                            interface = iface,
                            attempt,
                            addresses = ?addrs,
                            "udhcpc lease applied"
                        );
                        return Ok(());
                    }
                    last_err = format!(
                        "{bin} exited 0 but no address on {iface} (hook={})",
                        hook.unwrap_or("none")
                    );
                    tracing::warn!(%last_err, attempt, "DHCP incomplete");
                }
                Ok(status) => {
                    last_err = format!("{bin} status={status}");
                    tracing::warn!(
                        interface = iface,
                        attempt,
                        %status,
                        bin,
                        hook = hook.unwrap_or("(default.script)"),
                        "udhcpc failed"
                    );
                }
                Err(err) => {
                    last_err = format!("spawn {bin}: {err}");
                    tracing::warn!(interface = iface, bin, error = %err, "udhcpc spawn failed");
                }
            }
        }

        if let Some(bin) = dhcp_bin(DHCLIENT_BINS) {
            match Command::new(bin).args(["-1", "-v", iface]).output() {
                Ok(out) if out.status.success() => return Ok(()),
                Ok(out) => {
                    let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    last_err = format!("{bin}: {detail}");
                }
                Err(err) => {
                    last_err = format!("spawn {bin}: {err}");
                }
            }
        }

        if attempt < 4 {
            thread::sleep(Duration::from_secs(2));
        }
    }

    if dhcp_bin(UDHCPC_BINS).is_none() && dhcp_bin(DHCLIENT_BINS).is_none() {
        return Err(NetError::Msg(format!(
            "no DHCP client found for {iface} (tried /usr/sbin/udhcpc)"
        )));
    }
    Err(NetError::Msg(format!(
        "DHCP failed for {iface} after retries: {last_err}"
    )))
}
