use std::sync::Arc;

use russh::server::{Auth, Handler, Msg, Server as _, Session};
use russh::{ChannelId, CryptoVec, Pty};
use tokio::net::TcpListener;

use crate::pty::registry::{ClientKind, SessionRegistry};

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

    let config = russh::server::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
        auth_rejection_time: std::time::Duration::from_secs(3),
        auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
        keys: vec![host_key],
        ..Default::default()
    };
    let config = Arc::new(config);

    let mut server = DenSshServer { registry, password };

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
}

impl russh::server::Server for DenSshServer {
    type Handler = DenSshHandler;

    fn new_client(&mut self, addr: Option<std::net::SocketAddr>) -> DenSshHandler {
        tracing::info!("SSH client connected from {:?}", addr);
        DenSshHandler {
            registry: Arc::clone(&self.registry),
            password: self.password.clone(),
            session_name: None,
            client_id: None,
            channel_id: None,
            output_task: None,
            pty_cols: 80,
            pty_rows: 24,
        }
    }
}

struct DenSshHandler {
    registry: Arc<SessionRegistry>,
    password: String,
    // Per-connection state
    session_name: Option<String>,
    client_id: Option<u64>,
    channel_id: Option<ChannelId>,
    output_task: Option<tokio::task::JoinHandle<()>>,
    pty_cols: u16,
    pty_rows: u16,
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

        // replay data を送信
        if !replay.is_empty() {
            session.data(channel_id, CryptoVec::from_slice(&replay))?;
        }

        // Output: broadcast::Receiver → SSH channel
        let handle = session.handle();
        let name_for_task = session_name.to_string();
        let registry = Arc::clone(&self.registry);
        let _shared_session = shared_session; // keep alive reference for output task duration

        self.output_task = Some(tokio::spawn(async move {
            loop {
                match output_rx.recv().await {
                    Ok(data) => {
                        if handle
                            .data(channel_id, CryptoVec::from_slice(&data))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("SSH client lagged {n} messages on session {name_for_task}");
                        // continue receiving
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // セッション終了 → EOF + close
                        let _ = handle.eof(channel_id).await;
                        let _ = handle.close(channel_id).await;
                        break;
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

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        if constant_time_eq(password, &self.password) {
            tracing::info!("SSH auth: password accepted");
            Ok(Auth::Accept)
        } else {
            tracing::warn!("SSH auth: password rejected");
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

/// SSH クライアントのターミナルが返す応答シーケンスをフィルタする。
///
/// ConPTY は初期化時にクエリを送信し、ターミナルが応答を返す。
/// CPR (Cursor Position Report: `ESC[n;mR`) は ConPTY が必要とするので通過させるが、
/// DA (Device Attributes: `ESC[?...c` / `ESC[>...c`) はシェルに生入力として渡されて
/// 文字化けを起こすため除去する。
fn filter_terminal_responses(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;

    while i < data.len() {
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'[' {
            // CSI sequence: ESC [
            let start = i;
            i += 2;

            // Optional private prefix: ? > =
            let has_private_prefix =
                i < data.len() && (data[i] == b'?' || data[i] == b'>' || data[i] == b'=');
            if has_private_prefix {
                i += 1;
            }

            // Parameters: digits and semicolons
            while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
                i += 1;
            }

            // Final byte
            if i < data.len() && data[i].is_ascii_alphabetic() {
                let final_byte = data[i];
                i += 1;

                // DA 応答のみ除去（CPR は ConPTY が必要とするので通過させる）
                let is_da_response = final_byte == b'c' && has_private_prefix;

                if is_da_response {
                    continue;
                }

                result.extend_from_slice(&data[start..i]);
            } else {
                // 不完全なシーケンス → 保持
                result.extend_from_slice(&data[start..i]);
            }
        } else {
            result.push(data[i]);
            i += 1;
        }
    }

    result
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
        assert_eq!(filter_terminal_responses(data), data.to_vec());
    }

    #[test]
    fn keep_cpr_large_numbers() {
        let data = b"\x1b[24;80R";
        assert_eq!(filter_terminal_responses(data), data.to_vec());
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
        assert_eq!(filter_terminal_responses(data), data.to_vec());
    }

    #[test]
    fn keep_function_keys() {
        // ESC [ 1 5 ~ → F5 → 保持
        let data = b"\x1b[15~";
        assert_eq!(filter_terminal_responses(data), data.to_vec());
    }

    #[test]
    fn keep_plain_text() {
        let data = b"hello world";
        assert_eq!(filter_terminal_responses(data), data.to_vec());
    }

    #[test]
    fn filter_da_mixed_input() {
        // DA1 + 通常テキスト → DA のみ除去
        let data = b"\x1b[?1;2chello";
        assert_eq!(filter_terminal_responses(data), b"hello".to_vec());
    }

    #[test]
    fn keep_cpr_filter_da() {
        // CPR + DA1 → CPR は保持、DA は除去
        let data = b"\x1b[24;80R\x1b[?1;2c";
        assert_eq!(filter_terminal_responses(data), b"\x1b[24;80R".to_vec());
    }

    #[test]
    fn keep_text_between_responses() {
        // テキスト + DA → テキスト保持
        let data = b"abc\x1b[?1;2c";
        assert_eq!(filter_terminal_responses(data), b"abc".to_vec());
    }
}
