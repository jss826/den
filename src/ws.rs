use axum::{
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use portable_pty::PtySize;
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::validate_token;
use crate::pty::manager::PtyManager;

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: String,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

/// WebSocket エンドポイント
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !validate_token(&query.token, &state.config.password) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }

    let cols = query.cols.unwrap_or(80);
    let rows = query.rows.unwrap_or(24);
    let shell = state.config.shell.clone();

    ws.on_upgrade(move |socket| handle_socket(socket, shell, cols, rows))
}

async fn handle_socket(socket: WebSocket, shell: String, cols: u16, rows: u16) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // PTY セッションを起動
    let pty = match PtyManager::spawn(&shell, cols, rows) {
        Ok(pty) => pty,
        Err(e) => {
            tracing::error!("PTY spawn failed: {}", e);
            let _ = ws_tx
                .send(Message::Text(
                    format!("\r\nError: PTY spawn failed: {}\r\n", e).into(),
                ))
                .await;
            return;
        }
    };

    let mut pty_reader = pty.reader;
    let mut pty_writer = pty.writer;
    let master = pty.master;
    let _child = pty.child;

    // PTY → WS: blocking read を spawn_blocking で非同期化
    let (pty_data_tx, mut pty_data_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

    let read_task = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut pty_reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if pty_data_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // PTY → WS 転送
    let pty_to_ws = async {
        while let Some(data) = pty_data_rx.recv().await {
            if ws_tx.send(Message::Binary(data.into())).await.is_err() {
                break;
            }
        }
    };

    // resize コマンドは別チャネル経由で spawn_blocking 内で処理
    let (resize_tx, resize_rx) = std::sync::mpsc::channel::<(u16, u16)>();

    // master を blocking スレッドに移動 (MasterPty は !Sync)
    let resize_task = tokio::task::spawn_blocking(move || {
        while let Ok((cols, rows)) = resize_rx.recv() {
            let size = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            let _ = master.resize(size);
        }
    });

    // WS → PTY 転送
    let ws_to_pty = async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(data) => {
                    if std::io::Write::write_all(&mut pty_writer, &data).is_err() {
                        break;
                    }
                }
                Message::Text(text) => {
                    if let Ok(cmd) = serde_json::from_str::<serde_json::Value>(&text) {
                        match cmd["type"].as_str() {
                            Some("resize") => {
                                if let (Some(c), Some(r)) =
                                    (cmd["cols"].as_u64(), cmd["rows"].as_u64())
                                {
                                    let _ = resize_tx.send((c as u16, r as u16));
                                }
                            }
                            Some("input") => {
                                if let Some(data) = cmd["data"].as_str()
                                    && std::io::Write::write_all(&mut pty_writer, data.as_bytes())
                                        .is_err()
                                {
                                    break;
                                }
                            }
                            _ => {}
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

    read_task.abort();
    resize_task.abort();
    tracing::info!("WebSocket session ended");
}
