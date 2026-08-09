//! List containers via containerd `ctr` (lab CRI introspection).

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

const SOCK: &str = "/run/containerd/containerd.sock";
const NS: &str = "k8s.io";
const CTR_CANDIDATES: &[&str] = &["/usr/local/bin/ctr", "/usr/bin/ctr", "ctr"];

const LABEL_KIND: &str = "io.cri-containerd.kind";
const LABEL_POD_NAME: &str = "io.kubernetes.pod.name";
const LABEL_POD_NS: &str = "io.kubernetes.pod.namespace";
const LABEL_CONTAINER_NAME: &str = "io.kubernetes.container.name";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CriLabels {
    pub kind: String,
    pub pod_name: String,
    pub pod_namespace: String,
    pub container_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub namespace: String,
    pub kind: String,
    pub pod_name: String,
    pub pod_namespace: String,
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

    for row in &mut containers {
        if let Ok(info) = run_ctr(ctr, &["containers", "info", &row.id]) {
            let labels = parse_container_info_labels(&info);
            apply_labels(row, &labels);
        }
    }

    containers.sort_by(|a, b| {
        a.pod_namespace
            .cmp(&b.pod_namespace)
            .then(a.pod_name.cmp(&b.pod_name))
            .then(a.kind.cmp(&b.kind))
            .then(a.name.cmp(&b.name))
            .then(a.id.cmp(&b.id))
    });

    ContainersSnapshot {
        available: true,
        message: format!("listed {} container(s) in {NS}", containers.len()),
        containers,
    }
}

fn apply_labels(row: &mut ContainerRow, labels: &CriLabels) {
    if !labels.kind.is_empty() {
        row.kind = labels.kind.clone();
    }
    if !labels.pod_name.is_empty() {
        row.pod_name = labels.pod_name.clone();
    }
    if !labels.pod_namespace.is_empty() {
        row.pod_namespace = labels.pod_namespace.clone();
    }
    if !labels.container_name.is_empty() {
        row.name = labels.container_name.clone();
    } else if row.kind == "sandbox" && !labels.pod_name.is_empty() {
        row.name = labels.pod_name.clone();
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
            kind: "unknown".into(),
            pod_name: String::new(),
            pod_namespace: String::new(),
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

/// Extract CRI labels from `ctr containers info` JSON.
pub fn parse_container_info_labels(stdout: &str) -> CriLabels {
    let mut labels = CriLabels {
        kind: "unknown".into(),
        ..CriLabels::default()
    };
    if let Some(kind) = json_string_field(stdout, LABEL_KIND) {
        labels.kind = kind;
    }
    if let Some(name) = json_string_field(stdout, LABEL_POD_NAME) {
        labels.pod_name = name;
    }
    if let Some(ns) = json_string_field(stdout, LABEL_POD_NS) {
        labels.pod_namespace = ns;
    }
    if let Some(cname) = json_string_field(stdout, LABEL_CONTAINER_NAME) {
        labels.container_name = cname;
    }
    labels
}

/// Best-effort `"key": "value"` extractor (handles escaped quotes lightly).
fn json_string_field(haystack: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(&needle) {
        let start = from + rel + needle.len();
        let rest = haystack[start..].trim_start();
        if !rest.starts_with(':') {
            from = start;
            continue;
        }
        let rest = rest[1..].trim_start();
        if !rest.starts_with('"') {
            from = start;
            continue;
        }
        let mut out = String::new();
        let bytes = rest.as_bytes();
        let mut i = 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' if i + 1 < bytes.len() => {
                    out.push(bytes[i + 1] as char);
                    i += 2;
                }
                b'"' => return Some(out),
                c => {
                    out.push(c as char);
                    i += 1;
                }
            }
        }
        return None;
    }
    None
}

fn running_contains(running: &HashSet<String>, id: &str) -> bool {
    if running.contains(id) {
        return true;
    }
    // Match short prefix (ctr sometimes shows truncated ids in one listing).
    let short = if id.len() > 12 { &id[..12] } else { id };
    running
        .iter()
        .any(|r| r == id || r.starts_with(short) || id.starts_with(r.as_str()))
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
        assert_eq!(rows[0].kind, "unknown");
        assert_eq!(rows[1].name, "nginx");
        assert_eq!(rows[1].state, "created");
        assert_eq!(rows[1].namespace, "k8s.io");
    }

    #[test]
    fn parses_cri_labels_from_ctr_info_json() {
        let info = r#"{
  "ID": "abc123def4567890",
  "Labels": {
    "io.cri-containerd.kind": "sandbox",
    "io.kubernetes.pod.name": "coredns-5d4dd4d4f-abcde",
    "io.kubernetes.pod.namespace": "kube-system",
    "io.kubernetes.pod.uid": "49b28d61-8984-4ed3-8c4b-0762628c55fe"
  },
  "Image": "registry.k8s.io/pause:3.9"
}"#;
        let labels = parse_container_info_labels(info);
        assert_eq!(labels.kind, "sandbox");
        assert_eq!(labels.pod_name, "coredns-5d4dd4d4f-abcde");
        assert_eq!(labels.pod_namespace, "kube-system");
        assert!(labels.container_name.is_empty());
    }

    #[test]
    fn parses_container_kind_and_applies_name() {
        let info = r#"{
  "Labels": {
    "io.cri-containerd.kind": "container",
    "io.kubernetes.container.name": "coredns",
    "io.kubernetes.pod.name": "coredns-5d4dd4d4f-abcde",
    "io.kubernetes.pod.namespace": "kube-system"
  }
}"#;
        let labels = parse_container_info_labels(info);
        assert_eq!(labels.kind, "container");
        assert_eq!(labels.container_name, "coredns");

        let mut row = ContainerRow {
            id: "deadbeef".into(),
            name: "deadbeef".into(),
            image: "registry.k8s.io/coredns/coredns:v1.11.1".into(),
            state: "running".into(),
            namespace: NS.into(),
            kind: "unknown".into(),
            pod_name: String::new(),
            pod_namespace: String::new(),
        };
        apply_labels(&mut row, &labels);
        assert_eq!(row.kind, "container");
        assert_eq!(row.name, "coredns");
        assert_eq!(row.pod_name, "coredns-5d4dd4d4f-abcde");
        assert_eq!(row.pod_namespace, "kube-system");
    }

    #[test]
    fn sandbox_uses_pod_name_when_no_container_name() {
        let labels = CriLabels {
            kind: "sandbox".into(),
            pod_name: "my-pod".into(),
            pod_namespace: "default".into(),
            container_name: String::new(),
        };
        let mut row = ContainerRow {
            id: "abc".into(),
            name: "abc".into(),
            image: "pause".into(),
            state: "running".into(),
            namespace: NS.into(),
            kind: "unknown".into(),
            pod_name: String::new(),
            pod_namespace: String::new(),
        };
        apply_labels(&mut row, &labels);
        assert_eq!(row.name, "my-pod");
        assert_eq!(row.kind, "sandbox");
    }
}
