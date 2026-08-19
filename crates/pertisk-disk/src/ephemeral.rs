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

/// Result of expanding EPHEMERAL after a hypervisor disk resize.
#[derive(Debug, Clone, Default)]
pub struct GrowEphemeralResult {
    /// GPT partition end was moved.
    pub partition_grew: bool,
    /// ext4 was resized (online or offline).
    pub filesystem_grew: bool,
}

/// Expand EPHEMERAL to fill a resized disk (safe while `/var` is mounted).
///
/// Call after Proxmox/`qm resize` (or reboot so the kernel sees the new size).
/// Rescans the parent disk, grows GPT partition 6 to the end, then `resize2fs`.
pub fn grow_ephemeral_storage() -> Result<GrowEphemeralResult, EphemeralError> {
    #[cfg(target_os = "linux")]
    {
        let Some(dev) = find_ephemeral_device() else {
            return Err(EphemeralError::DeviceNotFound);
        };
        Ok(prepare_ephemeral_device(&dev))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(GrowEphemeralResult::default())
    }
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
        // After qemu-img / Proxmox resize, grow GPT then ensure ext4 fills the
        // partition (resize2fs; mkfs only when unformatted or inode-starved).
        let _ = prepare_ephemeral_device(&dev);
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

/// Grow GPT if the disk was resized, then mkfs/resize so `/var` has usable inodes.
#[cfg(target_os = "linux")]
fn prepare_ephemeral_device(part_dev: &Path) -> GrowEphemeralResult {
    rescan_parent_disk(part_dev);
    let partition_grew = grow_ephemeral_partition(part_dev);
    // After sgdisk recreate, re-resolve the partition node (size may have changed).
    let dev = if partition_grew {
        std::thread::sleep(Duration::from_millis(500));
        find_ephemeral_device().unwrap_or_else(|| part_dev.to_path_buf())
    } else {
        part_dev.to_path_buf()
    };
    let filesystem_grew = ensure_ephemeral_filesystem(&dev, partition_grew);
    GrowEphemeralResult {
        partition_grew,
        filesystem_grew,
    }
}

/// Ask the kernel to refresh whole-disk capacity after hypervisor resize.
#[cfg(target_os = "linux")]
fn rescan_parent_disk(part_dev: &Path) {
    use std::process::Command;

    let Some((disk, _, _)) = partition_sysfs(part_dev) else {
        return;
    };
    let disk_path = format!("/dev/{disk}");
    // SCSI/virtio-scsi: rescan capacity.
    let rescan = PathBuf::from(format!("/sys/class/block/{disk}/device/rescan"));
    if rescan.exists() {
        if let Err(err) = fs::write(&rescan, "1") {
            warn!(disk = %disk, error = %err, "block rescan write failed");
        } else {
            info!(disk = %disk, "rescanned block device capacity");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let _ = Command::new(disk_tool("partprobe"))
        .arg(&disk_path)
        .status();
}

/// Expand EPHEMERAL GPT partition to the end of a resized disk. Returns true when grown.
#[cfg(target_os = "linux")]
fn grow_ephemeral_partition(part_dev: &Path) -> bool {
    use std::process::Command;

    let Some((disk, part_num, sys_part)) = partition_sysfs(part_dev) else {
        warn!(device = %part_dev.display(), "cannot resolve EPHEMERAL sysfs for grow");
        return false;
    };
    let disk_sys = PathBuf::from(format!("/sys/class/block/{disk}"));
    let part_sys = PathBuf::from(format!("/sys/class/block/{sys_part}"));
    let Ok(disk_sectors) = read_u64(&disk_sys.join("size")) else {
        return false;
    };
    let Ok(part_start) = read_u64(&part_sys.join("start")) else {
        return false;
    };
    let Ok(part_size) = read_u64(&part_sys.join("size")) else {
        return false;
    };
    // Leave room for GPT backup header (~34 sectors) + alignment slack.
    let usable_end = disk_sectors.saturating_sub(2048);
    let part_end = part_start.saturating_add(part_size);
    if part_end + 2048 >= usable_end {
        info!(
            disk = %disk,
            part_end,
            disk_sectors,
            "EPHEMERAL already fills disk (or kernel still sees old capacity)"
        );
        return false;
    }

    let disk_path = PathBuf::from(format!("/dev/{disk}"));
    info!(
        disk = %disk_path.display(),
        partition = part_num,
        part_end,
        disk_sectors,
        usable_end,
        "growing EPHEMERAL partition to disk end"
    );

    let sgdisk = disk_tool("sgdisk");
    let partprobe = disk_tool("partprobe");

    // Move backup GPT to new end, then recreate partition from same start → end.
    if !run_ok(&sgdisk, &["-e", &disk_path.to_string_lossy()]) {
        warn!("sgdisk -e failed (backup GPT relocate)");
        return false;
    }
    if !run_ok(
        &sgdisk,
        &["-d", &part_num.to_string(), &disk_path.to_string_lossy()],
    ) {
        warn!(partition = part_num, "sgdisk -d failed");
        return false;
    }
    let n_arg = format!("{part_num}:{part_start}:0");
    let t_arg = format!("{part_num}:8300");
    let c_arg = format!("{part_num}:{PARTLABEL_EPHEMERAL}");
    if !run_ok(
        &sgdisk,
        &[
            "-n",
            &n_arg,
            "-t",
            &t_arg,
            "-c",
            &c_arg,
            &disk_path.to_string_lossy(),
        ],
    ) {
        warn!("sgdisk recreate EPHEMERAL failed");
        return false;
    }
    let _ = run_ok(&partprobe, &[&disk_path.to_string_lossy()]);
    // Tell the kernel the partition size changed (mounted EPHEMERAL grow).
    let part_size_sys = PathBuf::from(format!("/sys/class/block/{sys_part}/size"));
    if part_size_sys.exists() {
        // Reading size after partprobe is enough; force a partition table reread.
        let _ = Command::new(&partprobe).arg(&disk_path).status();
    }
    std::thread::sleep(Duration::from_millis(800));
    true
}

#[cfg(target_os = "linux")]
fn disk_tool(name: &str) -> String {
    for p in [
        format!("/usr/sbin/{name}"),
        format!("/sbin/{name}"),
        format!("/usr/bin/{name}"),
        name.to_string(),
    ] {
        if p == name || Path::new(&p).is_file() {
            return p;
        }
    }
    name.to_string()
}

#[cfg(target_os = "linux")]
fn run_ok(bin: &str, args: &[&str]) -> bool {
    use std::process::Command;
    match Command::new(bin).args(args).status() {
        Ok(st) if st.success() => true,
        Ok(st) => {
            warn!(bin, ?args, code = ?st.code(), "disk tool exited non-zero");
            false
        }
        Err(err) => {
            warn!(bin, error = %err, "disk tool spawn failed");
            false
        }
    }
}

/// Create or repair EPHEMERAL ext4. Prefer mkfs at final size over resize2fs of a
/// tiny `largefile4` FS (inode count does not grow → ENOSPC on image extracts).
/// Returns true when the filesystem size changed.
#[cfg(target_os = "linux")]
fn ensure_ephemeral_filesystem(part_dev: &Path, grew: bool) -> bool {
    use std::process::Command;

    let has_ext4 = blkid_type(part_dev).as_deref() == Some("ext4");
    let starved = has_ext4 && inode_starved_for_partition(part_dev);
    if !has_ext4 || starved {
        info!(
            device = %part_dev.display(),
            has_ext4,
            starved,
            grew,
            "formatting EPHEMERAL ext4 (final size / inode density)"
        );
        match Command::new(disk_tool("mkfs.ext4"))
            .args([
                "-F",
                "-q",
                "-L",
                PARTLABEL_EPHEMERAL,
                // lazy_* : return in seconds on large disks; kernel inits in background.
                // nodiscard: skip TRIM during format (Proxmox/ZFS virtio is slow otherwise).
                "-E",
                "lazy_itable_init=1,lazy_journal_init=1,nodiscard",
                &part_dev.to_string_lossy(),
            ])
            .status()
        {
            Ok(st) if st.success() => {
                info!(device = %part_dev.display(), "EPHEMERAL filesystem created");
                return true;
            }
            Ok(st) => warn!(code = ?st.code(), "mkfs.ext4 exited non-zero"),
            Err(err) => warn!(error = %err, "mkfs.ext4 not available"),
        }
        return false;
    }

    let needs = grew || filesystem_smaller_than_partition(part_dev);
    if !needs {
        return false;
    }
    match Command::new(disk_tool("resize2fs")).arg(part_dev).status() {
        Ok(st) if st.success() => {
            info!(device = %part_dev.display(), "EPHEMERAL filesystem resized");
            true
        }
        Ok(st) => {
            warn!(code = ?st.code(), "resize2fs exited non-zero");
            false
        }
        Err(err) => {
            warn!(error = %err, "resize2fs not available");
            false
        }
    }
}

/// True when ext4 is smaller than the partition (e.g. GPT grew but FS did not).
#[cfg(target_os = "linux")]
fn filesystem_smaller_than_partition(part_dev: &Path) -> bool {
    use std::process::Command;

    let Some((_, _, sys_part)) = partition_sysfs(part_dev) else {
        return false;
    };
    let Ok(part_sectors) = read_u64(Path::new(&format!("/sys/class/block/{sys_part}/size"))) else {
        return false;
    };
    let part_bytes = part_sectors.saturating_mul(512);

    let out = Command::new(disk_tool("tune2fs"))
        .args(["-l"])
        .arg(part_dev)
        .output();
    let Ok(out) = out else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut block_count: Option<u64> = None;
    let mut block_size: Option<u64> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Block count:") {
            block_count = rest.trim().parse().ok();
        }
        if let Some(rest) = line.strip_prefix("Block size:") {
            block_size = rest.trim().parse().ok();
        }
    }
    let (Some(bc), Some(bs)) = (block_count, block_size) else {
        return false;
    };
    let fs_bytes = bc.saturating_mul(bs);
    // Allow 16 MiB slack for alignment / reserved GPT.
    let needs = fs_bytes + 16 * 1024 * 1024 < part_bytes;
    if needs {
        info!(
            device = %part_dev.display(),
            fs_bytes,
            part_bytes,
            "EPHEMERAL filesystem smaller than partition — will resize2fs"
        );
    }
    needs
}

#[cfg(target_os = "linux")]
fn blkid_type(part_dev: &Path) -> Option<String> {
    use std::process::Command;

    let out = Command::new(disk_tool("blkid"))
        .args(["-o", "value", "-s", "TYPE"])
        .arg(part_dev)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// True when inode density is too low for the partition (e.g. largefile4 + grow).
#[cfg(target_os = "linux")]
fn inode_starved_for_partition(part_dev: &Path) -> bool {
    use std::process::Command;

    let Some((_, _, sys_part)) = partition_sysfs(part_dev) else {
        return false;
    };
    let Ok(part_sectors) = read_u64(Path::new(&format!("/sys/class/block/{sys_part}/size"))) else {
        return false;
    };
    let part_bytes = part_sectors.saturating_mul(512);

    let out = Command::new(disk_tool("tune2fs"))
        .args(["-l"])
        .arg(part_dev)
        .output();
    let Ok(out) = out else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut inodes: Option<u64> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Inode count:") {
            inodes = rest.trim().parse().ok();
            break;
        }
    }
    let Some(inodes) = inodes.filter(|&n| n > 0) else {
        return false;
    };
    // Default ext4 ≈ 16KiB/inode. largefile4 after grow is often MiB+/inode.
    let bytes_per_inode = part_bytes / inodes;
    let starved = bytes_per_inode > 128 * 1024;
    if starved {
        warn!(
            device = %part_dev.display(),
            inodes,
            part_bytes,
            bytes_per_inode,
            "EPHEMERAL inode density too low for partition size"
        );
    }
    starved
}

#[cfg(target_os = "linux")]
fn read_u64(path: &Path) -> Result<u64, ()> {
    let s = fs::read_to_string(path).map_err(|_| ())?;
    s.trim().parse().map_err(|_| ())
}

/// Map `/dev/vda6` → (`vda`, 6, `vda6`); `/dev/nvme0n1p6` → (`nvme0n1`, 6, `nvme0n1p6`).
#[cfg(target_os = "linux")]
fn partition_sysfs(part_dev: &Path) -> Option<(String, u32, String)> {
    let name = part_dev.file_name()?.to_str()?.to_string();
    let sys = PathBuf::from(format!("/sys/class/block/{name}"));
    if !sys.join("partition").exists() {
        return None;
    }
    let part_num: u32 = fs::read_to_string(sys.join("partition"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let disk = fs::canonicalize(sys.join(".."))
        .ok()?
        .file_name()?
        .to_str()?
        .to_string();
    Some((disk, part_num, name))
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
