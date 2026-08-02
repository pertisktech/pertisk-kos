//! GPT install onto a block device via `sgdisk` + `mkfs.*` (Linux).

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::layout::PARTLABEL_STATE;
use crate::plan::{plan_disk, DiskPlan, PlanError};
use crate::state::StateVolume;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("install not supported on this platform")]
    UnsupportedPlatform,
    #[error("disk already has Pertisk layout (STATE present); set wipe: true to reinstall")]
    AlreadyInstalled,
    #[error("command failed: {cmd}: {detail}")]
    Command { cmd: String, detail: String },
    #[error("{0}")]
    Msg(String),
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub disk: PathBuf,
    pub wipe: bool,
    /// Optional machine config YAML to seed onto STATE after format.
    pub seed_config: Option<PathBuf>,
}

/// True when a STATE partition is visible (udev symlink or sysfs PARTNAME).
pub fn layout_present() -> bool {
    crate::partlabel::find_by_partlabel(PARTLABEL_STATE).is_some()
        || crate::partlabel::guess_state_nodes().next().is_some()
}

/// Read block device (or image file) size in bytes.
pub fn disk_size(disk: &Path) -> Result<u64, InstallError> {
    let meta = fs::metadata(disk)?;
    let len = meta.len();
    if len > 0 {
        return Ok(len);
    }
    #[cfg(target_os = "linux")]
    {
        use std::fs::OpenOptions;
        use std::io::{Seek, SeekFrom};
        let mut f = OpenOptions::new().read(true).open(disk)?;
        let end = f.seek(SeekFrom::End(0))?;
        return Ok(end);
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(InstallError::Msg("unable to determine disk size".into()))
    }
}

/// Plan what would be installed (safe on any OS that can size the path).
pub fn plan_install(disk: &Path) -> Result<DiskPlan, InstallError> {
    let size = disk_size(disk)?;
    Ok(plan_disk(size)?)
}

/// Install GPT layout, format filesystems, optionally seed STATE config.
pub fn install_disk(opts: &InstallOptions) -> Result<StateVolume, InstallError> {
    #[cfg(target_os = "linux")]
    {
        linux::install(opts)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = opts;
        Err(InstallError::UnsupportedPlatform)
    }
}

/// Map whole-disk path + partition number → Linux partition node.
pub fn partition_node(disk: &Path, part_num: u32) -> PathBuf {
    let s = disk.to_string_lossy();
    if s.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        PathBuf::from(format!("{s}p{part_num}"))
    } else {
        PathBuf::from(format!("{s}{part_num}"))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    use tracing::{info, warn};

    use crate::layout::PartitionRole;
    use crate::plan::FsType;
    use crate::state::{prepare_state, DEFAULT_CONFIG_NAME};

    fn run_cmd(bin: &str, args: &[&str]) -> Result<(), InstallError> {
        let out = Command::new(bin).args(args).output();
        match out {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => {
                let detail = String::from_utf8_lossy(&o.stderr).trim().to_string();
                let detail = if detail.is_empty() {
                    String::from_utf8_lossy(&o.stdout).trim().to_string()
                } else {
                    detail
                };
                Err(InstallError::Command {
                    cmd: format!("{bin} {}", args.join(" ")),
                    detail,
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                warn!(bin, "required tool not found");
                Err(InstallError::Command {
                    cmd: bin.into(),
                    detail: format!("not found in PATH ({err})"),
                })
            }
            Err(err) => Err(err.into()),
        }
    }

    pub fn install(opts: &InstallOptions) -> Result<StateVolume, InstallError> {
        if layout_present() && !opts.wipe {
            return Err(InstallError::AlreadyInstalled);
        }

        let size = disk_size(&opts.disk)?;
        let plan = plan_disk(size)?;
        info!(
            disk = %opts.disk.display(),
            size,
            partitions = plan.partitions.len(),
            "installing Pertisk GPT layout"
        );

        write_gpt_sgdisk(&opts.disk, &plan, opts.wipe)?;
        let _ = Command::new("partprobe").arg(&opts.disk).status();
        thread::sleep(Duration::from_millis(750));

        for (idx, part) in plan.partitions.iter().enumerate() {
            let part_num = (idx + 1) as u32;
            let path = partition_node(&opts.disk, part_num);
            wait_for_path(&path, Duration::from_secs(15))?;
            format_partition(&path, part.fstype, part.role)?;
        }

        let state = prepare_state(None).map_err(|e| InstallError::Msg(e.to_string()))?;
        if let Some(seed) = &opts.seed_config {
            let dest = state.config_path();
            fs::copy(seed, &dest)?;
            info!(from = %seed.display(), to = %dest.display(), "seeded machine config");
        } else {
            info!(
                path = %state.root.join(DEFAULT_CONFIG_NAME).display(),
                "STATE ready (no seed config)"
            );
        }

        Ok(state)
    }

    fn write_gpt_sgdisk(disk: &Path, plan: &DiskPlan, wipe: bool) -> Result<(), InstallError> {
        if wipe {
            run_cmd("sgdisk", &["--zap-all", &disk.to_string_lossy()])?;
        }
        run_cmd("sgdisk", &["-o", &disk.to_string_lossy()])?;

        for (idx, part) in plan.partitions.iter().enumerate() {
            let num = (idx + 1).to_string();
            let size = part.size.ok_or_else(|| {
                InstallError::Msg(format!("partition {:?} missing size", part.role))
            })?;
            let mib = size.div_ceil(1024 * 1024);
            let size_arg = format!("+{mib}M");
            let typecode = match part.role {
                PartitionRole::Efi => "EF00",
                _ => "8300",
            };
            run_cmd(
                "sgdisk",
                &[
                    "-n",
                    &format!("{num}:0:{size_arg}"),
                    "-t",
                    &format!("{num}:{typecode}"),
                    "-c",
                    &format!("{num}:{}", part.role.partlabel()),
                    &disk.to_string_lossy(),
                ],
            )?;
        }

        info!(disk = %disk.display(), "GPT written via sgdisk");
        Ok(())
    }

    fn wait_for_path(path: &Path, timeout: Duration) -> Result<(), InstallError> {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if path.exists() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(InstallError::Msg(format!(
            "partition node {} did not appear",
            path.display()
        )))
    }

    fn format_partition(
        path: &Path,
        fstype: FsType,
        role: PartitionRole,
    ) -> Result<(), InstallError> {
        match fstype {
            FsType::None => {
                info!(path = %path.display(), role = ?role, "skip format");
                Ok(())
            }
            FsType::Vfat => run_cmd(
                "mkfs.vfat",
                &["-F", "32", "-n", role.partlabel(), &path.to_string_lossy()],
            ),
            FsType::Ext4 => run_cmd(
                "mkfs.ext4",
                &["-F", "-L", role.partlabel(), &path.to_string_lossy()],
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_node_naming() {
        assert_eq!(
            partition_node(Path::new("/dev/vda"), 5),
            PathBuf::from("/dev/vda5")
        );
        assert_eq!(
            partition_node(Path::new("/dev/nvme0n1"), 5),
            PathBuf::from("/dev/nvme0n1p5")
        );
    }
}
