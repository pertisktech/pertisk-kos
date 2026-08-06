//! Resolve kubeconfig paths and run `kubectl` against a cluster.

use std::path::{Path, PathBuf};

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
    let row: Option<(Option<String>, String, String)> = sqlx::query_as(
        "SELECT kubeconfig_path, name, status FROM clusters WHERE id = ?",
    )
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
    candidates.push(
        state
            .cfg()
            .kubeconfigs_dir()
            .join(&name)
            .join("admin.conf"),
    );
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
    let row: Option<(Option<String>, String, String)> = sqlx::query_as(
        "SELECT kubeconfig_path, name, status FROM clusters WHERE id = ?",
    )
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
    candidates.push(
        state
            .cfg()
            .kubeconfigs_dir()
            .join(&name)
            .join("admin.conf"),
    );
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
    serde_json::from_str(stdout.trim()).map_err(|e| {
        AppError::bad(format!("kubectl json parse: {e}"))
    })
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
