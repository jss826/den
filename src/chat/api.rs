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
}

/// POST /api/chat/sessions — create a new chat session (optionally resuming).
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    match state
        .chat_manager
        .create(body.resume_session_id.as_deref())
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
                    let _ = ws_tx
                        .send(Message::Text(
                            r#"{"type":"session_ended"}"#.to_string().into(),
                        ))
                        .await;
                    break;
                }
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    tracing::warn!("Chat WS client lagged {n} messages");
                    continue;
                }
                Err(_) => {
                    // Timeout — check if session is still alive
                    if !session_for_read.is_alive() {
                        let _ = ws_tx
                            .send(Message::Text(
                                r#"{"type":"session_ended"}"#.to_string().into(),
                            ))
                            .await;
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
                            ChatWsCommand::Message { text } => {
                                if let Err(e) = session_for_write.send_message(&text).await {
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
    #[serde(rename = "message")]
    Message { text: String },
    /// Respond to an AskUserQuestion (sent as a follow-up user message).
    #[serde(rename = "ask_response")]
    AskResponse {
        text: String,
        /// F003: Tool use ID for protocol correctness (currently informational).
        tool_use_id: Option<String>,
    },
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
