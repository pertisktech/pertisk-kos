//! Audit log read API (Phase D0).

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/audit", get(list))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    resource: Option<String>,
}

fn default_limit() -> i64 {
    100
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AuditOut {
    id: String,
    user_id: Option<String>,
    username: Option<String>,
    action: String,
    resource: Option<String>,
    detail: Option<String>,
    created_at: String,
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_user): CurrentUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<AuditOut>>> {
    let limit = q.limit.clamp(1, 500);
    let offset = q.offset.max(0);
    let action = q.action.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let resource = q
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let rows =
        match (action, resource) {
            (Some(a), Some(r)) => sqlx::query_as::<_, AuditOut>(
                r#"SELECT a.id, a.user_id, u.username, a.action, a.resource, a.detail, a.created_at
                   FROM audit_log a
                   LEFT JOIN users u ON u.id = a.user_id
                   WHERE a.action = ? AND a.resource = ?
                   ORDER BY a.created_at DESC
                   LIMIT ? OFFSET ?"#,
            )
            .bind(a)
            .bind(r)
            .bind(limit)
            .bind(offset)
            .fetch_all(state.pool())
            .await?,
            (Some(a), None) => sqlx::query_as::<_, AuditOut>(
                r#"SELECT a.id, a.user_id, u.username, a.action, a.resource, a.detail, a.created_at
                   FROM audit_log a
                   LEFT JOIN users u ON u.id = a.user_id
                   WHERE a.action = ?
                   ORDER BY a.created_at DESC
                   LIMIT ? OFFSET ?"#,
            )
            .bind(a)
            .bind(limit)
            .bind(offset)
            .fetch_all(state.pool())
            .await?,
            (None, Some(r)) => sqlx::query_as::<_, AuditOut>(
                r#"SELECT a.id, a.user_id, u.username, a.action, a.resource, a.detail, a.created_at
                   FROM audit_log a
                   LEFT JOIN users u ON u.id = a.user_id
                   WHERE a.resource = ?
                   ORDER BY a.created_at DESC
                   LIMIT ? OFFSET ?"#,
            )
            .bind(r)
            .bind(limit)
            .bind(offset)
            .fetch_all(state.pool())
            .await?,
            (None, None) => sqlx::query_as::<_, AuditOut>(
                r#"SELECT a.id, a.user_id, u.username, a.action, a.resource, a.detail, a.created_at
                   FROM audit_log a
                   LEFT JOIN users u ON u.id = a.user_id
                   ORDER BY a.created_at DESC
                   LIMIT ? OFFSET ?"#,
            )
            .bind(limit)
            .bind(offset)
            .fetch_all(state.pool())
            .await?,
        };

    Ok(Json(rows))
}
