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

/// 子プロセス監視ポーリング間隔
const CHILD_MONITOR_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// タスク join タイムアウト
const TASK_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
    /// resize チャネル（destroy 時に take → channel 閉鎖 → master drop → pipe 閉鎖）
    resize_tx: Option<std::sync::mpsc::Sender<(u16, u16)>>,
    /// resize_task の JoinHandle（destroy 時に await → master drop → ConPTY 閉鎖保証）
    resize_handle: Option<tokio::task::JoinHandle<()>>,
    // Clients
    clients: Vec<ClientInfo>,
    /// 現在アクティブなクライアント ID（入力 or リサイズした最後のクライアント）
    active_client_id: Option<u64>,
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
    /// 最後にアクティブだった時刻（入力 or リサイズ時に更新）
    pub last_active: std::time::Instant,
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
    ///
    /// 戻り値の `broadcast::Receiver` は read_task 開始前に作成されるため、
    /// ConPTY の初期出力（DSR 等）を確実に捕捉する。
    fn setup_pty_session(
        name: &str,
        pty_reader: Box<dyn std::io::Read + Send>,
        pty_writer: Box<dyn std::io::Write + Send>,
        master: Box<dyn portable_pty::MasterPty + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        #[cfg(windows)] job: Option<super::job::PtyJobObject>,
    ) -> (Arc<SharedSession>, broadcast::Receiver<Vec<u8>>) {
        let (output_tx, first_rx) = broadcast::channel(BROADCAST_CAPACITY);
        let (resize_tx, resize_rx) = std::sync::mpsc::channel::<(u16, u16)>();

        // resize task: blocking スレッドで master.resize()
        // master を所有 → recv() が Err (= resize_tx drop) で終了 → master drop → ConPTY 閉鎖
        let resize_handle = tokio::task::spawn_blocking(move || {
            while let Ok((cols, rows)) = resize_rx.recv() {
                let size = PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                };
                let _ = master.resize(size);
            }
            // master はここで drop → ClosePseudoConsole → OpenConsole.exe 終了
        });

        let session = Arc::new(SharedSession {
            name: name.to_string(),
            created_at: Utc::now(),
            alive: AtomicBool::new(true),
            replay_buf: std::sync::Mutex::new(RingBuffer::new(REPLAY_CAPACITY)),
            output_tx: std::sync::Mutex::new(Some(output_tx.clone())),
            inner: Mutex::new(SessionInner {
                pty_writer,
                resize_tx: Some(resize_tx),
                resize_handle: Some(resize_handle),
                clients: Vec::new(),
                active_client_id: None,
                #[cfg(windows)]
                job,
                child: Some(child),
            }),
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

        // Child exit monitor: ConPTY は子プロセス終了後も reader を
        // ブロックし続けるため、別タスクで子プロセス終了を検知して
        // alive を false にする。SSH output_task がこれを参照して切断する。
        let session_for_monitor = Arc::clone(&session);
        let monitor_name = name.to_string();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CHILD_MONITOR_INTERVAL).await;
                // read_task が先に alive=false にした場合はロック不要
                if !session_for_monitor.is_alive() {
                    break;
                }
                let mut inner = session_for_monitor.inner.lock().await;
                if let Some(ref mut child) = inner.child {
                    match child.try_wait() {
                        Ok(Some(_status)) => {
                            tracing::debug!("Session {monitor_name}: child process exited");
                            break;
                        }
                        Ok(None) => {} // still running
                        Err(_) => break,
                    }
                } else {
                    break; // child already taken (destroy)
                }
            }
            session_for_monitor
                .alive
                .store(false, std::sync::atomic::Ordering::Release);
            session_for_monitor.output_tx.lock().unwrap().take();
        });

        (session, first_rx)
    }

    /// セッション作成（デフォルトシェル）
    ///
    /// 戻り値の `broadcast::Receiver` は PTY 出力の pre-subscriber。
    /// 最初のクライアントはこれを使うことで、read_task の初期出力を漏れなく受信できる。
    pub async fn create(
        &self,
        name: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(Arc<SharedSession>, broadcast::Receiver<Vec<u8>>), RegistryError> {
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

        let (session, first_rx) = Self::setup_pty_session(
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
                let resize_handle = {
                    let mut inner = session.inner.lock().await;
                    if let Some(mut child) = inner.child.take() {
                        let _ = tokio::task::spawn_blocking(move || {
                            let _ = child.kill();
                            let _ = child.wait();
                        })
                        .await;
                    }
                    inner.pty_writer = Box::new(std::io::sink());
                    inner.resize_tx.take();
                    inner.resize_handle.take()
                };
                if let Some(handle) = resize_handle {
                    let _ = tokio::time::timeout(TASK_JOIN_TIMEOUT, handle).await;
                }
                return Err(RegistryError::AlreadyExists(name.to_string()));
            }
            sessions.insert(name.to_string(), Arc::clone(&session));
        }

        tracing::info!("Session created: {name}");
        Ok((session, first_rx))
    }

    /// カスタムコマンドでセッション作成（Claude CLI 等）
    pub async fn create_with_pty(
        &self,
        name: &str,
        pty: super::manager::PtySession,
    ) -> Result<(Arc<SharedSession>, broadcast::Receiver<Vec<u8>>), RegistryError> {
        if !is_valid_session_name(name) {
            return Err(RegistryError::InvalidName(name.to_string()));
        }

        // 高速チェック
        if self.sessions.read().await.contains_key(name) {
            return Err(RegistryError::AlreadyExists(name.to_string()));
        }

        let (session, first_rx) = Self::setup_pty_session(
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
                let resize_handle = {
                    let mut inner = session.inner.lock().await;
                    if let Some(mut child) = inner.child.take() {
                        let _ = tokio::task::spawn_blocking(move || {
                            let _ = child.kill();
                            let _ = child.wait();
                        })
                        .await;
                    }
                    inner.pty_writer = Box::new(std::io::sink());
                    inner.resize_tx.take();
                    inner.resize_handle.take()
                };
                if let Some(handle) = resize_handle {
                    let _ = tokio::time::timeout(TASK_JOIN_TIMEOUT, handle).await;
                }
                return Err(RegistryError::AlreadyExists(name.to_string()));
            }
            sessions.insert(name.to_string(), Arc::clone(&session));
        }

        tracing::info!("Session created (custom PTY): {name}");
        Ok((session, first_rx))
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
            last_active: std::time::Instant::now(),
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
            Ok((session, first_rx)) => {
                let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
                let mut inner = session.inner.lock().await;
                inner.clients.push(ClientInfo {
                    id: client_id,
                    kind,
                    cols,
                    rows,
                    last_active: std::time::Instant::now(),
                });

                // first_rx は read_task 開始前に作成済みのため、
                // ConPTY の初期出力（DSR 等）を確実に保持している。
                // replay は不要（first_rx が全データを持つ）。
                let rx = first_rx;
                let replay = Vec::new();

                // ConPTY は同一サイズの resize を無視するため、
                // 異なるサイズで一度 resize してから正しいサイズに戻す。
                // これにより ConPTY の画面バッファが再描画され、初期出力がフラッシュされる。
                let nudge_cols = if cols > 1 { cols - 1 } else { cols + 1 };
                if let Some(ref tx) = inner.resize_tx {
                    let _ = tx.send((nudge_cols, rows));
                    let _ = tx.send((cols, rows));
                }

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
        // RwLock を即解放してから各セッションの Mutex を取得する
        let session_arcs: Vec<_> = self.sessions.read().await.values().cloned().collect();

        let mut result = Vec::with_capacity(session_arcs.len());
        for session in &session_arcs {
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

        let resize_handle = {
            let mut inner = session.inner.lock().await;

            // 1. Job Object で child + OpenConsole を一括 terminate
            //    OpenConsole が先に死ぬことで ClosePseudoConsole がブロックしなくなる
            #[cfg(windows)]
            if let Some(ref job) = inner.job
                && let Err(e) = job.terminate()
            {
                tracing::warn!("Job Object terminate failed for session {name}: {e}");
            }

            // 2. child を kill/wait（Job Object 対象外の場合のフォールバック）
            if let Some(mut child) = inner.child.take() {
                let child_name = name.to_string();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = child.kill() {
                        tracing::debug!("Session {child_name} child kill: {e}");
                    }
                    if let Err(e) = child.wait() {
                        tracing::warn!("Session {child_name} child wait: {e}");
                    }
                })
                .await;
            }

            // 3. pty_writer を閉じる（stdin パイプ閉鎖 → conhost の ReadFile 解除）
            inner.pty_writer = Box::new(std::io::sink());

            // 4. resize_tx を drop → resize_task 終了 → master drop → ClosePseudoConsole
            inner.resize_tx.take();

            inner.resize_handle.take()
        };

        // resize_handle を await（master drop → ClosePseudoConsole）
        // OpenConsole は既に Job Object で terminate 済みなので即完了するはず
        if let Some(handle) = resize_handle
            && tokio::time::timeout(TASK_JOIN_TIMEOUT, handle)
                .await
                .is_err()
        {
            tracing::warn!("Session {name}: resize_task did not finish within 5s");
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

    /// リサイズ再計算: アクティブなクライアントのサイズを PTY に反映する
    ///
    /// アクティブなクライアントは、最後に入力またはリサイズしたクライアント。
    /// フォールバックとして last_active が最新のクライアントを使用する。
    fn recalculate_size(inner: &mut SessionInner) {
        if inner.clients.is_empty() {
            return;
        }

        let active = if let Some(id) = inner.active_client_id {
            inner.clients.iter().find(|c| c.id == id)
        } else {
            None
        }
        .or_else(|| inner.clients.iter().max_by_key(|c| c.last_active))
        .unwrap();

        if let Some(ref tx) = inner.resize_tx {
            let _ = tx.send((active.cols, active.rows));
        }
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
            client.last_active = std::time::Instant::now();
        }
        inner.active_client_id = Some(client_id);
        SessionRegistry::recalculate_size(&mut inner);
    }

    /// クライアントをアクティブにする（入力時に呼ばれる）
    /// アクティブなクライアントが変わった場合のみ PTY をリサイズする
    pub async fn activate_client(&self, client_id: u64) {
        let mut inner = self.inner.lock().await;
        if inner.active_client_id == Some(client_id) {
            return; // 既にアクティブ → 何もしない
        }
        if let Some(client) = inner.clients.iter_mut().find(|c| c.id == client_id) {
            client.last_active = std::time::Instant::now();
        }
        inner.active_client_id = Some(client_id);
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
