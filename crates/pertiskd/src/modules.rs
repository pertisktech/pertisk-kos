//! Load essential kernel modules shipped in the image.
//!
//! Alpine `linux-virt` builds `virtio_net` (and friends) as modules. Without
//! them, Proxmox/QEMU virtio NICs never appear (no eth0). Without `sd_mod` and
//! its deps (`t10-pi` → `crc64-rocksoft` → `crc64`), virtio-scsi disks never
//! create `/dev/sd*` nodes — STATE/EPHEMERAL stay ephemeral and reboot wipes
//! apply/bootstrap.

use tracing::{info, warn};

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

    const MODULE_DIR: &str = "/lib/pertisk/modules";
    /// Dependency order for Alpine linux-virt virtio networking + disk + fs + overlay.
    /// `sd_mod` requires `t10-pi` → `crc64-rocksoft` → `crc64` (Proxmox scsi).
    /// `ext4` requires `jbd2` + `crc16` + `mbcache` (STATE/EPHEMERAL mounts).
    /// `vfat` requires `fat` (+ nls_*) for the EFI system partition.
    /// `overlay` is required for containerd/runc rootfs mounts.
    const BOOT_MODULES: &[&str] = &[
        "failover",
        "net_failover",
        "virtio_net",
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
