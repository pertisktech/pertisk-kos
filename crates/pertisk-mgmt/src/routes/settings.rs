//! Settings / service configuration (read-only, no secrets).

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings", get(settings))
}

#[derive(Serialize)]
struct PathInfo {
    path: String,
    exists: bool,
}

#[derive(Serialize)]
struct SettingsResp {
    service: String,
    version: String,
    listen: String,
    public_url: String,
    /// Reverse-proxied pertisk-kube-web URL (`KUBE_WEB_PUBLIC_URL`), if set.
    kube_web_public_url: Option<String>,
    db: PathInfo,
    data_dir: PathInfo,
    jobs_dir: PathInfo,
    kubeconfigs_dir: PathInfo,
    images_dir: PathInfo,
    lab_up: PathInfo,
    pertiskctl: PathInfo,
    auth: AuthInfo,
    jwt_ttl_secs: i64,
    metrics_token_configured: bool,
}

#[derive(Serialize)]
struct AuthInfo {
    mode: String,
    local: bool,
    auth0: bool,
    admin_user: String,
    admin_password_configured: bool,
    auth0_domain: Option<String>,
    auth0_client_id: Option<String>,
    auth0_audience: Option<String>,
}

fn path_info(p: &std::path::Path) -> PathInfo {
    PathInfo {
        path: p.display().to_string(),
        exists: p.exists(),
    }
}

async fn settings(State(state): State<AppState>) -> Json<SettingsResp> {
    let cfg = state.cfg();
    let auth_mode = match cfg.auth_mode {
        crate::config::AuthMode::Local => "local",
        crate::config::AuthMode::Auth0 => "auth0",
        crate::config::AuthMode::Both => "both",
    };
    Json(SettingsResp {
        service: "pertisk-mgmt".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        listen: cfg.listen.to_string(),
        public_url: cfg.public_url.clone(),
        kube_web_public_url: cfg.kube_web_public_url.clone(),
        db: path_info(&cfg.db),
        data_dir: path_info(&cfg.data_dir),
        jobs_dir: path_info(&cfg.jobs_dir()),
        kubeconfigs_dir: path_info(&cfg.kubeconfigs_dir()),
        images_dir: path_info(&cfg.images_dir),
        lab_up: path_info(&cfg.lab_up),
        pertiskctl: path_info(&cfg.pertiskctl),
        auth: AuthInfo {
            mode: auth_mode.into(),
            local: cfg.auth_mode.allows_local(),
            auth0: cfg.auth_mode.allows_auth0(),
            admin_user: cfg.admin_user.clone(),
            admin_password_configured: cfg.admin_password.is_some(),
            auth0_domain: cfg.auth0_domain.clone(),
            auth0_client_id: cfg.auth0_client_id.clone(),
            auth0_audience: cfg.auth0_audience.clone(),
        },
        jwt_ttl_secs: cfg.jwt_ttl_secs,
        metrics_token_configured: cfg.metrics_token.is_some(),
    })
}
