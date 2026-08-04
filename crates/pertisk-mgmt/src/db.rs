use std::path::Path;

use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

pub async fn connect(path: &Path) -> anyhow::Result<SqlitePool> {
    let url = format!("sqlite:{}?mode=rwc", path.display());
    let opts = SqliteConnectOptions::from_str(&url)
        .context("sqlite url")?
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await
        .context("connect sqlite")?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY NOT NULL,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT,
            role TEXT NOT NULL DEFAULT 'viewer',
            auth0_sub TEXT UNIQUE,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS audit_log (
            id TEXT PRIMARY KEY NOT NULL,
            user_id TEXT,
            action TEXT NOT NULL,
            resource TEXT,
            detail TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS providers (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL DEFAULT 'proxmox',
            url TEXT NOT NULL,
            token_id TEXT NOT NULL,
            token_secret_enc TEXT NOT NULL,
            node TEXT NOT NULL,
            storage TEXT NOT NULL,
            bridge TEXT NOT NULL DEFAULT 'vmbr0',
            insecure INTEGER NOT NULL DEFAULT 0,
            defaults_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS clusters (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL UNIQUE,
            provider_id TEXT NOT NULL REFERENCES providers(id),
            status TEXT NOT NULL DEFAULT 'pending',
            controlplanes INTEGER NOT NULL DEFAULT 1,
            workers INTEGER NOT NULL DEFAULT 1,
            vip TEXT,
            vip6 TEXT,
            cni TEXT NOT NULL DEFAULT 'cilium',
            k8s_version TEXT NOT NULL DEFAULT 'v1.36.3',
            cp_memory INTEGER NOT NULL DEFAULT 4096,
            cp_cores INTEGER NOT NULL DEFAULT 2,
            cp_disk_gb INTEGER NOT NULL DEFAULT 50,
            worker_memory INTEGER NOT NULL DEFAULT 8192,
            worker_cores INTEGER NOT NULL DEFAULT 4,
            worker_disk_gb INTEGER NOT NULL DEFAULT 75,
            cp_vmid INTEGER,
            endpoint TEXT,
            kubeconfig_path TEXT,
            error TEXT,
            network_mode TEXT NOT NULL DEFAULT 'ipv4',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS nodes (
            id TEXT PRIMARY KEY NOT NULL,
            cluster_id TEXT NOT NULL REFERENCES clusters(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            role TEXT NOT NULL,
            vmid INTEGER,
            ip TEXT,
            ip6 TEXT,
            k8s_version TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(cluster_id, name)
        );

        CREATE TABLE IF NOT EXISTS jobs (
            id TEXT PRIMARY KEY NOT NULL,
            cluster_id TEXT REFERENCES clusters(id) ON DELETE SET NULL,
            kind TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'queued',
            payload_json TEXT NOT NULL DEFAULT '{}',
            log_path TEXT,
            error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            finished_at TEXT
        );
        "#,
    )
    .execute(pool)
    .await
    .context("migrate")?;

    // Additive columns for existing DBs (ignore if already present).
    let _ = sqlx::query(
        "ALTER TABLE clusters ADD COLUMN network_mode TEXT NOT NULL DEFAULT 'ipv4'",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN ip6 TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN k8s_version TEXT")
        .execute(pool)
        .await;

    Ok(())
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
