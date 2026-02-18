use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// セッションメタキャッシュ
struct SessionCache {
    data: Vec<SessionMeta>,
    updated_at: Instant,
}

/// サーバーサイド永続化ストア
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    /// セッションごとのイベントファイルハンドルキャッシュ（open() コスト削減）
    event_files: Arc<Mutex<HashMap<String, fs::File>>>,
    /// list_sessions のキャッシュ（ディスク読み込み削減）
    session_cache: Arc<Mutex<Option<SessionCache>>>,
}

// --- データモデル ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybarButton {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub send: String,
    #[serde(default)]
    #[serde(rename = "type")]
    pub btn_type: Option<String>,
    #[serde(default)]
    pub mod_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_font_size")]
    pub font_size: u8,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_scrollback")]
    pub terminal_scrollback: u32,
    #[serde(default)]
    pub claude_default_connection: Option<serde_json::Value>,
    #[serde(default)]
    pub claude_default_dir: Option<String>,
    #[serde(default)]
    pub keybar_buttons: Option<Vec<KeybarButton>>,
    #[serde(default)]
    pub claude_input_position: Option<String>,
    #[serde(default)]
    pub ssh_agent_forwarding: bool,
}

fn default_font_size() -> u8 {
    14
}
fn default_theme() -> String {
    "dark".to_string()
}
fn default_scrollback() -> u32 {
    1000
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: default_font_size(),
            theme: default_theme(),
            terminal_scrollback: default_scrollback(),
            claude_default_connection: None,
            claude_default_dir: None,
            keybar_buttons: None,
            claude_input_position: None,
            ssh_agent_forwarding: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub prompt: String,
    pub connection: serde_json::Value,
    pub working_dir: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub total_cost: Option<f64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// セッション ID が安全な文字列か検証（英数字・ハイフンのみ許可）
fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

// --- Store 実装 ---

impl Store {
    /// 環境変数からデータディレクトリを取得して初期化
    pub fn from_data_dir(data_dir: &str) -> std::io::Result<Self> {
        let root = PathBuf::from(data_dir);
        Self::new(root)
    }

    /// 指定パスで初期化（ディレクトリ自動作成）
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(root.join("sessions"))?;
        Ok(Self {
            root,
            event_files: Arc::new(Mutex::new(HashMap::new())),
            session_cache: Arc::new(Mutex::new(None)),
        })
    }

    // --- Settings ---

    pub fn load_settings(&self) -> Settings {
        let path = self.root.join("settings.json");
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Corrupt settings.json, using defaults: {e}");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> std::io::Result<()> {
        let path = self.root.join("settings.json");
        let json = serde_json::to_string_pretty(settings)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    // --- Sessions ---

    pub fn create_session(&self, meta: &SessionMeta) -> std::io::Result<()> {
        if !is_valid_session_id(&meta.id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid session ID",
            ));
        }
        let session_dir = self.root.join("sessions").join(&meta.id);
        fs::create_dir_all(&session_dir)?;

        let meta_path = session_dir.join("meta.json");
        let json = serde_json::to_string_pretty(meta)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(meta_path, json)?;

        // events.jsonl を空で作成
        fs::File::create(session_dir.join("events.jsonl"))?;
        self.invalidate_session_cache();
        Ok(())
    }

    pub fn append_event(&self, session_id: &str, line: &str) -> std::io::Result<()> {
        if !is_valid_session_id(session_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid session ID",
            ));
        }
        let mut cache = self.event_files.lock().unwrap();
        let file = match cache.get_mut(session_id) {
            Some(f) => f,
            None => {
                let path = self
                    .root
                    .join("sessions")
                    .join(session_id)
                    .join("events.jsonl");
                let f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                cache.entry(session_id.to_string()).or_insert(f)
            }
        };
        writeln!(file, "{}", line)?;
        Ok(())
    }

    /// セッションのイベントファイルハンドルをキャッシュから削除
    pub fn close_event_file(&self, session_id: &str) {
        if let Ok(mut cache) = self.event_files.lock() {
            cache.remove(session_id);
        }
    }

    /// サーバー起動時に status=="running" のセッションを "completed" にリセット。
    /// 再起動後にプロセスが生き残ることはないため安全。
    pub fn cleanup_stale_running_sessions(&self) {
        let sessions_dir = self.root.join("sessions");
        let entries = match fs::read_dir(&sessions_dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            if let Some(mut meta) = self.load_session_meta_from_path(&entry.path())
                && meta.status == "running"
            {
                tracing::info!(
                    "Cleaning up stale running session: {} (created {})",
                    meta.id,
                    meta.created_at
                );
                meta.status = "completed".to_string();
                if meta.finished_at.is_none() {
                    meta.finished_at = Some(chrono::Utc::now());
                }
                if let Err(e) = self.update_session_meta(&meta) {
                    tracing::warn!("Failed to clean up session {}: {}", meta.id, e);
                }
            }
        }
    }

    pub fn update_session_meta(&self, meta: &SessionMeta) -> std::io::Result<()> {
        if !is_valid_session_id(&meta.id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid session ID",
            ));
        }
        let path = self.root.join("sessions").join(&meta.id).join("meta.json");
        let json = serde_json::to_string_pretty(meta)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)?;
        self.invalidate_session_cache();
        Ok(())
    }

    pub fn list_sessions(&self) -> Vec<SessionMeta> {
        // キャッシュが有効（2秒以内）ならそのまま返す
        if let Ok(cache) = self.session_cache.lock()
            && let Some(ref c) = *cache
            && c.updated_at.elapsed().as_secs() < 2
        {
            return c.data.clone();
        }

        let sessions_dir = self.root.join("sessions");
        let mut sessions = Vec::new();

        let entries = match fs::read_dir(&sessions_dir) {
            Ok(e) => e,
            Err(_) => return sessions,
        };

        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            match self.load_session_meta_from_path(&entry.path()) {
                Some(meta) => sessions.push(meta),
                None => {
                    tracing::warn!("Skipping corrupt session: {}", entry.path().display());
                }
            }
        }

        // 新しい順
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        // キャッシュ更新
        if let Ok(mut cache) = self.session_cache.lock() {
            *cache = Some(SessionCache {
                data: sessions.clone(),
                updated_at: Instant::now(),
            });
        }

        sessions
    }

    /// セッションキャッシュを無効化
    fn invalidate_session_cache(&self) {
        if let Ok(mut cache) = self.session_cache.lock() {
            *cache = None;
        }
    }

    pub fn load_session_meta(&self, id: &str) -> Option<SessionMeta> {
        if !is_valid_session_id(id) {
            return None;
        }
        let session_dir = self.root.join("sessions").join(id);
        self.load_session_meta_from_path(&session_dir)
    }

    pub fn load_session_events(&self, id: &str) -> Vec<String> {
        if !is_valid_session_id(id) {
            return Vec::new();
        }
        let path = self.root.join("sessions").join(id).join("events.jsonl");
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        std::io::BufReader::new(file)
            .lines()
            .filter_map(|line| {
                let line = line.ok()?;
                if line.trim().is_empty() {
                    None
                } else {
                    Some(line)
                }
            })
            .collect()
    }

    pub fn delete_session(&self, id: &str) -> std::io::Result<()> {
        if !is_valid_session_id(id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid session ID",
            ));
        }
        let session_dir = self.root.join("sessions").join(id);
        if !session_dir.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Session not found",
            ));
        }
        self.close_event_file(id);
        fs::remove_dir_all(session_dir)?;
        self.invalidate_session_cache();
        Ok(())
    }

    fn load_session_meta_from_path(&self, dir: &Path) -> Option<SessionMeta> {
        let meta_path = dir.join("meta.json");
        let content = fs::read_to_string(&meta_path).ok()?;
        serde_json::from_str(&content).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store() -> (Store, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf()).unwrap();
        (store, tmp)
    }

    #[test]
    fn settings_default_when_missing() {
        let (store, _tmp) = temp_store();
        let settings = store.load_settings();
        assert_eq!(settings.font_size, 14);
        assert_eq!(settings.theme, "dark");
        assert_eq!(settings.terminal_scrollback, 1000);
    }

    #[test]
    fn settings_roundtrip() {
        let (store, _tmp) = temp_store();
        let mut settings = Settings::default();
        settings.font_size = 18;
        settings.claude_default_dir = Some("/home/user".to_string());

        store.save_settings(&settings).unwrap();
        let loaded = store.load_settings();
        assert_eq!(loaded.font_size, 18);
        assert_eq!(loaded.claude_default_dir.as_deref(), Some("/home/user"));
    }

    #[test]
    fn settings_corrupt_returns_default() {
        let (store, tmp) = temp_store();
        fs::write(tmp.path().join("settings.json"), "NOT JSON!!!").unwrap();
        let settings = store.load_settings();
        assert_eq!(settings.font_size, 14);
    }

    #[test]
    fn settings_partial_json_uses_defaults() {
        let (store, tmp) = temp_store();
        fs::write(tmp.path().join("settings.json"), r#"{"font_size": 20}"#).unwrap();
        let settings = store.load_settings();
        assert_eq!(settings.font_size, 20);
        assert_eq!(settings.theme, "dark"); // default
    }

    fn sample_meta(id: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            prompt: "test prompt".to_string(),
            connection: json!({"type": "local"}),
            working_dir: "~/project".to_string(),
            status: "running".to_string(),
            created_at: Utc::now(),
            finished_at: None,
            total_cost: None,
            duration_ms: None,
        }
    }

    #[test]
    fn session_create_and_load() {
        let (store, _tmp) = temp_store();
        let meta = sample_meta("sess-001");
        store.create_session(&meta).unwrap();

        let loaded = store.load_session_meta("sess-001").unwrap();
        assert_eq!(loaded.id, "sess-001");
        assert_eq!(loaded.prompt, "test prompt");
    }

    #[test]
    fn session_list_order() {
        let (store, _tmp) = temp_store();

        let mut m1 = sample_meta("aaa");
        m1.created_at = chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        store.create_session(&m1).unwrap();

        let mut m2 = sample_meta("bbb");
        m2.created_at = chrono::DateTime::parse_from_rfc3339("2024-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        store.create_session(&m2).unwrap();

        let list = store.list_sessions();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "bbb"); // newer first
        assert_eq!(list[1].id, "aaa");
    }

    #[test]
    fn append_and_load_events() {
        let (store, _tmp) = temp_store();
        let meta = sample_meta("sess-ev");
        store.create_session(&meta).unwrap();

        store
            .append_event("sess-ev", r#"{"type":"assistant"}"#)
            .unwrap();
        store
            .append_event("sess-ev", r#"{"type":"tool_use"}"#)
            .unwrap();

        let events = store.load_session_events("sess-ev");
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("assistant"));
    }

    #[test]
    fn update_session_meta() {
        let (store, _tmp) = temp_store();
        let mut meta = sample_meta("sess-up");
        store.create_session(&meta).unwrap();

        meta.status = "completed".to_string();
        meta.finished_at = Some(Utc::now());
        meta.total_cost = Some(0.05);
        meta.duration_ms = Some(12000);
        store.update_session_meta(&meta).unwrap();

        let loaded = store.load_session_meta("sess-up").unwrap();
        assert_eq!(loaded.status, "completed");
        assert!(loaded.total_cost.is_some());
    }

    #[test]
    fn path_traversal_rejected() {
        let (store, _tmp) = temp_store();
        // ".." を含む ID は拒否
        let meta = sample_meta("../escape");
        assert!(store.create_session(&meta).is_err());
        assert!(store.load_session_meta("../escape").is_none());
        assert!(store.load_session_events("../../../etc/passwd").is_empty());
        assert!(store.append_event("../escape", "data").is_err());
    }

    #[test]
    fn empty_session_id_rejected() {
        let (store, _tmp) = temp_store();
        let meta = sample_meta("");
        assert!(store.create_session(&meta).is_err());
    }

    #[test]
    fn delete_session_ok() {
        let (store, _tmp) = temp_store();
        let meta = sample_meta("sess-del");
        store.create_session(&meta).unwrap();
        assert!(store.load_session_meta("sess-del").is_some());

        store.delete_session("sess-del").unwrap();
        assert!(store.load_session_meta("sess-del").is_none());
    }

    #[test]
    fn delete_session_not_found() {
        let (store, _tmp) = temp_store();
        let result = store.delete_session("nonexistent");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn delete_session_invalid_id() {
        let (store, _tmp) = temp_store();
        let result = store.delete_session("../escape");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn list_sessions_skips_corrupt() {
        let (store, tmp) = temp_store();
        let meta = sample_meta("good");
        store.create_session(&meta).unwrap();

        // corrupt session
        let bad_dir = tmp.path().join("sessions").join("corrupt");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(bad_dir.join("meta.json"), "BROKEN").unwrap();

        let list = store.list_sessions();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "good");
    }

    #[test]
    fn cleanup_stale_running_sessions() {
        let (store, _tmp) = temp_store();
        // sample_meta creates with status: "running"
        let running = sample_meta("sess-running");
        store.create_session(&running).unwrap();

        let mut completed = sample_meta("sess-done");
        completed.status = "completed".to_string();
        completed.finished_at = Some(Utc::now());
        store.create_session(&completed).unwrap();
        store.update_session_meta(&completed).unwrap();

        store.cleanup_stale_running_sessions();

        let r = store.load_session_meta("sess-running").unwrap();
        assert_eq!(r.status, "completed");
        assert!(r.finished_at.is_some());

        // already-completed session is untouched
        let c = store.load_session_meta("sess-done").unwrap();
        assert_eq!(c.status, "completed");
    }

    #[test]
    fn cleanup_no_sessions_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::from_data_dir(tmp.path().to_str().unwrap()).unwrap();
        // sessions dir exists but is empty — should not panic
        store.cleanup_stale_running_sessions();
    }
}
