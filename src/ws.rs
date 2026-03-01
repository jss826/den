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
use std::borrow::Cow;
use std::sync::Arc;

use crate::AppState;
use crate::pty::registry::{ClientKind, RegistryError, SessionInfo};

/// PTY 出力受信タイムアウト（alive チェック間隔）
const OUTPUT_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Deserialize)]
pub struct WsQuery {
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
/// 認証は auth_middleware（Cookie / Authorization ヘッダー）で行われる。
/// WS upgrade リクエスト時にブラウザが自動で Cookie を送信するため、
/// first-message auth は不要。
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    let Some(session_name) = query.session else {
        return (
            StatusCode::BAD_REQUEST,
            "Missing required parameter: session",
        )
            .into_response();
    };
    let cols = query.cols.unwrap_or(80);
    let rows = query.rows.unwrap_or(24);
    let registry = Arc::clone(&state.registry);

    ws.on_upgrade(move |socket| handle_socket(socket, registry, session_name, cols, rows))
        .into_response()
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
    let name_for_input = session_name.clone();
    let ws_to_pty = async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(data) => {
                    let filtered = filter_mouse_sequences(&data);
                    if !filtered.is_empty()
                        && let Err(e) = session.write_input_from(client_id, &filtered).await
                    {
                        tracing::warn!("WS write_input failed for session {name_for_input}: {e}");
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
                                let filtered = filter_mouse_sequences(data.as_bytes());
                                if !filtered.is_empty()
                                    && let Err(e) =
                                        session.write_input_from(client_id, &filtered).await
                                {
                                    tracing::warn!(
                                        "WS write_input failed for session {name_for_input}: {e}"
                                    );
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
    let sessions = state.registry.list().await;
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
        Err(RegistryError::LimitExceeded) => {
            (StatusCode::TOO_MANY_REQUESTS, "Session limit exceeded").into_response()
        }
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

/// Strip mouse sequences from input (defense-in-depth; frontend filters first).
///
/// Handles three mouse encodings:
/// - **SGR**: `ESC [ < Btn ; X ; Y M/m`
/// - **URXVT**: `ESC [ Btn ; X ; Y M` (digits+semicolons, no `<`)
/// - **X10**: `ESC [ M Cb Cx Cy` (3 raw bytes after `M`)
///
/// ConPTY does not understand mouse reports — it consumes the CSI prefix
/// but passes the parameters through as literal text, producing garbage input
/// in applications like Zellij running over SSH.
fn filter_mouse_sequences(data: &[u8]) -> Cow<'_, [u8]> {
    // Fast path: no ESC → no mouse sequences possible
    if !data.contains(&0x1b) {
        return Cow::Borrowed(data);
    }

    // Second fast path: no CSI prefix (ESC [) → skip allocation.
    if !data.windows(2).any(|w| w == b"\x1b[") {
        return Cow::Borrowed(data);
    }

    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    let mut modified = false;

    while i < data.len() {
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'[' {
            // X10 mouse: ESC [ M Cb Cx Cy (exactly 3 raw bytes after M)
            if i + 2 < data.len() && data[i + 2] == b'M' && i + 5 < data.len() {
                // Skip ESC [ M + 3 bytes
                i += 6;
                modified = true;
                continue;
            }

            // SGR mouse: ESC [ < Btn ; X ; Y M/m
            if i + 2 < data.len() && data[i + 2] == b'<' {
                let start = i;
                i += 3; // skip ESC [ <

                // Parameter bytes: digits and semicolons
                while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
                    i += 1;
                }

                // Final byte must be M (press/move) or m (release)
                if i < data.len() && (data[i] == b'M' || data[i] == b'm') {
                    i += 1;
                    modified = true;
                    continue;
                }

                // Not a valid SGR mouse sequence → keep original bytes
                result.extend_from_slice(&data[start..i]);
                continue;
            }

            // URXVT mouse: ESC [ Btn ; X ; Y M (digits+semicolons, terminated by M only)
            // Must have at least one digit after ESC [
            if i + 2 < data.len() && data[i + 2].is_ascii_digit() {
                let start = i;
                let mut j = i + 2;
                let mut semicolons = 0;

                while j < data.len() && (data[j].is_ascii_digit() || data[j] == b';') {
                    if data[j] == b';' {
                        semicolons += 1;
                    }
                    j += 1;
                }

                // URXVT needs exactly 2 semicolons and final byte M
                if semicolons == 2 && j < data.len() && data[j] == b'M' {
                    i = j + 1;
                    modified = true;
                    continue;
                }

                // Not URXVT mouse → keep original bytes
                result.extend_from_slice(&data[start..start + 1]);
                i = start + 1;
                continue;
            }

            // Not a mouse sequence — keep ESC byte
            result.push(data[i]);
            i += 1;
        } else {
            result.push(data[i]);
            i += 1;
        }
    }

    if !modified {
        Cow::Borrowed(data)
    } else {
        Cow::Owned(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- SGR mouse tests ---

    #[test]
    fn no_esc_passthrough() {
        let data = b"hello world";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], &data[..]);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn strip_sgr_mouse_press() {
        let data = b"\x1b[<0;35;5M";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
    }

    #[test]
    fn strip_sgr_mouse_release() {
        let data = b"\x1b[<0;35;5m";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
    }

    #[test]
    fn strip_sgr_mouse_move() {
        let data = b"\x1b[<35;70;15M";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
    }

    #[test]
    fn strip_multiple_sgr_mouse_events() {
        let data = b"\x1b[<35;70;15M\x1b[<35;71;15M\x1b[<35;72;15m";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
    }

    #[test]
    fn keep_text_around_sgr_mouse() {
        let data = b"abc\x1b[<0;10;20Mdef";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], b"abcdef");
    }

    #[test]
    fn keep_non_mouse_csi() {
        // ESC [ 1 ; 2 H — cursor position (not mouse)
        let data = b"\x1b[1;2H";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], &data[..]);
    }

    #[test]
    fn keep_incomplete_sgr_mouse() {
        let data = b"\x1b[<0;35";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], &data[..]);
    }

    #[test]
    fn empty_input() {
        let data = b"";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn arrow_keys_no_alloc() {
        let data = b"\x1b[A\x1b[B\x1b[C\x1b[D";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], &data[..]);
    }

    #[test]
    fn minimal_sgr_mouse() {
        let data = b"\x1b[<0;0;0M";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
    }

    #[test]
    fn interleaved_text_and_multiple_sgr_mouse() {
        let data = b"hello\x1b[<0;1;2Mworld\x1b[<0;3;4m!";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], b"helloworld!");
    }

    // --- URXVT mouse tests ---

    #[test]
    fn strip_urxvt_mouse() {
        // ESC [ 35 ; 70 ; 15 M — URXVT mouse (no <)
        let data = b"\x1b[35;70;15M";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
    }

    #[test]
    fn strip_urxvt_mouse_with_text() {
        let data = b"abc\x1b[35;70;15Mdef";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], b"abcdef");
    }

    #[test]
    fn strip_multiple_urxvt_mouse() {
        let data = b"\x1b[35;70;15M\x1b[35;71;15M";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
    }

    #[test]
    fn keep_csi_with_one_semicolon() {
        // ESC [ 1 ; 2 H — not URXVT (only 1 semicolon)
        let data = b"\x1b[1;2H";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], &data[..]);
    }

    // --- X10 mouse tests ---

    #[test]
    fn strip_x10_mouse() {
        // ESC [ M Cb Cx Cy — X10 mouse (3 raw bytes)
        let data = b"\x1b[M !\"";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
    }

    #[test]
    fn strip_x10_mouse_with_text() {
        let data = b"abc\x1b[M !\"def";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], b"abcdef");
    }

    #[test]
    fn strip_multiple_x10_mouse() {
        let data = b"\x1b[M !\"\x1b[M #$";
        let result = filter_mouse_sequences(data);
        assert!(result.is_empty());
    }

    // --- Mixed format tests ---

    #[test]
    fn strip_mixed_sgr_and_urxvt() {
        let data = b"a\x1b[<0;1;2Mb\x1b[35;70;15Mc";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], b"abc");
    }

    #[test]
    fn strip_mixed_all_formats() {
        let data = b"a\x1b[<0;1;2Mb\x1b[35;70;15Mc\x1b[M !\"d";
        let result = filter_mouse_sequences(data);
        assert_eq!(&result[..], b"abcd");
    }
}
