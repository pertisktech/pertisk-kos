//! QEMU Guest Agent (`qemu-ga`) for Proxmox/QEMU: Shutdown, Summary IP, guest-ping.
//!
//! Proxmox enables the host-side channel (`agent=enabled=1`); without this process
//! in the guest, `qm shutdown` times out on `guest-ping`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use tracing::{info, warn};

const BIN: &str = "/usr/bin/qemu-ga";
const PORT_NAME: &str = "org.qemu.guest_agent.0";
const DEFAULT_PATH: &str = "/dev/virtio-ports/org.qemu.guest_agent.0";
const STATE_DIR: &str = "/var/run";
const LOG_PATH: &str = "/var/log/qemu-ga.log";

/// Supervised `qemu-ga` child (foreground — not `--daemonize`).
pub struct GuestAgentHandle {
    child: Child,
}

impl GuestAgentHandle {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                warn!(status = ?status, "qemu-ga exited");
                false
            }
            Err(err) => {
                warn!(error = %err, "qemu-ga wait failed");
                false
            }
        }
    }

    pub fn ensure_alive(&mut self) {
        if self.is_alive() {
            return;
        }
        warn!("restarting qemu-ga");
        match spawn() {
            Ok(h) => *self = h,
            Err(err) => warn!(error = %err, "qemu-ga restart failed"),
        }
    }

    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start when `/usr/bin/qemu-ga` is present. Soft-fails if the virtio channel is
/// missing (bare metal / clouds without a guest-agent port).
pub fn start() -> Option<GuestAgentHandle> {
    if !Path::new(BIN).is_file() {
        info!("qemu-ga binary missing; skip guest agent");
        return None;
    }
    match spawn() {
        Ok(h) => {
            info!(pid = h.pid(), "qemu-ga running");
            Some(h)
        }
        Err(err) => {
            warn!(error = %err, "qemu-ga failed to start");
            None
        }
    }
}

fn spawn() -> Result<GuestAgentHandle, String> {
    let _ = fs::create_dir_all(STATE_DIR);
    let _ = fs::create_dir_all("/var/log");

    let path = match ensure_device_path() {
        Ok(p) => p,
        Err(err) => {
            // Still start with the default path + --retry-path so a late
            // virtio-serial attach (or delayed sysfs) can recover.
            warn!(error = %err, "guest-agent device not ready; qemu-ga will retry");
            PathBuf::from(DEFAULT_PATH)
        }
    };

    let mut child = Command::new(BIN)
        .args([
            "-m",
            "virtio-serial",
            "-p",
            path.to_str().unwrap_or(DEFAULT_PATH),
            "-t",
            STATE_DIR,
            "-l",
            LOG_PATH,
            // Re-open if the channel appears later or is briefly closed.
            "-r",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {BIN}: {e}"))?;

    // Brief settle — catch immediate exit (missing libs).
    std::thread::sleep(std::time::Duration::from_millis(200));
    match child.try_wait() {
        Ok(Some(status)) => {
            return Err(format!("qemu-ga exited immediately: {status}"));
        }
        Ok(None) => {}
        Err(err) => return Err(format!("qemu-ga wait: {err}")),
    }

    Ok(GuestAgentHandle { child })
}

/// Without udev, only `/dev/vportNpM` exists. Symlink the well-known path that
/// `qemu-ga` (and Proxmox) expect.
fn ensure_device_path() -> Result<PathBuf, String> {
    let default = PathBuf::from(DEFAULT_PATH);
    if default.exists() {
        return Ok(default);
    }

    let class = Path::new("/sys/class/virtio-ports");
    if !class.is_dir() {
        return Err("no /sys/class/virtio-ports (virtio console / agent channel absent)".into());
    }

    for entry in fs::read_dir(class).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name_path = entry.path().join("name");
        let Ok(name) = fs::read_to_string(&name_path) else {
            continue;
        };
        if name.trim() != PORT_NAME {
            continue;
        }
        let vport = PathBuf::from("/dev").join(entry.file_name());
        if !vport.exists() {
            return Err(format!("{} missing for {PORT_NAME}", vport.display()));
        }
        fs::create_dir_all("/dev/virtio-ports").map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&default);
        std::os::unix::fs::symlink(&vport, &default).map_err(|e| e.to_string())?;
        info!(
            path = %default.display(),
            target = %vport.display(),
            "linked QEMU guest-agent virtio port"
        );
        return Ok(default);
    }

    Err(format!(
        "virtio port {PORT_NAME} not found under /sys/class/virtio-ports"
    ))
}
