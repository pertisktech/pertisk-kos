//! Resolve kubeconfig paths and run `kubectl` against a cluster.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::{ApiResult, AppError};
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadKind {
    Deployments,
    StatefulSets,
    DaemonSets,
    Jobs,
    CronJobs,
    Pods,
}

impl WorkloadKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "deployments" | "deployment" | "deploy" => Some(Self::Deployments),
            "statefulsets" | "statefulset" | "sts" => Some(Self::StatefulSets),
            "daemonsets" | "daemonset" | "ds" => Some(Self::DaemonSets),
            "jobs" | "job" => Some(Self::Jobs),
            "cronjobs" | "cronjob" | "cj" => Some(Self::CronJobs),
            "pods" | "pod" | "po" => Some(Self::Pods),
            _ => None,
        }
    }

    pub fn kubectl_resource(self) -> &'static str {
        match self {
            Self::Deployments => "deployments",
            Self::StatefulSets => "statefulsets",
            Self::DaemonSets => "daemonsets",
            Self::Jobs => "jobs",
            Self::CronJobs => "cronjobs",
            Self::Pods => "pods",
        }
    }

    pub fn as_str(self) -> &'static str {
        self.kubectl_resource()
    }
}

/// Resolve a readable kubeconfig for `cluster_id` (any status).
#[allow(dead_code)]
pub async fn resolve_cluster_kubeconfig(
    state: &AppState,
    cluster_id: &str,
) -> ApiResult<(PathBuf, String)> {
    let row: Option<(Option<String>, String, String)> =
        sqlx::query_as("SELECT kubeconfig_path, name, status FROM clusters WHERE id = ?")
            .bind(cluster_id)
            .fetch_optional(state.pool())
            .await?;
    let Some((path, name, _status)) = row else {
        return Err(AppError::NotFound);
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = path.filter(|s| !s.is_empty()) {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(state.cfg().kubeconfigs_dir().join(&name).join("admin.conf"));
    candidates.push(PathBuf::from("out/cluster/admin.conf"));
    for c in candidates {
        if kubeconfig_file_usable(&c) {
            return Ok((c, name));
        }
    }
    Err(AppError::bad("kubeconfig not available yet"))
}

/// Like [`resolve_cluster_kubeconfig`], but requires `status = ready`.
pub async fn resolve_ready_kubeconfig(
    state: &AppState,
    cluster_id: &str,
) -> ApiResult<(PathBuf, String)> {
    let row: Option<(Option<String>, String, String)> =
        sqlx::query_as("SELECT kubeconfig_path, name, status FROM clusters WHERE id = ?")
            .bind(cluster_id)
            .fetch_optional(state.pool())
            .await?;
    let Some((path, name, status)) = row else {
        return Err(AppError::NotFound);
    };
    if status != "ready" {
        return Err(AppError::bad(format!(
            "cluster is {status}; K8s API available when ready"
        )));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = path.filter(|s| !s.is_empty()) {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(state.cfg().kubeconfigs_dir().join(&name).join("admin.conf"));
    for c in candidates {
        if kubeconfig_file_usable(&c) {
            return Ok((c, name));
        }
    }
    Err(AppError::bad("kubeconfig not available yet"))
}

pub async fn kubectl_json(kubeconfig: &Path, args: &[&str]) -> ApiResult<serde_json::Value> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kubeconfig).args(args);
    let out = cmd
        .output()
        .await
        .map_err(|e| AppError::bad(format!("kubectl: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = stderr.trim();
        return Err(AppError::bad(if msg.is_empty() {
            format!("kubectl {:?} failed", args)
        } else {
            msg.to_string()
        }));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim())
        .map_err(|e| AppError::bad(format!("kubectl json parse: {e}")))
}

pub async fn kubectl_ok(kubeconfig: &Path, args: &[&str]) -> ApiResult<()> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kubeconfig).args(args);
    let out = cmd
        .output()
        .await
        .map_err(|e| AppError::bad(format!("kubectl: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = stderr.trim();
        return Err(AppError::bad(if msg.is_empty() {
            format!("kubectl {:?} failed", args)
        } else {
            msg.to_string()
        }));
    }
    Ok(())
}

/// `kubectl get … -o json`, treating missing resources as `None`.
pub async fn kubectl_json_optional(
    kubeconfig: &Path,
    args: &[&str],
) -> ApiResult<Option<serde_json::Value>> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig").arg(kubeconfig).args(args);
    let out = cmd
        .output()
        .await
        .map_err(|e| AppError::bad(format!("kubectl: {e}")))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let msg = stderr.trim().to_ascii_lowercase();
        if msg.contains("notfound")
            || msg.contains("not found")
            || msg.contains("no matches for kind")
            || msg.contains("the server could not find the requested resource")
        {
            return Ok(None);
        }
        return Err(AppError::bad(if stderr.trim().is_empty() {
            format!("kubectl {:?} failed", args)
        } else {
            stderr.trim().to_string()
        }));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(stdout.trim())
        .map(Some)
        .map_err(|e| AppError::bad(format!("kubectl json parse: {e}")))
}

/// `kubectl apply -f -` with YAML or JSON on stdin. Returns combined stdout/stderr on success.
pub async fn kubectl_apply_yaml(kubeconfig: &Path, doc: &str) -> ApiResult<String> {
    let mut child = Command::new("kubectl")
        .arg("--kubeconfig")
        .arg(kubeconfig)
        .args(["apply", "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::bad(format!("kubectl: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(doc.as_bytes())
            .await
            .map_err(|e| AppError::bad(format!("kubectl stdin: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::bad(format!("kubectl: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let msg = stderr.trim();
        return Err(AppError::bad(if msg.is_empty() {
            "kubectl apply failed".into()
        } else {
            msg.to_string()
        }));
    }
    Ok(format!("{stdout}{stderr}"))
}

/// `kubectl apply -f <url-or-path>`.
pub async fn kubectl_apply_url(kubeconfig: &Path, url: &str) -> ApiResult<String> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("--kubeconfig")
        .arg(kubeconfig)
        .args(["apply", "-f", url]);
    let out = cmd
        .output()
        .await
        .map_err(|e| AppError::bad(format!("kubectl: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let msg = stderr.trim();
        return Err(AppError::bad(if msg.is_empty() {
            format!("kubectl apply -f {url} failed")
        } else {
            msg.to_string()
        }));
    }
    Ok(format!("{stdout}{stderr}"))
}

/// Run `helm` with optional `--kubeconfig`. Returns combined stdout/stderr.
pub async fn helm_output(kubeconfig: Option<&Path>, args: &[&str]) -> ApiResult<String> {
    let mut cmd = Command::new("helm");
    if let Some(kc) = kubeconfig {
        cmd.arg("--kubeconfig").arg(kc);
    }
    cmd.args(args);
    let out = cmd.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::bad("helm not found on the management host PATH")
        } else {
            AppError::bad(format!("helm: {e}"))
        }
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let msg = stderr.trim();
        return Err(AppError::bad(if msg.is_empty() {
            format!("helm {:?} failed", args)
        } else {
            msg.to_string()
        }));
    }
    Ok(format!("{stdout}{stderr}"))
}

/// Skip empty / stub files left by a previous cluster of the same name.
pub fn kubeconfig_file_usable(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    kubeconfig_file_usable_str(&raw)
}

pub fn kubeconfig_tls_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("x509")
        || m.contains("certificate signed by unknown authority")
        || m.contains("tls: failed to verify")
        || m.contains("ecdsa verification failure")
}

/// Pull a fresh admin kubeconfig from a live guest (new CA after recreate).
pub async fn refresh_kubeconfig_from_guest(
    pertiskctl: &Path,
    guest_ip: &str,
    dest: &Path,
    cluster_name: &str,
) -> anyhow::Result<()> {
    if !pertiskctl.is_file() {
        anyhow::bail!("pertiskctl missing");
    }
    let ip = guest_ip.trim();
    if ip.is_empty() {
        anyhow::bail!("no guest IP");
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("conf.refresh");
    let out = Command::new(pertiskctl)
        .args(["-e", &format!("{ip}:50000"), "kubeconfig", "-f"])
        .arg(&tmp)
        .output()
        .await?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!(
            "pertiskctl kubeconfig: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let content = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    if !kubeconfig_file_usable_str(&content) {
        anyhow::bail!("guest returned empty kubeconfig");
    }
    let rewritten = crate::kubeconfig::rename_kubeconfig_context(&content, cluster_name);
    std::fs::write(dest, rewritten)?;
    Ok(())
}

fn kubeconfig_file_usable_str(raw: &str) -> bool {
    let t = raw.trim();
    !t.is_empty()
        && !t.starts_with('#')
        && (t.contains("certificate-authority-data") || t.contains("certificate-authority:"))
}
