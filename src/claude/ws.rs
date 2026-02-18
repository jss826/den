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
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::AppState;
use crate::auth::validate_token;
use crate::pty::registry::{SessionRegistry, SharedSession};
use crate::store::Store;

use super::connection::{self, ConnectionTarget};
use super::session;
use super::ssh_config;

/// PTY 出力受信タイムアウト（alive チェック間隔）
const OUTPUT_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Claude CLI の PTY サイズ（stream-json モードでは TUI を描画しないため大きめに設定）
const CLAUDE_PTY_COLS: u16 = 10000;
const CLAUDE_PTY_ROWS: u16 = 50;

#[derive(Deserialize)]
pub struct ClaudeWsQuery {
    pub token: String,
}

/// Claude セッションの状態（インタラクティブモード）
struct ClaudeSessionState {
    is_running: bool,
    process_alive: bool,
    registry_name: String,
    shared_session: Option<Arc<SharedSession>>,
}

type WsSink = Arc<Mutex<futures::stream::SplitSink<WebSocket, Message>>>;
type SessionStateMap = Arc<Mutex<HashMap<String, ClaudeSessionState>>>;

/// Claude 用 WebSocket エンドポイント
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<ClaudeWsQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !validate_token(&query.token, &state.config.password, &state.hmac_secret) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }

    let store = state.store.clone();
    let registry = Arc::clone(&state.registry);
    ws.on_upgrade(move |socket| handle_claude_ws(socket, store, registry))
}

async fn handle_claude_ws(socket: WebSocket, store: Store, registry: Arc<SessionRegistry>) {
    let (ws_tx, mut ws_rx) = socket.split();
    let ws_tx: WsSink = Arc::new(Mutex::new(ws_tx));
    let state_map: SessionStateMap = Arc::new(Mutex::new(HashMap::new()));

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
                let hosts = tokio::task::spawn_blocking(ssh_config::list_ssh_hosts)
                    .await
                    .unwrap_or_default();
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
                let registry_name = format!("claude-{}", session_id);

                let has_prompt = !prompt.is_empty();

                // Store にセッションメタを永続化
                let meta = crate::store::SessionMeta {
                    id: session_id.clone(),
                    prompt: prompt.clone(),
                    connection: serde_json::to_value(&conn).unwrap_or_default(),
                    working_dir: dir.clone(),
                    status: "idle".to_string(),
                    created_at: Utc::now(),
                    finished_at: None,
                    total_cost: None,
                    duration_ms: None,
                };
                if let Err(e) = store.create_session(&meta) {
                    tracing::error!("Failed to persist session meta: {}", e);
                }

                // インタラクティブモードで Claude CLI を起動
                let settings = store.load_settings();
                let agent_fwd = settings.ssh_agent_forwarding;
                let skip_perms = settings.claude_skip_permissions.unwrap_or(true);
                let pty_result = tokio::task::spawn_blocking({
                    let conn = conn.clone();
                    let dir = dir.clone();
                    move || {
                        session::spawn_claude_interactive(
                            &conn,
                            &dir,
                            agent_fwd,
                            skip_perms,
                            CLAUDE_PTY_COLS,
                            CLAUDE_PTY_ROWS,
                        )
                    }
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
                let (shared_session, pre_rx) =
                    match registry.create_with_pty(&registry_name, pty).await {
                        Ok(result) => result,
                        Err(e) => {
                            send_error(&ws_tx, &format!("Registry error: {e}")).await;
                            continue;
                        }
                    };
                let forwarder_rx = shared_session.subscribe();

                // セッション状態を作成
                {
                    let mut map = state_map.lock().await;
                    map.insert(
                        session_id.clone(),
                        ClaudeSessionState {
                            is_running: false,
                            process_alive: true,
                            registry_name: registry_name.clone(),
                            shared_session: Some(Arc::clone(&shared_session)),
                        },
                    );
                }

                // セッション開始通知
                let resp = json!({
                    "type": "session_created",
                    "session_id": &session_id,
                    "connection": &conn,
                    "dir": &dir,
                    "prompt": &prompt,
                    "status": "idle",
                });
                let _ = ws_tx
                    .lock()
                    .await
                    .send(Message::Text(resp.to_string().into()))
                    .await;

                // 永続 processor task（セッション全体で1つ）
                let processor_store = store.clone();
                let processor_session_id = session_id.clone();
                let processor_session = Arc::clone(&shared_session);
                let processor_state_map = Arc::clone(&state_map);
                let processor_registry = Arc::clone(&registry);
                let processor_registry_name = registry_name.clone();
                let processor_ws_tx = Arc::clone(&ws_tx);

                tokio::spawn(async move {
                    run_interactive_processor(
                        processor_session_id,
                        pre_rx,
                        processor_session,
                        processor_store,
                        processor_registry,
                        processor_registry_name,
                        processor_state_map,
                        processor_ws_tx,
                    )
                    .await;
                });

                // 永続 forwarder task
                let ws_tx_for_output = Arc::clone(&ws_tx);
                let sid_for_output = session_id.clone();
                let session_for_output = Arc::clone(&shared_session);
                let forwarder_state_map = Arc::clone(&state_map);

                tokio::spawn(async move {
                    forward_interactive_output(
                        sid_for_output,
                        forwarder_rx,
                        ws_tx_for_output,
                        session_for_output,
                        forwarder_state_map,
                    )
                    .await;
                });

                // プロンプトがあれば stdin に書き込み
                if has_prompt {
                    // ユーザープロンプトを events.jsonl に記録
                    let user_prompt_event =
                        json!({ "type": "user_prompt", "prompt": &prompt }).to_string();
                    if let Err(e) = store.append_event(&session_id, &user_prompt_event) {
                        tracing::warn!("Failed to append user_prompt event: {}", e);
                    }

                    // is_running フラグをセット
                    {
                        let mut map = state_map.lock().await;
                        if let Some(state) = map.get_mut(&session_id) {
                            state.is_running = true;
                        }
                    }

                    // turn_started 通知
                    let resp = json!({
                        "type": "turn_started",
                        "session_id": &session_id,
                    });
                    let _ = ws_tx
                        .lock()
                        .await
                        .send(Message::Text(resp.to_string().into()))
                        .await;

                    // Store メタを running に更新
                    if let Some(mut meta) = store.load_session_meta(&session_id) {
                        meta.status = "running".to_string();
                        let _ = store.update_session_meta(&meta);
                    }

                    // プロンプトを NDJSON 形式で stdin に書き込み
                    let input_msg = build_stream_json_input(&prompt, &session_id);
                    if let Err(e) = shared_session.write_input(input_msg.as_bytes()).await {
                        tracing::warn!("Failed to write prompt to stdin: {}", e);
                        // turn_started 済みなので turn_completed を送って UI をアンブロック
                        let mut map = state_map.lock().await;
                        if let Some(state) = map.get_mut(&session_id) {
                            state.is_running = false;
                        }
                        // Store メタを idle に戻す（F006: running のまま残る問題を修正）
                        if let Some(mut meta) = store.load_session_meta(&session_id) {
                            meta.status = "idle".to_string();
                            let _ = store.update_session_meta(&meta);
                        }
                        let resp = json!({ "type": "turn_completed", "session_id": &session_id });
                        let _ = ws_tx
                            .lock()
                            .await
                            .send(Message::Text(resp.to_string().into()))
                            .await;
                    }
                }
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

                if prompt.is_empty() {
                    send_error(&ws_tx, "Prompt is required").await;
                    continue;
                }

                // is_running チェックと shared_session 取得を同一ロック内で行う
                let shared_session = {
                    let mut map = state_map.lock().await;
                    match map.get_mut(&session_id) {
                        Some(state) => {
                            if !state.process_alive {
                                drop(map);
                                send_error(&ws_tx, "Process is no longer running").await;
                                continue;
                            }
                            if state.is_running {
                                drop(map);
                                send_error(
                                    &ws_tx,
                                    "Session is busy (processing a previous prompt)",
                                )
                                .await;
                                continue;
                            }
                            state.is_running = true;
                            state.shared_session.clone()
                        }
                        None => {
                            drop(map);
                            send_error(&ws_tx, "Session not found").await;
                            continue;
                        }
                    }
                };

                let Some(shared_session) = shared_session else {
                    send_error(&ws_tx, "Session process not available").await;
                    // Revert is_running
                    let mut map = state_map.lock().await;
                    if let Some(state) = map.get_mut(&session_id) {
                        state.is_running = false;
                    }
                    continue;
                };

                // ユーザープロンプトを events.jsonl に記録
                let user_prompt_event =
                    json!({ "type": "user_prompt", "prompt": &prompt }).to_string();
                if let Err(e) = store.append_event(&session_id, &user_prompt_event) {
                    tracing::warn!("Failed to append user_prompt event: {}", e);
                }

                // turn_started 通知
                let resp = json!({
                    "type": "turn_started",
                    "session_id": &session_id,
                });
                let _ = ws_tx
                    .lock()
                    .await
                    .send(Message::Text(resp.to_string().into()))
                    .await;

                // Store メタを running に更新
                if let Some(mut meta) = store.load_session_meta(&session_id) {
                    meta.status = "running".to_string();
                    let _ = store.update_session_meta(&meta);
                }

                // プロンプトを NDJSON 形式で stdin に書き込み
                let input_msg = build_stream_json_input(&prompt, &session_id);
                if let Err(e) = shared_session.write_input(input_msg.as_bytes()).await {
                    tracing::warn!("Failed to write prompt to stdin: {}", e);
                    send_error(&ws_tx, "Failed to send prompt to Claude process").await;
                    let mut map = state_map.lock().await;
                    if let Some(state) = map.get_mut(&session_id) {
                        state.is_running = false;
                    }
                    // Store メタを idle に戻す（F006: running のまま残る問題を修正）
                    if let Some(mut meta) = store.load_session_meta(&session_id) {
                        meta.status = "idle".to_string();
                        let _ = store.update_session_meta(&meta);
                    }
                    // turn_started 済みなので turn_completed を送って UI をアンブロック
                    let resp = json!({ "type": "turn_completed", "session_id": &session_id });
                    let _ = ws_tx
                        .lock()
                        .await
                        .send(Message::Text(resp.to_string().into()))
                        .await;
                }
            }

            Some("stop_session") => {
                let session_id = match cmd["session_id"].as_str() {
                    Some(id) => id.to_string(),
                    None => continue,
                };

                // 状態マップから削除 & registry 名を取得
                let registry_name = {
                    let mut map = state_map.lock().await;
                    match map.remove(&session_id) {
                        Some(state) => state.registry_name,
                        None => format!("claude-{}", session_id),
                    }
                };

                registry.destroy(&registry_name).await;

                // Store メタを stopped に更新
                if let Some(mut meta) = store.load_session_meta(&session_id) {
                    meta.status = "stopped".to_string();
                    meta.finished_at = Some(Utc::now());
                    let _ = store.update_session_meta(&meta);
                }

                let resp = json!({ "type": "session_stopped", "session_id": session_id });
                let _ = ws_tx
                    .lock()
                    .await
                    .send(Message::Text(resp.to_string().into()))
                    .await;
            }

            Some("attach_session") => {
                // WS 再接続時にセッション復帰
                let session_id = match cmd["session_id"].as_str() {
                    Some(id) => id.to_string(),
                    None => {
                        send_error(&ws_tx, "session_id is required").await;
                        continue;
                    }
                };

                // まずローカル state_map を確認
                let shared_session = {
                    let map = state_map.lock().await;
                    map.get(&session_id).and_then(|s| s.shared_session.clone())
                };

                // ローカルになければ registry から復元（WS 再接続ケース）
                let shared_session = if shared_session.is_some() {
                    shared_session
                } else {
                    let registry_name = format!("claude-{}", session_id);
                    if let Some(shared) = registry.get(&registry_name).await {
                        let meta = store.load_session_meta(&session_id);
                        let is_running = meta
                            .as_ref()
                            .map(|m| m.status == "running")
                            .unwrap_or(false);
                        let mut map = state_map.lock().await;
                        map.insert(
                            session_id.clone(),
                            ClaudeSessionState {
                                is_running,
                                process_alive: true,
                                registry_name,
                                shared_session: Some(Arc::clone(&shared)),
                            },
                        );
                        Some(shared)
                    } else {
                        None
                    }
                };

                if let Some(shared_session) = shared_session {
                    // 新しい forwarder を起動
                    let forwarder_rx = shared_session.subscribe();
                    let ws_tx_for_output = Arc::clone(&ws_tx);
                    let sid_for_output = session_id.clone();
                    let session_for_output = Arc::clone(&shared_session);
                    let forwarder_state_map = Arc::clone(&state_map);

                    tokio::spawn(async move {
                        forward_interactive_output(
                            sid_for_output,
                            forwarder_rx,
                            ws_tx_for_output,
                            session_for_output,
                            forwarder_state_map,
                        )
                        .await;
                    });

                    let resp = json!({
                        "type": "session_attached",
                        "session_id": &session_id,
                    });
                    let _ = ws_tx
                        .lock()
                        .await
                        .send(Message::Text(resp.to_string().into()))
                        .await;
                } else {
                    send_error(&ws_tx, "Session not found or process not running").await;
                }
            }

            _ => {}
        }
    }

    // WS 切断 — processor/forwarder は続行（WS ライフサイクルから独立）
    tracing::info!("Claude WebSocket disconnected");
}

/// インタラクティブプロセッサ: broadcast から出力を読み、Store に永続化
/// ターン境界は `{"type": "result", ...}` イベントで検知
#[allow(clippy::too_many_arguments)]
async fn run_interactive_processor(
    session_id: String,
    mut output_rx: tokio::sync::broadcast::Receiver<Vec<u8>>,
    session: Arc<SharedSession>,
    store: Store,
    registry: Arc<SessionRegistry>,
    registry_name: String,
    state_map: SessionStateMap,
    ws_tx: WsSink,
) {
    let mut line_buf = String::new();
    #[cfg(windows)]
    let mut dsr_responded = false;

    loop {
        match tokio::time::timeout(OUTPUT_RECV_TIMEOUT, output_rx.recv()).await {
            Ok(Ok(bytes)) => {
                // ConPTY DSR 検出 → CPR 応答（Windows のみ）
                #[cfg(windows)]
                if !dsr_responded && bytes.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = session.write_input(b"\x1b[1;1R").await;
                    dsr_responded = true;
                }

                let text = String::from_utf8_lossy(&bytes);
                line_buf.push_str(&text);

                while let Some(pos) = line_buf.find('\n') {
                    let raw_line: String = line_buf[..pos].trim().into();
                    // replace_range is O(remaining) same as drain, but avoids reallocating
                    line_buf.replace_range(..=pos, "");

                    if raw_line.is_empty() {
                        continue;
                    }

                    let line = extract_json_line(&raw_line).unwrap_or(&raw_line);

                    if let Err(e) = store.append_event(&session_id, line) {
                        tracing::warn!("Failed to append event: {}", e);
                    }

                    // ターン境界検出: {"type": "result", ...}
                    if is_result_event(line) {
                        // is_running を false に
                        {
                            let mut map = state_map.lock().await;
                            if let Some(state) = map.get_mut(&session_id) {
                                state.is_running = false;
                            }
                        }
                        // Store メタを idle に更新
                        if let Some(mut meta) = store.load_session_meta(&session_id) {
                            meta.status = "idle".to_string();
                            let _ = store.update_session_meta(&meta);
                        }
                    }
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                tracing::warn!("Claude processor lagged {n} messages for session {session_id}");
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                break;
            }
            Err(_) => {
                if !session.is_alive() {
                    break;
                }
            }
        }
    }

    // 残りのバッファを処理
    let remaining = line_buf.trim().to_string();
    if !remaining.is_empty() {
        let line = extract_json_line(&remaining).unwrap_or(&remaining);
        if let Err(e) = store.append_event(&session_id, line) {
            tracing::warn!("Failed to append final event: {}", e);
        }
    }

    // プロセス死亡通知
    let session_still_active = {
        let mut map = state_map.lock().await;
        if let Some(state) = map.get_mut(&session_id) {
            state.process_alive = false;
            state.is_running = false;
            state.shared_session = None;
            true
        } else {
            false
        }
    };

    if session_still_active {
        // Store メタを completed に更新
        if let Some(mut meta) = store.load_session_meta(&session_id) {
            meta.status = "completed".to_string();
            meta.finished_at = Some(Utc::now());
            let _ = store.update_session_meta(&meta);
        }

        // process_died 通知をクライアントに送信
        let resp = json!({ "type": "process_died", "session_id": &session_id });
        let _ = ws_tx
            .lock()
            .await
            .send(Message::Text(resp.to_string().into()))
            .await;
    }

    // registry から削除
    registry.destroy(&registry_name).await;

    tracing::info!("Claude interactive process ended for session {session_id}");
}

/// broadcast → WS 転送（インタラクティブ: ターン境界で turn_completed を送信）
async fn forward_interactive_output(
    session_id: String,
    mut output_rx: tokio::sync::broadcast::Receiver<Vec<u8>>,
    ws_tx: WsSink,
    session: Arc<SharedSession>,
    state_map: SessionStateMap,
) {
    let mut line_buf = String::new();

    loop {
        match tokio::time::timeout(OUTPUT_RECV_TIMEOUT, output_rx.recv()).await {
            Ok(Ok(bytes)) => {
                let text = String::from_utf8_lossy(&bytes);
                line_buf.push_str(&text);

                while let Some(pos) = line_buf.find('\n') {
                    let raw_line: String = line_buf[..pos].trim().into();
                    line_buf.replace_range(..=pos, "");

                    if raw_line.is_empty() {
                        continue;
                    }

                    let line = extract_json_line(&raw_line)
                        .unwrap_or(&raw_line)
                        .to_string();

                    let event = json!({
                        "type": "claude_event",
                        "session_id": &session_id,
                        "event": Value::String(line.clone()),
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

                    // ターン境界検出 → turn_completed 通知
                    if is_result_event(&line) {
                        let resp = json!({ "type": "turn_completed", "session_id": &session_id });
                        let _ = ws_tx
                            .lock()
                            .await
                            .send(Message::Text(resp.to_string().into()))
                            .await;
                    }
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                // プロセス終了 → 最終ターンの turn_completed（is_running の場合のみ）
                let was_running = {
                    let map = state_map.lock().await;
                    map.get(&session_id).map(|s| s.is_running).unwrap_or(false)
                };
                if was_running {
                    let resp = json!({ "type": "turn_completed", "session_id": &session_id });
                    let _ = ws_tx
                        .lock()
                        .await
                        .send(Message::Text(resp.to_string().into()))
                        .await;
                }
                break;
            }
            Err(_) => {
                if !session.is_alive() {
                    let was_running = {
                        let map = state_map.lock().await;
                        map.get(&session_id).map(|s| s.is_running).unwrap_or(false)
                    };
                    if was_running {
                        let resp = json!({ "type": "turn_completed", "session_id": &session_id });
                        let _ = ws_tx
                            .lock()
                            .await
                            .send(Message::Text(resp.to_string().into()))
                            .await;
                    }
                    break;
                }
            }
        }
    }

    // 残りのバッファを送信
    let remaining = line_buf.trim().to_string();
    if !remaining.is_empty() {
        let line = extract_json_line(&remaining)
            .unwrap_or(&remaining)
            .to_string();
        let event = json!({
            "type": "claude_event",
            "session_id": &session_id,
            "event": Value::String(line),
        });
        let _ = ws_tx
            .lock()
            .await
            .send(Message::Text(event.to_string().into()))
            .await;
    }
}

/// JSON 行が {"type": "result", ...} かチェック（文字列検索で高速判定）
fn is_result_event(line: &str) -> bool {
    // "type":"result" または "type": "result" のパターンを文字列検索で判定
    // フルパースを回避して高速化
    (line.contains("\"type\":\"result\"") || line.contains("\"type\": \"result\""))
        && line.starts_with('{')
}

async fn send_error(ws_tx: &WsSink, message: &str) {
    let resp = json!({ "type": "error", "message": message });
    let _ = ws_tx
        .lock()
        .await
        .send(Message::Text(resp.to_string().into()))
        .await;
}

/// ConPTY エスケープシーケンスが混入した行から JSON 部分を抽出
///
/// ConPTY は出力に ANSI エスケープシーケンス（カーソル移動、属性リセット等）を付加することがある。
/// Claude CLI の stream-json 出力は 1 行 1 JSON オブジェクトなので、
/// 最初の `{` から最後の `}` までを抽出すれば有効な JSON が得られる。
fn extract_json_line(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let end = line.rfind('}')?;
    if end >= start {
        Some(&line[start..=end])
    } else {
        None
    }
}

/// Claude CLI の stream-json 入力形式（NDJSON）でユーザーメッセージを構築
///
/// `session_id` は Claude CLI 内部のセッション識別に使用される。
/// 同一プロセス内では一貫した値を渡す必要がある。
/// serde_json の compact 出力（改行なし）に依存して末尾 `\n` を NDJSON デリミタとする。
fn build_stream_json_input(prompt: &str, session_id: &str) -> String {
    let msg = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": prompt,
        },
        "session_id": session_id,
        "parent_tool_use_id": null,
    });
    format!("{}\n", msg)
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

    #[test]
    fn extract_json_line_clean() {
        let line = r#"{"type":"message","content":"hello"}"#;
        assert_eq!(extract_json_line(line), Some(line));
    }

    #[test]
    fn extract_json_line_with_escape_prefix() {
        // ConPTY がカーソル移動等のエスケープを先頭に付加するケース
        let line = "\x1b[0m\x1b[?25l{\"type\":\"message\"}";
        assert_eq!(extract_json_line(line), Some("{\"type\":\"message\"}"));
    }

    #[test]
    fn extract_json_line_with_escape_suffix() {
        let line = "{\"type\":\"message\"}\x1b[0m";
        assert_eq!(extract_json_line(line), Some("{\"type\":\"message\"}"));
    }

    #[test]
    fn extract_json_line_no_json() {
        assert_eq!(extract_json_line("plain text"), None);
        assert_eq!(extract_json_line(""), None);
    }

    #[test]
    fn extract_json_line_nested_braces() {
        let line = r#"{"type":"result","data":{"key":"value"}}"#;
        assert_eq!(extract_json_line(line), Some(line));
    }

    #[test]
    fn is_result_event_true() {
        let line = r#"{"type":"result","total_cost_usd":0.05}"#;
        assert!(is_result_event(line));
    }

    #[test]
    fn is_result_event_false() {
        let line = r#"{"type":"assistant","message":{}}"#;
        assert!(!is_result_event(line));
    }

    #[test]
    fn is_result_event_invalid_json() {
        assert!(!is_result_event("not json"));
        assert!(!is_result_event(""));
    }

    #[test]
    fn build_stream_json_input_format() {
        let input = build_stream_json_input("hello world", "test-session-123");
        let parsed: Value = serde_json::from_str(input.trim()).unwrap();
        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["message"]["role"], "user");
        assert_eq!(parsed["message"]["content"], "hello world");
        assert_eq!(parsed["session_id"], "test-session-123");
        assert!(parsed["parent_tool_use_id"].is_null());
        assert!(input.ends_with('\n'));
    }

    #[test]
    fn build_stream_json_input_escapes_special_chars() {
        let input = build_stream_json_input("test \"quotes\" and\nnewlines", "s1");
        let parsed: Value = serde_json::from_str(input.trim()).unwrap();
        assert_eq!(
            parsed["message"]["content"],
            "test \"quotes\" and\nnewlines"
        );
    }
}
