//! `pertiskctl` — Talos-shaped node + cluster management CLI.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pertisk_bootstrap::{
    gen_config, patch_worker_ca, sanitize_kubeconfig, write_gen_config, DEFAULT_K8S_VERSION,
    DEFAULT_POD_SUBNET, DEFAULT_SERVICE_SUBNET,
};
use pertisk_proto::machine_service_client::MachineServiceClient;
use pertisk_proto::{
    ApplyConfigurationRequest, BootstrapRequest, HealthRequest, KubeconfigRequest, LogsRequest,
    MarkBootGoodRequest, RebootRequest, ServiceListRequest, ShutdownRequest, UpgradeRequest,
    UpgradeStatusRequest, VersionRequest,
};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

#[derive(Parser)]
#[command(name = "pertiskctl", about = "Pertisk KOS management CLI (Talos-shaped)")]
struct Cli {
    /// gRPC endpoint (host:port).
    #[arg(
        long,
        short = 'e',
        env = "PERTISK_ENDPOINTS",
        default_value = "127.0.0.1:50000"
    )]
    endpoints: String,

    /// CA certificate (PEM) for mTLS.
    #[arg(long, env = "PERTISK_TLS_CA")]
    ca: Option<PathBuf>,

    /// Client certificate (PEM).
    #[arg(long, env = "PERTISK_TLS_CERT")]
    cert: Option<PathBuf>,

    /// Client private key (PEM).
    #[arg(long, env = "PERTISK_TLS_KEY")]
    key: Option<PathBuf>,

    /// TLS server name / SNI (must match certificate).
    #[arg(long, default_value = "pertiskd")]
    tls_server_name: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Version,
    Health,
    Services,
    Validate {
        #[arg(short = 'f', long)]
        file: PathBuf,
    },
    Apply {
        #[arg(short = 'f', long)]
        file: PathBuf,
    },
    /// Generate machine configs (like `talosctl gen`).
    Gen {
        #[command(subcommand)]
        command: GenCommands,
    },
    /// Alias for `gen config`.
    #[command(name = "gen-config", hide = true)]
    GenConfig {
        /// Cluster name (DNS-1123 label).
        cluster_name: String,
        /// Kubernetes API endpoint, e.g. https://10.1.1.210:6443
        endpoint: String,
        #[arg(short = 'o', long, default_value = ".")]
        output: PathBuf,
        /// Kubernetes version written into machine configs (e.g. v1.36.3).
        #[arg(short = 'k', long, default_value = DEFAULT_K8S_VERSION)]
        kubernetes_version: String,
        #[arg(long, default_value = DEFAULT_POD_SUBNET)]
        pod_subnet: String,
        #[arg(long, default_value = DEFAULT_SERVICE_SUBNET)]
        service_subnet: String,
    },
    /// Bootstrap the first control-plane (PKI + static pods).
    Bootstrap {
        /// Optional advertise address (node IPv4).
        #[arg(long)]
        advertise_address: Option<String>,
    },
    /// Fetch admin kubeconfig from a bootstrapped control-plane.
    Kubeconfig {
        #[arg(short = 'f', long, default_value = "admin.conf")]
        file: PathBuf,
    },
    /// Patch worker.yaml with the cluster CA from a bootstrapped CP.
    JoinConfig {
        /// Worker machine config to patch (must already contain token + endpoint).
        #[arg(short = 'f', long)]
        file: PathBuf,
    },
    Reboot,
    Shutdown,
    Upgrade {
        #[arg(long)]
        bundle: String,
        #[arg(long, default_value_t = false)]
        reboot: bool,
    },
    UpgradeStatus,
    MarkBootGood,
    Logs {
        #[arg(default_value = "pertiskd")]
        service: String,
        #[arg(long, short = 'n', default_value_t = 100)]
        tail: u32,
    },
}

#[derive(Subcommand)]
enum GenCommands {
    /// Generate controlplane.yaml + worker.yaml (like `talosctl gen config`).
    Config {
        /// Cluster name (DNS-1123 label).
        cluster_name: String,
        /// Kubernetes API endpoint, e.g. https://10.1.1.210:6443
        endpoint: String,
        #[arg(short = 'o', long, default_value = ".")]
        output: PathBuf,
        /// Kubernetes version written into machine configs (e.g. v1.36.3).
        #[arg(short = 'k', long, default_value = DEFAULT_K8S_VERSION)]
        kubernetes_version: String,
        #[arg(long, default_value = DEFAULT_POD_SUBNET)]
        pod_subnet: String,
        #[arg(long, default_value = DEFAULT_SERVICE_SUBNET)]
        service_subnet: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Version => {
            println!(
                "pertiskctl {}",
                option_env!("PERTISK_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
            );
            match connect(&cli).await {
                Ok(mut client) => {
                    let resp = client.version(VersionRequest {}).await?.into_inner();
                    println!(
                        "node {} (api {} / {})",
                        resp.version, resp.api_version, resp.platform
                    );
                }
                Err(err) => {
                    println!("node: unreachable ({err})");
                    println!("api: v1alpha1");
                }
            }
        }
        Commands::Health => {
            let mut client = connect(&cli).await?;
            let resp = client.health(HealthRequest {}).await?.into_inner();
            println!(
                "ready={} containerd={} kubelet={} — {}",
                resp.ready, resp.containerd, resp.kubelet, resp.message
            );
        }
        Commands::Services => {
            let mut client = connect(&cli).await?;
            let resp = client
                .service_list(ServiceListRequest {})
                .await?
                .into_inner();
            for svc in resp.services {
                if svc.pid > 0 {
                    println!("{:<12} {:<8} pid={}", svc.name, svc.state, svc.pid);
                } else {
                    println!("{:<12} {}", svc.name, svc.state);
                }
            }
        }
        Commands::Validate { ref file } => {
            let yaml = std::fs::read_to_string(file)
                .with_context(|| format!("read {}", file.display()))?;
            let mut client = connect(&cli).await?;
            let resp = client
                .validate_configuration(ApplyConfigurationRequest { yaml })
                .await?
                .into_inner();
            if resp.ok {
                println!("ok: {}", resp.message);
            } else {
                anyhow::bail!("invalid: {}", resp.message);
            }
        }
        Commands::Apply { ref file } => {
            let yaml = std::fs::read_to_string(file)
                .with_context(|| format!("read {}", file.display()))?;
            let mut client = connect(&cli).await?;
            let resp = client
                .apply_configuration(ApplyConfigurationRequest { yaml })
                .await?
                .into_inner();
            if resp.ok {
                println!("applied → {} ({})", resp.path, resp.message);
            } else {
                anyhow::bail!("apply failed: {}", resp.message);
            }
        }
        Commands::Gen {
            command:
                GenCommands::Config {
                    ref cluster_name,
                    ref endpoint,
                    ref output,
                    ref kubernetes_version,
                    ref pod_subnet,
                    ref service_subnet,
                },
        }
        | Commands::GenConfig {
            ref cluster_name,
            ref endpoint,
            ref output,
            ref kubernetes_version,
            ref pod_subnet,
            ref service_subnet,
        } => {
            let gen = gen_config(
                cluster_name,
                endpoint,
                kubernetes_version,
                pod_subnet,
                service_subnet,
            )?;
            write_gen_config(output, &gen)?;
            println!(
                "wrote {}/controlplane.yaml + worker.yaml (token {})",
                output.display(),
                gen.token
            );
        }
        Commands::Bootstrap {
            ref advertise_address,
        } => {
            let mut client = connect(&cli).await?;
            let resp = client
                .bootstrap(BootstrapRequest {
                    advertise_address: advertise_address.clone().unwrap_or_default(),
                })
                .await?
                .into_inner();
            if resp.ok {
                println!(
                    "bootstrap ok already={} — {}",
                    resp.already_bootstrapped, resp.message
                );
            } else {
                anyhow::bail!("{}", resp.message);
            }
        }
        Commands::Kubeconfig { ref file } => {
            let mut client = connect(&cli).await?;
            let resp = client.kubeconfig(KubeconfigRequest {}).await?.into_inner();
            let kc = sanitize_kubeconfig(&resp.kubeconfig);
            std::fs::write(file, &kc).with_context(|| format!("write {}", file.display()))?;
            println!("wrote {}", file.display());
        }
        Commands::JoinConfig { ref file } => {
            let mut client = connect(&cli).await?;
            let resp = client.kubeconfig(KubeconfigRequest {}).await?.into_inner();
            let kc = sanitize_kubeconfig(&resp.kubeconfig);
            // Prefer ca.crt from node STATE via a second call path: parse from kubeconfig CA data
            // or use BootstrapResult — here decode certificate-authority-data if present.
            let ca = extract_ca_from_kubeconfig(&kc)
                .context("CA missing from kubeconfig; is the CP bootstrapped?")?;
            let yaml = std::fs::read_to_string(file)
                .with_context(|| format!("read {}", file.display()))?;
            let patched = patch_worker_ca(&yaml, &ca)?;
            std::fs::write(file, patched).with_context(|| format!("write {}", file.display()))?;
            println!("patched CA into {}", file.display());
        }
        Commands::Reboot => {
            let mut client = connect(&cli).await?;
            let resp = client
                .reboot(RebootRequest { graceful: true })
                .await?
                .into_inner();
            println!("reboot: {} — {}", resp.accepted, resp.message);
        }
        Commands::Shutdown => {
            let mut client = connect(&cli).await?;
            let resp = client
                .shutdown(ShutdownRequest { graceful: true })
                .await?
                .into_inner();
            println!("shutdown: {} — {}", resp.accepted, resp.message);
        }
        Commands::Upgrade { ref bundle, reboot } => {
            let mut client = connect(&cli).await?;
            let resp = client
                .upgrade(UpgradeRequest {
                    bundle_path: bundle.clone(),
                    reboot,
                })
                .await?
                .into_inner();
            if resp.ok {
                println!(
                    "upgrade ok slot={} version={} — {}",
                    resp.target_slot, resp.version, resp.message
                );
            } else {
                anyhow::bail!("{}", resp.message);
            }
        }
        Commands::UpgradeStatus => {
            let mut client = connect(&cli).await?;
            let resp = client
                .upgrade_status(UpgradeStatusRequest {})
                .await?
                .into_inner();
            println!(
                "active={} next={} previous_good={} boot_ok={} attempts={} version={} pending={}",
                resp.active_slot,
                resp.next_slot,
                resp.previous_good,
                resp.boot_ok,
                resp.boot_attempts,
                resp.active_version,
                resp.pending_version
            );
        }
        Commands::MarkBootGood => {
            let mut client = connect(&cli).await?;
            let resp = client
                .mark_boot_good(MarkBootGoodRequest {})
                .await?
                .into_inner();
            println!(
                "mark-boot-good: ok={} slot={} — {}",
                resp.ok, resp.active_slot, resp.message
            );
        }
        Commands::Logs { ref service, tail } => {
            let mut client = connect(&cli).await?;
            let resp = client
                .logs(LogsRequest {
                    service: service.clone(),
                    tail_lines: tail,
                })
                .await?
                .into_inner();
            eprintln!("# {} from {}", resp.service, resp.source);
            for line in resp.lines {
                println!("{line}");
            }
        }
    }
    Ok(())
}

fn extract_ca_from_kubeconfig(kc: &str) -> Option<String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    for line in kc.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("certificate-authority-data:") {
            let b64 = rest.trim();
            if let Ok(bytes) = B64.decode(b64) {
                return String::from_utf8(bytes).ok();
            }
        }
    }
    None
}

async fn connect(cli: &Cli) -> Result<MachineServiceClient<Channel>> {
    match (&cli.ca, &cli.cert, &cli.key) {
        (Some(ca), Some(cert), Some(key)) => {
            let ca_pem = std::fs::read(ca).with_context(|| format!("read {}", ca.display()))?;
            let cert_pem =
                std::fs::read(cert).with_context(|| format!("read {}", cert.display()))?;
            let key_pem = std::fs::read(key).with_context(|| format!("read {}", key.display()))?;
            let tls = ClientTlsConfig::new()
                .domain_name(cli.tls_server_name.clone())
                .ca_certificate(Certificate::from_pem(ca_pem))
                .identity(Identity::from_pem(cert_pem, key_pem));
            let endpoint = format!("https://{}", cli.endpoints);
            let channel = Channel::from_shared(endpoint.clone())?
                .tls_config(tls)?
                .connect()
                .await
                .with_context(|| format!("connect {endpoint}"))?;
            Ok(MachineServiceClient::new(channel))
        }
        (None, None, None) => {
            let endpoint =
                if cli.endpoints.starts_with("http://") || cli.endpoints.starts_with("https://") {
                    cli.endpoints.clone()
                } else {
                    format!("http://{}", cli.endpoints)
                };
            let client = MachineServiceClient::connect(endpoint.clone())
                .await
                .with_context(|| format!("connect {endpoint}"))?;
            Ok(client)
        }
        _ => anyhow::bail!("mTLS requires --ca, --cert, and --key together"),
    }
}
