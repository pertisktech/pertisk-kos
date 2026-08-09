//! Server-Sent Events stream for dashboard / cluster UI refresh.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use futures::stream::{Stream, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::auth::decode_token;
use crate::error::{ApiResult, AppError};
use crate::events::MgmtEvent;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/events", get(events))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    /// JWT — EventSource cannot set Authorization headers.
    token: String,
}

async fn events(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let _claims = decode_token(state.cfg(), &q.token)?;
    if q.token.is_empty() {
        return Err(AppError::Unauthorized);
    }

    let rx = state.inner.events.subscribe();
    let hello = MgmtEvent {
        kind: "hello".into(),
        cluster_id: None,
        job_id: None,
        job_kind: None,
        status: None,
        ts: chrono::Utc::now().timestamp(),
    };

    let stream = futures::stream::once(async move {
        Ok::<_, Infallible>(
            Event::default()
                .event("hello")
                .data(serde_json::to_string(&hello).unwrap_or_else(|_| "{}".into())),
        )
    })
    .chain(futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let name = ev.kind.clone();
                    let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
                    let event = Event::default().event(name).data(data);
                    return Some((Ok::<_, Infallible>(event), rx));
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    }));

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}
