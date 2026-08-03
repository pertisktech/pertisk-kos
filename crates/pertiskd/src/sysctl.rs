//! Kernel sysctls required before kubelet with `protectKernelDefaults: true`.

use tracing::debug;

/// Apply CIS / Kubernetes node sysctls via `/proc/sys`.
///
/// Soft-fails when `/proc/sys` is missing (dev hosts) or a key is unavailable.
pub fn apply_hardening_sysctls() {
    #[cfg(target_os = "linux")]
    {
        linux_impl::apply();
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

    pub fn apply() {
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
