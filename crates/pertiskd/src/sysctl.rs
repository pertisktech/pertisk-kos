//! Kernel sysctls required before kubelet with `protectKernelDefaults: true`.

use std::sync::atomic::{AtomicBool, Ordering};

use tracing::debug;

static DUAL_STACK: AtomicBool = AtomicBool::new(false);

/// Remember whether the machine config wants dual-stack (IPv4+IPv6).
pub fn set_dual_stack(enabled: bool) {
    DUAL_STACK.store(enabled, Ordering::Relaxed);
}

pub fn is_dual_stack() -> bool {
    DUAL_STACK.load(Ordering::Relaxed)
}

/// Apply IPv6 on/off policy from machine config (`cluster.networkMode`).
pub fn apply_ipv6_policy(dual_stack: bool) {
    set_dual_stack(dual_stack);
    pertisk_net::set_ipv6_enabled(dual_stack);
    if dual_stack {
        enable_ipv6();
    } else {
        disable_ipv6();
    }
}

/// Disable IPv6 addressing (SLAAC / DHCPv6 / link-local) on all interfaces.
///
/// Call **before** network bring-up so Proxmox guests never pick up `fe80::`
/// or global IPv6 from the LAN. Soft-fails when the IPv6 stack is absent.
/// No-op when [`set_dual_stack`](true).
pub fn disable_ipv6() {
    if is_dual_stack() {
        debug!("skipping IPv6 disable (dual-stack)");
        return;
    }
    #[cfg(target_os = "linux")]
    {
        linux_impl::apply_ipv6_off();
    }
    #[cfg(not(target_os = "linux"))]
    {
        debug!("skipping IPv6 disable (not Linux)");
    }
}

/// Re-enable IPv6 after an earlier IPv4-only boot (dual-stack config applied late).
pub fn enable_ipv6() {
    #[cfg(target_os = "linux")]
    {
        linux_impl::apply_ipv6_on();
    }
    #[cfg(not(target_os = "linux"))]
    {
        debug!("skipping IPv6 enable (not Linux)");
    }
}

/// Apply CIS / Kubernetes node sysctls via `/proc/sys`.
///
/// Soft-fails when `/proc/sys` is missing (dev hosts) or a key is unavailable.
/// Keeps IPv6 off unless dual-stack is configured.
pub fn apply_hardening_sysctls() {
    #[cfg(target_os = "linux")]
    {
        linux_impl::apply(is_dual_stack());
    }
    #[cfg(not(target_os = "linux"))]
    {
        debug!("skipping sysctls (not Linux)");
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use std::fs;
    use std::path::Path;

    use tracing::{debug, info, warn};

    const IPV6_OFF: &[(&str, &str)] = &[
        ("net/ipv6/conf/all/disable_ipv6", "1"),
        ("net/ipv6/conf/default/disable_ipv6", "1"),
        ("net/ipv6/conf/all/accept_ra", "0"),
        ("net/ipv6/conf/default/accept_ra", "0"),
        ("net/ipv6/conf/all/autoconf", "0"),
        ("net/ipv6/conf/default/autoconf", "0"),
    ];

    const IPV6_ON: &[(&str, &str)] = &[
        ("net/ipv6/conf/all/disable_ipv6", "0"),
        ("net/ipv6/conf/default/disable_ipv6", "0"),
        ("net/ipv6/conf/all/accept_ra", "1"),
        ("net/ipv6/conf/default/accept_ra", "1"),
        ("net/ipv6/conf/all/autoconf", "1"),
        ("net/ipv6/conf/default/autoconf", "1"),
    ];

    /// (proc path relative to /proc/sys, value)
    const SETTINGS: &[(&str, &str)] = &[
        // Kubernetes networking
        ("net/ipv4/ip_forward", "1"),
        ("net/bridge/bridge-nf-call-iptables", "1"),
        ("net/bridge/bridge-nf-call-ip6tables", "1"),
        // CIS / kubelet protectKernelDefaults expectations
        ("vm/overcommit_memory", "1"),
        ("kernel/panic", "10"),
        ("kernel/panic_on_oops", "1"),
        // Hardening
        ("kernel/kptr_restrict", "1"),
        ("kernel/dmesg_restrict", "1"),
        ("net/ipv4/conf/all/rp_filter", "2"),
        ("net/ipv4/conf/default/rp_filter", "2"),
        ("net/ipv4/conf/all/accept_source_route", "0"),
        ("net/ipv4/conf/default/accept_source_route", "0"),
        ("net/ipv4/conf/all/accept_redirects", "0"),
        ("net/ipv4/conf/default/accept_redirects", "0"),
        ("net/ipv6/conf/all/accept_redirects", "0"),
        ("net/ipv6/conf/default/accept_redirects", "0"),
        ("net/ipv4/tcp_syncookies", "1"),
        // Kubernetes UserNamespacesSupport / pod hostUsers (K8s ≥1.33).
        // Alpine linux-virt defaults this to 0 → kubelet rejects hostUsers.
        ("user/max_user_namespaces", "65536"),
    ];

    pub fn apply_ipv6_off() {
        let mut ok = 0usize;
        for (rel, value) in IPV6_OFF {
            let path = Path::new("/proc/sys").join(rel);
            match write_sysctl(&path, value) {
                Ok(()) => {
                    debug!(path = %path.display(), value, "IPv6 sysctl set");
                    ok += 1;
                }
                Err(err) => {
                    debug!(path = %path.display(), error = %err, "IPv6 sysctl skipped");
                }
            }
        }
        // Also flip any already-present iface (eth0 may exist before DHCP).
        if let Ok(entries) = fs::read_dir("/proc/sys/net/ipv6/conf") {
            for e in entries.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name == "all" || name == "default" {
                    continue;
                }
                let path = e.path().join("disable_ipv6");
                let _ = write_sysctl(&path, "1");
                let _ = write_sysctl(&e.path().join("accept_ra"), "0");
                let _ = write_sysctl(&e.path().join("autoconf"), "0");
            }
        }
        if ok > 0 {
            info!(ok, "IPv6 disabled on guest (IPv4-only)");
        }
    }

    pub fn apply_ipv6_on() {
        let mut ok = 0usize;
        for (rel, value) in IPV6_ON {
            let path = Path::new("/proc/sys").join(rel);
            if write_sysctl(&path, value).is_ok() {
                ok += 1;
            }
        }
        if let Ok(entries) = fs::read_dir("/proc/sys/net/ipv6/conf") {
            for e in entries.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name == "all" || name == "default" {
                    continue;
                }
                let _ = write_sysctl(&e.path().join("disable_ipv6"), "0");
                let _ = write_sysctl(&e.path().join("accept_ra"), "1");
                let _ = write_sysctl(&e.path().join("autoconf"), "1");
            }
        }
        if ok > 0 {
            info!(ok, "IPv6 enabled on guest (dual-stack)");
        }
    }

    pub fn apply(dual_stack: bool) {
        if dual_stack {
            apply_ipv6_on();
        } else {
            apply_ipv6_off();
        }
        let mut ok = 0usize;
        let mut skip = 0usize;
        for (rel, value) in SETTINGS {
            let path = Path::new("/proc/sys").join(rel);
            match write_sysctl(&path, value) {
                Ok(()) => {
                    debug!(path = %path.display(), value, "sysctl set");
                    ok += 1;
                }
                Err(err) => {
                    // bridge keys need br_netfilter; absent until module/CNI loads.
                    debug!(path = %path.display(), error = %err, "sysctl skipped");
                    skip += 1;
                }
            }
        }
        if ok > 0 {
            info!(ok, skip, "hardening sysctls applied");
        } else {
            warn!(skip, "no sysctls applied (/proc/sys unavailable?)");
        }
    }

    fn write_sysctl(path: &Path, value: &str) -> std::io::Result<()> {
        if !path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "sysctl path missing",
            ));
        }
        fs::write(path, value)
    }
}
