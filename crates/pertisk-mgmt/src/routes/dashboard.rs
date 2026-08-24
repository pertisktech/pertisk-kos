//! Dashboard aggregate endpoints.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::cluster_resources::{self, ClusterResourceSummary};
use crate::error::ApiResult;
use crate::provider_resources::{self, ProviderResourceSummary};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dashboard/resources", get(resources))
        .route("/dashboard/providers", get(providers))
}

async fn resources(State(state): State<AppState>) -> ApiResult<Json<Vec<ClusterResourceSummary>>> {
    Ok(Json(cluster_resources::gather_all(&state).await))
}

async fn providers(State(state): State<AppState>) -> ApiResult<Json<Vec<ProviderResourceSummary>>> {
    Ok(Json(provider_resources::gather_all(&state).await))
}
