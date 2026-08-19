//! Cluster join tokens (Phase D2) — snapshot of kube bootstrap token + join docs.
//!
//! Not a new auth system: copies token/endpoint/CA from the cluster's worker.yaml
//! so operators can hand bare-metal hosts join instructions (or use adopt).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::audit;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clusters/{id}/join-tokens", get(list).post(create))
        .route(
            "/clusters/{cid}/join-tokens/{tid}",
            get(get_one).delete(revoke),
        )
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct JoinTokenOut {
    id: String,
    cluster_id: String,
    role: String,
    label: String,
    token: String,
    endpoint: String,
    ca_pem: Option<String>,
    expires_at: Option<String>,
    created_by: Option<String>,
    created_at: String,
    revoked_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct JoinTokenDetail {
    #[serde(flatten)]
    token: JoinTokenOut,
    /// Human-readable join steps for copy/paste.
    instructions: String,
    cp_ip: Option<String>,
}

#[derive(Deserialize)]
struct CreateJoinToken {
    /// worker | controlplane (instructions differ; token is the same bootstrap secret).
    #[serde(default = "default_role")]
    role: String,
    #[serde(default)]
    label: String,
    /// Optional TTL hours (omit = no expiry recorded).
    #[serde(default)]
    expires_hours: Option<i64>,
}

fn default_role() -> String {
    "worker".into()
}

fn normalize_role(role: &str) -> ApiResult<String> {
    match role.trim().to_ascii_lowercase().as_str() {
        "controlplane" | "cp" => Ok("controlplane".into()),
        "worker" | "wk" | "" => Ok("worker".into()),
        other => Err(AppError::bad(format!(
            "role must be controlplane|worker (got {other})"
        ))),
    }
}

async fn cluster_name(state: &AppState, id: &str) -> ApiResult<String> {
    let row: Option<(String,)> = sqlx::query_as("SELECT name FROM clusters WHERE id = ?")
        .bind(id)
        .fetch_optional(state.pool())
        .await?;
    row.map(|r| r.0).ok_or(AppError::NotFound)
}

async fn resolve_cp_ip(state: &AppState, cid: &str) -> Option<String> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT ip FROM nodes WHERE cluster_id = ? AND role = 'controlplane' \
         AND ip IS NOT NULL AND ip != '' ORDER BY name ASC LIMIT 1",
    )
    .bind(cid)
    .fetch_optional(state.pool())
    .await
    .ok()
    .flatten();
    if let Some((Some(ip),)) = row {
        return Some(ip);
    }
    let vip: Option<(Option<String>,)> = sqlx::query_as("SELECT vip FROM clusters WHERE id = ?")
        .bind(cid)
        .fetch_optional(state.pool())
        .await
        .ok()
        .flatten();
    vip.and_then(|(v,)| v.filter(|s| !s.is_empty()))
}

fn read_bootstrap_from_worker(
    state: &AppState,
    cluster_name: &str,
) -> ApiResult<(String, String, Option<String>)> {
    let path = state
        .cfg()
        .kubeconfigs_dir()
        .join(cluster_name)
        .join("worker.yaml");
    if !path.is_file() {
        return Err(AppError::bad(format!(
            "missing {} — create/bootstrap the cluster first",
            path.display()
        )));
    }
    let yaml = std::fs::read_to_string(&path)
        .map_err(|e| AppError::bad(format!("read worker.yaml: {e}")))?;
    let cfg = pertisk_config::MachineConfig::from_yaml(&yaml)
        .map_err(|e| AppError::bad(format!("parse worker.yaml: {e}")))?;
    let cluster = cfg
        .cluster
        .as_ref()
        .ok_or_else(|| AppError::bad("worker.yaml missing cluster block"))?;
    let token = cluster
        .token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("worker.yaml has no cluster.token"))?
        .to_string();
    let endpoint = cluster.endpoint.trim().to_string();
    if endpoint.is_empty() {
        return Err(AppError::bad("worker.yaml has empty cluster.endpoint"));
    }
    let ca = cluster
        .ca
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok((token, endpoint, ca))
}

fn build_instructions(
    role: &str,
    endpoint: &str,
    token: &str,
    cp_ip: Option<&str>,
    cluster_name: &str,
) -> String {
    let cp = cp_ip.unwrap_or("<CP_IP>");
    let mut out = String::new();
    out.push_str("# Kubernetes bootstrap (kubelet TLS)\n");
    out.push_str(&format!("endpoint: {endpoint}\n"));
    out.push_str(&format!("token:    {token}\n"));
    out.push('\n');
    out.push_str("# Prefer: Adopt existing node in the mgmt UI (Machine API :50000)\n");
    out.push_str("# Or from the mgmt host:\n");
    out.push_str(&format!(
        "./scripts/adopt-node.sh --role {role} --name {cluster_name}-… \\\n"
    ));
    out.push_str(&format!("  --node-ip <NODE_IP> --cp-ip {cp} \\\n"));
    out.push_str(&format!(
        "  --cluster-out ./out/kubeconfigs/{cluster_name} --cluster-name {cluster_name}\n"
    ));
    out.push('\n');
    if role == "controlplane" {
        out.push_str("# Manual control-plane join:\n");
        out.push_str(&format!(
            "pertiskctl -e {cp}:50000 get-join-config --controlplane \\\n"
        ));
        out.push_str("  --controlplane-index <N> --cluster-name ");
        out.push_str(cluster_name);
        out.push_str(" -o controlplane-N.yaml\n");
        out.push_str("pertiskctl -e <NODE_IP>:50000 apply -f controlplane-N.yaml\n");
        out.push_str(&format!(
            "pertiskctl -e <NODE_IP>:50000 join-controlplane --etcd-endpoints https://{cp}:2379\n"
        ));
    } else {
        out.push_str("# Manual worker join:\n");
        out.push_str(&format!(
            "pertiskctl -e {cp}:50000 join-config -f worker.yaml\n"
        ));
        out.push_str("# set machine.network.hostname, then:\n");
        out.push_str("pertiskctl -e <NODE_IP>:50000 apply -f worker.yaml\n");
    }
    out
}

fn detail(token: JoinTokenOut, cp_ip: Option<String>, cluster_name: &str) -> JoinTokenDetail {
    let instructions = build_instructions(
        &token.role,
        &token.endpoint,
        &token.token,
        cp_ip.as_deref(),
        cluster_name,
    );
    JoinTokenDetail {
        token,
        instructions,
        cp_ip,
    }
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<JoinTokenOut>>> {
    let _ = cluster_name(&state, &id).await?;
    let rows = sqlx::query_as::<_, JoinTokenOut>(
        r#"SELECT id, cluster_id, role, label, token, endpoint, ca_pem,
                  expires_at, created_by, created_at, revoked_at
           FROM join_tokens
           WHERE cluster_id = ?
           ORDER BY created_at DESC"#,
    )
    .bind(&id)
    .fetch_all(state.pool())
    .await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((cid, tid)): Path<(String, String)>,
) -> ApiResult<Json<JoinTokenDetail>> {
    let name = cluster_name(&state, &cid).await?;
    let row = sqlx::query_as::<_, JoinTokenOut>(
        r#"SELECT id, cluster_id, role, label, token, endpoint, ca_pem,
                  expires_at, created_by, created_at, revoked_at
           FROM join_tokens WHERE id = ? AND cluster_id = ?"#,
    )
    .bind(&tid)
    .bind(&cid)
    .fetch_optional(state.pool())
    .await?
    .ok_or(AppError::NotFound)?;
    let cp = resolve_cp_ip(&state, &cid).await;
    Ok(Json(detail(row, cp, &name)))
}

async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<CreateJoinToken>,
) -> ApiResult<Json<JoinTokenDetail>> {
    require_mutate(&user)?;
    let name = cluster_name(&state, &id).await?;
    let role = normalize_role(&body.role)?;
    let (token, endpoint, ca) = read_bootstrap_from_worker(&state, &name)?;
    let tid = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    let expires_at = body.expires_hours.and_then(|h| {
        if h <= 0 {
            return None;
        }
        Some((chrono::Utc::now() + chrono::Duration::hours(h)).to_rfc3339())
    });
    let label = body.label.trim().to_string();
    sqlx::query(
        r#"INSERT INTO join_tokens
           (id, cluster_id, role, label, token, endpoint, ca_pem, expires_at, created_by, created_at, revoked_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)"#,
    )
    .bind(&tid)
    .bind(&id)
    .bind(&role)
    .bind(&label)
    .bind(&token)
    .bind(&endpoint)
    .bind(&ca)
    .bind(&expires_at)
    .bind(&user.id)
    .bind(&now)
    .execute(state.pool())
    .await?;
    audit(
        state.pool(),
        Some(&user.id),
        "join_token.create",
        Some(&id),
        Some(&format!("{role} {label}")),
    )
    .await;
    let row = sqlx::query_as::<_, JoinTokenOut>(
        r#"SELECT id, cluster_id, role, label, token, endpoint, ca_pem,
                  expires_at, created_by, created_at, revoked_at
           FROM join_tokens WHERE id = ?"#,
    )
    .bind(&tid)
    .fetch_one(state.pool())
    .await?;
    let cp = resolve_cp_ip(&state, &id).await;
    Ok(Json(detail(row, cp, &name)))
}

async fn revoke(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((cid, tid)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let now = db::now_rfc3339();
    let res = sqlx::query(
        "UPDATE join_tokens SET revoked_at = ? WHERE id = ? AND cluster_id = ? AND revoked_at IS NULL",
    )
    .bind(&now)
    .bind(&tid)
    .bind(&cid)
    .execute(state.pool())
    .await?;
    if res.rows_affected() == 0 {
        let exists: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM join_tokens WHERE id = ? AND cluster_id = ?")
                .bind(&tid)
                .bind(&cid)
                .fetch_optional(state.pool())
                .await?;
        if exists.is_none() {
            return Err(AppError::NotFound);
        }
        // Already revoked — idempotent OK.
    }
    audit(
        state.pool(),
        Some(&user.id),
        "join_token.revoke",
        Some(&tid),
        None,
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true, "revoked_at": now })))
}
