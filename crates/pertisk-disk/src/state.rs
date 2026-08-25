//! STATE volume discovery, mount, and directory layout.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::info;

use crate::layout::{MountPaths, PARTLABEL_STATE};

#[cfg(target_os = "linux")]
use crate::partlabel::{guess_state_nodes, wait_for_partlabel};
#[cfg(target_os = "linux")]
use std::time::Duration;

/// Default machine config filename under STATE.
pub const DEFAULT_CONFIG_NAME: &str = "config.yaml";

#[derive(Debug, Error)]
pub enum StateError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("STATE device not found (expected /dev/disk/by-partlabel/{PARTLABEL_STATE})")]
    DeviceNotFound,
    #[error("mount failed: {0}")]
    Mount(String),
}

/// A prepared STATE volume (directory mount or bind).
#[derive(Debug, Clone)]
pub struct StateVolume {
    /// Absolute path where STATE is available.
    pub root: PathBuf,
    /// How STATE was obtained.
    pub source: StateSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateSource {
    /// Developer / QEMU tmpfs path (`--state-dir`).
    Directory,
    /// Mounted block device by PARTLABEL.
    Partition,
    /// Created on tmpfs when no disk is present (initramfs smoke).
    EphemeralTmpfs,
}

impl StateVolume {
    pub fn config_path(&self) -> PathBuf {
        self.root.join(DEFAULT_CONFIG_NAME)
    }

    /// Ensure expected subdirectories exist under STATE.
    pub fn ensure_layout(&self) -> Result<(), StateError> {
        for sub in ["machine", "secrets", "log"] {
            fs::create_dir_all(self.root.join(sub))?;
        }
        // Secrets hold trust keys / TLS material — owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let secrets = self.root.join("secrets");
            let mut perms = fs::metadata(&secrets)?.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&secrets, perms)?;
        }
        Ok(())
    }
}

/// Prepare STATE for boot.
///
/// Order:
/// 1. If `state_dir` is set, use that directory (dev / tests).
/// 2. Else if PARTLABEL `STATE` exists, mount it at `/system/state`.
/// 3. Else create `/system/state` on the current root (tmpfs/initramfs fallback).
pub fn prepare_state(state_dir: Option<&Path>) -> Result<StateVolume, StateError> {
    let paths = MountPaths::standard();

    if let Some(dir) = state_dir {
        fs::create_dir_all(dir)?;
        let vol = StateVolume {
            root: dir.to_path_buf(),
            source: StateSource::Directory,
        };
        vol.ensure_layout()?;
        info!(path = %vol.root.display(), "STATE ready (directory)");
        return Ok(vol);
    }

    #[cfg(target_os = "linux")]
    {
        // Virtio + no udev: wait briefly, then sysfs PARTNAME / node guess.
        if let Some(dev) = find_state_device() {
            match mount_state_partition(&dev, Path::new(paths.state)) {
                Ok(vol) => {
                    info!(
                        path = %vol.root.display(),
                        source = ?vol.source,
                        config = %vol.config_path().display(),
                        config_exists = vol.config_path().exists(),
                        "STATE ready"
                    );
                    return Ok(vol);
                }
                Err(err) => {
                    tracing::warn!(
                        device = %dev.display(),
                        error = %err,
                        "STATE mount failed; trying ephemeral"
                    );
                }
            }
        }
        tracing::warn!("no STATE partition found; using ephemeral STATE on root (apply will not persist across reboot)");
        return prepare_ephemeral_state(Path::new(paths.state));
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = paths;
        // On macOS/dev hosts, default to a workspace-local path would be surprising
        // for library callers — require --state-dir from pertiskd instead.
        Err(StateError::DeviceNotFound)
    }
}

#[cfg(target_os = "linux")]
fn find_state_device() -> Option<PathBuf> {
    if let Some(dev) = wait_for_partlabel(PARTLABEL_STATE, Duration::from_secs(15)) {
        info!(device = %dev.display(), "found STATE partition");
        return Some(dev);
    }
    for dev in guess_state_nodes() {
        info!(device = %dev.display(), "guessing STATE partition node");
        return Some(dev);
    }
    None
}

#[cfg(target_os = "linux")]
fn mount_state_partition(dev: &Path, mountpoint: &Path) -> Result<StateVolume, StateError> {
    use nix::mount::{mount, MsFlags};

    fs::create_dir_all(mountpoint)?;

    // Prefer ext4; soft-fail messages if already mounted. Retry briefly — after
    // module load, blockdev lookup can race (ENOENT / "Can't lookup blockdev").
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
                    "mounted STATE"
                );
                last_err = None;
                break;
            }
            Err(nix::errno::Errno::EBUSY) => {
                info!(target = %mountpoint.display(), "STATE already mounted");
                last_err = None;
                break;
            }
            Err(err) => {
                last_err = Some(err);
                tracing::warn!(
                    device = %dev.display(),
                    target = %mountpoint.display(),
                    device_exists = dev.exists(),
                    mountpoint_exists = mountpoint.exists(),
                    attempt,
                    error = %err,
                    "STATE mount attempt failed"
                );
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    if let Some(err) = last_err {
        return Err(StateError::Mount(err.to_string()));
    }

    let vol = StateVolume {
        root: mountpoint.to_path_buf(),
        source: StateSource::Partition,
    };
    vol.ensure_layout()?;
    Ok(vol)
}

#[cfg(target_os = "linux")]
fn prepare_ephemeral_state(mountpoint: &Path) -> Result<StateVolume, StateError> {
    fs::create_dir_all(mountpoint)?;
    let vol = StateVolume {
        root: mountpoint.to_path_buf(),
        source: StateSource::EphemeralTmpfs,
    };
    vol.ensure_layout()?;
    info!(path = %vol.root.display(), "STATE ready (ephemeral)");
    Ok(vol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn directory_state_layout() {
        let dir = tempdir().unwrap();
        let vol = prepare_state(Some(dir.path())).unwrap();
        assert_eq!(vol.source, StateSource::Directory);
        assert!(vol.root.join("machine").is_dir());
        assert!(vol.root.join("secrets").is_dir());
        assert_eq!(vol.config_path(), dir.path().join("config.yaml"));
    }
}
