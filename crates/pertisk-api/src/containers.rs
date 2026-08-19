//! List containers via containerd `ctr` (lab CRI introspection).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const SOCK: &str = "/run/containerd/containerd.sock";
const NS: &str = "k8s.io";
const CTR_CANDIDATES: &[&str] = &["/usr/local/bin/ctr", "/usr/bin/ctr", "ctr"];

const LABEL_KIND: &str = "io.cri-containerd.kind";
const LABEL_POD_NAME: &str = "io.kubernetes.pod.name";
const LABEL_POD_NS: &str = "io.kubernetes.pod.namespace";
const LABEL_POD_UID: &str = "io.kubernetes.pod.uid";
const LABEL_CONTAINER_NAME: &str = "io.kubernetes.container.name";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CriLabels {
    pub kind: String,
    pub pod_name: String,
    pub pod_namespace: String,
    pub pod_uid: String,
    pub container_name: String,
}

/// Result of resolving a container id to a kubelet CRI log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriLogResolve {
    pub container_id: String,
    pub kind: String,
    pub pod_name: String,
    pub pod_namespace: String,
    pub container_name: String,
    /// Absolute path to the newest `*.log` under `/var/log/pods/...`, if found.
    pub path: Option<String>,
    pub message: String,
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
        return Err(format!("{ctr} {} failed: {}", sub.join(" "), err.trim()));
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
    if let Some(uid) = json_string_field(stdout, LABEL_POD_UID) {
        labels.pod_uid = uid;
    }
    if let Some(cname) = json_string_field(stdout, LABEL_CONTAINER_NAME) {
        labels.container_name = cname;
    }
    labels
}

/// Resolve `container:<id>` to a kubelet CRI log file under `/var/log/pods`.
///
/// Soft-fails with `path = None` and an explanatory `message` when the
/// container, sandbox, or log file cannot be found.
pub fn resolve_cri_log(id_or_prefix: &str) -> CriLogResolve {
    let needle = id_or_prefix.trim();
    if needle.is_empty() {
        return CriLogResolve {
            container_id: String::new(),
            kind: String::new(),
            pod_name: String::new(),
            pod_namespace: String::new(),
            container_name: String::new(),
            path: None,
            message: "empty container id".into(),
        };
    }

    if !Path::new(SOCK).exists() {
        return CriLogResolve {
            container_id: needle.into(),
            kind: String::new(),
            pod_name: String::new(),
            pod_namespace: String::new(),
            container_name: String::new(),
            path: None,
            message: format!("containerd socket missing at {SOCK}"),
        };
    }
    let Some(ctr) = find_ctr() else {
        return CriLogResolve {
            container_id: needle.into(),
            kind: String::new(),
            pod_name: String::new(),
            pod_namespace: String::new(),
            container_name: String::new(),
            path: None,
            message: "ctr binary not found".into(),
        };
    };

    let id = match resolve_container_id(ctr, needle) {
        Ok(id) => id,
        Err(msg) => {
            return CriLogResolve {
                container_id: needle.into(),
                kind: String::new(),
                pod_name: String::new(),
                pod_namespace: String::new(),
                container_name: String::new(),
                path: None,
                message: msg,
            };
        }
    };

    let info = match run_ctr(ctr, &["containers", "info", &id]) {
        Ok(s) => s,
        Err(err) => {
            return CriLogResolve {
                container_id: id,
                kind: String::new(),
                pod_name: String::new(),
                pod_namespace: String::new(),
                container_name: String::new(),
                path: None,
                message: err,
            };
        }
    };
    let labels = parse_container_info_labels(&info);

    if labels.kind == "sandbox" {
        return CriLogResolve {
            container_id: id,
            kind: labels.kind,
            pod_name: labels.pod_name,
            pod_namespace: labels.pod_namespace,
            container_name: String::new(),
            path: None,
            message: "sandbox has no application logs (pick a container id)".into(),
        };
    }

    if labels.pod_namespace.is_empty()
        || labels.pod_name.is_empty()
        || labels.pod_uid.is_empty()
        || labels.container_name.is_empty()
    {
        return CriLogResolve {
            container_id: id,
            kind: labels.kind,
            pod_name: labels.pod_name,
            pod_namespace: labels.pod_namespace,
            container_name: labels.container_name,
            path: None,
            message: "missing CRI pod labels (not a kubelet-managed container?)".into(),
        };
    }

    let dir = PathBuf::from(format!(
        "/var/log/pods/{}_{}_{}/{}",
        labels.pod_namespace, labels.pod_name, labels.pod_uid, labels.container_name
    ));
    match newest_log_in_dir(&dir) {
        Some(path) => CriLogResolve {
            container_id: id,
            kind: labels.kind.clone(),
            pod_name: labels.pod_name.clone(),
            pod_namespace: labels.pod_namespace.clone(),
            container_name: labels.container_name.clone(),
            path: Some(path.display().to_string()),
            message: format!(
                "{}/{} in {}",
                labels.pod_namespace, labels.pod_name, labels.container_name
            ),
        },
        None => CriLogResolve {
            container_id: id,
            kind: labels.kind,
            pod_name: labels.pod_name,
            pod_namespace: labels.pod_namespace,
            container_name: labels.container_name,
            path: None,
            message: format!("(no log file under {})", dir.display()),
        },
    }
}

fn resolve_container_id(ctr: &str, needle: &str) -> Result<String, String> {
    let out = run_ctr(ctr, &["containers", "ls"])?;
    let running = HashSet::new();
    let rows = parse_containers_ls(&out, &running);
    let matches: Vec<&ContainerRow> = rows
        .iter()
        .filter(|r| r.id == needle || r.id.starts_with(needle))
        .collect();
    match matches.as_slice() {
        [] => Err(format!("no container matching id prefix {needle:?}")),
        [one] => Ok(one.id.clone()),
        many => Err(format!(
            "ambiguous container id prefix {needle:?} ({} matches)",
            many.len()
        )),
    }
}

/// Pick the newest `*.log` in a kubelet container log directory.
pub fn newest_log_in_dir(dir: &Path) -> Option<PathBuf> {
    use std::fs;
    use std::time::SystemTime;

    let entries = fs::read_dir(dir).ok()?;
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for ent in entries.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let modified = ent
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        match &best {
            Some((t, _)) if modified <= *t => {}
            _ => best = Some((modified, path)),
        }
    }
    best.map(|(_, p)| p)
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
        assert_eq!(labels.pod_uid, "49b28d61-8984-4ed3-8c4b-0762628c55fe");
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
            pod_uid: "uid-1".into(),
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

    #[test]
    fn picks_newest_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let older = dir.path().join("0.log");
        let newer = dir.path().join("1.log");
        std::fs::write(&older, "old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&newer, "new").unwrap();
        let got = newest_log_in_dir(dir.path()).unwrap();
        assert_eq!(got, newer);
    }

    #[test]
    fn pod_log_dir_layout() {
        let path = format!(
            "/var/log/pods/{}_{}_{}/{}",
            "kube-system", "coredns-x", "uid", "coredns"
        );
        assert_eq!(path, "/var/log/pods/kube-system_coredns-x_uid/coredns");
    }
}
