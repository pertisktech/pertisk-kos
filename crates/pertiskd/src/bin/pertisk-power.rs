//! `/sbin/{poweroff,halt,reboot,shutdown}` for qemu-ga / Proxmox Shutdown.
//!
//! Alpine's qemu-ga tries `/sbin/shutdown`, then falls back to `/sbin/poweroff`.
//! BusyBox `poweroff` without `-f` signals PID 1 (busybox init) — which we are
//! not — so the VM never powers off and Proxmox times out. This binary always
//! issues the reboot(2) syscall (ignores extra args from qemu-ga).

fn main() {
    #[cfg(target_os = "linux")]
    {
        use nix::sys::reboot::{reboot, RebootMode};

        let arg0 = std::env::args().next().unwrap_or_default();
        let name = std::path::Path::new(&arg0)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("poweroff");

        // Best-effort flush before power transition.
        unsafe {
            libc::sync();
        }

        let mode = match name {
            "reboot" => RebootMode::RB_AUTOBOOT,
            "halt" => RebootMode::RB_HALT_SYSTEM,
            // poweroff, shutdown, or anything else → power off
            _ => RebootMode::RB_POWER_OFF,
        };
        let _ = reboot(mode);
    }
}
