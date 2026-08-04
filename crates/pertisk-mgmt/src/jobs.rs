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
    let row = sqlx::query_as::<_, (String, Option<String>, String, String, Option<String>)>(
        "SELECT id, cluster_id, kind, payload_json, log_path FROM jobs WHERE status = 'queued' ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(state.pool())
    .await?;

    let Some((id, cluster_id, kind, payload, log_path)) = row else {
        return Ok(());
    };

    let now = db::now_rfc3339();
    sqlx::query("UPDATE jobs SET status = 'running', updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&id)
        .execute(state.pool())
        .await?;

    if let Some(cid) = &cluster_id {
        let _ = sqlx::query("UPDATE clusters SET status = 'provisioning', updated_at = ?, error = NULL WHERE id = ?")
            .bind(&now)
            .bind(cid)
            .execute(state.pool())
            .await;
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
        "create_cluster" => run_create_cluster(state, &id, cluster_id.as_deref(), &payload, &log_file).await,
        "delete_cluster" => run_delete_cluster(state, cluster_id.as_deref(), &payload, &log_file).await,
        "add_node" => run_add_node(state, cluster_id.as_deref(), &payload, &log_file).await,
        "remove_node" => run_remove_node(state, cluster_id.as_deref(), &payload, &log_file).await,
        "upgrade_cluster" => run_upgrade(state, cluster_id.as_deref(), &payload, &log_file).await,
        "update_config" => run_update_config(state, cluster_id.as_deref(), &payload, &log_file).await,
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
                if kind == "delete_cluster" {
                    // cluster row may already be gone
                } else if kind != "delete_cluster" {
                    let status = if kind == "create_cluster" {
                        "ready"
                    } else {
                        "ready"
                    };
                    let _ = sqlx::query(
                        "UPDATE clusters SET status = ?, updated_at = ?, error = NULL WHERE id = ?",
                    )
                    .bind(status)
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
                  cp_memory, cp_cores, cp_disk_gb, worker_memory, worker_cores, worker_disk_gb, cp_vmid
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
    if let Some(vip) = &cluster.vip {
        if !vip.is_empty() {
            cmd.arg("--vip").arg(vip);
        }
    }
    if let Some(vip6) = &cluster.vip6 {
        if !vip6.is_empty() {
            cmd.arg("--vip6").arg(vip6).arg("--dual-stack");
        }
    }

    append_log(log_path, &format!("$ {:?}\n", cmd.as_std().get_program()))?;
    append_log(
        log_path,
        &format!(
            "create cluster={} cps={} workers={} vip={:?}\n",
            cluster.name, cluster.controlplanes, cluster.workers, cluster.vip
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

async fn seed_stub_nodes(state: &AppState, cluster: &ClusterRow) -> anyhow::Result<()> {
    let now = db::now_rfc3339();
    for i in 1..=cluster.controlplanes {
        let name = format!("{}-cp-{}", cluster.name, i);
        let id = Uuid::new_v4().to_string();
        let vmid = cluster.cp_vmid.map(|v| v + i - 1);
        let _ = sqlx::query(
            r#"INSERT INTO nodes (id, cluster_id, name, role, vmid, status, created_at, updated_at)
               VALUES (?, ?, ?, 'controlplane', ?, 'ready', ?, ?)
               ON CONFLICT(cluster_id, name) DO UPDATE SET status = 'ready', updated_at = excluded.updated_at"#,
        )
        .bind(&id)
        .bind(&cluster.id)
        .bind(&name)
        .bind(vmid)
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
            r#"INSERT INTO nodes (id, cluster_id, name, role, vmid, status, created_at, updated_at)
               VALUES (?, ?, ?, 'worker', ?, 'ready', ?, ?)
               ON CONFLICT(cluster_id, name) DO UPDATE SET status = 'ready', updated_at = excluded.updated_at"#,
        )
        .bind(&id)
        .bind(&cluster.id)
        .bind(&name)
        .bind(vmid)
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
    let cluster = sqlx::query_as::<_, (String, String, Option<i64>, i64, i64)>(
        "SELECT id, provider_id, cp_vmid, controlplanes, workers FROM clusters WHERE id = ?",
    )
    .bind(cid)
    .fetch_optional(state.pool())
    .await?;

    let Some((id, provider_id, cp_vmid, cps, workers)) = cluster else {
        append_log(log_path, "cluster already removed\n")?;
        return Ok(());
    };

    let provider = sqlx::query_as::<_, ProviderRow>(
        "SELECT id, url, token_id, token_secret_enc, node, storage, bridge, insecure FROM providers WHERE id = ?",
    )
    .bind(&provider_id)
    .fetch_one(state.pool())
    .await?;
    let secret = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?;
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
    .await?;

    let mut vmids: Vec<i64> = nodes.into_iter().filter_map(|(v,)| v).collect();
    if vmids.is_empty() {
        if let Some(base) = cp_vmid {
            for i in 0..(cps + workers) {
                vmids.push(base + i);
            }
        }
    }

    for vmid in vmids {
        append_log(log_path, &format!("deleting VM {vmid} on {}\n", provider.node))?;
        if let Err(e) = client.delete_vm(&provider.node, vmid).await {
            append_log(log_path, &format!("warn: delete {vmid}: {e}\n"))?;
        }
    }

    sqlx::query("DELETE FROM nodes WHERE cluster_id = ?")
        .bind(&id)
        .execute(state.pool())
        .await?;
    sqlx::query("DELETE FROM clusters WHERE id = ?")
        .bind(&id)
        .execute(state.pool())
        .await?;
    append_log(log_path, "cluster deleted\n")?;
    Ok(())
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

    let nodes = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT id, name, ip FROM nodes WHERE cluster_id = ? ORDER BY role ASC, name ASC",
    )
    .bind(cid)
    .fetch_all(state.pool())
    .await?;

    append_log(
        log_path,
        &format!(
            "rolling upgrade to {version} across {} nodes (workers then CP)\n",
            nodes.len()
        ),
    )?;

    // Workers first
    for (id, name, ip) in nodes.iter().filter(|n| n.1.contains("wk") || !n.1.contains("cp")) {
        append_log(log_path, &format!("upgrade worker {name} ip={ip:?}\n"))?;
        sqlx::query("UPDATE nodes SET status = 'upgrading', updated_at = ? WHERE id = ?")
            .bind(db::now_rfc3339())
            .bind(id)
            .execute(state.pool())
            .await?;
        // Invoke pertiskctl upgrade if binary + IP available
        if let Some(ip) = ip {
            if state.cfg().pertiskctl.exists() {
                let out = Command::new(&state.cfg().pertiskctl)
                    .args(["-e", &format!("{ip}:50000"), "upgrade", "--version", version])
                    .output()
                    .await;
                match out {
                    Ok(o) => {
                        append_log(log_path, &String::from_utf8_lossy(&o.stdout))?;
                        append_log(log_path, &String::from_utf8_lossy(&o.stderr))?;
                    }
                    Err(e) => append_log(log_path, &format!("pertiskctl upgrade skipped: {e}\n"))?,
                }
            } else {
                append_log(log_path, "pertiskctl not found; recorded upgrade intent\n")?;
            }
        }
        sqlx::query("UPDATE nodes SET status = 'ready', updated_at = ? WHERE id = ?")
            .bind(db::now_rfc3339())
            .bind(id)
            .execute(state.pool())
            .await?;
    }

    // Control planes one-by-one
    for (id, name, ip) in nodes.iter().filter(|n| n.1.contains("cp") || n.1.contains("-cp-")) {
        append_log(log_path, &format!("upgrade control plane {name} ip={ip:?}\n"))?;
        sqlx::query("UPDATE nodes SET status = 'upgrading', updated_at = ? WHERE id = ?")
            .bind(db::now_rfc3339())
            .bind(id)
            .execute(state.pool())
            .await?;
        if let Some(ip) = ip {
            if state.cfg().pertiskctl.exists() {
                let _ = Command::new(&state.cfg().pertiskctl)
                    .args(["-e", &format!("{ip}:50000"), "upgrade", "--version", version])
                    .output()
                    .await;
            }
        }
        sqlx::query("UPDATE nodes SET status = 'ready', updated_at = ? WHERE id = ?")
            .bind(db::now_rfc3339())
            .bind(id)
            .execute(state.pool())
            .await?;
    }

    let now = db::now_rfc3339();
    sqlx::query("UPDATE clusters SET k8s_version = ?, updated_at = ? WHERE id = ?")
        .bind(version)
        .bind(&now)
        .bind(cid)
        .execute(state.pool())
        .await?;
    append_log(log_path, "upgrade complete\n")?;
    Ok(())
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
