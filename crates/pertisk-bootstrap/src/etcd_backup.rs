//! etcd snapshot / restore helpers (lab; uses etcd-client + etcdutl via ctr).

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use etcd_client::{Certificate as EtcdCert, Client, ConnectOptions, Identity as EtcdIdentity};
use tracing::{info, warn};

use crate::paths::BootstrapPaths;
use crate::DEFAULT_ETCD_IMAGE;

const LOCAL_ETCD: &str = "https://127.0.0.1:2379";
const SNAPSHOT_DIR: &str = "/var/lib/pertisk/etcd-snapshots";
const ETCD_DATA: &str = "/var/lib/etcd";
const ETCD_MANIFEST_LIVE: &str = "/etc/kubernetes/manifests/etcd.yaml";
const ETCD_MANIFEST_DISABLED: &str = "/etc/kubernetes/manifests/etcd.yaml.pertisk-restore-disabled";

#[derive(Debug, Clone)]
pub struct EtcdSnapshotResult {
    pub available: bool,
    pub message: String,
    pub path: String,
    pub size_bytes: u64,
    pub revision: i64,
}

#[derive(Debug, Clone)]
pub struct EtcdRestoreResult {
    pub ok: bool,
    pub message: String,
}

fn etcd_tls_paths(state_root: &Path) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let live = Path::new("/etc/kubernetes/pki/etcd");
    let state = BootstrapPaths::default_state(state_root).etcd_pki();
    let base = if live.join("ca.crt").is_file() {
        live.to_path_buf()
    } else {
        state
    };
    let ca = base.join("ca.crt");
    let cert = base.join("server.crt");
    let key = base.join("server.key");
    if !(ca.is_file() && cert.is_file() && key.is_file()) {
        bail!(
            "etcd PKI missing under {} (bootstrap a control-plane first)",
            base.display()
        );
    }
    Ok((ca, cert, key))
}

async fn connect_local(state_root: &Path) -> Result<Client> {
    let (ca_path, cert_path, key_path) = etcd_tls_paths(state_root)?;
    let ca_pem = fs::read(&ca_path).with_context(|| format!("read {}", ca_path.display()))?;
    let cert_pem = fs::read(&cert_path).with_context(|| format!("read {}", cert_path.display()))?;
    let key_pem = fs::read(&key_path).with_context(|| format!("read {}", key_path.display()))?;
    let opts = ConnectOptions::new()
        .with_timeout(Duration::from_secs(5))
        .with_connect_timeout(Duration::from_secs(5))
        .with_tls(
            etcd_client::TlsOptions::new()
                .ca_certificate(EtcdCert::from_pem(ca_pem))
                .identity(EtcdIdentity::from_pem(cert_pem, key_pem)),
        );
    Client::connect([LOCAL_ETCD], Some(opts))
        .await
        .context("connect local etcd https://127.0.0.1:2379")
}

fn default_snapshot_path() -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    PathBuf::from(SNAPSHOT_DIR).join(format!("snapshot-{ts}.db"))
}

const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(20);

/// Take a live etcd snapshot and write it to `output_path` (or auto path under SNAPSHOT_DIR).
pub async fn etcd_snapshot(state_root: &Path, output_path: Option<&Path>) -> EtcdSnapshotResult {
    match tokio::time::timeout(SNAPSHOT_TIMEOUT, etcd_snapshot_inner(state_root, output_path)).await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => EtcdSnapshotResult {
            available: false,
            message: format!("etcd snapshot failed: {e}"),
            path: String::new(),
            size_bytes: 0,
            revision: 0,
        },
        Err(_) => EtcdSnapshotResult {
            available: false,
            message: format!(
                "etcd snapshot timed out after {}s (no leader / quorum lost — try `etcd recover --force-new-cluster`)",
                SNAPSHOT_TIMEOUT.as_secs()
            ),
            path: String::new(),
            size_bytes: 0,
            revision: 0,
        },
    }
}

async fn etcd_snapshot_inner(
    state_root: &Path,
    output_path: Option<&Path>,
) -> Result<EtcdSnapshotResult> {
    let path = match output_path {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => default_snapshot_path(),
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }

    let mut client = connect_local(state_root).await?;
    let status = client.status().await.context("etcd status")?;
    let revision = status.header().map(|h| h.revision()).unwrap_or(0);

    let mut stream = client.snapshot().await.context("etcd snapshot stream")?;
    let mut file = File::create(&path).with_context(|| format!("create {}", path.display()))?;
    let mut size = 0u64;
    while let Some(chunk) = stream.message().await.context("snapshot chunk")? {
        let blob = chunk.blob();
        file.write_all(blob)
            .with_context(|| format!("write {}", path.display()))?;
        size += blob.len() as u64;
    }
    file.sync_all().ok();

    info!(path = %path.display(), size, revision, "etcd snapshot written");
    Ok(EtcdSnapshotResult {
        available: true,
        message: format!("wrote {size} bytes (revision={revision})"),
        path: path.display().to_string(),
        size_bytes: size,
        revision,
    })
}

/// Offline restore of a snapshot into `/var/lib/etcd` (lab; requires `force`).
///
/// Stops the etcd static pod, runs `etcdutl snapshot restore` via the etcd image
/// (`ctr`), then re-enables the manifest. HA: only safe when this member is the
/// sole survivor / you know what you are doing — `force` acknowledges data loss.
pub async fn etcd_restore(
    state_root: &Path,
    snapshot_path: &Path,
    force: bool,
    member_name: &str,
    initial_cluster: &str,
    peer_url: &str,
) -> EtcdRestoreResult {
    match etcd_restore_inner(
        state_root,
        snapshot_path,
        force,
        member_name,
        initial_cluster,
        peer_url,
    )
    .await
    {
        Ok(msg) => EtcdRestoreResult {
            ok: true,
            message: msg,
        },
        Err(e) => EtcdRestoreResult {
            ok: false,
            message: format!("etcd restore failed: {e}"),
        },
    }
}

async fn etcd_restore_inner(
    state_root: &Path,
    snapshot_path: &Path,
    force: bool,
    member_name: &str,
    initial_cluster: &str,
    peer_url: &str,
) -> Result<String> {
    if !force {
        bail!("EtcdRestore requires force=true (destroys local etcd data dir)");
    }
    if !snapshot_path.is_file() {
        bail!("snapshot not found: {}", snapshot_path.display());
    }

    // Refuse multi-member restore unless caller already set force (still warn).
    if let Ok(mut client) = connect_local(state_root).await {
        if let Ok(list) = client.member_list().await {
            let n = list.members().len();
            if n > 1 {
                warn!(
                    members = n,
                    "restoring local etcd while cluster has {n} members — force=true"
                );
            }
        }
    }

    disable_etcd_static_pod()?;
    wait_etcd_down(Duration::from_secs(90))?;

    // Clear data dir.
    if Path::new(ETCD_DATA).exists() {
        fs::remove_dir_all(ETCD_DATA).context("remove /var/lib/etcd")?;
    }
    fs::create_dir_all(ETCD_DATA).context("mkdir /var/lib/etcd")?;

    run_etcdutl_restore(snapshot_path, member_name, initial_cluster, peer_url)?;

    enable_etcd_static_pod()?;
    wait_etcd_up(state_root, Duration::from_secs(120)).await?;

    Ok(format!(
        "restored {} into {ETCD_DATA}; etcd static pod re-enabled",
        snapshot_path.display()
    ))
}

/// Promote the local member to a one-node cluster using the existing data dir.
///
/// Used when HA etcd has no leader (typical after DHCP reassigns CP IPs so peer
/// URLs overlap the old member set). Does **not** wipe `/var/lib/etcd`. Lab-only;
/// `force` is required.
pub async fn etcd_force_new_cluster(
    state_root: &Path,
    force: bool,
    member_name: &str,
    advertise_ip: &str,
) -> EtcdRestoreResult {
    match etcd_force_new_cluster_inner(state_root, force, member_name, advertise_ip).await {
        Ok(msg) => EtcdRestoreResult {
            ok: true,
            message: msg,
        },
        Err(e) => EtcdRestoreResult {
            ok: false,
            message: format!("etcd force-new-cluster failed: {e}"),
        },
    }
}

async fn etcd_force_new_cluster_inner(
    state_root: &Path,
    force: bool,
    member_name: &str,
    advertise_ip: &str,
) -> Result<String> {
    if !force {
        bail!("EtcdRestore force_new_cluster requires force=true");
    }
    let advertise_ip = advertise_ip.trim();
    if advertise_ip.is_empty() {
        bail!("advertise address required for force-new-cluster");
    }

    let raw = read_etcd_manifest()?;
    let name = if member_name.trim().is_empty() {
        etcd_flag_value(&raw, "--name=").unwrap_or_else(|| "pertisk-cp-1".into())
    } else {
        member_name.trim().to_string()
    };

    disable_etcd_static_pod()?;
    wait_etcd_down(Duration::from_secs(90))?;

    let patched = patch_etcd_manifest_force_new_cluster(&raw, &name, advertise_ip);
    fs::write(ETCD_MANIFEST_LIVE, patched)
        .context("write etcd manifest with --force-new-cluster")?;
    let _ = fs::remove_file(ETCD_MANIFEST_DISABLED);
    info!(name = %name, advertise_ip, "etcd static pod enabled with --force-new-cluster");

    wait_etcd_up(state_root, Duration::from_secs(120)).await?;

    let live = fs::read_to_string(ETCD_MANIFEST_LIVE).context("re-read etcd manifest")?;
    let finalized = finalize_etcd_manifest_after_force_new(&live, &name, advertise_ip);
    fs::write(ETCD_MANIFEST_LIVE, finalized)
        .context("strip --force-new-cluster from etcd manifest")?;
    wait_etcd_up(state_root, Duration::from_secs(120)).await?;

    Ok(format!(
        "etcd --force-new-cluster as {name}=https://{advertise_ip}:2380; flag stripped after healthy"
    ))
}

fn read_etcd_manifest() -> Result<String> {
    for path in [ETCD_MANIFEST_LIVE, ETCD_MANIFEST_DISABLED] {
        let p = Path::new(path);
        if p.is_file() {
            return fs::read_to_string(p).with_context(|| format!("read {}", p.display()));
        }
    }
    bail!("etcd static pod manifest missing ({ETCD_MANIFEST_LIVE})")
}

fn etcd_flag_value(yaml: &str, flag: &str) -> Option<String> {
    for line in yaml.lines() {
        let Some(i) = line.find(flag) else {
            continue;
        };
        let rest = line[i + flag.len()..].trim();
        let val = rest
            .trim_start_matches('"')
            .trim_end_matches(['"', ',', ' ']);
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
}

fn yaml_list_prefix(line: &str) -> (String, String) {
    let start = line.trim_start();
    let indent = " ".repeat(line.len() - start.len());
    if start.starts_with("- ") {
        (indent, "- ".into())
    } else {
        (indent, String::new())
    }
}

fn replace_command_flag(yaml: &str, flag: &str, value: &str) -> String {
    let mut out = String::with_capacity(yaml.len());
    for line in yaml.lines() {
        if line.contains(flag) {
            let (indent, marker) = yaml_list_prefix(line);
            let comma = if line.trim_end().ends_with(',') {
                ","
            } else {
                ""
            };
            out.push_str(&indent);
            out.push_str(&marker);
            out.push_str(flag);
            out.push_str(value);
            out.push_str(comma);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn ensure_force_new_cluster_flag(yaml: &str) -> String {
    if yaml.lines().any(|l| l.contains("--force-new-cluster")) {
        return yaml.to_string();
    }
    let mut out = String::with_capacity(yaml.len() + 32);
    let mut inserted = false;
    for line in yaml.lines() {
        out.push_str(line);
        out.push('\n');
        if !inserted && line.contains("--data-dir=") {
            let (indent, marker) = yaml_list_prefix(line);
            out.push_str(&indent);
            out.push_str(&marker);
            out.push_str("--force-new-cluster");
            if line.trim_end().ends_with(',') {
                out.push(',');
            }
            out.push('\n');
            inserted = true;
        }
    }
    if !inserted {
        out.push_str("        - --force-new-cluster\n");
    }
    out
}

fn strip_force_new_cluster_flag(yaml: &str) -> String {
    let mut out = String::with_capacity(yaml.len());
    for line in yaml.lines() {
        let core = line
            .trim_start()
            .trim_start_matches("- ")
            .trim_start_matches('"')
            .trim_end_matches(['"', ',', ' ']);
        if core == "--force-new-cluster" {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Rewrite the static-pod command so this member can start alone from existing WAL.
pub(crate) fn patch_etcd_manifest_force_new_cluster(
    yaml: &str,
    name: &str,
    advertise_ip: &str,
) -> String {
    let peer = format!("https://{advertise_ip}:2380");
    let mut out = replace_command_flag(yaml, "--name=", name);
    out = replace_command_flag(
        &out,
        "--advertise-client-urls=",
        &format!("https://{advertise_ip}:2379,https://127.0.0.1:2379"),
    );
    out = replace_command_flag(&out, "--initial-advertise-peer-urls=", &peer);
    out = replace_command_flag(&out, "--initial-cluster=", &format!("{name}={peer}"));
    out = replace_command_flag(&out, "--initial-cluster-state=", "new");
    ensure_force_new_cluster_flag(&out)
}

fn finalize_etcd_manifest_after_force_new(yaml: &str, name: &str, advertise_ip: &str) -> String {
    let peer = format!("https://{advertise_ip}:2380");
    let mut out = strip_force_new_cluster_flag(yaml);
    out = replace_command_flag(&out, "--initial-cluster=", &format!("{name}={peer}"));
    replace_command_flag(&out, "--initial-cluster-state=", "existing")
}

fn disable_etcd_static_pod() -> Result<()> {
    let live = Path::new(ETCD_MANIFEST_LIVE);
    if live.is_file() {
        fs::rename(live, ETCD_MANIFEST_DISABLED).context("disable etcd static pod manifest")?;
        info!("disabled etcd static pod (manifest moved aside)");
    }
    Ok(())
}

fn enable_etcd_static_pod() -> Result<()> {
    let disabled = Path::new(ETCD_MANIFEST_DISABLED);
    let live = Path::new(ETCD_MANIFEST_LIVE);
    if disabled.is_file() {
        fs::rename(disabled, live).context("re-enable etcd static pod manifest")?;
        info!("re-enabled etcd static pod");
    } else if !live.is_file() {
        // Fall back to STATE copy.
        bail!("etcd manifest missing; cannot re-enable static pod");
    }
    Ok(())
}

fn wait_etcd_down(timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(
            &"127.0.0.1:2379".parse().unwrap(),
            Duration::from_secs(1),
        )
        .is_err()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    bail!("etcd still listening on :2379 after disabling static pod")
}

async fn wait_etcd_up(state_root: &Path, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        match connect_local(state_root).await {
            Ok(mut c) => {
                if c.status().await.is_ok() {
                    return Ok(());
                }
            }
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    bail!("etcd not healthy after restore: {last}")
}

fn run_etcdutl_restore(
    snapshot: &Path,
    name: &str,
    initial_cluster: &str,
    peer_url: &str,
) -> Result<()> {
    let snap = snapshot
        .canonicalize()
        .with_context(|| format!("canonicalize {}", snapshot.display()))?;
    let ctr = find_ctr().context("ctr not found (need containerd ctr for etcdutl)")?;
    // Prefer host etcdutl if present (debug images); else run via etcd image.
    if let Ok(etcdutl) = which("etcdutl") {
        let status = Command::new(etcdutl)
            .args([
                "snapshot",
                "restore",
                snap.to_str().unwrap(),
                "--data-dir",
                ETCD_DATA,
                "--name",
                name,
                "--initial-cluster",
                initial_cluster,
                "--initial-advertise-peer-urls",
                peer_url,
            ])
            .status()
            .context("run etcdutl")?;
        if !status.success() {
            bail!("etcdutl exited {status}");
        }
        return Ok(());
    }

    let image = std::env::var("PERTISK_ETCD_IMAGE").unwrap_or_else(|_| DEFAULT_ETCD_IMAGE.into());
    // Ensure image is present (pull if needed).
    let _ = Command::new(&ctr)
        .args(["-n", "k8s.io", "images", "pull", &image])
        .status();

    let name_id = format!("pertisk-etcd-restore-{}", std::process::id());
    let status = Command::new(&ctr)
        .args([
            "-n",
            "k8s.io",
            "run",
            "--rm",
            "--net-host",
            "--mount",
            &format!(
                "type=bind,src={},dst=/snapshot.db,options=rbind:ro",
                snap.display()
            ),
            "--mount",
            "type=bind,src=/var/lib/etcd,dst=/var/lib/etcd,options=rbind:rw",
            &image,
            &name_id,
            "etcdutl",
            "snapshot",
            "restore",
            "/snapshot.db",
            "--data-dir=/var/lib/etcd",
            &format!("--name={name}"),
            &format!("--initial-cluster={initial_cluster}"),
            &format!("--initial-advertise-peer-urls={peer_url}"),
        ])
        .status()
        .context("ctr run etcdutl")?;
    if !status.success() {
        bail!("ctr etcdutl restore exited {status}");
    }
    Ok(())
}

fn find_ctr() -> Option<PathBuf> {
    for p in ["/usr/local/bin/ctr", "/usr/bin/ctr"] {
        if Path::new(p).is_file() {
            return Some(PathBuf::from(p));
        }
    }
    which("ctr").ok().map(PathBuf::from)
}

fn which(bin: &str) -> Result<PathBuf> {
    let out = Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .output()
        .context("command -v")?;
    if !out.status.success() {
        bail!("{bin} not found");
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() {
        bail!("{bin} not found");
    }
    Ok(PathBuf::from(p))
}

/// Guess member name + initial-cluster from hostname / single-node advertise.
pub fn default_restore_identity(hostname: &str, advertise_ip: &str) -> (String, String, String) {
    let name = if hostname.is_empty() {
        "pertisk-cp-1".into()
    } else {
        hostname.to_string()
    };
    let peer = format!("https://{advertise_ip}:2380");
    let initial = format!("{name}={peer}");
    (name, initial, peer)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
command:
- etcd
- --advertise-client-urls=https://10.1.1.180:2379,https://127.0.0.1:2379
- --data-dir=/var/lib/etcd
- --initial-advertise-peer-urls=https://10.1.1.178:2380
- --initial-cluster=lab-cp-1=https://10.1.1.178:2380,lab-cp-2=https://10.1.1.179:2380,lab-cp-3=https://10.1.1.180:2380
- --initial-cluster-state=existing
- --name=lab-cp-1
";

    #[test]
    fn force_new_cluster_rewrites_peers_and_adds_flag() {
        let patched = patch_etcd_manifest_force_new_cluster(SAMPLE, "lab-cp-1", "10.1.1.180");
        assert!(patched.contains("--force-new-cluster"));
        assert!(patched.contains("--initial-cluster=lab-cp-1=https://10.1.1.180:2380"));
        assert!(!patched.contains("10.1.1.178"));
        assert!(patched.contains("--initial-cluster-state=new"));
        assert!(patched.contains("--initial-advertise-peer-urls=https://10.1.1.180:2380"));
        assert_eq!(
            patched.matches("--force-new-cluster").count(),
            1,
            "flag must not be duplicated"
        );
    }

    #[test]
    fn finalize_strips_flag_and_marks_existing() {
        let patched = patch_etcd_manifest_force_new_cluster(SAMPLE, "lab-cp-1", "10.1.1.180");
        let done = finalize_etcd_manifest_after_force_new(&patched, "lab-cp-1", "10.1.1.180");
        assert!(!done.contains("--force-new-cluster"));
        assert!(done.contains("--initial-cluster-state=existing"));
        assert!(done.contains("--initial-cluster=lab-cp-1=https://10.1.1.180:2380"));
    }
}
