//! Cluster-scoped Kubernetes resource API, Helm, pod logs/exec, and host shells.

use std::io::{Read, Write};
use std::process::Stdio;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::auth::{audit, decode_token, AuthUser};
use crate::error::{ApiResult, AppError};
use crate::k8s::{
    helm_json, helm_output, helm_with_values, json_to_yaml, kubectl_apply_yaml, kubectl_json,
    kubectl_ok, redact_secret_obj, resolve_ready_kubeconfig, transform_resource, ResourceKind,
    MAX_APPLY_YAML_BYTES,
};
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clusters/{id}/k8s/namespaces", get(list_namespaces))
        // Legacy workload routes (kept for compatibility)
        .route("/clusters/{id}/k8s/workloads/{kind}", get(list_workloads))
        .route(
            "/clusters/{id}/k8s/workloads/{kind}/{ns}/{name}",
            delete(delete_resource_legacy),
        )
        .route(
            "/clusters/{id}/k8s/deployments/{ns}/{name}/scale",
            post(scale_deployment),
        )
        .route(
            "/clusters/{id}/k8s/deployments/{ns}/{name}/restart",
            post(restart_deployment),
        )
        // Resource explorer
        .route("/clusters/{id}/k8s/resources/{kind}", get(list_resources))
        .route(
            "/clusters/{id}/k8s/resources/{kind}/{ns}/{name}",
            get(get_resource)
                .put(apply_resource)
                .delete(delete_resource),
        )
        .route("/clusters/{id}/k8s/apply", post(apply_yaml))
        // Pod logs / exec (WebSocket; JWT via ?token=)
        .route(
            "/clusters/{id}/k8s/pods/{ns}/{name}/logs",
            get(pod_logs_ws),
        )
        .route(
            "/clusters/{id}/k8s/pods/{ns}/{name}/exec",
            get(pod_exec_ws),
        )
        // Helm
        .route("/clusters/{id}/helm/releases", get(list_helm_releases))
        .route(
            "/clusters/{id}/helm/releases/{ns}/{name}",
            get(get_helm_release).delete(uninstall_helm_release),
        )
        .route("/clusters/{id}/helm/install", post(helm_install))
        // Host OS shell on the mgmt server with KUBECONFIG pointed at this cluster
        .route("/clusters/{id}/k8s/shell", get(host_shell_ws))
        .route("/mgmt/shell", get(mgmt_shell_ws))
}

#[derive(Debug, Deserialize)]
struct NsQuery {
    namespace: Option<String>,
}

async fn list_namespaces(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let doc = kubectl_json(&kc, &["get", "namespaces", "-o", "json"]).await?;
    let items = doc
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    let data: Vec<_> = items
        .iter()
        .map(crate::k8s::transform_namespace)
        .collect();
    Ok(Json(json!({ "data": data })))
}

fn transform_kind(kind: ResourceKind, obj: &serde_json::Value) -> serde_json::Value {
    transform_resource(kind.as_str(), obj)
}

async fn list_resources_inner(
    state: &AppState,
    id: &str,
    kind: ResourceKind,
    namespace: Option<&str>,
) -> ApiResult<Json<serde_json::Value>> {
    let (kc, _) = resolve_ready_kubeconfig(state, id).await?;
    let resource = kind.kubectl_resource();
    let mut args: Vec<&str> = vec!["get", resource, "-o", "json"];
    let ns_owned;
    if kind.namespaced() {
        if let Some(n) = namespace {
            if !n.is_empty() && n != "all" {
                ns_owned = n.to_string();
                args.extend_from_slice(&["-n", ns_owned.as_str()]);
            } else {
                args.push("-A");
            }
        } else {
            args.push("-A");
        }
    }
    let doc = kubectl_json(&kc, &args).await?;
    let items = doc
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    let data: Vec<_> = items.iter().map(|obj| transform_kind(kind, obj)).collect();
    Ok(Json(json!({ "data": data, "kind": kind.as_str() })))
}

async fn list_resources(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, kind)): Path<(String, String)>,
    Query(q): Query<NsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let kind = ResourceKind::parse(&kind).ok_or_else(|| AppError::bad("unknown resource kind"))?;
    list_resources_inner(&state, &id, kind, q.namespace.as_deref()).await
}

async fn list_workloads(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, kind)): Path<(String, String)>,
    Query(q): Query<NsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let kind = ResourceKind::parse(&kind).ok_or_else(|| AppError::bad("unknown workload kind"))?;
    if !matches!(
        kind,
        ResourceKind::Deployments
            | ResourceKind::StatefulSets
            | ResourceKind::DaemonSets
            | ResourceKind::Jobs
            | ResourceKind::CronJobs
            | ResourceKind::Pods
    ) {
        return Err(AppError::bad("unknown workload kind"));
    }
    list_resources_inner(&state, &id, kind, q.namespace.as_deref()).await
}

async fn get_resource(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, kind, ns, name)): Path<(String, String, String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let kind = ResourceKind::parse(&kind).ok_or_else(|| AppError::bad("unknown resource kind"))?;
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let resource = kind.kubectl_resource();
    let mut args: Vec<&str> = vec!["get", resource, &name, "-o", "json"];
    if kind.namespaced() {
        let ns_arg = if ns.is_empty() || ns == "_" || ns == "-" {
            "default"
        } else {
            ns.as_str()
        };
        args.extend_from_slice(&["-n", ns_arg]);
    }
    let mut obj = kubectl_json(&kc, &args).await?;
    if kind == ResourceKind::Secrets {
        obj = redact_secret_obj(obj);
    }
    let summary = transform_kind(kind, &obj);
    let yaml = json_to_yaml(&obj)?;

    // Related events (best-effort)
    let events = if kind.namespaced() {
        let ns_arg = if ns.is_empty() || ns == "_" || ns == "-" {
            "default"
        } else {
            ns.as_str()
        };
        let field = format!("involvedObject.name={name}");
        match kubectl_json(
            &kc,
            &["get", "events", "-n", ns_arg, "--field-selector", &field, "-o", "json"],
        )
        .await
        {
            Ok(doc) => doc
                .get("items")
                .and_then(|i| i.as_array())
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|e| transform_resource("events", e))
                .collect::<Vec<_>>(),
            Err(_) => vec![],
        }
    } else {
        vec![]
    };

    Ok(Json(json!({
        "kind": kind.as_str(),
        "summary": summary,
        "yaml": yaml,
        "events": events,
    })))
}

#[derive(Debug, Deserialize)]
struct ApplyBody {
    yaml: String,
}

async fn apply_resource(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, kind, ns, name)): Path<(String, String, String, String)>,
    Json(body): Json<ApplyBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let kind = ResourceKind::parse(&kind).ok_or_else(|| AppError::bad("unknown resource kind"))?;
    if body.yaml.len() > MAX_APPLY_YAML_BYTES {
        return Err(AppError::bad(format!(
            "YAML exceeds {MAX_APPLY_YAML_BYTES} bytes"
        )));
    }
    if kind == ResourceKind::Secrets && body.yaml.contains("***") {
        return Err(AppError::bad(
            "refusing to apply redacted secret YAML; edit secrets via kubectl or paste full values",
        ));
    }
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let out = kubectl_apply_yaml(&kc, &body.yaml).await?;
    audit(
        state.pool(),
        Some(&user.id),
        "k8s.apply",
        Some(&id),
        Some(&format!("{}/{ns}/{name}", kind.as_str())),
    )
    .await;
    Ok(Json(json!({ "ok": true, "output": out })))
}

async fn delete_resource(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, kind, ns, name)): Path<(String, String, String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let kind = ResourceKind::parse(&kind).ok_or_else(|| AppError::bad("unknown resource kind"))?;
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let resource = kind.kubectl_resource();
    let mut args: Vec<&str> = vec!["delete", resource, &name, "--wait=false"];
    if kind.namespaced() {
        let ns_arg = if ns.is_empty() || ns == "_" || ns == "-" {
            "default"
        } else {
            ns.as_str()
        };
        args.extend_from_slice(&["-n", ns_arg]);
    }
    kubectl_ok(&kc, &args).await?;
    audit(
        state.pool(),
        Some(&user.id),
        "k8s.delete",
        Some(&id),
        Some(&format!("{}/{ns}/{name}", kind.as_str())),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_resource_legacy(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, kind, ns, name)): Path<(String, String, String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    delete_resource(State(state), CurrentUser(user), Path((id, kind, ns, name))).await
}

async fn apply_yaml(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<ApplyBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    if body.yaml.trim().is_empty() {
        return Err(AppError::bad("yaml is required"));
    }
    if body.yaml.len() > MAX_APPLY_YAML_BYTES {
        return Err(AppError::bad(format!(
            "YAML exceeds {MAX_APPLY_YAML_BYTES} bytes"
        )));
    }
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let out = kubectl_apply_yaml(&kc, &body.yaml).await?;
    audit(
        state.pool(),
        Some(&user.id),
        "k8s.apply_yaml",
        Some(&id),
        Some(&format!("{} bytes", body.yaml.len())),
    )
    .await;
    Ok(Json(json!({ "ok": true, "output": out })))
}

#[derive(Debug, Deserialize)]
struct ScaleBody {
    replicas: u32,
}

async fn scale_deployment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, ns, name)): Path<(String, String, String)>,
    Json(body): Json<ScaleBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let replicas = body.replicas.to_string();
    kubectl_ok(
        &kc,
        &[
            "scale",
            "deployment",
            &name,
            "-n",
            &ns,
            &format!("--replicas={replicas}"),
        ],
    )
    .await?;
    audit(
        state.pool(),
        Some(&user.id),
        "k8s.scale",
        Some(&id),
        Some(&format!("{ns}/{name}={replicas}")),
    )
    .await;
    Ok(Json(json!({ "ok": true, "replicas": body.replicas })))
}

async fn restart_deployment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, ns, name)): Path<(String, String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    kubectl_ok(&kc, &["rollout", "restart", "deployment", &name, "-n", &ns]).await?;
    audit(
        state.pool(),
        Some(&user.id),
        "k8s.restart",
        Some(&id),
        Some(&format!("{ns}/{name}")),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/* ─── Helm ─────────────────────────────────────────────────────────────── */

async fn list_helm_releases(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let doc = helm_json(&kc, &["list", "-A", "-o", "json"]).await?;
    let data = if let Some(arr) = doc.as_array() {
        arr.clone()
    } else {
        vec![]
    };
    Ok(Json(json!({ "data": data })))
}

async fn get_helm_release(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, ns, name)): Path<(String, String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let status = helm_output(
        Some(&kc),
        &["status", &name, "-n", &ns, "-o", "json"],
    )
    .await?;
    let status_json: serde_json::Value =
        serde_json::from_str(status.trim()).unwrap_or(json!({ "raw": status }));
    let values = helm_output(Some(&kc), &["get", "values", &name, "-n", &ns, "-a"]).await?;
    // Light redaction of obvious secret keys in values text
    let values_safe = redact_values_text(&values);
    Ok(Json(json!({
        "name": name,
        "namespace": ns,
        "status": status_json,
        "values": values_safe,
    })))
}

fn redact_values_text(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            let sensitive = ["password", "secret", "token", "apikey", "api_key", "privatekey"]
                .iter()
                .any(|k| lower.contains(k));
            if sensitive && line.contains(':') {
                let (k, _) = line.split_once(':').unwrap();
                format!("{k}: \"***\"")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Deserialize)]
struct HelmInstallBody {
    name: String,
    namespace: String,
    chart: String,
    repo: Option<String>,
    version: Option<String>,
    values_yaml: Option<String>,
    create_namespace: Option<bool>,
}

async fn helm_install(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<HelmInstallBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    if body.name.trim().is_empty() || body.chart.trim().is_empty() || body.namespace.trim().is_empty()
    {
        return Err(AppError::bad("name, namespace, and chart are required"));
    }
    if let Some(ref v) = body.values_yaml {
        if v.len() > MAX_APPLY_YAML_BYTES {
            return Err(AppError::bad(format!(
                "values YAML exceeds {MAX_APPLY_YAML_BYTES} bytes"
            )));
        }
    }
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;

    if let Some(repo) = body.repo.as_deref().filter(|r| !r.is_empty()) {
        let repo_name = format!("pertisk-tmp-{}", &body.name);
        let _ = helm_output(Some(&kc), &["repo", "add", &repo_name, repo, "--force-update"]).await;
        let _ = helm_output(Some(&kc), &["repo", "update", &repo_name]).await;
    }

    let chart = if let Some(repo) = body.repo.as_deref().filter(|r| !r.is_empty()) {
        let _ = repo;
        // Prefer chart as given (oci:// or repo/chart). If bare name + repo URL, use name.
        body.chart.clone()
    } else {
        body.chart.clone()
    };

    let mut args: Vec<String> = vec![
        "upgrade".into(),
        "--install".into(),
        body.name.clone(),
        chart,
        "-n".into(),
        body.namespace.clone(),
    ];
    if body.create_namespace.unwrap_or(true) {
        args.push("--create-namespace".into());
    }
    if let Some(ver) = body.version.as_deref().filter(|v| !v.is_empty()) {
        args.push("--version".into());
        args.push(ver.to_string());
    }
    if body.repo.as_deref().filter(|r| !r.is_empty()).is_some()
        && !body.chart.contains('/')
        && !body.chart.starts_with("oci://")
    {
        // chart is bare name — use --repo
        if let Some(repo) = &body.repo {
            args.push("--repo".into());
            args.push(repo.clone());
        }
    }

    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = helm_with_values(
        &kc,
        &args_ref,
        body.values_yaml.as_deref().filter(|s| !s.trim().is_empty()),
    )
    .await?;
    audit(
        state.pool(),
        Some(&user.id),
        "helm.install",
        Some(&id),
        Some(&format!("{}/{}", body.namespace, body.name)),
    )
    .await;
    Ok(Json(json!({ "ok": true, "output": out })))
}

async fn uninstall_helm_release(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, ns, name)): Path<(String, String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let out = helm_output(Some(&kc), &["uninstall", &name, "-n", &ns]).await?;
    audit(
        state.pool(),
        Some(&user.id),
        "helm.uninstall",
        Some(&id),
        Some(&format!("{ns}/{name}")),
    )
    .await;
    Ok(Json(json!({ "ok": true, "output": out })))
}

/* ─── Pod logs / exec WebSockets ───────────────────────────────────────── */

#[derive(Debug, Deserialize)]
struct ShellQuery {
    token: String,
}

#[derive(Debug, Deserialize)]
struct PodLogsQuery {
    token: String,
    container: Option<String>,
    follow: Option<String>,
    tail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PodExecQuery {
    token: String,
    container: Option<String>,
    shell: Option<String>,
}

fn auth_from_token(state: &AppState, token: &str) -> ApiResult<AuthUser> {
    let claims = decode_token(state.cfg(), token)?;
    Ok(AuthUser {
        id: claims.sub,
        username: claims.username,
        role: claims.role,
        provider: claims.provider,
    })
}

async fn pod_logs_ws(
    State(state): State<AppState>,
    Path((id, ns, name)): Path<(String, String, String)>,
    Query(q): Query<PodLogsQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, AppError> {
    let _user = auth_from_token(&state, &q.token)?;
    // viewers may follow logs
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let follow = q
        .follow
        .as_deref()
        .map(|f| f == "1" || f.eq_ignore_ascii_case("true"))
        .unwrap_or(true);
    let tail = q.tail.unwrap_or_else(|| "200".into());
    let container = q.container.clone();
    Ok(ws.on_upgrade(move |socket| {
        handle_pod_logs(socket, kc, ns, name, container, follow, tail)
    }))
}

async fn handle_pod_logs(
    socket: WebSocket,
    kc: std::path::PathBuf,
    ns: String,
    name: String,
    container: Option<String>,
    follow: bool,
    tail: String,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut args: Vec<String> = vec![
        "--kubeconfig".into(),
        kc.display().to_string(),
        "logs".into(),
        name.clone(),
        "-n".into(),
        ns.clone(),
        format!("--tail={tail}"),
    ];
    if follow {
        args.push("-f".into());
    }
    if let Some(c) = container.filter(|c| !c.is_empty()) {
        args.push("-c".into());
        args.push(c);
    }

    let mut child = match Command::new("kubectl")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = ws_tx
                .send(Message::Text(format!("failed to start kubectl logs: {e}\n").into()))
                .await;
            let _ = ws_tx.close().await;
            return;
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);

    if let Some(out) = stdout {
        let tx = out_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(format!("{line}\n")).await.is_err() {
                    break;
                }
            }
        });
    }
    if let Some(err) = stderr {
        let tx = out_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(format!("{line}\n")).await.is_err() {
                    break;
                }
            }
        });
    }
    drop(out_tx);

    let forward = tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            if ws_tx.send(Message::Text(chunk.into())).await.is_err() {
                break;
            }
        }
        let _ = ws_tx.close().await;
    });

    while let Some(Ok(msg)) = ws_rx.next().await {
        if matches!(msg, Message::Close(_)) {
            break;
        }
    }
    let _ = child.kill().await;
    let _ = forward.await;
}

async fn pod_exec_ws(
    State(state): State<AppState>,
    Path((id, ns, name)): Path<(String, String, String)>,
    Query(q): Query<PodExecQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_from_token(&state, &q.token)?;
    require_mutate(&user)?;
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    audit(
        state.pool(),
        Some(&user.id),
        "k8s.exec",
        Some(&id),
        Some(&format!("{ns}/{name}")),
    )
    .await;
    let container = q.container.clone();
    let shell = q.shell.clone().unwrap_or_else(|| "sh".into());
    Ok(ws.on_upgrade(move |socket| {
        handle_pod_exec(socket, kc, ns, name, container, shell)
    }))
}

async fn handle_pod_exec(
    socket: WebSocket,
    kc: std::path::PathBuf,
    ns: String,
    name: String,
    container: Option<String>,
    shell: String,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);

    let pty_system = NativePtySystem::default();
    let pair = match pty_system.openpty(PtySize {
        rows: 30,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(err) => {
            let _ = ws_tx
                .send(Message::Text(
                    format!("\r\n\u{1b}[1;31mFailed to create PTY: {err}\u{1b}[0m\r\n").into(),
                ))
                .await;
            let _ = ws_tx.close().await;
            return;
        }
    };

    let mut cmd = CommandBuilder::new("kubectl");
    cmd.arg("--kubeconfig");
    cmd.arg(kc.display().to_string());
    cmd.arg("exec");
    cmd.arg("-it");
    cmd.arg(&name);
    cmd.arg("-n");
    cmd.arg(&ns);
    if let Some(c) = container.filter(|c| !c.is_empty()) {
        cmd.arg("-c");
        cmd.arg(c);
    }
    cmd.arg("--");
    cmd.arg(&shell);
    cmd.arg("-i");
    cmd.env("TERM", "xterm-256color");

    if let Err(err) = pair.slave.spawn_command(cmd) {
        let _ = ws_tx
            .send(Message::Text(
                format!("\r\n\u{1b}[1;31mFailed to start kubectl exec: {err}\u{1b}[0m\r\n").into(),
            ))
            .await;
        let _ = ws_tx.close().await;
        return;
    }

    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(_) => return,
    };
    let writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(_) => return,
    };
    let master = pair.master;
    let writer = Arc::new(std::sync::Mutex::new(writer));
    let master = Arc::new(std::sync::Mutex::new(master));

    let _ = out_tx
        .send(format!(
            "\r\n\u{1b}[1;36mpod exec\u{1b}[0m · {ns}/{name}\r\n\r\n"
        ))
        .await;

    let out_tx2 = out_tx.clone();
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let s = String::from_utf8_lossy(&buf[..n]).to_string();
                    if out_tx2.blocking_send(s).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let forward = tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            if ws_tx.send(Message::Text(chunk.into())).await.is_err() {
                break;
            }
        }
        let _ = ws_tx.close().await;
    });

    let writer_in = writer.clone();
    let master_in = master.clone();
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(t) => {
                if t.starts_with('{') {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                        if v.get("type").and_then(|x| x.as_str()) == Some("resize") {
                            let cols = v.get("cols").and_then(|c| c.as_u64()).unwrap_or(120) as u16;
                            let rows = v.get("rows").and_then(|r| r.as_u64()).unwrap_or(30) as u16;
                            if let Ok(m) = master_in.lock() {
                                let _ = m.resize(PtySize {
                                    rows: rows.max(2),
                                    cols: cols.max(2),
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                            }
                            continue;
                        }
                    }
                }
                if let Ok(mut w) = writer_in.lock() {
                    let _ = w.write_all(t.as_bytes());
                    let _ = w.flush();
                }
            }
            Message::Binary(b) => {
                if let Ok(mut w) = writer_in.lock() {
                    let _ = w.write_all(&b);
                    let _ = w.flush();
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    drop(writer);
    drop(master);
    let _ = forward.await;
}

/* ─── Host shells ──────────────────────────────────────────────────────── */

async fn host_shell_ws(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ShellQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_from_token(&state, &q.token)?;
    require_mutate(&user)?;
    let (kc, cluster_name) = resolve_ready_kubeconfig(&state, &id).await?;
    let kc = Arc::new(kc);
    let cluster_name = Arc::new(cluster_name);
    Ok(ws.on_upgrade(move |socket| {
        handle_host_shell(socket, Some(kc), Some(cluster_name), "cluster")
    }))
}

async fn mgmt_shell_ws(
    State(state): State<AppState>,
    Query(q): Query<ShellQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, AppError> {
    let user = auth_from_token(&state, &q.token)?;
    require_mutate(&user)?;
    Ok(ws.on_upgrade(|socket| handle_host_shell(socket, None, None, "mgmt")))
}

async fn handle_host_shell(
    socket: WebSocket,
    kubeconfig: Option<Arc<std::path::PathBuf>>,
    cluster_name: Option<Arc<String>>,
    mode: &'static str,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);

    let session = match spawn_host_shell(
        kubeconfig.as_deref().map(|p| p.as_path()),
        cluster_name.as_deref().map(|s| s.as_str()),
        mode,
        &out_tx,
    )
    .await
    {
        Some(s) => s,
        None => {
            let _ = ws_tx
                .send(Message::Text(
                    "\r\n\u{1b}[1;31mFailed to start host shell\u{1b}[0m\r\n".into(),
                ))
                .await;
            let _ = ws_tx.close().await;
            return;
        }
    };

    let master = session.master;
    let mut reader = session.reader;
    let writer = session.writer;
    let writer = Arc::new(std::sync::Mutex::new(writer));
    let master = Arc::new(std::sync::Mutex::new(master));

    let out_tx2 = out_tx.clone();
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let s = String::from_utf8_lossy(&buf[..n]).to_string();
                    if out_tx2.blocking_send(s).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let forward = tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            if ws_tx.send(Message::Text(chunk.into())).await.is_err() {
                break;
            }
        }
        let _ = ws_tx.close().await;
    });

    let writer_in = writer.clone();
    let master_in = master.clone();
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(t) => {
                if t.starts_with('{') {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                        if v.get("type").and_then(|x| x.as_str()) == Some("resize") {
                            let cols = v.get("cols").and_then(|c| c.as_u64()).unwrap_or(120) as u16;
                            let rows = v.get("rows").and_then(|r| r.as_u64()).unwrap_or(30) as u16;
                            if let Ok(m) = master_in.lock() {
                                let _ = m.resize(PtySize {
                                    rows: rows.max(2),
                                    cols: cols.max(2),
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                            }
                            continue;
                        }
                    }
                }
                if let Ok(mut w) = writer_in.lock() {
                    let _ = w.write_all(t.as_bytes());
                    let _ = w.flush();
                }
            }
            Message::Binary(b) => {
                if let Ok(mut w) = writer_in.lock() {
                    let _ = w.write_all(&b);
                    let _ = w.flush();
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    drop(writer);
    drop(master);
    let _ = forward.await;
}

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
}

fn pick_shell_bin() -> &'static str {
    for cand in [
        "/bin/bash",
        "/usr/bin/bash",
        "/bin/zsh",
        "/usr/bin/zsh",
        "/bin/sh",
    ] {
        if std::path::Path::new(cand).is_file() {
            return cand;
        }
    }
    "/bin/sh"
}

async fn spawn_host_shell(
    kubeconfig: Option<&std::path::Path>,
    cluster_name: Option<&str>,
    mode: &str,
    tx: &mpsc::Sender<String>,
) -> Option<PtySession> {
    let pty_system = NativePtySystem::default();
    let pair = match pty_system.openpty(PtySize {
        rows: 30,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(err) => {
            let _ = tx
                .send(format!(
                    "\r\n\u{1b}[1;31mFailed to create PTY: {err}\u{1b}[0m\r\n"
                ))
                .await;
            return None;
        }
    };

    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let path = std::env::var("PATH")
        .unwrap_or_else(|_| "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into());
    let shell = pick_shell_bin();
    let mut cmd = CommandBuilder::new(shell);
    if shell.ends_with("bash") || shell.ends_with("zsh") {
        cmd.arg("-il");
    } else {
        cmd.arg("-i");
    }
    cmd.env("HOME", &home);
    cmd.env("TERM", "xterm-256color");
    cmd.env(
        "LANG",
        std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()),
    );
    cmd.env("PATH", &path);
    if let Some(kc) = kubeconfig {
        cmd.env("KUBECONFIG", kc);
        if let Some(name) = cluster_name {
            cmd.env("PERTISK_CLUSTER", name);
        }
        cmd.env("HELM_KUBECONTEXT", "");
    }

    if let Err(err) = pair.slave.spawn_command(cmd) {
        let _ = tx
            .send(format!(
                "\r\n\u{1b}[1;31mFailed to start host shell: {err}\u{1b}[0m\r\n"
            ))
            .await;
        return None;
    }

    let reader = pair.master.try_clone_reader().ok()?;
    let writer = pair.master.take_writer().ok()?;
    let banner = if mode == "mgmt" {
        "\r\n\u{1b}[1;36mpertisk mgmt shell\u{1b}[0m · \u{1b}[1mpertiskctl\u{1b}[0m / ops tools\r\n\
         No cluster KUBECONFIG — use \u{1b}[1mpertiskctl\u{1b}[0m against guests or providers.\r\n\r\n"
            .to_string()
    } else {
        let name = cluster_name.unwrap_or("cluster");
        let kc = kubeconfig
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        format!(
            "\r\n\u{1b}[1;36mpertisk shell\u{1b}[0m · cluster \u{1b}[1m{name}\u{1b}[0m\r\n\
             KUBECONFIG={kc}\r\n\
             Use \u{1b}[1mkubectl\u{1b}[0m / \u{1b}[1mhelm\u{1b}[0m to install apps.\r\n\r\n"
        )
    };
    let _ = tx.send(banner).await;

    Some(PtySession {
        master: pair.master,
        reader,
        writer,
    })
}
