//! Linux netlink link/address helpers + DHCP via system clients.

#[cfg(target_os = "linux")]
use crate::apply::NetError;

#[cfg(target_os = "linux")]
use futures::stream::TryStreamExt;

use std::sync::atomic::{AtomicBool, Ordering};

static ALLOW_IPV6: AtomicBool = AtomicBool::new(false);

/// When true, skip per-iface IPv6 disable (dual-stack / SLAAC).
pub fn set_ipv6_enabled(enabled: bool) {
    ALLOW_IPV6.store(enabled, Ordering::Relaxed);
}

pub fn ipv6_enabled() -> bool {
    ALLOW_IPV6.load(Ordering::Relaxed)
}

/// Affirmatively allow SLAAC/RA on an interface (dual-stack).
#[cfg(target_os = "linux")]
pub fn enable_iface_ipv6(iface: &str) {
    let base = format!("/proc/sys/net/ipv6/conf/{iface}");
    for (key, val) in [("disable_ipv6", "0"), ("accept_ra", "1"), ("autoconf", "1")] {
        let path = format!("{base}/{key}");
        let _ = std::fs::write(&path, val);
    }
}

#[cfg(target_os = "linux")]
pub async fn set_link_up(iface: &str) -> Result<(), NetError> {
    use rtnetlink::new_connection;

    if ipv6_enabled() {
        enable_iface_ipv6(iface);
    } else {
        // Prefer IPv4-only before carrier comes up (SLAAC races otherwise).
        disable_iface_ipv6(iface);
    }

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

    if ipv6_enabled() {
        enable_iface_ipv6(iface);
    } else {
        disable_iface_ipv6(iface);
    }

    tracing::info!(interface = iface, "link up");
    Ok(())
}

/// Best-effort: turn off IPv6 / RA / autoconf on one interface (IPv4-only mode).
#[cfg(target_os = "linux")]
fn disable_iface_ipv6(iface: &str) {
    if ipv6_enabled() {
        return;
    }
    let base = format!("/proc/sys/net/ipv6/conf/{iface}");
    for (key, val) in [("disable_ipv6", "1"), ("accept_ra", "0"), ("autoconf", "0")] {
        let path = format!("{base}/{key}");
        let _ = std::fs::write(&path, val);
    }
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
                        NetError::Msg(format!("IPv4 add failed ioctl=({ioctl_err}) netlink=({e})"))
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
pub fn add_ipv4_ioctl(iface: &str, ip: std::net::Ipv4Addr, prefix: u8) -> Result<(), NetError> {
    use std::os::fd::AsRawFd;

    let sock = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)
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

    let sock = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)
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

    let sock = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)
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
                    // Global/ULA IPv6 only — hide link-local so fe80 alone ≠ "up".
                    IpAddr::V6(v6) => {
                        !v6.is_unicast_link_local()
                            && !v6.is_loopback()
                            && !v6.is_multicast()
                            && !v6.is_unspecified()
                    }
                };
                if keep {
                    out.push(format!("{ip}/{}", addr.header.prefix_len));
                }
            }
        }
    }
    Ok(out)
}

/// Prefer a public/global IPv6 (SLAAC GUA) over synthetic ULA (`fd00::/8`).
/// Returns the address **without** a `/prefix` when present in `cidr` form.
pub fn prefer_global_ipv6<'a, I>(addrs: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut ula: Option<&str> = None;
    for a in addrs {
        let ip = a.split('/').next().unwrap_or(a);
        if !is_usable_global_ipv6(ip) {
            continue;
        }
        if is_ula_ipv6(ip) {
            if ula.is_none() {
                ula = Some(ip);
            }
        } else {
            // GUA / non-ULA global wins immediately.
            return Some(ip);
        }
    }
    ula
}

/// True for non-link-local, non-loopback, non-multicast IPv6.
pub fn is_usable_global_ipv6(ip: &str) -> bool {
    ip.contains(':')
        && !ip.starts_with("fe80:")
        && !ip.eq_ignore_ascii_case("::1")
        && !ip.to_ascii_lowercase().starts_with("ff")
}

/// Unique-local `fc00::/7` (typically `fd00::/8` in labs).
pub fn is_ula_ipv6(ip: &str) -> bool {
    let lower = ip.to_ascii_lowercase();
    lower.starts_with("fc") || lower.starts_with("fd")
}

/// Stable ULA derived from IPv4 (`10.1.1.173` → `fd00:a:1:1::ad/64`).
pub fn ula_cidr_from_ipv4(v4: std::net::Ipv4Addr) -> String {
    let o = v4.octets();
    format!("fd00:{:x}:{:x}:{:x}::{:x}/64", o[0], o[1], o[2], o[3])
}

/// If dual-stack is on and the iface has IPv4 but no global/ULA IPv6, add a
/// stable ULA derived from the IPv4. Prefer SLAAC GUA (`2405:…`) when the LAN
/// sends RAs — wait briefly before synthesizing ULA so kubelet `--node-ip`
/// matches the real eth0 address. When a GUA is present, drop the synthetic ULA.
#[cfg(target_os = "linux")]
pub async fn ensure_stable_ula(iface: &str) -> Result<(), NetError> {
    if !ipv6_enabled() {
        return Ok(());
    }
    enable_iface_ipv6(iface);

    // Give SLAAC a short window (Proxmox bridges often RA within ~1–3s).
    let mut addrs = list_addresses(iface).await?;
    for _ in 0..16 {
        if addrs.iter().any(|a| {
            let ip = a.split('/').next().unwrap_or(a.as_str());
            is_usable_global_ipv6(ip) && !is_ula_ipv6(ip)
        }) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        addrs = list_addresses(iface).await?;
    }

    let v4 = addrs.iter().find_map(|a| {
        let ip = a.split('/').next().unwrap_or(a.as_str());
        if ip.contains('.') {
            ip.parse::<std::net::Ipv4Addr>().ok()
        } else {
            None
        }
    });

    let has_gua = addrs.iter().any(|a| {
        let ip = a.split('/').next().unwrap_or(a.as_str());
        is_usable_global_ipv6(ip) && !is_ula_ipv6(ip)
    });
    if has_gua {
        if let Some(v4) = v4 {
            let synthetic = ula_cidr_from_ipv4(v4);
            let syn_ip = synthetic.split('/').next().unwrap_or(synthetic.as_str());
            let still_present = addrs
                .iter()
                .any(|a| a.split('/').next().unwrap_or(a.as_str()) == syn_ip);
            if still_present {
                match del_address(iface, &synthetic).await {
                    Ok(()) => tracing::info!(
                        interface = iface,
                        ula = %synthetic,
                        "removed synthetic ULA (GUA present)"
                    ),
                    Err(err) => tracing::warn!(
                        interface = iface,
                        ula = %synthetic,
                        error = %err,
                        "failed to remove synthetic ULA"
                    ),
                }
            }
        }
        if let Some(gua) = prefer_global_ipv6(addrs.iter().map(|s| s.as_str())) {
            tracing::info!(interface = iface, ipv6 = %gua, "dual-stack using SLAAC GUA");
        }
        return Ok(());
    }

    let has_global_v6 = addrs.iter().any(|a| {
        let ip = a.split('/').next().unwrap_or(a.as_str());
        is_usable_global_ipv6(ip)
    });
    if has_global_v6 {
        return Ok(());
    }
    let Some(v4) = v4 else {
        return Ok(());
    };
    let ula = ula_cidr_from_ipv4(v4);
    match add_address(iface, &ula).await {
        Ok(()) => {
            tracing::info!(interface = iface, ula = %ula, "dual-stack ULA assigned (no RA)");
            Ok(())
        }
        Err(err) => {
            tracing::warn!(interface = iface, ula = %ula, error = %err, "ULA assign failed");
            // Soft-fail — DHCP/API still work on IPv4.
            Ok(())
        }
    }
}

/// Best-effort delete of an address CIDR on an interface.
#[cfg(target_os = "linux")]
pub async fn del_address(iface: &str, cidr: &str) -> Result<(), NetError> {
    use std::process::Command;
    use std::str::FromStr;

    use ipnet::IpNet;
    use netlink_packet_route::address::AddressAttribute;
    use rtnetlink::new_connection;

    // Prefer iproute2 `ip` (shipped in the image); fall back to netlink below.
    for bin in ["/sbin/ip", "/usr/sbin/ip", "ip"] {
        let status = Command::new(bin)
            .args(["addr", "del", cidr, "dev", iface])
            .status();
        if matches!(status, Ok(s) if s.success()) {
            return Ok(());
        }
    }

    let net = IpNet::from_str(cidr).map_err(|e| NetError::Msg(format!("bad CIDR {cidr}: {e}")))?;
    let want = net.addr();
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
        let matches = addr
            .attributes
            .iter()
            .any(|attr| matches!(attr, AddressAttribute::Address(ip) if *ip == want));
        if !matches {
            continue;
        }
        handle
            .address()
            .del(addr)
            .execute()
            .await
            .map_err(|e| NetError::Msg(e.to_string()))?;
        return Ok(());
    }
    Err(NetError::Msg(format!(
        "address {cidr} not found on {iface}"
    )))
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

/// Apply a DHCPv4 lease via ioctl (no netlink) — used by the builtin DHCP client.
#[cfg(target_os = "linux")]
pub fn apply_dhcp_v4_lease(
    iface: &str,
    ip: std::net::Ipv4Addr,
    prefix: u8,
    routers: &[std::net::Ipv4Addr],
) -> Result<(), NetError> {
    let _ = del_ipv4_ioctl(iface);
    add_ipv4_ioctl(iface, ip, prefix)?;
    for gw in routers {
        if let Err(err) = add_default_route_v4_ioctl(*gw) {
            tracing::warn!(interface = iface, gateway = %gw, error = %err, "DHCP default route failed");
        }
    }
    Ok(())
}

/// Run DHCPv4 on `iface` via the in-process client (no BusyBox / udhcpc).
#[cfg(target_os = "linux")]
pub fn run_dhcp(iface: &str) -> Result<(), NetError> {
    use std::time::Duration;

    wait_carrier(iface, Duration::from_secs(10))?;

    if let Err(err) = crate::dhcp::run_dhcp(iface) {
        tracing::warn!(interface = iface, error = %err, "builtin DHCP failed");
        return Err(err);
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| NetError::Msg(e.to_string()))?;
    let addrs = rt.block_on(list_addresses(iface)).unwrap_or_default();
    if addrs.iter().any(|a| a.contains('.')) {
        tracing::info!(
            interface = iface,
            addresses = ?addrs,
            "builtin DHCP IPv4 lease applied"
        );
        return Ok(());
    }
    Err(NetError::Msg(format!(
        "DHCP failed for {iface}: no IPv4 after lease (addrs={addrs:?})"
    )))
}
