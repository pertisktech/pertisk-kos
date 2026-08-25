//! EFI System Partition mount helpers.

use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::warn;

use crate::layout::{MountPaths, PARTLABEL_EFI};

#[cfg(target_os = "linux")]
use crate::partlabel::find_by_partlabel;

#[derive(Debug, Error)]
pub enum EspError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("EFI device not found (expected /dev/disk/by-partlabel/{PARTLABEL_EFI})")]
    DeviceNotFound,
    #[error("mount failed: {0}")]
    Mount(String),
}

/// A mounted (or directory) ESP.
#[derive(Debug, Clone)]
pub struct EspVolume {
    pub root: PathBuf,
}

/// Mount PARTLABEL `EFI` at the standard mount point when present.
///
/// Returns `Ok(None)` when no EFI partition exists (dev/QEMU without disk).
pub fn prepare_esp() -> Result<Option<EspVolume>, EspError> {
    let paths = MountPaths::standard();
    prepare_esp_at(Path::new(paths.efi))
}

/// Mount EFI at `mountpoint`, or `Ok(None)` if the device is missing.
pub fn prepare_esp_at(mountpoint: &Path) -> Result<Option<EspVolume>, EspError> {
    #[cfg(target_os = "linux")]
    {
        let Some(dev) = find_efi_device() else {
            return Ok(None);
        };
        Ok(Some(mount_efi_partition(&dev, mountpoint)?))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = mountpoint;
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
fn find_efi_device() -> Option<PathBuf> {
    find_by_partlabel(PARTLABEL_EFI).or_else(|| {
        ["/dev/vda1", "/dev/sda1", "/dev/nvme0n1p1"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
    })
}

#[cfg(target_os = "linux")]
fn mount_efi_partition(dev: &Path, mountpoint: &Path) -> Result<EspVolume, EspError> {
    use std::fs;

    use nix::mount::{mount, MsFlags};
    use tracing::info;

    fs::create_dir_all(mountpoint)?;
    match mount(
        Some(dev),
        mountpoint,
        Some("vfat"),
        MsFlags::MS_RELATIME,
        Some("umask=0077,codepage=437,iocharset=iso8859-1"),
    ) {
        Ok(()) => info!(device = %dev.display(), target = %mountpoint.display(), "mounted EFI"),
        Err(nix::errno::Errno::EBUSY) => {
            info!(target = %mountpoint.display(), "EFI already mounted");
        }
        Err(err) => {
            // Retry without iocharset if nls naming differs across kernels.
            match mount(
                Some(dev),
                mountpoint,
                Some("vfat"),
                MsFlags::MS_RELATIME,
                Some("umask=0077"),
            ) {
                Ok(()) => info!(
                    device = %dev.display(),
                    target = %mountpoint.display(),
                    "mounted EFI (basic options)"
                ),
                Err(nix::errno::Errno::EBUSY) => {
                    info!(target = %mountpoint.display(), "EFI already mounted");
                }
                Err(err2) => return Err(EspError::Mount(format!("{err}; retry: {err2}"))),
            }
        }
    }
    Ok(EspVolume {
        root: mountpoint.to_path_buf(),
    })
}

/// Best-effort ESP prepare for boot; logs and continues on soft failures.
pub fn try_prepare_esp() -> Option<EspVolume> {
    match prepare_esp() {
        Ok(v) => v,
        Err(err) => {
            warn!(error = %err, "EFI mount failed");
            None
        }
    }
}
