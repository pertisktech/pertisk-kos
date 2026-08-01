//! `pertiskd` — Pertisk KOS init / node supervisor.
//!
//! Milestone M4: management gRPC API + containerd/kubelet supervision.

mod hostname;
mod linux;
mod reaper;
mod services;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process;
use std::thread;

use anyhow::{Context, Result};
use clap::Parser;
use pertisk_api::{shared, SharedState, TlsPaths, DEFAULT_LISTEN};
use pertisk_config::MachineConfig;
use pertisk_disk::{layout_present, prepare_state, try_prepare_esp, StateVolume};
use pertisk_update::{record_boot_attempt_with_layout, SlotLayout};
#[cfg(target_os = "linux")]
use pertisk_update::{bootstrap_esp, BootAssets, EspPaths, INSTALLER_BOOT_DIR};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use services::NodeServices;

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

    /// Exit after boot smoke checks (no long-running supervise loop).
    #[arg(long, default_value_t = false)]
    smoke: bool,

    /// Skip network configuration.
    #[arg(long, default_value_t = false)]
    skip_network: bool,

    /// Skip starting containerd/kubelet.
    #[arg(long, default_value_t = false)]
    skip_runtime: bool,

    /// Skip starting the management gRPC API.
    #[arg(long, default_value_t = false)]
    skip_api: bool,

    /// Management API listen address.
    #[arg(long, env = "PERTISK_API_LISTEN", default_value = DEFAULT_LISTEN)]
    api_listen: String,

    /// CA certificate for mTLS (enables TLS when set with server cert/key).
    #[arg(long, env = "PERTISK_TLS_CA")]
    tls_ca: Option<PathBuf>,

    /// Server certificate (PEM).
    #[arg(long, env = "PERTISK_TLS_CERT")]
    tls_cert: Option<PathBuf>,

    /// Server private key (PEM).
    #[arg(long, env = "PERTISK_TLS_KEY")]
    tls_key: Option<PathBuf>,

    /// Ed25519 public key (hex) trusted for OS upgrade signatures.
    #[arg(long, env = "PERTISK_TRUST_KEY", default_value = "/system/state/secrets/os-trust.pk")]
    trust_key: PathBuf,
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

    let early_cfg = load_early_config(&args)?;

    if let Some(ref cfg) = early_cfg {
        maybe_install(cfg, &args)?;
    }

    let volume = prepare_boot_state(args.state_dir.as_deref())?;
    let _esp = try_prepare_esp();
    let cfg = load_boot_config(&args, &volume)?.or(early_cfg);

    if let Some(ref cfg) = cfg {
        if let Some(name) = cfg.machine.network.hostname.as_deref() {
            hostname::set_hostname(name)?;
        }
        if !args.skip_network {
            if let Err(err) = pertisk_net::apply_network(&cfg.machine.network) {
                warn!(error = %err, "network apply failed");
            }
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

    // Track A/B boot attempts / auto-rollback before starting workloads.
    let boot_layout = SlotLayout::new(volume.root.clone(), resolve_trust_key(&args, &volume));
    match record_boot_attempt_with_layout(&volume.root, Some(&boot_layout)) {
        Ok(meta) => info!(
            active = %meta.active,
            next = %meta.next,
            boot_ok = meta.boot_ok,
            attempts = meta.boot_attempts,
            "boot meta loaded"
        ),
        Err(err) => warn!(error = %err, "boot meta update failed"),
    }

    let mut services = if args.skip_runtime {
        NodeServices {
            containerd: None,
            kubelet: None,
        }
    } else if let Some(ref cfg) = cfg {
        NodeServices::start(cfg)?
    } else {
        NodeServices {
            containerd: None,
            kubelet: None,
        }
    };

    let trust_key = resolve_trust_key(&args, &volume);
    let api_state = shared(volume.root.clone(), trust_key);
    {
        let (cd, kl, cd_pid, kl_pid) = services.status_parts();
        if let Ok(mut st) = api_state.lock() {
            st.set_runtime_status(cd, kl, cd_pid, kl_pid);
            st.message = "running".into();
        }
    }

    let tls = resolve_tls(&args);
    if !args.skip_api {
        start_api_thread(api_state.clone(), &args.api_listen, tls)?;
    }

    if args.smoke || !is_pid1 {
        info!(
            state = %volume.root.display(),
            source = ?volume.source,
            layout_present = layout_present(),
            runtime = %services.status_summary(),
            api = %args.api_listen,
            skip_api = args.skip_api,
            "M4 smoke complete"
        );
        if let Some(cd) = services.containerd.take() {
            cd.stop();
        }
        if let Some(kl) = services.kubelet.take() {
            kl.stop();
        }
        if !is_pid1 {
            info!("dev mode (not PID 1); use --force-init to supervise + serve API");
            return Ok(());
        }
        // PID 1 must not return — power off so QEMU -no-reboot exits cleanly.
        info!("smoke done; powering off");
        #[cfg(target_os = "linux")]
        {
            use nix::sys::reboot::{reboot, RebootMode};
            let _ = reboot(RebootMode::RB_POWER_OFF);
        }
        loop {
            std::thread::park();
        }
    }

    info!("running as init");
    reaper::supervise(cfg, services, api_state)?;
    Ok(())
}

fn start_api_thread(
    state: SharedState,
    listen: &str,
    tls: Option<TlsPaths>,
) -> Result<()> {
    let addr: SocketAddr = listen
        .parse()
        .with_context(|| format!("invalid --api-listen {listen}"))?;
    let mode = if tls.is_some() { "mTLS" } else { "plaintext" };
    thread::Builder::new()
        .name("pertisk-api".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    warn!(error = %err, "failed to build API runtime");
                    return;
                }
            };
            if let Err(err) = rt.block_on(pertisk_api::serve(state, addr, tls)) {
                warn!(error = %err, "management API stopped");
            }
        })?;
    info!(%addr, mode, "management API starting");
    Ok(())
}

fn resolve_tls(args: &Args) -> Option<TlsPaths> {
    match (&args.tls_ca, &args.tls_cert, &args.tls_key) {
        (Some(ca), Some(cert), Some(key)) => Some(TlsPaths {
            ca_cert: ca.clone(),
            server_cert: cert.clone(),
            server_key: key.clone(),
        }),
        (None, None, None) => None,
        _ => {
            warn!("incomplete TLS flags (need --tls-ca, --tls-cert, --tls-key); using plaintext");
            None
        }
    }
}

fn resolve_trust_key(args: &Args, volume: &StateVolume) -> PathBuf {
    if args.trust_key.exists() {
        return args.trust_key.clone();
    }
    // Dev default under STATE.
    let fallback = volume.root.join("secrets/os-trust.pk");
    if fallback.exists() {
        return fallback;
    }
    args.trust_key.clone()
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn load_early_config(args: &Args) -> Result<Option<MachineConfig>> {
    if let Some(ref path) = args.config {
        return Ok(Some(read_config(path)?));
    }
    for candidate in [
        PathBuf::from("/system/state/config.yaml"),
        PathBuf::from("/config.yaml"),
    ] {
        if candidate.exists() {
            return Ok(Some(read_config(&candidate)?));
        }
    }
    Ok(None)
}

fn maybe_install(cfg: &MachineConfig, args: &Args) -> Result<()> {
    let Some(install) = cfg.machine.install.as_ref() else {
        return Ok(());
    };

    if layout_present() && !install.wipe {
        info!(disk = %install.disk, "Pertisk layout present; skipping install");
        return Ok(());
    }

    if args.state_dir.is_some() {
        info!("--state-dir set; skipping disk install");
        return Ok(());
    }

    #[cfg(not(target_os = "linux"))]
    {
        info!(disk = %install.disk, "install planned (not Linux); skipping");
        let _ = install;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        use pertisk_disk::{install_disk, InstallOptions};

        let seed = args
            .config
            .clone()
            .filter(|p| p.exists())
            .or_else(|| {
                let p = PathBuf::from("/system/state/config.yaml");
                p.exists().then_some(p)
            });

        let seed_path = if let Some(ref cfg_path) = seed {
            let mut seeded = read_config(cfg_path)?;
            seeded.machine.install = None;
            let tmp = PathBuf::from("/run/pertisk-seed-config.yaml");
            std::fs::create_dir_all("/run")?;
            std::fs::write(&tmp, serde_yaml::to_string(&seeded)?)?;
            Some(tmp)
        } else {
            None
        };

        let opts = InstallOptions {
            disk: PathBuf::from(&install.disk),
            wipe: install.wipe,
            seed_config: seed_path,
        };
        info!(disk = %opts.disk.display(), wipe = opts.wipe, "starting disk install");
        install_disk(&opts)?;
        info!("disk install complete");

        // Mount fresh EFI and seed systemd-boot + slot A when installer assets exist.
        bootstrap_esp_after_install()?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn bootstrap_esp_after_install() -> Result<()> {
    use pertisk_disk::prepare_esp;

    let installer = PathBuf::from(INSTALLER_BOOT_DIR);
    if !installer.join("kernel").exists() {
        warn!(
            dir = %installer.display(),
            "installer boot assets missing; ESP left empty (re-build with PERTISK_EMBED_BOOT=1)"
        );
        return Ok(());
    }

    let assets = BootAssets::from_installer_dir(&installer)
        .context("resolve installer boot assets")?;
    let esp_vol = prepare_esp().context("mount EFI after install")?.ok_or_else(|| {
        anyhow::anyhow!("EFI partition not found after install")
    })?;
    let esp = EspPaths {
        root: esp_vol.root,
    };
    bootstrap_esp(&esp, &assets).context("bootstrap ESP")?;
    info!(esp = %esp.root.display(), "ESP bootstrapped with systemd-boot + slot A");
    Ok(())
}

fn prepare_boot_state(state_dir: Option<&std::path::Path>) -> Result<StateVolume> {
    match prepare_state(state_dir) {
        Ok(vol) => Ok(vol),
        Err(err) => {
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

    Ok(Some(read_config(&path)?))
}

fn read_config(path: &std::path::Path) -> Result<MachineConfig> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let cfg = MachineConfig::from_yaml(&raw)?;
    info!(path = %path.display(), "config loaded");
    Ok(cfg)
}
