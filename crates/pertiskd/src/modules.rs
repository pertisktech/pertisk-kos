//! Load essential kernel modules shipped in the image.
//!
//! Alpine `linux-virt` builds `virtio_net` (and friends) as modules. Without
//! them, Proxmox/QEMU virtio NICs never appear (no eth0). Without `sd_mod` and
//! its deps (`t10-pi` → `crc64-rocksoft` → `crc64`), virtio-scsi disks never
//! create `/dev/sd*` nodes — STATE/EPHEMERAL stay ephemeral and reboot wipes
//! apply/bootstrap. ESXi uses LSI Logic Parallel + e1000e/vmxnet3 — needs
//! `mptspi` (+ `mptbase`/`mptscsih`/`scsi_transport_spi`) and `e1000e`/`vmxnet3`.
//! Host Client VGA needs `simpledrm`/`vmwgfx` (linux-virt builds `CONFIG_FB=m`).

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
    /// NFS client (`sunrpc`…`nfsv4`): in-tree NFS PVs / nfs-subdir provisioner.
    const BOOT_MODULES: &[&str] = &[
        // Console first so ESXi Host Client leaves the frozen EFI stub line.
        "fbdev",
        "fb_io_fops",
        "fb",
        "fb_sys_fops",
        "syscopyarea",
        "sysfillrect",
        "sysimgblt",
        "drm_panel_orientation_quirks",
        "i2c-core",
        "drm",
        "drm_kms_helper",
        "drm_shmem_helper",
        "simpledrm",
        "ttm",
        "drm_ttm_helper",
        "vmwgfx",
        "failover",
        "net_failover",
        "virtio_net",
        // ESXi: e1000e (CreateVM default) + vmxnet3 (paravirt NIC).
        "e1000e",
        "vmxnet3",
        // Early: kube-vip needs AF_PACKET as soon as the VIP static pod starts.
        "af_packet",
        // scsi_common + scsi_mod must precede virtio_scsi/sd_mod (finit_module
        // does not auto-load deps). On amd64 linux-virt these are often builtin;
        // on aarch64 they are modules — without them /sys/block stays empty,
        // EPHEMERAL never mounts, /var stays ~2GiB tmpfs → disk-pressure.
        "scsi_common",
        "scsi_mod",
        "virtio_scsi",
        "virtio_blk",
        "cdrom",
        "sr_mod",
        "isofs",
        "ata_piix",
        "ahci",
        // ESXi VirtualLsiLogicController → mptspi (Fusion SPI).
        "scsi_transport_spi",
        "mptbase",
        "mptscsih",
        "mptspi",
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
        // NFS client (storage extension) — netfs/fscache before nfs (linux 6.6+)
        "netfs",
        "fscache",
        "sunrpc",
        "lockd",
        "grace",
        "auth_rpcgss",
        "nfs",
        "nfsv2",
        "nfsv3",
        "nfsv4",
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
        // Guard against circular depends= in .modinfo.
        thread_local! {
            static LOADING: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let already = LOADING.with(|stack| stack.borrow().iter().any(|n| n == &name));
        if already {
            return Ok(());
        }
        LOADING.with(|stack| stack.borrow_mut().push(name.clone()));
        let result = (|| {
            // finit_module does not pull deps — load `depends=` from .modinfo first.
            for dep in module_depends(path) {
                let dep_path = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(format!("{dep}.ko"));
                match load_module(&dep_path) {
                    Ok(()) => {}
                    Err(err) if err.contains("File exists") || err.contains("EEXIST") => {}
                    Err(err) if err.contains("missing ") => {
                        // Builtin or already provided under another name — continue.
                        warn!(module = %dep, error = %err, "module dependency missing; continuing");
                    }
                    Err(err) => {
                        warn!(module = %dep, error = %err, "module dependency load failed");
                    }
                }
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
        })();
        LOADING.with(|stack| {
            stack.borrow_mut().pop();
        });
        result
    }

    /// Parse `depends=foo,bar` from a module's `.modinfo` blob (scanned raw).
    fn module_depends(path: &Path) -> Vec<String> {
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        const KEY: &[u8] = b"depends=";
        let mut deps = Vec::new();
        let mut i = 0;
        while i + KEY.len() < bytes.len() {
            if &bytes[i..i + KEY.len()] != KEY {
                i += 1;
                continue;
            }
            let start = i + KEY.len();
            let end = bytes[start..]
                .iter()
                .position(|&b| b == 0)
                .map(|n| start + n)
                .unwrap_or(bytes.len());
            let list = String::from_utf8_lossy(&bytes[start..end]);
            for dep in list.split(',') {
                let dep = dep.trim();
                if !dep.is_empty() {
                    // modinfo uses dashes; ko files use underscores interchangeably.
                    deps.push(dep.replace('-', "_"));
                }
            }
            break;
        }
        deps
    }
}
