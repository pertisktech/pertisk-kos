//! EPHEMERAL volume discovery and mount over `/var`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;
use tracing::{info, warn};

use crate::layout::{MountPaths, PARTLABEL_EPHEMERAL};
use crate::partlabel::{find_by_partlabel, wait_for_partlabel};

#[derive(Debug, Error)]
pub enum EphemeralError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("EPHEMERAL device not found")]
    DeviceNotFound,
    #[error("mount failed: {0}")]
    Mount(String),
}

/// Mounted EPHEMERAL backing writable `/var`.
#[derive(Debug, Clone)]
pub struct EphemeralVolume {
    pub root: PathBuf,
}

/// Mount PARTLABEL `EPHEMERAL` and bind it over `/var` when present.
///
/// Order:
/// 1. Mount the partition at `/system/ephemeral`.
/// 2. Bind-mount that onto `/var` (replacing the early tmpfs).
///
/// Returns `Ok(None)` when no partition exists (initramfs-only smoke).
pub fn prepare_ephemeral() -> Result<Option<EphemeralVolume>, EphemeralError> {
    let paths = MountPaths::standard();
    prepare_ephemeral_at(Path::new(paths.ephemeral), Path::new(paths.var))
}

/// Mount EPHEMERAL at `mountpoint` and bind onto `var`.
pub fn prepare_ephemeral_at(
    mountpoint: &Path,
    var: &Path,
) -> Result<Option<EphemeralVolume>, EphemeralError> {
    #[cfg(target_os = "linux")]
    {
        let Some(dev) = find_ephemeral_device() else {
            warn!("no EPHEMERAL partition; keeping tmpfs /var");
            return Ok(None);
        };
        Ok(Some(mount_ephemeral_partition(&dev, mountpoint, var)?))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (mountpoint, var);
        Ok(None)
    }
}

/// Best-effort EPHEMERAL prepare; logs and continues on soft failures.
pub fn try_prepare_ephemeral() -> Option<EphemeralVolume> {
    match prepare_ephemeral() {
        Ok(v) => v,
        Err(err) => {
            warn!(error = %err, "EPHEMERAL mount failed");
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn find_ephemeral_device() -> Option<PathBuf> {
    if let Some(dev) = wait_for_partlabel(PARTLABEL_EPHEMERAL, Duration::from_secs(5)) {
        info!(device = %dev.display(), "found EPHEMERAL partition");
        return Some(dev);
    }
    if let Some(dev) = find_by_partlabel(PARTLABEL_EPHEMERAL) {
        return Some(dev);
    }
    for dev in guess_ephemeral_nodes() {
        info!(device = %dev.display(), "guessing EPHEMERAL partition node");
        return Some(dev);
    }
    None
}

/// Fallback nodes (cloud layout: EFI=1 … STATE=5, EPHEMERAL=6).
#[cfg(target_os = "linux")]
fn guess_ephemeral_nodes() -> impl Iterator<Item = PathBuf> {
    ["/dev/vda6", "/dev/sda6", "/dev/nvme0n1p6", "/dev/xvda6"]
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

#[cfg(target_os = "linux")]
fn mount_ephemeral_partition(
    dev: &Path,
    mountpoint: &Path,
    var: &Path,
) -> Result<EphemeralVolume, EphemeralError> {
    use nix::mount::{mount, MsFlags};

    fs::create_dir_all(mountpoint)?;
    let mut last_err = None;
    for attempt in 1..=8 {
        match mount(
            Some(dev),
            mountpoint,
            Some("ext4"),
            MsFlags::MS_RELATIME,
            None::<&str>,
        ) {
            Ok(()) => {
                info!(
                    device = %dev.display(),
                    target = %mountpoint.display(),
                    attempt,
                    "mounted EPHEMERAL"
                );
                last_err = None;
                break;
            }
            Err(nix::errno::Errno::EBUSY) => {
                info!(target = %mountpoint.display(), "EPHEMERAL already mounted");
                last_err = None;
                break;
            }
            Err(err) => {
                last_err = Some(err);
                warn!(
                    device = %dev.display(),
                    target = %mountpoint.display(),
                    device_exists = dev.exists(),
                    mountpoint_exists = mountpoint.exists(),
                    attempt,
                    error = %err,
                    "EPHEMERAL mount attempt failed"
                );
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    if let Some(err) = last_err {
        return Err(EphemeralError::Mount(err.to_string()));
    }

    // Ensure expected subdirs exist on the disk before covering /var.
    for sub in ["lib", "log", "tmp"] {
        fs::create_dir_all(mountpoint.join(sub))?;
    }

    fs::create_dir_all(var)?;
    match mount(
        Some(mountpoint),
        var,
        None::<&str>,
        MsFlags::MS_BIND,
        None::<&str>,
    ) {
        Ok(()) => {
            info!(from = %mountpoint.display(), to = %var.display(), "bound EPHEMERAL → /var")
        }
        Err(nix::errno::Errno::EBUSY) => {
            info!(target = %var.display(), "/var already bound from EPHEMERAL");
        }
        Err(err) => return Err(EphemeralError::Mount(format!("bind /var: {err}"))),
    }

    // Cilium hostPath Bidirectional on /var/run/netns requires the /var mount
    // to be shared/slave. Bind mounts default to private.
    if let Err(err) = mount(
        None::<&str>,
        var,
        None::<&str>,
        MsFlags::MS_REC | MsFlags::MS_SHARED,
        None::<&str>,
    ) {
        warn!(target = %var.display(), error = %err, "make-rshared /var after EPHEMERAL bind failed");
    } else {
        info!(target = %var.display(), "EPHEMERAL /var mount propagation set to rshared");
    }

    Ok(EphemeralVolume {
        root: var.to_path_buf(),
    })
}
