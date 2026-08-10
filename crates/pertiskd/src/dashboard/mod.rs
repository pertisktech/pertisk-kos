//! Console status dashboard — panel TUI (serial-safe buffer dump).
//!
//! Layout/borders adapted from ptkube-dashboard (ASCII frames, network rows,
//! hand-painted logs); named palettes follow feedo's theme module. Renders via
//! ratatui `TestBackend`, then prints the frame with 16-color SGR so Proxmox
//! Serial stays readable (no crossterm cursor addressing).
//!
//! Size is probed once at startup (see `probe`). Overrides, in order:
//! 1. `machine.dashboard` in config.yaml (overwrites early built-ins)
//! 2. Kernel cmdline `PERTISK_DASHBOARD_*` / `pertisk.dashboard.*` when YAML omits that field
//! 3. Built-in defaults ([`DEFAULT_THEME`], [`DEFAULT_BORDER`], …)
//!
//! Enable/disable: `--no-dashboard`, smoke mode, or `pertisk.dashboard.disabled=1`
//! (see [`crate::cmdline`]). Console device: `pertisk.dashboard.console=ttyS0`.

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
pub const DEFAULT_THEME: &str = pertisk_config::Dashboard::DEFAULT_THEME;
pub const DEFAULT_BORDER: &str = pertisk_config::Dashboard::DEFAULT_BORDER;
pub const DEFAULT_COLS: u16 = pertisk_config::Dashboard::DEFAULT_COLS;
pub const DEFAULT_ROWS: u16 = pertisk_config::Dashboard::DEFAULT_ROWS;

/// Push dashboard settings into the process env.
///
/// Early boot (`cfg == None`) only fills theme/border when unset. When YAML
/// provides `machine.dashboard`, those fields **overwrite** early built-ins so
/// `pertiskctl apply` + reboot actually changes the console. The TUI also
/// reloads from STATE each tick (see `tui`).
pub fn apply_config(cfg: Option<&MachineConfig>) {
    match cfg.and_then(|c| c.machine.dashboard.as_ref()) {
        Some(dash) => {
            if let Some(theme) = dash.theme.as_deref() {
                set_var("PERTISK_DASHBOARD_THEME", theme);
            } else {
                set_if_unset("PERTISK_DASHBOARD_THEME", DEFAULT_THEME);
            }
            if let Some(border) = dash.border.as_deref() {
                set_var("PERTISK_DASHBOARD_BORDER", border);
            } else {
                set_if_unset("PERTISK_DASHBOARD_BORDER", DEFAULT_BORDER);
            }
            if let Some(background) = dash.background.as_deref() {
                set_var("PERTISK_DASHBOARD_BACKGROUND", background);
            }
            if let Some(cols) = dash.cols {
                set_var("PERTISK_DASHBOARD_COLS", &cols.to_string());
            }
            if let Some(rows) = dash.rows {
                set_var("PERTISK_DASHBOARD_ROWS", &rows.to_string());
            }
            if let Some(utf8) = dash.utf8 {
                set_var(
                    "PERTISK_DASHBOARD_UTF8",
                    if utf8 { "1" } else { "0" },
                );
            }
            if let Some(url) = dash.mgmt_url.as_deref() {
                set_var("MGMT_PUBLIC_URL", url);
            }
        }
        None => {
            set_if_unset("PERTISK_DASHBOARD_THEME", DEFAULT_THEME);
            set_if_unset("PERTISK_DASHBOARD_BORDER", DEFAULT_BORDER);
            // Leave UTF-8 unset so the console probe can select font-safe
            // ASCII glyphs. YAML or the kernel cmdline can still force it.
        }
    }
}

/// Public management UI base URL for the serial console.
///
/// Prefer `machine.dashboard.mgmt_url` (applied into `MGMT_PUBLIC_URL`), else
/// kernel/env `MGMT_PUBLIC_URL` / `PERTISK_MGMT_URL`.
pub fn mgmt_public_url() -> Option<String> {
    for key in ["MGMT_PUBLIC_URL", "PERTISK_MGMT_URL"] {
        if let Ok(raw) = std::env::var(key) {
            let url = raw.trim().trim_end_matches('/').to_string();
            if !url.is_empty() {
                return Some(url);
            }
        }
    }
    None
}

fn set_if_unset(key: &str, value: &str) {
    if std::env::var_os(key).is_none() {
        set_var(key, value);
    }
}

fn set_var(key: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    // SAFETY: PID 1 dashboard env; boot path + TUI tick serialize writers.
    unsafe { std::env::set_var(key, value) };
}

#[cfg(test)]
pub(crate) static DASH_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use pertisk_config::{Dashboard, Machine, MachineType, Network, CONFIG_VERSION};

    fn clear_dash_env() {
        unsafe {
            for k in [
                "PERTISK_DASHBOARD_THEME",
                "PERTISK_DASHBOARD_BORDER",
                "PERTISK_DASHBOARD_BACKGROUND",
                "PERTISK_DASHBOARD_COLS",
                "PERTISK_DASHBOARD_ROWS",
                "PERTISK_DASHBOARD_UTF8",
                "MGMT_PUBLIC_URL",
                "PERTISK_MGMT_URL",
            ] {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    fn apply_config_leaves_builtin_utf8_to_console_probe() {
        let _guard = DASH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_dash_env();
        apply_config(None);
        assert_eq!(
            std::env::var("PERTISK_DASHBOARD_THEME").unwrap(),
            DEFAULT_THEME
        );
        assert_eq!(
            std::env::var("PERTISK_DASHBOARD_BORDER").unwrap(),
            DEFAULT_BORDER
        );
        assert!(std::env::var_os("PERTISK_DASHBOARD_BACKGROUND").is_none());
        assert!(std::env::var_os("PERTISK_DASHBOARD_COLS").is_none());
        assert!(std::env::var_os("PERTISK_DASHBOARD_ROWS").is_none());
        assert!(std::env::var_os("PERTISK_DASHBOARD_UTF8").is_none());
        clear_dash_env();
    }

    #[test]
    fn cmdline_env_wins_over_early_builtins() {
        let _guard = DASH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    fn yaml_overwrites_early_builtins() {
        let _guard = DASH_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_dash_env();
        apply_config(None);
        assert_eq!(
            std::env::var("PERTISK_DASHBOARD_THEME").unwrap(),
            DEFAULT_THEME
        );

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
                    background: Some("#1E1E2E".into()),
                    cols: Some(120),
                    rows: Some(40),
                    utf8: Some(true),
                    mgmt_url: Some("https://ptkos.apps.thaidevops.co".into()),
                }),
                kubelet: None,
            },
            cluster: None,
        };
        apply_config(Some(&cfg));
        assert_eq!(
            std::env::var("PERTISK_DASHBOARD_THEME").unwrap(),
            "wild-cherry"
        );
        assert_eq!(std::env::var("PERTISK_DASHBOARD_BORDER").unwrap(), "double");
        assert_eq!(
            std::env::var("PERTISK_DASHBOARD_BACKGROUND").unwrap(),
            "#1E1E2E"
        );
        assert_eq!(std::env::var("PERTISK_DASHBOARD_COLS").unwrap(), "120");
        assert_eq!(std::env::var("PERTISK_DASHBOARD_ROWS").unwrap(), "40");
        assert_eq!(std::env::var("PERTISK_DASHBOARD_UTF8").unwrap(), "1");
        assert_eq!(
            std::env::var("MGMT_PUBLIC_URL").unwrap(),
            "https://ptkos.apps.thaidevops.co"
        );
        assert_eq!(
            mgmt_public_url().as_deref(),
            Some("https://ptkos.apps.thaidevops.co")
        );
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

pub fn should_enable_dashboard(no_dashboard: bool, smoke: bool, cmdline_disabled: bool) -> bool {
    !no_dashboard && !smoke && !cmdline_disabled
}

/// Dump one Serial-style dashboard frame to stdout (local layout check).
pub fn preview_serial_frame() -> Result<(), String> {
    tui::preview_serial_frame()
}

#[cfg(test)]
mod enable_tests {
    use super::should_enable_dashboard;

    #[test]
    fn enabled_by_default() {
        assert!(should_enable_dashboard(false, false, false));
    }

    #[test]
    fn disabled_by_flag() {
        assert!(!should_enable_dashboard(true, false, false));
    }

    #[test]
    fn disabled_by_smoke() {
        assert!(!should_enable_dashboard(false, true, false));
    }

    #[test]
    fn disabled_by_cmdline() {
        assert!(!should_enable_dashboard(false, false, true));
    }
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
