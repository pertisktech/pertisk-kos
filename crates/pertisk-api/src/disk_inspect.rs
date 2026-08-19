//! STATE / EPHEMERAL volume inspect (lab ops).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskVolumeRow {
    pub label: String,
    pub mountpoint: String,
    pub device: String,
    pub mounted: bool,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskInspectSnapshot {
    pub available: bool,
    pub message: String,
    pub volumes: Vec<DiskVolumeRow>,
}

/// Inspect STATE + EPHEMERAL (mount + capacity).
pub fn inspect_disks() -> DiskInspectSnapshot {
    #[cfg(not(target_os = "linux"))]
    {
        DiskInspectSnapshot {
            available: false,
            message: "disk inspect is Linux-only".into(),
            volumes: Vec::new(),
        }
    }
    #[cfg(target_os = "linux")]
    {
        use pertisk_disk::{find_by_partlabel, MountPaths, PARTLABEL_EPHEMERAL, PARTLABEL_STATE};

        let paths = MountPaths::standard();
        let volumes = vec![
            inspect_volume("STATE", paths.state, PARTLABEL_STATE),
            inspect_volume("EPHEMERAL", paths.var, PARTLABEL_EPHEMERAL),
        ];
        let any = volumes.iter().any(|v| v.mounted || !v.device.is_empty());
        DiskInspectSnapshot {
            available: any,
            message: if any {
                format!(
                    "{} volume(s); {} mounted",
                    volumes.len(),
                    volumes.iter().filter(|v| v.mounted).count()
                )
            } else {
                "no STATE/EPHEMERAL partitions or mounts found".into()
            },
            volumes,
        }
    }
}

#[cfg(target_os = "linux")]
fn inspect_volume(label: &str, mountpoint: &str, partlabel: &str) -> DiskVolumeRow {
    use std::path::Path;

    use pertisk_disk::find_by_partlabel;

    let device = find_by_partlabel(partlabel)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mp = Path::new(mountpoint);
    let mounted = is_mountpoint(mp);
    let (total_bytes, used_bytes) = if mounted {
        statvfs_usage(mp).unwrap_or((0, 0))
    } else {
        (0, 0)
    };
    DiskVolumeRow {
        label: label.into(),
        mountpoint: mountpoint.into(),
        device,
        mounted,
        total_bytes,
        used_bytes,
    }
}

#[cfg(target_os = "linux")]
fn is_mountpoint(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    if !path.is_dir() {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(parent_meta) = path
        .parent()
        .map(std::fs::metadata)
        .transpose()
        .map(|o| o.unwrap_or(meta.clone()))
    else {
        return path.exists();
    };
    meta.dev() != parent_meta.dev() || path == Path::new("/")
}

#[cfg(target_os = "linux")]
fn statvfs_usage(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    let cpath = CString::new(path.to_str()?).ok()?;
    // SAFETY: path is a valid CString; libc::statvfs fills the struct.
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(cpath.as_ptr(), &mut s) != 0 {
            return None;
        }
        let frsize = s.f_frsize as u64;
        let total = s.f_blocks.saturating_mul(frsize);
        let avail = s.f_bavail.saturating_mul(frsize);
        let used = total.saturating_sub(avail);
        Some((total, used))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_shape_on_host() {
        let snap = inspect_disks();
        assert!(snap.volumes.len() <= 2);
        for v in &snap.volumes {
            assert!(!v.label.is_empty());
            assert!(!v.mountpoint.is_empty());
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn device_path_optional() {
        use pertisk_disk::PARTLABEL_STATE;
        let row = inspect_volume("STATE", "/system/state", PARTLABEL_STATE);
        assert_eq!(row.label, "STATE");
        assert_eq!(row.mountpoint, "/system/state");
    }
}
