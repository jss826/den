use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use portable_pty::PtySize;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock, broadcast};

use super::manager::PtyManager;
use super::ring_buffer::RingBuffer;

/// SessionRegistry の操作エラー
#[derive(Debug)]
pub enum RegistryError {
    /// セッション名が不正
    InvalidName(String),
    /// セッションが既に存在する
    AlreadyExists(String),
    /// セッションが見つからない
    NotFound(String),
    /// セッションが終了済み
    SessionDead(String),
    /// PTY spawn 失敗
    SpawnFailed(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(f, "Invalid session name: {name}"),
            Self::AlreadyExists(name) => write!(f, "Session already exists: {name}"),
            Self::NotFound(name) => write!(f, "Session not found: {name}"),
            Self::SessionDead(name) => write!(f, "Session is dead: {name}"),
            Self::SpawnFailed(msg) => write!(f, "Spawn failed: {msg}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// リプレイバッファ容量: 64KB
const REPLAY_CAPACITY: usize = 64 * 1024;

/// broadcast チャネル容量
const BROADCAST_CAPACITY: usize = 256;

/// クライアント ID 生成用グローバルカウンター
static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

/// グローバルセッション管理
pub struct SessionRegistry {
    sessions: RwLock<HashMap<String, Arc<SharedSession>>>,
    shell: String,
}

/// 1 つの名前付き PTY セッション
pub struct SharedSession {
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// PTY プロセスが生存しているか（AtomicBool: read_task から常に設定可能）
    alive: AtomicBool,
    /// リプレイバッファ（std::sync::Mutex: blocking context から常にアクセス可能）
    replay_buf: std::sync::Mutex<RingBuffer>,
    /// broadcast 送信側（read_task 終了時に drop してチャネルを閉じる）
    output_tx: std::sync::Mutex<Option<broadcast::Sender<Vec<u8>>>>,
    /// PTY 内部状態（pty_writer, clients, child 等）
    pub inner: Mutex<SessionInner>,
}

pub struct SessionInner {
    // PTY
    pub pty_writer: Box<dyn std::io::Write + Send>,
    resize_tx: std::sync::mpsc::Sender<(u16, u16)>,
    // Clients
    clients: Vec<ClientInfo>,
    // Resources
    #[cfg(windows)]
    pub job: Option<super::job::PtyJobObject>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}

#[derive(Debug)]
pub struct ClientInfo {
    pub id: u64,
    pub kind: ClientKind,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    WebSocket,
    Ssh,
}

/// UI/API 向けセッション情報
#[derive(Serialize)]
pub struct SessionInfo {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub alive: bool,
    pub client_count: usize,
}

/// セッション名バリデーション: 英数字 + ハイフンのみ、最大 64 文字
fn is_valid_session_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl SessionRegistry {
    pub fn new(shell: String) -> Arc<Self> {
        Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            shell,
        })
    }

    /// PTY を spawn し read_task/resize_task を起動する共通ヘルパー
    fn setup_pty_session(
        name: &str,
        pty_reader: Box<dyn std::io::Read + Send>,
        pty_writer: Box<dyn std::io::Write + Send>,
        master: Box<dyn portable_pty::MasterPty + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        #[cfg(windows)] job: Option<super::job::PtyJobObject>,
    ) -> Arc<SharedSession> {
        let (output_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (resize_tx, resize_rx) = std::sync::mpsc::channel::<(u16, u16)>();

        let session = Arc::new(SharedSession {
            name: name.to_string(),
            created_at: Utc::now(),
            alive: AtomicBool::new(true),
            replay_buf: std::sync::Mutex::new(RingBuffer::new(REPLAY_CAPACITY)),
            output_tx: std::sync::Mutex::new(Some(output_tx.clone())),
            inner: Mutex::new(SessionInner {
                pty_writer,
                resize_tx,
                clients: Vec::new(),
                #[cfg(windows)]
                job,
                child: Some(child),
            }),
        });

        // resize task: blocking スレッドで master.resize()
        tokio::task::spawn_blocking(move || {
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

        // PTY read_task: 出力を replay buffer + broadcast に流す
        let session_for_read = Arc::clone(&session);
        let broadcast_tx = output_tx;

        tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            let mut reader = pty_reader;
            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = buf[..n].to_vec();

                        // replay buffer: std::sync::Mutex なので常に取得可能
                        if let Ok(mut rb) = session_for_read.replay_buf.lock() {
                            rb.write(&data);
                        }

                        // broadcast（receiver がいなくても OK）
                        let _ = broadcast_tx.send(data);
                    }
                    Err(_) => break,
                }
            }

            // EOF: AtomicBool なので常に設定可能
            session_for_read.alive.store(false, Ordering::Release);

            // broadcast sender を drop してチャネルを閉じる
            // → 全 receiver に RecvError::Closed が通知される
            session_for_read.output_tx.lock().unwrap().take();
            drop(broadcast_tx);
        });

        session
    }

    /// セッション作成（デフォルトシェル）
    pub async fn create(
        &self,
        name: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<SharedSession>, RegistryError> {
        if !is_valid_session_name(name) {
            return Err(RegistryError::InvalidName(name.to_string()));
        }

        // 高速チェック（最適化: 大半のケースで不要な PTY spawn を回避）
        if self.sessions.read().await.contains_key(name) {
            return Err(RegistryError::AlreadyExists(name.to_string()));
        }

        // PTY を spawn（blocking）
        let pty = tokio::task::spawn_blocking({
            let shell = self.shell.clone();
            move || PtyManager::spawn(&shell, cols, rows)
        })
        .await
        .map_err(|e| RegistryError::SpawnFailed(e.to_string()))?
        .map_err(|e| RegistryError::SpawnFailed(e.to_string()))?;

        let session = Self::setup_pty_session(
            name,
            pty.reader,
            pty.writer,
            pty.master,
            pty.child,
            #[cfg(windows)]
            pty.job,
        );

        // 権威的な挿入: write lock で再チェック（TOCTOU 防止）
        {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(name) {
                // レース: 別の呼び出しが先に作成した → クリーンアップ
                session.alive.store(false, Ordering::Release);
                if let Some(mut child) = session.inner.lock().await.child.take() {
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = child.kill();
                        let _ = child.wait();
                    })
                    .await;
                }
                return Err(RegistryError::AlreadyExists(name.to_string()));
            }
            sessions.insert(name.to_string(), Arc::clone(&session));
        }

        tracing::info!("Session created: {name}");
        Ok(session)
    }

    /// カスタムコマンドでセッション作成（Claude CLI 等）
    pub async fn create_with_pty(
        &self,
        name: &str,
        pty: super::manager::PtySession,
    ) -> Result<Arc<SharedSession>, RegistryError> {
        if !is_valid_session_name(name) {
            return Err(RegistryError::InvalidName(name.to_string()));
        }

        // 高速チェック
        if self.sessions.read().await.contains_key(name) {
            return Err(RegistryError::AlreadyExists(name.to_string()));
        }

        let session = Self::setup_pty_session(
            name,
            pty.reader,
            pty.writer,
            pty.master,
            pty.child,
            #[cfg(windows)]
            pty.job,
        );

        // 権威的な挿入（TOCTOU 防止）
        {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(name) {
                session.alive.store(false, Ordering::Release);
                if let Some(mut child) = session.inner.lock().await.child.take() {
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = child.kill();
                        let _ = child.wait();
                    })
                    .await;
                }
                return Err(RegistryError::AlreadyExists(name.to_string()));
            }
            sessions.insert(name.to_string(), Arc::clone(&session));
        }

        tracing::info!("Session created (custom PTY): {name}");
        Ok(session)
    }

    /// 既存セッションに attach（クライアント追加 + broadcast::Receiver + replay data）
    pub async fn attach(
        &self,
        name: &str,
        kind: ClientKind,
        cols: u16,
        rows: u16,
    ) -> Result<
        (
            Arc<SharedSession>,
            broadcast::Receiver<Vec<u8>>,
            Vec<u8>,
            u64,
        ),
        RegistryError,
    > {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(name)
            .ok_or_else(|| RegistryError::NotFound(name.to_string()))?;

        let session = Arc::clone(session);
        drop(sessions); // RwLock 解放してから Mutex 取得

        if !session.is_alive() {
            return Err(RegistryError::SessionDead(name.to_string()));
        }

        let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
        let mut inner = session.inner.lock().await;
        inner.clients.push(ClientInfo {
            id: client_id,
            kind,
            cols,
            rows,
        });

        let rx = session.subscribe();
        let replay = session.replay_buf.lock().unwrap().read_all();

        // リサイズ再計算
        Self::recalculate_size(&mut inner);

        drop(inner);

        tracing::info!("Client {client_id} ({kind:?}) attached to session {name}");
        Ok((session, rx, replay, client_id))
    }

    /// 既存セッションに attach。なければ create して attach
    pub async fn get_or_create(
        &self,
        name: &str,
        kind: ClientKind,
        cols: u16,
        rows: u16,
    ) -> Result<
        (
            Arc<SharedSession>,
            broadcast::Receiver<Vec<u8>>,
            Vec<u8>,
            u64,
        ),
        RegistryError,
    > {
        // まず attach 試行
        match self.attach(name, kind, cols, rows).await {
            Ok(result) => return Ok(result),
            Err(RegistryError::NotFound(_)) => {
                // セッションが存在しない → 作成を試みる
            }
            Err(RegistryError::SessionDead(_)) => {
                // dead セッション → クリーンアップしてから再作成
                self.destroy(name).await;
            }
            Err(e) => return Err(e),
        }

        // create → inline attach
        match self.create(name, cols, rows).await {
            Ok(session) => {
                let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
                let mut inner = session.inner.lock().await;
                inner.clients.push(ClientInfo {
                    id: client_id,
                    kind,
                    cols,
                    rows,
                });

                let rx = session.subscribe();
                let replay = session.replay_buf.lock().unwrap().read_all();

                // ConPTY は同一サイズの resize を無視するため、
                // 異なるサイズで一度 resize してから正しいサイズに戻す。
                // これにより ConPTY の画面バッファが再描画され、初期出力がフラッシュされる。
                let nudge_cols = if cols > 1 { cols - 1 } else { cols + 1 };
                let _ = inner.resize_tx.send((nudge_cols, rows));
                let _ = inner.resize_tx.send((cols, rows));

                tracing::info!("Client {client_id} ({kind:?}) created+attached to session {name}");
                Ok((Arc::clone(&session), rx, replay, client_id))
            }
            Err(RegistryError::AlreadyExists(_)) => {
                // レース: attach と create の間に別クライアントが作成した → retry attach
                self.attach(name, kind, cols, rows).await
            }
            Err(e) => Err(e),
        }
    }

    /// クライアント切断
    pub async fn detach(&self, name: &str, client_id: u64) {
        let sessions = self.sessions.read().await;
        let Some(session) = sessions.get(name) else {
            return;
        };
        let session = Arc::clone(session);
        drop(sessions);

        let mut inner = session.inner.lock().await;
        inner.clients.retain(|c| c.id != client_id);

        // リサイズ再計算（クライアントが残っている場合のみ）
        if !inner.clients.is_empty() {
            Self::recalculate_size(&mut inner);
        }

        tracing::info!(
            "Client {client_id} detached from session {name} ({} remaining)",
            inner.clients.len()
        );
    }

    /// セッション一覧
    pub async fn list(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;
        let mut result = Vec::with_capacity(sessions.len());

        for (_, session) in sessions.iter() {
            let inner = session.inner.lock().await;
            result.push(SessionInfo {
                name: session.name.clone(),
                created_at: session.created_at,
                alive: session.is_alive(),
                client_count: inner.clients.len(),
            });
        }

        result.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        result
    }

    /// セッション破棄
    pub async fn destroy(&self, name: &str) {
        let session = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(name)
        };

        let Some(session) = session else {
            return;
        };

        session.alive.store(false, Ordering::Release);

        // inner を lock して child を取り出す（lock は必ず解放される）
        let child = {
            let mut inner = session.inner.lock().await;

            #[cfg(windows)]
            if let Some(ref job) = inner.job
                && let Err(e) = job.terminate()
            {
                tracing::warn!("Job Object terminate failed for session {name}: {e}");
            }

            inner.child.take()
            // inner (MutexGuard) はここで drop → resize_tx も drop → resize_task 停止
        };

        if let Some(mut child) = child {
            let name = name.to_string();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = child.kill() {
                    tracing::debug!("Session {name} child kill: {e}");
                }
                if let Err(e) = child.wait() {
                    tracing::warn!("Session {name} child wait: {e}");
                }
            })
            .await
            .ok();
        }

        tracing::info!("Session destroyed: {name}");
    }

    /// セッションが存在するか
    pub async fn exists(&self, name: &str) -> bool {
        self.sessions.read().await.contains_key(name)
    }

    /// セッション取得
    pub async fn get(&self, name: &str) -> Option<Arc<SharedSession>> {
        self.sessions.read().await.get(name).cloned()
    }

    /// 死んだセッションを registry から削除
    pub async fn remove_dead(&self, name: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get(name)
            && !session.is_alive()
        {
            sessions.remove(name);
        }
    }

    /// リサイズ再計算: 全 clients の min(cols), min(rows)
    fn recalculate_size(inner: &mut SessionInner) {
        if inner.clients.is_empty() {
            return;
        }

        let min_cols = inner.clients.iter().map(|c| c.cols).min().unwrap_or(80);
        let min_rows = inner.clients.iter().map(|c| c.rows).min().unwrap_or(24);

        let _ = inner.resize_tx.send((min_cols, min_rows));
    }
}

impl SharedSession {
    /// PTY への入力書き込み
    pub async fn write_input(&self, data: &[u8]) -> Result<(), String> {
        if !self.is_alive() {
            return Err("Session is dead".to_string());
        }
        let mut inner = self.inner.lock().await;
        std::io::Write::write_all(&mut inner.pty_writer, data)
            .map_err(|e| format!("Write failed: {e}"))
    }

    /// クライアントのリサイズ通知
    pub async fn resize(&self, client_id: u64, cols: u16, rows: u16) {
        let mut inner = self.inner.lock().await;
        if let Some(client) = inner.clients.iter_mut().find(|c| c.id == client_id) {
            client.cols = cols;
            client.rows = rows;
        }
        SessionRegistry::recalculate_size(&mut inner);
    }

    /// broadcast::Receiver を新たに取得
    /// セッション終了済みの場合は即座に Closed を返す receiver を返す
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        let guard = self.output_tx.lock().unwrap();
        match guard.as_ref() {
            Some(tx) => tx.subscribe(),
            None => {
                // sender は既に drop 済み → 即 Closed になる receiver を返す
                let (_, rx) = broadcast::channel::<Vec<u8>>(1);
                rx
            }
        }
    }

    /// alive 状態を取得（AtomicBool: Mutex 不要）
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_session_names() {
        assert!(is_valid_session_name("default"));
        assert!(is_valid_session_name("work-session"));
        assert!(is_valid_session_name("claude-abc123"));
        assert!(is_valid_session_name("A"));
        assert!(is_valid_session_name("123"));
    }

    #[test]
    fn invalid_session_names() {
        assert!(!is_valid_session_name(""));
        assert!(!is_valid_session_name("foo bar"));
        assert!(!is_valid_session_name("../etc/passwd"));
        assert!(!is_valid_session_name("hello/world"));
        assert!(!is_valid_session_name(&"x".repeat(65)));
    }

    #[test]
    fn session_name_max_length() {
        // Exactly 64 characters should be valid
        assert!(is_valid_session_name(&"a".repeat(64)));
        // 65 should be invalid
        assert!(!is_valid_session_name(&"a".repeat(65)));
    }

    #[test]
    fn session_name_underscore_invalid() {
        assert!(!is_valid_session_name("has_underscore"));
        assert!(!is_valid_session_name("_leading"));
    }

    #[test]
    fn session_name_special_chars_invalid() {
        assert!(!is_valid_session_name("hello@world"));
        assert!(!is_valid_session_name("test!"));
        assert!(!is_valid_session_name("name.with.dots"));
        assert!(!is_valid_session_name("tab\there"));
    }
}
