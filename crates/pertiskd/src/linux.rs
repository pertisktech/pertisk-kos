//! Early filesystem setup for when `pertiskd` is PID 1.

use anyhow::Result;
use tracing::info;

/// Mount essential virtual filesystems and ensure basic dirs exist.
///
/// No-ops on non-Linux hosts so `cargo run` works during development.
pub fn prepare_filesystem() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux_impl::prepare()
    }
    #[cfg(not(target_os = "linux"))]
    {
        info!("skipping mounts (not Linux)");
        Ok(())
    }
}

/// Ensure writable `/var` exists (tmpfs in initramfs; EPHEMERAL bind later).
pub fn prepare_var() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux_impl::prepare_var()
    }
    #[cfg(not(target_os = "linux"))]
    {
        info!("skipping /var setup (not Linux)");
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::*;
    use std::fs;
    use std::path::Path;

    use nix::mount::{mount, MsFlags};
    use tracing::debug;

    pub fn prepare() -> Result<()> {
        ensure_dir("/proc")?;
        ensure_dir("/sys")?;
        ensure_dir("/dev")?;
        ensure_dir("/run")?;
        ensure_dir("/var")?;
        ensure_dir("/tmp")?;
        ensure_dir("/system")?;
        ensure_dir("/system/state")?;
        ensure_dir("/etc")?;

        try_mount(
            "proc",
            "/proc",
            "proc",
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
        )?;
        try_mount(
            "sysfs",
            "/sys",
            "sysfs",
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
        )?;
        try_mount("devtmpfs", "/dev", "devtmpfs", MsFlags::MS_NOSUID)?;
        try_mount(
            "tmpfs",
            "/run",
            "tmpfs",
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        )?;
        try_mount(
            "tmpfs",
            "/tmp",
            "tmpfs",
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        )?;

        info!("essential filesystems ready");
        Ok(())
    }

    pub fn prepare_var() -> Result<()> {
        ensure_dir("/var")?;
        ensure_dir("/var/log")?;
        ensure_dir("/var/lib")?;
        try_mount(
            "tmpfs",
            "/var",
            "tmpfs",
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        )?;
        ensure_dir("/var/log")?;
        ensure_dir("/var/lib")?;
        info!("/var ready");
        Ok(())
    }

    fn ensure_dir(path: &str) -> Result<()> {
        if !Path::new(path).exists() {
            fs::create_dir_all(path)?;
            debug!(path, "created directory");
        }
        Ok(())
    }

    fn try_mount(source: &str, target: &str, fstype: &str, flags: MsFlags) -> Result<()> {
        match mount(Some(source), target, Some(fstype), flags, None::<&str>) {
            Ok(()) => {
                info!(source, target, fstype, "mounted");
                Ok(())
            }
            Err(nix::errno::Errno::EBUSY) | Err(nix::errno::Errno::EEXIST) => {
                debug!(target, "already mounted");
                Ok(())
            }
            Err(err) => {
                tracing::warn!(target, error = %err, "mount skipped");
                Ok(())
            }
        }
    }
}
