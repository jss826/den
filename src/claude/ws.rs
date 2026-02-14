use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::AppState;
use crate::auth::validate_token;
use crate::pty::registry::SessionRegistry;
use crate::store::{SessionMeta, Store};

use super::connection::{self, ConnectionTarget};
use super::session;
use super::ssh_config;

#[derive(Deserialize)]
pub struct ClaudeWsQuery {
    pub token: String,
}

/// Claude 用 WebSocket エンドポイント
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<ClaudeWsQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !validate_token(&query.token, &state.config.password) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }

    let store = state.store.clone();
    let registry = Arc::clone(&state.registry);
    ws.on_upgrade(move |socket| handle_claude_ws(socket, store, registry))
}

async fn handle_claude_ws(socket: WebSocket, store: Store, registry: Arc<SessionRegistry>) {
    let (ws_tx, mut ws_rx) = socket.split();
    let ws_tx = Arc::new(Mutex::new(ws_tx));

    while let Some(Ok(msg)) = ws_rx.next().await {
        let Message::Text(text) = msg else {
            continue;
        };

        let cmd: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let ws_tx = Arc::clone(&ws_tx);

        match cmd["type"].as_str() {
            Some("get_ssh_hosts") => {
                let hosts = ssh_config::list_ssh_hosts();
                let resp = json!({ "type": "ssh_hosts", "hosts": hosts });
                let _ = ws_tx
                    .lock()
                    .await
                    .send(Message::Text(resp.to_string().into()))
                    .await;
            }

            Some("list_dirs") => {
                let conn: ConnectionTarget = match serde_json::from_value(cmd["connection"].clone())
                {
                    Ok(c) => c,
                    Err(_) => {
                        send_error(&ws_tx, "Invalid connection target").await;
                        continue;
                    }
                };
                let path = cmd["path"].as_str().unwrap_or("~");

                // ディレクトリ一覧をブロッキングで取得
                let result = tokio::task::spawn_blocking({
                    let conn = conn.clone();
                    let path = path.to_string();
                    move || connection::list_dirs(&conn, &path)
                })
                .await;

                match result {
                    Ok(Ok(listing)) => {
                        let resp = json!({ "type": "dir_list", "listing": listing });
                        let _ = ws_tx
                            .lock()
                            .await
                            .send(Message::Text(resp.to_string().into()))
                            .await;
                    }
                    Ok(Err(e)) => send_error(&ws_tx, &e).await,
                    Err(e) => send_error(&ws_tx, &e.to_string()).await,
                }
            }

            Some("start_session") => {
                let conn: ConnectionTarget = match serde_json::from_value(cmd["connection"].clone())
                {
                    Ok(c) => c,
                    Err(_) => {
                        send_error(&ws_tx, "Invalid connection target").await;
                        continue;
                    }
                };
                let dir = cmd["dir"].as_str().unwrap_or("~").to_string();
                let prompt = cmd["prompt"].as_str().unwrap_or("").to_string();
                let session_id = uuid_v4();

                if prompt.is_empty() {
                    send_error(&ws_tx, "Prompt is required").await;
                    continue;
                }

                // Store にセッションメタを永続化
                let meta = SessionMeta {
                    id: session_id.clone(),
                    prompt: prompt.clone(),
                    connection: serde_json::to_value(&conn).unwrap_or_default(),
                    working_dir: dir.clone(),
                    status: "running".to_string(),
                    created_at: Utc::now(),
                    finished_at: None,
                    total_cost: None,
                    duration_ms: None,
                };
                if let Err(e) = store.create_session(&meta) {
                    tracing::error!("Failed to persist session meta: {}", e);
                }

                // セッション開始通知
                let resp = json!({
                    "type": "session_created",
                    "session_id": &session_id,
                    "connection": &conn,
                    "dir": &dir,
                });
                let _ = ws_tx
                    .lock()
                    .await
                    .send(Message::Text(resp.to_string().into()))
                    .await;

                // PTY で claude CLI を起動
                let pty_result = tokio::task::spawn_blocking({
                    let conn = conn.clone();
                    let dir = dir.clone();
                    let prompt = prompt.clone();
                    move || session::spawn_claude_session(&conn, &dir, &prompt, 200, 50)
                })
                .await;

                let pty = match pty_result {
                    Ok(Ok(pty)) => pty,
                    Ok(Err(e)) => {
                        send_error(&ws_tx, &format!("Failed to spawn claude: {}", e)).await;
                        continue;
                    }
                    Err(e) => {
                        send_error(&ws_tx, &format!("Spawn task failed: {}", e)).await;
                        continue;
                    }
                };

                // SessionRegistry に登録
                let registry_name = format!("claude-{}", session_id);
                let shared_session = match registry.create_with_pty(&registry_name, pty).await {
                    Ok(s) => s,
                    Err(e) => {
                        send_error(&ws_tx, &format!("Registry error: {}", e)).await;
                        continue;
                    }
                };

                // processor task を spawn（WS ライフサイクルから独立）
                let processor_store = store.clone();
                let processor_session_id = session_id.clone();
                let processor_meta = meta;
                let processor_registry_name = registry_name.clone();
                let processor_registry = Arc::clone(&registry);

                tokio::spawn(async move {
                    run_claude_processor(
                        processor_session_id,
                        processor_registry_name,
                        processor_store,
                        processor_meta,
                        processor_registry,
                    )
                    .await;
                });

                // WS にリアルタイム出力を転送する output task を spawn
                let output_rx = shared_session.subscribe();
                let ws_tx_for_output = Arc::clone(&ws_tx);
                let sid_for_output = session_id.clone();

                tokio::spawn(async move {
                    forward_claude_output(sid_for_output, output_rx, ws_tx_for_output).await;
                });
            }

            Some("send_prompt") => {
                let session_id = match cmd["session_id"].as_str() {
                    Some(id) => id.to_string(),
                    None => {
                        send_error(&ws_tx, "session_id is required").await;
                        continue;
                    }
                };
                let prompt = cmd["prompt"].as_str().unwrap_or("").to_string();
                let registry_name = format!("claude-{}", session_id);

                if let Some(session) = registry.get(&registry_name).await {
                    let input = format!("{}\n", prompt);
                    if let Err(e) = session.write_input(input.as_bytes()).await {
                        send_error(&ws_tx, &format!("Write failed: {}", e)).await;
                    }
                } else {
                    send_error(&ws_tx, "Session not found").await;
                }
            }

            Some("stop_session") => {
                let session_id = match cmd["session_id"].as_str() {
                    Some(id) => id.to_string(),
                    None => continue,
                };
                let registry_name = format!("claude-{}", session_id);

                registry.destroy(&registry_name).await;

                let resp = json!({ "type": "session_stopped", "session_id": session_id });
                let _ = ws_tx
                    .lock()
                    .await
                    .send(Message::Text(resp.to_string().into()))
                    .await;
            }

            _ => {}
        }
    }

    // WS 切断 — processor は続行（WS ライフサイクルから独立）
    tracing::info!("Claude WebSocket disconnected");
}

/// Claude プロセッサー: broadcast から出力を読み、JSON パースして Store に永続化
/// WS から独立したタスクとして実行
async fn run_claude_processor(
    session_id: String,
    registry_name: String,
    store: Store,
    mut meta: SessionMeta,
    registry: Arc<SessionRegistry>,
) {
    // broadcast receiver を取得
    let mut output_rx = {
        let Some(session) = registry.get(&registry_name).await else {
            return;
        };
        session.subscribe()
    };

    let mut line_buf = String::new();

    loop {
        match output_rx.recv().await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                line_buf.push_str(&text);

                // 改行ごとに JSON イベントとして処理
                while let Some(pos) = line_buf.find('\n') {
                    let line = line_buf[..pos].trim().to_string();
                    line_buf.drain(..=pos);

                    if line.is_empty() {
                        continue;
                    }

                    // Store にイベント追記
                    if let Err(e) = store.append_event(&session_id, &line) {
                        tracing::warn!("Failed to append event: {}", e);
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("Claude processor lagged {n} messages for session {session_id}");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                break;
            }
        }
    }

    // 残りのバッファを処理
    let remaining = line_buf.trim().to_string();
    if !remaining.is_empty()
        && let Err(e) = store.append_event(&session_id, &remaining)
    {
        tracing::warn!("Failed to append final event: {}", e);
    }

    // セッション完了 → メタデータ更新
    meta.status = "completed".to_string();
    meta.finished_at = Some(Utc::now());
    if let Ok(duration) = meta
        .finished_at
        .unwrap()
        .signed_duration_since(meta.created_at)
        .num_milliseconds()
        .try_into()
    {
        meta.duration_ms = Some(duration);
    }
    if let Err(e) = store.update_session_meta(&meta) {
        tracing::error!("Failed to update session meta: {}", e);
    }

    // registry から削除
    registry.remove_dead(&registry_name).await;

    tracing::info!("Claude processor completed for session {session_id}");
}

/// broadcast → WS 転送（セッション出力をリアルタイムで WS に送信）
async fn forward_claude_output(
    session_id: String,
    mut output_rx: tokio::sync::broadcast::Receiver<Vec<u8>>,
    ws_tx: Arc<Mutex<futures::stream::SplitSink<WebSocket, Message>>>,
) {
    let mut line_buf = String::new();

    loop {
        match output_rx.recv().await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                line_buf.push_str(&text);

                while let Some(pos) = line_buf.find('\n') {
                    let line = line_buf[..pos].trim().to_string();
                    line_buf.drain(..=pos);

                    if line.is_empty() {
                        continue;
                    }

                    let event = json!({
                        "type": "claude_event",
                        "session_id": &session_id,
                        "event": Value::String(line),
                    });

                    if ws_tx
                        .lock()
                        .await
                        .send(Message::Text(event.to_string().into()))
                        .await
                        .is_err()
                    {
                        return; // WS closed
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                // continue
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                // セッション完了通知
                let resp = json!({ "type": "session_completed", "session_id": &session_id });
                let _ = ws_tx
                    .lock()
                    .await
                    .send(Message::Text(resp.to_string().into()))
                    .await;
                break;
            }
        }
    }

    // 残りのバッファを送信
    let remaining = line_buf.trim().to_string();
    if !remaining.is_empty() {
        let event = json!({
            "type": "claude_event",
            "session_id": &session_id,
            "event": Value::String(remaining),
        });
        let _ = ws_tx
            .lock()
            .await
            .send(Message::Text(event.to_string().into()))
            .await;
    }
}

async fn send_error(
    ws_tx: &Arc<Mutex<futures::stream::SplitSink<WebSocket, Message>>>,
    message: &str,
) {
    let resp = json!({ "type": "error", "message": message });
    let _ = ws_tx
        .lock()
        .await
        .send(Message::Text(resp.to_string().into()))
        .await;
}

fn uuid_v4() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.r#gen();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]) & 0x0FFF,
        (u16::from_be_bytes([bytes[8], bytes[9]]) & 0x3FFF) | 0x8000,
        u64::from_be_bytes([
            0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        ])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v4_format() {
        let id = uuid_v4();
        // 8-4-4-4-12 format
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
    }

    #[test]
    fn uuid_v4_version_nibble() {
        let id = uuid_v4();
        let parts: Vec<&str> = id.split('-').collect();
        // Third group starts with '4' (version 4)
        assert!(parts[2].starts_with('4'));
    }

    #[test]
    fn uuid_v4_variant_bits() {
        let id = uuid_v4();
        let parts: Vec<&str> = id.split('-').collect();
        // Fourth group first char should be 8, 9, a, or b
        let first_char = parts[3].chars().next().unwrap();
        assert!(
            "89ab".contains(first_char),
            "variant nibble '{}' not in 89ab",
            first_char
        );
    }

    #[test]
    fn uuid_v4_hex_chars() {
        let id = uuid_v4();
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }

    #[test]
    fn uuid_v4_uniqueness() {
        let a = uuid_v4();
        let b = uuid_v4();
        assert_ne!(a, b);
    }
}
