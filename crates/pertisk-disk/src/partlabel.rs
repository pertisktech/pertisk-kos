//! Locate GPT partitions by PARTLABEL without udev.
//!
//! Initramfs images often have no `udevd`, so `/dev/disk/by-partlabel/*` never
//! appears. Scan `/sys/class/block/*/uevent` for `PARTNAME=` instead.
//!
//! Also ensure `/dev/<name>` block nodes exist (devtmpfs race / missing nodes)
//! by reading `sysfs .../dev` (major:minor) and `mknod` when needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use tracing::{info, warn};

/// Resolve a block device path for a GPT partition label.
///
/// Order:
/// 1. `/dev/disk/by-partlabel/<label>` (when udev ran)
/// 2. Sysfs `PARTNAME=` scan → ensure `/dev/<name>` → return it
pub fn find_by_partlabel(label: &str) -> Option<PathBuf> {
    let by_udev = PathBuf::from(format!("/dev/disk/by-partlabel/{label}"));
    if by_udev.exists() {
        return Some(by_udev);
    }
    if let Some(dev) = find_via_sysfs(label) {
        return Some(dev);
    }
    None
}

/// Poll for a partlabel for up to `timeout`, sleeping between attempts.
pub fn wait_for_partlabel(label: &str, timeout: Duration) -> Option<PathBuf> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(dev) = find_by_partlabel(label) {
            return Some(dev);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// After loading disk modules, ask the kernel to (re)scan partitions and wait
/// briefly so `/dev/sd*` nodes and PARTNAME uevents settle.
pub fn settle_block_devices() {
    for disk in ["sda", "vda", "nvme0n1", "xvda"] {
        let path = format!("/dev/{disk}");
        if !Path::new(&path).exists() {
            continue;
        }
        // Best-effort; BusyBox/sgdisk images ship `partprobe`.
        let status = Command::new("partprobe").arg(&path).status();
        match status {
            Ok(s) if s.success() => info!(disk = %path, "partprobe ok"),
            Ok(s) => warn!(disk = %path, code = ?s.code(), "partprobe exited non-zero"),
            Err(err) => warn!(disk = %path, error = %err, "partprobe failed to spawn"),
        }
    }
    thread::sleep(Duration::from_millis(500));
}

fn find_via_sysfs(label: &str) -> Option<PathBuf> {
    let Ok(entries) = fs::read_dir("/sys/class/block") else {
        return None;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Skip whole disks (no partition file).
        if !entry.path().join("partition").exists() {
            continue;
        }
        let uevent = entry.path().join("uevent");
        let Ok(text) = fs::read_to_string(&uevent) else {
            continue;
        };
        for line in text.lines() {
            if let Some(partname) = line.strip_prefix("PARTNAME=") {
                if partname == label {
                    return ensure_block_node(&name);
                }
            }
        }
    }
    None
}

/// Ensure `/dev/<sysfs_name>` is a block device with the maj:min from sysfs.
fn ensure_block_node(sysfs_name: &str) -> Option<PathBuf> {
    let sys_dev = PathBuf::from(format!("/sys/class/block/{sysfs_name}/dev"));
    let Ok(text) = fs::read_to_string(&sys_dev) else {
        return None;
    };
    let text = text.trim();
    let mut parts = text.split(':');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next()?.parse().ok()?;

    let dev = PathBuf::from(format!("/dev/{sysfs_name}"));
    if block_node_matches(&dev, major, minor) {
        return Some(dev);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if dev.exists() {
            // Wrong type or maj/min — replace.
            let _ = fs::remove_file(&dev);
        }
        // SAFETY: mknod with computed maj/min from sysfs for a well-known /dev path.
        let mode = 0o0600 | libc::S_IFBLK;
        let rdev = libc::makedev(major as _, minor as _);
        let c_path = std::ffi::CString::new(dev.to_str()?).ok()?;
        let rc = unsafe { libc::mknod(c_path.as_ptr(), mode, rdev) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            // EEXIST race with concurrent devtmpfs creation is fine if node matches.
            if err.kind() != std::io::ErrorKind::AlreadyExists {
                warn!(
                    device = %dev.display(),
                    major,
                    minor,
                    error = %err,
                    "mknod block device failed"
                );
                return None;
            }
        }
        if block_node_matches(&dev, major, minor) {
            info!(device = %dev.display(), major, minor, "ensured block device node");
            return Some(dev);
        }
        // Fallback: accept whatever exists if FileType is block.
        if let Ok(meta) = fs::metadata(&dev) {
            if meta.file_type().is_block_device() {
                return Some(dev);
            }
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = (major, minor);
        None
    }
}

#[cfg(unix)]
fn block_node_matches(path: &Path, major: u64, minor: u64) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.file_type().is_block_device() {
        return false;
    }
    let rdev = meta.rdev();
    let maj = libc::major(rdev as libc::dev_t) as u64;
    let min = libc::minor(rdev as libc::dev_t) as u64;
    maj == major && min == minor
}

#[cfg(not(unix))]
fn block_node_matches(_path: &Path, _major: u64, _minor: u64) -> bool {
    false
}

/// Fallback nodes when PARTNAME is unavailable (legacy / broken sysfs).
pub fn guess_state_nodes() -> impl Iterator<Item = PathBuf> {
    [
        "/dev/vda5",
        "/dev/sda5",
        "/dev/nvme0n1p5",
        "/dev/xvda5",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guess_filters_missing() {
        // On macOS/dev hosts these usually do not exist.
        let _ = guess_state_nodes().count();
    }
}
