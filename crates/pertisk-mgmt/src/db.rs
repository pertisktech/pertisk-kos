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
            email TEXT,
            disabled INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT
        );

        CREATE TABLE IF NOT EXISTS password_reset_tokens (
            id TEXT PRIMARY KEY NOT NULL,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            token_hash TEXT NOT NULL UNIQUE,
            expires_at TEXT NOT NULL,
            used_at TEXT,
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
            arch TEXT NOT NULL DEFAULT 'amd64',
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
            max_pods INTEGER NOT NULL DEFAULT 250,
            arch TEXT NOT NULL DEFAULT 'amd64',
            pod_subnet TEXT NOT NULL DEFAULT '10.244.0.0/16',
            service_subnet TEXT NOT NULL DEFAULT '10.96.0.0/12',
            pod_subnet_ipv6 TEXT,
            service_subnet_ipv6 TEXT,
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
            os_version TEXT,
            kernel_version TEXT,
            container_runtime TEXT,
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

        CREATE TABLE IF NOT EXISTS config_templates (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL UNIQUE,
            description TEXT NOT NULL DEFAULT '',
            role TEXT NOT NULL DEFAULT 'any',
            yaml TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS os_packages (
            id TEXT PRIMARY KEY NOT NULL,
            version TEXT NOT NULL,
            arch TEXT NOT NULL DEFAULT 'amd64',
            path TEXT NOT NULL,
            size_bytes INTEGER NOT NULL DEFAULT 0,
            has_trust_pk INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(version, arch)
        );

        CREATE TABLE IF NOT EXISTS join_tokens (
            id TEXT PRIMARY KEY NOT NULL,
            cluster_id TEXT NOT NULL REFERENCES clusters(id) ON DELETE CASCADE,
            role TEXT NOT NULL DEFAULT 'worker',
            label TEXT NOT NULL DEFAULT '',
            token TEXT NOT NULL,
            endpoint TEXT NOT NULL,
            ca_pem TEXT,
            expires_at TEXT,
            created_by TEXT,
            created_at TEXT NOT NULL,
            revoked_at TEXT
        );
        "#,
    )
    .execute(pool)
    .await
    .context("migrate")?;

    // Additive columns for existing DBs (ignore if already present).
    let _ =
        sqlx::query("ALTER TABLE clusters ADD COLUMN network_mode TEXT NOT NULL DEFAULT 'ipv4'")
            .execute(pool)
            .await;
    let _ = sqlx::query("ALTER TABLE clusters ADD COLUMN max_pods INTEGER NOT NULL DEFAULT 250")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE clusters ADD COLUMN arch TEXT NOT NULL DEFAULT 'amd64'")
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "ALTER TABLE clusters ADD COLUMN pod_subnet TEXT NOT NULL DEFAULT '10.244.0.0/16'",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "ALTER TABLE clusters ADD COLUMN service_subnet TEXT NOT NULL DEFAULT '10.96.0.0/12'",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query("ALTER TABLE clusters ADD COLUMN pod_subnet_ipv6 TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE clusters ADD COLUMN service_subnet_ipv6 TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE providers ADD COLUMN arch TEXT NOT NULL DEFAULT 'amd64'")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN ip6 TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN k8s_version TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN memory INTEGER")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN cores INTEGER")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN disk_gb INTEGER")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN ak_public_b64 TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN ak_enrolled_at TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN ek_fingerprint TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN source TEXT NOT NULL DEFAULT 'proxmox'")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN os_version TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN kernel_version TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN container_runtime TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS os_packages (
            id TEXT PRIMARY KEY NOT NULL,
            version TEXT NOT NULL,
            arch TEXT NOT NULL DEFAULT 'amd64',
            path TEXT NOT NULL,
            size_bytes INTEGER NOT NULL DEFAULT 0,
            has_trust_pk INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(version, arch)
        )"#,
    )
    .execute(pool)
    .await;
    let _ = sqlx::query("ALTER TABLE users ADD COLUMN email TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE users ADD COLUMN disabled INTEGER NOT NULL DEFAULT 0")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE users ADD COLUMN updated_at TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS password_reset_tokens (
            id TEXT PRIMARY KEY NOT NULL,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            token_hash TEXT NOT NULL UNIQUE,
            expires_at TEXT NOT NULL,
            used_at TEXT,
            created_at TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await;

    // Backfill node hardware from cluster role defaults when unset.
    let _ = sqlx::query(
        r#"UPDATE nodes SET
             memory = COALESCE(nodes.memory, CASE WHEN nodes.role = 'controlplane' THEN c.cp_memory ELSE c.worker_memory END),
             cores = COALESCE(nodes.cores, CASE WHEN nodes.role = 'controlplane' THEN c.cp_cores ELSE c.worker_cores END),
             disk_gb = COALESCE(nodes.disk_gb, CASE WHEN nodes.role = 'controlplane' THEN c.cp_disk_gb ELSE c.worker_disk_gb END)
           FROM clusters c
           WHERE nodes.cluster_id = c.id
             AND (nodes.memory IS NULL OR nodes.cores IS NULL OR nodes.disk_gb IS NULL)"#,
    )
    .execute(pool)
    .await;
    // SQLite may not support UPDATE…FROM; fall back to two role-specific updates.
    let _ = sqlx::query(
        r#"UPDATE nodes SET
             memory = COALESCE(memory, (SELECT cp_memory FROM clusters WHERE id = nodes.cluster_id)),
             cores = COALESCE(cores, (SELECT cp_cores FROM clusters WHERE id = nodes.cluster_id)),
             disk_gb = COALESCE(disk_gb, (SELECT cp_disk_gb FROM clusters WHERE id = nodes.cluster_id))
           WHERE role = 'controlplane'
             AND (memory IS NULL OR cores IS NULL OR disk_gb IS NULL)"#,
    )
    .execute(pool)
    .await;
    let _ = sqlx::query(
        r#"UPDATE nodes SET
             memory = COALESCE(memory, (SELECT worker_memory FROM clusters WHERE id = nodes.cluster_id)),
             cores = COALESCE(cores, (SELECT worker_cores FROM clusters WHERE id = nodes.cluster_id)),
             disk_gb = COALESCE(disk_gb, (SELECT worker_disk_gb FROM clusters WHERE id = nodes.cluster_id))
           WHERE role != 'controlplane'
             AND (memory IS NULL OR cores IS NULL OR disk_gb IS NULL)"#,
    )
    .execute(pool)
    .await;

    Ok(())
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
