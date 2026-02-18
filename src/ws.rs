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

use crate::AppState;
use crate::auth::validate_token;
use crate::pty::registry::{ClientKind, SessionInfo};

/// PTY 出力受信タイムアウト（alive チェック間隔）
const OUTPUT_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: String,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub session: Option<String>,
}

/// WebSocket コマンド（型付きデシリアライズ）
#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsCommand {
    #[serde(rename = "resize")]
    Resize { cols: u16, rows: u16 },
    #[serde(rename = "input")]
    Input { data: String },
}

/// WebSocket エンドポイント
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !validate_token(&query.token, &state.config.password, &state.hmac_secret) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }

    let cols = query.cols.unwrap_or(80);
    let rows = query.rows.unwrap_or(24);
    let session_name = query.session.unwrap_or_else(|| "default".to_string());
    let registry = Arc::clone(&state.registry);

    ws.on_upgrade(move |socket| handle_socket(socket, registry, session_name, cols, rows))
}

async fn handle_socket(
    socket: WebSocket,
    registry: Arc<crate::pty::registry::SessionRegistry>,
    session_name: String,
    cols: u16,
    rows: u16,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // SessionRegistry に attach（なければ create）
    let (session, mut output_rx, replay, client_id) = match registry
        .get_or_create(&session_name, ClientKind::WebSocket, cols, rows)
        .await
    {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Session attach failed: {e}");
            let _ = ws_tx
                .send(Message::Text(format!("\r\nError: {e}\r\n").into()))
                .await;
            return;
        }
    };

    // replay data を送信
    if !replay.is_empty() && ws_tx.send(Message::Binary(replay.into())).await.is_err() {
        registry.detach(&session_name, client_id).await;
        return;
    }

    // broadcast → WS 転送
    let session_for_output = Arc::clone(&session);
    let name_for_output = session_name.clone();
    let pty_to_ws = async {
        loop {
            // recv with timeout: ConPTY は子プロセス終了後も broadcast チャネルが
            // 閉じないため、定期的に alive を確認する
            match tokio::time::timeout(OUTPUT_RECV_TIMEOUT, output_rx.recv()).await {
                Ok(Ok(data)) => {
                    if ws_tx.send(Message::Binary(data.into())).await.is_err() {
                        break;
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                    tracing::warn!("WS client lagged {n} messages on session {name_for_output}");
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    // セッション終了
                    let msg = r#"{"type":"session_ended"}"#.to_string();
                    let _ = ws_tx.send(Message::Text(msg.into())).await;
                    break;
                }
                Err(_) => {
                    // タイムアウト: セッション生存チェック
                    if !session_for_output.is_alive() {
                        let msg = r#"{"type":"session_ended"}"#.to_string();
                        let _ = ws_tx.send(Message::Text(msg.into())).await;
                        break;
                    }
                }
            }
        }
        drop(session_for_output); // keep session alive during output
    };

    // WS → PTY 転送
    let ws_to_pty = async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(data) => {
                    if session.write_input_from(client_id, &data).await.is_err() {
                        break;
                    }
                }
                Message::Text(text) => {
                    if let Ok(cmd) = serde_json::from_str::<WsCommand>(&text) {
                        match cmd {
                            WsCommand::Resize { cols, rows } => {
                                session.resize(client_id, cols, rows).await;
                            }
                            WsCommand::Input { data } => {
                                if session
                                    .write_input_from(client_id, data.as_bytes())
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };

    tokio::select! {
        _ = pty_to_ws => {},
        _ = ws_to_pty => {},
    }

    // detach（セッションは維持）
    registry.detach(&session_name, client_id).await;

    tracing::info!("WebSocket client detached from session {session_name}");
}

// --- REST API for terminal session management ---

/// GET /api/terminal/sessions
pub async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<SessionInfo>> {
    let sessions = state
        .registry
        .list()
        .await
        .into_iter()
        .filter(|s| !s.name.starts_with("claude-"))
        .collect();
    Json(sessions)
}

/// POST /api/terminal/sessions { "name": "..." }
#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub name: String,
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    match state.registry.create(&req.name, 80, 24).await {
        Ok(_session) => StatusCode::CREATED.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// DELETE /api/terminal/sessions/{name}
pub async fn destroy_session(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> StatusCode {
    state.registry.destroy(&name).await;
    StatusCode::NO_CONTENT
}
