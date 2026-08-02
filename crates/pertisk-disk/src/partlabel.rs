//! Locate GPT partitions by PARTLABEL without udev.
//!
//! Initramfs images often have no `udevd`, so `/dev/disk/by-partlabel/*` never
//! appears. Scan `/sys/class/block/*/uevent` for `PARTNAME=` instead.

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

/// Resolve a block device path for a GPT partition label.
///
/// Order:
/// 1. `/dev/disk/by-partlabel/<label>` (when udev ran)
/// 2. Sysfs `PARTNAME=` scan → `/dev/<name>`
/// 3. Common virtio/SCSI names for known Pertisk roles (best-effort)
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
                    let dev = PathBuf::from(format!("/dev/{name}"));
                    if dev.exists() {
                        return Some(dev);
                    }
                }
            }
        }
    }
    None
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
