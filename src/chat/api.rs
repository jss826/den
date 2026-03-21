use axum::{
    Json,
    extract::{
        Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::broadcast;

use super::manager::{ChatError, ChatManager};
use crate::AppState;

// ── REST endpoints ──────────────────────────────────────────────

#[derive(Deserialize, Default)]
pub struct CreateSessionRequest {
    /// Claude CLI session ID to resume (from persisted session).
    #[serde(default)]
    pub resume_session_id: Option<String>,
    /// Working directory for the new session.
    #[serde(default)]
    pub cwd: Option<String>,
    /// If true, continue the most recent persisted session (ignored if resume_session_id is set).
    #[serde(default)]
    pub continue_last: bool,
    /// Allowed tools for this session (passed as --allowedTools to claude CLI).
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// If true, enable MCP permission gate for modifying tools.
    #[serde(default)]
    pub permission_gate: bool,
}

/// POST /api/chat/sessions — create a new chat session (optionally resuming).
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    // Resolve resume ID: explicit > continue_last > none
    let resume_id = if body.resume_session_id.is_some() {
        body.resume_session_id.clone()
    } else if body.continue_last {
        let mgr = Arc::clone(&state.chat_manager);
        tokio::task::spawn_blocking(move || mgr.latest_persisted_claude_session_id())
            .await
            .unwrap_or(None)
    } else {
        None
    };

    match state
        .chat_manager
        .create(
            resume_id.as_deref(),
            body.cwd.as_deref(),
            body.allowed_tools.as_deref(),
            body.permission_gate,
        )
        .await
    {
        Ok(session) => {
            let claude_sid = session.claude_session_id().await;
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "id": session.id,
                    "claude_session_id": claude_sid,
                })),
            )
                .into_response()
        }
        Err(e) => chat_error_response(e),
    }
}

/// GET /api/chat/sessions — list all active chat sessions.
pub async fn list_sessions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let sessions = state.chat_manager.list().await;
    Json(sessions).into_response()
}

/// POST /api/chat/sessions/{id}/stop — interrupt a running chat session.
pub async fn stop_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.chat_manager.get(&id).await {
        Ok(session) => {
            session.interrupt().await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => chat_error_response(e),
    }
}

#[derive(Deserialize)]
pub struct RenameSessionRequest {
    pub name: Option<String>,
}

/// Maximum length for a session name.
const MAX_SESSION_NAME_LEN: usize = 200;

/// PATCH /api/chat/sessions/{id} — update session metadata (e.g. name).
pub async fn rename_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<RenameSessionRequest>,
) -> impl IntoResponse {
    let name = body
        .name
        .map(|n| n.chars().take(MAX_SESSION_NAME_LEN).collect::<String>());
    match state.chat_manager.get(&id).await {
        Ok(session) => {
            session.set_name(name).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => chat_error_response(e),
    }
}

/// DELETE /api/chat/sessions/{id} — destroy an active chat session.
pub async fn destroy_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.chat_manager.destroy(&id).await {
        Ok(()) => {
            // F013: Evict old persisted sessions after a new one is saved
            let mgr = Arc::clone(&state.chat_manager);
            let _: Option<()> = tokio::task::spawn_blocking(move || {
                mgr.evict_old_persisted();
            })
            .await
            .ok();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => chat_error_response(e),
    }
}

/// GET /api/chat/history — list persisted (past) chat sessions.
pub async fn list_history(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let sessions = tokio::task::spawn_blocking({
        let mgr = Arc::clone(&state.chat_manager);
        move || mgr.list_persisted()
    })
    .await
    .unwrap_or_default();
    Json(sessions).into_response()
}

/// GET /api/chat/history/{id} — get a persisted session's full history.
pub async fn get_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let result = tokio::task::spawn_blocking({
        let mgr = Arc::clone(&state.chat_manager);
        move || mgr.load_persisted(&id)
    })
    .await
    .unwrap_or(None);
    match result {
        Some(session) => Json(serde_json::json!(session)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "persisted session not found"})),
        )
            .into_response(),
    }
}

/// PATCH /api/chat/history/{id} — rename a persisted session.
pub async fn rename_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<RenameSessionRequest>,
) -> impl IntoResponse {
    let renamed = tokio::task::spawn_blocking({
        let mgr = Arc::clone(&state.chat_manager);
        let name = body
            .name
            .map(|n| n.chars().take(MAX_SESSION_NAME_LEN).collect::<String>());
        move || mgr.rename_persisted(&id, name)
    })
    .await
    .unwrap_or(false);
    if renamed {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "persisted session not found"})),
        )
            .into_response()
    }
}

/// DELETE /api/chat/history/{id} — delete a persisted session.
pub async fn delete_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let deleted = tokio::task::spawn_blocking({
        let mgr = Arc::clone(&state.chat_manager);
        move || mgr.delete_persisted(&id)
    })
    .await
    .unwrap_or(false);
    if deleted {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "persisted session not found"})),
        )
            .into_response()
    }
}

// ── Permission gate endpoint (called by MCP gate server) ────────

#[derive(Deserialize)]
pub struct GateRequest {
    pub request_id: String,
    pub tool: String,
    pub input: serde_json::Value,
}

/// POST /api/chat/sessions/{id}/gate/request — MCP gate server requests permission.
/// Long-polls until the user responds via WS or timeout (5 min).
pub async fn gate_request(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<GateRequest>,
) -> impl IntoResponse {
    let session = match state.chat_manager.get(&id).await {
        Ok(s) => s,
        Err(e) => return chat_error_response(e),
    };

    // Verify gate token
    let perm = match &session.permission {
        Some(p) => Arc::clone(p),
        None => {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "permission gate not enabled"})),
            )
                .into_response();
        }
    };
    let provided_token = headers
        .get("X-Gate-Token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided_token != perm.gate_token {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "invalid gate token"})),
        )
            .into_response();
    }

    // Register pending permission and broadcast to WS clients
    let rx = perm.register(body.request_id.clone()).await;

    let event = serde_json::json!({
        "type": "permission_request",
        "request_id": body.request_id,
        "tool": body.tool,
        "input": body.input,
    });
    session.broadcast_event(&event.to_string());

    // Wait for user response or timeout
    let request_id = body.request_id.clone();
    let perm_cleanup = Arc::clone(&perm);
    match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
        Ok(Ok(allowed)) => Json(serde_json::json!({ "allowed": allowed })).into_response(),
        _ => {
            // Timeout or channel dropped — auto-deny
            perm_cleanup.remove(&request_id).await;
            Json(serde_json::json!({ "allowed": false })).into_response()
        }
    }
}

// ── WebSocket endpoint ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct ChatWsQuery {
    pub session: String,
}

/// GET /api/chat/ws?session={id} — WebSocket for chat streaming.
pub async fn chat_ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<ChatWsQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let chat_manager = Arc::clone(&state.chat_manager);
    let session_id = query.session;

    ws.on_upgrade(move |socket| handle_chat_socket(socket, chat_manager, session_id))
}

async fn handle_chat_socket(socket: WebSocket, chat_manager: Arc<ChatManager>, session_id: String) {
    let session = match chat_manager.get(&session_id).await {
        Ok(s) => s,
        Err(_) => return,
    };

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Subscribe BEFORE reading history to prevent message gaps
    let mut output_rx = session.subscribe();

    // Send replay history
    let history = session.history().await;
    for event in history {
        if ws_tx.send(Message::Text(event.into())).await.is_err() {
            return;
        }
    }

    // Claude stdout → WebSocket
    let session_for_read = Arc::clone(&session);
    let claude_to_ws = async move {
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(1), output_rx.recv()).await {
                Ok(Ok(event)) => {
                    if ws_tx.send(Message::Text(event.into())).await.is_err() {
                        break;
                    }
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    let claude_sid = session_for_read.claude_session_id().await;
                    tracing::debug!(
                        target: "chat",
                        session_id = %session_id,
                        claude_session_id = ?claude_sid,
                        "Chat session ended (broadcast closed)"
                    );
                    let msg = serde_json::json!({
                        "type": "session_ended",
                        "claude_session_id": claude_sid,
                    });
                    let _ = ws_tx.send(Message::Text(msg.to_string().into())).await;
                    break;
                }
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    tracing::warn!("Chat WS client lagged {n} messages");
                    continue;
                }
                Err(_) => {
                    // Timeout — check if session is still alive
                    if !session_for_read.is_alive() {
                        let claude_sid = session_for_read.claude_session_id().await;
                        let msg = serde_json::json!({
                            "type": "session_ended",
                            "claude_session_id": claude_sid,
                        });
                        let _ = ws_tx.send(Message::Text(msg.to_string().into())).await;
                        break;
                    }
                }
            }
        }
    };

    // WebSocket → Claude stdin
    let session_for_write = Arc::clone(&session);
    let ws_to_claude = async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Text(text) => {
                    match serde_json::from_str::<ChatWsCommand>(&text) {
                        Ok(cmd) => match cmd {
                            ChatWsCommand::Message { text, files } => {
                                let msg = if files.is_empty() {
                                    text
                                } else {
                                    // F002: Run sync I/O in spawn_blocking
                                    let t = text.clone();
                                    let f = files.clone();
                                    let s = Arc::clone(&session_for_write);
                                    match tokio::task::spawn_blocking(move || {
                                        build_message_with_files(&t, &f, &s)
                                    })
                                    .await
                                    {
                                        Ok(result) => result,
                                        Err(e) => {
                                            tracing::warn!("attach spawn_blocking failed: {e}");
                                            text
                                        }
                                    }
                                };
                                if let Err(e) = session_for_write.send_message(&msg).await {
                                    tracing::warn!("Chat send_message failed: {e}");
                                    break;
                                }
                            }
                            // F003: AskResponse includes tool_use_id for protocol correctness
                            ChatWsCommand::AskResponse { text, tool_use_id } => {
                                // F008/F010: Distinguish ask_response from regular message in logs
                                tracing::debug!(
                                    target: "chat",
                                    tool_use_id = tool_use_id.as_deref().unwrap_or("none"),
                                    "ask_response received"
                                );
                                if let Err(e) = session_for_write.send_message(&text).await {
                                    tracing::warn!("Chat ask_response failed: {e}");
                                    break;
                                }
                            }
                            ChatWsCommand::PermissionResponse {
                                request_id,
                                allowed,
                            } => {
                                tracing::debug!(
                                    target: "chat",
                                    request_id = %request_id,
                                    allowed = allowed,
                                    "permission_response received"
                                );
                                if let Some(perm) = &session_for_write.permission {
                                    perm.resolve(&request_id, allowed).await;
                                }
                            }
                        },
                        // F007: Log WS command parse failures for observability
                        Err(e) => {
                            tracing::debug!(
                                target: "chat",
                                error = %e,
                                "Failed to parse chat WS command"
                            );
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };

    tokio::select! {
        _ = claude_to_ws => {},
        _ = ws_to_claude => {},
    }
}

/// WebSocket commands from the frontend.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum ChatWsCommand {
    /// Send a user message (will be wrapped in stream-json format).
    /// Optional `files` are read server-side and prepended as context.
    #[serde(rename = "message")]
    Message {
        text: String,
        #[serde(default)]
        files: Vec<String>,
    },
    /// Respond to an AskUserQuestion (sent as a follow-up user message).
    #[serde(rename = "ask_response")]
    AskResponse {
        text: String,
        /// F003: Tool use ID for protocol correctness (currently informational).
        tool_use_id: Option<String>,
    },
    /// Respond to a permission gate request (Allow/Deny from frontend).
    #[serde(rename = "permission_response")]
    PermissionResponse { request_id: String, allowed: bool },
}

/// Maximum size per attached file (100 KB).
const MAX_ATTACH_FILE_SIZE: u64 = 100 * 1024;
/// Maximum total size for all attached files (500 KB).
const MAX_ATTACH_TOTAL_SIZE: u64 = 500 * 1024;
/// Maximum number of attached files per message.
const MAX_ATTACH_FILES: usize = 20;

/// Read attached files and prepend their contents to the user message.
/// Broadcasts warnings via the session's broadcast channel for skipped files.
/// Files are sandboxed to the session's cwd (or home directory as fallback).
fn build_message_with_files(
    text: &str,
    files: &[String],
    session: &super::manager::ChatSession,
) -> String {
    use crate::filer::api::{is_binary, resolve_path};

    // F001: Sandbox — restrict file access to session cwd or home dir
    let sandbox_root = session
        .cwd
        .as_ref()
        .and_then(|c| resolve_path(c).ok())
        .or_else(|| {
            std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .ok()
                .map(std::path::PathBuf::from)
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let mut context_parts: Vec<String> = Vec::new();
    // total_size tracks only successfully attached text files (binary-skipped files excluded)
    let mut total_size: u64 = 0;

    for raw_path in files.iter().take(MAX_ATTACH_FILES) {
        let path = match resolve_path(raw_path) {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!(target: "chat", path = %raw_path, "attach: resolve failed");
                broadcast_attach_warning(session, &format!("File not found: {raw_path}"));
                continue;
            }
        };

        // F001: Verify file is within sandbox root
        if !path.starts_with(&sandbox_root) {
            tracing::warn!(
                target: "chat",
                path = %path.display(),
                sandbox = %sandbox_root.display(),
                "attach: path outside sandbox"
            );
            broadcast_attach_warning(
                session,
                &format!("Access denied (outside workspace): {raw_path}"),
            );
            continue;
        }

        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(target: "chat", path = %raw_path, error = %e, "attach: metadata failed");
                broadcast_attach_warning(session, &format!("Cannot read: {raw_path}"));
                continue;
            }
        };

        if !metadata.is_file() {
            broadcast_attach_warning(session, &format!("Not a file: {raw_path}"));
            continue;
        }

        let size = metadata.len();
        if size > MAX_ATTACH_FILE_SIZE {
            tracing::warn!(target: "chat", path = %raw_path, size, "attach: file too large");
            broadcast_attach_warning(
                session,
                &format!(
                    "File too large ({} KB, max {} KB): {raw_path}",
                    size / 1024,
                    MAX_ATTACH_FILE_SIZE / 1024
                ),
            );
            continue;
        }

        if total_size + size > MAX_ATTACH_TOTAL_SIZE {
            tracing::warn!(target: "chat", path = %raw_path, total_size, "attach: total size exceeded");
            broadcast_attach_warning(
                session,
                &format!(
                    "Total attachment size exceeded ({} KB max): {raw_path} skipped",
                    MAX_ATTACH_TOTAL_SIZE / 1024
                ),
            );
            continue;
        }

        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(target: "chat", path = %raw_path, error = %e, "attach: read failed");
                broadcast_attach_warning(session, &format!("Read error for {raw_path}: {e}"));
                continue;
            }
        };

        if is_binary(&data) {
            tracing::debug!(target: "chat", path = %raw_path, "attach: binary file skipped");
            broadcast_attach_warning(session, &format!("Binary file skipped: {raw_path}"));
            continue;
        }

        let content = String::from_utf8_lossy(&data);
        total_size += size;
        // F004: Escape path for safe embedding in XML-like tag
        let escaped_path = raw_path
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        context_parts.push(format!(
            "<file path=\"{escaped_path}\">\n{content}\n</file>"
        ));
        tracing::debug!(target: "chat", path = %raw_path, size, "attach: file added");
    }

    if files.len() > MAX_ATTACH_FILES {
        broadcast_attach_warning(
            session,
            &format!(
                "Too many files ({}, max {}): extra files skipped",
                files.len(),
                MAX_ATTACH_FILES
            ),
        );
    }

    if context_parts.is_empty() {
        return text.to_string();
    }

    format!("{}\n\n{text}", context_parts.join("\n\n"))
}

/// Broadcast an attachment warning as a synthetic event via the session's channel.
fn broadcast_attach_warning(session: &super::manager::ChatSession, msg: &str) {
    let payload = serde_json::json!({
        "type": "attach_warning",
        "message": msg,
    });
    session.broadcast_event(&payload.to_string());
}

fn chat_error_response(e: ChatError) -> axum::response::Response {
    let (status, msg) = match &e {
        ChatError::TooManySessions => (StatusCode::TOO_MANY_REQUESTS, e.to_string()),
        ChatError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
        ChatError::Dead => (StatusCode::GONE, e.to_string()),
        ChatError::ClaudeNotFound => (
            StatusCode::SERVICE_UNAVAILABLE,
            "claude CLI is not installed or not in PATH".to_string(),
        ),
        ChatError::SpawnFailed(_) | ChatError::WriteFailed(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    };
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}
