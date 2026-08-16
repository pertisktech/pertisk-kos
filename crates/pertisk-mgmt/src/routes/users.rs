//! Admin user management and public local password-reset endpoints.

use axum::extract::{Path, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{
    audit, consume_password_reset_token, create_password_reset_token, ensure_not_last_enabled_admin,
    get_user, hash_password, list_users, set_user_password, Role, UserRecord,
};
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::mail::{self, password_reset_email};
use crate::rbac::require_admin;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", get(list).post(create))
        .route("/users/{id}", patch(update))
        .route("/users/{id}/reset-password", post(admin_reset))
        .route("/auth/password-reset/request", post(reset_request))
        .route("/auth/password-reset/confirm", post(reset_confirm))
}

#[derive(Serialize)]
struct UserResp {
    id: String,
    username: String,
    email: Option<String>,
    role: Role,
    source: &'static str,
    local: bool,
    disabled: bool,
    created_at: String,
    updated_at: Option<String>,
}

impl From<UserRecord> for UserResp {
    fn from(u: UserRecord) -> Self {
        Self {
            id: u.id.clone(),
            username: u.username.clone(),
            email: u.email.clone(),
            role: u.role,
            source: u.source(),
            local: u.is_local(),
            disabled: u.disabled,
            created_at: u.created_at,
            updated_at: u.updated_at,
        }
    }
}

async fn list(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> ApiResult<Json<Vec<UserResp>>> {
    require_admin(&user)?;
    let users = list_users(state.pool()).await?;
    Ok(Json(users.into_iter().map(UserResp::from).collect()))
}

#[derive(Deserialize)]
struct CreateUserReq {
    username: String,
    email: Option<String>,
    role: String,
    /// Admin-set temporary password. Mutually exclusive with `send_reset_email`.
    password: Option<String>,
    /// When true, email a reset link instead of setting a password.
    #[serde(default)]
    send_reset_email: bool,
}

async fn create(
    State(state): State<AppState>,
    CurrentUser(actor): CurrentUser,
    Json(body): Json<CreateUserReq>,
) -> ApiResult<Json<UserResp>> {
    require_admin(&actor)?;
    if !state.cfg().auth_mode.allows_local() {
        return Err(AppError::bad("local auth disabled"));
    }
    let username = body.username.trim().to_string();
    if username.is_empty() {
        return Err(AppError::bad("username required"));
    }
    let role = Role::parse(&body.role).ok_or_else(|| AppError::bad("invalid role"))?;
    let email = body
        .email
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if body.send_reset_email {
        if email.is_none() {
            return Err(AppError::bad("email required to send reset"));
        }
        if !mail::mail_enabled(state.cfg()) {
            return Err(AppError::bad("SMTP is not configured"));
        }
    } else {
        let pw = body.password.as_deref().unwrap_or("").trim();
        if pw.len() < 8 {
            return Err(AppError::bad("password must be at least 8 characters"));
        }
    }

    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    let password_hash = if body.send_reset_email {
        None
    } else {
        Some(
            hash_password(body.password.as_deref().unwrap_or(""))
                .map_err(AppError::Anyhow)?,
        )
    };

    let result = sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, email, disabled, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
    )
    .bind(&id)
    .bind(&username)
    .bind(&password_hash)
    .bind(role.as_str())
    .bind(&email)
    .bind(&now)
    .bind(&now)
    .execute(state.pool())
    .await;

    if let Err(e) = result {
        if let sqlx::Error::Database(db_err) = &e {
            if db_err.message().contains("UNIQUE") {
                return Err(AppError::Conflict("username already exists".into()));
            }
        }
        return Err(e.into());
    }

    if body.send_reset_email {
        let raw = create_password_reset_token(state.pool(), &id).await?;
        let (subject, body_text) = password_reset_email(state.cfg(), &username, &raw);
        mail::spawn_send(
            state.cfg(),
            vec![email.clone().unwrap()],
            subject,
            body_text,
        );
        audit(
            state.pool(),
            Some(&actor.id),
            "user.reset_request",
            Some(&id),
            Some(&username),
        )
        .await;
    }

    audit(
        state.pool(),
        Some(&actor.id),
        "user.create",
        Some(&id),
        Some(&format!("{username} role={}", role.as_str())),
    )
    .await;

    let user = get_user(state.pool(), &id)
        .await?
        .ok_or_else(|| AppError::Anyhow(anyhow::anyhow!("user missing after create")))?;
    Ok(Json(UserResp::from(user)))
}

#[derive(Deserialize)]
struct UpdateUserReq {
    role: Option<String>,
    disabled: Option<bool>,
    email: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    CurrentUser(actor): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserReq>,
) -> ApiResult<Json<UserResp>> {
    require_admin(&actor)?;
    let mut user = get_user(state.pool(), &id)
        .await?
        .ok_or(AppError::NotFound)?;

    if let Some(role_s) = body.role.as_ref() {
        let new_role = Role::parse(role_s).ok_or_else(|| AppError::bad("invalid role"))?;
        if user.role.is_admin() && !new_role.is_admin() {
            ensure_not_last_enabled_admin(state.pool(), &user).await?;
        }
        let now = db::now_rfc3339();
        sqlx::query("UPDATE users SET role = ?, updated_at = ? WHERE id = ?")
            .bind(new_role.as_str())
            .bind(&now)
            .bind(&id)
            .execute(state.pool())
            .await?;
        audit(
            state.pool(),
            Some(&actor.id),
            "user.role_change",
            Some(&id),
            Some(&format!("{} -> {}", user.role.as_str(), new_role.as_str())),
        )
        .await;
        user.role = new_role;
    }

    if let Some(disabled) = body.disabled {
        if disabled && !user.disabled {
            ensure_not_last_enabled_admin(state.pool(), &user).await?;
        }
        let now = db::now_rfc3339();
        sqlx::query("UPDATE users SET disabled = ?, updated_at = ? WHERE id = ?")
            .bind(if disabled { 1 } else { 0 })
            .bind(&now)
            .bind(&id)
            .execute(state.pool())
            .await?;
        audit(
            state.pool(),
            Some(&actor.id),
            if disabled {
                "user.disable"
            } else {
                "user.enable"
            },
            Some(&id),
            Some(&user.username),
        )
        .await;
        user.disabled = disabled;
    }

    if let Some(email) = body.email {
        let email = email.trim().to_string();
        let email = if email.is_empty() { None } else { Some(email) };
        let now = db::now_rfc3339();
        sqlx::query("UPDATE users SET email = ?, updated_at = ? WHERE id = ?")
            .bind(&email)
            .bind(&now)
            .bind(&id)
            .execute(state.pool())
            .await?;
        user.email = email;
    }

    let user = get_user(state.pool(), &id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(UserResp::from(user)))
}

#[derive(Serialize)]
struct OkMsg {
    ok: bool,
}

/// Admin-initiated reset email for a local user.
async fn admin_reset(
    State(state): State<AppState>,
    CurrentUser(actor): CurrentUser,
    Path(id): Path<String>,
) -> ApiResult<Json<OkMsg>> {
    require_admin(&actor)?;
    if !state.cfg().auth_mode.allows_local() {
        return Err(AppError::bad("local auth disabled"));
    }
    if !mail::mail_enabled(state.cfg()) {
        return Err(AppError::bad("SMTP is not configured"));
    }
    let user = get_user(state.pool(), &id)
        .await?
        .ok_or(AppError::NotFound)?;
    if user.is_auth0_only() {
        return Err(AppError::bad("cannot reset password for Auth0-only users"));
    }
    let email = user
        .email
        .clone()
        .filter(|e| !e.is_empty())
        .ok_or_else(|| AppError::bad("user has no email address"))?;

    let raw = create_password_reset_token(state.pool(), &id).await?;
    let (subject, body_text) = password_reset_email(state.cfg(), &user.username, &raw);
    mail::spawn_send(state.cfg(), vec![email], subject, body_text);
    audit(
        state.pool(),
        Some(&actor.id),
        "user.reset_request",
        Some(&id),
        Some(&user.username),
    )
    .await;
    Ok(Json(OkMsg { ok: true }))
}

#[derive(Deserialize)]
struct ResetRequestReq {
    /// Username or email (enumeration-safe: always succeeds).
    identifier: String,
}

/// Public: always returns the same success payload.
async fn reset_request(
    State(state): State<AppState>,
    Json(body): Json<ResetRequestReq>,
) -> ApiResult<Json<OkMsg>> {
    let ok = Json(OkMsg { ok: true });
    if !state.cfg().auth_mode.allows_local() {
        return Ok(ok);
    }
    if !mail::mail_enabled(state.cfg()) {
        return Ok(ok);
    }
    let ident = body.identifier.trim();
    if ident.is_empty() {
        return Ok(ok);
    }

    let row = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>, i64)>(
        "SELECT id, username, email, password_hash, auth0_sub, COALESCE(disabled, 0) FROM users \
         WHERE username = ? OR email = ?",
    )
    .bind(ident)
    .bind(ident)
    .fetch_optional(state.pool())
    .await?;

    if let Some((id, username, email, password_hash, auth0_sub, disabled)) = row {
        let auth0_only = auth0_sub.is_some() && password_hash.is_none();
        let can_reset = disabled == 0 && !auth0_only;
        if can_reset {
            if let Some(email) = email.filter(|e| !e.is_empty()) {
                match create_password_reset_token(state.pool(), &id).await {
                    Ok(raw) => {
                        let (subject, body_text) =
                            password_reset_email(state.cfg(), &username, &raw);
                        mail::spawn_send(state.cfg(), vec![email], subject, body_text);
                        audit(
                            state.pool(),
                            Some(&id),
                            "user.reset_request",
                            Some(&id),
                            Some(&username),
                        )
                        .await;
                    }
                    Err(e) => tracing::warn!(error = %e, "reset token create failed"),
                }
            }
        }
    }

    Ok(ok)
}

#[derive(Deserialize)]
struct ResetConfirmReq {
    token: String,
    password: String,
}

async fn reset_confirm(
    State(state): State<AppState>,
    Json(body): Json<ResetConfirmReq>,
) -> ApiResult<Json<OkMsg>> {
    if !state.cfg().auth_mode.allows_local() {
        return Err(AppError::bad("local auth disabled"));
    }
    let password = body.password.trim();
    if password.len() < 8 {
        return Err(AppError::bad("password must be at least 8 characters"));
    }
    let user_id = consume_password_reset_token(state.pool(), body.token.trim()).await?;
    let user = get_user(state.pool(), &user_id)
        .await?
        .ok_or_else(|| AppError::bad("invalid or expired reset token"))?;
    if user.disabled {
        return Err(AppError::bad("account disabled"));
    }
    if user.is_auth0_only() {
        return Err(AppError::bad("cannot set password for Auth0-only users"));
    }
    set_user_password(state.pool(), &user_id, password).await?;
    audit(
        state.pool(),
        Some(&user_id),
        "user.reset_complete",
        Some(&user_id),
        Some(&user.username),
    )
    .await;
    Ok(Json(OkMsg { ok: true }))
}
