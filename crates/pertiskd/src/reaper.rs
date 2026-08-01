//! PID 1 supervise loop: reap zombies, babysit services, honor API power actions.

use anyhow::Result;
use pertisk_api::{PowerAction, SharedState};
use pertisk_config::MachineConfig;
use tracing::{info, warn};

use crate::services::NodeServices;

/// Block forever until stop signal or API-requested power action.
pub fn supervise(
    cfg: Option<MachineConfig>,
    mut services: NodeServices,
    state: SharedState,
) -> Result<()> {
    #[cfg(unix)]
    {
        unix_impl::supervise(cfg, &mut services, state)
    }
    #[cfg(not(unix))]
    {
        let _ = (cfg, services, state);
        anyhow::bail!("pertiskd supervise loop requires Unix")
    }
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;

    static STOP: AtomicBool = AtomicBool::new(false);

    extern "C" fn handle_stop(_: nix::libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    extern "C" fn handle_chld(_: nix::libc::c_int) {}

    pub fn supervise(
        cfg: Option<MachineConfig>,
        services: &mut NodeServices,
        state: SharedState,
    ) -> Result<()> {
        install_handlers()?;
        refresh_state(services, &state);
        info!(status = %services.status_summary(), "supervise loop entered");

        while !STOP.load(Ordering::SeqCst) {
            reap_zombies();
            if let Some(ref cfg) = cfg {
                services.ensure_alive(cfg);
            }
            refresh_state(services, &state);

            let power = state.lock().map(|s| s.power).unwrap_or(PowerAction::None);
            match power {
                PowerAction::Reboot => {
                    info!("executing API reboot");
                    stop_services(services);
                    do_reboot()?;
                    break;
                }
                PowerAction::Shutdown => {
                    info!("executing API shutdown");
                    stop_services(services);
                    do_shutdown()?;
                    break;
                }
                PowerAction::None => {}
            }

            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        info!("supervise loop exiting");
        stop_services(services);
        Ok(())
    }

    fn refresh_state(services: &mut NodeServices, state: &SharedState) {
        let (cd, kl, cd_pid, kl_pid) = services.status_parts();
        if let Ok(mut st) = state.lock() {
            st.set_runtime_status(cd, kl, cd_pid, kl_pid);
        }
    }

    fn stop_services(services: &mut NodeServices) {
        if let Some(cd) = services.containerd.take() {
            cd.stop();
        }
        if let Some(kl) = services.kubelet.take() {
            kl.stop();
        }
    }

    fn do_reboot() -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use nix::sys::reboot::{reboot, RebootMode};
            let _ = nix::unistd::sync();
            reboot(RebootMode::RB_AUTOBOOT)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            info!("reboot requested (dev host — exiting process)");
            STOP.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn do_shutdown() -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use nix::sys::reboot::{reboot, RebootMode};
            let _ = nix::unistd::sync();
            reboot(RebootMode::RB_POWER_OFF)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            info!("shutdown requested (dev host — exiting process)");
            STOP.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn install_handlers() -> Result<()> {
        let stop = SigAction::new(
            SigHandler::Handler(handle_stop),
            SaFlags::empty(),
            SigSet::empty(),
        );
        let chld = SigAction::new(
            SigHandler::Handler(handle_chld),
            SaFlags::SA_NOCLDSTOP,
            SigSet::empty(),
        );

        unsafe {
            sigaction(Signal::SIGTERM, &stop)?;
            sigaction(Signal::SIGINT, &stop)?;
            sigaction(Signal::SIGQUIT, &stop)?;
            sigaction(Signal::SIGCHLD, &chld)?;
        }
        Ok(())
    }

    fn reap_zombies() {
        loop {
            match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(pid, code)) => {
                    info!(pid = pid.as_raw(), code, "reaped child (exit)");
                }
                Ok(WaitStatus::Signaled(pid, sig, _)) => {
                    warn!(pid = pid.as_raw(), signal = %sig, "reaped child (signal)");
                }
                Ok(WaitStatus::StillAlive) | Ok(_) => break,
                Err(nix::errno::Errno::ECHILD) => break,
                Err(err) => {
                    warn!(error = %err, "waitpid failed");
                    break;
                }
            }
        }
    }
}
