//! Console status dashboard — panel TUI (serial-safe buffer dump).
//!
//! Layout/borders adapted from ptkube-dashboard (ASCII frames, network rows,
//! hand-painted logs). Renders via ratatui `TestBackend`, then prints the frame
//! so Proxmox Serial stays readable (no crossterm cursor addressing).

mod banner;
mod panels;
mod snapshot;
mod tui;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use pertisk_api::SharedState;
use pertisk_config::MachineConfig;
use tracing::{info, warn};

use crate::log_ring::LogRing;

/// Handle that stops the status thread.
pub struct DashboardHandle {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl DashboardHandle {
    #[allow(dead_code)]
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for DashboardHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub fn should_enable_dashboard(no_dashboard: bool, smoke: bool) -> bool {
    !no_dashboard && !smoke
}

/// Start the console panel TUI (falls back to text banner only if TUI panics).
pub fn start_dashboard(
    cfg: Option<MachineConfig>,
    state: SharedState,
    state_root: PathBuf,
    logs: LogRing,
) -> Option<DashboardHandle> {
    // Keep tracing/child lines in the ring only — stderr fights the Serial TUI
    // and makes the cursor blink.
    logs.set_silence_stderr(true);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = stop.clone();
    let logs_stop = logs.clone();
    let join = thread::Builder::new()
        .name("pertisk-dashboard".into())
        .spawn(move || {
            match tui::run_tui_loop(
                stop_t.clone(),
                cfg.clone(),
                state.clone(),
                state_root.clone(),
                logs.clone(),
            ) {
                Ok(()) => {}
                Err(err) => {
                    eprintln!("pertiskd: TUI failed ({err}); using text dashboard");
                    warn!(error = %err, "console TUI failed; using text dashboard");
                    info!("console text dashboard started");
                    banner::run_banner_loop(stop_t, cfg, state, state_root, logs);
                }
            }
            logs_stop.set_silence_stderr(false);
        })
        .ok()?;
    Some(DashboardHandle {
        stop,
        join: Some(join),
    })
}
