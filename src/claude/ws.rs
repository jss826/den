use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::AppState;
use crate::auth::generate_token;

use super::connection::{self, ConnectionTarget};
use super::session::{self, SessionMap};
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
    let expected = generate_token(&state.config.password);
    if query.token != expected {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }

    let sessions = session::new_session_map();
    ws.on_upgrade(move |socket| handle_claude_ws(socket, sessions))
}

async fn handle_claude_ws(socket: WebSocket, sessions: SessionMap) {
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
        let sessions = Arc::clone(&sessions);

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

                let pty_reader = pty.reader;
                let pty_writer = pty.writer;
                let _child = pty.child;
                let _master = pty.master;

                // writer をセッションマップに格納
                let writer = Arc::new(Mutex::new(pty_writer));
                let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();

                {
                    let mut map = sessions.lock().await;
                    map.insert(
                        session_id.clone(),
                        session::ClaudeSessionHandle {
                            connection: conn,
                            working_dir: dir,
                            writer: Arc::clone(&writer),
                            stop_tx,
                        },
                    );
                }

                // PTY 読み取りタスクを起動
                tokio::spawn(stream_pty_output(
                    session_id.clone(),
                    pty_reader,
                    Arc::clone(&ws_tx),
                    Arc::clone(&sessions),
                    stop_rx,
                ));
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

                let map = sessions.lock().await;
                if let Some(handle) = map.get(&session_id) {
                    let mut writer = handle.writer.lock().await;
                    if let Err(e) = session::send_to_session(&mut **writer, &prompt) {
                        drop(writer);
                        drop(map);
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
                let mut map = sessions.lock().await;
                if let Some(handle) = map.remove(&session_id) {
                    let _ = handle.stop_tx.send(());
                    let resp = json!({ "type": "session_stopped", "session_id": session_id });
                    let _ = ws_tx
                        .lock()
                        .await
                        .send(Message::Text(resp.to_string().into()))
                        .await;
                }
            }

            _ => {}
        }
    }

    // WebSocket 切断時に全セッション停止
    let mut map = sessions.lock().await;
    for (_, handle) in map.drain() {
        let _ = handle.stop_tx.send(());
    }
}

/// PTY の出力を WebSocket に中継
async fn stream_pty_output(
    session_id: String,
    mut reader: Box<dyn std::io::Read + Send>,
    ws_tx: Arc<Mutex<futures::stream::SplitSink<WebSocket, Message>>>,
    sessions: SessionMap,
    mut stop_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let (data_tx, mut data_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

    // Blocking read task
    let read_task = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if data_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // 出力を WebSocket に転送
    let forward = async {
        // PTY 出力をバッファに蓄積し、NDJSON の行単位で送信
        let mut line_buf = String::new();

        loop {
            tokio::select! {
                data = data_rx.recv() => {
                    match data {
                        Some(bytes) => {
                            let text = String::from_utf8_lossy(&bytes);
                            line_buf.push_str(&text);

                            // 改行ごとに JSON イベントとして送信
                            while let Some(pos) = line_buf.find('\n') {
                                let line = line_buf[..pos].trim().to_string();
                                line_buf = line_buf[pos + 1..].to_string();

                                if line.is_empty() {
                                    continue;
                                }

                                let event = json!({
                                    "type": "claude_event",
                                    "session_id": &session_id,
                                    "event": Value::String(line),
                                });
                                let _ = ws_tx.lock().await
                                    .send(Message::Text(event.to_string().into()))
                                    .await;
                            }
                        }
                        None => break,
                    }
                }
                _ = &mut stop_rx => break,
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
    };

    forward.await;
    read_task.abort();

    // セッション完了通知
    let resp = json!({ "type": "session_completed", "session_id": &session_id });
    let _ = ws_tx
        .lock()
        .await
        .send(Message::Text(resp.to_string().into()))
        .await;

    // セッションマップから削除
    sessions.lock().await.remove(&session_id);
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
