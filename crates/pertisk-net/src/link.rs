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

    // IPv4: ioctl first (no netlink). IPv6-LL-only boots were caused by netlink
    // "success" that never installed a visible address on some virt kernels.
    if let IpNet::V4(v4) = net {
        match add_ipv4_ioctl(iface, v4.addr(), v4.prefix_len()) {
            Ok(()) => {
                tracing::info!(interface = iface, cidr, "address added");
                return Ok(());
            }
            Err(ioctl_err) => {
                tracing::warn!(
                    interface = iface,
                    cidr,
                    error = %ioctl_err,
                    "ioctl IPv4 add failed; trying netlink / ip"
                );
                if try_ip_addr_replace(iface, cidr).is_ok() {
                    tracing::info!(interface = iface, cidr, "address added via ip");
                    return Ok(());
                }
                // fall through to netlink below
                let (connection, handle, _) =
                    new_connection().map_err(|e| NetError::Msg(e.to_string()))?;
                tokio::spawn(connection);
                let mut links = handle.link().get().match_name(iface.to_string()).execute();
                let link = links
                    .try_next()
                    .await
                    .map_err(|e| NetError::Msg(e.to_string()))?
                    .ok_or_else(|| NetError::Msg(format!("interface {iface} not found")))?;
                handle
                    .address()
                    .add(
                        link.header.index,
                        std::net::IpAddr::V4(v4.addr()),
                        v4.prefix_len(),
                    )
                    .replace()
                    .execute()
                    .await
                    .map_err(|e| {
                        NetError::Msg(format!(
                            "IPv4 add failed ioctl=({ioctl_err}) netlink=({e})"
                        ))
                    })?;
                tracing::info!(interface = iface, cidr, "address added");
                return Ok(());
            }
        }
    }

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
        IpNet::V4(_) => unreachable!("handled above"),
        IpNet::V6(v6) => {
            match handle
                .address()
                .add(index, std::net::IpAddr::V6(v6.addr()), v6.prefix_len())
                .replace()
                .execute()
                .await
            {
                Ok(()) => {}
                Err(err) => {
                    let msg = err.to_string();
                    if is_af_unsupported(&msg) {
                        tracing::warn!(
                            interface = iface,
                            cidr,
                            error = %msg,
                            "IPv6 address skipped (stack absent)"
                        );
                        return Ok(());
                    }
                    return Err(NetError::Msg(msg));
                }
            }
        }
    }
    tracing::info!(interface = iface, cidr, "address added");
    Ok(())
}

#[cfg(target_os = "linux")]
fn try_ip_addr_replace(iface: &str, cidr: &str) -> Result<(), NetError> {
    use std::process::Command;
    for bin in ["/sbin/ip", "/usr/sbin/ip", "ip"] {
        let status = Command::new(bin)
            .args(["addr", "replace", cidr, "dev", iface])
            .status();
        match status {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => {
                return Err(NetError::Msg(format!("{bin} addr replace status={s}")));
            }
            Err(_) => continue,
        }
    }
    Err(NetError::Msg("no ip binary".into()))
}

/// Add an IPv4 address via `SIOCSIFADDR` / `SIOCSIFNETMASK` (no netlink).
#[cfg(target_os = "linux")]
pub fn add_ipv4_ioctl(
    iface: &str,
    ip: std::net::Ipv4Addr,
    prefix: u8,
) -> Result<(), NetError> {
    use std::os::fd::AsRawFd;

    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        None,
    )
    .map_err(|e| NetError::Msg(format!("ioctl socket: {e}")))?;

    let mut req: libc::ifreq = unsafe { std::mem::zeroed() };
    for (dst, src) in req.ifr_name.iter_mut().zip(iface.bytes()) {
        *dst = src as libc::c_char;
    }

    let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    addr.sin_family = libc::AF_INET as libc::sa_family_t;
    addr.sin_addr.s_addr = u32::from(ip).to_be();
    unsafe {
        std::ptr::write(
            std::ptr::addr_of_mut!(req.ifr_ifru) as *mut libc::sockaddr_in,
            addr,
        );
    }
    let rc = unsafe { libc::ioctl(sock.as_raw_fd(), libc::SIOCSIFADDR as libc::Ioctl, &req) };
    if rc < 0 {
        return Err(NetError::Msg(format!(
            "SIOCSIFADDR {iface} {ip}: {}",
            std::io::Error::last_os_error()
        )));
    }

    let mask_bits = if prefix >= 32 {
        !0u32
    } else {
        (!0u32) << (32 - u32::from(prefix))
    };
    addr.sin_addr.s_addr = mask_bits.to_be();
    unsafe {
        std::ptr::write(
            std::ptr::addr_of_mut!(req.ifr_ifru) as *mut libc::sockaddr_in,
            addr,
        );
    }
    let rc = unsafe { libc::ioctl(sock.as_raw_fd(), libc::SIOCSIFNETMASK as libc::Ioctl, &req) };
    if rc < 0 {
        return Err(NetError::Msg(format!(
            "SIOCSIFNETMASK {iface} /{prefix}: {}",
            std::io::Error::last_os_error()
        )));
    }
    tracing::info!(interface = iface, %ip, prefix, "IPv4 added via ioctl");
    Ok(())
}

/// Remove the primary IPv4 address (`SIOCDIFADDR`).
#[cfg(target_os = "linux")]
pub fn del_ipv4_ioctl(iface: &str) -> Result<(), NetError> {
    use std::os::fd::AsRawFd;

    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        None,
    )
    .map_err(|e| NetError::Msg(format!("ioctl socket: {e}")))?;
    let mut req: libc::ifreq = unsafe { std::mem::zeroed() };
    for (dst, src) in req.ifr_name.iter_mut().zip(iface.bytes()) {
        *dst = src as libc::c_char;
    }
    let rc = unsafe { libc::ioctl(sock.as_raw_fd(), libc::SIOCDIFADDR as libc::Ioctl, &req) };
    if rc < 0 {
        return Err(NetError::Msg(format!(
            "SIOCDIFADDR {iface}: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

/// Add IPv4 default route via `SIOCADDRT`.
#[cfg(target_os = "linux")]
pub fn add_default_route_v4_ioctl(gateway: std::net::Ipv4Addr) -> Result<(), NetError> {
    use std::os::fd::AsRawFd;

    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        None,
    )
    .map_err(|e| NetError::Msg(format!("route ioctl socket: {e}")))?;

    let mut rt: libc::rtentry = unsafe { std::mem::zeroed() };
    // libc::rtentry uses sockaddr (not sockaddr_in); write inet fields through raw bytes.
    unsafe {
        let dst = &mut rt.rt_dst as *mut libc::sockaddr as *mut libc::sockaddr_in;
        let gw = &mut rt.rt_gateway as *mut libc::sockaddr as *mut libc::sockaddr_in;
        let mask = &mut rt.rt_genmask as *mut libc::sockaddr as *mut libc::sockaddr_in;
        (*dst).sin_family = libc::AF_INET as libc::sa_family_t;
        (*gw).sin_family = libc::AF_INET as libc::sa_family_t;
        (*mask).sin_family = libc::AF_INET as libc::sa_family_t;
        (*gw).sin_addr.s_addr = u32::from(gateway).to_be();
    }
    rt.rt_flags = (libc::RTF_UP | libc::RTF_GATEWAY) as libc::c_ushort;

    let rc = unsafe { libc::ioctl(sock.as_raw_fd(), libc::SIOCADDRT as libc::Ioctl, &rt) };
    if rc < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EEXIST) {
            return Ok(());
        }
        return Err(NetError::Msg(format!(
            "SIOCADDRT default via {gateway}: {err}"
        )));
    }
    tracing::info!(%gateway, "default route added via ioctl");
    Ok(())
}

/// Relax rp_filter so DHCPv4 replies are not dropped before the address exists.
#[cfg(target_os = "linux")]
pub fn relax_rp_filter(iface: &str) {
    for path in [
        "/proc/sys/net/ipv4/conf/all/rp_filter".to_string(),
        "/proc/sys/net/ipv4/conf/default/rp_filter".to_string(),
        format!("/proc/sys/net/ipv4/conf/{iface}/rp_filter"),
    ] {
        let _ = std::fs::write(&path, b"0");
    }
}

/// Remove IPv4 addresses from an interface (used by DHCP deconfig/renew).
///
/// IPv4-only on purpose: deleting AF_INET6 while `ipv6` is unloaded returns
/// `Address family not supported` and used to abort the lease install, leaving
/// eth0 with `(no ip)`.
#[cfg(target_os = "linux")]
pub async fn flush_addresses(iface: &str) -> Result<(), NetError> {
    use netlink_packet_route::AddressFamily;
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
        if addr.header.family != AddressFamily::Inet {
            continue;
        }
        match handle.address().del(addr).execute().await {
            Ok(()) => {}
            Err(err) => {
                let msg = err.to_string();
                if is_af_unsupported(&msg) {
                    tracing::warn!(
                        interface = iface,
                        error = %msg,
                        "skip address delete (AF unsupported)"
                    );
                    continue;
                }
                return Err(NetError::Msg(msg));
            }
        }
    }
    tracing::info!(interface = iface, "IPv4 addresses flushed");
    Ok(())
}

#[cfg(target_os = "linux")]
fn is_af_unsupported(msg: &str) -> bool {
    msg.contains("Address family not supported")
        || msg.contains("Address family not supported by protocol")
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
                    // IPv4-only inventory — AF_INET6 is optional on this image.
                    IpAddr::V6(_) => false,
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
            match handle
                .route()
                .add()
                .v6()
                .destination_prefix(std::net::Ipv6Addr::UNSPECIFIED, 0)
                .gateway(v6)
                .execute()
                .await
            {
                Ok(()) => {}
                Err(err) => {
                    let msg = err.to_string();
                    if is_af_unsupported(&msg) {
                        tracing::warn!(
                            gateway,
                            error = %msg,
                            "IPv6 default route skipped (stack absent)"
                        );
                        return Ok(());
                    }
                    return Err(NetError::Msg(msg));
                }
            }
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

    // BusyBox udhcpc first — more reliable on virtio than the in-process client
    // when only IPv6 link-local is present and DHCPv4 must still win.
    let mut last_err = String::from("no DHCP client ran");
    for attempt in 1..=3 {
        if let Some(bin) = dhcp_bin(UDHCPC_BINS) {
            let mut args = vec!["-i", iface, "-f", "-n", "-q", "-t", "8"];
            if let Some(script) = hook {
                args.extend_from_slice(&["-s", script]);
            }
            use std::process::Stdio;
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
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| NetError::Msg(e.to_string()))?;
                    let addrs = rt.block_on(list_addresses(iface)).unwrap_or_default();
                    let has_v4 = addrs.iter().any(|a| a.contains('.'));
                    if has_v4 {
                        tracing::info!(
                            interface = iface,
                            attempt,
                            addresses = ?addrs,
                            "udhcpc IPv4 lease applied"
                        );
                        return Ok(());
                    }
                    last_err = format!(
                        "{bin} exited 0 but no IPv4 on {iface} (hook={}, addrs={addrs:?})",
                        hook.unwrap_or("none")
                    );
                    tracing::warn!(%last_err, attempt, "DHCP incomplete (IPv6-only is not enough)");
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
        } else {
            last_err = "udhcpc not found".into();
            break;
        }

        if attempt < 3 {
            thread::sleep(Duration::from_secs(2));
        }
    }

    match crate::dhcp::run_dhcp(iface) {
        Ok(()) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| NetError::Msg(e.to_string()))?;
            let addrs = rt.block_on(list_addresses(iface)).unwrap_or_default();
            if addrs.iter().any(|a| a.contains('.')) {
                return Ok(());
            }
            last_err = format!("builtin DHCP returned Ok but no IPv4 on {iface}: {addrs:?}");
            tracing::warn!(%last_err, "builtin DHCP incomplete");
        }
        Err(err) => {
            tracing::warn!(interface = iface, error = %err, "builtin DHCP failed");
            last_err = err.to_string();
        }
    }

    if let Some(bin) = dhcp_bin(DHCLIENT_BINS) {
        match Command::new(bin).args(["-1", "-v", iface]).output() {
            Ok(out) if out.status.success() => {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| NetError::Msg(e.to_string()))?;
                let addrs = rt.block_on(list_addresses(iface)).unwrap_or_default();
                if addrs.iter().any(|a| a.contains('.')) {
                    return Ok(());
                }
            }
            Ok(out) => {
                let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
                last_err = format!("{bin}: {detail}");
            }
            Err(err) => {
                last_err = format!("spawn {bin}: {err}");
            }
        }
    }

    if dhcp_bin(UDHCPC_BINS).is_none() && dhcp_bin(DHCLIENT_BINS).is_none() {
        return Err(NetError::Msg(format!(
            "no DHCP client found for {iface} (tried /usr/sbin/udhcpc)"
        )));
    }
    Err(NetError::Msg(format!(
        "DHCP failed for {iface} (no IPv4): {last_err}"
    )))
}
