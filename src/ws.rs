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
use crate::pty::registry::{ClientKind, RegistryError, SessionInfo, SshSessionConfig};
use crate::store::SshAuthType;
use crate::terminal_filter::{filter_conpty_private_modes, filter_terminal_responses};

/// PTY 出力受信タイムアウト（alive チェック間隔）
const OUTPUT_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Snapshot control frame: the next binary frame is a full, self-contained
/// redraw (byte-ring history followed by a clean VT screen snapshot). The
/// client resets its terminal before applying it — so there is no overlap with
/// prior scrollback (no duplication) and the current viewport is authoritative.
const SNAPSHOT_MSG: &str = r#"{"type":"snapshot"}"#;

/// Build the snapshot binary frame: `[8-byte be seq][history ++ snapshot]`.
/// The combined buffer is run through `filter_conpty_private_modes`; the VT
/// snapshot never contains the blocked `?9001`/`?1004` modes, so filtering is a
/// no-op on its bytes and only scrubs the raw history portion.
fn build_snapshot_binary(end_seq: u64, history: &[u8], snapshot: &[u8]) -> Vec<u8> {
    let mut combined = Vec::with_capacity(history.len() + snapshot.len());
    combined.extend_from_slice(history);
    combined.extend_from_slice(snapshot);
    let filtered = filter_conpty_private_modes(&combined);
    seq_frame(end_seq, &filtered)
}

/// Prepend the 8-byte big-endian absolute sequence to a terminal data frame.
/// The client strips this prefix and records the seq so it can request a delta
/// replay (`?since=N`) on reconnect, avoiding scrollback duplication.
fn seq_frame(seq_end: u64, data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(8 + data.len());
    frame.extend_from_slice(&seq_end.to_be_bytes());
    frame.extend_from_slice(data);
    frame
}

#[derive(Deserialize)]
pub struct WsQuery {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub session: Option<String>,
    /// Last absolute sequence the client already has (for delta replay on reconnect).
    pub since: Option<u64>,
}

/// WebSocket コマンド（型付きデシリアライズ）
#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsCommand {
    #[serde(rename = "resize")]
    Resize { cols: u16, rows: u16 },
    #[serde(rename = "input")]
    Input { data: String },
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "nudge")]
    Nudge,
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
    let Some(session_name) = query.session.filter(|s| !s.is_empty()) else {
        tracing::warn!("WebSocket rejected: missing or empty session parameter");
        return (
            StatusCode::BAD_REQUEST,
            "Missing required parameter: session",
        )
            .into_response();
    };
    let cols = query.cols.unwrap_or(80);
    let rows = query.rows.unwrap_or(24);
    let since = query.since;
    let registry = Arc::clone(&state.registry);

    ws.on_upgrade(move |socket| handle_socket(socket, registry, session_name, cols, rows, since))
        .into_response()
}

async fn handle_socket(
    socket: WebSocket,
    registry: Arc<crate::pty::registry::SessionRegistry>,
    session_name: String,
    cols: u16,
    rows: u16,
    since: Option<u64>,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // SessionRegistry に attach（なければ create）。`since` で差分リプレイを要求。
    let (session, mut output_rx, replay, client_id) = match registry
        .get_or_create(&session_name, ClientKind::WebSocket, cols, rows, since)
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

    // 初期リプレイ。full かつ snapshot 付き → snapshot プロトコル（reset → 履歴 → snapshot）。
    // それ以外（差分）は従来どおり seq 前置バイナリを追記。
    let mut client_seq = replay.end_seq;
    if replay.full {
        if let Some(ref snapshot) = replay.snapshot {
            if ws_tx
                .send(Message::Text(SNAPSHOT_MSG.into()))
                .await
                .is_err()
            {
                registry.detach(&session_name, client_id).await;
                return;
            }
            let frame = build_snapshot_binary(replay.end_seq, &replay.data, snapshot);
            if ws_tx.send(Message::Binary(frame.into())).await.is_err() {
                registry.detach(&session_name, client_id).await;
                return;
            }
        }
    } else if !replay.data.is_empty() {
        let filtered = filter_conpty_private_modes(&replay.data);
        if ws_tx
            .send(Message::Binary(seq_frame(replay.end_seq, &filtered).into()))
            .await
            .is_err()
        {
            registry.detach(&session_name, client_id).await;
            return;
        }
    }

    // ── 出力転送 ──
    // broadcast は「新しい出力が来た」起床信号としてのみ使い、実データは常に
    // セッションのリングバッファから `replay_since(client_seq)` で取り出す。
    // これにより lag で broadcast を取りこぼしても、リングバッファが保持している限り
    // 穴/重複なく差分を送れる（窓を外れた場合のみ full + reset でデグレード）。
    let session_for_output = Arc::clone(&session);
    let name_for_output = session_name.clone();
    let pty_to_ws = async {
        loop {
            // recv with timeout: ConPTY は子プロセス終了後も broadcast チャネルが
            // 閉じないため、定期的に alive を確認する
            let ended = match tokio::time::timeout(OUTPUT_RECV_TIMEOUT, output_rx.recv()).await {
                Ok(Ok(_)) => false, // woke: 内容は無視（リングバッファが真実）
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                    tracing::warn!("WS client lagged {n} messages on session {name_for_output}");
                    false // 取りこぼしは下の replay_since で復旧する
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => true, // セッション終了
                Err(_) => {
                    // タイムアウト: 生存確認のみ（出力なし → 差分も無い）
                    if !session_for_output.is_alive() {
                        true
                    } else {
                        continue;
                    }
                }
            };

            // 溜まった追加の起床信号を捨てる（次の replay_since で一括取得するため）。
            // Empty / Closed で止まる。Ok と Lagged は読み捨てて継続。
            while matches!(
                output_rx.try_recv(),
                Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
            ) {}

            // client_seq 以降の差分をリングバッファから取得して送る。
            // client_seq は「実際に送出できた」ブランチでのみ進める。full かつ
            // snapshot 無し（Task 2 不変条件違反・本来到達不能）は何も送らず client_seq を
            // 据え置き、次回起床で replay_since を再試行する（無音スキップ＝サイレント
            // データ欠落を避ける）。
            let slice = session_for_output.replay_since(Some(client_seq));
            if slice.end_seq != client_seq {
                if slice.full {
                    if let Some(ref snapshot) = slice.snapshot {
                        if ws_tx
                            .send(Message::Text(SNAPSHOT_MSG.into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                        let frame = build_snapshot_binary(slice.end_seq, &slice.data, snapshot);
                        if ws_tx.send(Message::Binary(frame.into())).await.is_err() {
                            break;
                        }
                        client_seq = slice.end_seq;
                    } else {
                        // Invariant violation (full ⟹ Some). Should be unreachable.
                        // Do NOT advance client_seq — retry on the next wake rather
                        // than silently dropping this output window.
                        tracing::warn!(
                            "full replay slice without snapshot on session {name_for_output} (end_seq={}); retrying",
                            slice.end_seq
                        );
                    }
                } else {
                    let filtered = filter_conpty_private_modes(&slice.data);
                    if ws_tx
                        .send(Message::Binary(seq_frame(slice.end_seq, &filtered).into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    client_seq = slice.end_seq;
                }
            }

            if ended {
                let _ = ws_tx
                    .send(Message::Text(r#"{"type":"session_ended"}"#.into()))
                    .await;
                break;
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
                    let filtered = filter_terminal_responses(&filtered);
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
                                let filtered = filter_terminal_responses(&filtered);
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
                            WsCommand::Nudge => {
                                session.nudge_resize(client_id).await;
                            }
                            WsCommand::Ping => {
                                // Keepalive — no response needed; the message
                                // itself prevents idle-timeout disconnection.
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

/// POST /api/terminal/sessions { "name": "...", "ssh": { ... }, "backend": "zellij" }
#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub name: String,
    pub ssh: Option<CreateSessionSsh>,
    #[serde(default)]
    pub backend: Option<crate::pty::backend::SessionBackend>,
}

#[derive(Deserialize)]
pub struct CreateSessionSsh {
    pub host: String,
    pub port: Option<u16>,
    pub username: String,
    pub auth_type: SshAuthType,
    pub key_path: Option<String>,
    pub initial_dir: Option<String>,
}

/// Shell metacharacters that must not appear in SSH command arguments.
fn contains_shell_meta(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            ';' | '&'
                | '|'
                | '$'
                | '`'
                | '('
                | ')'
                | '{'
                | '}'
                | '\''
                | '"'
                | '\\'
                | '\n'
                | '\r'
                | '<'
                | '>'
                | '!'
                | '#'
                | '~'
                | '*'
                | '?'
                | '['
                | ']'
        )
    })
}

/// Validate SSH config fields for safe shell injection.
pub fn validate_ssh_fields(ssh: &SshSessionConfig) -> Result<(), &'static str> {
    if contains_shell_meta(&ssh.host) || ssh.host.is_empty() {
        return Err("invalid ssh host");
    }
    if contains_shell_meta(&ssh.username) || ssh.username.is_empty() {
        return Err("invalid ssh username");
    }
    if ssh.key_path.as_deref().is_some_and(contains_shell_meta) {
        return Err("invalid ssh key_path");
    }
    if ssh.initial_dir.as_deref().is_some_and(|d| {
        d.chars()
            .any(|c| matches!(c, ';' | '&' | '|' | '$' | '`' | '\n' | '\r'))
    }) {
        return Err("invalid ssh initial_dir");
    }
    Ok(())
}

/// Build the SSH command to inject into the PTY.
/// All fields must be pre-validated via `validate_ssh_fields`.
pub fn build_ssh_command(ssh: &SshSessionConfig) -> String {
    let mut cmd = String::from("ssh");
    if ssh.port != 22 {
        cmd.push_str(&format!(" -p {}", ssh.port));
    }
    if let Some(ref key_path) = ssh.key_path {
        cmd.push_str(&format!(" -i {}", key_path));
    }
    cmd.push_str(&format!(" {}@{}", ssh.username, ssh.host));
    cmd
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionRequest>,
) -> axum::response::Response {
    // SSH 指定時は従来の ssh 経路（無改変）
    if req.ssh.is_some() {
        return create_session_ssh(state, req).await;
    }

    // backend 経路（省略時 Shell）。1:1 同名 create-or-attach:
    // AlreadyExists は既存セッションへの合流として 200（frontend は switch のみ）。
    let backend = req.backend.unwrap_or_default();
    match state
        .registry
        .create_with_backend(&req.name, 80, 24, backend)
        .await
    {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(RegistryError::LimitExceeded) => {
            (StatusCode::TOO_MANY_REQUESTS, "Session limit exceeded").into_response()
        }
        Err(RegistryError::AlreadyExists(_)) => StatusCode::OK.into_response(),
        // 同名セッションが別 backend で存在 → 別種への誤 attach を避け 409 を返す
        Err(e @ RegistryError::BackendMismatch(_)) => {
            (StatusCode::CONFLICT, e.to_string()).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// SSH セッション作成（従来ロジック、ssh パス無改変）。
async fn create_session_ssh(
    state: Arc<AppState>,
    req: CreateSessionRequest,
) -> axum::response::Response {
    let ssh_config = req.ssh.map(|s| SshSessionConfig {
        host: s.host,
        port: s.port.unwrap_or(22),
        username: s.username,
        auth_type: s.auth_type,
        key_path: s.key_path,
        initial_dir: s.initial_dir,
    });

    // Validate SSH fields before creating session
    if let Some(ref ssh) = ssh_config
        && let Err(msg) = validate_ssh_fields(ssh)
    {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    let result = state
        .registry
        .create_with_ssh(&req.name, 80, 24, ssh_config.clone())
        .await;

    match result {
        Ok((session, _rx)) => {
            if let Some(ref ssh) = ssh_config {
                let ssh_cmd = build_ssh_command(ssh);
                let inject = format!("{}\r", ssh_cmd);
                if let Err(e) = session.write_input(inject.as_bytes()).await {
                    tracing::warn!("Failed to inject SSH command: {e}");
                }

                // For key/agent auth: inject cd after delay.
                // For password auth: skip cd injection (user must type password first;
                // a blind delay would inject cd into the password prompt).
                if ssh.auth_type != SshAuthType::Password
                    && let Some(ref dir) = ssh.initial_dir
                {
                    let dir = dir.clone();
                    let session = Arc::clone(&session);
                    tokio::spawn(async move {
                        // For key/agent auth: inject cd after delay.
                        // For password auth: skip (user must type password first).
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        let cd_cmd = format!("cd '{}'\r", dir);
                        if let Err(e) = session.write_input(cd_cmd.as_bytes()).await {
                            tracing::warn!("Failed to inject cd command: {e}");
                        }
                    });
                }
            }
            StatusCode::CREATED.into_response()
        }
        Err(RegistryError::LimitExceeded) => {
            (StatusCode::TOO_MANY_REQUESTS, "Session limit exceeded").into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// PUT /api/terminal/sessions/{name}
#[derive(Deserialize)]
pub struct RenameSessionRequest {
    pub name: String,
}

pub async fn rename_session(
    State(state): State<Arc<AppState>>,
    Path(old_name): Path<String>,
    Json(req): Json<RenameSessionRequest>,
) -> impl IntoResponse {
    match state.registry.rename(&old_name, &req.name).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// PUT /api/terminal/sessions/order
pub async fn reorder_sessions(
    State(state): State<Arc<AppState>>,
    Json(order): Json<Vec<String>>,
) -> impl IntoResponse {
    if let Err(e) = state.store.save_session_order(&order) {
        tracing::warn!("Failed to save session order: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
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

    // --- Snapshot protocol unit tests ---

    #[test]
    fn snapshot_binary_frame_concatenates_history_then_snapshot() {
        let history = b"HIST";
        let snapshot = b"SNAP";
        let frame = build_snapshot_binary(42, history, snapshot);
        // 8-byte big-endian seq prefix.
        assert_eq!(&frame[..8], &42u64.to_be_bytes());
        // history then snapshot, in order.
        assert_eq!(&frame[8..], b"HISTSNAP");
    }

    #[test]
    fn snapshot_control_frame_is_typed_json() {
        assert_eq!(SNAPSHOT_MSG, r#"{"type":"snapshot"}"#);
    }

    // --- CreateSessionRequest backend parsing ---

    #[test]
    fn create_session_request_parses_backend() {
        let json = r#"{"name":"work","backend":"zellij"}"#;
        let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req.backend,
            Some(crate::pty::backend::SessionBackend::Zellij)
        );
    }

    #[test]
    fn create_session_request_backend_absent_is_none() {
        let json = r#"{"name":"work"}"#;
        let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
        assert!(req.backend.is_none());
    }

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
