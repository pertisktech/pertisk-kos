mod addons;
mod audit;
mod auth_routes;
mod clusters;
mod dashboard;
mod events;
mod health;
pub(crate) mod images;
mod join_tokens;
pub(crate) mod k8s;
mod machines;
mod meta;
pub(crate) mod nodes;
pub(crate) mod os_packages;
pub(crate) mod providers;
mod settings;
mod templates;
mod users;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::middleware::{from_fn_with_state, Next};
use axum::response::Response;
use axum::Router;

use crate::auth::{decode_token, AuthUser};
use crate::error::{ApiResult, AppError};
use crate::state::AppState;
use crate::static_files;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(health::routes())
        .merge(auth_routes::routes())
        .merge(users::routes())
        .merge(meta::routes())
        .merge(settings::routes())
        .merge(dashboard::routes())
        .merge(events::routes())
        .merge(providers::routes())
        .merge(clusters::routes())
        .merge(nodes::routes())
        .merge(k8s::routes())
        .merge(addons::routes())
        .merge(audit::routes())
        .merge(machines::routes())
        .merge(templates::routes())
        .merge(os_packages::routes())
        .merge(images::routes())
        .merge(join_tokens::routes())
        .layer(from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .nest("/api", api)
        .fallback(static_files::static_handler)
        .with_state(state)
}

async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let path = req.uri().path();
    let public = path == "/health"
        || path == "/auth/login"
        || path == "/auth/mode"
        || path == "/auth/logout"
        || path.starts_with("/auth/oidc/")
        || path.starts_with("/auth/password-reset/");
    if public {
        return Ok(next.run(req).await);
    }

    // Host shell WebSocket + SSE: JWT arrives as ?token= (browsers cannot set Authorization).
    if path.ends_with("/k8s/shell") || path == "/events" {
        return Ok(next.run(req).await);
    }

    let auth = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let Some(token) = auth else {
        return Err(AppError::Unauthorized);
    };
    let claims = decode_token(state.cfg(), token)?;
    let user = AuthUser {
        id: claims.sub,
        username: claims.username,
        role: claims.role,
        provider: claims.provider,
    };
    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
}

/// Extract authenticated user from request extensions.
pub struct CurrentUser(pub AuthUser);

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> ApiResult<Self> {
        parts
            .extensions
            .get::<AuthUser>()
            .cloned()
            .map(CurrentUser)
            .ok_or(AppError::Unauthorized)
    }
}
