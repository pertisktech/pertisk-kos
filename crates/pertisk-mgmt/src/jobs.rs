use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use crate::crypto;
use crate::db;
use crate::state::AppState;

/// Background worker that drains the jobs table.
pub fn spawn_worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = tick(&state).await {
                tracing::error!(error = %e, "job worker tick failed");
            }
            tokio::select! {
                _ = state.inner.job_notify.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
            }
        }
    });
}

async fn tick(state: &AppState) -> anyhow::Result<()> {
    // Prefer deletes so failed clusters can be cleaned up while creates run/queue.
    let row = sqlx::query_as::<_, (String, Option<String>, String, String, Option<String>)>(
        r#"SELECT id, cluster_id, kind, payload_json, log_path FROM jobs
           WHERE status = 'queued'
           ORDER BY CASE WHEN kind = 'delete_cluster' THEN 0 ELSE 1 END, created_at ASC
           LIMIT 1"#,
    )
    .fetch_optional(state.pool())
    .await?;

    let Some((id, cluster_id, kind, payload, log_path)) = row else {
        return Ok(());
    };

    // Skip create/add if cluster is already deleting / gone.
    if matches!(
        kind.as_str(),
        "create_cluster" | "add_node" | "upgrade_cluster" | "update_config" | "remove_node"
    ) {
        if let Some(cid) = &cluster_id {
            let st: Option<String> =
                sqlx::query_scalar("SELECT status FROM clusters WHERE id = ?")
                    .bind(cid)
                    .fetch_optional(state.pool())
                    .await?;
            if st.as_deref() == Some("deleting") || st.is_none() {
                let now = db::now_rfc3339();
                sqlx::query(
                    "UPDATE jobs SET status = 'cancelled', error = 'cluster deleting or removed', updated_at = ?, finished_at = ? WHERE id = ?",
                )
                .bind(&now)
                .bind(&now)
                .bind(&id)
                .execute(state.pool())
                .await?;
                return Ok(());
            }
        }
    }

    let now = db::now_rfc3339();
    sqlx::query("UPDATE jobs SET status = 'running', updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&id)
        .execute(state.pool())
        .await?;

    if let Some(cid) = &cluster_id {
        if kind != "delete_cluster" {
            let _ = sqlx::query(
                "UPDATE clusters SET status = 'provisioning', updated_at = ?, error = NULL WHERE id = ?",
            )
            .bind(&now)
            .bind(cid)
            .execute(state.pool())
            .await;
        }
    }

    let log_file = log_path.unwrap_or_else(|| {
        let p = state.cfg().jobs_dir().join(format!("{id}.log"));
        p.to_string_lossy().into_owned()
    });
    std::fs::create_dir_all(state.cfg().jobs_dir())?;
    sqlx::query("UPDATE jobs SET log_path = ?, updated_at = ? WHERE id = ?")
        .bind(&log_file)
        .bind(&now)
        .bind(&id)
        .execute(state.pool())
        .await?;

    let result = match kind.as_str() {
        "create_cluster" => {
            run_create_cluster(state, &id, cluster_id.as_deref(), &payload, &log_file).await
        }
        "delete_cluster" => {
            run_delete_cluster(state, cluster_id.as_deref(), &payload, &log_file).await
        }
        "add_node" => run_add_node(state, cluster_id.as_deref(), &payload, &log_file).await,
        "remove_node" => run_remove_node(state, cluster_id.as_deref(), &payload, &log_file).await,
        "upgrade_cluster" => run_upgrade(state, cluster_id.as_deref(), &payload, &log_file).await,
        "update_config" => {
            run_update_config(state, cluster_id.as_deref(), &payload, &log_file).await
        }
        other => Err(anyhow::anyhow!("unknown job kind: {other}")),
    };

    let now = db::now_rfc3339();
    match result {
        Ok(()) => {
            sqlx::query(
                "UPDATE jobs SET status = 'succeeded', updated_at = ?, finished_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&now)
            .bind(&id)
            .execute(state.pool())
            .await?;
            if let Some(cid) = &cluster_id {
                if kind != "delete_cluster" {
                    let _ = sqlx::query(
                        "UPDATE clusters SET status = 'ready', updated_at = ?, error = NULL WHERE id = ?",
                    )
                    .bind(&now)
                    .bind(cid)
                    .execute(state.pool())
                    .await;
                }
            }
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::error!(job = %id, error = %msg, "job failed");
            append_log(&log_file, &format!("ERROR: {msg}\n"))?;
            sqlx::query(
                "UPDATE jobs SET status = 'failed', error = ?, updated_at = ?, finished_at = ? WHERE id = ?",
            )
            .bind(&msg)
            .bind(&now)
            .bind(&now)
            .bind(&id)
            .execute(state.pool())
            .await?;
            if let Some(cid) = &cluster_id {
                if kind == "delete_cluster" {
                    // Best-effort: still purge DB so UI is not stuck on "deleting".
                    let _ = purge_cluster_db(state, cid).await;
                } else {
                    let _ = sqlx::query(
                        "UPDATE clusters SET status = 'error', error = ?, updated_at = ? WHERE id = ?",
                    )
                    .bind(&msg)
                    .bind(&now)
                    .bind(cid)
                    .execute(state.pool())
                    .await;
                }
            }
        }
    }
    Ok(())
}

fn append_log(path: &str, line: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

async fn run_create_cluster(
    state: &AppState,
    _job_id: &str,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: serde_json::Value = serde_json::from_str(payload)?;

    let cluster = sqlx::query_as::<_, ClusterRow>(
        r#"SELECT id, name, provider_id, controlplanes, workers, vip, vip6, cni, k8s_version,
                  cp_memory, cp_cores, cp_disk_gb, worker_memory, worker_cores, worker_disk_gb, cp_vmid,
                  COALESCE(network_mode, 'ipv4') as network_mode
           FROM clusters WHERE id = ?"#,
    )
    .bind(cid)
    .fetch_one(state.pool())
    .await?;

    let provider = sqlx::query_as::<_, ProviderRow>(
        "SELECT id, url, token_id, token_secret_enc, node, storage, bridge, insecure FROM providers WHERE id = ?",
    )
    .bind(&cluster.provider_id)
    .fetch_one(state.pool())
    .await?;

    let secret = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?;

    let cp_vmid = cluster.cp_vmid.unwrap_or(
        p.get("cp_vmid")
            .and_then(|v| v.as_i64())
            .unwrap_or(210) as i64,
    );

    let kc_dir = state.cfg().kubeconfigs_dir();
    std::fs::create_dir_all(&kc_dir)?;
    let cluster_out = kc_dir.join(&cluster.name);
    std::fs::create_dir_all(&cluster_out)?;

    let mut cmd = Command::new(&state.cfg().lab_up);
    cmd.arg("--skip-build")
        .arg("--cluster")
        .arg(&cluster.name)
        .arg("--prefix")
        .arg(&cluster.name)
        .arg("--controlplanes")
        .arg(cluster.controlplanes.to_string())
        .arg("--workers")
        .arg(cluster.workers.to_string())
        .arg("--cni")
        .arg(&cluster.cni)
        .arg("--cp-vmid")
        .arg(cp_vmid.to_string())
        .arg("--cp-memory")
        .arg(cluster.cp_memory.to_string())
        .arg("--cp-cores")
        .arg(cluster.cp_cores.to_string())
        .arg("--cp-disk-gb")
        .arg(cluster.cp_disk_gb.to_string())
        .arg("--worker-memory")
        .arg(cluster.worker_memory.to_string())
        .arg("--worker-cores")
        .arg(cluster.worker_cores.to_string())
        .arg("--worker-disk-gb")
        .arg(cluster.worker_disk_gb.to_string())
        .arg("--k8s")
        .arg(&cluster.k8s_version)
        .env("PROXMOX_URL", &provider.url)
        .env("PROXMOX_TOKEN_ID", &provider.token_id)
        .env("PROXMOX_TOKEN_SECRET", &secret)
        .env("PROXMOX_NODE", &provider.node)
        .env("PROXMOX_STORAGE", &provider.storage)
        .env("PROXMOX_BRIDGE", &provider.bridge)
        .env("CLUSTER_OUT", cluster_out.display().to_string())
        .env("K8S_VER", &cluster.k8s_version)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if provider.insecure != 0 {
        cmd.env("PROXMOX_INSECURE", "1");
    }

    let mode = cluster.network_mode.to_ascii_lowercase();
    let dual = mode == "dual-stack" || mode == "ipv6";
    if dual {
        cmd.arg("--dual-stack");
    }
    if let Some(vip) = &cluster.vip {
        if !vip.is_empty() && mode != "ipv6" {
            cmd.arg("--vip").arg(vip);
        }
    }
    if let Some(vip6) = &cluster.vip6 {
        if !vip6.is_empty() && mode != "ipv4" {
            cmd.arg("--vip6").arg(vip6);
        }
    }

    append_log(log_path, &format!("$ {:?}\n", cmd.as_std().get_program()))?;
    append_log(
        log_path,
        &format!(
            "create cluster={} cps={} workers={} k8s={} network={} vip={:?} vip6={:?}\n",
            cluster.name,
            cluster.controlplanes,
            cluster.workers,
            cluster.k8s_version,
            mode,
            cluster.vip,
            cluster.vip6
        ),
    )?;

    // If lab-up script missing, simulate for UI/dev
    if !state.cfg().lab_up.exists() {
        append_log(
            log_path,
            "WARNING: lab-up script not found; marking cluster ready (dev stub)\n",
        )?;
        seed_stub_nodes(state, &cluster).await?;
        let now = db::now_rfc3339();
        let endpoint = cluster
            .vip
            .clone()
            .unwrap_or_else(|| "127.0.0.1".into());
        let kc = cluster_out.join("admin.conf");
        std::fs::write(&kc, "# stub kubeconfig\n")?;
        sqlx::query(
            "UPDATE clusters SET status = 'ready', endpoint = ?, kubeconfig_path = ?, cp_vmid = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&endpoint)
        .bind(kc.to_string_lossy().as_ref())
        .bind(cp_vmid)
        .bind(&now)
        .bind(cid)
        .execute(state.pool())
        .await?;
        return Ok(());
    }

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let log_path_out = log_path.to_string();
    let log_path_err = log_path.to_string();

    let out_task = tokio::spawn(async move {
        if let Some(out) = stdout {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = append_log(&log_path_out, &format!("{line}\n"));
            }
        }
    });
    let err_task = tokio::spawn(async move {
        if let Some(err) = stderr {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = append_log(&log_path_err, &format!("{line}\n"));
            }
        }
    });

    let status = child.wait().await?;
    let _ = out_task.await;
    let _ = err_task.await;

    if !status.success() {
        anyhow::bail!("lab-up exited with {status}");
    }

    let kc = resolve_kubeconfig(&cluster_out, &cluster.name, log_path)?;
    let endpoint = cluster
        .vip
        .clone()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let now = db::now_rfc3339();
    sqlx::query(
        "UPDATE clusters SET status = 'ready', endpoint = ?, kubeconfig_path = ?, cp_vmid = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&endpoint)
    .bind(kc.to_string_lossy().as_ref())
    .bind(cp_vmid)
    .bind(&now)
    .bind(cid)
    .execute(state.pool())
    .await?;

    // Upsert node placeholders from counts
    seed_stub_nodes(state, &cluster).await?;
    let _ = crate::node_sync::sync_cluster_nodes(
        state.pool(),
        cid,
        Some(kc.as_path()),
        Some(log_path),
    )
    .await;
    Ok(())
}

/// Prefer CLUSTER_OUT/admin.conf; fall back to repo out/cluster/admin.conf and copy in.
fn resolve_kubeconfig(
    cluster_out: &std::path::Path,
    cluster_name: &str,
    log_path: &str,
) -> anyhow::Result<PathBuf> {
    let dest = cluster_out.join("admin.conf");
    if dest.is_file() {
        rewrite_stored_kubeconfig(&dest, cluster_name)?;
        return Ok(dest);
    }

    let candidates = [
        PathBuf::from("out/cluster/admin.conf"),
        PathBuf::from("./out/cluster/admin.conf"),
        std::env::current_dir()
            .unwrap_or_default()
            .join("out/cluster/admin.conf"),
    ];
    for src in candidates {
        if src.is_file() {
            std::fs::create_dir_all(cluster_out)?;
            std::fs::copy(&src, &dest)?;
            rewrite_stored_kubeconfig(&dest, cluster_name)?;
            append_log(
                log_path,
                &format!(
                    "copied kubeconfig {} → {} (cluster={})\n",
                    src.display(),
                    dest.display(),
                    cluster_name
                ),
            )?;
            return Ok(dest);
        }
    }
    anyhow::bail!(
        "admin.conf not found at {} or out/cluster/admin.conf after lab-up",
        dest.display()
    );
}

fn rewrite_stored_kubeconfig(path: &std::path::Path, cluster_name: &str) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path)?;
    let rewritten = crate::kubeconfig::rename_kubeconfig_context(&content, cluster_name);
    if rewritten != content {
        std::fs::write(path, rewritten)?;
    }
    Ok(())
}

async fn seed_stub_nodes(state: &AppState, cluster: &ClusterRow) -> anyhow::Result<()> {
    let now = db::now_rfc3339();
    for i in 1..=cluster.controlplanes {
        let name = format!("{}-cp-{}", cluster.name, i);
        let id = Uuid::new_v4().to_string();
        let vmid = cluster.cp_vmid.map(|v| v + i - 1);
        let _ = sqlx::query(
            r#"INSERT INTO nodes (id, cluster_id, name, role, vmid, k8s_version, status, created_at, updated_at)
               VALUES (?, ?, ?, 'controlplane', ?, ?, 'ready', ?, ?)
               ON CONFLICT(cluster_id, name) DO UPDATE SET
                 vmid = COALESCE(excluded.vmid, nodes.vmid),
                 k8s_version = COALESCE(nodes.k8s_version, excluded.k8s_version),
                 status = 'ready',
                 updated_at = excluded.updated_at"#,
        )
        .bind(&id)
        .bind(&cluster.id)
        .bind(&name)
        .bind(vmid)
        .bind(&cluster.k8s_version)
        .bind(&now)
        .bind(&now)
        .execute(state.pool())
        .await;
    }
    let worker_base = cluster.cp_vmid.unwrap_or(210) + cluster.controlplanes;
    for i in 1..=cluster.workers {
        let name = format!("{}-wk-{}", cluster.name, i);
        let id = Uuid::new_v4().to_string();
        let vmid = worker_base + i - 1;
        let _ = sqlx::query(
            r#"INSERT INTO nodes (id, cluster_id, name, role, vmid, k8s_version, status, created_at, updated_at)
               VALUES (?, ?, ?, 'worker', ?, ?, 'ready', ?, ?)
               ON CONFLICT(cluster_id, name) DO UPDATE SET
                 vmid = COALESCE(excluded.vmid, nodes.vmid),
                 k8s_version = COALESCE(nodes.k8s_version, excluded.k8s_version),
                 status = 'ready',
                 updated_at = excluded.updated_at"#,
        )
        .bind(&id)
        .bind(&cluster.id)
        .bind(&name)
        .bind(vmid)
        .bind(&cluster.k8s_version)
        .bind(&now)
        .bind(&now)
        .execute(state.pool())
        .await;
    }
    Ok(())
}

async fn run_delete_cluster(
    state: &AppState,
    cluster_id: Option<&str>,
    _payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    purge_cluster(state, cid, log_path).await
}

/// Remove cluster from DB (nodes + cluster row). Ignores missing rows.
pub async fn purge_cluster_db(state: &AppState, cid: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM nodes WHERE cluster_id = ?")
        .bind(cid)
        .execute(state.pool())
        .await?;
    sqlx::query("DELETE FROM clusters WHERE id = ?")
        .bind(cid)
        .execute(state.pool())
        .await?;
    Ok(())
}

/// Best-effort Proxmox VM destroy + DB purge. Never fails the DB purge on VM errors.
pub async fn purge_cluster(state: &AppState, cid: &str, log_path: &str) -> anyhow::Result<()> {
    let cluster = sqlx::query_as::<_, (String, String, Option<i64>, i64, i64, String)>(
        "SELECT id, provider_id, cp_vmid, controlplanes, workers, status FROM clusters WHERE id = ?",
    )
    .bind(cid)
    .fetch_optional(state.pool())
    .await?;

    let Some((id, provider_id, cp_vmid, cps, workers, status)) = cluster else {
        append_log(log_path, "cluster already removed\n")?;
        return Ok(());
    };

    append_log(log_path, &format!("purging cluster status={status}\n"))?;

    // Cancel other queued jobs for this cluster.
    let now = db::now_rfc3339();
    let _ = sqlx::query(
        r#"UPDATE jobs SET status = 'cancelled', error = 'superseded by delete', updated_at = ?, finished_at = ?
           WHERE cluster_id = ? AND status = 'queued'"#,
    )
    .bind(&now)
    .bind(&now)
    .bind(&id)
    .execute(state.pool())
    .await;

    // Best-effort Proxmox cleanup (failed creates may have 0 VMs).
    match sqlx::query_as::<_, ProviderRow>(
        "SELECT id, url, token_id, token_secret_enc, node, storage, bridge, insecure FROM providers WHERE id = ?",
    )
    .bind(&provider_id)
    .fetch_optional(state.pool())
    .await
    {
        Ok(Some(provider)) => {
            match crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc) {
                Ok(secret) => {
                    let client = crate::proxmox::ProxmoxClient {
                        url: provider.url,
                        token_id: provider.token_id,
                        token_secret: secret,
                        insecure: provider.insecure != 0,
                    };
                    let nodes = sqlx::query_as::<_, (Option<i64>,)>(
                        "SELECT vmid FROM nodes WHERE cluster_id = ?",
                    )
                    .bind(&id)
                    .fetch_all(state.pool())
                    .await
                    .unwrap_or_default();

                    let mut vmids: Vec<i64> =
                        nodes.into_iter().filter_map(|(v,)| v).collect();
                    // Only guess VMID range if we never recorded nodes (failed mid-create may
                    // still have created some VMs — try the planned range).
                    if vmids.is_empty() {
                        if let Some(base) = cp_vmid {
                            for i in 0..(cps + workers) {
                                vmids.push(base + i);
                            }
                        }
                    }
                    // Dedup
                    vmids.sort_unstable();
                    vmids.dedup();

                    for vmid in vmids {
                        append_log(
                            log_path,
                            &format!("deleting VM {vmid} on {}\n", provider.node),
                        )?;
                        if let Err(e) = client.delete_vm(&provider.node, vmid).await {
                            append_log(log_path, &format!("warn: delete {vmid}: {e}\n"))?;
                        }
                    }
                }
                Err(e) => append_log(log_path, &format!("warn: decrypt secret: {e}\n"))?,
            }
        }
        Ok(None) => append_log(log_path, "warn: provider missing — DB-only delete\n")?,
        Err(e) => append_log(log_path, &format!("warn: provider lookup: {e}\n"))?,
    }

    purge_cluster_db(state, &id).await?;
    append_log(log_path, "cluster deleted\n")?;
    Ok(())
}

/// Cancel queued jobs and purge cluster immediately (used by API force-delete).
pub async fn force_delete_cluster(state: &AppState, cid: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(state.cfg().jobs_dir())?;
    let log_path = state
        .cfg()
        .jobs_dir()
        .join(format!("force-delete-{cid}.log"));
    let log = log_path.to_string_lossy().into_owned();
    purge_cluster(state, cid, &log).await
}

async fn run_add_node(
    state: &AppState,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: serde_json::Value = serde_json::from_str(payload)?;
    let role = p.get("role").and_then(|v| v.as_str()).unwrap_or("worker");
    let now = db::now_rfc3339();

    let (name_prefix, cps, workers, cp_vmid): (String, i64, i64, Option<i64>) =
        sqlx::query_as("SELECT name, controlplanes, workers, cp_vmid FROM clusters WHERE id = ?")
            .bind(cid)
            .fetch_one(state.pool())
            .await?;

    if role == "controlplane" {
        let new_cps = cps + 1;
        if new_cps % 2 == 0 {
            append_log(
                log_path,
                "WARNING: even control-plane count reduces etcd quorum safety\n",
            )?;
        }
        let idx = new_cps;
        let name = format!("{name_prefix}-cp-{idx}");
        let vmid = cp_vmid.map(|b| b + idx - 1);
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO nodes (id, cluster_id, name, role, vmid, status, created_at, updated_at) VALUES (?, ?, ?, 'controlplane', ?, 'ready', ?, ?)",
        )
        .bind(&id)
        .bind(cid)
        .bind(&name)
        .bind(vmid)
        .bind(&now)
        .bind(&now)
        .execute(state.pool())
        .await?;
        sqlx::query("UPDATE clusters SET controlplanes = ?, updated_at = ? WHERE id = ?")
            .bind(new_cps)
            .bind(&now)
            .bind(cid)
            .execute(state.pool())
            .await?;
        append_log(
            log_path,
            &format!("added control plane {name} (join-controlplane via lab tooling)\n"),
        )?;
    } else {
        let new_w = workers + 1;
        let name = format!("{name_prefix}-wk-{new_w}");
        let base = cp_vmid.unwrap_or(210) + cps;
        let vmid = base + new_w - 1;
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO nodes (id, cluster_id, name, role, vmid, status, created_at, updated_at) VALUES (?, ?, ?, 'worker', ?, 'ready', ?, ?)",
        )
        .bind(&id)
        .bind(cid)
        .bind(&name)
        .bind(vmid)
        .bind(&now)
        .bind(&now)
        .execute(state.pool())
        .await?;
        sqlx::query("UPDATE clusters SET workers = ?, updated_at = ? WHERE id = ?")
            .bind(new_w)
            .bind(&now)
            .bind(cid)
            .execute(state.pool())
            .await?;
        append_log(log_path, &format!("added worker {name}\n"))?;
    }
    Ok(())
}

async fn run_remove_node(
    state: &AppState,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: serde_json::Value = serde_json::from_str(payload)?;
    let node_id = p
        .get("node_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("node_id required"))?;

    let node = sqlx::query_as::<_, (String, String, Option<i64>)>(
        "SELECT name, role, vmid FROM nodes WHERE id = ? AND cluster_id = ?",
    )
    .bind(node_id)
    .bind(cid)
    .fetch_optional(state.pool())
    .await?
    .ok_or_else(|| anyhow::anyhow!("node not found"))?;

    let (cps,): (i64,) =
        sqlx::query_as("SELECT controlplanes FROM clusters WHERE id = ?")
            .bind(cid)
            .fetch_one(state.pool())
            .await?;

    if node.1 == "controlplane" {
        let remaining = cps - 1;
        if remaining < 1 {
            anyhow::bail!("cannot remove last control plane");
        }
        // Quorum: majority of original; block if remaining < majority of previous odd set
        let majority = cps / 2 + 1;
        if remaining < majority && cps > 1 {
            anyhow::bail!(
                "removing CP would break etcd quorum (have {cps}, need majority {majority})"
            );
        }
        sqlx::query("UPDATE clusters SET controlplanes = controlplanes - 1, updated_at = ? WHERE id = ?")
            .bind(db::now_rfc3339())
            .bind(cid)
            .execute(state.pool())
            .await?;
    } else {
        let (w,): (i64,) = sqlx::query_as("SELECT workers FROM clusters WHERE id = ?")
            .bind(cid)
            .fetch_one(state.pool())
            .await?;
        sqlx::query("UPDATE clusters SET workers = ?, updated_at = ? WHERE id = ?")
            .bind((w - 1).max(0))
            .bind(db::now_rfc3339())
            .bind(cid)
            .execute(state.pool())
            .await?;
    }

    // Best-effort Proxmox delete
    if let Some(vmid) = node.2 {
        let provider_id: String =
            sqlx::query_scalar("SELECT provider_id FROM clusters WHERE id = ?")
                .bind(cid)
                .fetch_one(state.pool())
                .await?;
        let provider = sqlx::query_as::<_, ProviderRow>(
            "SELECT id, url, token_id, token_secret_enc, node, storage, bridge, insecure FROM providers WHERE id = ?",
        )
        .bind(&provider_id)
        .fetch_one(state.pool())
        .await?;
        let secret = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?;
        let client = crate::proxmox::ProxmoxClient {
            url: provider.url.clone(),
            token_id: provider.token_id,
            token_secret: secret,
            insecure: provider.insecure != 0,
        };
        append_log(log_path, &format!("deleting VM {vmid}\n"))?;
        let _ = client.delete_vm(&provider.node, vmid).await;
    }

    sqlx::query("DELETE FROM nodes WHERE id = ?")
        .bind(node_id)
        .execute(state.pool())
        .await?;
    append_log(log_path, &format!("removed node {}\n", node.0))?;
    Ok(())
}

async fn run_upgrade(
    state: &AppState,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: serde_json::Value = serde_json::from_str(payload)?;
    let version = p
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("latest");

    let cluster = sqlx::query_as::<_, (String, i64, Option<String>)>(
        "SELECT name, controlplanes, kubeconfig_path FROM clusters WHERE id = ?",
    )
    .bind(cid)
    .fetch_one(state.pool())
    .await?;
    let (cluster_name, controlplanes, kc) = cluster;

    let zero_downtime = controlplanes >= 3;
    append_log(
        log_path,
        &format!(
            "rolling upgrade → {version} (kubeadm-shaped: CP one-by-one, then workers)\n"
        ),
    )?;
    if zero_downtime {
        append_log(
            log_path,
            "zero-downtime mode: HA control planes (≥3) — drain → upgrade → wait Ready → uncordon\n",
        )?;
    } else {
        append_log(
            log_path,
            &format!(
                "NOTE: controlplanes={controlplanes} (<3) — API may blip during CP upgrade; use M=3 + VIP for zero-downtime\n"
            ),
        )?;
    }

    // Refresh IPs before drain/upgrade.
    if let Some(path) = kc.as_ref().filter(|s| !s.is_empty()) {
        let _ = crate::node_sync::sync_cluster_nodes(
            state.pool(),
            cid,
            Some(std::path::Path::new(path)),
            Some(log_path),
        )
        .await;
    }

    // Re-read nodes after sync (IPs may have filled in).
    let nodes = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT id, name, role, ip FROM nodes WHERE cluster_id = ? ORDER BY name ASC",
    )
    .bind(cid)
    .fetch_all(state.pool())
    .await?;

    // 1) Control planes first (kubeadm: primary then additional), one at a time.
    let mut cps: Vec<_> = nodes
        .iter()
        .filter(|(_, _, role, _)| role == "controlplane")
        .cloned()
        .collect();
    cps.sort_by(|a, b| a.1.cmp(&b.1));
    for (i, (id, name, _role, ip)) in cps.iter().enumerate() {
        let first = i == 0;
        append_log(
            log_path,
            &format!(
                "==> CP {}/{} {} ({})\n",
                i + 1,
                cps.len(),
                name,
                if first {
                    "primary — upgrade apply"
                } else {
                    "additional — upgrade node"
                }
            ),
        )?;
        upgrade_node_zero_downtime(
            state,
            cid,
            id,
            name,
            "controlplane",
            ip,
            log_path,
            version,
            &kc,
            &cluster_name,
            zero_downtime,
        )
        .await?;
    }

    // 2) Workers one at a time (capacity-safe).
    let mut wks: Vec<_> = nodes
        .iter()
        .filter(|(_, _, role, _)| role == "worker")
        .cloned()
        .collect();
    wks.sort_by(|a, b| a.1.cmp(&b.1));
    for (i, (id, name, _role, ip)) in wks.iter().enumerate() {
        append_log(
            log_path,
            &format!("==> worker {}/{} {}\n", i + 1, wks.len(), name),
        )?;
        upgrade_node_zero_downtime(
            state,
            cid,
            id,
            name,
            "worker",
            ip,
            log_path,
            version,
            &kc,
            &cluster_name,
            true, // always drain workers for workload ZD
        )
        .await?;
    }

    let now = db::now_rfc3339();
    sqlx::query("UPDATE clusters SET k8s_version = ?, updated_at = ? WHERE id = ?")
        .bind(version)
        .bind(&now)
        .bind(cid)
        .execute(state.pool())
        .await?;
    append_log(log_path, "upgrade complete — verify: kubectl get nodes\n")?;
    Ok(())
}

/// kubeadm-shaped per-node upgrade: drain → bump version on-node → wait Ready → uncordon.
async fn upgrade_node_zero_downtime(
    state: &AppState,
    cluster_id: &str,
    id: &str,
    name: &str,
    role: &str,
    ip: &Option<String>,
    log_path: &str,
    version: &str,
    kc: &Option<String>,
    cluster_name: &str,
    do_drain: bool,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE nodes SET status = 'upgrading', updated_at = ? WHERE id = ?")
        .bind(db::now_rfc3339())
        .bind(id)
        .execute(state.pool())
        .await?;

    let kc_path = kc.as_ref().filter(|s| !s.is_empty());
    let want = normalize_k8s_version(version);
    let is_cp = role == "controlplane";

    if do_drain {
        if let Some(kc) = kc_path {
            append_log(log_path, &format!("drain {name}\n"))?;
            let _ = kubectl(
                kc,
                &[
                    "drain",
                    name,
                    "--ignore-daemonsets",
                    "--delete-emptydir-data",
                    "--force",
                    "--grace-period=30",
                    "--timeout=5m",
                ],
                log_path,
            )
            .await;
        } else {
            append_log(log_path, "skip drain (no kubeconfig)\n")?;
        }
    }

    // Persist desired kubernetesVersion via machine config + apply (pertiskd reload).
    // Live upgrade (static-pod tags + kubelet binary) is done by the in-cluster agent below —
    // existing guest images do not yet bump versions on config reload alone.
    if let Some(ip) = ip {
        if state.cfg().pertiskctl.exists() {
            if let Some(cfg_path) = resolve_node_machine_config(state, cluster_name, name) {
                let yaml = std::fs::read_to_string(&cfg_path)?;
                let patched = patch_kubernetes_version(&yaml, &want);
                let tmp = state
                    .cfg()
                    .data_dir
                    .join(format!("upgrade-{}-{}.yaml", id, Uuid::new_v4()));
                std::fs::write(&tmp, &patched)?;
                let _ = std::fs::write(&cfg_path, &patched);
                append_log(
                    log_path,
                    &format!(
                        "apply {} (kubernetesVersion={}) @ {ip}:50000\n",
                        cfg_path.display(),
                        want
                    ),
                )?;
                let apply = Command::new(&state.cfg().pertiskctl)
                    .args([
                        "-e",
                        &format!("{ip}:50000"),
                        "apply",
                        "-f",
                        &tmp.to_string_lossy(),
                    ])
                    .output()
                    .await;
                match apply {
                    Ok(a) => {
                        append_log(log_path, &String::from_utf8_lossy(&a.stdout))?;
                        append_log(log_path, &String::from_utf8_lossy(&a.stderr))?;
                        if !a.status.success() {
                            append_log(log_path, "apply returned non-zero\n")?;
                        }
                    }
                    Err(e) => append_log(log_path, &format!("apply failed: {e}\n"))?,
                }
                let _ = std::fs::remove_file(&tmp);
            } else {
                append_log(
                    log_path,
                    &format!(
                        "no machine config for {name}; skip apply (agent will still upgrade runtime)\n"
                    ),
                )?;
            }
        } else {
            append_log(log_path, "pertiskctl not found; skipping apply\n")?;
        }
    } else {
        append_log(log_path, &format!("no IP for {name}; skip on-node apply\n"))?;
    }

    if let Some(kc) = kc_path {
        let already = node_kubelet_version(kc, name).await;
        if already.as_deref() == Some(want.as_str()) {
            append_log(
                log_path,
                &format!("skip agent — {name} already kubelet={want}\n"),
            )?;
        } else {
            append_log(
                log_path,
                &format!(
                    "upgrade agent on {name} (static pods={}, kubelet → {want})\n",
                    if is_cp { "yes" } else { "n/a" }
                ),
            )?;
            apply_node_version_via_agent(kc, name, &want, is_cp, log_path).await?;
        }

        append_log(log_path, &format!("wait Ready {name}\n"))?;
        let _ = kubectl(
            kc,
            &[
                "wait",
                "--for=condition=Ready",
                &format!("node/{name}"),
                "--timeout=10m",
            ],
            log_path,
        )
        .await;

        append_log(log_path, &format!("wait kubelet {want} on {name}\n"))?;
        wait_node_kubelet_version(kc, name, &want, log_path).await?;

        if do_drain {
            append_log(log_path, &format!("uncordon {name}\n"))?;
            let _ = kubectl(kc, &["uncordon", name], log_path).await;
        }

        let _ = crate::node_sync::sync_cluster_nodes(
            state.pool(),
            cluster_id,
            Some(std::path::Path::new(kc)),
            Some(log_path),
        )
        .await;
    } else {
        append_log(
            log_path,
            "no kubeconfig — cannot run upgrade agent or wait for version\n",
        )?;
    }

    let now = db::now_rfc3339();
    sqlx::query(
        "UPDATE nodes SET status = 'ready', k8s_version = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&want)
    .bind(&now)
    .bind(id)
    .execute(state.pool())
    .await?;
    Ok(())
}

/// Privileged hostPath job: bump CP static-pod images + replace kubelet, then kill kubelet
/// so pertiskd restarts it. Works on existing guest images without a new pertiskd.
async fn apply_node_version_via_agent(
    kubeconfig: &str,
    node_name: &str,
    version: &str,
    is_controlplane: bool,
    log_path: &str,
) -> anyhow::Result<()> {
    let pod = format!(
        "pertisk-upg-{}",
        &Uuid::new_v4().to_string().replace('-', "")[..12]
    );
    let cp_flag = if is_controlplane { "1" } else { "0" };
    let script = format!(
        r#"set -eu
apk add --no-cache curl >/dev/null
VER="{version}"
CP="{cp_flag}"
if [ "$CP" = "1" ]; then
  for f in /host/manifests/kube-apiserver.yaml \
           /host/manifests/kube-controller-manager.yaml \
           /host/manifests/kube-scheduler.yaml; do
    [ -f "$f" ] || continue
    sed -i -E "s#(image:[[:space:]]*registry\.k8s\.io/kube-[^:]+):v[0-9]+\.[0-9]+\.[0-9]+#\1:${{VER}}#g" "$f"
    echo "bumped $f"
  done
fi
ARCH=$(uname -m)
case "$ARCH" in x86_64) A=amd64;; aarch64|arm64) A=arm64;; *) A=amd64;; esac
echo "download kubelet $VER ($A)"
curl -fsSL -o /host/bin/.kubelet.new "https://dl.k8s.io/release/${{VER}}/bin/linux/${{A}}/kubelet"
chmod 755 /host/bin/.kubelet.new
mv /host/bin/.kubelet.new /host/bin/kubelet
echo "installed kubelet"
# hostPID: kill host kubelet; pertiskd will restart it. Do this last — CNI may
# die and the agent pod can be torn down before exit 0 is observed.
killed=0
for p in /proc/[0-9]*; do
  cmd=$(cat "$p/cmdline" 2>/dev/null | tr '\0' ' ' || true)
  case "$cmd" in
    */usr/local/bin/kubelet*)
      pid=${{p#/proc/}}
      echo "kill kubelet pid=$pid"
      kill "$pid" 2>/dev/null || true
      killed=1
      break
      ;;
  esac
done
echo "upgrade agent done killed=$killed"
exit 0
"#
    );
    // Escape for YAML literal block: indent every line.
    let indented: String = script
        .lines()
        .map(|l| format!("        {l}"))
        .collect::<Vec<_>>()
        .join("\n");

    let yaml = format!(
        r#"apiVersion: v1
kind: Pod
metadata:
  name: {pod}
  namespace: kube-system
  labels:
    app: pertisk-upgrade-agent
spec:
  nodeName: {node_name}
  hostNetwork: true
  hostPID: true
  restartPolicy: Never
  tolerations:
  - operator: Exists
  containers:
  - name: upgrade
    image: alpine:3.20
    imagePullPolicy: IfNotPresent
    securityContext:
      privileged: true
    volumeMounts:
    - name: manifests
      mountPath: /host/manifests
    - name: bin
      mountPath: /host/bin
    command: ["/bin/sh", "-c"]
    args:
      - |
{indented}
  volumes:
  - name: manifests
    hostPath:
      path: /etc/kubernetes/manifests
      type: DirectoryOrCreate
  - name: bin
    hostPath:
      path: /usr/local/bin
      type: Directory
"#
    );

    let tmp = std::env::temp_dir().join(format!("{pod}.yaml"));
    std::fs::write(&tmp, &yaml)?;
    let apply = Command::new("kubectl")
        .args(["--kubeconfig", kubeconfig, "apply", "-f"])
        .arg(&tmp)
        .output()
        .await?;
    append_log(log_path, &String::from_utf8_lossy(&apply.stdout))?;
    append_log(log_path, &String::from_utf8_lossy(&apply.stderr))?;
    let _ = std::fs::remove_file(&tmp);
    if !apply.status.success() {
        anyhow::bail!("failed to create upgrade agent pod on {node_name}");
    }

    // Wait for Succeeded/Failed, or for the node kubelet version to already match
    // (apiserver blips during CP upgrade make phase polls unreliable).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    let mut phase = String::new();
    while std::time::Instant::now() < deadline {
        if node_kubelet_version(kubeconfig, node_name)
            .await
            .as_deref()
            == Some(version)
        {
            append_log(
                log_path,
                &format!("upgrade agent on {node_name}: kubelet already {version}\n"),
            )?;
            phase = "Succeeded".into();
            break;
        }
        let out = Command::new("kubectl")
            .args([
                "--kubeconfig",
                kubeconfig,
                "get",
                "pod",
                "-n",
                "kube-system",
                &pod,
                "-o",
                "jsonpath={.status.phase}",
            ])
            .output()
            .await;
        if let Ok(out) = out {
            if out.status.success() {
                phase = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if phase == "Succeeded" || phase == "Failed" {
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }

    let logs = Command::new("kubectl")
        .args([
            "--kubeconfig",
            kubeconfig,
            "logs",
            "-n",
            "kube-system",
            &pod,
            "--tail=200",
        ])
        .output()
        .await;
    if let Ok(l) = logs {
        append_log(log_path, &String::from_utf8_lossy(&l.stdout))?;
        append_log(log_path, &String::from_utf8_lossy(&l.stderr))?;
    }

    let _ = Command::new("kubectl")
        .args([
            "--kubeconfig",
            kubeconfig,
            "delete",
            "pod",
            "-n",
            "kube-system",
            &pod,
            "--wait=false",
            "--ignore-not-found=true",
        ])
        .output()
        .await;

    if phase != "Succeeded" {
        // Killing kubelet often tears down the agent pod (CNI/runtime), so phase may be
        // Failed even when the binary was replaced. Caller waits on kubeletVersion.
        append_log(
            log_path,
            &format!(
                "upgrade agent on {node_name} phase={phase} (ok if kubelet version matches next)\n"
            ),
        )?;
    } else {
        append_log(log_path, &format!("upgrade agent on {node_name} succeeded\n"))?;
    }
    Ok(())
}

async fn wait_node_kubelet_version(
    kubeconfig: &str,
    node_name: &str,
    want: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    while std::time::Instant::now() < deadline {
        match node_kubelet_version(kubeconfig, node_name).await {
            Some(got) if got == want => {
                append_log(log_path, &format!("node/{node_name} kubelet={got}\n"))?;
                return Ok(());
            }
            Some(got) => {
                append_log(
                    log_path,
                    &format!("node/{node_name} kubelet={got} (want {want}); retry\n"),
                )?;
            }
            None => {}
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    anyhow::bail!("timed out waiting for {node_name} kubelet={want}")
}

async fn node_kubelet_version(kubeconfig: &str, node_name: &str) -> Option<String> {
    let out = Command::new("kubectl")
        .args([
            "--kubeconfig",
            kubeconfig,
            "get",
            "node",
            node_name,
            "-o",
            "jsonpath={.status.nodeInfo.kubeletVersion}",
        ])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if got.is_empty() {
        None
    } else {
        Some(got)
    }
}

fn normalize_k8s_version(v: &str) -> String {
    let v = v.trim();
    if v.starts_with('v') {
        v.to_string()
    } else {
        format!("v{v}")
    }
}

async fn kubectl(kubeconfig: &str, args: &[&str], log_path: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kubeconfig).args(args);
    let out = cmd.output().await?;
    append_log(log_path, &String::from_utf8_lossy(&out.stdout))?;
    append_log(log_path, &String::from_utf8_lossy(&out.stderr))?;
    if !out.status.success() {
        anyhow::bail!("kubectl {} failed", args.first().unwrap_or(&""));
    }
    Ok(())
}

fn patch_kubernetes_version(yaml: &str, version: &str) -> String {
    let mut out = String::with_capacity(yaml.len() + 32);
    let mut patched = false;
    for line in yaml.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("kubernetesVersion:") {
            let indent = line.len() - trimmed.len();
            out.push_str(&" ".repeat(indent));
            out.push_str("kubernetesVersion: ");
            out.push_str(version);
            out.push('\n');
            patched = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !patched && out.contains("cluster:") {
        let mut inserted = false;
        let mut rebuilt = String::new();
        for line in out.lines() {
            rebuilt.push_str(line);
            rebuilt.push('\n');
            if !inserted && line.trim() == "cluster:" {
                rebuilt.push_str("  kubernetesVersion: ");
                rebuilt.push_str(version);
                rebuilt.push('\n');
                inserted = true;
            }
        }
        return rebuilt;
    }
    out
}

fn resolve_node_machine_config(
    state: &AppState,
    cluster_name: &str,
    node_name: &str,
) -> Option<PathBuf> {
    let dir = state.cfg().kubeconfigs_dir().join(cluster_name);
    // Prefer per-node yaml (worker-1.yaml / controlplane-2.yaml), then role templates.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(suffix) = node_name.strip_prefix(&format!("{cluster_name}-")) {
        // cp-1 → controlplane.yaml, cp-2 → controlplane-2.yaml, wk-1 → worker-1.yaml
        if let Some(rest) = suffix.strip_prefix("cp-") {
            if rest == "1" {
                candidates.push(dir.join("controlplane.yaml"));
            } else {
                candidates.push(dir.join(format!("controlplane-{rest}.yaml")));
            }
        } else if let Some(rest) = suffix.strip_prefix("wk-") {
            candidates.push(dir.join(format!("worker-{rest}.yaml")));
            candidates.push(dir.join("worker.yaml"));
        }
    }
    candidates.push(dir.join(format!("{node_name}.yaml")));
    candidates.into_iter().find(|p| p.is_file())
}

async fn run_update_config(
    state: &AppState,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: serde_json::Value = serde_json::from_str(payload)?;
    let config_yaml = p
        .get("config_yaml")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let node_id = p.get("node_id").and_then(|v| v.as_str());

    let nodes = if let Some(nid) = node_id {
        sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT id, name, ip FROM nodes WHERE id = ? AND cluster_id = ?",
        )
        .bind(nid)
        .bind(cid)
        .fetch_all(state.pool())
        .await?
    } else {
        sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT id, name, ip FROM nodes WHERE cluster_id = ?",
        )
        .bind(cid)
        .fetch_all(state.pool())
        .await?
    };

    let tmp = state.cfg().data_dir.join(format!("cfg-{}.yaml", Uuid::new_v4()));
    std::fs::write(&tmp, config_yaml)?;

    for (_id, name, ip) in nodes {
        append_log(log_path, &format!("apply config to {name}\n"))?;
        if let Some(ip) = ip {
            if state.cfg().pertiskctl.exists() {
                let out = Command::new(&state.cfg().pertiskctl)
                    .args([
                        "-e",
                        &format!("{ip}:50000"),
                        "apply",
                        "-f",
                        &tmp.to_string_lossy(),
                    ])
                    .output()
                    .await;
                if let Ok(o) = out {
                    append_log(log_path, &String::from_utf8_lossy(&o.stdout))?;
                    append_log(log_path, &String::from_utf8_lossy(&o.stderr))?;
                }
            } else {
                append_log(log_path, "pertiskctl missing; config update recorded\n")?;
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct ClusterRow {
    id: String,
    name: String,
    provider_id: String,
    controlplanes: i64,
    workers: i64,
    vip: Option<String>,
    vip6: Option<String>,
    cni: String,
    k8s_version: String,
    cp_memory: i64,
    cp_cores: i64,
    cp_disk_gb: i64,
    worker_memory: i64,
    worker_cores: i64,
    worker_disk_gb: i64,
    cp_vmid: Option<i64>,
    network_mode: String,
}

#[derive(Debug, sqlx::FromRow)]
struct ProviderRow {
    id: String,
    url: String,
    token_id: String,
    token_secret_enc: String,
    node: String,
    storage: String,
    bridge: String,
    insecure: i64,
}

pub async fn enqueue(
    state: &AppState,
    cluster_id: Option<&str>,
    kind: &str,
    payload: serde_json::Value,
) -> anyhow::Result<String> {
    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    let log_path: PathBuf = state.cfg().jobs_dir().join(format!("{id}.log"));
    std::fs::create_dir_all(state.cfg().jobs_dir())?;
    sqlx::query(
        r#"INSERT INTO jobs (id, cluster_id, kind, status, payload_json, log_path, created_at, updated_at)
           VALUES (?, ?, ?, 'queued', ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(cluster_id)
    .bind(kind)
    .bind(payload.to_string())
    .bind(log_path.to_string_lossy().as_ref())
    .bind(&now)
    .bind(&now)
    .execute(state.pool())
    .await?;
    state.notify_jobs();
    Ok(id)
}
