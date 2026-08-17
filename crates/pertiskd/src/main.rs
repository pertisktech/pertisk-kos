//! `pertiskd` — Pertisk KOS init / node supervisor.
//!
//! Milestone M4: management gRPC API + containerd/kubelet supervision.

mod cmdline;
mod dashboard;
mod guest_agent;
mod hostname;
mod linux;
mod log_ring;
mod modules;
mod reaper;
mod services;
mod sysctl;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process;
use std::sync::OnceLock;
use std::thread;

use anyhow::{Context, Result};
use clap::Parser;
use pertisk_api::{
    apply_loki_push, apply_prom_push, init_loki_cli, init_prom_push_cli, shared, SharedState,
    TlsPaths, DEFAULT_LISTEN,
    DEFAULT_METRICS_LISTEN,
};
use pertisk_config::MachineConfig;
use pertisk_disk::{
    layout_present, prepare_state, settle_block_devices, try_prepare_ephemeral, try_prepare_esp,
    StateVolume,
};
#[cfg(target_os = "linux")]
use pertisk_update::{bootstrap_esp, BootAssets, EspPaths, INSTALLER_BOOT_DIR};
use pertisk_update::{record_boot_attempt_with_layout, SlotLayout};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use dashboard::{should_enable_dashboard, start_dashboard};
use log_ring::LogRing;
use services::NodeServices;

static LOG_RING: OnceLock<LogRing> = OnceLock::new();

fn parse_args_safe() -> Args {
    let filtered: Vec<std::ffi::OsString> = std::env::args_os()
        .enumerate()
        .filter(|(i, a)| {
            if *i == 0 {
                return true;
            }
            let s = a.to_string_lossy();
            // Our flags are `--foo`; kernel leftovers look like `key=value`.
            s.starts_with('-')
        })
        .map(|(_, a)| a)
        .collect();
    match Args::try_parse_from(&filtered) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("pertiskd: arg parse warning (using defaults)");
            Args::parse_from(["pertiskd"])
        }
    }
}

fn log_ring() -> &'static LogRing {
    LOG_RING.get_or_init(LogRing::default)
}

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

    /// Skip starting the Prometheus metrics HTTP endpoint.
    #[arg(long, default_value_t = false)]
    skip_metrics: bool,

    /// Management API listen address.
    #[arg(long, env = "PERTISK_API_LISTEN", default_value = DEFAULT_LISTEN)]
    api_listen: String,

    /// Prometheus metrics listen address.
    #[arg(long, env = "PERTISK_METRICS_LISTEN", default_value = DEFAULT_METRICS_LISTEN)]
    metrics_listen: String,

    /// Optional bearer token for GET /metrics (`Authorization: Bearer …`).
    /// Also loaded from STATE `secrets/metrics.token` when unset.
    /// When `--tls-*` is set, metrics are served over mTLS (same PEMs as the API).
    #[arg(long, env = "PERTISK_METRICS_TOKEN")]
    metrics_token: Option<String>,

    /// Loki / Alloy push URL (`/loki/api/v1/push`). Empty disables log ship.
    /// Overrides `machine.observability.lokiUrl`.
    #[arg(long, env = "PERTISK_LOKI_URL")]
    loki_url: Option<String>,

    /// Optional bearer for Loki push (`Authorization: Bearer`).
    #[arg(long, env = "PERTISK_LOKI_TOKEN")]
    loki_token: Option<String>,

    /// Prometheus Pushgateway base URL (`http://host:9091`). Empty disables.
    /// Overrides `machine.observability.prometheusPushUrl`. When unset, derived
    /// from Loki URL if that uses compose Alloy `:3500` → `:9091`.
    #[arg(long, env = "PERTISK_PROM_PUSH_URL")]
    prometheus_push_url: Option<String>,

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
    #[arg(
        long,
        env = "PERTISK_TRUST_KEY",
        default_value = "/system/state/secrets/os-trust.pk"
    )]
    trust_key: PathBuf,

    /// Disable the fullscreen serial/console status dashboard.
    #[arg(long, default_value_t = false)]
    no_dashboard: bool,

    /// Print one Serial-style dashboard frame to stdout and exit (local preview).
    #[arg(long, default_value_t = false)]
    dashboard_preview: bool,
}

fn main() {
    let pid = process::id();
    // Fan out panic text to every console path before stdio redirect — otherwise
    // aarch64 virt (ttyAMA0) never shows Rust's default stderr panic (exit 101).
    install_console_panic_hook();
    let run_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    match run_result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            eprintln!("pertiskd fatal: {err:#}");
            if pid != 1 {
                process::exit(1);
            }
            // PID 1 must never exit — that becomes "Attempted to kill init".
            eprintln!("pertiskd: staying alive as PID 1 after fatal error");
        }
        Err(_) => {
            eprintln!("pertiskd: panicked (see panic hook output)");
            if pid != 1 {
                process::exit(101);
            }
            eprintln!("pertiskd: staying alive as PID 1 after panic");
        }
    }
    if pid == 1 {
        loop {
            thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
}

fn install_console_panic_hook() {
    use std::io::Write;
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("pertiskd PANIC: {info}\n");
        for path in [
            "/dev/ttyAMA0",
            "/dev/console",
            "/dev/ttyS0",
            "/dev/tty0",
        ] {
            if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path) {
                let _ = f.write_all(msg.as_bytes());
                let _ = f.flush();
            }
        }
        // Still print via stderr in case it is already redirected.
        let _ = std::io::stderr().write_all(msg.as_bytes());
        default(info);
    }));
}

fn run() -> Result<()> {
    let pid = process::id();
    // PID 1: mount /dev and bind stdio to serial *before* any logs, otherwise
    // "Run /init as init process" is the last thing visible on Proxmox Serial.
    if pid == 1 {
        // Kernel starts init with an empty PATH; disk/net helpers live in /usr/sbin.
        if std::env::var_os("PATH").is_none() {
            // SAFETY: single-threaded PID 1 boot; no concurrent env readers yet.
            unsafe {
                std::env::set_var("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
            }
        }
        if let Err(err) = linux::prepare_filesystem() {
            eprintln!("pertiskd: filesystem prepare failed: {err:#}");
        }
        // os-release is also written inside prepare_filesystem; ensure even if
        // mounts partially failed so kubelet never reports OS-IMAGE=Unknown.
        if let Err(err) = linux::ensure_os_release() {
            eprintln!("pertiskd: os-release write failed: {err:#}");
        }
        if let Err(err) = linux::redirect_stdio_serial() {
            eprintln!("pertiskd: serial stdio redirect failed: {err:#}");
        }
        // Alpine linux-virt ships virtio_net as a module — load before DHCP.
        modules::load_boot_modules();
    }

    init_tracing();

    // Kernel forwards unrecognized cmdline tokens to PID 1 as argv (e.g.
    // `console=ttyAMA0`). Clap treats those as unknown args; formatting the
    // error can panic inside clap (exit 101 → "Attempted to kill init").
    // Keep argv0 + dash-options only.
    let args = parse_args_safe();
    if args.dashboard_preview {
        return dashboard::preview_serial_frame().map_err(|e| anyhow::anyhow!(e));
    }
    let is_pid1 = pid == 1 || args.force_init;

    info!(
        pid,
        version = pertisk_config::release_version(),
        is_pid1,
        "pertiskd starting"
    );

    // Start the Serial dashboard ASAP — DHCP / STATE / containerd can take
    // minutes and previously blocked the TUI until everything finished.
    let provisional_root = PathBuf::from("/system/state");
    let provisional_trust = args.trust_key.clone();
    let api_state = shared(provisional_root.clone(), provisional_trust);
    if let Ok(mut st) = api_state.lock() {
        st.set_message("booting");
        st.ready = false;
    }
    let _dashboard = if should_enable_dashboard(
        args.no_dashboard,
        args.smoke,
        cmdline::dashboard_settings().disabled,
    ) {
        dashboard::apply_config(None);
        match start_dashboard(
            None,
            api_state.clone(),
            provisional_root,
            log_ring().clone(),
        ) {
            Some(handle) => {
                info!("console dashboard started (early)");
                Some(handle)
            }
            None => {
                warn!("console dashboard failed to start");
                None
            }
        }
    } else {
        info!(
            no_dashboard = args.no_dashboard,
            smoke = args.smoke,
            cmdline_disabled = cmdline::dashboard_settings().disabled,
            "console dashboard disabled"
        );
        None
    };

    if is_pid1 {
        // After disk modules: rescans so STATE/EPHEMERAL nodes are mountable.
        if let Ok(mut st) = api_state.lock() {
            st.set_message("settling disks");
        }
        settle_block_devices();
    }

    if is_pid1 && pid != 1 {
        // --force-init on a non-PID1 process still needs mounts in some lab setups.
        linux::prepare_filesystem()?;
    }

    let early_cfg = match load_early_config(&args) {
        Ok(c) => c,
        Err(err) => {
            warn!(error = %err, "early config load failed");
            None
        }
    };

    // DHCP + API *before* STATE mount. On AHV, virtio-scsi I/O can hang while
    // mounting STATE; lab-up still needs a live address / :50000. After STATE is
    // up we re-apply network so INIT-REBOOT can reclaim the lease file.
    let early_dual = early_cfg
        .as_ref()
        .and_then(|c| c.cluster.as_ref())
        .map(|c| c.is_dual_stack())
        .unwrap_or(false);
    sysctl::apply_ipv6_policy(early_dual);
    if let Ok(mut st) = api_state.lock() {
        st.set_message("network");
    }
    if let Some(ref cfg) = early_cfg {
        if let Some(name) = cfg.machine.network.hostname.as_deref() {
            if let Err(err) = hostname::set_hostname(name) {
                warn!(error = %err, "hostname apply failed");
            }
        }
        if !args.skip_network {
            if let Err(err) = pertisk_net::apply_network(&cfg.machine.network) {
                warn!(error = %err, "early network apply failed");
            }
        }
    } else if !args.skip_network {
        let early_net = pertisk_config::Network {
            hostname: None,
            interfaces: vec![pertisk_config::Interface {
                interface: "eth0".into(),
                dhcp: true,
                addresses: vec![],
                gateway: None,
            }],
            nameservers: vec![],
        };
        if let Err(err) = pertisk_net::apply_network(&early_net) {
            warn!(error = %err, "early default DHCP failed");
        }
    }

    let tls = resolve_tls(&args);
    let mut api_started = false;
    if !args.skip_api {
        if let Ok(mut st) = api_state.lock() {
            st.set_message("API listening (boot continuing)");
        }
        match start_api_thread(api_state.clone(), &args.api_listen, tls.clone()) {
            Ok(()) => api_started = true,
            Err(err) => warn!(error = %err, "management API failed to start"),
        }
    }

    // Start qemu-ga before STATE/EPHEMERAL so Proxmox QGA can report DHCP IPs
    // while lab-up is still waiting (no L2 ARP required).
    let mut guest_agent = guest_agent::start();

    if let Ok(mut st) = api_state.lock() {
        st.set_message("mounting STATE");
    }
    let volume = match prepare_boot_state(args.state_dir.as_deref()) {
        Ok(v) => v,
        Err(err) => {
            warn!(error = %err, "STATE prepare failed; using /tmp/pertisk-state");
            let fallback = PathBuf::from("/tmp/pertisk-state");
            let _ = std::fs::create_dir_all(&fallback);
            match prepare_boot_state(Some(&fallback)) {
                Ok(v) => v,
                Err(err2) => {
                    warn!(error = %err2, "fallback STATE failed");
                    return Err(err2);
                }
            }
        }
    };
    log_ring().set_state_root(&volume.root);
    pertisk_net::set_lease_dir(Some(&volume.root.join("machine/dhcp")));
    let trust_key = resolve_trust_key(&args, &volume);
    if let Ok(mut st) = api_state.lock() {
        st.bind_state(volume.root.clone(), trust_key.clone());
        st.set_message("STATE ready");
    }
    info!(
        path = %volume.root.display(),
        source = ?volume.source,
        layout_present = layout_present(),
        config_exists = volume.config_path().exists(),
        "STATE volume ready"
    );

    if let Some(ref cfg) = early_cfg {
        if let Err(err) = maybe_install(cfg, &args) {
            warn!(error = %err, "install step failed");
        }
    }

    // First-boot install may have just created STATE on disk — remount PARTLABEL.
    let volume = if args.state_dir.is_none() {
        match prepare_boot_state(None) {
            Ok(v) => {
                if v.root != volume.root {
                    log_ring().set_state_root(&v.root);
                    pertisk_net::set_lease_dir(Some(&v.root.join("machine/dhcp")));
                    info!(
                        path = %v.root.display(),
                        source = ?v.source,
                        "STATE remounted after install"
                    );
                }
                v
            }
            Err(err) => {
                warn!(error = %err, "STATE re-prepare after install failed; keeping prior volume");
                volume
            }
        }
    } else {
        volume
    };
    let trust_key = resolve_trust_key(&args, &volume);
    if let Ok(mut st) = api_state.lock() {
        st.bind_state(volume.root.clone(), trust_key.clone());
    }

    let _esp = try_prepare_esp();
    let cfg = load_boot_config(&args, &volume)
        .unwrap_or_else(|err| {
            warn!(error = %err, "boot config load failed");
            None
        })
        .or(early_cfg);

    if let Some(ref cfg) = cfg {
        if let Some(name) = cfg.machine.network.hostname.as_deref() {
            if let Err(err) = hostname::set_hostname(name) {
                warn!(error = %err, "hostname apply failed");
            }
        }
        // Re-apply after seed/config load (may reclaim lease if early DHCP differed).
        if !args.skip_network {
            let dual = cfg
                .cluster
                .as_ref()
                .map(|c| c.is_dual_stack())
                .unwrap_or(false);
            sysctl::apply_ipv6_policy(dual);
            if let Err(err) = pertisk_net::apply_network(&cfg.machine.network) {
                warn!(error = %err, "network apply failed");
            }
        }
        info!(
            machine_type = ?cfg.machine.machine_type,
            hostname = ?cfg.machine.network.hostname,
            "machine config applied"
        );
        // Theme/border from YAML — env only fills unset keys (early start used built-ins).
        dashboard::apply_config(Some(cfg));
    } else {
        warn!("no machine config found; continuing without");
    }

    if !api_started && !args.skip_api {
        if let Ok(mut st) = api_state.lock() {
            st.set_message("API listening (EPHEMERAL pending)");
        }
        if let Err(err) = start_api_thread(api_state.clone(), &args.api_listen, tls) {
            warn!(error = %err, "management API failed to start");
        }
    }

    if let Err(err) = linux::prepare_var() {
        warn!(error = %err, "/var prepare failed");
    }
    // Prefer disk-backed /var (container images, etcd, logs) over tmpfs.
    if let Ok(mut st) = api_state.lock() {
        st.set_message("preparing EPHEMERAL");
    }
    let ephemeral = try_prepare_ephemeral();
    info!(
        ephemeral_mounted = ephemeral.is_some(),
        "EPHEMERAL /var status"
    );
    // Cilium hostPath Bidirectional on /var/run/netns requires /var shared.
    // EPHEMERAL (or tmpfs) is a separate mount from `/`, so rshared(`/sys`) is
    // not enough — mark /var after it is the final mount.
    if let Err(err) = linux::make_rshared("/var") {
        warn!(error = %err, "make-rshared /var failed");
    }
    if let Err(err) = linux::ensure_var_run_shared() {
        warn!(error = %err, "ensure /var/run shared for Cilium netns failed");
    }
    let _ = std::fs::create_dir_all("/var/run/netns");
    let _ = std::fs::create_dir_all("/run/netns");
    // Re-publish CP static pods + cert kubeconfigs before kubelet starts.
    // Without this, reboot loses /etc/kubernetes and kubelet exits immediately.
    match pertisk_bootstrap::restore_control_plane(&volume.root) {
        Ok(true) => info!("control-plane material restored from STATE"),
        Ok(false) => {}
        Err(err) => warn!(error = %err, "control-plane restore failed"),
    }
    // Before kubelet (`protectKernelDefaults: true`).
    sysctl::apply_hardening_sysctls();

    // Track A/B boot attempts / auto-rollback before starting workloads.
    let boot_layout = SlotLayout::new(volume.root.clone(), trust_key.clone());
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

    if let Ok(mut st) = api_state.lock() {
        st.set_message("starting runtime");
    }
    let mut services = if args.skip_runtime {
        NodeServices {
            containerd: None,
            kubelet: None,
            guest_agent,
        }
    } else if let Some(ref cfg) = cfg {
        match NodeServices::start_with_guest_agent(cfg, log_ring(), guest_agent) {
            Ok(s) => s,
            Err(err) => {
                warn!(error = %err, "runtime services failed to start");
                NodeServices {
                    containerd: None,
                    kubelet: None,
                    guest_agent: guest_agent::start(),
                }
            }
        }
    } else {
        NodeServices {
            containerd: None,
            kubelet: None,
            guest_agent,
        }
    };

    {
        let (cd, kl, cd_pid, kl_pid) = services.status_parts();
        if let Ok(mut st) = api_state.lock() {
            st.set_runtime_status(cd, kl, cd_pid, kl_pid);
        }
    }

    if !args.skip_metrics {
        let token = resolve_metrics_token(&args, &volume);
        let tls = resolve_tls(&args);
        if let Err(err) = start_metrics_thread(api_state.clone(), &args.metrics_listen, token, tls) {
            warn!(error = %err, "metrics endpoint failed to start");
        }
    }

    init_loki_cli(args.loki_url.clone(), args.loki_token.clone());
    apply_loki_push(cfg.as_ref(), &volume.root);
    init_prom_push_cli(args.prometheus_push_url.clone(), args.loki_url.clone());
    apply_prom_push(cfg.as_ref(), api_state.clone());

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
        if let Some(ga) = services.guest_agent.take() {
            ga.stop();
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
    if let Err(err) = reaper::supervise(cfg, services, api_state) {
        warn!(error = %err, "supervise loop exited");
    }
    // Keep dashboard thread alive for the process lifetime.
    drop(_dashboard);
    Ok(())
}

fn start_api_thread(state: SharedState, listen: &str, tls: Option<TlsPaths>) -> Result<()> {
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

fn start_metrics_thread(
    state: SharedState,
    listen: &str,
    bearer_token: Option<String>,
    tls: Option<TlsPaths>,
) -> Result<()> {
    let addr: SocketAddr = listen
        .parse()
        .with_context(|| format!("invalid --metrics-listen {listen}"))?;
    let auth = match (tls.is_some(), bearer_token.is_some()) {
        (true, true) => "mtls+bearer",
        (true, false) => "mtls",
        (false, true) => "bearer",
        (false, false) => "none",
    };
    thread::Builder::new()
        .name("pertisk-metrics".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    warn!(error = %err, "failed to build metrics runtime");
                    return;
                }
            };
            if let Err(err) =
                rt.block_on(pertisk_api::serve_metrics(state, addr, bearer_token, tls))
            {
                warn!(error = %err, "metrics endpoint stopped");
            }
        })?;
    info!(%addr, auth, "metrics endpoint starting");
    Ok(())
}

fn resolve_metrics_token(args: &Args, volume: &StateVolume) -> Option<String> {
    if let Some(ref t) = args.metrics_token {
        let t = t.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    let path = volume.root.join("secrets/metrics.token");
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            let s = s.trim().to_string();
            if s.is_empty() {
                None
            } else {
                info!(path = %path.display(), "loaded metrics bearer token from STATE");
                Some(s)
            }
        }
        Err(_) => None,
    }
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
    let ring = log_ring().clone();
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(ring.make_writer())
        // Escape codes in the ring survive into the dashboard as literal
        // "[32m" text on Proxmox Serial.
        .with_ansi(false)
        .compact()
        .init();
}

fn load_early_config(args: &Args) -> Result<Option<MachineConfig>> {
    if let Some(ref path) = args.config {
        return Ok(Some(read_config(path)?));
    }
    // Do NOT read /system/state/config.yaml here — that path is the initramfs
    // seed until the STATE partition is mounted, and using it early makes apply
    // look like it "didn't stick" across reboot when STATE was ephemeral.
    for candidate in [PathBuf::from("/config.yaml")] {
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

    // Cloud / golden images already have GPT+STATE. Never wipe them from the
    // early initramfs config (which historically shipped wipe:true for lab
    // installs and raced ahead of the mounted STATE config).
    if layout_present() {
        if install.wipe {
            warn!(
                disk = %install.disk,
                "install.wipe ignored; Pertisk layout already present"
            );
        } else {
            info!(disk = %install.disk, "Pertisk layout present; skipping install");
        }
        return Ok(());
    }

    if args.state_dir.is_some() {
        info!("--state-dir set; skipping disk install");
        return Ok(());
    }

    let disk = PathBuf::from(&install.disk);
    if !disk.exists() {
        warn!(
            disk = %disk.display(),
            "install disk missing; skipping (cloud images use scsi /dev/sda, not virtio /dev/vda)"
        );
        return Ok(());
    }

    #[cfg(not(target_os = "linux"))]
    {
        info!(disk = %install.disk, "install planned (not Linux); skipping");
        let _ = install;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        use pertisk_disk::{install_disk, InstallOptions};

        let seed = args.config.clone().filter(|p| p.exists()).or_else(|| {
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
            disk,
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

    let assets =
        BootAssets::from_installer_dir(&installer).context("resolve installer boot assets")?;
    let esp_vol = prepare_esp()
        .context("mount EFI after install")?
        .ok_or_else(|| anyhow::anyhow!("EFI partition not found after install"))?;
    let esp = EspPaths { root: esp_vol.root };
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
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let cfg = MachineConfig::from_yaml(&raw)?;
    info!(path = %path.display(), "config loaded");
    Ok(cfg)
}
