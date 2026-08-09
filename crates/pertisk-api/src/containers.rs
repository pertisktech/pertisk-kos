//! List containers via containerd `ctr` (lab CRI introspection).

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

const SOCK: &str = "/run/containerd/containerd.sock";
const NS: &str = "k8s.io";
const CTR_CANDIDATES: &[&str] = &["/usr/local/bin/ctr", "/usr/bin/ctr", "ctr"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub namespace: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainersSnapshot {
    pub available: bool,
    pub message: String,
    pub containers: Vec<ContainerRow>,
}

/// List containers in the kubelet containerd namespace.
pub fn list_containers() -> ContainersSnapshot {
    if !Path::new(SOCK).exists() {
        return ContainersSnapshot {
            available: false,
            message: format!("containerd socket missing at {SOCK}"),
            containers: Vec::new(),
        };
    }
    let Some(ctr) = find_ctr() else {
        return ContainersSnapshot {
            available: false,
            message: "ctr binary not found".into(),
            containers: Vec::new(),
        };
    };

    let containers_out = match run_ctr(ctr, &["containers", "ls"]) {
        Ok(s) => s,
        Err(err) => {
            return ContainersSnapshot {
                available: false,
                message: err,
                containers: Vec::new(),
            };
        }
    };
    let tasks_out = run_ctr(ctr, &["tasks", "ls"]).unwrap_or_default();
    let running = parse_task_ids(&tasks_out);
    let mut containers = parse_containers_ls(&containers_out, &running);
    containers.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));

    ContainersSnapshot {
        available: true,
        message: format!("listed {} container(s) in {NS}", containers.len()),
        containers,
    }
}

fn find_ctr() -> Option<&'static str> {
    CTR_CANDIDATES
        .iter()
        .copied()
        .find(|p| Path::new(p).is_file() || *p == "ctr")
        .filter(|p| {
            if *p == "ctr" {
                Command::new("ctr")
                    .arg("--version")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            } else {
                true
            }
        })
}

fn run_ctr(ctr: &str, sub: &[&str]) -> Result<String, String> {
    let mut args = vec!["-a", SOCK, "-n", NS];
    args.extend_from_slice(sub);
    let out = Command::new(ctr)
        .args(&args)
        .output()
        .map_err(|e| format!("spawn {ctr}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "{ctr} {} failed: {}",
            sub.join(" "),
            err.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse `ctr containers ls` tabular output.
pub fn parse_containers_ls(stdout: &str, running: &HashSet<String>) -> Vec<ContainerRow> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("CONTAINER") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let id = parts[0].to_string();
        let image = parts[1].to_string();
        let name = container_display_name(&id);
        let state = if running_contains(running, &id) {
            "running"
        } else {
            "created"
        };
        rows.push(ContainerRow {
            id,
            name,
            image,
            state: state.into(),
            namespace: NS.into(),
        });
    }
    rows
}

/// Parse `ctr tasks ls` — first column is task/container id.
pub fn parse_task_ids(stdout: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("TASK") {
            continue;
        }
        if let Some(id) = line.split_whitespace().next() {
            set.insert(id.to_string());
        }
    }
    set
}

fn running_contains(running: &HashSet<String>, id: &str) -> bool {
    if running.contains(id) {
        return true;
    }
    // Match short prefix (ctr sometimes shows truncated ids in one listing).
    let short = if id.len() > 12 { &id[..12] } else { id };
    running.iter().any(|r| r == id || r.starts_with(short) || id.starts_with(r.as_str()))
}

fn container_display_name(id: &str) -> String {
    // Prefer last path segment when id looks like a path-ish name.
    if let Some(tail) = id.rsplit('/').next() {
        if !tail.is_empty() && tail != id {
            return tail.to_string();
        }
    }
    if id.len() > 12 {
        id[..12].to_string()
    } else {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_containers_and_tasks() {
        let containers = "\
CONTAINER                                                           IMAGE                                                              RUNTIME
abc123def4567890                                                    registry.k8s.io/pause:3.9                                          io.containerd.runc.v2
k8s.io/pod-uid/nginx                                                docker.io/library/nginx:1.25                                       io.containerd.runc.v2
";
        let tasks = "\
TASK                                                                PID      STATUS
abc123def4567890                                                    1234     RUNNING
";
        let running = parse_task_ids(tasks);
        assert!(running.contains("abc123def4567890"));
        let rows = parse_containers_ls(containers, &running);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].state, "running");
        assert_eq!(rows[0].image, "registry.k8s.io/pause:3.9");
        assert_eq!(rows[1].name, "nginx");
        assert_eq!(rows[1].state, "created");
        assert_eq!(rows[1].namespace, "k8s.io");
    }
}
