//! `pertiskctl` — node management CLI.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pertisk_proto::machine_service_client::MachineServiceClient;
use pertisk_proto::{
    ApplyConfigurationRequest, HealthRequest, MarkBootGoodRequest, RebootRequest,
    ServiceListRequest, ShutdownRequest, UpgradeRequest, UpgradeStatusRequest, VersionRequest,
};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

#[derive(Parser)]
#[command(name = "pertiskctl", about = "Pertisk KOS management CLI")]
struct Cli {
    /// gRPC endpoint (host:port).
    #[arg(long, short = 'e', env = "PERTISK_ENDPOINTS", default_value = "127.0.0.1:50000")]
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
    Reboot,
    Shutdown,
    /// Stage a signed OS bundle onto the inactive A/B slot.
    Upgrade {
        /// Bundle directory on the node (absolute path).
        #[arg(long)]
        bundle: String,
        /// Reboot after staging.
        #[arg(long, default_value_t = false)]
        reboot: bool,
    },
    /// Show A/B boot / upgrade metadata.
    UpgradeStatus,
    /// Mark the current boot as healthy (cancel auto-rollback).
    MarkBootGood,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Version => {
            println!("pertiskctl {}", env!("CARGO_PKG_VERSION"));
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
            let resp = client.service_list(ServiceListRequest {}).await?.into_inner();
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
    }
    Ok(())
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
