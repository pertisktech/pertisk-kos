//! Console status dashboard — panel TUI (serial-safe buffer dump).
//!
//! Layout/borders adapted from ptkube-dashboard (ASCII frames, network rows,
//! hand-painted logs); named palettes follow feedo's theme module. Renders via
//! ratatui `TestBackend`, then prints the frame with 16-color SGR so Proxmox
//! Serial stays readable (no crossterm cursor addressing).
//!
//! Size is probed once at startup (see `probe`). Overrides, in order:
//! 1. Kernel cmdline env (`PERTISK_DASHBOARD_*`)
//! 2. `machine.dashboard` in config.yaml (optional — omit for built-ins)
//! 3. Built-in defaults ([`DEFAULT_THEME`], [`DEFAULT_BORDER`], …)

mod banner;
mod panels;
mod probe;
mod snapshot;
mod theme;
mod tui;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use pertisk_api::SharedState;
use pertisk_config::MachineConfig;
use tracing::{info, warn};

use crate::log_ring::LogRing;

/// Built-in dashboard when config.yaml omits `machine.dashboard`.
pub const DEFAULT_THEME: &str = "catppuccin";
pub const DEFAULT_BORDER: &str = "rounded";
/// Probe fallback when size is not pinned by YAML/env (`93`×`25`).
pub use probe::{FALLBACK_COLS as DEFAULT_COLS, FALLBACK_ROWS as DEFAULT_ROWS};
pub const DEFAULT_UTF8: bool = true;

/// Push dashboard settings into the process env when the matching
/// `PERTISK_DASHBOARD_*` variable is unset (cmdline wins).
///
/// Fresh install needs no YAML — theme/border/size/utf8 all get built-ins.
/// Optional `machine.dashboard` or cmdline env still overrides any field.
pub fn apply_config(cfg: Option<&MachineConfig>) {
    let dash = cfg.and_then(|c| c.machine.dashboard.as_ref());
    set_if_unset(
        "PERTISK_DASHBOARD_THEME",
        dash.and_then(|d| d.theme.as_deref())
            .or(Some(DEFAULT_THEME)),
    );
    set_if_unset(
        "PERTISK_DASHBOARD_BORDER",
        dash.and_then(|d| d.border.as_deref())
            .or(Some(DEFAULT_BORDER)),
    );
    // Always pin size to the install default unless YAML/env overrides.
    // Proxmox Serial often fails the size probe; without this the TUI boots
    // at 80×24 and looks "broken" until the operator applies a dashboard YAML.
    let cols = dash.and_then(|d| d.cols).unwrap_or(DEFAULT_COLS);
    let rows = dash.and_then(|d| d.rows).unwrap_or(DEFAULT_ROWS);
    set_if_unset("PERTISK_DASHBOARD_COLS", Some(&cols.to_string()));
    set_if_unset("PERTISK_DASHBOARD_ROWS", Some(&rows.to_string()));
    let utf8 = dash.and_then(|d| d.utf8).unwrap_or(DEFAULT_UTF8);
    set_if_unset(
        "PERTISK_DASHBOARD_UTF8",
        Some(if utf8 { "1" } else { "0" }),
    );
}

fn set_if_unset(key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };
    if std::env::var_os(key).is_none() {
        // SAFETY: called once from PID 1 before the dashboard thread starts;
        // no other threads read these keys concurrently yet.
        unsafe { std::env::set_var(key, value) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pertisk_config::{Dashboard, Machine, MachineType, Network, CONFIG_VERSION};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_dash_env() {
        unsafe {
            for k in [
                "PERTISK_DASHBOARD_THEME",
                "PERTISK_DASHBOARD_BORDER",
                "PERTISK_DASHBOARD_COLS",
                "PERTISK_DASHBOARD_ROWS",
                "PERTISK_DASHBOARD_UTF8",
            ] {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    fn apply_config_sets_builtin_defaults_without_yaml() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_dash_env();
        apply_config(None);
        assert_eq!(std::env::var("PERTISK_DASHBOARD_THEME").unwrap(), DEFAULT_THEME);
        assert_eq!(std::env::var("PERTISK_DASHBOARD_BORDER").unwrap(), DEFAULT_BORDER);
        assert_eq!(std::env::var("PERTISK_DASHBOARD_UTF8").unwrap(), "1");
        assert_eq!(
            std::env::var("PERTISK_DASHBOARD_COLS").unwrap(),
            DEFAULT_COLS.to_string()
        );
        assert_eq!(
            std::env::var("PERTISK_DASHBOARD_ROWS").unwrap(),
            DEFAULT_ROWS.to_string()
        );
        clear_dash_env();
    }

    #[test]
    fn cmdline_env_wins_over_defaults() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_dash_env();
        unsafe {
            std::env::set_var("PERTISK_DASHBOARD_THEME", "nord");
            std::env::set_var("PERTISK_DASHBOARD_BORDER", "ascii");
        }
        apply_config(None);
        assert_eq!(std::env::var("PERTISK_DASHBOARD_THEME").unwrap(), "nord");
        assert_eq!(std::env::var("PERTISK_DASHBOARD_BORDER").unwrap(), "ascii");
        clear_dash_env();
    }

    #[test]
    fn yaml_overrides_builtin_defaults() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_dash_env();
        let cfg = MachineConfig {
            version: CONFIG_VERSION.into(),
            machine: Machine {
                machine_type: MachineType::Controlplane,
                network: Network {
                    hostname: None,
                    interfaces: vec![],
                    nameservers: vec![],
                },
                install: None,
                dashboard: Some(Dashboard {
                    theme: Some("wild-cherry".into()),
                    border: Some("double".into()),
                    cols: Some(120),
                    rows: Some(40),
                    utf8: Some(false),
                }),
            },
            cluster: None,
        };
        apply_config(Some(&cfg));
        assert_eq!(std::env::var("PERTISK_DASHBOARD_THEME").unwrap(), "wild-cherry");
        assert_eq!(std::env::var("PERTISK_DASHBOARD_BORDER").unwrap(), "double");
        assert_eq!(std::env::var("PERTISK_DASHBOARD_COLS").unwrap(), "120");
        assert_eq!(std::env::var("PERTISK_DASHBOARD_ROWS").unwrap(), "40");
        assert_eq!(std::env::var("PERTISK_DASHBOARD_UTF8").unwrap(), "0");
        clear_dash_env();
    }
}

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
