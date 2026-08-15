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

/// kubelet `status.nodeInfo.osImage` (`kubectl get nodes -o wide` OS-IMAGE).
pub fn os_image_pretty_name(version: &str) -> String {
    let v = version.trim();
    if v.is_empty() {
        "pertisk-kos".into()
    } else {
        format!("pertisk-kos {v}")
    }
}

/// `/etc/os-release` body. `PRETTY_NAME` is what kubelet stamps on the node.
pub fn os_release_contents(version: &str) -> String {
    let ver = version.trim();
    let pretty = os_image_pretty_name(ver);
    format!(
        "PRETTY_NAME=\"{pretty}\"\n\
         NAME=\"pertisk-kos\"\n\
         ID=pertisk-kos\n\
         ID_LIKE=pertisk\n\
         VERSION_ID=\"{ver}\"\n\
         VERSION=\"{ver}\"\n\
         BUILD_ID=\"{ver}\"\n\
         HOME_URL=\"https://github.com/pertisk-tech/pertisk-kos\"\n\
         SUPPORT_URL=\"https://github.com/pertisk-tech/pertisk-kos\"\n\
         BUG_REPORT_URL=\"https://github.com/pertisk-tech/pertisk-kos/issues\"\n"
    )
}

/// Write `/etc/os-release` so kubelet reports OS-IMAGE as `pertisk-kos <version>`.
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

/// Mark a mount rshared (Cilium hostPath Bidirectional needs this on `/var`).
pub fn make_rshared(path: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux_impl::make_rshared(path)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        Ok(())
    }
}

/// Ensure `/var/run/netns` lives on a shared mount for Cilium Bidirectional hostPath.
///
/// EPHEMERAL bind-mounts `/var` as private by default; even after `make-rshared`,
/// some containerd versions still reject `/var/run/netns`. Binding `/run` (already
/// rshared) over `/var/run` makes the netns path share `/run`'s propagation —
/// same shape as Debian/FHS (`/var/run` → `/run`).
pub fn ensure_var_run_shared() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux_impl::ensure_var_run_shared()
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

    pub fn make_rshared(path: &str) -> Result<()> {
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

    pub fn ensure_var_run_shared() -> Result<()> {
        ensure_dir("/run")?;
        ensure_dir("/run/netns")?;
        ensure_dir("/var")?;
        ensure_dir("/var/run")?;

        // Self-bind then rshared — some kernels ignore make-rshared on a
        // non-root mount without an explicit bind first.
        let _ = mount(
            Some("/var"),
            "/var",
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REC,
            None::<&str>,
        );
        make_rshared("/var")?;

        // Cover /var/run with /run (already rshared from prepare()). Cilium's
        // default hostPath is /var/run/netns; containerd requires that path's
        // covering mount to be shared/slave.
        let var_run_is_mount = fs::read_to_string("/proc/self/mountinfo")
            .map(|s| {
                s.lines().any(|l| {
                    l.split_whitespace()
                        .nth(4)
                        .is_some_and(|tgt| tgt == "/var/run")
                })
            })
            .unwrap_or(false);
        if !var_run_is_mount {
            match mount(
                Some("/run"),
                "/var/run",
                None::<&str>,
                MsFlags::MS_BIND | MsFlags::MS_REC,
                None::<&str>,
            ) {
                Ok(()) => info!("bound /run → /var/run for shared netns"),
                Err(nix::errno::Errno::EBUSY) => {
                    info!("/var/run already a mountpoint");
                }
                Err(err) => {
                    tracing::warn!(error = %err, "bind /run → /var/run failed");
                }
            }
        }
        make_rshared("/var/run")?;
        ensure_dir("/var/run/netns")?;

        if let Some(line) = covering_mountinfo_line("/var/run/netns") {
            let shared = line.contains("shared:") || line.contains("master:");
            if shared {
                info!(%line, "/var/run/netns covering mount is shared/slave");
            } else {
                tracing::warn!(
                    %line,
                    "/var/run/netns covering mount is NOT shared/slave — Cilium Bidirectional hostPath will fail"
                );
            }
        }
        Ok(())
    }

    fn covering_mountinfo_line(path: &str) -> Option<String> {
        let info = fs::read_to_string("/proc/self/mountinfo").ok()?;
        let mut best: Option<(usize, String)> = None;
        for line in info.lines() {
            let tgt = line.split_whitespace().nth(4)?;
            if path == tgt || path.starts_with(&format!("{tgt}/")) {
                let rank = tgt.len();
                if best.as_ref().map(|(r, _)| rank >= *r).unwrap_or(true) {
                    best = Some((rank, line.to_string()));
                }
            }
        }
        best.map(|(_, l)| l)
    }

    pub fn ensure_os_release() -> Result<()> {
        ensure_dir("/etc")?;
        let ver = pertisk_config::release_version();
        fs::write("/etc/os-release", super::os_release_contents(ver))?;
        // Some tools also read /usr/lib/os-release.
        ensure_dir("/usr/lib")?;
        let _ = fs::copy("/etc/os-release", "/usr/lib/os-release");
        info!(version = ver, pretty = %super::os_image_pretty_name(ver), "wrote /etc/os-release");
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
        // Prefer explicit dashboard console (`pertisk.dashboard.console`).
        if let Some(path) = crate::cmdline::dashboard_console_path() {
            if try_redirect_stdio(&path)? {
                return Ok(());
            }
            eprintln!(
                "pertiskd: dashboard console {} unavailable; falling back",
                path.display()
            );
        }

        // Order matters: on aarch64 `virt`, the real console is PL011 (ttyAMA0).
        // The 8250 driver still creates /dev/ttyS0, so preferring ttyS0 sends all
        // eprintln!/panic output to a dead UART while Proxmox Serial stays silent.
        #[cfg(target_arch = "aarch64")]
        const CANDIDATES: &[&str] = &[
            "/dev/ttyAMA0",
            "/dev/console",
            "/dev/ttyS0",
            "/dev/tty0",
        ];
        #[cfg(not(target_arch = "aarch64"))]
        const CANDIDATES: &[&str] = &["/dev/ttyS0", "/dev/console", "/dev/tty0"];

        for path in CANDIDATES {
            if try_redirect_stdio(Path::new(path))? {
                return Ok(());
            }
        }
        eprintln!("pertiskd: no serial/console tty for stdio redirect");
        Ok(())
    }

    fn try_redirect_stdio(path: &Path) -> Result<bool> {
        use std::os::unix::io::AsRawFd;

        let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(path) else {
            return Ok(false);
        };
        let fd = file.as_raw_fd();
        // SAFETY: dup2 replaces stdio fds; kernel keeps references after close.
        unsafe {
            if libc::dup2(fd, 0) < 0 || libc::dup2(fd, 1) < 0 || libc::dup2(fd, 2) < 0 {
                return Err(anyhow::anyhow!("dup2 to {} failed", path.display()));
            }
        }
        drop(file);
        eprintln!("pertiskd: stdio -> {}", path.display());
        Ok(true)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_name_stamps_version() {
        assert_eq!(os_image_pretty_name("0.2.87"), "pertisk-kos 0.2.87");
        assert_eq!(os_image_pretty_name(" v1.0 "), "pertisk-kos v1.0");
        assert_eq!(os_image_pretty_name(""), "pertisk-kos");
    }

    #[test]
    fn os_release_pretty_name_is_kubelet_os_image() {
        let body = os_release_contents("0.2.87");
        assert!(body.contains("PRETTY_NAME=\"pertisk-kos 0.2.87\""));
        assert!(body.contains("VERSION_ID=\"0.2.87\""));
        assert!(body.contains("BUILD_ID=\"0.2.87\""));
    }
}
