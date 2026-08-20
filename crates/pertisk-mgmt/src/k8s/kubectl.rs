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
        if c.is_file() {
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
        if c.is_file() {
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
