//! Cluster-scoped Kubernetes workload API + authenticated host shell WebSocket.

use std::io::{Read, Write};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;

use crate::auth::{decode_token, AuthUser};
use crate::error::{ApiResult, AppError};
use crate::k8s::{
    kubectl_json, kubectl_ok, resolve_ready_kubeconfig, transform_cronjob, transform_daemonset,
    transform_deployment, transform_job, transform_namespace, transform_pod, transform_statefulset,
    WorkloadKind,
};
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clusters/{id}/k8s/namespaces", get(list_namespaces))
        .route("/clusters/{id}/k8s/workloads/{kind}", get(list_workloads))
        .route(
            "/clusters/{id}/k8s/workloads/{kind}/{ns}/{name}",
            delete(delete_workload),
        )
        .route(
            "/clusters/{id}/k8s/deployments/{ns}/{name}/scale",
            post(scale_deployment),
        )
        .route(
            "/clusters/{id}/k8s/deployments/{ns}/{name}/restart",
            post(restart_deployment),
        )
        // Host OS shell on the mgmt server with KUBECONFIG pointed at this cluster
        // (kubectl / helm for app install). Not pod exec.
        .route("/clusters/{id}/k8s/shell", get(host_shell_ws))
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
    let data: Vec<_> = items.iter().map(transform_namespace).collect();
    Ok(Json(json!({ "data": data })))
}

async fn list_workloads(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((id, kind)): Path<(String, String)>,
    Query(q): Query<NsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let kind = WorkloadKind::parse(&kind).ok_or_else(|| AppError::bad("unknown workload kind"))?;
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    let resource = kind.kubectl_resource();
    let mut args: Vec<&str> = vec!["get", resource, "-o", "json"];
    let ns;
    if let Some(ref n) = q.namespace {
        if !n.is_empty() && n != "all" {
            ns = n.clone();
            args.extend_from_slice(&["-n", ns.as_str()]);
        } else {
            args.push("-A");
        }
    } else {
        args.push("-A");
    }
    let doc = kubectl_json(&kc, &args).await?;
    let items = doc
        .get("items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    let data: Vec<_> = items
        .iter()
        .map(|obj| match kind {
            WorkloadKind::Deployments => transform_deployment(obj),
            WorkloadKind::StatefulSets => transform_statefulset(obj),
            WorkloadKind::DaemonSets => transform_daemonset(obj),
            WorkloadKind::Jobs => transform_job(obj),
            WorkloadKind::CronJobs => transform_cronjob(obj),
            WorkloadKind::Pods => transform_pod(obj),
        })
        .collect();
    Ok(Json(json!({ "data": data, "kind": kind.as_str() })))
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
    Ok(Json(json!({ "ok": true, "replicas": body.replicas })))
}

async fn restart_deployment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, ns, name)): Path<(String, String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    kubectl_ok(
        &kc,
        &["rollout", "restart", "deployment", &name, "-n", &ns],
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_workload(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, kind, ns, name)): Path<(String, String, String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let kind = WorkloadKind::parse(&kind).ok_or_else(|| AppError::bad("unknown workload kind"))?;
    let (kc, _) = resolve_ready_kubeconfig(&state, &id).await?;
    kubectl_ok(
        &kc,
        &[
            "delete",
            kind.kubectl_resource(),
            &name,
            "-n",
            &ns,
            "--wait=false",
        ],
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
struct ShellQuery {
    /// JWT (WebSocket cannot set Authorization easily).
    token: String,
}

async fn host_shell_ws(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ShellQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, AppError> {
    let claims = decode_token(state.cfg(), &q.token)?;
    let user = AuthUser {
        id: claims.sub,
        username: claims.username,
        role: claims.role,
    };
    require_mutate(&user)?;
    let (kc, cluster_name) = resolve_ready_kubeconfig(&state, &id).await?;
    let kc = Arc::new(kc);
    let cluster_name = Arc::new(cluster_name);
    Ok(ws.on_upgrade(move |socket| handle_host_shell(socket, kc, cluster_name)))
}

async fn handle_host_shell(
    socket: WebSocket,
    kubeconfig: Arc<std::path::PathBuf>,
    cluster_name: Arc<String>,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);

    let session = match spawn_host_shell(&kubeconfig, &cluster_name, &out_tx).await {
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
    for cand in ["/bin/bash", "/usr/bin/bash", "/bin/zsh", "/usr/bin/zsh", "/bin/sh"] {
        if std::path::Path::new(cand).is_file() {
            return cand;
        }
    }
    "/bin/sh"
}

async fn spawn_host_shell(
    kubeconfig: &std::path::Path,
    cluster_name: &str,
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
    let path = std::env::var("PATH").unwrap_or_else(|_| {
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into()
    });
    // Prefer login-capable shells so profile / helm completions load when present.
    let shell = pick_shell_bin();
    let mut cmd = CommandBuilder::new(shell);
    if shell.ends_with("bash") || shell.ends_with("zsh") {
        cmd.arg("-il");
    } else {
        cmd.arg("-i");
    }
    cmd.env("HOME", &home);
    cmd.env("TERM", "xterm-256color");
    cmd.env("LANG", std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()));
    cmd.env("PATH", &path);
    cmd.env("KUBECONFIG", kubeconfig);
    cmd.env("PERTISK_CLUSTER", cluster_name);
    // Helm reads KUBECONFIG the same way.
    cmd.env("HELM_KUBECONTEXT", "");

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
    // Banner after spawn — write via a short delay so the shell owns the TTY first.
    let _ = tx
        .send(format!(
            "\r\n\u{1b}[1;36mpertisk shell\u{1b}[0m · cluster \u{1b}[1m{cluster_name}\u{1b}[0m\r\n\
             KUBECONFIG={}\r\n\
             Use \u{1b}[1mkubectl\u{1b}[0m / \u{1b}[1mhelm\u{1b}[0m to install apps.\r\n\r\n",
            kubeconfig.display()
        ))
        .await;

    Some(PtySession {
        master: pair.master,
        reader,
        writer,
    })
}
