//! Pertisk management control plane — axum API + embedded React UI.

mod auth;
mod cluster_availability;
mod cluster_resources;
mod cluster_versions;
mod config;
mod crypto;
mod db;
mod error;
mod events;
mod jobs;
mod k8s;
mod kubeconfig;
mod node_attestation;
mod node_availability;
mod node_status;
mod node_sync;
mod nutanix;
mod os_upgrade;
mod proxmox;
mod rbac;
mod vsphere;
mod routes;
mod state;
mod static_files;

use anyhow::Context;
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

#[derive(Debug, Parser)]
#[command(name = "pertisk-mgmt", about = "Pertisk cluster management (API + UI)")]
struct Args {
    /// Listen address (single port for API + UI).
    #[arg(long, env = "MGMT_LISTEN", default_value = "0.0.0.0:8080")]
    listen: SocketAddr,

    /// SQLite database path.
    #[arg(long, env = "MGMT_DB", default_value = "./data/mgmt.db")]
    db: PathBuf,

    /// Directory for job logs and kubeconfigs.
    #[arg(long, env = "MGMT_DATA_DIR", default_value = "./data")]
    data_dir: PathBuf,

    /// Path to proxmox-lab-up.sh (Phase 3 job runner).
    #[arg(long, env = "MGMT_LAB_UP", default_value = "./scripts/proxmox-lab-up.sh")]
    lab_up: PathBuf,

    /// Path to pertiskctl binary.
    #[arg(long, env = "MGMT_PERTISKCTL", default_value = "./out/bin/pertiskctl")]
    pertiskctl: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("pertisk_mgmt=info,tower_http=info,sqlx=warn")
        }))
        .init();

    let args = Args::parse();
    let cfg = Config::from_env(args.listen, args.db, args.data_dir, args.lab_up, args.pertiskctl)?;

    std::fs::create_dir_all(&cfg.data_dir).context("create data dir")?;
    if let Some(parent) = cfg.db.parent() {
        std::fs::create_dir_all(parent).context("create db parent")?;
    }

    let pool = db::connect(&cfg.db).await?;
    db::migrate(&pool).await?;
    auth::seed_admin(&pool, &cfg).await?;

    let state = AppState::new(cfg.clone(), pool);
    jobs::spawn_worker(state.clone());

    let app = routes::router(state).layer(TraceLayer::new_for_http());

    tracing::info!(listen = %cfg.listen, "pertisk-mgmt listening (API + UI)");
    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .with_context(|| format!("bind {}", cfg.listen))?;
    axum::serve(listener, app).await.context("serve")?;
    Ok(())
}
