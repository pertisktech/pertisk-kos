//! Load essential kernel modules shipped in the image.
//!
//! Alpine `linux-virt` builds `virtio_net` (and friends) as modules. Without
//! them, Proxmox/QEMU virtio NICs never appear (no eth0). Without `sd_mod` and
//! its deps (`t10-pi` → `crc64-rocksoft` → `crc64`), virtio-scsi disks never
//! create `/dev/sd*` nodes — STATE/EPHEMERAL stay ephemeral and reboot wipes
//! apply/bootstrap.

use tracing::info;

/// Load boot-critical modules from `/lib/pertisk/modules` (order matters).
pub fn load_boot_modules() {
    #[cfg(target_os = "linux")]
    {
        linux_impl::load_boot_modules();
    }
    #[cfg(not(target_os = "linux"))]
    {
        info!("skipping module load (not Linux)");
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::*;
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};
    use tracing::{info, warn};

    const MODULE_DIR: &str = "/lib/pertisk/modules";
    /// Dependency order for Alpine linux-virt virtio networking + disk + fs + overlay.
    /// `sd_mod` requires `t10-pi` → `crc64-rocksoft` → `crc64` (Proxmox scsi).
    /// `ext4` requires `jbd2` + `crc16` + `mbcache` (STATE/EPHEMERAL mounts).
    /// `vfat` requires `fat` (+ nls_*) for the EFI system partition.
    /// `overlay` is required for containerd/runc rootfs mounts.
    /// Shared by Flannel / Calico / Cilium (+ kube-proxy when not using eBPF KPR):
    /// bridge/br_netfilter/veth, Calico IPIP+ipset, kube-proxy xt_*, Cilium vxlan/nft.
    /// `xfrm_user` is required even without IPSec: Cilium's netlink handle opens
    /// NETLINK_XFRM (`protocol not supported` without the module).
    /// `af_packet` (CONFIG_PACKET=m): kube-vip gratuitous ARP for the control-plane VIP.
    const BOOT_MODULES: &[&str] = &[
        "failover",
        "net_failover",
        "virtio_net",
        // Early: kube-vip needs AF_PACKET as soon as the VIP static pod starts.
        "af_packet",
        "virtio_scsi",
        "virtio_blk",
        "crc64",
        "crc64-rocksoft",
        "t10-pi",
        "sd_mod",
        "crc16",
        "mbcache",
        "jbd2",
        "crc32c_generic",
        "ext4",
        "fat",
        "nls_cp437",
        "nls_iso8859-1",
        "vfat",
        "overlay",
        // Bridge / CNI veth (Flannel, Calico)
        "llc",
        "stp",
        "bridge",
        "br_netfilter",
        "veth",
        // Calico IPIP
        "tunnel4",
        "ipip",
        // Netfilter core before ip_set (ip_set needs nfnetlink symbols)
        "libcrc32c",
        "nfnetlink",
        "nf_defrag_ipv4",
        "nf_defrag_ipv6",
        "nf_conntrack",
        "nf_nat",
        // Calico ipset (after nfnetlink)
        "ip_set",
        "ip_set_hash_ip",
        "ip_set_hash_net",
        // x_tables must load before any xt_* / ip_tables (else unknown-symbol failures)
        "x_tables",
        "xt_set",
        "xt_tcpudp",
        "xt_comment",
        "xt_mark",
        "xt_conntrack",
        "nf_socket_ipv4",
        "nf_socket_ipv6",
        "xt_socket",
        "xt_nat",
        "xt_statistic",
        "xt_multiport",
        "xt_MASQUERADE",
        "xt_addrtype",
        "xt_CT",
        "nf_tproxy_ipv4",
        "nf_tproxy_ipv6",
        "xt_TPROXY",
        "xt_REDIRECT",
        "xt_rpfilter",
        "ip_tables",
        "iptable_filter",
        "iptable_nat",
        "iptable_mangle",
        "iptable_raw",
        "ip6_tables",
        "ip6table_filter",
        "ip6table_nat",
        "ip6table_mangle",
        "ip6table_raw",
        "nf_tables",
        "nft_compat",
        "udp_tunnel",
        "ip6_udp_tunnel",
        "vxlan",
        "geneve",
        // Cilium: NETLINK_XFRM + sock-diag (socket LB terminate) + tc/bpf qdisc
        "xfrm_algo",
        "xfrm_user",
        "inet_diag",
        "tcp_diag",
        "udp_diag",
        "cls_bpf",
        "act_bpf",
        "sch_fq",
    ];

    pub fn load_boot_modules() {
        let dir = PathBuf::from(MODULE_DIR);
        if !dir.is_dir() {
            warn!(
                dir = MODULE_DIR,
                "no shipped modules; virtio NICs may be missing (rebuild with fetch-kernel modules)"
            );
            return;
        }
        if let Ok(ver) = std::fs::read_to_string(dir.join("version")) {
            info!(kver = %ver.trim(), "loading boot modules");
        }
        for name in BOOT_MODULES {
            let path = dir.join(format!("{name}.ko"));
            match load_module(&path) {
                Ok(()) => {
                    info!(module = name, "module loaded");
                }
                Err(err) => {
                    // EEXIST (already loaded) is fine.
                    if err.contains("File exists") || err.contains("EEXIST") {
                        info!(module = name, "module already loaded");
                    } else {
                        warn!(module = name, error = %err, "module load failed");
                    }
                }
            }
        }
        // Prefer iptables-legacy for kube-proxy's iptables-wrapper (CNI uses legacy tables).
        ensure_iptables_legacy_hint();
    }

    /// Create `KUBE-IPTABLES-HINT` in the legacy mangle table so kube-proxy's
    /// `/usr/sbin/iptables-wrapper` selects legacy instead of broken nft on linux-virt.
    fn ensure_iptables_legacy_hint() {
        use std::process::Command;
        let ipt = "/usr/sbin/iptables-legacy";
        if !Path::new(ipt).is_file() && !Path::new("/usr/sbin/iptables").is_file() {
            return;
        }
        let bin = if Path::new(ipt).is_file() {
            ipt
        } else {
            "/usr/sbin/iptables"
        };
        let _ = Command::new(bin)
            .args(["-t", "mangle", "-N", "KUBE-IPTABLES-HINT"])
            .status();
        let _ = Command::new(bin)
            .args(["-t", "mangle", "-C", "KUBE-IPTABLES-HINT", "-j", "RETURN"])
            .status()
            .ok()
            .filter(|s| s.success())
            .or_else(|| {
                Command::new(bin)
                    .args(["-t", "mangle", "-A", "KUBE-IPTABLES-HINT", "-j", "RETURN"])
                    .status()
                    .ok()
            });
    }

    fn load_module(path: &Path) -> Result<(), String> {
        if !path.is_file() {
            return Err(format!("missing {}", path.display()));
        }
        use std::ffi::CString;

        let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        let fd = file.as_raw_fd();
        let params = CString::new("").map_err(|e| e.to_string())?;
        // SAFETY: finit_module with a valid fd and NUL-terminated empty params.
        let rc = unsafe { libc::syscall(libc::SYS_finit_module, fd, params.as_ptr(), 0_i32) };
        if rc == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        Err(format!("{err}"))
    }
}
