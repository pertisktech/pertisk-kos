//! Live UI updates over WebSocket (JWT as the first message, not on the URL).

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures::stream::{SplitStream, StreamExt};
use futures::SinkExt;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::auth::decode_token;
use crate::error::AppError;
use crate::events::MgmtEvent;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws", get(live_ws))
}

#[derive(Debug, Deserialize)]
struct WsAuth {
    token: String,
}

async fn live_ws(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, AppError> {
    Ok(ws.on_upgrade(move |socket| handle_live_ws(state, socket)))
}

fn hello_event() -> MgmtEvent {
    MgmtEvent {
        kind: "hello".into(),
        cluster_id: None,
        job_id: None,
        job_kind: None,
        status: None,
        ts: chrono::Utc::now().timestamp(),
    }
}

async fn read_ws_auth(
    state: &AppState,
    rx: &mut SplitStream<WebSocket>,
) -> Result<(), AppError> {
    let first = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match rx.next().await {
                Some(Ok(Message::Text(text))) => return Ok(text.as_str().to_string()),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None => {
                    return Err(AppError::Unauthorized);
                }
                Some(Ok(_)) => return Err(AppError::Unauthorized),
                Some(Err(_)) => return Err(AppError::Unauthorized),
            }
        }
    })
    .await
    .map_err(|_| AppError::Unauthorized)??;

    let token = serde_json::from_str::<WsAuth>(&first)
        .ok()
        .map(|a| a.token)
        .filter(|t| !t.is_empty())
        .ok_or(AppError::Unauthorized)?;
    decode_token(state.cfg(), &token)?;
    Ok(())
}

async fn handle_live_ws(state: AppState, socket: WebSocket) {
    let (mut tx, mut rx) = socket.split();
    if read_ws_auth(&state, &mut rx).await.is_err() {
        let _ = tx.send(Message::Close(None)).await;
        return;
    }

    let hello = serde_json::to_string(&hello_event()).unwrap_or_else(|_| "{}".into());
    if tx.send(Message::Text(hello.into())).await.is_err() {
        return;
    }

    let mut bus = state.inner.events.subscribe();
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ping.tick() => {
                if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            incoming = rx.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Pong(_))) | Some(Ok(Message::Ping(_))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            ev = bus.recv() => {
                match ev {
                    Ok(ev) => {
                        let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
                        if tx.send(Message::Text(data.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }
}
