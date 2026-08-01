//! `pertiskd` — Pertisk KOS init / node supervisor.
//!
//! Milestone M1: prepare STATE, load machine config from STATE (or `--config`),
//! apply hostname, then enter the supervise loop when acting as init.

mod hostname;
mod linux;
mod reaper;

use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::Parser;
use pertisk_config::MachineConfig;
use pertisk_disk::{prepare_state, StateVolume};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "pertiskd", about = "Pertisk KOS init and node supervisor")]
struct Args {
    /// Explicit machine config path (overrides STATE config when set).
    #[arg(long, env = "PERTISK_CONFIG")]
    config: Option<PathBuf>,

    /// Directory to use as STATE (dev / QEMU without a disk).
    #[arg(long, env = "PERTISK_STATE_DIR")]
    state_dir: Option<PathBuf>,

    /// Run the supervise loop even when not PID 1.
    #[arg(long, default_value_t = false)]
    force_init: bool,

    /// Exit after STATE + config smoke checks (no supervise loop).
    #[arg(long, default_value_t = false)]
    smoke: bool,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("pertiskd fatal: {err:#}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    init_tracing();

    let args = Args::parse();
    let pid = process::id();
    let is_pid1 = pid == 1 || args.force_init;

    info!(
        pid,
        version = env!("CARGO_PKG_VERSION"),
        is_pid1,
        "pertiskd starting"
    );

    if is_pid1 {
        linux::prepare_filesystem()?;
    }

    let state = prepare_boot_state(args.state_dir.as_deref())?;
    let cfg = load_boot_config(&args, &state)?;

    if let Some(ref cfg) = cfg {
        if let Some(name) = cfg.machine.network.hostname.as_deref() {
            hostname::set_hostname(name)?;
        }
        info!(
            machine_type = ?cfg.machine.machine_type,
            hostname = ?cfg.machine.network.hostname,
            "machine config applied"
        );
    } else {
        warn!("no machine config found; continuing without");
    }

    linux::prepare_var()?;

    if args.smoke || !is_pid1 {
        info!(
            state = %state.root.display(),
            source = ?state.source,
            "M1 smoke complete"
        );
        if !is_pid1 {
            info!("dev mode (not PID 1); use --force-init to supervise");
        }
        return Ok(());
    }

    info!("running as init");
    reaper::supervise()?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn prepare_boot_state(state_dir: Option<&std::path::Path>) -> Result<StateVolume> {
    match prepare_state(state_dir) {
        Ok(vol) => Ok(vol),
        Err(err) => {
            // Dev hosts without --state-dir: create ./.pertisk-state for convenience.
            if state_dir.is_none() && cfg!(not(target_os = "linux")) {
                let fallback = PathBuf::from(".pertisk-state");
                warn!(
                    error = %err,
                    path = %fallback.display(),
                    "STATE discover failed; using local fallback directory"
                );
                Ok(prepare_state(Some(&fallback))?)
            } else {
                Err(err.into())
            }
        }
    }
}

fn load_boot_config(args: &Args, state: &StateVolume) -> Result<Option<MachineConfig>> {
    let path = if let Some(ref explicit) = args.config {
        explicit.clone()
    } else {
        state.config_path()
    };

    if !path.exists() {
        warn!(path = %path.display(), "config file missing");
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let cfg = MachineConfig::from_yaml(&raw)?;
    info!(path = %path.display(), "config loaded");
    Ok(Some(cfg))
}
