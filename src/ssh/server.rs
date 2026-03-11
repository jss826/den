use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use russh::keys::ssh_key;
use russh::server::{Auth, Handler, Msg, Server as _, Session};
use russh::{ChannelId, CryptoVec, Pty};
use tokio::net::TcpListener;

use crate::auth::constant_time_eq;
use crate::peer::{PeerRegistry, PeerStatus};
use crate::pty::registry::{ClientKind, SessionRegistry, SharedSession};
use crate::store::{PeerConfig, Store};

/// SSH セッション非アクティブタイムアウト（1時間）
/// `claude -p` 等の長時間コマンドでも切断されないよう余裕を持たせる。
/// 実際の死活監視は keepalive で行う。
const SSH_INACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

/// SSH keepalive 送信間隔（30秒ごと）
const SSH_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// keepalive 無応答でコネクション切断する回数（3回 = 最大90秒）
const SSH_KEEPALIVE_MAX: usize = 3;

/// パスワード認証失敗時の遅延（ブルートフォース対策）
const SSH_PASSWORD_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// PTY 出力受信タイムアウト（alive チェック間隔）
const OUTPUT_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Maximum concurrent SSH connections from localhost (loopback self-connection guard)
const MAX_SSH_LOOPBACK: usize = 10;

/// Escape character state machine for `~` sequences (like OpenSSH).
/// Detects `Enter → ~ → command` patterns in SSH input.
#[derive(Default, Clone, Copy)]
enum EscapeState {
    #[default]
    Normal,
    /// CR or LF received — next `~` starts an escape sequence
    AfterNewline,
    /// `~` received after newline — waiting for the command character
    AfterTilde,
}

/// `{data_dir}/ssh/authorized_keys` から公開鍵を読み込む。
/// 各行の "algorithm base64" 部分（コメント除去）を返す。
fn load_authorized_keys(data_dir: &str) -> HashSet<String> {
    let path = std::path::Path::new(data_dir)
        .join("ssh")
        .join("authorized_keys");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return HashSet::new(),
    };
    let keys: HashSet<String> = content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let algo = parts.next()?;
            let data = parts.next()?;
            Some(format!("{algo} {data}"))
        })
        .collect();
    if !keys.is_empty() {
        tracing::info!("SSH: loaded {} authorized key(s)", keys.len());
    }
    keys
}

/// OpenSSH 形式の鍵文字列から "algorithm base64" 部分を抽出する。
fn key_identity(openssh_line: &str) -> String {
    let mut parts = openssh_line.split_whitespace();
    let algo = parts.next().unwrap_or("");
    let data = parts.next().unwrap_or("");
    format!("{algo} {data}")
}

/// SSH サーバーを起動
pub async fn run(
    registry: Arc<SessionRegistry>,
    password: String,
    port: u16,
    data_dir: String,
    bind_address: String,
    store: Store,
    peer_registry: Arc<PeerRegistry>,
) -> anyhow::Result<()> {
    // ホストキー読み込み/生成
    let host_key = super::keys::load_or_generate_host_key(std::path::Path::new(&data_dir))?;

    let authorized_keys: Arc<HashSet<String>> = Arc::new(load_authorized_keys(&data_dir));

    // auth_rejection_time を 0 にして、パスワード認証のみハンドラ側で遅延させる。
    // これにより公開鍵認証の拒否が即座に完了し、クライアントがパスワード認証に
    // 素早くフォールバックできる。
    let config = russh::server::Config {
        inactivity_timeout: Some(SSH_INACTIVITY_TIMEOUT),
        keepalive_interval: Some(SSH_KEEPALIVE_INTERVAL),
        keepalive_max: SSH_KEEPALIVE_MAX,
        auth_rejection_time: std::time::Duration::from_secs(0),
        auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
        keys: vec![host_key],
        ..Default::default()
    };
    let config = Arc::new(config);

    let instance_id = registry.instance_id().to_string();
    let mut server = DenSshServer {
        registry,
        password,
        authorized_keys,
        instance_id,
        loopback_count: Arc::new(AtomicUsize::new(0)),
        ssh_port: port,
        store,
        peer_registry,
    };

    let addr = format!("{bind_address}:{port}");
    let socket = TcpListener::bind(&addr).await?;
    tracing::info!("SSH server listening on {addr}");

    server.run_on_socket(config, &socket).await?;

    Ok(())
}

#[derive(Clone)]
struct DenSshServer {
    registry: Arc<SessionRegistry>,
    password: String,
    authorized_keys: Arc<HashSet<String>>,
    instance_id: String,
    loopback_count: Arc<AtomicUsize>,
    ssh_port: u16,
    store: Store,
    peer_registry: Arc<PeerRegistry>,
}

impl russh::server::Server for DenSshServer {
    type Handler = DenSshHandler;

    fn new_client(&mut self, addr: Option<std::net::SocketAddr>) -> DenSshHandler {
        tracing::info!("SSH client connected from {:?}", addr);
        let is_local = addr.is_some_and(|a| super::loopback::is_local_address(&a));
        if is_local {
            self.loopback_count.fetch_add(1, Ordering::Relaxed);
        }
        DenSshHandler {
            registry: Arc::clone(&self.registry),
            password: self.password.clone(),
            authorized_keys: Arc::clone(&self.authorized_keys),
            instance_id: self.instance_id.clone(),
            is_loopback: is_local,
            self_connection_detected: false,
            loopback_count: Arc::clone(&self.loopback_count),
            peer_addr: addr,
            ssh_port: self.ssh_port,
            store: self.store.clone(),
            peer_registry: Arc::clone(&self.peer_registry),
            session_name: None,
            client_id: None,
            channel_id: None,
            shared_session: None,
            output_task: None,
            pty_cols: 80,
            pty_rows: 24,
            pty_requested: false,
            escape_state: EscapeState::default(),
            connected_at: None,
            is_remote: false,
            remote_ws_tx: None,
            remote_enc_key: None,
        }
    }
}

struct DenSshHandler {
    registry: Arc<SessionRegistry>,
    password: String,
    authorized_keys: Arc<HashSet<String>>,
    // Self-connection detection
    instance_id: String,
    is_loopback: bool,
    self_connection_detected: bool,
    loopback_count: Arc<AtomicUsize>,
    peer_addr: Option<std::net::SocketAddr>,
    ssh_port: u16,
    // Peer networking
    store: Store,
    peer_registry: Arc<PeerRegistry>,
    // Per-connection state
    session_name: Option<String>,
    client_id: Option<u64>,
    channel_id: Option<ChannelId>,
    shared_session: Option<Arc<SharedSession>>,
    output_task: Option<tokio::task::JoinHandle<()>>,
    pty_cols: u16,
    pty_rows: u16,
    pty_requested: bool,
    escape_state: EscapeState,
    connected_at: Option<std::time::Instant>,
    /// Whether the current session is a remote peer session
    is_remote: bool,
    /// WebSocket sender for remote peer sessions (SSH → remote WS)
    remote_ws_tx: Option<RemoteWsTx>,
    /// Encryption key for remote peer WS (set when starting remote bridge)
    remote_enc_key: Option<String>,
}

type RemoteWsTx = Arc<
    tokio::sync::Mutex<
        futures::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::Message,
        >,
    >,
>;

impl DenSshHandler {
    /// セッションに attach して I/O ブリッジを開始
    async fn start_bridge(
        &mut self,
        session_name: &str,
        session: &mut Session,
    ) -> Result<(), anyhow::Error> {
        let channel_id = self
            .channel_id
            .ok_or_else(|| anyhow::anyhow!("No channel"))?;

        // Layer 1: DEN_INSTANCE env var match → definite self-connection
        if self.self_connection_detected {
            tracing::warn!("SSH self-connection detected via DEN_INSTANCE env var");
            session.data(
                channel_id,
                CryptoVec::from_slice(
                    b"Error: Self-connection detected (DEN_INSTANCE match).\r\n\
                      Connecting to Den from within a Den session creates an infinite loop.\r\n",
                ),
            )?;
            session.close(channel_id)?;
            return Ok(());
        }

        // Layer 2: Too many loopback connections → likely self-connection loop
        if self.is_loopback {
            let count = self.loopback_count.load(Ordering::Relaxed);
            if count > MAX_SSH_LOOPBACK {
                tracing::warn!(
                    "SSH loopback connection limit exceeded: {count}/{MAX_SSH_LOOPBACK}"
                );
                let msg = format!(
                    "Error: Too many SSH connections from localhost ({count} exceeds limit of {MAX_SSH_LOOPBACK}).\r\n\
                     This may indicate a self-connection loop (SSH into Den from within Den).\r\n"
                );
                session.data(channel_id, CryptoVec::from_slice(msg.as_bytes()))?;
                session.close(channel_id)?;
                return Ok(());
            }
        }

        // Layer 3: Process tree inspection — check if the SSH client process
        // is a descendant of any Den PTY child process (definitive detection)
        if let Some(peer) = self.peer_addr
            && self.is_loopback
        {
            let child_pids = self.registry.collect_child_pids().await;
            if !child_pids.is_empty() {
                let ssh_port = self.ssh_port;
                let is_self = tokio::task::spawn_blocking(move || {
                    super::loopback::is_self_connection(peer, ssh_port, &child_pids)
                })
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("is_self_connection task failed: {e}");
                    false
                });

                if is_self {
                    tracing::warn!("SSH self-connection detected via process tree (peer={peer})");
                    session.data(
                        channel_id,
                        CryptoVec::from_slice(
                            b"Error: Self-connection detected.\r\n\
                              Connecting to Den from within a Den terminal session \
                              creates an infinite loop.\r\n",
                        ),
                    )?;
                    session.close(channel_id)?;
                    return Ok(());
                }
            }
        }

        let cols = self.pty_cols;
        let rows = self.pty_rows;

        let (shared_session, mut output_rx, replay, client_id) = self
            .registry
            .get_or_create(session_name, ClientKind::Ssh, cols, rows)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        self.session_name = Some(session_name.to_string());
        self.client_id = Some(client_id);
        self.shared_session = Some(Arc::clone(&shared_session));
        self.connected_at = Some(std::time::Instant::now());
        self.escape_state = EscapeState::AfterNewline;

        // replay data を送信（SSH 非互換モードを除去）
        tracing::debug!(
            "SSH start_bridge: session={session_name}, replay={} bytes",
            replay.len()
        );
        // Pre-compute the OSC replacement once per connection (avoids format! on every chunk).
        // filter_output_for_ssh does not strip OSC 0/1/2, so replace_osc_title always sees them.
        let osc_replacement: Vec<u8> = format!("\x1b]0;Den SSH [{session_name}]\x07").into_bytes();

        // ターミナル画面をクリアしてカーソルをホームに戻す。
        // これにより、クライアントの既存の画面内容（ssh コマンド等）と
        // リプレイバッファの内容が混ざって表示が崩れるのを防ぐ。
        session.data(channel_id, CryptoVec::from_slice(b"\x1b[2J\x1b[H"))?;

        if !replay.is_empty() {
            let filtered_replay = filter_output_for_ssh(&replay);
            let filtered_replay = replace_osc_title(&filtered_replay, &osc_replacement);
            if !filtered_replay.is_empty() {
                session.data(channel_id, CryptoVec::from_slice(&filtered_replay))?;
            }
        }

        // Set terminal title to "Den SSH [session_name]"
        session.data(channel_id, CryptoVec::from_slice(&osc_replacement))?;

        // Output: broadcast::Receiver → SSH channel
        let handle = session.handle();
        let name_for_task = session_name.to_string();
        // Keep Arc alive so the session isn't dropped while output_task runs.
        // Also used for is_alive() checks inside the task.
        let session_ref = shared_session;

        self.output_task = Some(tokio::spawn(async move {
            let reason;
            loop {
                // recv with timeout: ConPTY は子プロセス終了後も reader を
                // ブロックし続けるため、定期的に alive を確認する
                match tokio::time::timeout(OUTPUT_RECV_TIMEOUT, output_rx.recv()).await {
                    Ok(Ok(data)) => {
                        let filtered = filter_output_for_ssh(&data);
                        let filtered = replace_osc_title(&filtered, &osc_replacement);
                        if filtered.is_empty() {
                            continue;
                        }
                        if handle
                            .data(channel_id, CryptoVec::from_slice(&filtered))
                            .await
                            .is_err()
                        {
                            tracing::info!(
                                "SSH output_task: handle.data() failed for {name_for_task}, client disconnected"
                            );
                            reason = "client_disconnected";
                            break;
                        }
                    }
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                        tracing::warn!("SSH client lagged {n} messages on {name_for_task}");
                    }
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                        tracing::debug!(
                            "SSH output_task: broadcast closed for {name_for_task}, session ended"
                        );
                        let _ = handle.exit_status_request(channel_id, 0).await;
                        let _ = handle.eof(channel_id).await;
                        let _ = handle.close(channel_id).await;
                        reason = "broadcast_closed";
                        break;
                    }
                    Err(_) => {
                        if !session_ref.is_alive() {
                            tracing::debug!(
                                "SSH output_task: session {name_for_task} is no longer alive"
                            );
                            let _ = handle.exit_status_request(channel_id, 0).await;
                            let _ = handle.eof(channel_id).await;
                            let _ = handle.close(channel_id).await;
                            reason = "session_dead";
                            break;
                        }
                    }
                }
            }

            tracing::debug!(
                "SSH output_task ended for session {name_for_task} (alive={}, reason={reason})",
                session_ref.is_alive()
            );
        }));

        Ok(())
    }

    /// Look up a connected peer by name
    fn lookup_connected_peer(&self, peer_name: &str) -> Option<PeerConfig> {
        let settings = self.store.load_settings();
        let peers = settings.peers.unwrap_or_default();
        let peer = peers.into_iter().find(|p| p.name == peer_name)?;

        // Only allow connected peers
        let (status, _, _) = self.peer_registry.get_health(&peer.name).unwrap_or((
            PeerStatus::Disconnected,
            None,
            None,
        ));
        if status != PeerStatus::Connected {
            return None;
        }
        Some(peer)
    }

    /// Get our peer name from settings
    fn my_peer_name(&self) -> String {
        let settings = self.store.load_settings();
        settings
            .peer_name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().to_string())
    }

    /// Start a remote bridge: SSH channel ↔ encrypted WebSocket relay to a peer
    async fn start_remote_bridge(
        &mut self,
        peer: &PeerConfig,
        session_name: &str,
        session: &mut Session,
    ) -> Result<(), anyhow::Error> {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{Message as TungMessage, client::IntoClientRequest};

        let enc_key = peer
            .encryption_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Peer {} has no encryption key", peer.name))?
            .to_string();

        let channel_id = self
            .channel_id
            .ok_or_else(|| anyhow::anyhow!("No channel"))?;

        let display_name = format!("{}:{}", peer.name, session_name);

        // Build remote encrypted WS URL
        let base = peer.url.trim_end_matches('/');
        let ws_base = if base.starts_with("https://") {
            base.replacen("https://", "wss://", 1)
        } else {
            base.replacen("http://", "ws://", 1)
        };
        let my_name = self.my_peer_name();
        let cols = self.pty_cols;
        let rows = self.pty_rows;
        let url = format!(
            "{}/api/peer-ws?peer={}&session={}&cols={}&rows={}",
            ws_base, my_name, session_name, cols, rows
        );

        let request = url
            .into_client_request()
            .map_err(|e| anyhow::anyhow!("Invalid WS URL: {e}"))?;

        // Connect to remote encrypted WS
        let (remote_ws, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to peer {}: {e}", peer.name))?;

        tracing::info!(
            "SSH encrypted remote bridge established: {} → {}",
            display_name,
            peer.name
        );

        self.session_name = Some(display_name.clone());
        self.connected_at = Some(std::time::Instant::now());
        self.escape_state = EscapeState::AfterNewline;
        self.is_remote = true;

        // Clear screen and set title
        let osc_title: Vec<u8> = format!("\x1b]0;Den SSH [{display_name}]\x07").into_bytes();
        session.data(channel_id, CryptoVec::from_slice(b"\x1b[2J\x1b[H"))?;
        session.data(channel_id, CryptoVec::from_slice(&osc_title))?;

        // Split WS
        let (remote_tx, mut remote_rx) = remote_ws.split();
        let remote_tx = Arc::new(tokio::sync::Mutex::new(remote_tx));

        // Store remote_tx for input forwarding
        let remote_tx_for_input = Arc::clone(&remote_tx);

        // Output: remote encrypted WS → decrypt → SSH channel
        let handle = session.handle();
        let enc_key_for_output = enc_key.clone();
        self.output_task = Some(tokio::spawn(async move {
            while let Some(msg) = remote_rx.next().await {
                match msg {
                    Ok(TungMessage::Binary(encrypted_data)) => {
                        // Decrypt the frame
                        let plain =
                            match crate::crypto::decrypt(&encrypted_data, &enc_key_for_output) {
                                Ok(p) => p,
                                Err(e) => {
                                    tracing::warn!(
                                        "SSH remote decrypt failed for {display_name}: {e}"
                                    );
                                    break;
                                }
                            };
                        if plain.is_empty() {
                            continue;
                        }
                        // First byte: 0=text, 1=binary
                        let type_byte = plain[0];
                        let payload = &plain[1..];

                        if type_byte == 0 {
                            // Text message — check for session_ended
                            let text = String::from_utf8_lossy(payload);
                            if text.contains("session_ended") {
                                let _ = handle.exit_status_request(channel_id, 0).await;
                                let _ = handle.eof(channel_id).await;
                                let _ = handle.close(channel_id).await;
                                break;
                            }
                        } else {
                            // Binary message — terminal output
                            let filtered = filter_output_for_ssh(payload);
                            let filtered = replace_osc_title(&filtered, &osc_title);
                            if filtered.is_empty() {
                                continue;
                            }
                            if handle
                                .data(channel_id, CryptoVec::from_slice(&filtered))
                                .await
                                .is_err()
                            {
                                tracing::debug!(
                                    "SSH remote output: handle.data() failed for {display_name}"
                                );
                                break;
                            }
                        }
                    }
                    Ok(TungMessage::Close(_)) | Err(_) => {
                        let _ = handle.exit_status_request(channel_id, 0).await;
                        let _ = handle.eof(channel_id).await;
                        let _ = handle.close(channel_id).await;
                        break;
                    }
                    _ => {}
                }
            }
            // Close remote WS
            let mut tx = remote_tx.lock().await;
            let _ = tx.close().await;
            tracing::debug!("SSH remote output ended for {display_name}");
        }));

        // Store the remote_tx for use in data() handler
        self.shared_session = None;
        self.client_id = None;
        // We use a separate channel to pass remote_tx to the data handler.
        // Since russh Handler methods take &mut self, we store it directly.
        self.remote_ws_tx = Some(remote_tx_for_input);
        self.remote_enc_key = Some(enc_key);

        Ok(())
    }

    /// Parse `peer:session` format and route to local or remote bridge
    async fn attach_or_remote(
        &mut self,
        name: &str,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), anyhow::Error> {
        if let Some((peer_name, session_name)) = name.split_once(':') {
            // Remote peer session
            let peer = match self.lookup_connected_peer(peer_name) {
                Some(p) => p,
                None => {
                    let msg = format!("Peer not found or not connected: {peer_name}\r\n");
                    session.data(channel, CryptoVec::from_slice(msg.as_bytes()))?;
                    session.close(channel)?;
                    return Ok(());
                }
            };
            if session_name.is_empty() {
                session.data(
                    channel,
                    CryptoVec::from_slice(b"Missing session name after ':'\r\n"),
                )?;
                session.close(channel)?;
                return Ok(());
            }
            self.start_remote_bridge(&peer, session_name, session)
                .await?;
        } else {
            self.start_bridge(name, session).await?;
        }
        Ok(())
    }

    /// Fetch session lists from all connected peers (via encrypted RPC)
    async fn fetch_remote_sessions(&self) -> Vec<(String, Vec<serde_json::Value>)> {
        let settings = self.store.load_settings();
        let peers = settings.peers.unwrap_or_default();
        let my_name = self.my_peer_name();

        let mut results = Vec::new();
        for peer in &peers {
            let (status, _, _) = self.peer_registry.get_health(&peer.name).unwrap_or((
                PeerStatus::Disconnected,
                None,
                None,
            ));
            if status != PeerStatus::Connected {
                continue;
            }

            let enc_key = match &peer.encryption_key {
                Some(k) => k,
                None => continue,
            };

            // Use shared encrypt-send-decrypt helper (same protocol as send_encrypted_rpc)
            let rpc_req = crate::peer::RpcRequest {
                method: "GET".to_string(),
                path: "/api/terminal/sessions".to_string(),
                query: None,
                headers: std::collections::HashMap::new(),
                body: vec![],
            };
            let plaintext = match serde_json::to_vec(&rpc_req) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let encrypted = match crate::crypto::encrypt(&plaintext, enc_key) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
            {
                Ok(c) => c,
                Err(_) => continue,
            };

            let url = format!("{}/api/peer-rpc", peer.url.trim_end_matches('/'));
            let resp = match client
                .post(&url)
                .header("Content-Type", "application/octet-stream")
                .header("X-Peer-Name", &my_name)
                .body(encrypted)
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => r,
                _ => continue,
            };

            let enc_body = match resp.bytes().await {
                Ok(b) => b,
                Err(_) => continue,
            };

            let dec_body = match crate::crypto::decrypt(&enc_body, enc_key) {
                Ok(d) => d,
                Err(_) => continue,
            };

            // Parse the RPC response envelope (same format as RpcResponse)
            let rpc_resp: crate::peer::RpcResponse = match serde_json::from_slice(&dec_body) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if rpc_resp.status == 200
                && let Ok(sessions) =
                    serde_json::from_slice::<Vec<serde_json::Value>>(&rpc_resp.body)
                && !sessions.is_empty()
            {
                results.push((peer.name.clone(), sessions));
            }
        }
        results
    }

    /// Filter and forward buffered bytes to the PTY.
    async fn flush_to_pty(
        shared: &SharedSession,
        client_id: Option<u64>,
        session_name: Option<&str>,
        buf: &[u8],
    ) {
        if buf.is_empty() {
            return;
        }
        let filtered = filter_terminal_responses(buf);
        if buf.len() != filtered.len()
            && let Some(name) = session_name
        {
            tracing::debug!(
                "SSH data: {} bytes in, {} bytes after filter (session {name})",
                buf.len(),
                filtered.len(),
            );
        }
        if filtered.is_empty() {
            return;
        }
        if let Some(client_id) = client_id {
            let _ = shared.write_input_from(client_id, &filtered).await;
        } else if let Some(name) = session_name {
            tracing::warn!("SSH data: client_id is None, dropping input (session {name})");
        }
    }

    /// Format the `~s` status message to inject into the SSH channel.
    async fn format_status(&self) -> String {
        let session_name = self.session_name.as_deref().unwrap_or("(none)");

        let connected = match self.connected_at {
            Some(t) => {
                let secs = t.elapsed().as_secs();
                let h = secs / 3600;
                let m = (secs % 3600) / 60;
                if h > 0 {
                    format!("{h}h {m:02}m")
                } else {
                    format!("{m}m")
                }
            }
            None => "-".to_string(),
        };

        let (process, clients) = if let Some(ref ss) = self.shared_session {
            let alive = if ss.is_alive() { "alive" } else { "dead" };
            let count = ss.inner.lock().await.client_count();
            (alive.to_string(), count.to_string())
        } else {
            ("-".to_string(), "-".to_string())
        };

        format!(
            "\r\n\x1b[1m── Den SSH ─────────────────────\x1b[0m\r\n\
             \x1b[1m  Session:\x1b[0m   {session_name}\r\n\
             \x1b[1m  Connected:\x1b[0m {connected}\r\n\
             \x1b[1m  Process:\x1b[0m   {process}\r\n\
             \x1b[1m  Clients:\x1b[0m   {clients}\r\n\
             \x1b[1m─────────────────────────────────\x1b[0m\r\n"
        )
    }

    /// Format the `~?` help message.
    fn format_help() -> &'static str {
        "\r\n\
         \x1b[1m  ~s\x1b[0m  Show status\r\n\
         \x1b[1m  ~r\x1b[0m  Force screen redraw\r\n\
         \x1b[1m  ~?\x1b[0m  Show help\r\n\
         \x1b[1m  ~~\x1b[0m  Send literal ~\r\n"
    }

    /// cleanup: detach + output_task abort
    async fn cleanup(&mut self) {
        if !self.is_remote {
            if let (Some(name), Some(client_id)) = (self.session_name.take(), self.client_id.take())
            {
                self.registry.detach(&name, client_id).await;
            }
        } else {
            self.session_name.take();
        }
        self.shared_session.take();
        // Close remote WS if present
        if let Some(remote_tx) = self.remote_ws_tx.take() {
            use futures::SinkExt;
            let mut tx = remote_tx.lock().await;
            let _ = tx.close().await;
        }
        if let Some(task) = self.output_task.take() {
            task.abort();
        }
    }
}

impl Handler for DenSshHandler {
    type Error = anyhow::Error;

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        public_key: &ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        if self.authorized_keys.is_empty() {
            return Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            });
        }
        let offered = key_identity(&public_key.to_string());
        if self.authorized_keys.contains(&offered) {
            tracing::info!("SSH auth: public key offered — accepted for verification");
            Ok(Auth::Accept)
        } else {
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        public_key: &ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        let offered = key_identity(&public_key.to_string());
        if self.authorized_keys.contains(&offered) {
            tracing::info!("SSH auth: public key accepted");
            Ok(Auth::Accept)
        } else {
            tracing::warn!("SSH auth: public key rejected");
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        if constant_time_eq(password, &self.password) {
            tracing::info!("SSH auth: password accepted");
            Ok(Auth::Accept)
        } else {
            tracing::warn!("SSH auth: password rejected");
            // auth_rejection_time を 0 にしたため、ブルートフォース対策の遅延をここで入れる
            tokio::time::sleep(SSH_PASSWORD_DELAY).await;
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.channel_id = Some(channel.id());
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        _channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.pty_cols = col_width as u16;
        self.pty_rows = row_height as u16;
        self.pty_requested = true;
        let ch = self
            .channel_id
            .ok_or_else(|| anyhow::anyhow!("No channel open"))?;
        session.channel_success(ch)?;
        Ok(())
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        variable_name: &str,
        variable_value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if variable_name == "DEN_INSTANCE" {
            if variable_value == self.instance_id {
                tracing::debug!("SSH env_request: DEN_INSTANCE matches — self-connection");
                self.self_connection_detected = true;
            }
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    async fn shell_request(
        &mut self,
        _channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // shell_request はデフォルトセッション "default" に attach
        let ch = self
            .channel_id
            .ok_or_else(|| anyhow::anyhow!("No channel open"))?;
        session.channel_success(ch)?;
        self.start_bridge("default", session).await?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).trim().to_string();
        let parts: Vec<&str> = command.splitn(2, ' ').collect();

        match parts.first().copied() {
            Some("list") => {
                // セッション一覧をテキストで返す（ローカル + リモートピア）
                session.channel_success(channel)?;
                let sessions = self.registry.list().await;
                let mut output = String::new();
                let mut has_any = false;

                // Local sessions
                if !sessions.is_empty() {
                    has_any = true;
                    output.push_str("Sessions:\r\n");
                    for s in &sessions {
                        let status = if s.alive { "alive" } else { "dead" };
                        output.push_str(&format!(
                            "  {} ({}, {} clients)\r\n",
                            s.name, status, s.client_count
                        ));
                    }
                }

                // Remote peer sessions
                let remote = self.fetch_remote_sessions().await;
                if !remote.is_empty() {
                    has_any = true;
                    for (peer_name, peer_sessions) in &remote {
                        for s in peer_sessions {
                            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                            output.push_str(&format!("  {}:{} (peer)\r\n", peer_name, name));
                        }
                    }
                }

                if !has_any {
                    output.push_str("No active sessions\r\n");
                }

                session.data(channel, CryptoVec::from_slice(output.as_bytes()))?;
                session.close(channel)?;
                Ok(())
            }

            Some("attach") => {
                let name = parts.get(1).unwrap_or(&"default").trim();
                session.channel_success(channel)?;
                if name.is_empty() {
                    session.data(
                        channel,
                        CryptoVec::from_slice(b"Usage: attach <session-name>\r\n"),
                    )?;
                    session.close(channel)?;
                    return Ok(());
                }
                if !self.pty_requested {
                    session.data(
                        channel,
                        CryptoVec::from_slice(
                            b"Error: PTY required. Use: ssh -t ... attach <name>\r\n",
                        ),
                    )?;
                    session.close(channel)?;
                    return Ok(());
                }
                self.attach_or_remote(name, channel, session).await?;
                Ok(())
            }

            Some("new") => {
                let name = parts.get(1).unwrap_or(&"default").trim();
                session.channel_success(channel)?;
                if name.is_empty() {
                    session.data(
                        channel,
                        CryptoVec::from_slice(b"Usage: new <session-name>\r\n"),
                    )?;
                    session.close(channel)?;
                    return Ok(());
                }
                if !self.pty_requested {
                    session.data(
                        channel,
                        CryptoVec::from_slice(
                            b"Error: PTY required. Use: ssh -t ... new <name>\r\n",
                        ),
                    )?;
                    session.close(channel)?;
                    return Ok(());
                }
                // 既存セッションがあればエラー
                if self.registry.exists(name).await {
                    let msg = format!("Session already exists: {name}\r\n");
                    session.data(channel, CryptoVec::from_slice(msg.as_bytes()))?;
                    session.close(channel)?;
                    return Ok(());
                }
                self.start_bridge(name, session).await?;
                Ok(())
            }

            _ => {
                // コマンドなし or 不明 → attach default
                session.channel_success(channel)?;
                if !self.pty_requested {
                    session.data(
                        channel,
                        CryptoVec::from_slice(
                            b"Error: PTY required. Use: ssh -t ... attach <name>\r\n",
                        ),
                    )?;
                    session.close(channel)?;
                    return Ok(());
                }
                self.start_bridge("default", session).await?;
                Ok(())
            }
        }
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let channel_id = match self.channel_id {
            Some(ch) => ch,
            None => return Ok(()),
        };

        let (forward, commands) = process_escape_input(&mut self.escape_state, data);

        // Inject escape command outputs into SSH channel
        for cmd in &commands {
            match cmd {
                EscapeCommand::ShowStatus => {
                    let output = self.format_status().await;
                    session.data(channel_id, CryptoVec::from_slice(output.as_bytes()))?;
                }
                EscapeCommand::ShowHelp => {
                    let output = Self::format_help().to_string();
                    session.data(channel_id, CryptoVec::from_slice(output.as_bytes()))?;
                }
                EscapeCommand::ForceRedraw => {
                    if !self.is_remote
                        && let (Some(shared), Some(client_id)) =
                            (&self.shared_session, self.client_id)
                    {
                        shared.nudge_resize(client_id).await;
                    }
                }
            }
        }

        if forward.is_empty() {
            return Ok(());
        }

        if self.is_remote {
            // Encrypt and forward to remote peer via WebSocket
            if let (Some(remote_tx), Some(enc_key)) = (&self.remote_ws_tx, &self.remote_enc_key) {
                use futures::SinkExt;
                use tokio_tungstenite::tungstenite::Message as TungMessage;
                // Binary frame: type_byte=1 + payload
                let mut plain = Vec::with_capacity(1 + forward.len());
                plain.push(1u8); // binary type
                plain.extend_from_slice(&forward);
                if let Ok(encrypted) = crate::crypto::encrypt(&plain, enc_key) {
                    let mut tx = remote_tx.lock().await;
                    let _ = tx.send(TungMessage::Binary(encrypted.into())).await;
                }
            }
        } else if let Some(ref shared) = self.shared_session {
            // Forward to local PTY
            Self::flush_to_pty(
                shared,
                self.client_id,
                self.session_name.as_deref(),
                &forward,
            )
            .await;
        }

        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.pty_cols = col_width as u16;
        self.pty_rows = row_height as u16;

        if self.is_remote {
            // Encrypt and send resize command to remote peer via WebSocket
            if let (Some(remote_tx), Some(enc_key)) = (&self.remote_ws_tx, &self.remote_enc_key) {
                use futures::SinkExt;
                use tokio_tungstenite::tungstenite::Message as TungMessage;
                let json = format!(
                    r#"{{"type":"resize","cols":{},"rows":{}}}"#,
                    col_width, row_height
                );
                // Text frame: type_byte=0 + payload
                let mut plain = Vec::with_capacity(1 + json.len());
                plain.push(0u8); // text type
                plain.extend_from_slice(json.as_bytes());
                if let Ok(encrypted) = crate::crypto::encrypt(&plain, enc_key) {
                    let mut tx = remote_tx.lock().await;
                    let _ = tx.send(TungMessage::Binary(encrypted.into())).await;
                }
            }
        } else if let (Some(session), Some(client_id)) = (&self.shared_session, self.client_id) {
            session
                .resize(client_id, col_width as u16, row_height as u16)
                .await;
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.cleanup().await;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.cleanup().await;
        Ok(())
    }
}

impl Drop for DenSshHandler {
    fn drop(&mut self) {
        if self.is_loopback {
            self.loopback_count.fetch_sub(1, Ordering::Relaxed);
        }

        // Drop 時に cleanup できない（async）のでタスクを spawn
        if !self.is_remote {
            let session_name = self.session_name.take();
            let client_id = self.client_id.take();
            let registry = Arc::clone(&self.registry);

            if let (Some(name), Some(id)) = (session_name, client_id) {
                tokio::spawn(async move {
                    registry.detach(&name, id).await;
                });
            }
        }

        // Close remote WS sender
        if let Some(remote_tx) = self.remote_ws_tx.take() {
            tokio::spawn(async move {
                use futures::SinkExt;
                let mut tx = remote_tx.lock().await;
                let _ = tx.close().await;
            });
        }

        if let Some(task) = self.output_task.take() {
            task.abort();
        }
    }
}

/// Escape command detected during input processing.
#[derive(Debug, PartialEq)]
enum EscapeCommand {
    /// `~s` — show status (inject into SSH channel, don't forward to PTY)
    ShowStatus,
    /// `~?` — show help
    ShowHelp,
    /// `~r` — force redraw (nudge ConPTY)
    ForceRedraw,
}

/// Process input bytes through the escape state machine.
/// Returns (bytes to forward to PTY, list of escape commands detected).
///
/// The returned forward buffer does NOT include bytes consumed by escape commands.
/// `~~` produces a single literal `~` in the forward buffer.
fn process_escape_input(state: &mut EscapeState, data: &[u8]) -> (Vec<u8>, Vec<EscapeCommand>) {
    let mut forward = Vec::with_capacity(data.len());
    let mut commands = Vec::new();

    for &byte in data {
        match *state {
            EscapeState::Normal => {
                if byte == b'\r' || byte == b'\n' {
                    *state = EscapeState::AfterNewline;
                }
                forward.push(byte);
            }
            EscapeState::AfterNewline => {
                if byte == b'~' {
                    *state = EscapeState::AfterTilde;
                } else if byte == b'\r' || byte == b'\n' {
                    // Stay in AfterNewline
                    forward.push(byte);
                } else {
                    *state = EscapeState::Normal;
                    forward.push(byte);
                }
            }
            EscapeState::AfterTilde => {
                *state = EscapeState::Normal;
                match byte {
                    b's' => commands.push(EscapeCommand::ShowStatus),
                    b'?' => commands.push(EscapeCommand::ShowHelp),
                    b'r' => commands.push(EscapeCommand::ForceRedraw),
                    b'~' => forward.push(b'~'),
                    _ => {
                        forward.push(b'~');
                        if byte == b'\r' || byte == b'\n' {
                            *state = EscapeState::AfterNewline;
                        }
                        forward.push(byte);
                    }
                }
            }
        }
    }

    (forward, commands)
}

/// SSH クライアントへの出力から、SSH 非互換な ConPTY モードを除去する。
///
/// ConPTY は直結ターミナル向けに特殊モードを有効化するが、SSH 経由では
/// ローカルターミナルがモード切替を実行してしまい入力形式が変わる。
/// - `ESC[?9001h/l` — Win32 input mode: 全入力が `CSI ... _` 形式になり文字化け
/// - `ESC[?1004h/l` — Focus events: 不要な `ESC[I`/`ESC[O` が入力に混入
fn filter_output_for_ssh(data: &[u8]) -> Cow<'_, [u8]> {
    const BLOCKED: &[&[u8]] = &[
        b"\x1b[?9001h",
        b"\x1b[?9001l",
        b"\x1b[?1004h",
        b"\x1b[?1004l",
    ];

    // 高速パス: ESC がなければフィルタ不要
    if !data.contains(&0x1b) {
        return Cow::Borrowed(data);
    }

    // 二段階チェック: ブロック対象が含まれない場合はアロケーション不要
    if !BLOCKED
        .iter()
        .any(|seq| data.windows(seq.len()).any(|w| w == *seq))
    {
        return Cow::Borrowed(data);
    }

    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;

    while i < data.len() {
        let remaining = &data[i..];
        if let Some(seq) = BLOCKED.iter().find(|s| remaining.starts_with(s)) {
            i += seq.len();
        } else {
            result.push(data[i]);
            i += 1;
        }
    }

    Cow::Owned(result)
}

/// Check if position `start` in `data` begins an OSC 0, 1, or 2 sequence.
/// Expects `data[start]` to be `ESC` and `data[start+1]` to be `]`.
fn is_title_osc(data: &[u8], start: usize) -> bool {
    let i = start + 2; // skip ESC ]
    if i >= data.len() {
        return false;
    }
    match data[i] {
        // OSC 0 ; ... or OSC 1 ; ... or OSC 2 ; ...
        b'0' | b'1' | b'2' => {
            let next = i + 1;
            // Must be followed by `;` or terminator (BEL/ST) or end of data
            next >= data.len()
                || data[next] == b';'
                || data[next] == 0x07
                || (data[next] == 0x1b && next + 1 < data.len() && data[next + 1] == b'\\')
        }
        _ => false,
    }
}

/// Fast scan: does `data` contain any OSC 0/1/2 title sequence?
fn has_osc_title_sequence(data: &[u8]) -> bool {
    let mut i = 0;
    while i + 1 < data.len() {
        if data[i] == 0x1b {
            if data[i + 1] == b']' && is_title_osc(data, i) {
                return true;
            }
            i += 2; // skip ESC + next byte
        } else {
            i += 1;
        }
    }
    false
}

/// Replace OSC 0/1/2 title sequences with a pre-built replacement.
///
/// PowerShell (and other shells) continuously set the terminal title via
/// OSC 0/1/2 sequences. This overwrites any title we set initially.
/// By replacing every title OSC in the PTY output stream, the SSH client's
/// terminal always displays the Den session identifier.
///
/// Both BEL (0x07) and ST (ESC \) terminators are handled; the replacement
/// always uses BEL terminator regardless of the original.
/// Unterminated sequences are passed through unchanged (chunk-boundary safe).
fn replace_osc_title<'a>(data: &'a [u8], replacement: &[u8]) -> Cow<'a, [u8]> {
    // Fast path 1: no ESC at all
    if !data.contains(&0x1b) {
        return Cow::Borrowed(data);
    }

    // Fast path 2: ESC present but no title OSC — avoids allocation
    if !has_osc_title_sequence(data) {
        return Cow::Borrowed(data);
    }

    let mut result = Vec::with_capacity(data.len());
    let mut span_start = 0;
    let mut i = 0;

    while i < data.len() {
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b']' && is_title_osc(data, i) {
            let end = skip_osc_sequence(data, i);
            if end > i {
                // Terminated → flush preceding plain bytes, then replace
                result.extend_from_slice(&data[span_start..i]);
                result.extend_from_slice(replacement);
                i = end;
                span_start = i;
            } else {
                // Unterminated → pass through as-is (keep in current span)
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    // Flush trailing plain bytes
    result.extend_from_slice(&data[span_start..]);

    Cow::Owned(result)
}

/// SSH クライアントのターミナルが返す応答シーケンスをフィルタする。
///
/// ConPTY は初期化時にクエリを送信し、ターミナルが応答を返す。
/// CPR (Cursor Position Report: `ESC[n;mR`) は ConPTY が必要とするので通過させるが、
/// private prefix 付き CSI（DA, DECRQM 等）や DCS/OSC 文字列シーケンスは
/// シェルに生入力として渡されて文字化けを起こすため除去する。
fn filter_terminal_responses(data: &[u8]) -> Cow<'_, [u8]> {
    // 高速パス: ESC がなければフィルタ不要
    if !data.contains(&0x1b) {
        return Cow::Borrowed(data);
    }

    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;

    while i < data.len() {
        if data[i] != 0x1b {
            result.push(data[i]);
            i += 1;
            continue;
        }

        // ESC found
        if i + 1 >= data.len() {
            // Trailing ESC → keep
            result.push(data[i]);
            i += 1;
            continue;
        }

        match data[i + 1] {
            b'[' => {
                // CSI sequence: ESC [
                let start = i;
                i += 2;

                // Private prefix: ? > =
                // Note: `<` is NOT included — SGR mouse reports use CSI < ... M/m
                let has_private_prefix =
                    i < data.len() && (data[i] == b'?' || data[i] == b'>' || data[i] == b'=');
                if has_private_prefix {
                    i += 1;
                }

                // Parameter bytes: 0x30-0x3F (digits, ;, :, etc.)
                while i < data.len() && (0x30..=0x3f).contains(&data[i]) {
                    i += 1;
                }

                // Intermediate bytes: 0x20-0x2F ($, !, ", space, etc.)
                while i < data.len() && (0x20..=0x2f).contains(&data[i]) {
                    i += 1;
                }

                // Final byte: 0x40-0x7E
                if i < data.len() && (0x40..=0x7e).contains(&data[i]) {
                    i += 1;

                    if has_private_prefix {
                        // Private prefix CSI → filter (DA, DECRQM, DECSET responses, etc.)
                        continue;
                    }

                    result.extend_from_slice(&data[start..i]);
                } else {
                    // Incomplete CSI → keep as-is
                    result.extend_from_slice(&data[start..i]);
                }
            }

            // DCS (ESC P), SOS (ESC X), PM (ESC ^), APC (ESC _)
            b'P' | b'X' | b'^' | b'_' => {
                let end = skip_string_sequence(data, i);
                if end > i {
                    i = end; // Terminated → filter
                } else {
                    // Unterminated → keep ESC, advance 1 (rest follows as plain bytes)
                    result.push(data[i]);
                    i += 1;
                }
            }

            // OSC (ESC ])
            b']' => {
                let end = skip_osc_sequence(data, i);
                if end > i {
                    i = end; // Terminated → filter
                } else {
                    // Unterminated → keep ESC, advance 1
                    result.push(data[i]);
                    i += 1;
                }
            }

            _ => {
                // Other ESC sequences (e.g. ESC O for SS3) → keep
                result.push(data[i]);
                i += 1;
            }
        }
    }

    if result.len() == data.len() {
        Cow::Borrowed(data)
    } else {
        Cow::Owned(result)
    }
}

/// ST (`ESC \`) で終端される文字列シーケンスをスキップする。
/// DCS, SOS, PM, APC 用。
fn skip_string_sequence(data: &[u8], start: usize) -> usize {
    let mut i = start + 2; // skip ESC + introducer
    while i < data.len() {
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
            return i + 2; // consume ST
        }
        i += 1;
    }
    // Unterminated → keep bytes as-is to avoid losing subsequent input
    start
}

/// BEL (0x07) または ST (`ESC \`) で終端される OSC シーケンスをスキップする。
fn skip_osc_sequence(data: &[u8], start: usize) -> usize {
    let mut i = start + 2; // skip ESC ]
    while i < data.len() {
        if data[i] == 0x07 {
            return i + 1; // consume BEL
        }
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
            return i + 2; // consume ST
        }
        i += 1;
    }
    // Unterminated → keep bytes as-is to avoid losing subsequent input
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_cpr_response() {
        // ESC [ 1 ; 1 R → CPR (Cursor Position Report) → ConPTY が必要 → 保持
        let data = b"\x1b[1;1R";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn keep_cpr_large_numbers() {
        let data = b"\x1b[24;80R";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn filter_da1_response() {
        // ESC [ ? 1 ; 2 c → DA1 → 除去
        let data = b"\x1b[?1;2c";
        assert!(filter_terminal_responses(data).is_empty());
    }

    #[test]
    fn filter_da2_response() {
        // ESC [ > 0 ; 1 3 6 ; 0 c → DA2 → 除去
        let data = b"\x1b[>0;136;0c";
        assert!(filter_terminal_responses(data).is_empty());
    }

    #[test]
    fn keep_arrow_keys() {
        // ESC [ A/B/C/D → 矢印キー → 保持
        let data = b"\x1b[A\x1b[B\x1b[C\x1b[D";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn keep_function_keys() {
        // ESC [ 1 5 ~ → F5 → 保持
        let data = b"\x1b[15~";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn keep_plain_text() {
        let data = b"hello world";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn filter_da_mixed_input() {
        // DA1 + 通常テキスト → DA のみ除去
        let data = b"\x1b[?1;2chello";
        assert_eq!(filter_terminal_responses(data), &b"hello"[..]);
    }

    #[test]
    fn keep_cpr_filter_da() {
        // CPR + DA1 → CPR は保持、DA は除去
        let data = b"\x1b[24;80R\x1b[?1;2c";
        assert_eq!(filter_terminal_responses(data), &b"\x1b[24;80R"[..]);
    }

    #[test]
    fn keep_text_between_responses() {
        // テキスト + DA → テキスト保持
        let data = b"abc\x1b[?1;2c";
        assert_eq!(filter_terminal_responses(data), &b"abc"[..]);
    }

    #[test]
    fn key_identity_with_comment() {
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKey user@host";
        assert_eq!(
            key_identity(line),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKey"
        );
    }

    #[test]
    fn key_identity_without_comment() {
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKey";
        assert_eq!(
            key_identity(line),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleKey"
        );
    }

    #[test]
    fn output_filter_strips_win32_input_mode() {
        let data = b"\x1b[?9001h";
        assert!(filter_output_for_ssh(data).is_empty());
    }

    #[test]
    fn output_filter_strips_win32_input_mode_disable() {
        let data = b"\x1b[?9001l";
        assert!(filter_output_for_ssh(data).is_empty());
    }

    #[test]
    fn output_filter_strips_focus_events() {
        let data = b"\x1b[?1004h";
        assert!(filter_output_for_ssh(data).is_empty());
    }

    #[test]
    fn output_filter_keeps_other_sequences() {
        // ESC[?25h (show cursor) should be kept
        let data = b"\x1b[?25h";
        assert_eq!(filter_output_for_ssh(data), &data[..]);
    }

    #[test]
    fn output_filter_mixed() {
        // win32 mode + text + focus events → text only
        let data = b"\x1b[?9001h\x1b[?1004hHello\x1b[?25h";
        assert_eq!(filter_output_for_ssh(data), &b"Hello\x1b[?25h"[..]);
    }

    #[test]
    fn output_filter_strips_from_conpty_init() {
        // Realistic ConPTY init sequence: win32 + focus + DSR
        let data = b"\x1b[?9001h\x1b[?1004h\x1b[6n";
        assert_eq!(filter_output_for_ssh(data), &b"\x1b[6n"[..]);
    }

    #[test]
    fn filter_decrqm_response() {
        // ESC [ ? 1 ; 1 $ y → DECRQM → has private prefix → 除去
        let data = b"\x1b[?1;1$y";
        assert!(filter_terminal_responses(data).is_empty());
    }

    #[test]
    fn filter_generic_private_prefix_csi() {
        // ESC [ ? 2 0 0 4 h — DECSET (bracketed paste mode report) → 除去
        let data = b"\x1b[?2004h";
        assert!(filter_terminal_responses(data).is_empty());
    }

    #[test]
    fn filter_dcs_xtversion() {
        // DCS >|version ST → XTVERSION 応答 → 除去
        let data = b"\x1bP>|xterm(388)\x1b\\";
        assert!(filter_terminal_responses(data).is_empty());
    }

    #[test]
    fn filter_dcs_decrqss() {
        // DCS 1 $ r ... ST → DECRQSS 応答 → 除去
        let data = b"\x1bP1$r0m\x1b\\";
        assert!(filter_terminal_responses(data).is_empty());
    }

    #[test]
    fn filter_osc_bel_terminated() {
        // OSC 10;rgb:ff/ff/ff BEL → 色クエリ応答 → 除去
        let data = b"\x1b]10;rgb:ff/ff/ff\x07";
        assert!(filter_terminal_responses(data).is_empty());
    }

    #[test]
    fn filter_osc_st_terminated() {
        // OSC 11;rgb:00/00/00 ST → 除去
        let data = b"\x1b]11;rgb:00/00/00\x1b\\";
        assert!(filter_terminal_responses(data).is_empty());
    }

    #[test]
    fn mixed_da_decrqm_cpr_dcs() {
        // DA + DECRQM + CPR + DCS → CPR のみ残る
        let data = b"\x1b[?1;2c\x1b[?1;1$y\x1b[24;80R\x1bP>|term\x1b\\";
        assert_eq!(filter_terminal_responses(data), &b"\x1b[24;80R"[..]);
    }

    #[test]
    fn keep_incomplete_csi() {
        // ESC [ 1 (no final byte) → keep
        let data = b"\x1b[1";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn keep_unterminated_dcs() {
        // ESC P ... (no ST) → keep as-is to avoid losing input on chunk split
        let data = b"\x1bPsome data without terminator";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn keep_unterminated_osc() {
        // ESC ] ... (no BEL/ST) → keep as-is
        let data = b"\x1b]10;rgb:ff/ff/ff";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn keep_sgr_mouse_report() {
        // ESC [ < 0 ; 35 ; 5 M → SGR mouse press → keep
        let data = b"\x1b[<0;35;5M";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn keep_sgr_mouse_release() {
        // ESC [ < 0 ; 35 ; 5 m → SGR mouse release → keep
        let data = b"\x1b[<0;35;5m";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn keep_trailing_esc() {
        // text + trailing ESC → keep all
        let data = b"hello\x1b";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn keep_ss3_sequences() {
        // ESC O P → SS3 F1 key → keep
        let data = b"\x1bOP";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn filter_dcs_with_text_around() {
        // text + DCS + text → DCS のみ除去
        let data = b"before\x1bP>|ver\x1b\\after";
        assert_eq!(filter_terminal_responses(data), &b"beforeafter"[..]);
    }

    #[test]
    fn key_identity_empty() {
        assert_eq!(key_identity(""), " ");
    }

    #[test]
    fn load_authorized_keys_missing_file() {
        let keys = load_authorized_keys("/nonexistent/path");
        assert!(keys.is_empty());
    }

    #[test]
    fn load_authorized_keys_with_comments() {
        let dir = tempfile::tempdir().unwrap();
        let ssh_dir = dir.path().join("ssh");
        std::fs::create_dir_all(&ssh_dir).unwrap();
        std::fs::write(
            ssh_dir.join("authorized_keys"),
            "# comment\nssh-ed25519 AAAAB3NzaKey1 user@host\n\nssh-rsa AAAAB3NzaKey2 other\n",
        )
        .unwrap();
        let keys = load_authorized_keys(dir.path().to_str().unwrap());
        assert_eq!(keys.len(), 2);
        assert!(keys.contains("ssh-ed25519 AAAAB3NzaKey1"));
        assert!(keys.contains("ssh-rsa AAAAB3NzaKey2"));
    }

    // ── Escape state machine tests ──────────────────────────────────

    #[test]
    fn escape_plain_text_passthrough() {
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"hello world");
        assert_eq!(fwd, b"hello world");
        assert!(cmds.is_empty());
    }

    #[test]
    fn escape_tilde_s_after_cr() {
        // CR → ~ → s triggers ShowStatus
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"\r~s");
        assert_eq!(fwd, b"\r");
        assert_eq!(cmds, vec![EscapeCommand::ShowStatus]);
    }

    #[test]
    fn escape_tilde_s_after_lf() {
        // LF → ~ → s triggers ShowStatus
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"\n~s");
        assert_eq!(fwd, b"\n");
        assert_eq!(cmds, vec![EscapeCommand::ShowStatus]);
    }

    #[test]
    fn escape_tilde_question_after_newline() {
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"\r~?");
        assert_eq!(fwd, b"\r");
        assert_eq!(cmds, vec![EscapeCommand::ShowHelp]);
    }

    #[test]
    fn escape_double_tilde_sends_literal() {
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"\r~~");
        assert_eq!(fwd, b"\r~");
        assert!(cmds.is_empty());
    }

    #[test]
    fn escape_tilde_unknown_forwards_both() {
        // ~ + unknown char → forward both
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"\r~x");
        assert_eq!(fwd, b"\r~x");
        assert!(cmds.is_empty());
    }

    #[test]
    fn escape_tilde_without_newline_is_literal() {
        // In Normal state, ~ is just a regular character
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"a~s");
        assert_eq!(fwd, b"a~s");
        assert!(cmds.is_empty());
    }

    #[test]
    fn escape_initial_after_newline_state() {
        // start_bridge sets AfterNewline — can ~s immediately
        let mut state = EscapeState::AfterNewline;
        let (fwd, cmds) = process_escape_input(&mut state, b"~s");
        assert!(fwd.is_empty());
        assert_eq!(cmds, vec![EscapeCommand::ShowStatus]);
    }

    #[test]
    fn escape_multiple_newlines_before_tilde() {
        // Multiple newlines keep AfterNewline state
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"\r\n\r\n~s");
        assert_eq!(fwd, b"\r\n\r\n");
        assert_eq!(cmds, vec![EscapeCommand::ShowStatus]);
    }

    #[test]
    fn escape_tilde_then_newline_resets() {
        // ~<newline> → forwards both, and newline sets AfterNewline
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"\r~\r");
        assert_eq!(fwd, b"\r~\r");
        assert!(cmds.is_empty());
        // State should be AfterNewline now (the second \r)
        assert!(matches!(state, EscapeState::AfterNewline));
    }

    #[test]
    fn escape_multiple_commands_in_one_chunk() {
        // Two status requests in one data chunk
        let mut state = EscapeState::Normal;
        let (fwd, cmds) = process_escape_input(&mut state, b"\r~s\r~?");
        assert_eq!(fwd, b"\r\r");
        assert_eq!(
            cmds,
            vec![EscapeCommand::ShowStatus, EscapeCommand::ShowHelp]
        );
    }

    #[test]
    fn escape_across_chunks() {
        // State persists across calls: first chunk ends with \r
        let mut state = EscapeState::Normal;
        let (fwd1, cmds1) = process_escape_input(&mut state, b"hello\r");
        assert_eq!(fwd1, b"hello\r");
        assert!(cmds1.is_empty());
        assert!(matches!(state, EscapeState::AfterNewline));

        // Second chunk starts with ~s
        let (fwd2, cmds2) = process_escape_input(&mut state, b"~s");
        assert!(fwd2.is_empty());
        assert_eq!(cmds2, vec![EscapeCommand::ShowStatus]);
    }

    #[test]
    fn escape_tilde_held_across_chunks() {
        // First chunk: \r~ (tilde held)
        let mut state = EscapeState::Normal;
        let (fwd1, cmds1) = process_escape_input(&mut state, b"\r~");
        assert_eq!(fwd1, b"\r");
        assert!(cmds1.is_empty());
        assert!(matches!(state, EscapeState::AfterTilde));

        // Second chunk: s (completes the escape)
        let (fwd2, cmds2) = process_escape_input(&mut state, b"s");
        assert!(fwd2.is_empty());
        assert_eq!(cmds2, vec![EscapeCommand::ShowStatus]);
    }

    #[test]
    fn escape_format_help_content() {
        let help = DenSshHandler::format_help();
        assert!(help.contains("~s"));
        assert!(help.contains("~?"));
        assert!(help.contains("~~"));
    }

    // ── replace_osc_title tests ─────────────────────────────────────

    const TEST_REPLACEMENT: &[u8] = b"\x1b]0;Den SSH [test]\x07";

    #[test]
    fn osc_title_replace_osc0_bel() {
        // OSC 0 with BEL terminator → replaced
        let data = b"\x1b]0;PowerShell\x07";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert_eq!(&result[..], TEST_REPLACEMENT);
    }

    #[test]
    fn osc_title_replace_osc0_st() {
        // OSC 0 with ST terminator → replaced (ST input → BEL output)
        let data = b"\x1b]0;PowerShell\x1b\\";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert_eq!(&result[..], TEST_REPLACEMENT);
    }

    #[test]
    fn osc_title_replace_osc1_bel() {
        // OSC 1 (icon name) → replaced
        let data = b"\x1b]1;icon\x07";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert_eq!(&result[..], TEST_REPLACEMENT);
    }

    #[test]
    fn osc_title_replace_osc2_bel() {
        // OSC 2 (window title) → replaced
        let data = b"\x1b]2;Window Title\x07";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert_eq!(&result[..], TEST_REPLACEMENT);
    }

    #[test]
    fn osc_title_keep_other_osc() {
        // OSC 7 (current directory) → keep unchanged
        let data = b"\x1b]7;file:///home/user\x07";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert_eq!(&result[..], &data[..]);
    }

    #[test]
    fn osc_title_keep_high_osc_numbers() {
        // OSC 10 (foreground color query) → keep unchanged
        let data = b"\x1b]10;rgb:ff/ff/ff\x07";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert_eq!(&result[..], &data[..]);
    }

    #[test]
    fn osc_title_multiple_sequences() {
        // Two title sequences in one chunk → both replaced
        let data = b"\x1b]0;Title1\x07some text\x1b]2;Title2\x07";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        let mut expected = Vec::new();
        expected.extend_from_slice(TEST_REPLACEMENT);
        expected.extend_from_slice(b"some text");
        expected.extend_from_slice(TEST_REPLACEMENT);
        assert_eq!(&result[..], &expected[..]);
    }

    #[test]
    fn osc_title_unterminated_passthrough() {
        // Unterminated OSC → pass through unchanged (chunk boundary)
        let data = b"\x1b]0;partial title";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert_eq!(&result[..], &data[..]);
    }

    #[test]
    fn osc_title_plain_text_fast_path() {
        // No ESC at all → Cow::Borrowed
        let data = b"hello world";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&result[..], &data[..]);
    }

    #[test]
    fn osc_title_esc_but_no_title_osc() {
        // ESC present but not a title OSC → Cow::Borrowed
        let data = b"\x1b[?25h";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&result[..], &data[..]);
    }

    #[test]
    fn osc_title_mixed_with_text() {
        // Text + OSC 0 + text → only OSC replaced, text preserved
        let data = b"before\x1b]0;PS\x07after";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"before");
        expected.extend_from_slice(TEST_REPLACEMENT);
        expected.extend_from_slice(b"after");
        assert_eq!(&result[..], &expected[..]);
    }

    #[test]
    fn osc_title_empty_title_bel() {
        // OSC 0 with empty title (immediately terminated) → replaced
        let data = b"\x1b]0;\x07";
        let result = replace_osc_title(data, TEST_REPLACEMENT);
        assert_eq!(&result[..], TEST_REPLACEMENT);
    }
}
