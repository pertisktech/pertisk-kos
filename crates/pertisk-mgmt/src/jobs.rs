use std::path::PathBuf;
use std::process::Stdio;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use crate::crypto;
use crate::db;
use crate::state::AppState;

/// Shared env for packaged lab-up / add-node (images dir, optional host overrides).
/// Returns a job-log note when global `PROXMOX_SSH` is retargeted to this provider.
fn apply_lab_env(cmd: &mut Command, state: &AppState, provider_url: &str) -> Option<String> {
    let cfg = state.cfg();
    let _ = std::fs::create_dir_all(&cfg.images_dir);
    cmd.env("PERTISK_IMAGES_DIR", cfg.images_dir.display().to_string());
    cmd.env("PERTISKCTL", cfg.pertiskctl.display().to_string());
    // Settings → Public URL → machine.dashboard.mgmt_url on gen/apply.
    // Never push http://0.0.0.0:… onto guests (listen wildcard).
    let public_url = cfg.public_url.trim();
    if !public_url.is_empty() && !crate::config::public_url_host_unusable(public_url) {
        cmd.env("MGMT_PUBLIC_URL", public_url);
    }
    if let Some(root) = cfg.lab_up.parent().and_then(|p| p.parent()) {
        cmd.env("PERTISK_ROOT", root.display().to_string());
    }
    for key in [
        "PROXMOX_DISK",
        "PROXMOX_SSH",
        "PROXMOX_NO_SSH",
        "PROXMOX_UPLOAD_STORAGE",
        "PROXMOX_SSH_AUTO",
        "LAB_SUBNET",
        "LAB_GATEWAY",
        "NUTANIX_GATEWAY",
        "PROXMOX_IMAGES_DIR",
        "ARCH",
        "PERTISK_ARCH",
        "PROXMOX_ARM64_TEMPLATE",
        "BOOTSTRAP_TIMEOUT",
        "PERTISK_VM_JOBS",
        // Talos-style static IPs (no DHCP; Proxmox only). Set on the mgmt
        // process env to steer new clusters away from known-conflicting
        // addresses (e.g. a Nutanix CVM sharing the same LAN/DHCP pool).
        "PROXMOX_STATIC_BASE",
        "PROXMOX_STATIC_SUBNET",
        "PROXMOX_STATIC_GATEWAY",
        "PROXMOX_STATIC_NAMESERVER",
        "PROXMOX_STATIC_EXCLUDE",
        "PROXMOX_STATIC_IPS",
        // Nutanix IPAM reserved IPs (space-separated list of addresses).
        "NUTANIX_STATIC_IPS",
        // vSphere guest static IPs (space-separated list of addresses).
        "VSPHERE_STATIC_IPS",
    ] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                cmd.env(key, v);
            }
        }
    }
    // Omni-style default: Proxmox API only (provider token). Do not invent
    // PROXMOX_SSH from the provider URL — that forced scp and broke RPM labs
    // without keys. Opt in with PROXMOX_SSH=… or PROXMOX_SSH_AUTO=1.
    let no_ssh = std::env::var("PROXMOX_NO_SSH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let ssh_auto = std::env::var("PROXMOX_SSH_AUTO")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let ssh_from_host = std::env::var("PROXMOX_SSH")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some();
    let mut using_ssh = false;
    let mut note = None;
    if no_ssh {
        cmd.env_remove("PROXMOX_SSH");
    } else if ssh_from_host {
        let ssh = std::env::var("PROXMOX_SSH").unwrap_or_default();
        // One global env for many Proxmox providers: keep the user, SSH to
        // this job's provider host (10.1.1.196 leftover must not hit 10.1.1.195).
        if let Some(rewritten) = rewrite_proxmox_ssh_for_provider(&ssh, provider_url) {
            cmd.env("PROXMOX_SSH", &rewritten);
            note = Some(format!("PROXMOX_SSH={ssh} → {rewritten} (this provider)\n"));
        }
        using_ssh = true;
    } else if ssh_auto {
        if let Some(host) = pve_host_from_url(provider_url) {
            if host.chars().all(|c| c.is_ascii_digit() || c == '.') && host.contains('.') {
                cmd.env("PROXMOX_SSH", format!("root@{host}"));
                using_ssh = true;
            }
        }
    }
    if !using_ssh
        && std::env::var("PROXMOX_UPLOAD_STORAGE")
            .ok()
            .filter(|s| !s.is_empty())
            .is_none()
    {
        // Directory storage for content=import; VM disks can still be local-zfs.
        cmd.env("PROXMOX_UPLOAD_STORAGE", "local");
    }
    note
}

/// Merge every registered provider's host (Proxmox/Nutanix/vSphere — not just
/// this cluster's), any existing cluster node IPs, plus any operator-set
/// `PROXMOX_STATIC_EXCLUDE` into the job's exclude list, so a static-IP scan
/// never hands out a hypervisor's own management address or an existing node.
async fn auto_exclude_provider_hosts(cmd: &mut Command, state: &AppState) {
    let want_static = std::env::var("PROXMOX_STATIC_BASE")
        .or_else(|_| std::env::var("PROXMOX_STATIC_SUBNET"))
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if !want_static {
        return;
    }
    let mut hosts: Vec<String> = Vec::new();

    // Provider hosts (hypervisor management addresses).
    let urls: Vec<String> = sqlx::query_scalar("SELECT url FROM providers")
        .fetch_all(state.pool())
        .await
        .unwrap_or_default();
    hosts.extend(urls.iter().filter_map(|u| pve_host_from_url(u)));

    // Existing cluster node IPs (both IPv4 and IPv6).
    let node_ips: Vec<Option<String>> = sqlx::query_scalar("SELECT ip FROM nodes WHERE ip IS NOT NULL")
        .fetch_all(state.pool())
        .await
        .unwrap_or_default();
    hosts.extend(node_ips.into_iter().flatten());

    let node_ip6s: Vec<Option<String>> = sqlx::query_scalar("SELECT ip6 FROM nodes WHERE ip6 IS NOT NULL")
        .fetch_all(state.pool())
        .await
        .unwrap_or_default();
    hosts.extend(node_ip6s.into_iter().flatten());

    // Operator-specified exclusions.
    if let Ok(extra) = std::env::var("PROXMOX_STATIC_EXCLUDE") {
        hosts.extend(
            extra
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }

    hosts.sort();
    hosts.dedup();
    if !hosts.is_empty() {
        cmd.env("PROXMOX_STATIC_EXCLUDE", hosts.join(","));
    }
}

fn pve_host_from_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split(['/', ':']).next()?.trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn proxmox_ssh_host(ssh: &str) -> Option<String> {
    let rest = ssh.split_once('@').map(|(_, h)| h).unwrap_or(ssh.trim());
    let host = rest.split(['/', ':']).next()?.trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn proxmox_ssh_user(ssh: &str) -> &str {
    match ssh.split_once('@') {
        Some((u, h)) if !u.is_empty() && !h.is_empty() => u,
        _ => "root",
    }
}

/// Extract IP from provider URL (https://10.1.1.111:9440 → 10.1.1.111).
pub(crate) fn ip_from_provider_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split(['/', ':']).next()?.trim();
    if host.is_empty() || !host.chars().any(|c| c.is_numeric()) {
        return None;
    }
    Some(host.to_string())
}

/// Infer /24 subnet from provider IP. E.g., 10.1.1.111 → 10.1.1.0/24.
pub(crate) fn subnet_from_provider_ip(ip: &str) -> Option<String> {
    let octets: Vec<&str> = ip.split('.').collect();
    if octets.len() == 4 {
        Some(format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2]))
    } else {
        None
    }
}

/// Extract gateway IP from subnet. E.g., 10.1.1.0/24 → 10.1.1.1.
fn gateway_from_subnet(subnet: &str) -> Option<String> {
    let net_part = subnet.split('/').next()?;
    let octets: Vec<&str> = net_part.split('.').collect();
    if octets.len() == 4 {
        Some(format!("{}.{}.{}.1", octets[0], octets[1], octets[2]))
    } else {
        None
    }
}

/// Auto-detect the default gateway from the mgmt server's routing table.
/// Runs `ip route` and parses `default via X.X.X.X`.
fn detect_default_gateway() -> Option<String> {
    let output = std::process::Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // Parse: "default via 10.1.1.10 dev eth0 ..."
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("default via ") {
            let gw = rest.split_whitespace().next()?;
            if !gw.is_empty() && gw.contains('.') {
                return Some(gw.to_string());
            }
        }
    }
    None
}

/// Check if an IP is in-use via TCP connection to common ports (more reliable than ICMP ping).
pub(crate) async fn ip_is_in_use(ip: String, timeout_ms: u64) -> bool {
    use std::net::{IpAddr, SocketAddr};
    use std::str::FromStr;
    use std::time::Duration;

    // Common ports: HTTP, HTTPS, SSH, management
    let ports = [80u16, 443, 22, 8006, 9440, 443];
    let timeout_dur = Duration::from_millis(timeout_ms);
    let ip_addr = match IpAddr::from_str(&ip) {
        Ok(a) => a,
        Err(_) => return false,
    };

    // Try any port; if one connects, the host is in use.
    for &port in &ports {
        let addr = SocketAddr::new(ip_addr, port);
        match tokio::time::timeout(timeout_dur, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(_)) => return true,
            _ => continue,
        }
    }
    false
}

/// Scan a subnet for N free IPs (native async; no subprocess or ping).
/// Returns up to N IPs that don't respond to TCP on common ports.
pub(crate) async fn scan_subnet_for_free_ips(
    subnet: &str,
    count: i64,
    exclude: &[String],
) -> anyhow::Result<Vec<String>> {
    use std::net::IpAddr;
    use std::str::FromStr;

    // Parse subnet (e.g., "10.1.1.0/24")
    let net = ipnetwork::IpNetwork::from_str(subnet)
        .map_err(|e| anyhow::anyhow!("invalid subnet {}: {}", subnet, e))?;

    // Build exclusion set
    let mut excluded: std::collections::HashSet<IpAddr> = std::collections::HashSet::new();
    for exc in exclude {
        if let Ok(ip) = IpAddr::from_str(exc.trim()) {
            excluded.insert(ip);
        }
    }

    // Gateway (typically .1) is always unavailable
    let gateway = net.network();
    excluded.insert(gateway);

    // Collect candidate IPs
    let mut candidates: Vec<IpAddr> = net
        .iter()
        .filter(|ip| !excluded.contains(ip) && *ip != gateway)
        .collect();

    // Randomize to avoid always scanning the same IPs first
    use rand::seq::SliceRandom;
    candidates.shuffle(&mut rand::thread_rng());

    // Scan candidates concurrently (32 at a time)
    let scan_count = std::cmp::min(count as usize * 2, candidates.len());
    let mut free_ips = Vec::new();

    for chunk in candidates.iter().take(scan_count).collect::<Vec<_>>().chunks(32) {
        let futures: Vec<_> = chunk
            .iter()
            .map(|&&ip| ip_is_in_use(ip.to_string(), 500))
            .collect();

        let results = futures::future::join_all(futures).await;
        for (idx, in_use) in results.iter().enumerate() {
            if !in_use {
                let ip = chunk[idx];
                free_ips.push(format!("{}/{}", ip, net.prefix()));
                if free_ips.len() >= count as usize {
                    return Ok(free_ips);
                }
            }
        }
    }

    Ok(free_ips)
}


/// Arm64 guests on a native aarch64 PVE use API create (default arch). Cross-arch
/// (x86 PVE + aarch64 guest) still needs `PROXMOX_ARM64_TEMPLATE` or root SSH.
async fn apply_proxmox_arm64_create_env(
    cmd: &mut Command,
    log_path: &str,
    provider: &ProviderRow,
    secret: &str,
) -> anyhow::Result<()> {
    if matches!(
        provider.kind.as_str(),
        "nutanix" | "ahv" | "prism" | "vsphere" | "esxi"
    ) {
        return Ok(());
    }
    let client = crate::proxmox::ProxmoxClient {
        url: provider.url.clone(),
        token_id: provider.token_id.clone(),
        token_secret: secret.to_string(),
        insecure: provider.insecure != 0,
    };
    let native = client
        .detect_node_arch(&provider.node)
        .await
        .ok()
        .as_deref()
        == Some("arm64");
    if native {
        append_log(
            log_path,
            "note: aarch64 PVE — arm64 guests via API (default arch; no SSH/template)\n",
        )?;
        return Ok(());
    }
    let template = std::env::var("PROXMOX_ARM64_TEMPLATE")
        .ok()
        .filter(|s| !s.is_empty());
    if let Some(tmpl) = template {
        append_log(
            log_path,
            &format!(
                "note: arch=arm64 via API clone of PROXMOX_ARM64_TEMPLATE={tmpl} (no SSH required)\n"
            ),
        )?;
        cmd.env("PROXMOX_ARM64_TEMPLATE", &tmpl);
        cmd.env("PROXMOX_NO_SSH", "1");
        cmd.env_remove("PROXMOX_SSH");
        return Ok(());
    }
    append_log(
        log_path,
        "note: arch=arm64 on amd64 PVE — needs pertisk-cloud-arm64*.qcow2 and either PROXMOX_ARM64_TEMPLATE=<vmid> (API) or PROXMOX_SSH=root@<pve>\n",
    )?;
    cmd.env_remove("PROXMOX_NO_SSH");
    let has_ssh = std::env::var("PROXMOX_SSH")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some();
    if !has_ssh {
        if let Some(host) = pve_host_from_url(&provider.url) {
            let ssh = format!("root@{host}");
            append_log(
                log_path,
                &format!("auto PROXMOX_SSH={ssh} for cross-arch aarch64 guests\n"),
            )?;
            cmd.env("PROXMOX_SSH", ssh);
        } else {
            append_log(
                log_path,
                "warn: set PROXMOX_ARM64_TEMPLATE or PROXMOX_SSH=root@<pve>\n",
            )?;
        }
    }
    Ok(())
}

/// `PROXMOX_SSH` is a user + “prefer SSH” flag. The host is always this provider.
fn rewrite_proxmox_ssh_for_provider(ssh: &str, provider_url: &str) -> Option<String> {
    let api_h = pve_host_from_url(provider_url)?;
    if api_h.is_empty() {
        return None;
    }
    let user = proxmox_ssh_user(ssh);
    let rewritten = format!("{user}@{api_h}");
    match proxmox_ssh_host(ssh) {
        Some(h) if h == api_h => None,
        _ => Some(rewritten),
    }
}

/// Exclusive jobs (create/delete/upgrade/node ops) run one at a time **per cluster**.
/// Different clusters do not wait on each other. `install_addon` can overlap with
/// other clusters and with other add-ons, but waits if *this* cluster already has
/// an exclusive job. Delete aborts that cluster's in-flight jobs so create is not
/// stuck `queued`.
pub fn spawn_worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            match start_available_jobs(&state).await {
                Ok(n) if n > 0 => continue,
                Ok(_) => {}
                Err(e) => tracing::error!(error = %e, "job worker tick failed"),
            }
            tokio::select! {
                _ = state.inner.job_notify.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
            }
        }
    });
}

/// Crash / RPM restart can leave rows `running` with no worker. Drop jobs whose
/// cluster is gone, then put the rest back in line.
pub async fn requeue_orphaned_jobs(pool: &sqlx::SqlitePool) -> anyhow::Result<()> {
    let now = db::now_rfc3339();
    let dropped = sqlx::query(
        r#"UPDATE jobs
           SET status = 'cancelled', error = 'cluster removed', updated_at = ?, finished_at = ?
           WHERE status IN ('queued', 'running') AND finished_at IS NULL
             AND (cluster_id IS NULL
                  OR NOT EXISTS (SELECT 1 FROM clusters c WHERE c.id = jobs.cluster_id))"#,
    )
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?
    .rows_affected();
    if dropped > 0 {
        tracing::warn!(count = dropped, "cancelled jobs whose cluster is gone");
    }
    let n = sqlx::query(
        r#"UPDATE jobs
           SET status = 'queued', error = 'requeued after mgmt restart', updated_at = ?
           WHERE status = 'running' AND finished_at IS NULL"#,
    )
    .bind(&now)
    .execute(pool)
    .await?
    .rows_affected();
    if n > 0 {
        tracing::warn!(count = n, "requeued jobs left running after restart");
    }
    Ok(())
}

/// Cancel queued/running jobs for a cluster and abort the worker task if it is that cluster.
pub async fn cancel_cluster_jobs(
    state: &AppState,
    cid: &str,
    except_job_id: Option<&str>,
) -> anyhow::Result<u64> {
    state.abort_running_job_for_cluster(cid, except_job_id);
    let now = db::now_rfc3339();
    let rows: Vec<(String, String)> = if let Some(ex) = except_job_id {
        sqlx::query_as(
            r#"SELECT id, kind FROM jobs
               WHERE cluster_id = ? AND status IN ('queued', 'running')
                 AND kind != 'delete_cluster' AND id != ?"#,
        )
        .bind(cid)
        .bind(ex)
        .fetch_all(state.pool())
        .await?
    } else {
        sqlx::query_as(
            r#"SELECT id, kind FROM jobs
               WHERE cluster_id = ? AND status IN ('queued', 'running')
                 AND kind != 'delete_cluster'"#,
        )
        .bind(cid)
        .fetch_all(state.pool())
        .await?
    };
    let n = rows.len() as u64;
    for (id, kind) in &rows {
        sqlx::query(
            r#"UPDATE jobs SET status = 'cancelled', error = 'superseded by delete',
               updated_at = ?, finished_at = ? WHERE id = ? AND status IN ('queued', 'running')"#,
        )
        .bind(&now)
        .bind(&now)
        .bind(id)
        .execute(state.pool())
        .await?;
        state.emit_job(Some(cid), id, Some(kind), "cancelled");
    }
    if n > 0 {
        state.notify_jobs();
    }
    Ok(n)
}

struct ClaimedJob {
    id: String,
    cluster_id: Option<String>,
    kind: String,
    payload: String,
    log_file: String,
}

/// Start every currently eligible queued job without waiting for any of them.
async fn start_available_jobs(state: &AppState) -> anyhow::Result<usize> {
    let mut started = 0;
    while let Some(job) = claim_next(state).await? {
        started += 1;
        let job_id = job.id.clone();
        let cluster_id = job.cluster_id.clone();
        let kind = job.kind.clone();
        let state_run = state.clone();
        let handle = tokio::spawn(async move { execute_job(&state_run, job).await });
        state.set_running_job(
            job_id.clone(),
            cluster_id.clone(),
            kind.clone(),
            handle.abort_handle(),
        );
        let state_done = state.clone();
        tokio::spawn(async move {
            let join = handle.await;
            state_done.clear_running_job(&job_id);
            match join {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(job = %job_id, error = %e, "job task returned error")
                }
                Err(e) if e.is_cancelled() => {
                    tracing::info!(job = %job_id, "job aborted");
                    let now = db::now_rfc3339();
                    let _ = sqlx::query(
                        r#"UPDATE jobs SET status = 'cancelled', error = 'aborted because cluster was deleted',
                           updated_at = ?, finished_at = ? WHERE id = ? AND status = 'running'"#,
                    )
                    .bind(&now)
                    .bind(&now)
                    .bind(&job_id)
                    .execute(state_done.pool())
                    .await;
                    state_done.emit_job(cluster_id.as_deref(), &job_id, Some(&kind), "cancelled");
                }
                Err(e) => tracing::error!(job = %job_id, error = %e, "job task panicked"),
            }
            state_done.notify_jobs();
        });
    }
    Ok(started)
}

/// Create/upgrade/node ops must not overlap on the same cluster. Add-on installs
/// are the exception (and different clusters never block each other).
fn job_is_exclusive(kind: &str) -> bool {
    kind != "install_addon"
}

const MAX_PARALLEL_ADDON_JOBS: usize = 8;

fn same_cluster_has_exclusive(running: &[(Option<String>, String)], cid: &str) -> bool {
    running
        .iter()
        .any(|(c, k)| c.as_deref() == Some(cid) && job_is_exclusive(k))
}

fn same_cluster_has_any(running: &[(Option<String>, String)], cid: &str) -> bool {
    running.iter().any(|(c, _)| c.as_deref() == Some(cid))
}

/// Whether this queued job can start given the in-flight set.
fn job_can_start(
    kind: &str,
    cluster_id: Option<&str>,
    running: &[(Option<String>, String)],
) -> bool {
    if kind == "delete_cluster" {
        // Same-cluster exclusive work is aborted; other clusters keep running.
        return true;
    }
    if kind == "install_addon" {
        let Some(cid) = cluster_id else {
            return false;
        };
        let addon_n = running
            .iter()
            .filter(|(_, k)| *k == "install_addon")
            .count();
        if addon_n >= MAX_PARALLEL_ADDON_JOBS {
            return false;
        }
        return !same_cluster_has_exclusive(running, cid);
    }
    let Some(cid) = cluster_id else {
        return !running.iter().any(|(_, k)| job_is_exclusive(k));
    };
    !same_cluster_has_any(running, cid)
}

async fn claim_next(state: &AppState) -> anyhow::Result<Option<ClaimedJob>> {
    type QueuedJobRow = (String, Option<String>, String, String, Option<String>);

    // Prefer deletes so failed clusters can be cleaned up while creates queue.
    // Skip ineligible rows (do not claim them) so an add-on is not stuck behind
    // another cluster's create.
    loop {
        let rows: Vec<QueuedJobRow> = sqlx::query_as(
            r#"SELECT id, cluster_id, kind, payload_json, log_path FROM jobs
               WHERE status = 'queued'
               ORDER BY CASE WHEN kind = 'delete_cluster' THEN 0 ELSE 1 END, created_at ASC"#,
        )
        .fetch_all(state.pool())
        .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let running = state.running_jobs_snapshot();
        let mut cancelled_stale = false;

        for (id, cluster_id, kind, payload, log_path) in rows {
            if job_is_stale(state, cluster_id.as_deref(), &kind).await? {
                let now = db::now_rfc3339();
                sqlx::query(
                    r#"UPDATE jobs SET status = 'cancelled', error = 'cluster deleting or removed',
                       updated_at = ?, finished_at = ? WHERE id = ? AND status = 'queued'"#,
                )
                .bind(&now)
                .bind(&now)
                .bind(&id)
                .execute(state.pool())
                .await?;
                state.emit_job(cluster_id.as_deref(), &id, Some(&kind), "cancelled");
                cancelled_stale = true;
                continue;
            }

            if !job_can_start(&kind, cluster_id.as_deref(), &running) {
                continue;
            }

            let now = db::now_rfc3339();
            let claimed = sqlx::query(
                "UPDATE jobs SET status = 'running', updated_at = ? WHERE id = ? AND status = 'queued'",
            )
            .bind(&now)
            .bind(&id)
            .execute(state.pool())
            .await?;
            if claimed.rows_affected() == 0 {
                continue;
            }
            state.emit_job(cluster_id.as_deref(), &id, Some(&kind), "running");

            if let Some(cid) = &cluster_id {
                if kind != "delete_cluster" && !is_node_maintenance_job(&kind) {
                    let _ = sqlx::query(
                        "UPDATE clusters SET status = 'provisioning', updated_at = ?, error = NULL WHERE id = ?",
                    )
                    .bind(&now)
                    .bind(cid)
                    .execute(state.pool())
                    .await;
                    state.emit_cluster(cid, "provisioning");
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

            return Ok(Some(ClaimedJob {
                id,
                cluster_id,
                kind,
                payload,
                log_file,
            }));
        }

        if !cancelled_stale {
            return Ok(None);
        }
    }
}

async fn job_is_stale(
    state: &AppState,
    cluster_id: Option<&str>,
    kind: &str,
) -> anyhow::Result<bool> {
    if kind == "delete_cluster" {
        return Ok(false);
    }
    let Some(cid) = cluster_id else {
        return Ok(true);
    };
    let st: Option<String> = sqlx::query_scalar("SELECT status FROM clusters WHERE id = ?")
        .bind(cid)
        .fetch_optional(state.pool())
        .await?;
    Ok(st.as_deref() == Some("deleting") || st.is_none())
}

async fn execute_job(state: &AppState, job: ClaimedJob) -> anyhow::Result<()> {
    let ClaimedJob {
        id,
        cluster_id,
        kind,
        payload,
        log_file,
    } = job;

    let result = match kind.as_str() {
        "create_cluster" => {
            run_create_cluster(state, &id, cluster_id.as_deref(), &payload, &log_file).await
        }
        "delete_cluster" => {
            run_delete_cluster(state, cluster_id.as_deref(), &payload, &log_file).await
        }
        "add_node" => run_add_node(state, cluster_id.as_deref(), &payload, &log_file).await,
        "adopt_node" => run_adopt_node(state, cluster_id.as_deref(), &payload, &log_file).await,
        "remove_node" => run_remove_node(state, cluster_id.as_deref(), &payload, &log_file).await,
        "resize_node" => run_resize_node(state, cluster_id.as_deref(), &payload, &log_file).await,
        "reboot_node" => run_reboot_node(state, cluster_id.as_deref(), &payload, &log_file).await,
        "upgrade_cluster" => run_upgrade(state, cluster_id.as_deref(), &payload, &log_file).await,
        "upgrade_os" => run_upgrade_os(state, cluster_id.as_deref(), &payload, &log_file).await,
        "update_config" => {
            run_update_config(state, cluster_id.as_deref(), &payload, &log_file).await
        }
        "install_addon" => {
            crate::addons::run_install_job(state, cluster_id.as_deref(), &payload, &log_file).await
        }
        other => Err(anyhow::anyhow!("unknown job kind: {other}")),
    };

    let now = db::now_rfc3339();
    match result {
        Ok(()) => {
            if kind == "delete_cluster" {
                let _ = sqlx::query("DELETE FROM jobs WHERE id = ?")
                    .bind(&id)
                    .execute(state.pool())
                    .await;
                state.emit_job(cluster_id.as_deref(), &id, Some(&kind), "succeeded");
                return Ok(());
            }
            sqlx::query(
                "UPDATE jobs SET status = 'succeeded', updated_at = ?, finished_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&now)
            .bind(&id)
            .execute(state.pool())
            .await?;
            state.emit_job(cluster_id.as_deref(), &id, Some(&kind), "succeeded");
            if let Some(cid) = &cluster_id {
                if is_node_maintenance_job(&kind) {
                    // Node-level / config apply: keep cluster ready and clear any
                    // sticky error left by older builds that marked update_config as cluster failure.
                    let _ = sqlx::query(
                        "UPDATE clusters SET status = 'ready', error = NULL, updated_at = ? WHERE id = ? AND status != 'deleting'",
                    )
                    .bind(&now)
                    .bind(cid)
                    .execute(state.pool())
                    .await;
                    state.emit_cluster(cid, "ready");
                } else {
                    let _ = sqlx::query(
                        "UPDATE clusters SET status = 'ready', updated_at = ?, error = NULL WHERE id = ?",
                    )
                    .bind(&now)
                    .bind(cid)
                    .execute(state.pool())
                    .await;
                    state.emit_cluster(cid, "ready");
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
            state.emit_job(cluster_id.as_deref(), &id, Some(&kind), "failed");
            if kind == "delete_cluster" {
                if let Some(cid) = &cluster_id {
                    let _ = purge_cluster_db(state, cid).await;
                    state.emit_cluster(cid, "deleted");
                }
                let _ = sqlx::query("DELETE FROM jobs WHERE id = ?")
                    .bind(&id)
                    .execute(state.pool())
                    .await;
                return Ok(());
            }
            if let Some(cid) = &cluster_id {
                if is_node_maintenance_job(&kind) {
                    // Node-level ops must not mark a healthy cluster as broken.
                    // Clear sticky cluster.error from older update_config failures.
                    let _ = sqlx::query(
                        "UPDATE clusters SET status = 'ready', error = NULL, updated_at = ? WHERE id = ? AND status != 'deleting'",
                    )
                    .bind(&now)
                    .bind(cid)
                    .execute(state.pool())
                    .await;
                    state.emit_cluster(cid, "ready");
                } else {
                    let _ = sqlx::query(
                        "UPDATE clusters SET status = 'error', error = ?, updated_at = ? WHERE id = ?",
                    )
                    .bind(&msg)
                    .bind(&now)
                    .bind(cid)
                    .execute(state.pool())
                    .await;
                    state.emit_cluster(cid, "error");
                }
            }
        }
    }
    Ok(())
}

/// Jobs that only touch individual nodes — cluster stays ready on failure.
fn is_node_maintenance_job(kind: &str) -> bool {
    matches!(
        kind,
        "resize_node"
            | "remove_node"
            | "add_node"
            | "reboot_node"
            | "update_config"
            | "install_addon"
    )
}

pub(crate) fn append_log(path: &str, line: &str) -> anyhow::Result<()> {
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
                  COALESCE(network_mode, 'ipv4') as network_mode,
                  COALESCE(max_pods, 250) as max_pods,
                  COALESCE(arch, 'amd64') as arch,
                  COALESCE(pod_subnet, '10.244.0.0/16') as pod_subnet,
                  COALESCE(service_subnet, '10.96.0.0/12') as service_subnet,
                  pod_subnet_ipv6,
                  service_subnet_ipv6
           FROM clusters WHERE id = ?"#,
    )
    .bind(cid)
    .fetch_one(state.pool())
    .await?;

    let provider = sqlx::query_as::<_, ProviderRow>(
        "SELECT id, kind, url, token_id, token_secret_enc, node, storage, bridge, insecure FROM providers WHERE id = ?",
    )
    .bind(&cluster.provider_id)
    .fetch_one(state.pool())
    .await?;

    let secret = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?;

    let cp_vmid = cluster
        .cp_vmid
        .unwrap_or(p.get("cp_vmid").and_then(|v| v.as_i64()).unwrap_or(210) as i64);

    let kc_dir = state.cfg().kubeconfigs_dir();
    std::fs::create_dir_all(&kc_dir)?;
    let cluster_out = kc_dir.join(&cluster.name);
    // Recreate must not keep the previous cluster's admin.conf (wrong pertisk-ca →
    // kubectl "ECDSA verification failure" / unknown authority).
    if cluster_out.exists() {
        append_log(
            log_path,
            &format!(
                "clear leftover kubeconfig dir {} from a previous cluster of this name\n",
                cluster_out.display()
            ),
        )?;
        let _ = std::fs::remove_dir_all(&cluster_out);
    }
    std::fs::create_dir_all(&cluster_out)?;

    let lab_up = if provider.kind == "vsphere" {
        vsphere_lab_up_path(state)
    } else if provider.kind == "nutanix" {
        nutanix_lab_up_path(state)
    } else {
        state.cfg().lab_up.clone()
    };

    let mut cmd = Command::new(&lab_up);
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
        .arg("--max-pods")
        .arg(cluster.max_pods.to_string())
        .arg("--pod-subnet")
        .arg(&cluster.pod_subnet)
        .arg("--service-subnet")
        .arg(&cluster.service_subnet)
        .env("CLUSTER_OUT", cluster_out.display().to_string())
        .env("K8S_VER", &cluster.k8s_version)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if provider.kind == "vsphere" {
        cmd.env("PROVIDER_KIND", "vsphere")
            .env("VSPHERE_URL", &provider.url)
            .env("VSPHERE_USER", &provider.token_id)
            .env("VSPHERE_PASSWORD", &secret)
            .env("VSPHERE_HOST", &provider.node)
            .env("VSPHERE_DATASTORE", &provider.storage)
            .env("VSPHERE_NETWORK", &provider.bridge);
        if provider.insecure != 0 {
            cmd.env("VSPHERE_INSECURE", "1");
        }
    } else if provider.kind == "nutanix" {
        cmd.env("PROVIDER_KIND", "nutanix")
            .env("NUTANIX_URL", &provider.url)
            .env("NUTANIX_USER", &provider.token_id)
            .env("NUTANIX_PASSWORD", &secret)
            .env("NUTANIX_CLUSTER", &provider.node)
            .env("NUTANIX_STORAGE", &provider.storage)
            .env("NUTANIX_NETWORK", &provider.bridge)
            .env(
                "NUTANIX_MAC_SALT",
                format!("{}|{}", provider.url.trim_end_matches('/'), provider.node),
            );
        if provider.insecure != 0 {
            cmd.env("NUTANIX_INSECURE", "1");
        }
    } else {
        cmd.env("PROXMOX_URL", &provider.url)
            .env("PROXMOX_TOKEN_ID", &provider.token_id)
            .env("PROXMOX_TOKEN_SECRET", &secret)
            .env("PROXMOX_NODE", &provider.node)
            // Keep guest MACs unique across Proxmox hosts on the same LAN
            // (MAC = OUI + salt(url|node) + VMID); see proxmox-upload-vm.sh.
            .env(
                "PROXMOX_MAC_SALT",
                format!("{}|{}", provider.url.trim_end_matches('/'), provider.node),
            )
            .env("PROXMOX_STORAGE", &provider.storage)
            .env("PROXMOX_BRIDGE", &provider.bridge);
        if provider.insecure != 0 {
            cmd.env("PROXMOX_INSECURE", "1");
        }
    }

    // Auto-detect static IPs for provider if not already operator-configured.
    // Extract provider IP, infer /24 subnet, scan for free IPs.
    let need_ips = (cluster.controlplanes + cluster.workers) as i64;
    if let Some(provider_ip) = ip_from_provider_url(&provider.url) {
        if let Some(subnet) = subnet_from_provider_ip(&provider_ip) {
            // Collect all provider IPs + all existing node IPs for exclusion.
            let mut exclude_ips: Vec<String> = Vec::new();
            let urls: Vec<String> = sqlx::query_scalar("SELECT url FROM providers")
                .fetch_all(state.pool())
                .await
                .unwrap_or_default();
            exclude_ips.extend(urls.iter().filter_map(|u| ip_from_provider_url(u)));

            let node_ips: Vec<Option<String>> =
                sqlx::query_scalar("SELECT ip FROM nodes WHERE ip IS NOT NULL")
                    .fetch_all(state.pool())
                    .await
                    .unwrap_or_default();
            exclude_ips.extend(node_ips.into_iter().flatten());

            let node_ip6s: Vec<Option<String>> =
                sqlx::query_scalar("SELECT ip6 FROM nodes WHERE ip6 IS NOT NULL")
                    .fetch_all(state.pool())
                    .await
                    .unwrap_or_default();
            exclude_ips.extend(node_ip6s.into_iter().flatten());

            // Also exclude this cluster's VIP and VIP6 (reserved for kube-vip).
            if let Some(vip) = &cluster.vip {
                if !vip.is_empty() {
                    exclude_ips.push(vip.clone());
                }
            }
            if let Some(vip6) = &cluster.vip6 {
                if !vip6.is_empty() {
                    exclude_ips.push(vip6.clone());
                }
            }

            exclude_ips.sort();
            exclude_ips.dedup();

            match provider.kind.as_str() {
                "nutanix" | "ahv" | "prism" => {
                    // For Nutanix, check if operator set NUTANIX_STATIC_IPS; else auto-detect.
                    let has_nutanix_static = std::env::var("NUTANIX_STATIC_IPS")
                        .map(|v| !v.trim().is_empty())
                        .unwrap_or(false);
                    if !has_nutanix_static {
                        // Use IPAM's requested_ip_address to reserve specific IPs.
                        match scan_subnet_for_free_ips(&subnet, need_ips, &exclude_ips).await {
                            Ok(free_ips) if !free_ips.is_empty() => {
                                let ips_to_use = free_ips[..std::cmp::min(need_ips as usize, free_ips.len())].to_vec();
                                cmd.env("NUTANIX_STATIC_IPS", ips_to_use.join(" "));
                                append_log(
                                    log_path,
                                    &format!(
                                        "=> auto-assigned Nutanix IPAM reserved IPs: {}\n",
                                        ips_to_use.join(", ")
                                    ),
                                )?;
                            }
                            _ => {
                                append_log(
                                    log_path,
                                    &format!(
                                        "=> could not auto-assign IPs in {}, falling back to DHCP\n",
                                        subnet
                                    ),
                                )?;
                            }
                        }
                    }
                }
                "vsphere" | "esxi" => {
                    // For vSphere, check if operator set VSPHERE_STATIC_IPS; else auto-detect.
                    let has_vsphere_static = std::env::var("VSPHERE_STATIC_IPS")
                        .map(|v| !v.trim().is_empty())
                        .unwrap_or(false);
                    if !has_vsphere_static {
                        // Scan and pass IPs to create-cluster-vms via VSPHERE_STATIC_IPS.
                        match scan_subnet_for_free_ips(&subnet, need_ips, &exclude_ips).await {
                            Ok(free_ips) if !free_ips.is_empty() => {
                                let ips_to_use = free_ips[..std::cmp::min(need_ips as usize, free_ips.len())].to_vec();
                                cmd.env("VSPHERE_STATIC_IPS", ips_to_use.join(" "));
                                append_log(
                                    log_path,
                                    &format!(
                                        "=> auto-assigned vSphere guest IPs: {}\n",
                                        ips_to_use.join(", ")
                                    ),
                                )?;
                            }
                            _ => {
                                append_log(
                                    log_path,
                                    &format!(
                                        "=> could not auto-assign IPs in {}, falling back to DHCP\n",
                                        subnet
                                    ),
                                )?;
                            }
                        }
                    }
                }
                _ => {
                    // Proxmox: Auto-scan for free IPs and use static assignment (if not operator-configured).
                    let has_proxmox_static = std::env::var("PROXMOX_STATIC_BASE")
                        .or_else(|_| std::env::var("PROXMOX_STATIC_SUBNET"))
                        .or_else(|_| std::env::var("PROXMOX_STATIC_IPS"))
                        .map(|v| !v.trim().is_empty())
                        .unwrap_or(false);
                    if !has_proxmox_static {
                        // Scan subnet for free IPs and assign directly (avoids rejection loop when DHCP limited).
                        match scan_subnet_for_free_ips(&subnet, need_ips, &exclude_ips).await {
                            Ok(free_ips) if !free_ips.is_empty() => {
                                let ips_to_use = free_ips[..std::cmp::min(need_ips as usize, free_ips.len())].to_vec();
                                cmd.env("PROXMOX_STATIC_IPS", ips_to_use.join(" "));
                                // Auto-detect gateway: operator env > detect from system route > derive from subnet.
                                let gw = std::env::var("PROXMOX_STATIC_GATEWAY")
                                    .or_else(|_| std::env::var("LAB_GATEWAY"))
                                    .ok()
                                    .filter(|s| !s.is_empty())
                                    .or_else(detect_default_gateway)
                                    .or_else(|| gateway_from_subnet(&subnet))
                                    .unwrap_or_else(|| "10.1.1.1".to_string());
                                cmd.env("PROXMOX_STATIC_GATEWAY", &gw);
                                append_log(
                                    log_path,
                                    &format!(
                                        "=> auto-assigned Proxmox static IPs (TCP-scanned free): {} gateway={}\n",
                                        ips_to_use.join(", "),
                                        gw
                                    ),
                                )?;
                            }
                            _ => {
                                // Fallback to exclusion-based approach if scan fails.
                                cmd.env("PROXMOX_STATIC_EXCLUDE", exclude_ips.join(","));
                                append_log(
                                    log_path,
                                    &format!(
                                        "=> could not auto-scan, falling back to exclusion-based DHCP: {}\n",
                                        exclude_ips.join(", ")
                                    ),
                                )?;
                            }
                        }
                    }
                }
            }
        }
    }


    if let Some(note) = apply_lab_env(&mut cmd, state, &provider.url) {
        append_log(log_path, &note)?;
    }
    auto_exclude_provider_hosts(&mut cmd, state).await;

    let mode = cluster.network_mode.to_ascii_lowercase();
    let dual = mode == "dual-stack" || mode == "ipv6";
    if dual {
        cmd.arg("--dual-stack");
        let pod_v6 = cluster
            .pod_subnet_ipv6
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("2001:db8:10:0::/56");
        let svc_v6 = cluster
            .service_subnet_ipv6
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("2001:db8:96:1::/112");
        cmd.arg("--pod-subnet-ipv6")
            .arg(pod_v6)
            .arg("--service-subnet-ipv6")
            .arg(svc_v6);
    }
    // Guest arch from cluster (UI). Optional ops override: PERTISK_ARCH=arm64|amd64.
    let guest_arch = std::env::var("PERTISK_ARCH")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cluster.arch.clone());
    let guest_arch = match guest_arch.to_ascii_lowercase().as_str() {
        "arm64" | "aarch64" => "arm64",
        _ => "amd64",
    };
    cmd.arg("--arch").arg(guest_arch);
    cmd.env("PERTISK_ARCH", guest_arch).env("ARCH", guest_arch);
    if guest_arch == "arm64" {
        apply_proxmox_arm64_create_env(&mut cmd, log_path, &provider, &secret).await?;
    }
    // VIP / kube-vip is HA-only (controlplanes > 1). Single-CP uses the node IP.
    if cluster.controlplanes > 1 {
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
    }

    append_log(log_path, &format!("$ {:?}\n", cmd.as_std().get_program()))?;
    append_log(
        log_path,
        &format!(
            "create cluster={} arch={} cps={} workers={} k8s={} network={} pod={} svc={} vip={:?} vip6={:?}\n",
            cluster.name,
            guest_arch,
            cluster.controlplanes,
            cluster.workers,
            cluster.k8s_version,
            mode,
            cluster.pod_subnet,
            cluster.service_subnet,
            cluster.vip,
            cluster.vip6
        ),
    )?;

    // Persist planned base VMID so seed + UI show correct IDs during create.
    {
        let now = db::now_rfc3339();
        let _ = sqlx::query("UPDATE clusters SET cp_vmid = ?, updated_at = ? WHERE id = ?")
            .bind(cp_vmid)
            .bind(&now)
            .bind(cid)
            .execute(state.pool())
            .await;
    }
    let mut cluster = cluster;
    cluster.cp_vmid = Some(cp_vmid);

    // Show node list immediately (provisioning) while Proxmox VMs are created.
    seed_stub_nodes(state, &cluster, "provisioning").await?;
    append_log(
        log_path,
        &format!(
            "seeded {} CP + {} worker node rows (status=provisioning)\n",
            cluster.controlplanes, cluster.workers
        ),
    )?;

    // If lab-up script missing: optional UI/dev stub, otherwise fail clearly.
    if !lab_up.exists() {
        let allow_stub = std::env::var("MGMT_ALLOW_LAB_STUB")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if !allow_stub {
            let path = lab_up.display();
            let _ = mark_nodes_status(state.pool(), cid, "provisioning", "error").await;
            let _ = mark_nodes_status(state.pool(), cid, "pending", "error").await;
            anyhow::bail!(
                "lab-up script not found at {path}\n\
                 Set MGMT_LAB_UP to an absolute path (RPM default: /usr/share/pertisk-mgmt/scripts/proxmox-lab-up.sh),\n\
                 or set MGMT_ALLOW_LAB_STUB=1 for local UI-only stub."
            );
        }
        append_log(
            log_path,
            "WARNING: lab-up script not found; MGMT_ALLOW_LAB_STUB=1 — marking cluster ready (dev stub)\n",
        )?;
        seed_stub_nodes(state, &cluster, "ready").await?;
        let now = db::now_rfc3339();
        let endpoint = cluster.vip.clone().unwrap_or_else(|| "127.0.0.1".into());
        let kc = cluster_out.join("admin.conf");
        std::fs::write(&kc, "# stub kubeconfig\n")?;
        let stub_msg = format!(
            "dev stub: lab-up missing at {} — not a real cluster",
            lab_up.display()
        );
        sqlx::query(
            "UPDATE clusters SET status = 'ready', endpoint = ?, kubeconfig_path = ?, cp_vmid = ?, error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&endpoint)
        .bind(kc.to_string_lossy().as_ref())
        .bind(cp_vmid)
        .bind(&stub_msg)
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
    let pool_out = state.pool().clone();
    let pool_err = state.pool().clone();
    let cid_out = cid.to_string();
    let cid_err = cid.to_string();
    let cluster_name_out = cluster.name.clone();
    let cluster_name_err = cluster.name.clone();

    let out_task = tokio::spawn(async move {
        if let Some(out) = stdout {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = append_log(&log_path_out, &format!("{line}\n"));
                let _ =
                    apply_create_log_progress(&pool_out, &cid_out, &cluster_name_out, &line).await;
            }
        }
    });
    let err_task = tokio::spawn(async move {
        if let Some(err) = stderr {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = append_log(&log_path_err, &format!("{line}\n"));
                let _ =
                    apply_create_log_progress(&pool_err, &cid_err, &cluster_name_err, &line).await;
            }
        }
    });

    let status = child.wait().await?;
    let _ = out_task.await;
    let _ = err_task.await;

    if !status.success() {
        let _ = mark_nodes_status(state.pool(), cid, "provisioning", "error").await;
        let _ = mark_nodes_status(state.pool(), cid, "pending", "error").await;
        anyhow::bail!("lab-up exited with {status}");
    }

    let kc = resolve_kubeconfig(&cluster_out, &cluster.name, log_path)?;
    let endpoint = endpoint_from_kubeconfig(&kc)
        .or_else(|| {
            cluster
                .vip
                .clone()
                .filter(|v| !v.is_empty())
                .filter(|_| cluster.controlplanes > 1)
        })
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

    // Mark all planned nodes ready, then sync IPs / versions from kubectl.
    seed_stub_nodes(state, &cluster, "ready").await?;
    let _ =
        crate::node_sync::sync_cluster_nodes(state.pool(), cid, Some(kc.as_path()), Some(log_path))
            .await;
    match crate::k8s::approve_pending_kubelet_serving_csrs(kc.as_path()).await {
        Ok(csrs) if !csrs.is_empty() => {
            append_log(
                log_path,
                &format!("approved kubelet serving CSRs: {}\n", csrs.join(", ")),
            )?;
        }
        Err(e) => {
            append_log(
                log_path,
                &format!("warn: kubelet serving CSR approval: {e}\n"),
            )?;
        }
        _ => {}
    }
    match crate::addons::enqueue_restored_installs(state, cid).await {
        Ok(addons) if !addons.is_empty() => {
            append_log(
                log_path,
                &format!(
                    "reinstall add-ons from saved config: {}\n",
                    addons.join(", ")
                ),
            )?;
        }
        Err(e) => {
            append_log(
                log_path,
                &format!("warn: could not queue saved add-ons: {e}\n"),
            )?;
        }
        _ => {}
    }
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

/// Host from kubeconfig `server:` — preferred over DB VIP for the clusters.endpoint column.
fn endpoint_from_kubeconfig(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("server:") {
            let server = rest.trim();
            let hostport = server
                .trim_start_matches("https://")
                .trim_start_matches("http://");
            if let Some(rest) = hostport.strip_prefix('[') {
                return Some(rest.split(']').next().unwrap_or(rest).to_string());
            }
            let host = hostport.split(':').next().unwrap_or(hostport);
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }
    }
    None
}

async fn seed_stub_nodes(
    state: &AppState,
    cluster: &ClusterRow,
    status: &str,
) -> anyhow::Result<()> {
    let now = db::now_rfc3339();
    let kind: Option<String> = sqlx::query_scalar("SELECT kind FROM providers WHERE id = ?")
        .bind(&cluster.provider_id)
        .fetch_optional(state.pool())
        .await?;
    let source = crate::routes::providers::hypervisor_node_source(kind.as_deref().unwrap_or(""));
    for i in 1..=cluster.controlplanes {
        let name = format!("{}-cp-{}", cluster.name, i);
        let id = Uuid::new_v4().to_string();
        let vmid = cluster.cp_vmid.map(|v| v + i - 1);
        let _ = sqlx::query(
            r#"INSERT INTO nodes (id, cluster_id, name, role, vmid, k8s_version, memory, cores, disk_gb, source, status, created_at, updated_at)
               VALUES (?, ?, ?, 'controlplane', ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(cluster_id, name) DO UPDATE SET
                 vmid = COALESCE(excluded.vmid, nodes.vmid),
                 k8s_version = COALESCE(nodes.k8s_version, excluded.k8s_version),
                 memory = COALESCE(nodes.memory, excluded.memory),
                 cores = COALESCE(nodes.cores, excluded.cores),
                 disk_gb = COALESCE(nodes.disk_gb, excluded.disk_gb),
                 source = CASE
                   WHEN nodes.source IN ('adopted', 'baremetal') THEN nodes.source
                   ELSE excluded.source
                 END,
                 status = excluded.status,
                 updated_at = excluded.updated_at"#,
        )
        .bind(&id)
        .bind(&cluster.id)
        .bind(&name)
        .bind(vmid)
        .bind(&cluster.k8s_version)
        .bind(cluster.cp_memory)
        .bind(cluster.cp_cores)
        .bind(cluster.cp_disk_gb)
        .bind(source)
        .bind(status)
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
            r#"INSERT INTO nodes (id, cluster_id, name, role, vmid, k8s_version, memory, cores, disk_gb, source, status, created_at, updated_at)
               VALUES (?, ?, ?, 'worker', ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(cluster_id, name) DO UPDATE SET
                 vmid = COALESCE(excluded.vmid, nodes.vmid),
                 k8s_version = COALESCE(nodes.k8s_version, excluded.k8s_version),
                 memory = COALESCE(nodes.memory, excluded.memory),
                 cores = COALESCE(nodes.cores, excluded.cores),
                 disk_gb = COALESCE(nodes.disk_gb, excluded.disk_gb),
                 source = CASE
                   WHEN nodes.source IN ('adopted', 'baremetal') THEN nodes.source
                   ELSE excluded.source
                 END,
                 status = excluded.status,
                 updated_at = excluded.updated_at"#,
        )
        .bind(&id)
        .bind(&cluster.id)
        .bind(&name)
        .bind(vmid)
        .bind(&cluster.k8s_version)
        .bind(cluster.worker_memory)
        .bind(cluster.worker_cores)
        .bind(cluster.worker_disk_gb)
        .bind(source)
        .bind(status)
        .bind(&now)
        .bind(&now)
        .execute(state.pool())
        .await;
    }
    Ok(())
}

async fn mark_nodes_status(
    pool: &sqlx::SqlitePool,
    cluster_id: &str,
    from: &str,
    to: &str,
) -> anyhow::Result<()> {
    let now = db::now_rfc3339();
    sqlx::query("UPDATE nodes SET status = ?, updated_at = ? WHERE cluster_id = ? AND status = ?")
        .bind(to)
        .bind(&now)
        .bind(cluster_id)
        .bind(from)
        .execute(pool)
        .await?;
    Ok(())
}

/// Update node rows from lab-up / upload-vm log lines while create is running.
async fn apply_create_log_progress(
    pool: &sqlx::SqlitePool,
    cluster_id: &str,
    cluster_name: &str,
    line: &str,
) -> anyhow::Result<()> {
    let raw = line.trim().trim_start_matches("==> ").trim();
    if raw.is_empty() {
        return Ok(());
    }
    let now = db::now_rfc3339();

    // VIP reassigned IPv4 10.1.1.250 -> 10.1.1.254 (guest DHCP or busy LAN)
    if let Some(rest) = raw.strip_prefix("VIP reassigned IPv4 ") {
        if let Some((_, to_rest)) = rest.split_once(" -> ") {
            let to = to_rest.split_whitespace().next().unwrap_or("");
            if !to.is_empty() {
                sqlx::query("UPDATE clusters SET vip = ?, updated_at = ? WHERE id = ?")
                    .bind(to)
                    .bind(&now)
                    .bind(cluster_id)
                    .execute(pool)
                    .await?;
            }
        }
        return Ok(());
    }
    if let Some(rest) = raw.strip_prefix("VIP reassigned IPv6 ") {
        if let Some((_, to_rest)) = rest.split_once(" -> ") {
            let to = to_rest.split_whitespace().next().unwrap_or("");
            if !to.is_empty() {
                sqlx::query("UPDATE clusters SET vip6 = ?, updated_at = ? WHERE id = ?")
                    .bind(to)
                    .bind(&now)
                    .bind(cluster_id)
                    .execute(pool)
                    .await?;
            }
        }
        return Ok(());
    }

    // control-plane VMID=210 name=lab-cp-1 …
    // worker VMID=213 name=lab-wk-1 …
    if let Some((role, rest)) = raw
        .strip_prefix("control-plane ")
        .map(|r| ("controlplane", r))
        .or_else(|| raw.strip_prefix("worker ").map(|r| ("worker", r)))
    {
        if let (Some(vmid), Some(name)) = (extract_kv(rest, "VMID"), extract_kv(rest, "name")) {
            if let Ok(vmid_n) = vmid.parse::<i64>() {
                touch_node_progress(
                    pool,
                    cluster_id,
                    name,
                    role,
                    Some(vmid_n),
                    None,
                    "provisioning",
                    &now,
                )
                .await?;
            }
        }
        return Ok(());
    }

    // creating VM 210 (lab-cp-1) …
    if let Some(rest) = raw.strip_prefix("creating VM ") {
        if let Some((vmid_s, name_part)) = rest.split_once(' ') {
            if let Ok(vmid_n) = vmid_s.parse::<i64>() {
                let name = name_part
                    .trim_start_matches('(')
                    .split(')')
                    .next()
                    .unwrap_or("")
                    .trim();
                if !name.is_empty() {
                    let role = if name.contains("-cp-") {
                        "controlplane"
                    } else {
                        "worker"
                    };
                    touch_node_progress(
                        pool,
                        cluster_id,
                        name,
                        role,
                        Some(vmid_n),
                        None,
                        "provisioning",
                        &now,
                    )
                    .await?;
                }
            }
        }
        return Ok(());
    }

    // done — open Console for lab-cp-1 (vmid 210)
    if let Some(rest) = raw
        .strip_prefix("done — open Console for ")
        .or_else(|| raw.strip_prefix("done - open Console for "))
    {
        let name = rest.split_whitespace().next().unwrap_or("").trim();
        if !name.is_empty() {
            let role = if name.contains("-cp-") {
                "controlplane"
            } else {
                "worker"
            };
            touch_node_progress(
                pool,
                cluster_id,
                name,
                role,
                None,
                None,
                "provisioning",
                &now,
            )
            .await?;
        }
        return Ok(());
    }

    // VM 210 → 10.1.1.50 (API :50000 up)
    if let Some(rest) = raw.strip_prefix("VM ") {
        if let Some((vmid_s, after)) = rest.split_once('→').or_else(|| rest.split_once("->")) {
            let vmid_s = vmid_s.trim();
            let ip = after
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(|c: char| c == '(' || c == ')');
            if let Ok(vmid_n) = vmid_s.parse::<i64>() {
                if !ip.is_empty() && ip.contains('.') {
                    let _ = sqlx::query(
                        r#"UPDATE nodes SET ip = ?, status = 'provisioning', updated_at = ?
                           WHERE cluster_id = ? AND vmid = ?"#,
                    )
                    .bind(ip)
                    .bind(&now)
                    .bind(cluster_id)
                    .bind(vmid_n)
                    .execute(pool)
                    .await;
                }
            }
        }
        return Ok(());
    }

    // apply controlplane → 10.1.1.50  (first CP)
    if let Some(rest) = raw
        .strip_prefix("apply controlplane → ")
        .or_else(|| raw.strip_prefix("apply controlplane -> "))
    {
        let ip = rest.split_whitespace().next().unwrap_or("").trim();
        let name = format!("{cluster_name}-cp-1");
        if !ip.is_empty() {
            touch_node_progress(
                pool,
                cluster_id,
                &name,
                "controlplane",
                None,
                Some(ip),
                "provisioning",
                &now,
            )
            .await?;
        }
        return Ok(());
    }

    // bootstrap CP1 done (must be before the "bootstrap CP1" prefix match)
    if raw.starts_with("bootstrap CP1 done") {
        let name = format!("{cluster_name}-cp-1");
        touch_node_progress(
            pool,
            cluster_id,
            &name,
            "controlplane",
            None,
            None,
            "ready",
            &now,
        )
        .await?;
        return Ok(());
    }

    // bootstrap CP1 — still provisioning until kubeconfig / joins finish
    if raw.starts_with("bootstrap CP1") || raw == "bootstrap CP1" {
        let name = format!("{cluster_name}-cp-1");
        touch_node_progress(
            pool,
            cluster_id,
            &name,
            "controlplane",
            None,
            None,
            "provisioning",
            &now,
        )
        .await?;
        return Ok(());
    }

    // apply + join-controlplane lab-cp-2 @ 10.1.1.51
    if let Some(rest) = raw.strip_prefix("apply + join-controlplane ") {
        if let Some((name, ip_part)) = rest.split_once(" @ ") {
            let name = name.trim();
            let ip = ip_part.split_whitespace().next().unwrap_or("").trim();
            touch_node_progress(
                pool,
                cluster_id,
                name,
                "controlplane",
                None,
                if ip.is_empty() { None } else { Some(ip) },
                "ready",
                &now,
            )
            .await?;
        }
        return Ok(());
    }

    // join worker lab-wk-1 @ 10.1.1.52
    if let Some(rest) = raw.strip_prefix("join worker ") {
        if let Some((name, ip_part)) = rest.split_once(" @ ") {
            let name = name.trim();
            let ip = ip_part.split_whitespace().next().unwrap_or("").trim();
            touch_node_progress(
                pool,
                cluster_id,
                name,
                "worker",
                None,
                if ip.is_empty() { None } else { Some(ip) },
                "ready",
                &now,
            )
            .await?;
        }
        return Ok(());
    }

    Ok(())
}

fn extract_kv<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let v = rest[..end].trim();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

#[allow(clippy::too_many_arguments)]
async fn touch_node_progress(
    pool: &sqlx::SqlitePool,
    cluster_id: &str,
    name: &str,
    role: &str,
    vmid: Option<i64>,
    ip: Option<&str>,
    status: &str,
    now: &str,
) -> anyhow::Result<()> {
    // Prefer update by name (seeded stubs use {cluster}-cp-N / -wk-N).
    let updated = sqlx::query(
        r#"UPDATE nodes SET
             role = ?,
             vmid = COALESCE(?, vmid),
             ip = COALESCE(?, ip),
             status = ?,
             updated_at = ?
           WHERE cluster_id = ? AND name = ?"#,
    )
    .bind(role)
    .bind(vmid)
    .bind(ip)
    .bind(status)
    .bind(now)
    .bind(cluster_id)
    .bind(name)
    .execute(pool)
    .await?
    .rows_affected();

    if updated > 0 {
        return Ok(());
    }

    // Hypervisor name may be {cluster}-{vmid} while the stub is {cluster}-cp-1 —
    // match the existing row by VMID so we do not create duplicates.
    if let Some(v) = vmid {
        let updated_vmid = sqlx::query(
            r#"UPDATE nodes SET
                 ip = COALESCE(?, ip),
                 status = ?,
                 updated_at = ?
               WHERE cluster_id = ? AND vmid = ?"#,
        )
        .bind(ip)
        .bind(status)
        .bind(now)
        .bind(cluster_id)
        .bind(v)
        .execute(pool)
        .await?
        .rows_affected();
        if updated_vmid > 0 {
            return Ok(());
        }
    }

    let kind: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        r#"SELECT p.kind FROM clusters c
           LEFT JOIN providers p ON p.id = c.provider_id
           WHERE c.id = ?"#,
    )
    .bind(cluster_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    let source = crate::routes::providers::hypervisor_node_source(kind.as_deref().unwrap_or(""));

    let id = Uuid::new_v4().to_string();
    let _ = sqlx::query(
        r#"INSERT INTO nodes (id, cluster_id, name, role, vmid, ip, source, status, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(cluster_id, name) DO UPDATE SET
             vmid = COALESCE(excluded.vmid, nodes.vmid),
             ip = COALESCE(excluded.ip, nodes.ip),
             source = CASE
               WHEN nodes.source IN ('adopted', 'baremetal') THEN nodes.source
               ELSE excluded.source
             END,
             status = excluded.status,
             updated_at = excluded.updated_at"#,
    )
    .bind(&id)
    .bind(cluster_id)
    .bind(name)
    .bind(role)
    .bind(vmid)
    .bind(ip)
    .bind(source)
    .bind(status)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await;
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

/// Remove cluster from DB (nodes + jobs + cluster row). Ignores missing rows.
pub async fn purge_cluster_db(state: &AppState, cid: &str) -> anyhow::Result<()> {
    let _ = crate::addons::snapshot_cluster(state, cid).await;
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM clusters WHERE id = ?")
        .bind(cid)
        .fetch_optional(state.pool())
        .await?;
    if let Some(name) = name.filter(|s| !s.is_empty()) {
        let dir = state.cfg().kubeconfigs_dir().join(&name);
        if dir.is_dir() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    sqlx::query("DELETE FROM jobs WHERE cluster_id = ? AND kind != 'delete_cluster'")
        .bind(cid)
        .execute(state.pool())
        .await?;
    sqlx::query("DELETE FROM nodes WHERE cluster_id = ?")
        .bind(cid)
        .execute(state.pool())
        .await?;
    sqlx::query("DELETE FROM clusters WHERE id = ?")
        .bind(cid)
        .execute(state.pool())
        .await?;
    // delete_cluster row is SET NULL by FK; drop it so it cannot stay queued.
    sqlx::query(
        r#"DELETE FROM jobs
           WHERE cluster_id IS NULL AND kind = 'delete_cluster' AND status != 'running'"#,
    )
    .execute(state.pool())
    .await?;
    state.emit_cluster(cid, "deleted");
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

    let _ = cancel_cluster_jobs(state, &id, None).await;

    // Best-effort hypervisor cleanup (failed creates may have 0 VMs).
    match sqlx::query_as::<_, ProviderRow>(
        "SELECT id, kind, url, token_id, token_secret_enc, node, storage, bridge, insecure FROM providers WHERE id = ?",
    )
    .bind(&provider_id)
    .fetch_optional(state.pool())
    .await
    {
        Ok(Some(provider)) => {
            match crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc) {
                Ok(secret) => {
                    let cluster_name: Option<String> =
                        sqlx::query_scalar("SELECT name FROM clusters WHERE id = ?")
                            .bind(&id)
                            .fetch_optional(state.pool())
                            .await
                            .ok()
                            .flatten();
                    let prefix = cluster_name.as_deref();

                    let nodes = sqlx::query_as::<_, (String, Option<i64>)>(
                        "SELECT name, vmid FROM nodes WHERE cluster_id = ?",
                    )
                    .bind(&id)
                    .fetch_all(state.pool())
                    .await
                    .unwrap_or_default();

                    let mut vmids: Vec<i64> =
                        nodes.iter().filter_map(|(_, v)| *v).collect();
                    let node_names: Vec<String> = nodes.iter().map(|(n, _)| n.clone()).collect();
                    if vmids.is_empty() {
                        if let Some(base) = cp_vmid {
                            for i in 0..(cps + workers) {
                                vmids.push(base + i);
                            }
                        }
                    }
                    vmids.sort_unstable();
                    vmids.dedup();

                    if provider.kind == "vsphere" {
                        let client = crate::vsphere::VsphereClient::new(
                            provider.url.clone(),
                            provider.token_id.clone(),
                            secret,
                            provider.insecure != 0,
                        );
                        // Delete by DB node name first ({cluster}-cp-N), then by vmid
                        // (covers legacy {cluster}-{vmid} inventory names).
                        let mut tried = std::collections::HashSet::new();
                        for name in &node_names {
                            if !tried.insert(name.clone()) {
                                continue;
                            }
                            append_log(
                                log_path,
                                &format!("deleting VM {name} on ESXi\n"),
                            )?;
                            if let Err(e) = client.delete_vm_by_name(name).await {
                                append_log(log_path, &format!("warn: delete {name}: {e}\n"))?;
                            }
                        }
                        for vmid in vmids {
                            let legacy = crate::vsphere::VsphereClient::vm_name(prefix, vmid);
                            if !tried.insert(legacy.clone()) {
                                continue;
                            }
                            append_log(
                                log_path,
                                &format!("deleting VM {legacy} on ESXi (by vmid)\n"),
                            )?;
                            if let Err(e) = client.delete_vm(prefix, vmid).await {
                                append_log(log_path, &format!("warn: delete {legacy}: {e}\n"))?;
                            }
                        }
                    } else if provider.kind == "nutanix" {
                        let client = crate::nutanix::NutanixClient::new(
                            provider.url.clone(),
                            provider.token_id.clone(),
                            secret,
                            provider.insecure != 0,
                        );
                        let mut tried = std::collections::HashSet::new();
                        for name in &node_names {
                            if !tried.insert(name.clone()) {
                                continue;
                            }
                            append_log(
                                log_path,
                                &format!("deleting VM {name} on Nutanix\n"),
                            )?;
                            if let Err(e) = client.delete_vm_by_name(name).await {
                                append_log(log_path, &format!("warn: delete {name}: {e}\n"))?;
                            }
                        }
                        for vmid in vmids {
                            let legacy = crate::nutanix::NutanixClient::vm_name(prefix, vmid);
                            if !tried.insert(legacy.clone()) {
                                continue;
                            }
                            append_log(
                                log_path,
                                &format!("deleting VM {legacy} on Nutanix (by vmid)\n"),
                            )?;
                            if let Err(e) = client.delete_vm(prefix, vmid).await {
                                append_log(log_path, &format!("warn: delete {legacy}: {e}\n"))?;
                            }
                        }
                    } else {
                        let client = crate::proxmox::ProxmoxClient {
                            url: provider.url,
                            token_id: provider.token_id,
                            token_secret: secret,
                            insecure: provider.insecure != 0,
                        };
                        for vmid in vmids {
                            append_log(
                                log_path,
                                &format!("deleting VM {vmid} on {}\n", provider.node),
                            )?;
                            match client.delete_vm(&provider.node, vmid).await {
                                Ok(()) => {
                                    append_log(log_path, &format!("deleted or already gone: {vmid}\n"))?;
                                }
                                Err(e) => {
                                    append_log(log_path, &format!("warn: delete {vmid}: {e}\n"))?;
                                }
                            }
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
    let count = p
        .get("count")
        .and_then(|v| v.as_i64())
        .unwrap_or(1)
        .clamp(1, 16);

    let cluster = sqlx::query_as::<_, ClusterRow>(
        r#"SELECT id, name, provider_id, controlplanes, workers, vip, vip6, cni, k8s_version,
                  cp_memory, cp_cores, cp_disk_gb, worker_memory, worker_cores, worker_disk_gb, cp_vmid,
                  COALESCE(network_mode, 'ipv4') as network_mode,
                  COALESCE(max_pods, 250) as max_pods,
                  COALESCE(arch, 'amd64') as arch,
                  COALESCE(pod_subnet, '10.244.0.0/16') as pod_subnet,
                  COALESCE(service_subnet, '10.96.0.0/12') as service_subnet,
                  pod_subnet_ipv6,
                  service_subnet_ipv6
           FROM clusters WHERE id = ?"#,
    )
    .bind(cid)
    .fetch_one(state.pool())
    .await?;

    let provider = sqlx::query_as::<_, ProviderRow>(
        "SELECT id, kind, url, token_id, token_secret_enc, node, storage, bridge, insecure FROM providers WHERE id = ?",
    )
    .bind(&cluster.provider_id)
    .fetch_one(state.pool())
    .await?;
    let secret = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?;

    let (def_mem, def_cores, def_disk) = if role == "controlplane" {
        (cluster.cp_memory, cluster.cp_cores, cluster.cp_disk_gb)
    } else {
        (
            cluster.worker_memory,
            cluster.worker_cores,
            cluster.worker_disk_gb,
        )
    };
    let memory = p.get("memory").and_then(|v| v.as_i64()).unwrap_or(def_mem);
    let cores = p.get("cores").and_then(|v| v.as_i64()).unwrap_or(def_cores);
    let disk_gb = p
        .get("disk_gb")
        .and_then(|v| v.as_i64())
        .unwrap_or(def_disk);

    let cp_ip = resolve_cp_ip(state, cid, &cluster).await?;
    let cluster_out = state.cfg().kubeconfigs_dir().join(&cluster.name);
    std::fs::create_dir_all(&cluster_out)?;
    let add_script = add_node_script_path(state, &provider.kind);

    let now = db::now_rfc3339();
    sqlx::query("UPDATE clusters SET status = 'provisioning', updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(cid)
        .execute(state.pool())
        .await?;

    append_log(
        log_path,
        &format!(
            "add {count}× {role} → cluster={} (cp_api={cp_ip}, {cores}vCPU {memory}MB {disk_gb}GiB)\n",
            cluster.name
        ),
    )?;

    if !add_script.exists() {
        anyhow::bail!(
            "add-node script missing at {} — cannot provision VMs",
            add_script.display()
        );
    }

    let mode = cluster.network_mode.to_ascii_lowercase();
    let want_ip6 = mode == "dual-stack" || mode == "ipv6";
    let mut joined_names: Vec<String> = Vec::new();

    for n in 1..=count {
        let now = db::now_rfc3339();
        let (name, vmid, cp_index) = if role == "controlplane" {
            let (cps_now,): (i64,) =
                sqlx::query_as("SELECT controlplanes FROM clusters WHERE id = ?")
                    .bind(cid)
                    .fetch_one(state.pool())
                    .await?;
            let new_cps = cps_now + 1;
            if new_cps % 2 == 0 {
                append_log(
                    log_path,
                    "WARNING: even control-plane count reduces etcd quorum safety\n",
                )?;
            }
            let name = format!("{}-cp-{new_cps}", cluster.name);
            let vmid = cluster
                .cp_vmid
                .map(|b| b + new_cps - 1)
                .unwrap_or(210 + new_cps - 1);
            sqlx::query("UPDATE clusters SET controlplanes = ?, updated_at = ? WHERE id = ?")
                .bind(new_cps)
                .bind(&now)
                .bind(cid)
                .execute(state.pool())
                .await?;
            (name, vmid, Some(new_cps))
        } else {
            let (cps_now, workers_now): (i64, i64) =
                sqlx::query_as("SELECT controlplanes, workers FROM clusters WHERE id = ?")
                    .bind(cid)
                    .fetch_one(state.pool())
                    .await?;
            let new_w = workers_now + 1;
            let name = format!("{}-wk-{new_w}", cluster.name);
            let base = cluster.cp_vmid.unwrap_or(210) + cps_now;
            let vmid = base + new_w - 1;
            sqlx::query("UPDATE clusters SET workers = ?, updated_at = ? WHERE id = ?")
                .bind(new_w)
                .bind(&now)
                .bind(cid)
                .execute(state.pool())
                .await?;
            (name, vmid, None)
        };

        let node_source = crate::routes::providers::hypervisor_node_source(&provider.kind);
        let node_id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"INSERT INTO nodes (id, cluster_id, name, role, vmid, memory, cores, disk_gb, k8s_version, source, status, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'provisioning', ?, ?)"#,
        )
        .bind(&node_id)
        .bind(cid)
        .bind(&name)
        .bind(role)
        .bind(vmid)
        .bind(memory)
        .bind(cores)
        .bind(disk_gb)
        .bind(&cluster.k8s_version)
        .bind(node_source)
        .bind(&now)
        .bind(&now)
        .execute(state.pool())
        .await?;

        append_log(
            log_path,
            &format!(
                "==> [{n}/{count}] provisioning {name} (vmid={vmid}) — create VM → wait IP → join\n"
            ),
        )?;

        let mut cmd = Command::new(&add_script);
        cmd.arg("--role")
            .arg(role)
            .arg("--vmid")
            .arg(vmid.to_string())
            .arg("--name")
            .arg(&name)
            .arg("--memory")
            .arg(memory.to_string())
            .arg("--cores")
            .arg(cores.to_string())
            .arg("--disk-gb")
            .arg(disk_gb.to_string())
            .arg("--cluster-out")
            .arg(&cluster_out)
            .arg("--cluster-name")
            .arg(&cluster.name)
            .arg("--cp-ip")
            .arg(&cp_ip)
            .arg("--bridge")
            .arg(&provider.bridge)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(note) = apply_lab_env(&mut cmd, state, &provider.url) {
            append_log(log_path, &note)?;
        }
        auto_exclude_provider_hosts(&mut cmd, state).await;

        if provider.kind == "nutanix" || provider.kind == "ahv" || provider.kind == "prism" {
            cmd.env("PROVIDER_KIND", "nutanix")
                .env("NUTANIX_URL", &provider.url)
                .env("NUTANIX_USER", &provider.token_id)
                .env("NUTANIX_PASSWORD", &secret)
                .env("NUTANIX_CLUSTER", &provider.node)
                .env("NUTANIX_STORAGE", &provider.storage)
                .env("NUTANIX_NETWORK", &provider.bridge)
                .env(
                    "NUTANIX_MAC_SALT",
                    format!("{}|{}", provider.url.trim_end_matches('/'), provider.node),
                );
            if provider.insecure != 0 {
                cmd.env("NUTANIX_INSECURE", "1");
            }
        } else if provider.kind == "vsphere" || provider.kind == "esxi" {
            cmd.env("PROVIDER_KIND", "vsphere")
                .env("VSPHERE_URL", &provider.url)
                .env("VSPHERE_USER", &provider.token_id)
                .env("VSPHERE_PASSWORD", &secret)
                .env("VSPHERE_HOST", &provider.node)
                .env("VSPHERE_DATASTORE", &provider.storage)
                .env("VSPHERE_NETWORK", &provider.bridge);
            if provider.insecure != 0 {
                cmd.env("VSPHERE_INSECURE", "1");
            }
        } else {
            cmd.env("PROXMOX_URL", &provider.url)
                .env("PROXMOX_TOKEN_ID", &provider.token_id)
                .env("PROXMOX_TOKEN_SECRET", &secret)
                .env("PROXMOX_NODE", &provider.node)
                .env(
                    "PROXMOX_MAC_SALT",
                    format!("{}|{}", provider.url.trim_end_matches('/'), provider.node),
                )
                .env("PROXMOX_STORAGE", &provider.storage)
                .env("PROXMOX_BRIDGE", &provider.bridge);
            if provider.insecure != 0 {
                cmd.env("PROXMOX_INSECURE", "1");
            }
        }
        let node_arch = match cluster.arch.to_ascii_lowercase().as_str() {
            "arm64" | "aarch64" => "arm64",
            _ => "amd64",
        };
        cmd.arg("--arch").arg(node_arch);
        cmd.env("PERTISK_ARCH", node_arch).env("ARCH", node_arch);
        if node_arch == "arm64" {
            apply_proxmox_arm64_create_env(&mut cmd, log_path, &provider, &secret).await?;
        }
        // First-boot EPHEMERAL mkfs on large worker disks can exceed 7+ minutes on
        // older images; give wait_ip headroom proportional to disk size.
        let api_after_ip = (disk_gb * 15).clamp(900, 1800);
        cmd.env("API_AFTER_IP_TIMEOUT", api_after_ip.to_string());
        if let Some(idx) = cp_index {
            cmd.arg("--controlplane-index").arg(idx.to_string());
        }

        match stream_command(&mut cmd, log_path).await {
            Ok(output) => {
                let ip = output
                    .lines()
                    .rev()
                    .find_map(|l| l.strip_prefix("NODE_IP=").map(str::to_string));
                let now = db::now_rfc3339();
                sqlx::query(
                    "UPDATE nodes SET status = 'ready', ip = COALESCE(?, ip), updated_at = ? WHERE id = ?",
                )
                .bind(&ip)
                .bind(&now)
                .bind(&node_id)
                .execute(state.pool())
                .await?;
                joined_names.push(name.clone());
                append_log(
                    log_path,
                    &format!(
                        "ready {name}{}\n",
                        ip.as_ref().map(|i| format!(" @ {i}")).unwrap_or_default()
                    ),
                )?;
            }
            Err(e) => {
                let now = db::now_rfc3339();
                // Keep the optimistic workers/CP bump + error node row so Remove
                // can delete the orphaned Proxmox VM; a count rollback would reuse
                // the same vmid while the guest still exists.
                let _ =
                    sqlx::query("UPDATE nodes SET status = 'error', updated_at = ? WHERE id = ?")
                        .bind(&now)
                        .bind(&node_id)
                        .execute(state.pool())
                        .await;
                let _ = sqlx::query(
                    "UPDATE clusters SET status = 'ready', error = ?, updated_at = ? WHERE id = ?",
                )
                .bind(e.to_string())
                .bind(&now)
                .bind(cid)
                .execute(state.pool())
                .await;
                return Err(e);
            }
        }
    }

    let kc = cluster_out.join("admin.conf");
    if kc.is_file() {
        for name in &joined_names {
            append_log(
                log_path,
                &format!(
                    "wait for kubectl addresses on {name}{}\n",
                    if want_ip6 { " (incl. IPv6)" } else { "" }
                ),
            )?;
            match crate::node_sync::wait_node_addresses(
                &kc,
                name,
                want_ip6,
                std::time::Duration::from_secs(if want_ip6 { 300 } else { 180 }),
            )
            .await
            {
                Ok(snap) => {
                    let _ =
                        crate::node_sync::persist_snapshot_by_name(state.pool(), cid, name, &snap)
                            .await;
                    append_log(
                        log_path,
                        &format!(
                            "synced {name} ip={} ip6={}\n",
                            snap.ip.as_deref().unwrap_or("—"),
                            snap.ip6.as_deref().unwrap_or("—")
                        ),
                    )?;
                }
                Err(e) => {
                    append_log(log_path, &format!("warn: wait addresses {name}: {e}\n"))?;
                }
            }
            append_log(
                log_path,
                &format!("approve kubelet serving certificate for {name}\n"),
            )?;
            match crate::k8s::wait_kubelet_serving_cert(
                &kc,
                name,
                std::time::Duration::from_secs(120),
            )
            .await
            {
                Ok(()) => {
                    append_log(
                        log_path,
                        &format!("kubelet serving cert issued for {name}\n"),
                    )?;
                }
                Err(e) => {
                    append_log(
                        log_path,
                        &format!("warn: kubelet serving cert {name}: {e}\n"),
                    )?;
                }
            }
        }
        let _ = crate::node_sync::sync_cluster_nodes(state.pool(), cid, Some(&kc), Some(log_path))
            .await;
        let _ = crate::k8s::approve_pending_kubelet_serving_csrs(&kc).await;
    }

    let now = db::now_rfc3339();
    sqlx::query("UPDATE clusters SET status = 'ready', error = NULL, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(cid)
        .execute(state.pool())
        .await?;
    append_log(log_path, "add-node complete\n")?;
    Ok(())
}

fn add_node_script_path(state: &AppState, kind: &str) -> PathBuf {
    let name = if matches!(kind, "nutanix" | "ahv" | "prism") {
        "nutanix-add-node.sh"
    } else if matches!(kind, "vsphere" | "esxi") {
        "vsphere-add-node.sh"
    } else {
        "proxmox-add-node.sh"
    };
    let beside = state.cfg().lab_up.parent().map(|p| p.join(name));
    if let Some(p) = beside {
        if p.exists() {
            return p;
        }
    }
    // Prefer sibling of configured lab-up even if missing (clearer error path).
    if let Some(p) = state.cfg().lab_up.parent().map(|d| d.join(name)) {
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(format!("./scripts/{name}"))
}

fn adopt_node_script_path(state: &AppState) -> PathBuf {
    let beside = state.cfg().lab_up.parent().map(|p| p.join("adopt-node.sh"));
    if let Some(p) = beside {
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("./scripts/adopt-node.sh")
}

async fn run_adopt_node(
    state: &AppState,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: serde_json::Value = serde_json::from_str(payload)?;
    let role = p.get("role").and_then(|v| v.as_str()).unwrap_or("worker");
    let node_ip = p
        .get("ip")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("ip required"))?
        .to_string();
    let source = p
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("adopted");
    let custom_name = p
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let cluster = sqlx::query_as::<_, ClusterRow>(
        r#"SELECT id, name, provider_id, controlplanes, workers, vip, vip6, cni, k8s_version,
                  cp_memory, cp_cores, cp_disk_gb, worker_memory, worker_cores, worker_disk_gb, cp_vmid,
                  COALESCE(network_mode, 'ipv4') as network_mode,
                  COALESCE(max_pods, 250) as max_pods,
                  COALESCE(arch, 'amd64') as arch,
                  COALESCE(pod_subnet, '10.244.0.0/16') as pod_subnet,
                  COALESCE(service_subnet, '10.96.0.0/12') as service_subnet,
                  pod_subnet_ipv6,
                  service_subnet_ipv6
           FROM clusters WHERE id = ?"#,
    )
    .bind(cid)
    .fetch_one(state.pool())
    .await?;

    let cp_ip = resolve_cp_ip(state, cid, &cluster).await?;
    let cluster_out = state.cfg().kubeconfigs_dir().join(&cluster.name);
    std::fs::create_dir_all(&cluster_out)?;
    let adopt_script = adopt_node_script_path(state);
    if !adopt_script.exists() {
        anyhow::bail!(
            "adopt-node script missing at {} — cannot join existing nodes",
            adopt_script.display()
        );
    }

    let now = db::now_rfc3339();
    sqlx::query("UPDATE clusters SET status = 'provisioning', updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(cid)
        .execute(state.pool())
        .await?;

    append_log(
        log_path,
        &format!(
            "adopt {role} @ {node_ip} → cluster={} (cp_api={cp_ip}, source={source})\n",
            cluster.name
        ),
    )?;

    let now = db::now_rfc3339();
    let (name, cp_index) = if role == "controlplane" {
        let (cps_now,): (i64,) = sqlx::query_as("SELECT controlplanes FROM clusters WHERE id = ?")
            .bind(cid)
            .fetch_one(state.pool())
            .await?;
        let new_cps = cps_now + 1;
        if new_cps % 2 == 0 {
            append_log(
                log_path,
                "WARNING: even control-plane count reduces etcd quorum safety\n",
            )?;
        }
        let name = custom_name
            .clone()
            .unwrap_or_else(|| format!("{}-cp-{new_cps}", cluster.name));
        sqlx::query("UPDATE clusters SET controlplanes = ?, updated_at = ? WHERE id = ?")
            .bind(new_cps)
            .bind(&now)
            .bind(cid)
            .execute(state.pool())
            .await?;
        (name, Some(new_cps))
    } else {
        let (workers_now,): (i64,) = sqlx::query_as("SELECT workers FROM clusters WHERE id = ?")
            .bind(cid)
            .fetch_one(state.pool())
            .await?;
        let new_w = workers_now + 1;
        let name = custom_name
            .clone()
            .unwrap_or_else(|| format!("{}-wk-{new_w}", cluster.name));
        sqlx::query("UPDATE clusters SET workers = ?, updated_at = ? WHERE id = ?")
            .bind(new_w)
            .bind(&now)
            .bind(cid)
            .execute(state.pool())
            .await?;
        (name, None)
    };

    // Unique name check
    let clash: Option<(String,)> =
        sqlx::query_as("SELECT id FROM nodes WHERE cluster_id = ? AND name = ?")
            .bind(cid)
            .bind(&name)
            .fetch_optional(state.pool())
            .await?;
    if clash.is_some() {
        // Roll back count bump
        if role == "controlplane" {
            let _ = sqlx::query(
                "UPDATE clusters SET controlplanes = controlplanes - 1, updated_at = ? WHERE id = ?",
            )
            .bind(db::now_rfc3339())
            .bind(cid)
            .execute(state.pool())
            .await;
        } else {
            let (w,): (i64,) = sqlx::query_as("SELECT workers FROM clusters WHERE id = ?")
                .bind(cid)
                .fetch_one(state.pool())
                .await
                .unwrap_or((1,));
            let _ = sqlx::query("UPDATE clusters SET workers = ?, updated_at = ? WHERE id = ?")
                .bind((w - 1).max(0))
                .bind(db::now_rfc3339())
                .bind(cid)
                .execute(state.pool())
                .await;
        }
        anyhow::bail!("node name {name} already exists on this cluster");
    }

    let (def_mem, def_cores, def_disk) = if role == "controlplane" {
        (cluster.cp_memory, cluster.cp_cores, cluster.cp_disk_gb)
    } else {
        (
            cluster.worker_memory,
            cluster.worker_cores,
            cluster.worker_disk_gb,
        )
    };

    let node_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"INSERT INTO nodes (id, cluster_id, name, role, vmid, ip, memory, cores, disk_gb, k8s_version, source, status, created_at, updated_at)
           VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, 'provisioning', ?, ?)"#,
    )
    .bind(&node_id)
    .bind(cid)
    .bind(&name)
    .bind(role)
    .bind(&node_ip)
    .bind(def_mem)
    .bind(def_cores)
    .bind(def_disk)
    .bind(&cluster.k8s_version)
    .bind(source)
    .bind(&now)
    .bind(&now)
    .execute(state.pool())
    .await?;

    append_log(
        log_path,
        &format!("==> joining existing {name} @ {node_ip} (no VM create)\n"),
    )?;

    let mut cmd = Command::new(&adopt_script);
    cmd.arg("--role")
        .arg(role)
        .arg("--name")
        .arg(&name)
        .arg("--node-ip")
        .arg(&node_ip)
        .arg("--cp-ip")
        .arg(&cp_ip)
        .arg("--cluster-out")
        .arg(&cluster_out)
        .arg("--cluster-name")
        .arg(&cluster.name)
        .env("PERTISKCTL", state.cfg().pertiskctl.display().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(root) = state.cfg().lab_up.parent().and_then(|p| p.parent()) {
        cmd.env("PERTISK_ROOT", root.display().to_string());
    }
    if let Some(idx) = cp_index {
        cmd.arg("--controlplane-index").arg(idx.to_string());
    }

    let mode = cluster.network_mode.to_ascii_lowercase();
    let want_ip6 = mode == "dual-stack" || mode == "ipv6";

    match stream_command(&mut cmd, log_path).await {
        Ok(_output) => {
            let now = db::now_rfc3339();
            sqlx::query("UPDATE nodes SET status = 'ready', ip = ?, updated_at = ? WHERE id = ?")
                .bind(&node_ip)
                .bind(&now)
                .bind(&node_id)
                .execute(state.pool())
                .await?;
            append_log(log_path, &format!("ready {name} @ {node_ip}\n"))?;
        }
        Err(e) => {
            let now = db::now_rfc3339();
            let _ = sqlx::query("UPDATE nodes SET status = 'error', updated_at = ? WHERE id = ?")
                .bind(&now)
                .bind(&node_id)
                .execute(state.pool())
                .await;
            let _ = sqlx::query(
                "UPDATE clusters SET status = 'ready', error = ?, updated_at = ? WHERE id = ?",
            )
            .bind(e.to_string())
            .bind(&now)
            .bind(cid)
            .execute(state.pool())
            .await;
            return Err(e);
        }
    }

    let kc = cluster_out.join("admin.conf");
    if kc.is_file() {
        append_log(
            log_path,
            &format!(
                "wait for kubectl addresses on {name}{}\n",
                if want_ip6 { " (incl. IPv6)" } else { "" }
            ),
        )?;
        match crate::node_sync::wait_node_addresses(
            &kc,
            &name,
            want_ip6,
            std::time::Duration::from_secs(if want_ip6 { 300 } else { 180 }),
        )
        .await
        {
            Ok(snap) => {
                let _ =
                    crate::node_sync::persist_snapshot_by_id(state.pool(), &node_id, &snap).await;
                append_log(
                    log_path,
                    &format!(
                        "synced {name} ip={} ip6={}\n",
                        snap.ip.as_deref().unwrap_or("—"),
                        snap.ip6.as_deref().unwrap_or("—")
                    ),
                )?;
            }
            Err(e) => {
                append_log(log_path, &format!("warn: wait addresses {name}: {e}\n"))?;
            }
        }
        append_log(
            log_path,
            &format!("approve kubelet serving certificate for {name}\n"),
        )?;
        match crate::k8s::wait_kubelet_serving_cert(&kc, &name, std::time::Duration::from_secs(120))
            .await
        {
            Ok(()) => {
                append_log(
                    log_path,
                    &format!("kubelet serving cert issued for {name}\n"),
                )?;
            }
            Err(e) => {
                append_log(
                    log_path,
                    &format!("warn: kubelet serving cert {name}: {e}\n"),
                )?;
            }
        }
        let _ = crate::node_sync::sync_cluster_nodes(state.pool(), cid, Some(&kc), Some(log_path))
            .await;
        let _ = crate::k8s::approve_pending_kubelet_serving_csrs(&kc).await;
    }

    let now = db::now_rfc3339();
    sqlx::query("UPDATE clusters SET status = 'ready', error = NULL, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(cid)
        .execute(state.pool())
        .await?;
    append_log(log_path, "adopt-node complete\n")?;
    Ok(())
}

async fn resolve_cp_ip(
    state: &AppState,
    cid: &str,
    cluster: &ClusterRow,
) -> anyhow::Result<String> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT ip FROM nodes WHERE cluster_id = ? AND role = 'controlplane' AND ip IS NOT NULL AND ip != '' ORDER BY name ASC LIMIT 1",
    )
    .bind(cid)
    .fetch_optional(state.pool())
    .await?;
    if let Some((Some(ip),)) = row {
        return Ok(ip);
    }
    if let Some(vip) = cluster.vip.as_ref().filter(|v| !v.is_empty()) {
        return Ok(vip.clone());
    }
    anyhow::bail!("no control-plane IP found — wait until CP nodes have IPs before adding nodes")
}

async fn stream_command(cmd: &mut Command, log_path: &str) -> anyhow::Result<String> {
    append_log(log_path, &format!("$ {:?}\n", cmd.as_std().get_program()))?;
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let log_out = log_path.to_string();
    let log_err = log_path.to_string();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let tx_out = tx.clone();
    let tx_err = tx.clone();
    drop(tx);

    let out_task = tokio::spawn(async move {
        if let Some(out) = stdout {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = append_log(&log_out, &format!("{line}\n"));
                let _ = tx_out.send(line);
            }
        }
    });
    let err_task = tokio::spawn(async move {
        if let Some(err) = stderr {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = append_log(&log_err, &format!("{line}\n"));
                let _ = tx_err.send(line);
            }
        }
    });

    let status = child.wait().await?;
    let _ = out_task.await;
    let _ = err_task.await;

    let mut captured = String::new();
    while let Some(line) = rx.recv().await {
        captured.push_str(&line);
        captured.push('\n');
    }

    if !status.success() {
        anyhow::bail!("command exited with {status}");
    }
    Ok(captured)
}

async fn run_remove_node(
    state: &AppState,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: serde_json::Value = serde_json::from_str(payload)?;
    let mut ids: Vec<String> = p
        .get("node_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        if let Some(one) = p.get("node_id").and_then(|v| v.as_str()) {
            ids.push(one.to_string());
        }
    }
    if ids.is_empty() {
        anyhow::bail!("node_ids required");
    }

    for node_id in &ids {
        remove_one_node(state, cid, node_id, log_path).await?;
    }
    Ok(())
}

async fn remove_one_node(
    state: &AppState,
    cid: &str,
    node_id: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let node = sqlx::query_as::<_, (String, String, Option<i64>)>(
        "SELECT name, role, vmid FROM nodes WHERE id = ? AND cluster_id = ?",
    )
    .bind(node_id)
    .bind(cid)
    .fetch_optional(state.pool())
    .await?
    .ok_or_else(|| anyhow::anyhow!("node not found: {node_id}"))?;

    let cluster = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT controlplanes, kubeconfig_path FROM clusters WHERE id = ?",
    )
    .bind(cid)
    .fetch_one(state.pool())
    .await?;
    let (cps, kc) = cluster;

    if node.1 == "controlplane" {
        let remaining = cps - 1;
        if remaining < 1 {
            anyhow::bail!("cannot remove last control plane ({})", node.0);
        }
        let majority = cps / 2 + 1;
        if remaining < majority && cps > 1 {
            anyhow::bail!(
                "removing {} would break etcd quorum (have {cps}, need majority {majority})",
                node.0
            );
        }
    }

    // Drain + delete from Kubernetes before tearing down the VM.
    if let Some(kc) = kc.as_deref().filter(|s| !s.is_empty()) {
        if std::path::Path::new(kc).is_file() {
            append_log(log_path, &format!("drain {}\n", node.0))?;
            let _ = kubectl(
                kc,
                &[
                    "drain",
                    &node.0,
                    "--ignore-daemonsets",
                    "--delete-emptydir-data",
                    "--force",
                    "--grace-period=30",
                    "--timeout=3m",
                ],
                log_path,
            )
            .await;
            append_log(log_path, &format!("kubectl delete node {}\n", node.0))?;
            match kubectl(kc, &["delete", "node", &node.0, "--wait=true"], log_path).await {
                Ok(()) => {}
                Err(e) => {
                    append_log(
                        log_path,
                        &format!("warn: kubectl delete node {}: {e} (continuing)\n", node.0),
                    )?;
                }
            }
        } else {
            append_log(
                log_path,
                &format!("skip k8s delete (kubeconfig missing: {kc})\n"),
            )?;
        }
    } else {
        append_log(log_path, "skip k8s delete (no kubeconfig_path)\n")?;
    }

    if node.1 == "controlplane" {
        sqlx::query(
            "UPDATE clusters SET controlplanes = controlplanes - 1, updated_at = ? WHERE id = ?",
        )
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

    if let Some(vmid) = node.2 {
        if let Ok(provider) = provider_row_for_cluster(state, cid).await {
            if let Ok(secret) = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)
            {
                let cluster_name: String =
                    sqlx::query_scalar("SELECT name FROM clusters WHERE id = ?")
                        .bind(cid)
                        .fetch_one(state.pool())
                        .await
                        .unwrap_or_default();
                if provider.kind == "vsphere" {
                    append_log(log_path, &format!("deleting VM {}\n", node.0))?;
                    let client = crate::vsphere::VsphereClient::new(
                        provider.url,
                        provider.token_id,
                        secret,
                        provider.insecure != 0,
                    );
                    if let Err(e) = client.delete_vm_by_name(&node.0).await {
                        append_log(log_path, &format!("warn: delete {}: {e}\n", node.0))?;
                    }
                    // Legacy inventory name {cluster}-{vmid}
                    let legacy = crate::vsphere::VsphereClient::vm_name(Some(&cluster_name), vmid);
                    if legacy != node.0 {
                        let _ = client.delete_vm(Some(&cluster_name), vmid).await;
                    }
                } else if provider.kind == "nutanix" {
                    append_log(log_path, &format!("deleting VM {}\n", node.0))?;
                    let client = crate::nutanix::NutanixClient::new(
                        provider.url,
                        provider.token_id,
                        secret,
                        provider.insecure != 0,
                    );
                    if let Err(e) = client.delete_vm_by_name(&node.0).await {
                        append_log(log_path, &format!("warn: delete {}: {e}\n", node.0))?;
                    }
                    let legacy = crate::nutanix::NutanixClient::vm_name(Some(&cluster_name), vmid);
                    if legacy != node.0 {
                        let _ = client.delete_vm(Some(&cluster_name), vmid).await;
                    }
                } else {
                    append_log(log_path, &format!("deleting VM {vmid} ({})\n", node.0))?;
                    let client = crate::proxmox::ProxmoxClient {
                        url: provider.url,
                        token_id: provider.token_id,
                        token_secret: secret,
                        insecure: provider.insecure != 0,
                    };
                    let _ = client.delete_vm(&provider.node, vmid).await;
                }
            }
        }
    }

    sqlx::query("DELETE FROM nodes WHERE id = ?")
        .bind(node_id)
        .execute(state.pool())
        .await?;
    append_log(log_path, &format!("removed node {}\n", node.0))?;
    Ok(())
}

async fn run_resize_node(
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

    let row = sqlx::query_as::<_, (String, Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT name, vmid, memory, cores, disk_gb FROM nodes WHERE id = ? AND cluster_id = ?",
    )
    .bind(node_id)
    .bind(cid)
    .fetch_optional(state.pool())
    .await?
    .ok_or_else(|| anyhow::anyhow!("node not found"))?;

    let (name, vmid, cur_mem, cur_cores, cur_disk) = row;
    let want_mem = p.get("memory").and_then(|v| v.as_i64()).or(cur_mem);
    let want_cores = p.get("cores").and_then(|v| v.as_i64()).or(cur_cores);
    let want_disk = p.get("disk_gb").and_then(|v| v.as_i64()).or(cur_disk);

    let mut apply_disk = want_disk;
    if let Some(d) = want_disk {
        if let Some(cur) = cur_disk {
            if d < cur {
                append_log(
                    log_path,
                    &format!(
                        "warn: disk can only grow (have {cur} GiB, asked {d} GiB) — skipping disk shrink\n"
                    ),
                )?;
                apply_disk = Some(cur);
            }
        }
    }

    let vmid = vmid.ok_or_else(|| anyhow::anyhow!("node {name} has no VMID"))?;
    let provider = provider_row_for_cluster(state, cid).await?;
    let secret = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?;
    let vm_name = name.clone();

    let set_cores = if want_cores != cur_cores {
        want_cores
    } else {
        None
    };
    let set_mem = if want_mem != cur_mem { want_mem } else { None };
    let cpu_mem_changed = set_cores.is_some() || set_mem.is_some();
    let disk_requested = p.get("disk_gb").and_then(|v| v.as_i64()).is_some();

    append_log(
        log_path,
        &format!(
            "resize {name} (vmid={vmid}): cores={want_cores:?} memory={want_mem:?}MB disk={apply_disk:?}GiB\n"
        ),
    )?;

    let mut disk_grew_hypervisor = false;

    if provider.kind == "vsphere" || provider.kind == "esxi" {
        let client = crate::vsphere::VsphereClient::new(
            provider.url.clone(),
            provider.token_id.clone(),
            secret,
            provider.insecure != 0,
        );
        if cpu_mem_changed {
            append_log(
                log_path,
                "powering off VM so ESXi can apply CPU/memory (hot-plug not supported for this guest)\n",
            )?;
            client
                .set_vm_hardware(&vm_name, set_cores, set_mem)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            append_log(log_path, "updated ESXi CPU/memory config\n")?;
        }
        if let Some(want) = apply_disk {
            let actual = cur_disk.unwrap_or(0);
            if want > actual {
                client
                    .grow_vm_disk(&vm_name, want)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                append_log(log_path, &format!("grew ESXi disk {actual} → {want} GiB\n"))?;
                disk_grew_hypervisor = true;
            } else if disk_requested {
                append_log(
                    log_path,
                    &format!("ESXi disk already >= {want} GiB — will grow guest EPHEMERAL\n"),
                )?;
            }
        }
        if cpu_mem_changed {
            append_log(log_path, "restarting VM so CPU/memory take effect…\n")?;
            client
                .restart_vm_by_name(&vm_name)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            append_log(log_path, "VM restarted\n")?;
        }
    } else if provider.kind == "nutanix" {
        let client = crate::nutanix::NutanixClient::new(
            provider.url.clone(),
            provider.token_id.clone(),
            secret,
            provider.insecure != 0,
        );
        if cpu_mem_changed {
            client
                .set_vm_hardware(&vm_name, set_cores, set_mem)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            append_log(log_path, "updated Nutanix CPU/memory config\n")?;
        }
        if let Some(want) = apply_disk {
            let actual = cur_disk.unwrap_or(0);
            if want > actual {
                client
                    .grow_vm_disk(&vm_name, want)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                append_log(
                    log_path,
                    &format!("grew Nutanix disk {actual} → {want} GiB\n"),
                )?;
                disk_grew_hypervisor = true;
            } else if disk_requested {
                append_log(
                    log_path,
                    &format!("Nutanix disk already >= {want} GiB — will grow guest EPHEMERAL\n"),
                )?;
            }
        }
        if cpu_mem_changed {
            append_log(log_path, "restarting VM so CPU/memory take effect…\n")?;
            client
                .restart_vm_by_name(&vm_name)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            append_log(log_path, "VM restarted\n")?;
        }
    } else {
        let client = crate::proxmox::ProxmoxClient {
            url: provider.url.clone(),
            token_id: provider.token_id.clone(),
            token_secret: secret,
            insecure: provider.insecure != 0,
        };
        let pve_node = provider.node.clone();

        let proxmox_disk = client
            .vm_disk_gb(&pve_node, vmid)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(actual) = proxmox_disk {
            append_log(
                log_path,
                &format!("Proxmox scsi0 size={actual} GiB (db disk_gb={cur_disk:?})\n"),
            )?;
        } else {
            append_log(log_path, "Proxmox scsi0 size unknown\n")?;
        }

        if cpu_mem_changed {
            client
                .set_vm_hardware(&pve_node, vmid, set_cores, set_mem)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            append_log(log_path, "updated Proxmox CPU/memory config\n")?;
        }

        if let Some(want) = apply_disk {
            let actual = proxmox_disk.or(cur_disk).unwrap_or(0);
            if want > actual {
                client
                    .grow_vm_disk(&pve_node, vmid, want)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                append_log(
                    log_path,
                    &format!("grew Proxmox disk {actual} → {want} GiB\n"),
                )?;
                disk_grew_hypervisor = true;
            } else if disk_requested {
                append_log(
                    log_path,
                    &format!("Proxmox disk already >= {want} GiB — will grow guest EPHEMERAL\n"),
                )?;
            }
        }

        if cpu_mem_changed {
            append_log(log_path, "restarting VM so CPU/memory take effect…\n")?;
            client
                .restart_vm(&pve_node, vmid)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            append_log(log_path, "VM restarted\n")?;
        }
    }

    // Guest EPHEMERAL (/var) must expand after hypervisor disk grow.
    if disk_requested || disk_grew_hypervisor {
        let ip: Option<String> =
            sqlx::query_scalar("SELECT ip FROM nodes WHERE id = ? AND cluster_id = ?")
                .bind(node_id)
                .bind(cid)
                .fetch_optional(state.pool())
                .await?
                .flatten();
        let mut guest_ok = false;
        if let Some(ref ip) = ip {
            if state.cfg().pertiskctl.exists() {
                append_log(
                    log_path,
                    &format!("waiting for guest API {ip}:50000 then grow-disk…\n"),
                )?;
                guest_ok = wait_and_grow_guest_disk(state, ip, log_path).await?;
            } else {
                append_log(
                    log_path,
                    "pertiskctl missing — cannot grow guest EPHEMERAL via API\n",
                )?;
            }
        } else {
            append_log(
                log_path,
                "node has no IP — cannot grow guest EPHEMERAL via API\n",
            )?;
        }
        if !guest_ok {
            append_log(
                log_path,
                "grow-disk unavailable; trying offline EPHEMERAL grow via PROXMOX_SSH…\n",
            )?;
            guest_ok = offline_grow_ephemeral(state, vmid, log_path).await?;
        }
        if !guest_ok {
            append_log(
                log_path,
                "offline grow unavailable; restarting VM so boot may grow EPHEMERAL…\n",
            )?;
            if provider.kind == "vsphere" || provider.kind == "esxi" {
                let client = crate::vsphere::VsphereClient::new(
                    provider.url.clone(),
                    provider.token_id.clone(),
                    crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?,
                    provider.insecure != 0,
                );
                client
                    .restart_vm_by_name(&vm_name)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            } else if provider.kind == "nutanix" {
                let client = crate::nutanix::NutanixClient::new(
                    provider.url.clone(),
                    provider.token_id.clone(),
                    crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?,
                    provider.insecure != 0,
                );
                client
                    .restart_vm_by_name(&vm_name)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            } else {
                let secret = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?;
                let client = crate::proxmox::ProxmoxClient {
                    url: provider.url.clone(),
                    token_id: provider.token_id.clone(),
                    token_secret: secret,
                    insecure: provider.insecure != 0,
                };
                client
                    .restart_vm(&provider.node, vmid)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            append_log(log_path, "VM restarted\n")?;
            if let Some(ref ip) = ip {
                if state.cfg().pertiskctl.exists() {
                    let _ = wait_and_grow_guest_disk(state, ip, log_path).await?;
                }
            }
        }
    }

    let now = db::now_rfc3339();
    sqlx::query("UPDATE nodes SET memory = ?, cores = ?, disk_gb = ?, updated_at = ? WHERE id = ?")
        .bind(want_mem)
        .bind(want_cores)
        .bind(apply_disk)
        .bind(&now)
        .bind(node_id)
        .execute(state.pool())
        .await?;
    append_log(log_path, &format!("hardware updated for {name}\n"))?;
    Ok(())
}

/// Stop VM on Proxmox, expand GPT EPHEMERAL + resize2fs, start VM (needs PROXMOX_SSH).
async fn offline_grow_ephemeral(
    state: &AppState,
    vmid: i64,
    log_path: &str,
) -> anyhow::Result<bool> {
    let ssh = std::env::var("PROXMOX_SSH").unwrap_or_default();
    if ssh.is_empty() {
        append_log(
            log_path,
            "PROXMOX_SSH unset — cannot offline-grow EPHEMERAL (set PROXMOX_SSH=root@pve)\n",
        )?;
        return Ok(false);
    }
    let script = state
        .cfg()
        .lab_up
        .parent()
        .map(|d| d.join("proxmox-grow-ephemeral.sh"))
        .filter(|p| p.exists())
        .or_else(|| {
            let p = PathBuf::from("./scripts/proxmox-grow-ephemeral.sh");
            p.exists().then_some(p)
        })
        .or_else(|| {
            let p = PathBuf::from("/usr/share/pertisk-mgmt/scripts/proxmox-grow-ephemeral.sh");
            p.exists().then_some(p)
        });
    let Some(script) = script else {
        append_log(log_path, "proxmox-grow-ephemeral.sh not found\n")?;
        return Ok(false);
    };
    append_log(
        log_path,
        &format!(
            "offline grow via {} --vmid {vmid} (SSH {ssh})\n",
            script.display()
        ),
    )?;
    let out = Command::new(&script)
        .env("PROXMOX_SSH", &ssh)
        .args(["--vmid", &vmid.to_string()])
        .output()
        .await;
    match out {
        Ok(o) => {
            append_log(log_path, &String::from_utf8_lossy(&o.stdout))?;
            append_log(log_path, &String::from_utf8_lossy(&o.stderr))?;
            if o.status.success() {
                append_log(log_path, "offline EPHEMERAL grow ok\n")?;
                Ok(true)
            } else {
                append_log(
                    log_path,
                    &format!("offline grow failed (exit {})\n", o.status),
                )?;
                Ok(false)
            }
        }
        Err(err) => {
            append_log(log_path, &format!("offline grow spawn error: {err}\n"))?;
            Ok(false)
        }
    }
}

/// Wait for guest :50000 then run `pertiskctl grow-disk`.
async fn wait_and_grow_guest_disk(
    state: &AppState,
    ip: &str,
    log_path: &str,
) -> anyhow::Result<bool> {
    use tokio::net::TcpStream;
    use tokio::time::{sleep, timeout, Duration};

    let addr = format!("{ip}:50000");
    for i in 1..=60 {
        if timeout(Duration::from_secs(2), TcpStream::connect(&addr))
            .await
            .ok()
            .and_then(|r| r.ok())
            .is_some()
        {
            append_log(log_path, &format!("guest API up ({addr}) after ~{i}s\n"))?;
            break;
        }
        if i == 60 {
            append_log(log_path, &format!("guest API {addr} not ready\n"))?;
            return Ok(false);
        }
        sleep(Duration::from_secs(2)).await;
    }
    // Brief settle so block layer sees the resized disk.
    sleep(Duration::from_secs(3)).await;

    let out = Command::new(&state.cfg().pertiskctl)
        .args(["-e", &addr, "grow-disk"])
        .output()
        .await;
    match out {
        Ok(o) => {
            append_log(log_path, &String::from_utf8_lossy(&o.stdout))?;
            append_log(log_path, &String::from_utf8_lossy(&o.stderr))?;
            if o.status.success() {
                append_log(log_path, "guest EPHEMERAL grow-disk ok\n")?;
                Ok(true)
            } else {
                append_log(
                    log_path,
                    &format!(
                        "grow-disk failed (exit {}) — older image may lack GrowDisk RPC\n",
                        o.status
                    ),
                )?;
                Ok(false)
            }
        }
        Err(err) => {
            append_log(log_path, &format!("grow-disk spawn error: {err}\n"))?;
            Ok(false)
        }
    }
}

async fn run_reboot_node(
    state: &AppState,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: serde_json::Value = serde_json::from_str(payload)?;
    let mut ids: Vec<String> = p
        .get("node_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        if let Some(one) = p.get("node_id").and_then(|v| v.as_str()) {
            ids.push(one.to_string());
        }
    }
    if ids.is_empty() {
        anyhow::bail!("node_id or node_ids required");
    }

    let mut failures = 0usize;
    for node_id in ids {
        let row = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT name, ip FROM nodes WHERE id = ? AND cluster_id = ?",
        )
        .bind(&node_id)
        .bind(cid)
        .fetch_optional(state.pool())
        .await?
        .ok_or_else(|| anyhow::anyhow!("node not found: {node_id}"))?;

        let (name, ip) = row;
        append_log(log_path, &format!("reboot {name}\n"))?;
        let Some(ip) = ip else {
            append_log(log_path, "skip (no IP)\n")?;
            failures += 1;
            continue;
        };
        if !state.cfg().pertiskctl.exists() {
            append_log(log_path, "pertiskctl missing\n")?;
            failures += 1;
            continue;
        }

        let out = Command::new(&state.cfg().pertiskctl)
            .args(["-e", &format!("{ip}:50000"), "reboot"])
            .output()
            .await;

        match out {
            Ok(o) => {
                append_log(log_path, &String::from_utf8_lossy(&o.stdout))?;
                append_log(log_path, &String::from_utf8_lossy(&o.stderr))?;
                if !o.status.success() {
                    failures += 1;
                    append_log(
                        log_path,
                        &format!("reboot failed on {name} (exit {})\n", o.status),
                    )?;
                }
            }
            Err(err) => {
                failures += 1;
                append_log(log_path, &format!("pertiskctl error on {name}: {err}\n"))?;
            }
        }
    }
    if failures > 0 {
        anyhow::bail!("{failures} node(s) failed to reboot");
    }
    Ok(())
}

async fn provider_row_for_cluster(state: &AppState, cid: &str) -> anyhow::Result<ProviderRow> {
    let provider_id: String = sqlx::query_scalar("SELECT provider_id FROM clusters WHERE id = ?")
        .bind(cid)
        .fetch_one(state.pool())
        .await?;
    let provider = sqlx::query_as::<_, ProviderRow>(
        "SELECT id, kind, url, token_id, token_secret_enc, node, storage, bridge, insecure FROM providers WHERE id = ?",
    )
    .bind(&provider_id)
    .fetch_one(state.pool())
    .await?;
    Ok(provider)
}

#[allow(dead_code)]
async fn provider_client_for_cluster(
    state: &AppState,
    cid: &str,
) -> anyhow::Result<crate::proxmox::ProxmoxClient> {
    let provider = provider_row_for_cluster(state, cid).await?;
    let secret = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?;
    Ok(crate::proxmox::ProxmoxClient {
        url: provider.url.clone(),
        token_id: provider.token_id,
        token_secret: secret,
        insecure: provider.insecure != 0,
    })
}

#[allow(dead_code)]
async fn provider_node_for_cluster(state: &AppState, cid: &str) -> anyhow::Result<String> {
    Ok(provider_row_for_cluster(state, cid).await?.node)
}

fn vsphere_lab_up_path(state: &AppState) -> std::path::PathBuf {
    let lab = &state.cfg().lab_up;
    if let Some(dir) = lab.parent() {
        let candidate = dir.join("vsphere-lab-up.sh");
        if candidate.exists() {
            return candidate;
        }
    }
    // Fall back to shared proxmox-lab-up.sh with PROVIDER_KIND=vsphere (jobs set the env).
    lab.clone()
}

fn nutanix_lab_up_path(state: &AppState) -> std::path::PathBuf {
    let lab = &state.cfg().lab_up;
    if let Some(dir) = lab.parent() {
        let candidate = dir.join("nutanix-lab-up.sh");
        if candidate.exists() {
            return candidate;
        }
    }
    lab.clone()
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
        &format!("rolling upgrade → {version} (kubeadm-shaped: CP one-by-one, then workers)\n"),
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
    let extra = https_cp_servers(&nodes);
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
        // After a CP bumps apiserver/kubelet, VIP may blip — wait before the next node.
        if let Some(path) = kc.as_ref().filter(|s| !s.is_empty()) {
            wait_api_ready_ex(path, &extra, log_path).await?;
        }
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

async fn run_upgrade_os(
    state: &AppState,
    cluster_id: Option<&str>,
    payload: &str,
    log_path: &str,
) -> anyhow::Result<()> {
    use std::path::Path;

    let cid = cluster_id.ok_or_else(|| anyhow::anyhow!("cluster_id required"))?;
    let p: serde_json::Value = serde_json::from_str(payload)?;
    let bundle_dir = p
        .get("bundle_dir")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("bundle_dir required"))?;
    let _version = p
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let reboot = p.get("reboot").and_then(|v| v.as_bool()).unwrap_or(true);
    let filter: Option<Vec<String>> = p.get("node_ids").and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
    });

    let bundle_path = Path::new(bundle_dir);
    let verified = crate::os_upgrade::validate_bundle_dir(bundle_path)?;
    crate::os_upgrade::ensure_trust_pk(
        bundle_path,
        &[
            state.cfg().os_trust_pk(),
            PathBuf::from("/etc/pertisk-mgmt/os-trust.pk"),
        ],
    )?;
    append_log(
        log_path,
        &format!(
            "OS A/B upgrade → {verified} (bundle {bundle_dir})\nwill install {} on nodes if missing\n",
            crate::os_upgrade::HOST_TRUST_PK
        ),
    )?;

    let cluster = sqlx::query_as::<_, (String, i64, Option<String>)>(
        "SELECT name, controlplanes, kubeconfig_path FROM clusters WHERE id = ?",
    )
    .bind(cid)
    .fetch_one(state.pool())
    .await?;
    let (_cluster_name, controlplanes, kc) = cluster;
    let kc_path = kc.as_ref().filter(|s| !s.is_empty()).cloned();
    let Some(ref kc) = kc_path else {
        anyhow::bail!(
            "kubeconfig required to stage the OS bundle (privileged hostPath pod); Kubernetes is not changed"
        );
    };

    if controlplanes < 3 {
        append_log(
            log_path,
            &format!(
                "NOTE: controlplanes={controlplanes} (<3) — API may blip while a CP reboots; STATE/etcd stay on disk\n"
            ),
        )?;
    } else {
        append_log(
            log_path,
            "HA control planes (≥3): workers first, then CPs one-by-one; Kubernetes version unchanged\n",
        )?;
    }

    let _ = crate::node_sync::sync_cluster_nodes(
        state.pool(),
        cid,
        Some(Path::new(kc)),
        Some(log_path),
    )
    .await;

    let mut nodes = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT id, name, role, ip FROM nodes WHERE cluster_id = ? ORDER BY name ASC",
    )
    .bind(cid)
    .fetch_all(state.pool())
    .await?;
    let extra = https_cp_servers(&nodes);
    let all_cps: Vec<_> = nodes
        .iter()
        .filter(|(_, _, role, _)| role == "controlplane")
        .cloned()
        .collect();
    if let Some(ids) = &filter {
        nodes.retain(|(id, _, _, _)| ids.iter().any(|x| x == id));
        if nodes.is_empty() {
            anyhow::bail!("no matching nodes for node_ids filter");
        }
    }

    let mut workers: Vec<_> = nodes
        .iter()
        .filter(|(_, _, role, _)| role != "controlplane")
        .cloned()
        .collect();
    workers.sort_by(|a, b| a.1.cmp(&b.1));
    let mut cps: Vec<_> = nodes
        .iter()
        .filter(|(_, _, role, _)| role == "controlplane")
        .cloned()
        .collect();
    cps.sort_by(|a, b| a.1.cmp(&b.1));

    let api_server = match wait_api_ready_ex(kc, &extra, log_path).await {
        Ok(s) => s,
        Err(e) => {
            append_log(log_path, &format!("{e}\n"))?;
            recover_etcd_on_cp1(&state.cfg().pertiskctl, &all_cps, log_path).await?;
            wait_api_ready_ex(kc, &extra, log_path).await?
        }
    };
    if let Some(ref s) = api_server {
        append_log(
            log_path,
            &format!("using apiserver {s} for kubectl (kubeconfig VIP not required)\n"),
        )?;
    }

    for (i, (id, name, role, ip)) in workers.iter().enumerate() {
        append_log(
            log_path,
            &format!(
                "==> worker {}/{} {name} (os {verified})\n",
                i + 1,
                workers.len()
            ),
        )?;
        upgrade_os_node(
            state,
            cid,
            id,
            name,
            role,
            ip,
            kc,
            &extra,
            bundle_path,
            verified.as_str(),
            reboot,
            true,
            log_path,
        )
        .await?;
    }
    for (i, (id, name, role, ip)) in cps.iter().enumerate() {
        append_log(
            log_path,
            &format!("==> CP {}/{} {name} (os {verified})\n", i + 1, cps.len()),
        )?;
        upgrade_os_node(
            state,
            cid,
            id,
            name,
            role,
            ip,
            kc,
            &extra,
            bundle_path,
            verified.as_str(),
            reboot,
            true,
            log_path,
        )
        .await?;
    }

    append_log(
        log_path,
        "OS upgrade complete — Kubernetes version unchanged; verify: kubectl get nodes\n",
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn upgrade_os_node(
    state: &AppState,
    cluster_id: &str,
    id: &str,
    name: &str,
    _role: &str,
    ip: &Option<String>,
    kubeconfig: &str,
    extra_servers: &[String],
    bundle_dir: &std::path::Path,
    version: &str,
    reboot: bool,
    do_drain: bool,
    log_path: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE nodes SET status = 'upgrading', updated_at = ? WHERE id = ?")
        .bind(db::now_rfc3339())
        .bind(id)
        .execute(state.pool())
        .await?;

    let ip = ip
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("no IP for {name}; cannot reach Machine API"))?;

    if !state.cfg().pertiskctl.exists() {
        anyhow::bail!(
            "pertiskctl not found at {}",
            state.cfg().pertiskctl.display()
        );
    }

    let api_server = wait_api_ready_ex(kubeconfig, extra_servers, log_path).await?;
    let api_srv = api_server.as_deref();
    append_log(
        log_path,
        &format!("stage OS bundle on {name} via hostPath\n"),
    )?;
    stage_os_bundle_via_pod(
        kubeconfig,
        api_srv,
        extra_servers,
        name,
        bundle_dir,
        log_path,
    )
    .await?;

    if do_drain {
        append_log(log_path, &format!("drain {name}\n"))?;
        let _ = kubectl_srv(
            kubeconfig,
            api_srv,
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
    }

    let mut args = vec![
        "-e".to_string(),
        format!("{ip}:50000"),
        "upgrade".into(),
        "--bundle".into(),
        crate::os_upgrade::HOST_BUNDLE_DIR.to_string(),
    ];
    if reboot {
        args.push("--reboot".into());
    }
    append_log(
        log_path,
        &format!(
            "pertiskctl upgrade --bundle {}{}\n",
            crate::os_upgrade::HOST_BUNDLE_DIR,
            if reboot { " --reboot" } else { "" }
        ),
    )?;
    let out = Command::new(&state.cfg().pertiskctl)
        .args(&args)
        .output()
        .await?;
    append_log(log_path, &String::from_utf8_lossy(&out.stdout))?;
    append_log(log_path, &String::from_utf8_lossy(&out.stderr))?;
    if !out.status.success() {
        anyhow::bail!("pertiskctl upgrade failed on {name}");
    }

    let mut api_ip = ip.to_string();
    if reboot {
        append_log(
            log_path,
            &format!("wait Machine API {ip}:50000 after OS reboot\n"),
        )?;
        api_ip = wait_guest_after_os_reboot(state, cluster_id, id, name, ip, log_path).await?;
    }

    let mut marked = false;
    for attempt in 1..=8 {
        let mark = Command::new(&state.cfg().pertiskctl)
            .args(["-e", &format!("{api_ip}:50000"), "mark-boot-good"])
            .output()
            .await;
        match mark {
            Ok(o) => {
                append_log(log_path, &String::from_utf8_lossy(&o.stdout))?;
                append_log(log_path, &String::from_utf8_lossy(&o.stderr))?;
                if o.status.success() {
                    marked = true;
                    break;
                }
            }
            Err(e) => append_log(log_path, &format!("mark-boot-good: {e}\n"))?,
        }
        append_log(log_path, &format!("mark-boot-good retry {attempt}/8\n"))?;
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    if !marked {
        append_log(
            log_path,
            "WARNING: mark-boot-good failed — node may auto-rollback after 3 failed boots\n",
        )?;
    }

    let status = Command::new(&state.cfg().pertiskctl)
        .args(["-e", &format!("{api_ip}:50000"), "upgrade-status"])
        .output()
        .await;
    let mut got_ver = version.to_string();
    if let Ok(o) = status {
        let stdout = String::from_utf8_lossy(&o.stdout);
        append_log(log_path, &stdout)?;
        append_log(log_path, &String::from_utf8_lossy(&o.stderr))?;
        if let Some(v) = crate::os_upgrade::parse_upgrade_status_version(&stdout) {
            got_ver = v;
        }
    }

    let api_server = wait_api_ready_ex(kubeconfig, extra_servers, log_path).await?;
    append_log(log_path, &format!("wait Ready {name}\n"))?;
    let _ = kubectl_srv(
        kubeconfig,
        api_server.as_deref(),
        &[
            "wait",
            "--for=condition=Ready",
            &format!("node/{name}"),
            "--timeout=10m",
        ],
        log_path,
    )
    .await;
    if do_drain {
        append_log(log_path, &format!("uncordon {name}\n"))?;
        let _ = kubectl_srv(
            kubeconfig,
            api_server.as_deref(),
            &["uncordon", name],
            log_path,
        )
        .await;
    }

    let now = db::now_rfc3339();
    sqlx::query("UPDATE nodes SET status = 'ready', os_version = ?, updated_at = ? WHERE id = ?")
        .bind(&got_ver)
        .bind(&now)
        .bind(id)
        .execute(state.pool())
        .await?;
    let _ = crate::node_sync::sync_cluster_nodes(
        state.pool(),
        cluster_id,
        Some(std::path::Path::new(kubeconfig)),
        Some(log_path),
    )
    .await;
    Ok(())
}

async fn stage_os_bundle_via_pod(
    kubeconfig: &str,
    server: Option<&str>,
    extra_servers: &[String],
    node_name: &str,
    bundle_dir: &std::path::Path,
    log_path: &str,
) -> anyhow::Result<()> {
    crate::os_upgrade::validate_bundle_dir(bundle_dir)?;
    if crate::os_upgrade::bundle_trust_pk(bundle_dir).is_none() {
        anyhow::bail!(
            "os-trust.pk missing next to the bundle; make os-bundle includes it, or copy the public key onto the mgmt host"
        );
    }
    let pod = format!(
        "pertisk-os-{}",
        &Uuid::new_v4().to_string().replace('-', "")[..12]
    );
    let host = crate::os_upgrade::HOST_BUNDLE_DIR;
    let yaml = format!(
        r#"apiVersion: v1
kind: Pod
metadata:
  name: {pod}
  namespace: kube-system
  labels:
    app: pertisk-os-upgrade
spec:
  nodeName: {node_name}
  hostPID: true
  restartPolicy: Never
  tolerations:
  - operator: Exists
  containers:
  - name: stage
    image: alpine:3.20
    imagePullPolicy: IfNotPresent
    securityContext:
      privileged: true
    command: ["sleep", "600"]
    volumeMounts:
    - name: bundle
      mountPath: /bundle
  volumes:
  - name: bundle
    hostPath:
      path: {host}
      type: DirectoryOrCreate
"#
    );
    let tmp = std::env::temp_dir().join(format!("{pod}.yaml"));
    std::fs::write(&tmp, &yaml)?;

    let mut applied = false;
    let mut last_err = String::new();
    for attempt in 1..=6 {
        wait_api_ready_ex(kubeconfig, extra_servers, log_path).await?;
        let apply = kubectl_cmd(kubeconfig, server)
            .args(["apply", "--validate=false", "--request-timeout=30s", "-f"])
            .arg(&tmp)
            .output()
            .await?;
        append_log(log_path, &String::from_utf8_lossy(&apply.stdout))?;
        append_log(log_path, &String::from_utf8_lossy(&apply.stderr))?;
        if apply.status.success() {
            applied = true;
            break;
        }
        last_err = format!(
            "kubectl apply staging pod failed (attempt {attempt}): {}",
            String::from_utf8_lossy(&apply.stderr)
        );
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    let _ = std::fs::remove_file(&tmp);
    if !applied {
        anyhow::bail!("{last_err}");
    }

    let wait = kubectl_cmd(kubeconfig, server)
        .args([
            "wait",
            "--for=condition=Ready",
            &format!("pod/{pod}"),
            "-n",
            "kube-system",
            "--timeout=120s",
        ])
        .output()
        .await?;
    append_log(log_path, &String::from_utf8_lossy(&wait.stdout))?;
    append_log(log_path, &String::from_utf8_lossy(&wait.stderr))?;
    if !wait.status.success() {
        let _ = delete_os_stage_pod(kubeconfig, server, &pod, false).await;
        anyhow::bail!("staging pod {pod} not Ready on {node_name}");
    }

    let tar_files = [
        "kernel",
        "initramfs",
        "manifest.json",
        "manifest.sig",
        crate::os_upgrade::TRUST_PK_NAME,
    ];
    let mut tar = std::process::Command::new("tar")
        .current_dir(bundle_dir)
        .arg("-cf")
        .arg("-")
        .args(tar_files)
        .stdout(Stdio::piped())
        .spawn()
        .context("spawn tar")?;
    let stdout = tar
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("tar stdout"))?;
    let copy = kubectl_cmd(kubeconfig, server)
        .args([
            "exec",
            "-i",
            "-n",
            "kube-system",
            &pod,
            "--",
            "tar",
            "-C",
            "/bundle",
            "-xf",
            "-",
        ])
        .stdin(stdout)
        .output()
        .await?;
    let tar_status = tar.wait()?;
    append_log(log_path, &String::from_utf8_lossy(&copy.stdout))?;
    append_log(log_path, &String::from_utf8_lossy(&copy.stderr))?;
    if !copy.status.success() || !tar_status.success() {
        let _ = delete_os_stage_pod(kubeconfig, server, &pod, false).await;
        anyhow::bail!("copy OS bundle onto {node_name} failed");
    }

    // Write the public key through PID 1's root (real STATE). Do not apk/nsenter:
    // alpine may have no repos, and paths like /var/lib/... are not visible in the pod.
    append_log(
        log_path,
        &format!(
            "install {} via /proc/1/root\n",
            crate::os_upgrade::HOST_TRUST_PK
        ),
    )?;
    let install = kubectl_cmd(kubeconfig, server)
        .args([
            "exec",
            "-n",
            "kube-system",
            &pod,
            "--",
            "sh",
            "-c",
            r#"set -eux
echo "pid1=$(cat /proc/1/comm 2>/dev/null || echo unknown)"
ls -l /bundle || true
ls -l /bundle/os-trust.pk
test -s /bundle/os-trust.pk
ls -ld /proc/1/root /proc/1/root/system /proc/1/root/system/state || true
mkdir -p /proc/1/root/system/state/secrets
cp /bundle/os-trust.pk /proc/1/root/system/state/secrets/os-trust.pk
chmod 700 /proc/1/root/system/state/secrets
chmod 600 /proc/1/root/system/state/secrets/os-trust.pk
ls -l /proc/1/root/system/state/secrets/os-trust.pk
test -s /proc/1/root/system/state/secrets/os-trust.pk
echo "os-trust.pk present on STATE via /proc/1/root"
"#,
        ])
        .output()
        .await?;
    append_log(log_path, &String::from_utf8_lossy(&install.stdout))?;
    append_log(log_path, &String::from_utf8_lossy(&install.stderr))?;
    if !install.status.success() {
        let _ = delete_os_stage_pod(kubeconfig, server, &pod, false).await;
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&install.stdout),
            String::from_utf8_lossy(&install.stderr)
        );
        anyhow::bail!(
            "failed to install os-trust.pk into PID 1 STATE on {node_name}: {}",
            detail.trim()
        );
    }

    delete_os_stage_pod(kubeconfig, server, &pod, true).await?;
    append_log(
        log_path,
        &format!("staged bundle at {host} on {node_name}; trust key installed\n"),
    )?;
    Ok(())
}

async fn delete_os_stage_pod(
    kubeconfig: &str,
    server: Option<&str>,
    pod: &str,
    wait: bool,
) -> anyhow::Result<()> {
    let mut cmd = kubectl_cmd(kubeconfig, server);
    cmd.args([
        "delete",
        "pod",
        pod,
        "-n",
        "kube-system",
        "--ignore-not-found=true",
    ]);
    if wait {
        cmd.args(["--wait=true", "--timeout=60s"]);
    } else {
        cmd.arg("--wait=false");
    }
    let _ = cmd.output().await;
    Ok(())
}

async fn guest_api_open(ip: &str) -> bool {
    use tokio::net::TcpStream;
    use tokio::time::{timeout, Duration};

    let addr = format!("{ip}:50000");
    timeout(Duration::from_secs(2), TcpStream::connect(&addr))
        .await
        .ok()
        .and_then(|r| r.ok())
        .is_some()
}

fn provider_kind_is_nutanix(kind: &str) -> bool {
    matches!(kind, "nutanix" | "ahv" | "prism")
}

async fn nutanix_client_for_cluster(
    state: &AppState,
    cid: &str,
) -> anyhow::Result<Option<crate::nutanix::NutanixClient>> {
    let provider = provider_row_for_cluster(state, cid).await?;
    if !provider_kind_is_nutanix(&provider.kind) {
        return Ok(None);
    }
    let secret = crypto::decrypt(&state.cfg().secret_key, &provider.token_secret_enc)?;
    Ok(Some(crate::nutanix::NutanixClient::new(
        provider.url,
        provider.token_id,
        secret,
        provider.insecure != 0,
    )))
}

async fn node_ipv4(state: &AppState, node_id: &str) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT ip FROM nodes WHERE id = ?")
        .bind(node_id)
        .fetch_optional(state.pool())
        .await
        .ok()
        .flatten()
        .flatten()
        .filter(|s| !s.is_empty())
}

async fn wait_guest_api_down(ip: &str, log_path: &str, attempts: u32) -> anyhow::Result<()> {
    for i in 1..=attempts {
        if !guest_api_open(ip).await {
            append_log(
                log_path,
                &format!("guest API down ({ip}:50000) after ~{}s\n", i * 2),
            )?;
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    anyhow::bail!(
        "guest API {ip}:50000 never dropped after upgrade --reboot — reboot did not start"
    )
}

/// Wait until the Machine API is gone (reboot started), then until it is back.
/// Nutanix: pin UEFI to the OS disk; if firmware hangs, power-cycle. Rediscover IPAM/DHCP.
async fn wait_guest_after_os_reboot(
    state: &AppState,
    cluster_id: &str,
    node_id: &str,
    name: &str,
    old_ip: &str,
    log_path: &str,
) -> anyhow::Result<String> {
    wait_guest_api_down(old_ip, log_path, 45).await?;

    let nutanix = nutanix_client_for_cluster(state, cluster_id)
        .await
        .ok()
        .flatten();
    if let Some(ref client) = nutanix {
        append_log(
            log_path,
            &format!(
                "pin UEFI boot to OS disk on {name} (extra virtio disks can steal AHV boot)\n"
            ),
        )?;
        match client.pin_uefi_os_disk(name).await {
            Ok(()) => append_log(log_path, "UEFI boot pinned to disk 0\n")?,
            Err(e) => append_log(log_path, &format!("UEFI pin (best-effort): {e}\n"))?,
        }
    }

    let mut live_ip = old_ip.to_string();
    let mut healed = false;
    for i in 1..=180u32 {
        if guest_api_open(&live_ip).await {
            append_log(
                log_path,
                &format!(
                    "guest API up ({live_ip}:50000) after OS reboot (~{}s)\n",
                    i * 2
                ),
            )?;
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            return Ok(live_ip);
        }

        if let Some(ref client) = nutanix {
            if i % 3 == 0 {
                if let Ok(Some(hv)) = client.vm_ipv4(name).await {
                    if hv != live_ip {
                        append_log(
                            log_path,
                            &format!("node IP from Prism after reboot: {live_ip} → {hv}\n"),
                        )?;
                        let _ = sqlx::query("UPDATE nodes SET ip = ?, updated_at = ? WHERE id = ?")
                            .bind(&hv)
                            .bind(db::now_rfc3339())
                            .bind(node_id)
                            .execute(state.pool())
                            .await;
                        live_ip = hv;
                        continue;
                    }
                }
            }
        } else if i % 8 == 0 {
            let _ = crate::node_sync::rediscover_cluster_ips_now(state, cluster_id).await;
            if let Some(next) = node_ipv4(state, node_id).await {
                if next != live_ip {
                    append_log(
                        log_path,
                        &format!("node IP after reboot: {live_ip} → {next}\n"),
                    )?;
                    live_ip = next;
                    continue;
                }
            }
        }

        if !healed && i == 30 {
            if let Some(ref client) = nutanix {
                healed = true;
                append_log(
                    log_path,
                    &format!(
                        "AHV guest still down — pin UEFI OS disk and power-cycle {name} (firmware may show 'Unable to find valid boot device')\n"
                    ),
                )?;
                match client.pin_uefi_boot_and_power_cycle(name).await {
                    Ok(()) => append_log(log_path, "power-cycled after UEFI pin\n")?,
                    Err(e) => append_log(log_path, &format!("power-cycle: {e}\n"))?,
                }
            } else {
                let _ = crate::node_sync::rediscover_cluster_ips_now(state, cluster_id).await;
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    anyhow::bail!(
        "guest API not reachable after OS reboot on {name} (last IP {live_ip}). On Nutanix, Prism Serial may show 'Unable to find valid boot device' if the netcfg virtio disk stole UEFI; power off, pin boot to pci:0, detach extra disk, power on."
    )
}

/// kubeadm-shaped per-node upgrade: drain → bump version on-node → wait Ready → uncordon.
#[allow(clippy::too_many_arguments)]
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
                let tmp =
                    state
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
        // Machine-config apply / static-pod bump can bounce the API VIP; ensure
        // kubectl can talk to the cluster before creating the upgrade agent.
        wait_api_ready(kc, log_path).await?;

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
            // Agent is best-effort: VIP OpenAPI blips can fail create while the
            // node still reaches the target version via config reload / retry.
            if let Err(e) = apply_node_version_via_agent(kc, name, &want, is_cp, log_path).await {
                append_log(
                    log_path,
                    &format!(
                        "upgrade agent create/run issue on {name}: {e} — will wait for kubelet {want}\n"
                    ),
                )?;
            }
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
    sqlx::query("UPDATE nodes SET status = 'ready', k8s_version = ?, updated_at = ? WHERE id = ?")
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

    // Skip OpenAPI validation: during rolling CP upgrades the VIP often refuses
    // connections mid-apply ("failed to download openapi … connection refused").
    // Retry with API-ready waits for transient kube-vip / apiserver blips.
    let mut applied = false;
    let mut last_err = String::new();
    for attempt in 1..=8 {
        wait_api_ready(kubeconfig, log_path).await?;
        let apply = Command::new("kubectl")
            .args([
                "--kubeconfig",
                kubeconfig,
                "apply",
                "--validate=false",
                "--request-timeout=30s",
                "-f",
            ])
            .arg(&tmp)
            .output()
            .await?;
        append_log(log_path, &String::from_utf8_lossy(&apply.stdout))?;
        append_log(log_path, &String::from_utf8_lossy(&apply.stderr))?;
        if apply.status.success() {
            applied = true;
            break;
        }
        last_err = format!(
            "{}{}",
            String::from_utf8_lossy(&apply.stdout),
            String::from_utf8_lossy(&apply.stderr)
        );
        append_log(
            log_path,
            &format!("upgrade agent apply attempt {attempt}/8 failed; retrying after API wait\n"),
        )?;
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    let _ = std::fs::remove_file(&tmp);
    if !applied {
        // Config reload may have already bumped the node; treat as soft failure.
        if node_kubelet_version(kubeconfig, node_name).await.as_deref() == Some(version) {
            append_log(
                log_path,
                &format!(
                    "upgrade agent apply failed but kubelet already {version} on {node_name} — continuing\n"
                ),
            )?;
            return Ok(());
        }
        anyhow::bail!("failed to create upgrade agent pod on {node_name}: {last_err}");
    }

    // Wait for Succeeded/Failed, or for the node kubelet version to already match
    // (apiserver blips during CP upgrade make phase polls unreliable).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    let mut phase = String::new();
    while std::time::Instant::now() < deadline {
        if node_kubelet_version(kubeconfig, node_name).await.as_deref() == Some(version) {
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
        append_log(
            log_path,
            &format!("upgrade agent on {node_name} succeeded\n"),
        )?;
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
    kubectl_srv(kubeconfig, None, args, log_path).await
}

async fn kubectl_srv(
    kubeconfig: &str,
    server: Option<&str>,
    args: &[&str],
    log_path: &str,
) -> anyhow::Result<()> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kubeconfig);
    if let Some(s) = server {
        cmd.arg("--server").arg(s);
    }
    cmd.args(args);
    let out = cmd.output().await?;
    append_log(log_path, &String::from_utf8_lossy(&out.stdout))?;
    append_log(log_path, &String::from_utf8_lossy(&out.stderr))?;
    if !out.status.success() {
        anyhow::bail!("kubectl {} failed", args.first().unwrap_or(&""));
    }
    Ok(())
}

fn kubectl_cmd(kubeconfig: &str, server: Option<&str>) -> Command {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kubeconfig);
    if let Some(s) = server {
        cmd.arg("--server").arg(s);
    }
    cmd
}

fn https_cp_servers(nodes: &[(String, String, String, Option<String>)]) -> Vec<String> {
    nodes
        .iter()
        .filter(|(_, _, role, _)| role == "controlplane")
        .filter_map(|(_, _, _, ip)| ip.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|ip| format!("https://{ip}:6443"))
        .collect()
}

/// Wait until kubeconfig (VIP) or a CP node IP answers. Returns `--server` override
/// (`None` = kubeconfig default worked).
async fn wait_api_ready(kubeconfig: &str, log_path: &str) -> anyhow::Result<()> {
    wait_api_ready_ex(kubeconfig, &[], log_path)
        .await
        .map(|_| ())
}

async fn wait_api_ready_ex(
    kubeconfig: &str,
    extra_servers: &[String],
    log_path: &str,
) -> anyhow::Result<Option<String>> {
    let kc = std::path::Path::new(kubeconfig);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    let mut logged = false;
    let mut last: Option<Option<String>> = None;
    let mut ok_streak = 0u32;
    while std::time::Instant::now() < deadline {
        let hit = crate::cluster_availability::first_usable_server(kc, extra_servers).await;
        if let Some(server) = hit {
            if last.as_ref() == Some(&server) {
                ok_streak += 1;
            } else {
                ok_streak = 1;
                last = Some(server.clone());
            }
            if ok_streak >= 2 {
                if logged {
                    match &server {
                        None => append_log(log_path, "API reachable (kubeconfig / VIP)\n")?,
                        Some(s) => append_log(
                            log_path,
                            &format!("API reachable via {s} (kube-vip down or settling)\n"),
                        )?,
                    }
                }
                return Ok(server);
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        }
        ok_streak = 0;
        last = None;
        if !logged {
            let extras = if extra_servers.is_empty() {
                String::new()
            } else {
                format!("; also probing {}", extra_servers.join(" "))
            };
            append_log(
                log_path,
                &format!("wait API (VIP + control-plane :6443{extras})…\n"),
            )?;
            logged = true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    anyhow::bail!("timed out waiting for API (kube-vip and control-plane :6443 unreachable)")
}

async fn recover_etcd_on_cp1(
    pertiskctl: &std::path::Path,
    cps: &[(String, String, String, Option<String>)],
    log_path: &str,
) -> anyhow::Result<()> {
    let Some((_, name, _, ip)) = cps
        .iter()
        .find(|(_, n, _, _)| n.ends_with("-cp-1") || n.ends_with("-cp1"))
    else {
        anyhow::bail!("no *-cp-1 node to recover etcd");
    };
    let ip = ip
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("no IP for {name}"))?;
    append_log(
        log_path,
        &format!(
            "API down on VIP and CP IPs — etcd recover --force-new-cluster on {name} ({ip})\n"
        ),
    )?;
    let out = Command::new(pertiskctl)
        .args([
            "-e",
            &format!("{ip}:50000"),
            "etcd",
            "recover",
            "--force-new-cluster",
            "--force",
        ])
        .output()
        .await?;
    append_log(log_path, &String::from_utf8_lossy(&out.stdout))?;
    append_log(log_path, &String::from_utf8_lossy(&out.stderr))?;
    if !out.status.success() {
        anyhow::bail!(
            "etcd recover failed on {name} (guest may be too old for the RPC). \
             Run ./scripts/recover-not-ready-nodes.sh against this cluster kubeconfig, then retry OS upgrade."
        );
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
    let config_yaml = p.get("config_yaml").and_then(|v| v.as_str()).unwrap_or("");
    let node_id = p.get("node_id").and_then(|v| v.as_str());

    if !config_yaml.contains("version:") {
        anyhow::bail!(
            "config_yaml missing version (expected v1alpha1); partial dashboard-only YAML is OK"
        );
    }

    let public_url = state.cfg().public_url.trim();
    let config_yaml =
        if !public_url.is_empty() && !crate::config::public_url_host_unusable(public_url) {
            pertisk_config::ensure_dashboard_mgmt_url(config_yaml, public_url)
                .map_err(|e| anyhow::anyhow!("inject dashboard.mgmt_url: {e}"))?
        } else {
            config_yaml.to_string()
        };
    let config_yaml = config_yaml.as_str();

    let nodes = if let Some(nid) = node_id {
        sqlx::query_as::<_, (String, String, Option<String>, String)>(
            "SELECT id, name, ip, role FROM nodes WHERE id = ? AND cluster_id = ?",
        )
        .bind(nid)
        .bind(cid)
        .fetch_all(state.pool())
        .await?
    } else {
        sqlx::query_as::<_, (String, String, Option<String>, String)>(
            "SELECT id, name, ip, role FROM nodes WHERE cluster_id = ?",
        )
        .bind(cid)
        .fetch_all(state.pool())
        .await?
    };

    let mut failures = 0usize;
    for (_id, name, ip, role) in nodes {
        append_log(log_path, &format!("apply config to {name}\n"))?;
        let Some(ip) = ip else {
            append_log(log_path, "skip (no IP)\n")?;
            failures += 1;
            continue;
        };
        if !state.cfg().pertiskctl.exists() {
            append_log(log_path, "pertiskctl missing; config update recorded\n")?;
            continue;
        }

        let machine_type = if role == "controlplane" {
            pertisk_config::MachineType::Controlplane
        } else {
            pertisk_config::MachineType::Worker
        };
        let node_yaml = pertisk_config::set_machine_type_yaml(config_yaml, machine_type)
            .map_err(|e| anyhow::anyhow!("rewrite machine.type for {name}: {e}"))?;

        let tmp = state
            .cfg()
            .data_dir
            .join(format!("cfg-{}-{}.yaml", Uuid::new_v4(), name));
        std::fs::write(&tmp, &node_yaml)?;
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
        let _ = std::fs::remove_file(&tmp);

        match out {
            Ok(o) => {
                append_log(log_path, &String::from_utf8_lossy(&o.stdout))?;
                append_log(log_path, &String::from_utf8_lossy(&o.stderr))?;
                if !o.status.success() {
                    failures += 1;
                    append_log(
                        log_path,
                        &format!("apply failed on {name} (exit {})\n", o.status),
                    )?;
                }
            }
            Err(err) => {
                failures += 1;
                append_log(log_path, &format!("pertiskctl error on {name}: {err}\n"))?;
            }
        }
    }
    if failures > 0 {
        anyhow::bail!("{failures} node(s) failed to apply config");
    }
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
    max_pods: i64,
    arch: String,
    pod_subnet: String,
    service_subnet: String,
    pod_subnet_ipv6: Option<String>,
    service_subnet_ipv6: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ProviderRow {
    #[allow(dead_code)]
    id: String,
    kind: String,
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
    state.emit_job(cluster_id, &id, Some(kind), "queued");
    state.notify_jobs();
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pve_host_from_url_strips_scheme_and_port() {
        assert_eq!(
            pve_host_from_url("https://10.1.1.195:8006"),
            Some("10.1.1.195".into())
        );
        assert_eq!(
            pve_host_from_url("https://pve.example/"),
            Some("pve.example".into())
        );
    }

    #[test]
    fn proxmox_ssh_host_parses_user_at_host() {
        assert_eq!(
            proxmox_ssh_host("root@10.1.1.196"),
            Some("10.1.1.196".into())
        );
        assert_eq!(
            proxmox_ssh_host("root@10.1.1.196:22"),
            Some("10.1.1.196".into())
        );
    }

    #[test]
    fn addon_install_starts_while_other_cluster_create_runs() {
        let running = vec![(Some("a".into()), "create_cluster".into())];
        assert!(job_can_start("install_addon", Some("b"), &running));
        assert!(!job_can_start("install_addon", Some("a"), &running));
        assert!(job_can_start("create_cluster", Some("b"), &running));
        assert!(!job_can_start("add_node", Some("a"), &running));
    }

    #[test]
    fn two_addons_can_run_in_parallel() {
        let running = vec![
            (Some("a".into()), "install_addon".into()),
            (Some("b".into()), "install_addon".into()),
        ];
        assert!(job_can_start("install_addon", Some("a"), &running));
        assert!(job_can_start("install_addon", Some("c"), &running));
        assert!(job_can_start("create_cluster", Some("d"), &running));
        assert!(!job_can_start("create_cluster", Some("a"), &running));
    }

    #[test]
    fn exclusive_jobs_run_in_parallel_across_clusters() {
        let running = vec![(Some("a".into()), "create_cluster".into())];
        assert!(job_can_start("create_cluster", Some("b"), &running));
        assert!(job_can_start("add_node", Some("b"), &running));
        assert!(job_can_start("upgrade_cluster", Some("c"), &running));
        assert!(!job_can_start("upgrade_cluster", Some("a"), &running));
        assert!(!job_can_start("add_node", Some("a"), &running));
    }

    #[test]
    fn delete_can_start_on_same_cluster_while_create_aborts() {
        let running = vec![(Some("a".into()), "create_cluster".into())];
        assert!(job_can_start("delete_cluster", Some("a"), &running));
        assert!(job_can_start("delete_cluster", Some("b"), &running));
    }

    #[test]
    fn rewrite_proxmox_ssh_uses_provider_host() {
        assert_eq!(
            rewrite_proxmox_ssh_for_provider("root@10.1.1.196", "https://10.1.1.195:8006"),
            Some("root@10.1.1.195".into())
        );
        assert_eq!(
            rewrite_proxmox_ssh_for_provider("admin@10.1.1.196:22", "https://10.1.1.195:8006"),
            Some("admin@10.1.1.195".into())
        );
        assert_eq!(
            rewrite_proxmox_ssh_for_provider("root@10.1.1.195", "https://10.1.1.195:8006"),
            None
        );
        assert_eq!(
            rewrite_proxmox_ssh_for_provider("root@pve-a", "https://pve-b:8006"),
            Some("root@pve-b".into())
        );
    }
}
