//! Host networking for Pertisk KOS (Phase 1 / M2).
//!
//! Brings links up, applies static addressing via netlink/ioctl, or requests
//! DHCPv4 via the **in-process** client (`dhcp::run_dhcp`) with T1 renew /
//! T2 rebind maintainers. No BusyBox `udhcpc`.

mod apply;
#[cfg(target_os = "linux")]
mod dhcp;
mod dns;
mod link;
mod provider_net;

pub use apply::{apply_network, NetError};
pub use link::{
    ipv6_enabled, is_ula_ipv6, is_usable_global_ipv6, prefer_global_ipv6, set_ipv6_enabled,
};
pub use provider_net::apply_provider_netcfg;

/// Point DHCP lease persistence at STATE (`machine/dhcp`). No-op off Linux.
#[cfg(target_os = "linux")]
pub use dhcp::set_lease_dir;

/// Point DHCP lease persistence at STATE (`machine/dhcp`). No-op off Linux.
#[cfg(not(target_os = "linux"))]
pub fn set_lease_dir(_dir: Option<&std::path::Path>) {}

/// Comma-separated `v4,v6` for kubelet `--node-ip` when dual-stack (required by k8s).
/// Prefers SLAAC GUA over synthetic ULA so the node InternalIP matches eth0.
pub fn dual_stack_node_ip(iface: &str) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let addrs = list_addresses(iface).ok()?;
        let v4 = pick_node_ipv4(addrs.iter().map(|s| s.as_str()), &[])?;
        let v6 = prefer_global_ipv6(addrs.iter().map(|s| s.as_str()))?.to_string();
        Some(format!("{v4},{v6}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = iface;
        None
    }
}

/// Drive a netlink future on a private current-thread runtime.
///
/// These helpers are called from both PID 1 (no tokio) and async Machine API
/// handlers (`join-controlplane`, etcd heal). `Runtime::block_on` inside an
/// existing runtime panics and drops the gRPC HTTP/2 stream:
/// `h2 protocol error: stream no longer needed`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn block_on_isolated<F, T>(fut: F) -> Result<T, NetError>
where
    F: std::future::Future<Output = Result<T, NetError>> + Send + 'static,
    T: Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::Builder::new()
            .name("pertisk-netlink".into())
            .spawn(move || block_on_new(fut))
            .map_err(|e| NetError::Msg(format!("spawn netlink thread: {e}")))?
            .join()
            .unwrap_or_else(|_| Err(NetError::Msg("netlink thread panicked".into())))
    } else {
        block_on_new(fut)
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn block_on_new<F, T>(fut: F) -> Result<T, NetError>
where
    F: std::future::Future<Output = Result<T, NetError>>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| NetError::Msg(e.to_string()))?;
    rt.block_on(fut)
}

/// List non-link-local addresses on an interface (Linux). Empty elsewhere.
pub fn list_addresses(iface: &str) -> Result<Vec<String>, NetError> {
    #[cfg(target_os = "linux")]
    {
        let iface = iface.to_string();
        block_on_isolated(async move { link::list_addresses(&iface).await })
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
        block_on_isolated(link::list_interfaces())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(Vec::new())
    }
}

/// Skip CNI/bridge/tunnel ifaces when picking a node advertise address.
pub fn is_virtual_iface(name: &str) -> bool {
    matches!(name, "lo" | "dummy0" | "sit0")
        || name.starts_with("docker")
        || name.starts_with("cni")
        || name.starts_with("flannel")
        || name.starts_with("cilium")
        || name.starts_with("vxlan")
        || name.starts_with("veth")
        || name.starts_with("br-")
        || name.starts_with("tun")
        || name.starts_with("wg")
        || name.starts_with("kube-")
        || name.starts_with("nodelocaldns")
}

fn is_usable_ipv4(ip: &str) -> bool {
    ip.contains('.')
        && !ip.starts_with("127.")
        && !ip.starts_with("169.254.")
        && !ip.starts_with("0.")
}

/// Split `10.1.1.10` or `10.1.1.10/24` into `(ip, prefix)`. Missing prefix → 32.
pub fn split_ip_prefix(cidr: &str) -> (&str, u8) {
    match cidr.split_once('/') {
        Some((ip, p)) => (ip, p.parse().unwrap_or(32)),
        None => (cidr, 32),
    }
}

/// Pick a node IPv4 from `ip` / `ip/prefix` strings.
///
/// Prefers prefix **< 32** (DHCP/static LAN) over `/32` (kube-vip secondary).
/// `skip_ips` is the HA VIP so kubelet/etcd never advertise the floating address.
pub fn pick_node_ipv4<'a, I>(cidrs: I, skip_ips: &[&str]) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut lan: Option<String> = None;
    let mut host: Option<String> = None;
    for cidr in cidrs {
        let (ip, prefix) = split_ip_prefix(cidr);
        if !is_usable_ipv4(ip) || skip_ips.iter().any(|s| *s == ip) {
            continue;
        }
        if prefix < 32 {
            if lan.is_none() {
                lan = Some(ip.to_string());
            }
        } else if host.is_none() {
            host = Some(ip.to_string());
        }
    }
    lan.or(host)
}

/// First DHCP/LAN IPv4 on a real NIC. Works on isolated bridges with no default
/// route (UDP-to-1.1.1.1 advertise detection fails there). Skips kube-vip `/32`.
pub fn first_global_ipv4() -> Option<String> {
    first_global_ipv4_skip(&[])
}

/// Like [`first_global_ipv4`] but never returns `skip_ips` (cluster VIP).
pub fn first_global_ipv4_skip(skip_ips: &[&str]) -> Option<String> {
    let mut cidrs = Vec::new();
    for name in list_interfaces().ok()? {
        if is_virtual_iface(&name) {
            continue;
        }
        if let Ok(addrs) = list_addresses(&name) {
            cidrs.extend(addrs);
        }
    }
    pick_node_ipv4(cidrs.iter().map(|s| s.as_str()), skip_ips)
}

/// NIC that currently holds `ip` (no prefix).
pub fn iface_holding_ipv4(ip: &str) -> Option<String> {
    let ip = ip.trim();
    if ip.is_empty() {
        return None;
    }
    for name in list_interfaces().ok()? {
        if is_virtual_iface(&name) {
            continue;
        }
        let Ok(addrs) = list_addresses(&name) else {
            continue;
        };
        for a in addrs {
            if split_ip_prefix(&a).0 == ip {
                return Some(name);
            }
        }
    }
    None
}

/// First non-virtual NIC (kube-vip `vip_interface` fallback).
pub fn first_physical_iface() -> Option<String> {
    list_interfaces()
        .ok()?
        .into_iter()
        .find(|n| !is_virtual_iface(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_ifaces_skipped() {
        assert!(is_virtual_iface("lo"));
        assert!(is_virtual_iface("cni0"));
        assert!(is_virtual_iface("vethabc"));
        assert!(is_virtual_iface("cilium_host"));
        assert!(!is_virtual_iface("eth0"));
        assert!(!is_virtual_iface("ens18"));
        assert!(!is_virtual_iface("enp1s0"));
    }

    #[tokio::test]
    async fn block_on_isolated_from_async_runtime() {
        let n = block_on_isolated(async { Ok::<_, NetError>(7u8) }).unwrap();
        assert_eq!(n, 7);
        // Same helpers join-controlplane uses (must not panic inside tokio).
        let _ = list_interfaces();
        let _ = first_global_ipv4();
        let _ = iface_holding_ipv4("10.1.1.1");
        let _ = first_physical_iface();
    }

    #[test]
    fn pick_node_ipv4_prefers_lan_over_kube_vip_slash32() {
        let addrs = ["10.1.1.254/32", "10.1.1.134/24"];
        assert_eq!(pick_node_ipv4(addrs, &[]).as_deref(), Some("10.1.1.134"));
        assert_eq!(
            pick_node_ipv4(addrs, &["10.1.1.134"]).as_deref(),
            Some("10.1.1.254")
        );
        assert_eq!(pick_node_ipv4(addrs, &["10.1.1.134", "10.1.1.254"]), None);
        assert_eq!(
            pick_node_ipv4(["10.1.1.254/32"], &[]).as_deref(),
            Some("10.1.1.254")
        );
    }
}
