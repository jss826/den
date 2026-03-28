//! Channel API endpoints — lightweight message broker between Chat UI and den-channel.
//!
//! Two auth modes:
//! - **User auth** (Cookie/Bearer): for Chat UI endpoints (message, ws, verdict)
//! - **Token auth** (X-Channel-Token + loopback): for den-channel endpoints (poll, reply, permission)

use super::channel_state::{ChannelMessage, PermissionRequest, PermissionVerdict};
use crate::AppState;
use axum::Json;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::sync::Arc;

// ── Token auth helper ──────────────────────────────────────────

/// Extract and validate the channel token from X-Channel-Token header.
fn validate_channel_token(headers: &HeaderMap, state: &AppState) -> bool {
    headers
        .get("X-Channel-Token")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|token| state.channel_state.validate_token(token))
}

// ── UI endpoints (user auth via middleware) ─────────────────────

/// POST /api/channel/message — UI sends a user message.
pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Json(msg): Json<ChannelMessage>,
) -> StatusCode {
    state.channel_state.push_message(msg);
    StatusCode::OK
}

/// POST /api/channel/verdict — UI sends approve/deny for a permission request.
pub async fn send_verdict(
    State(state): State<Arc<AppState>>,
    Json(verdict): Json<PermissionVerdict>,
) -> StatusCode {
    if verdict.behavior != "allow" && verdict.behavior != "deny" {
        return StatusCode::BAD_REQUEST;
    }
    state.channel_state.push_verdict(verdict);
    StatusCode::OK
}

/// GET /api/channel/ws — WebSocket for real-time replies + permission events to UI.
pub async fn channel_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_channel_ws(socket, state))
}

async fn handle_channel_ws(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.channel_state.subscribe();

    // Forward broadcast events to WebSocket
    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let json = match serde_json::to_string(&event) {
                            Ok(j) => j,
                            Err(_) => continue,
                        };
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("channel ws lagged by {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            // Also handle incoming messages (ping/pong, close)
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

// ── Channel server endpoints (token auth) ──────────────────────

#[derive(Deserialize)]
pub struct PollQuery {
    pub token: String,
    #[allow(dead_code)]
    pub session: Option<String>,
}

/// GET /api/channel/poll — den-channel fetches pending messages.
/// Long-polls: waits up to 30 seconds for a message, then returns empty.
pub async fn poll_message(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PollQuery>,
) -> Response {
    if !state.channel_state.validate_token(&query.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Try immediate poll first
    if let Some(msg) = state.channel_state.poll_message() {
        return Json(msg).into_response();
    }

    // Long-poll: check every 500ms for up to 30 seconds
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Some(msg) = state.channel_state.poll_message() {
            return Json(msg).into_response();
        }
        if tokio::time::Instant::now() >= deadline {
            return StatusCode::NO_CONTENT.into_response();
        }
    }
}

/// Reply payload from den-channel.
#[derive(Deserialize)]
pub struct ReplyPayload {
    pub chat_id: String,
    pub text: String,
}

/// POST /api/channel/reply — den-channel posts Claude's reply.
pub async fn post_reply(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ReplyPayload>,
) -> StatusCode {
    if !validate_channel_token(&headers, &state) {
        return StatusCode::UNAUTHORIZED;
    }
    state
        .channel_state
        .broadcast_reply(payload.chat_id, payload.text);
    StatusCode::OK
}

/// POST /api/channel/permission — den-channel forwards a permission request.
pub async fn post_permission(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PermissionRequest>,
) -> StatusCode {
    if !validate_channel_token(&headers, &state) {
        return StatusCode::UNAUTHORIZED;
    }
    state.channel_state.push_permission_request(req);
    StatusCode::OK
}

#[derive(Deserialize)]
pub struct VerdictQuery {
    pub token: String,
    pub request_id: String,
}

/// GET /api/channel/verdict — den-channel polls for user's permission decision.
/// Long-polls: waits up to 5 minutes for a verdict.
pub async fn poll_verdict(
    State(state): State<Arc<AppState>>,
    Query(query): Query<VerdictQuery>,
) -> Response {
    if !state.channel_state.validate_token(&query.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Try immediate poll first
    if let Some(verdict) = state.channel_state.poll_verdict(&query.request_id) {
        return Json(verdict).into_response();
    }

    // Long-poll: check every 500ms for up to 5 minutes
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Some(verdict) = state.channel_state.poll_verdict(&query.request_id) {
            return Json(verdict).into_response();
        }
        if tokio::time::Instant::now() >= deadline {
            // Timeout: auto-deny
            return Json(PermissionVerdict {
                request_id: query.request_id,
                behavior: "deny".into(),
            })
            .into_response();
        }
    }
}
