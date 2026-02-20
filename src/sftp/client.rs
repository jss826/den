use russh::keys::ssh_key;
use russh_sftp::client::SftpSession;
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};

// --- エラー型 ---

#[derive(Debug)]
pub enum SftpError {
    NotConnected,
    AuthFailed,
    Ssh(russh::Error),
    Sftp(russh_sftp::client::error::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for SftpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SftpError::NotConnected => write!(f, "Not connected"),
            SftpError::AuthFailed => write!(f, "Authentication failed"),
            SftpError::Ssh(e) => write!(f, "SSH error: {}", e),
            SftpError::Sftp(e) => write!(f, "SFTP error: {}", e),
            SftpError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl From<russh::Error> for SftpError {
    fn from(e: russh::Error) -> Self {
        SftpError::Ssh(e)
    }
}

impl From<russh_sftp::client::error::Error> for SftpError {
    fn from(e: russh_sftp::client::error::Error) -> Self {
        SftpError::Sftp(e)
    }
}

impl From<std::io::Error> for SftpError {
    fn from(e: std::io::Error) -> Self {
        SftpError::Io(e)
    }
}

// --- 認証方式 ---

pub enum SftpAuth {
    Password(String),
    KeyFile(String),
}

// --- SSH クライアントハンドラ ---

struct SftpClientHandler;

impl russh::client::Handler for SftpClientHandler {
    type Error = anyhow::Error;

    // v1: 全ホストキーを受け入れ（known_hosts 検証は v2 で対応）
    async fn check_server_key(
        &mut self,
        _server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

// --- 接続状態 ---

pub struct SftpConnection {
    pub sftp: SftpSession,
    handle: russh::client::Handle<SftpClientHandler>,
    pub host: String,
    pub port: u16,
    pub username: String,
}

// --- SftpManager ---

#[derive(Clone)]
pub struct SftpManager {
    conn: Arc<Mutex<Option<SftpConnection>>>,
}

pub struct SftpStatus {
    pub connected: bool,
    pub host: Option<String>,
    pub username: Option<String>,
}

impl Default for SftpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SftpManager {
    pub fn new() -> Self {
        SftpManager {
            conn: Arc::new(Mutex::new(None)),
        }
    }

    /// リモートホストに SSH + SFTP 接続
    pub async fn connect(
        &self,
        host: &str,
        port: u16,
        username: &str,
        auth: SftpAuth,
    ) -> Result<(), SftpError> {
        // 既存接続があれば切断
        self.disconnect().await;

        let config = russh::client::Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(300)),
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            keepalive_max: 5,
            ..Default::default()
        };

        let mut session = russh::client::connect(Arc::new(config), (host, port), SftpClientHandler)
            .await
            .map_err(|e| SftpError::Ssh(russh::Error::IO(std::io::Error::other(e.to_string()))))?;

        // 認証
        let auth_result = match &auth {
            SftpAuth::Password(password) => {
                session.authenticate_password(username, password).await?
            }
            SftpAuth::KeyFile(key_path) => {
                let key_data = tokio::fs::read_to_string(key_path).await?;
                let key_pair = russh::keys::decode_secret_key(&key_data, None).map_err(|e| {
                    SftpError::Io(std::io::Error::other(format!("Invalid key: {e}")))
                })?;
                let key_with_alg = russh::keys::PrivateKeyWithHashAlg::new(
                    Arc::new(key_pair),
                    None, // デフォルトのハッシュアルゴリズム
                );
                session
                    .authenticate_publickey(username, key_with_alg)
                    .await?
            }
        };

        if !auth_result.success() {
            let _ = session
                .disconnect(russh::Disconnect::ByApplication, "", "")
                .await;
            return Err(SftpError::AuthFailed);
        }

        // SFTP サブシステムを開く
        let channel = session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let sftp = SftpSession::new(channel.into_stream()).await?;

        let connection = SftpConnection {
            sftp,
            handle: session,
            host: host.to_string(),
            port,
            username: username.to_string(),
        };

        *self.conn.lock().await = Some(connection);
        tracing::info!("sftp: connected to {}@{}:{}", username, host, port);
        Ok(())
    }

    /// 切断
    pub async fn disconnect(&self) {
        let mut guard = self.conn.lock().await;
        if let Some(conn) = guard.take() {
            let _ = conn.sftp.close().await;
            let _ = conn
                .handle
                .disconnect(russh::Disconnect::ByApplication, "", "")
                .await;
            tracing::info!(
                "sftp: disconnected from {}@{}:{}",
                conn.username,
                conn.host,
                conn.port
            );
        }
    }

    /// 接続状態を返す
    pub async fn status(&self) -> SftpStatus {
        let guard = self.conn.lock().await;
        match guard.as_ref() {
            Some(conn) => SftpStatus {
                connected: true,
                host: Some(format!("{}:{}", conn.host, conn.port)),
                username: Some(conn.username.clone()),
            },
            None => SftpStatus {
                connected: false,
                host: None,
                username: None,
            },
        }
    }

    /// Mutex ガードを取得。未接続なら NotConnected エラー。
    /// ガード保持中は他の SFTP 操作はブロックされる（単一ユーザーなので許容）。
    pub async fn get(&self) -> Result<SftpGuard<'_>, SftpError> {
        let guard = self.conn.lock().await;
        if guard.is_none() {
            return Err(SftpError::NotConnected);
        }
        Ok(SftpGuard { guard })
    }
}

/// SFTP セッションへのアクセスを提供するガード型
pub struct SftpGuard<'a> {
    guard: MutexGuard<'a, Option<SftpConnection>>,
}

impl SftpGuard<'_> {
    pub fn sftp(&self) -> &SftpSession {
        // get() で None チェック済み
        &self.guard.as_ref().unwrap().sftp
    }
}
