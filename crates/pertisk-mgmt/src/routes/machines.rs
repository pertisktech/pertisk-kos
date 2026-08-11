//! Cross-cluster machine inventory (Phase D0).

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::error::ApiResult;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/machines", get(list))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct MachineOut {
    id: String,
    name: String,
    role: String,
    status: String,
    ip: Option<String>,
    ip6: Option<String>,
    k8s_version: Option<String>,
    vmid: Option<i64>,
    /// proxmox | vsphere | adopted | baremetal
    source: String,
    cluster_id: String,
    cluster_name: String,
    cluster_status: String,
    provider_name: Option<String>,
    /// 1 when AK public is stored (TOFU enrolled).
    ak_enrolled: i64,
    ak_enrolled_at: Option<String>,
    updated_at: String,
    /// Live Machine API reachability: `online` | `offline` | `unknown` (not stored).
    #[sqlx(skip)]
    #[serde(default = "default_availability")]
    availability: String,
}

fn default_availability() -> String {
    "unknown".into()
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_user): CurrentUser,
) -> ApiResult<Json<Vec<MachineOut>>> {
    let mut rows = sqlx::query_as::<_, MachineOut>(
        r#"SELECT n.id, n.name, n.role, n.status, n.ip, n.ip6, n.k8s_version, n.vmid,
                  COALESCE(n.source, 'proxmox') AS source,
                  n.cluster_id, c.name AS cluster_name, c.status AS cluster_status,
                  p.name AS provider_name,
                  CASE WHEN n.ak_public_b64 IS NOT NULL AND n.ak_public_b64 != '' THEN 1 ELSE 0 END AS ak_enrolled,
                  n.ak_enrolled_at,
                  n.updated_at
           FROM nodes n
           JOIN clusters c ON c.id = n.cluster_id
           LEFT JOIN providers p ON p.id = c.provider_id
           ORDER BY c.name, n.role, n.name"#,
    )
    .fetch_all(state.pool())
    .await?;

    let futs: Vec<_> = rows
        .iter()
        .map(|m| crate::node_availability::probe(m.ip.as_deref(), &m.status))
        .collect();
    let avails = futures::future::join_all(futs).await;
    for (m, a) in rows.iter_mut().zip(avails) {
        m.availability = a;
    }

    Ok(Json(rows))
}
