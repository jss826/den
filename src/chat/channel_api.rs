//! Channel API endpoints — lightweight message broker between Chat UI and den-channel.
//!
//! Two auth modes:
//! - **User auth** (Cookie/Bearer): for Chat UI endpoints (message, ws, verdict, sessions)
//! - **Token auth** (X-Channel-Token + loopback): for den-channel endpoints (poll, reply, permission)
//!
//! All endpoints are session-aware. User endpoints use a `session` field in the
//! request body or query. Den-channel endpoints identify sessions by token.

use super::channel_state::{ChannelMessage, PermissionRequest, PermissionVerdict};
use super::session::{ChatSessionInfo, CreateSessionRequest};
use crate::AppState;
use axum::Json;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::sync::Arc;

// ── Session management endpoints (user auth) ──────────────────

/// POST /api/channel/sessions — Create a new chat session (starts Claude Code).
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionRequest>,
) -> Response {
    match state
        .chat_sessions
        .create_session(&req.permission_mode)
        .await
    {
        Ok(session) => {
            let info = ChatSessionInfo {
                id: session.id.clone(),
                permission_mode: session.permission_mode.clone(),
                created_at: session.created_at,
                alive: session.is_alive().await,
            };
            (StatusCode::CREATED, Json(info)).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// GET /api/channel/sessions — List all chat sessions.
pub async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<ChatSessionInfo>> {
    Json(state.chat_sessions.list_sessions().await)
}

/// DELETE /api/channel/sessions/{id} — Stop a chat session.
pub async fn stop_session(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.chat_sessions.stop_session(&id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}

// ── UI endpoints (user auth via middleware) ─────────────────────

#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub session: String,
    pub text: String,
    #[serde(default)]
    pub meta: std::collections::HashMap<String, String>,
}

/// POST /api/channel/message — UI sends a user message.
pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendMessageRequest>,
) -> Response {
    let session = match state.chat_sessions.get_session(&req.session).await {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "Session not found").into_response(),
    };
    session.channel_state.push_message(ChannelMessage {
        text: req.text,
        meta: req.meta,
    });
    StatusCode::OK.into_response()
}

#[derive(Deserialize)]
pub struct SendVerdictRequest {
    pub session: String,
    pub request_id: String,
    pub behavior: String,
}

/// POST /api/channel/verdict — UI sends approve/deny for a permission request.
pub async fn send_verdict(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendVerdictRequest>,
) -> Response {
    if req.behavior != "allow" && req.behavior != "deny" {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let session = match state.chat_sessions.get_session(&req.session).await {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "Session not found").into_response(),
    };
    session.channel_state.push_verdict(PermissionVerdict {
        request_id: req.request_id,
        behavior: req.behavior,
    });
    StatusCode::OK.into_response()
}

#[derive(Deserialize)]
pub struct WsQuery {
    pub session: String,
}

/// GET /api/channel/ws?session=<id> — WebSocket for real-time replies + permission events to UI.
pub async fn channel_ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let session = match state.chat_sessions.get_session(&query.session).await {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "Session not found").into_response(),
    };
    ws.on_upgrade(move |socket| handle_channel_ws(socket, session))
        .into_response()
}

async fn handle_channel_ws(mut socket: WebSocket, session: Arc<super::session::ChatSession>) {
    let mut rx = session.channel_state.subscribe();

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
    let session = match state.chat_sessions.find_by_token(&query.token).await {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Try immediate poll first
    if let Some(msg) = session.channel_state.poll_message() {
        return Json(msg).into_response();
    }

    // Long-poll: check every 500ms for up to 30 seconds
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Some(msg) = session.channel_state.poll_message() {
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
    let token = match extract_channel_token(&headers) {
        Some(t) => t,
        None => return StatusCode::UNAUTHORIZED,
    };
    let session = match state.chat_sessions.find_by_token(token).await {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED,
    };
    session
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
    let token = match extract_channel_token(&headers) {
        Some(t) => t,
        None => return StatusCode::UNAUTHORIZED,
    };
    let session = match state.chat_sessions.find_by_token(token).await {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED,
    };
    session.channel_state.push_permission_request(req);
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
    let session = match state.chat_sessions.find_by_token(&query.token).await {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Try immediate poll first
    if let Some(verdict) = session.channel_state.poll_verdict(&query.request_id) {
        return Json(verdict).into_response();
    }

    // Long-poll: check every 500ms for up to 5 minutes
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Some(verdict) = session.channel_state.poll_verdict(&query.request_id) {
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

// ── Helpers ────────────────────────────────────────────────────

fn extract_channel_token(headers: &HeaderMap) -> Option<&str> {
    headers.get("X-Channel-Token").and_then(|v| v.to_str().ok())
}
