//! PID 1 supervise loop: reap children and exit on stop signals.

use anyhow::Result;
use tracing::{info, warn};

/// Block forever reaping zombies until SIGTERM/SIGINT/SIGQUIT.
pub fn supervise() -> Result<()> {
    #[cfg(unix)]
    {
        unix_impl::supervise()
    }
    #[cfg(not(unix))]
    {
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

    extern "C" fn handle_chld(_: nix::libc::c_int) {
        // Actual reaping happens in the loop via waitpid(WNOHANG).
    }

    pub fn supervise() -> Result<()> {
        install_handlers()?;
        info!("supervise loop entered");

        while !STOP.load(Ordering::SeqCst) {
            reap_zombies();
            // Cheap sleep so we do not spin; SIGCHLD/signals still interrupt.
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        info!("stop signal received; shutting down");
        // Later phases: stop kubelet/containerd orderly here.
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
