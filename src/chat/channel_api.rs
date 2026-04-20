//! Channel API endpoints — lightweight message broker between Chat UI and den-channel.
//!
//! Two auth modes:
//! - **User auth** (Cookie/Bearer): for Chat UI endpoints (message, ws, verdict, sessions)
//! - **Token auth** (X-Channel-Token + loopback): for den-channel endpoints (poll, reply, permission)
//!
//! All endpoints are session-aware. User endpoints use a `session` field in the
//! request body or query. Den-channel endpoints identify sessions by token.

use super::channel_state::{PermissionRequest, PermissionVerdict};
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
        .create_session(
            &req.permission_mode,
            req.cwd.as_deref(),
            &req.auto_tools,
            &req.escalate_tools,
        )
        .await
    {
        Ok(session) => {
            let info = ChatSessionInfo {
                id: session.id.clone(),
                permission_mode: session.permission_mode.clone(),
                created_at: session.created_at,
                alive: session.is_alive().await,
                cwd: session.cwd.clone(),
                auto_tools: session.auto_tools.clone(),
                escalate_tools: session.escalate_tools.clone(),
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
    /// Retained for API compatibility with earlier clients; no longer used now
    /// that messages go directly to Claude Code over stdin stream-json.
    #[serde(default)]
    #[allow(dead_code)]
    pub meta: std::collections::HashMap<String, String>,
}

/// POST /api/channel/message — UI sends a user message.
///
/// The message is serialized as a stream-json `user` event and written directly
/// to Claude Code's stdin. Claude Code's assistant reply arrives on stdout and
/// is forwarded to the UI via the reply WebSocket broadcast (see
/// `spawn_stdout_parse_task` in `session.rs`).
pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendMessageRequest>,
) -> Response {
    let session = match state.chat_sessions.get_session(&req.session).await {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "Session not found").into_response(),
    };
    match session.send_input(&req.text).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => {
            tracing::warn!(
                chat_session = %req.session,
                "send_message: stdin forward failed: {e}"
            );
            (StatusCode::INTERNAL_SERVER_ERROR, e).into_response()
        }
    }
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
pub struct SendDirectiveRequest {
    pub session: String,
    pub text: String,
}

/// POST /api/channel/directive — UI pushes a one-shot directive that the worker
/// picks up via the MCP `check_directive` tool. Overwrites any pending
/// directive so a newer instruction always wins (matches orch's file-based
/// `HUB_DIRECTIVE` semantics).
pub async fn send_directive(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendDirectiveRequest>,
) -> Response {
    let trimmed = req.text.trim();
    if trimmed.is_empty() {
        return (StatusCode::BAD_REQUEST, "Directive text must be non-empty").into_response();
    }
    let session = match state.chat_sessions.get_session(&req.session).await {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "Session not found").into_response(),
    };
    session.channel_state.set_directive(trimmed.to_string());
    tracing::info!(
        chat_session = %session.id,
        "directive queued ({} chars)",
        trimmed.len()
    );
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
                        // A lagged subscriber has already lost events and would
                        // remain desynced indefinitely if we kept polling. Drop
                        // the connection so the UI reconnects and rebuilds state
                        // from a clean subscribe().
                        tracing::warn!(
                            "channel ws lagged by {n} events; disconnecting subscriber"
                        );
                        break;
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
///
/// Uses `tokio::sync::Notify` so newly pushed messages wake the poll
/// immediately instead of the pre-#101-Phase-3 500 ms sleep-poll floor.
pub async fn poll_message(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PollQuery>,
) -> Response {
    let session = match state.chat_sessions.find_by_token(&query.token).await {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);

    loop {
        // Register interest BEFORE checking the queue so a message pushed
        // between the check and the await is still observed.
        let notified = session.channel_state.message_notify().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if let Some(msg) = session.channel_state.poll_message() {
            return Json(msg).into_response();
        }

        tokio::select! {
            _ = notified => {
                // Loop back and re-check; a spurious wake is harmless.
            }
            _ = tokio::time::sleep_until(deadline) => {
                return StatusCode::NO_CONTENT.into_response();
            }
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

/// Payload shape for `/api/channel/status` and `/api/channel/notification`.
///
/// Both endpoints are targets of the Claude Code hook runner (`den --chat-hook`).
/// The hook relays the raw hook JSON untouched so future UI surfaces can
/// inspect tool names, file paths, etc. without a server-side schema pin.
#[derive(Deserialize)]
pub struct StatusPayload {
    /// Hook name: `session-start` / `stop` / `post-tool-use`.
    pub event: String,
    /// Raw JSON payload the hook received on stdin.
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Deserialize)]
pub struct NotificationPayload {
    /// Raw JSON payload from the Notification hook.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// POST /api/channel/status — den-chat-hook posts a session lifecycle event.
pub async fn post_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<StatusPayload>,
) -> StatusCode {
    let token = match extract_channel_token(&headers) {
        Some(t) => t,
        None => return StatusCode::UNAUTHORIZED,
    };
    let session = match state.chat_sessions.find_by_token(token).await {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED,
    };
    tracing::info!(
        chat_session = %session.id,
        hook_event = %req.event,
        "chat hook status"
    );
    session
        .channel_state
        .broadcast_status(req.event, req.payload);
    StatusCode::OK
}

#[derive(Deserialize)]
pub struct DirectiveQuery {
    pub token: String,
}

/// GET /api/channel/directive — den-channel (MCP) polls the pending directive.
///
/// Returns `{ text }` on hit, 204 on empty. The directive is consumed by the
/// first reader so a Worker that successfully calls `check_directive` won't
/// re-surface the same instruction on the next call.
pub async fn get_directive(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirectiveQuery>,
) -> Response {
    let session = match state.chat_sessions.find_by_token(&query.token).await {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    match session.channel_state.take_directive() {
        Some(text) => Json(serde_json::json!({ "text": text })).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

/// POST /api/channel/notification — den-chat-hook posts a Notification hook payload.
pub async fn post_notification(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<NotificationPayload>,
) -> StatusCode {
    let token = match extract_channel_token(&headers) {
        Some(t) => t,
        None => return StatusCode::UNAUTHORIZED,
    };
    let session = match state.chat_sessions.find_by_token(token).await {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED,
    };
    tracing::info!(
        chat_session = %session.id,
        "chat hook notification"
    );
    session.channel_state.broadcast_notification(req.payload);
    StatusCode::OK
}

#[derive(Deserialize)]
pub struct VerdictQuery {
    pub token: String,
    pub request_id: String,
}

/// GET /api/channel/verdict — den-channel polls for user's permission decision.
/// Long-polls: waits up to 5 minutes for a verdict, then auto-denies.
///
/// Uses `tokio::sync::Notify` (woken by `push_verdict` via `notify_waiters()`)
/// so the caller returns as soon as a verdict for the requested id lands,
/// instead of paying the 500 ms sleep-poll floor from #86.
pub async fn poll_verdict(
    State(state): State<Arc<AppState>>,
    Query(query): Query<VerdictQuery>,
) -> Response {
    let session = match state.chat_sessions.find_by_token(&query.token).await {
        Some(s) => s,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);

    loop {
        // Register interest BEFORE checking the verdict map so a verdict
        // pushed between the check and the await is still observed.
        let notified = session.channel_state.verdict_notify().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if let Some(verdict) = session.channel_state.poll_verdict(&query.request_id) {
            return Json(verdict).into_response();
        }

        tokio::select! {
            _ = notified => {
                // A verdict arrived for *some* request_id; re-check ours.
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Json(PermissionVerdict {
                    request_id: query.request_id,
                    behavior: "deny".into(),
                })
                .into_response();
            }
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────

fn extract_channel_token(headers: &HeaderMap) -> Option<&str> {
    headers.get("X-Channel-Token").and_then(|v| v.to_str().ok())
}
