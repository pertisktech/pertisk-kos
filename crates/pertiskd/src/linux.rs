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

/// Point stdin/stdout/stderr at the serial console (Proxmox xterm.js / ttyS0).
///
/// Kernel printk goes to all `console=` devices, but PID 1 stdio follows
/// `/dev/console` (last `console=`). Explicitly binding to ttyS0 makes logs
/// and the dashboard visible on Serial even if cmdline order is wrong.
pub fn redirect_stdio_serial() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux_impl::redirect_stdio_serial()
    }
    #[cfg(not(target_os = "linux"))]
    {
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

/// Write `/etc/os-release` so kubelet reports OS-IMAGE as `pertisk-kos`.
pub fn ensure_os_release() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux_impl::ensure_os_release()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

/// Remount `/proc` if thread-self/diskstats disappeared (Cilium Bidirectional fallout).
pub fn ensure_proc_readable() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux_impl::ensure_proc_readable()
    }
    #[cfg(not(target_os = "linux"))]
    {
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
        ensure_dir("/system/ephemeral")?;
        ensure_dir("/etc")?;
        ensure_dir("/boot")?;
        ensure_dir("/boot/efi")?;

        // Minimal hosts file — containerd CRI reads /etc/hosts when creating sandboxes.
        ensure_hosts_file()?;

        try_mount(
            "proc",
            "/proc",
            "proc",
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
        )?;
        ensure_proc_readable()?;
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

        // Share only the mounts Cilium/kubelet need. Do NOT make `/` rshared —
        // Bidirectional hostPath umounts under a shared `/` have been observed to
        // tear down host `/proc` (runc abort → missing /proc/thread-self), after
        // which containerd reports "stat /proc/.../ns/pid" and the node goes NotReady.
        make_rshared("/sys")?;
        make_rshared("/run")?;
        prepare_bpffs()?;

        prepare_cgroups()?;
        ensure_os_release()?;
        ensure_proc_readable()?;

        info!("essential filesystems ready");
        Ok(())
    }

    /// Remount procfs if `/proc/thread-self` / `/proc/diskstats` are missing.
    pub fn ensure_proc_readable() -> Result<()> {
        if proc_looks_healthy() {
            return Ok(());
        }
        tracing::warn!("/proc incomplete; remounting procfs");
        // Best-effort detach of a broken/empty mount, then fresh proc.
        let _ = nix::mount::umount("/proc");
        match mount(
            Some("proc"),
            "/proc",
            Some("proc"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
            None::<&str>,
        ) {
            Ok(()) => info!("remounted /proc"),
            Err(nix::errno::Errno::EBUSY) | Err(nix::errno::Errno::EEXIST) => {
                // Already mounted but incomplete — try remount.
                let _ = mount(
                    None::<&str>,
                    "/proc",
                    None::<&str>,
                    MsFlags::MS_REMOUNT | MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
                    None::<&str>,
                );
            }
            Err(err) => tracing::warn!(error = %err, "proc remount failed"),
        }
        if proc_looks_healthy() {
            info!("/proc healthy (thread-self + diskstats present)");
        } else {
            tracing::warn!("/proc still unhealthy after remount");
        }
        Ok(())
    }

    fn proc_looks_healthy() -> bool {
        Path::new("/proc/thread-self/mountinfo").is_file()
            && Path::new("/proc/diskstats").is_file()
            && Path::new("/proc/1").is_dir()
    }

    /// Mount BPF filesystem for Cilium / kube-proxy eBPF.
    fn prepare_bpffs() -> Result<()> {
        ensure_dir("/sys/fs/bpf")?;
        try_mount(
            "bpffs",
            "/sys/fs/bpf",
            "bpf",
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
        )?;
        make_rshared("/sys/fs/bpf")?;
        Ok(())
    }

    fn make_rshared(path: &str) -> Result<()> {
        match mount(
            None::<&str>,
            path,
            None::<&str>,
            MsFlags::MS_REC | MsFlags::MS_SHARED,
            None::<&str>,
        ) {
            Ok(()) => {
                info!(path, "mount propagation set to rshared");
                Ok(())
            }
            Err(err) => {
                tracing::warn!(path, error = %err, "make-rshared failed");
                Ok(())
            }
        }
    }

    pub fn ensure_os_release() -> Result<()> {
        ensure_dir("/etc")?;
        let ver = pertisk_config::release_version();
        let body = format!(
            "PRETTY_NAME=\"pertisk-kos\"\n\
             NAME=\"pertisk-kos\"\n\
             ID=pertisk-kos\n\
             ID_LIKE=pertisk\n\
             VERSION_ID=\"{ver}\"\n\
             VERSION=\"{ver}\"\n\
             HOME_URL=\"https://github.com/pertisk-tech/pertisk-kos\"\n\
             SUPPORT_URL=\"https://github.com/pertisk-tech/pertisk-kos\"\n\
             BUG_REPORT_URL=\"https://github.com/pertisk-tech/pertisk-kos/issues\"\n"
        );
        fs::write("/etc/os-release", body)?;
        // Some tools also read /usr/lib/os-release.
        ensure_dir("/usr/lib")?;
        let _ = fs::copy("/etc/os-release", "/usr/lib/os-release");
        info!(version = ver, "wrote /etc/os-release");
        Ok(())
    }

    /// Mount unified cgroup v2 so containerd/kubelet can manage containers.
    fn prepare_cgroups() -> Result<()> {
        ensure_dir("/sys/fs/cgroup")?;
        // Prefer cgroup v2 (Alpine linux-virt default). Ignore if already mounted.
        match mount(
            Some("none"),
            "/sys/fs/cgroup",
            Some("cgroup2"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
            Some("nsdelegate"),
        ) {
            Ok(()) => info!(target = "/sys/fs/cgroup", "mounted cgroup2"),
            Err(nix::errno::Errno::EBUSY) | Err(nix::errno::Errno::EEXIST) => {
                debug!(target = "/sys/fs/cgroup", "cgroup2 already mounted");
            }
            Err(err) => {
                // Fall back to legacy cgroup v1 cpu/memory (older kernels).
                tracing::warn!(error = %err, "cgroup2 mount failed; trying cgroup v1");
                for (name, opts) in [
                    ("cpu,cpuacct", "cpu,cpuacct"),
                    ("cpuset", "cpuset"),
                    ("memory", "memory"),
                    ("pids", "pids"),
                    ("systemd", "none"),
                ] {
                    let target = format!("/sys/fs/cgroup/{name}");
                    ensure_dir(&target)?;
                    let _ = mount(
                        Some(name),
                        target.as_str(),
                        Some("cgroup"),
                        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
                        Some(opts),
                    );
                }
            }
        }
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

    pub fn redirect_stdio_serial() -> Result<()> {
        use std::os::unix::io::AsRawFd;

        for path in ["/dev/ttyS0", "/dev/console", "/dev/tty0"] {
            let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(path) else {
                continue;
            };
            let fd = file.as_raw_fd();
            // SAFETY: dup2 replaces stdio fds; kernel keeps references after close.
            unsafe {
                if libc::dup2(fd, 0) < 0 || libc::dup2(fd, 1) < 0 || libc::dup2(fd, 2) < 0 {
                    return Err(anyhow::anyhow!("dup2 to {path} failed"));
                }
            }
            drop(file);
            eprintln!("pertiskd: stdio -> {path}");
            return Ok(());
        }
        eprintln!("pertiskd: no ttyS0/console for stdio redirect");
        Ok(())
    }

    fn ensure_dir(path: &str) -> Result<()> {
        if !Path::new(path).exists() {
            fs::create_dir_all(path)?;
            debug!(path, "created directory");
        }
        Ok(())
    }

    fn ensure_hosts_file() -> Result<()> {
        let path = Path::new("/etc/hosts");
        if path.is_file() {
            return Ok(());
        }
        ensure_dir("/etc")?;
        fs::write(
            path,
            "# Generated by pertiskd\n127.0.0.1\tlocalhost\n::1\t\tlocalhost ip6-localhost ip6-loopback\n",
        )?;
        info!(path = %path.display(), "wrote /etc/hosts");
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
