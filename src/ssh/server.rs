use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use russh::keys::ssh_key;
use russh::server::{Auth, Handler, Msg, Server as _, Session};
use russh::{ChannelId, CryptoVec, Pty};
use tokio::net::TcpListener;

use crate::pty::registry::{ClientKind, SessionRegistry};

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
) -> anyhow::Result<()> {
    // ホストキー読み込み/生成
    let host_key = super::keys::load_or_generate_host_key(std::path::Path::new(&data_dir))?;

    let authorized_keys: Arc<HashSet<String>> = Arc::new(load_authorized_keys(&data_dir));

    // auth_rejection_time を 0 にして、パスワード認証のみハンドラ側で遅延させる。
    // これにより公開鍵認証の拒否が即座に完了し、クライアントがパスワード認証に
    // 素早くフォールバックできる。
    let config = russh::server::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
        auth_rejection_time: std::time::Duration::from_secs(0),
        auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
        keys: vec![host_key],
        ..Default::default()
    };
    let config = Arc::new(config);

    let mut server = DenSshServer {
        registry,
        password,
        authorized_keys,
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
}

impl russh::server::Server for DenSshServer {
    type Handler = DenSshHandler;

    fn new_client(&mut self, addr: Option<std::net::SocketAddr>) -> DenSshHandler {
        tracing::info!("SSH client connected from {:?}", addr);
        DenSshHandler {
            registry: Arc::clone(&self.registry),
            password: self.password.clone(),
            authorized_keys: Arc::clone(&self.authorized_keys),
            session_name: None,
            client_id: None,
            channel_id: None,
            output_task: None,
            pty_cols: 80,
            pty_rows: 24,
            pty_requested: false,
        }
    }
}

struct DenSshHandler {
    registry: Arc<SessionRegistry>,
    password: String,
    authorized_keys: Arc<HashSet<String>>,
    // Per-connection state
    session_name: Option<String>,
    client_id: Option<u64>,
    channel_id: Option<ChannelId>,
    output_task: Option<tokio::task::JoinHandle<()>>,
    pty_cols: u16,
    pty_rows: u16,
    pty_requested: bool,
}

impl DenSshHandler {
    /// セッションに attach して I/O ブリッジを開始
    async fn start_bridge(
        &mut self,
        session_name: &str,
        session: &mut Session,
    ) -> Result<(), anyhow::Error> {
        let cols = self.pty_cols;
        let rows = self.pty_rows;

        let (shared_session, mut output_rx, replay, client_id) = self
            .registry
            .get_or_create(session_name, ClientKind::Ssh, cols, rows)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        self.session_name = Some(session_name.to_string());
        self.client_id = Some(client_id);

        let channel_id = self
            .channel_id
            .ok_or_else(|| anyhow::anyhow!("No channel"))?;

        // replay data を送信（SSH 非互換モードを除去）
        tracing::debug!(
            "SSH start_bridge: session={session_name}, replay={} bytes",
            replay.len()
        );
        if !replay.is_empty() {
            let filtered_replay = filter_output_for_ssh(&replay);
            if !filtered_replay.is_empty() {
                session.data(channel_id, CryptoVec::from_slice(&filtered_replay))?;
            }
        }

        // Output: broadcast::Receiver → SSH channel
        let handle = session.handle();
        let name_for_task = session_name.to_string();
        let registry = Arc::clone(&self.registry);
        let _shared_session = shared_session; // keep alive reference for output task duration

        self.output_task = Some(tokio::spawn(async move {
            loop {
                // recv with timeout: ConPTY は子プロセス終了後も reader を
                // ブロックし続けるため、定期的に alive を確認する
                match tokio::time::timeout(std::time::Duration::from_secs(1), output_rx.recv())
                    .await
                {
                    Ok(Ok(data)) => {
                        let filtered = filter_output_for_ssh(&data);
                        if filtered.is_empty() {
                            continue;
                        }
                        if handle
                            .data(channel_id, CryptoVec::from_slice(&filtered))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                        tracing::warn!("SSH client lagged {n} messages on {name_for_task}");
                    }
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                        let _ = handle.exit_status_request(channel_id, 0).await;
                        let _ = handle.eof(channel_id).await;
                        let _ = handle.close(channel_id).await;
                        break;
                    }
                    Err(_) => {
                        if !_shared_session.is_alive() {
                            let _ = handle.exit_status_request(channel_id, 0).await;
                            let _ = handle.eof(channel_id).await;
                            let _ = handle.close(channel_id).await;
                            break;
                        }
                    }
                }
            }

            // セッションが死んでいたら registry から削除
            registry.remove_dead(&name_for_task).await;
        }));

        Ok(())
    }

    /// cleanup: detach + output_task abort
    async fn cleanup(&mut self) {
        if let (Some(name), Some(client_id)) = (self.session_name.take(), self.client_id.take()) {
            self.registry.detach(&name, client_id).await;
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
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
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
                // セッション一覧をテキストで返す
                session.channel_success(channel)?;
                let sessions = self.registry.list().await;
                let mut output = String::new();
                if sessions.is_empty() {
                    output.push_str("No active sessions\r\n");
                } else {
                    output.push_str("Sessions:\r\n");
                    for s in &sessions {
                        let status = if s.alive { "alive" } else { "dead" };
                        output.push_str(&format!(
                            "  {} ({}, {} clients)\r\n",
                            s.name, status, s.client_count
                        ));
                    }
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
                self.start_bridge(name, session).await?;
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
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // stdin データ → PTY writer（ターミナル応答シーケンスを除去）
        if let Some(ref name) = self.session_name
            && let Some(session) = self.registry.get(name).await
        {
            let filtered = filter_terminal_responses(data);
            if data.len() != filtered.len() {
                tracing::debug!(
                    "SSH data: {} bytes in, {} bytes after filter (session {name})",
                    data.len(),
                    filtered.len(),
                );
            }
            if !filtered.is_empty() {
                let _ = session.write_input(&filtered).await;
            }
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

        if let (Some(name), Some(client_id)) = (&self.session_name, self.client_id)
            && let Some(session) = self.registry.get(name).await
        {
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
        // Drop 時に cleanup できない（async）のでタスクを spawn
        let session_name = self.session_name.take();
        let client_id = self.client_id.take();
        let registry = Arc::clone(&self.registry);

        if let (Some(name), Some(id)) = (session_name, client_id) {
            tokio::spawn(async move {
                registry.detach(&name, id).await;
            });
        }

        if let Some(task) = self.output_task.take() {
            task.abort();
        }
    }
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

    // フィルタで何も除去されなかった場合は元データを返す
    if result.len() == data.len() {
        Cow::Borrowed(data)
    } else {
        Cow::Owned(result)
    }
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

/// タイミング攻撃防止用の定数時間文字列比較
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq("password123", "password123"));
    }

    #[test]
    fn constant_time_eq_different() {
        assert!(!constant_time_eq("password123", "password456"));
    }

    #[test]
    fn constant_time_eq_different_length() {
        assert!(!constant_time_eq("short", "longer-string"));
    }

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
}
