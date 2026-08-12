use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{
    audit, find_or_create_auth0_user, find_user_by_username, issue_token, verify_password, AuthUser,
    Role,
};
use crate::error::{ApiResult, AppError};
use crate::rbac::role_from_claims;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/mode", get(auth_mode))
        .route("/auth/login", post(login))
        .route("/auth/me", get(me))
        .route("/auth/logout", get(logout))
        .route("/auth/oidc/start", get(oidc_start))
        .route("/auth/oidc/callback", get(oidc_callback))
}

#[derive(Serialize)]
struct ModeResp {
    mode: String,
    local: bool,
    auth0: bool,
    auth0_domain: Option<String>,
    auth0_client_id: Option<String>,
}

async fn auth_mode(State(state): State<AppState>) -> Json<ModeResp> {
    let cfg = state.cfg();
    Json(ModeResp {
        mode: match cfg.auth_mode {
            crate::config::AuthMode::Local => "local",
            crate::config::AuthMode::Auth0 => "auth0",
            crate::config::AuthMode::Both => "both",
        }
        .into(),
        local: cfg.auth_mode.allows_local(),
        auth0: cfg.auth_mode.allows_auth0(),
        auth0_domain: cfg.auth0_domain.clone(),
        auth0_client_id: cfg.auth0_client_id.clone(),
    })
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct TokenResp {
    token: String,
    username: String,
    role: Role,
    provider: String,
}

async fn login(State(state): State<AppState>, Json(body): Json<LoginReq>) -> ApiResult<Json<TokenResp>> {
    if !state.cfg().auth_mode.allows_local() {
        return Err(AppError::bad("local auth disabled"));
    }
    let Some((id, hash, role)) = find_user_by_username(state.pool(), &body.username).await? else {
        return Err(AppError::Unauthorized);
    };
    let Some(hash) = hash else {
        return Err(AppError::Unauthorized);
    };
    if !verify_password(&body.password, &hash) {
        return Err(AppError::Unauthorized);
    }
    let user = AuthUser {
        id: id.clone(),
        username: body.username.clone(),
        role,
        provider: "local".into(),
    };
    let token = issue_token(state.cfg(), &user)?;
    audit(
        state.pool(),
        Some(&id),
        "login",
        Some("local"),
        Some(&body.username),
    )
    .await;
    Ok(Json(TokenResp {
        token,
        username: user.username,
        role: user.role,
        provider: "local".into(),
    }))
}

async fn me(CurrentUser(user): CurrentUser) -> Json<Value> {
    Json(json!({
        "id": user.id,
        "username": user.username,
        "role": user.role,
        "provider": user.provider,
    }))
}

/// End the browser Auth0 SSO session, then return to the login page.
/// Without this, "Continue with Auth0" silently reuses the previous account.
async fn logout(State(state): State<AppState>) -> impl IntoResponse {
    let cfg = state.cfg();
    let return_to = format!("{}/", cfg.public_url.trim_end_matches('/'));
    if !cfg.auth_mode.allows_auth0() {
        return Redirect::temporary("/#/login");
    }
    let Some(domain) = cfg.auth0_domain.as_ref() else {
        return Redirect::temporary("/#/login");
    };
    let Some(client_id) = cfg.auth0_client_id.as_ref() else {
        return Redirect::temporary("/#/login");
    };
    // `federated` also clears the upstream IdP session (e.g. Google) so the
    // next login can show an account picker instead of the previous user.
    let url = format!(
        "https://{domain}/v2/logout?client_id={client_id}&returnTo={}&federated",
        urlencoding(&return_to)
    );
    Redirect::temporary(&url)
}

async fn oidc_start(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    if !state.cfg().auth_mode.allows_auth0() {
        return Err(AppError::bad("Auth0 disabled"));
    }
    let domain = state.cfg().auth0_domain.as_ref().unwrap();
    let client_id = state.cfg().auth0_client_id.as_ref().unwrap();
    let redirect = format!("{}/api/auth/oidc/callback", state.cfg().public_url.trim_end_matches('/'));
    let state_nonce = Uuid::new_v4().to_string();
    // prompt=login forces Auth0 to show the login / account UI instead of
    // silently reusing an existing SSO cookie.
    let url = format!(
        "https://{domain}/authorize?response_type=code&client_id={client_id}&redirect_uri={}&scope=openid%20profile%20email&state={state_nonce}&prompt=login",
        urlencoding(&redirect)
    );
    Ok(Redirect::temporary(&url))
}

#[derive(Deserialize)]
struct OidcCallback {
    code: Option<String>,
    error: Option<String>,
}

async fn oidc_callback(
    State(state): State<AppState>,
    Query(q): Query<OidcCallback>,
) -> ApiResult<impl IntoResponse> {
    if !state.cfg().auth_mode.allows_auth0() {
        return Err(AppError::bad("Auth0 disabled"));
    }
    if let Some(err) = q.error {
        return Err(AppError::bad(format!("oidc error: {err}")));
    }
    let code = q.code.ok_or_else(|| AppError::bad("missing code"))?;
    let cfg = state.cfg();
    let domain = cfg.auth0_domain.as_ref().unwrap();
    let client_id = cfg.auth0_client_id.as_ref().unwrap();
    let client_secret = cfg.auth0_client_secret.as_ref().unwrap();
    let redirect = format!("{}/api/auth/oidc/callback", cfg.public_url.trim_end_matches('/'));

    let token_url = format!("https://{domain}/oauth/token");
    let resp = state
        .inner
        .http
        .post(&token_url)
        .json(&json!({
            "grant_type": "authorization_code",
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
            "redirect_uri": redirect,
        }))
        .send()
        .await
        .map_err(|e| AppError::bad(format!("token exchange: {e}")))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::bad(format!("token exchange failed: {body}")));
    }
    let token_body: Value = resp.json().await.map_err(|e| AppError::Anyhow(e.into()))?;
    let access = token_body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::bad("no access_token"))?;

    // Fetch userinfo
    let userinfo: Value = state
        .inner
        .http
        .get(format!("https://{domain}/userinfo"))
        .bearer_auth(access)
        .send()
        .await
        .map_err(|e| AppError::bad(format!("userinfo: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Anyhow(e.into()))?;

    let sub = userinfo
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::bad("no sub"))?;
    let username = userinfo
        .get("email")
        .or_else(|| userinfo.get("nickname"))
        .or_else(|| userinfo.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or(sub)
        .to_string();
    let role = role_from_claims(&userinfo);
    let user = find_or_create_auth0_user(state.pool(), sub, &username, role).await?;
    let jwt = issue_token(cfg, &user)?;
    audit(
        state.pool(),
        Some(&user.id),
        "login",
        Some("auth0"),
        Some(&username),
    )
    .await;

    // Redirect to UI with token in hash (SPA picks it up)
    let dest = format!("/#/auth/callback?token={jwt}");
    Ok(Redirect::temporary(&dest))
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
