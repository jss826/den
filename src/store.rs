use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// スリープ抑止モード
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SleepPreventionMode {
    Always,
    #[default]
    UserActivity,
    Off,
}

/// サーバーサイド永続化ストア
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    /// Write-through cache for settings (updated on save, avoids file I/O on read)
    settings_cache: Arc<Mutex<Option<Settings>>>,
    /// Write-through cache for clipboard history
    clipboard_cache: Arc<Mutex<Option<Vec<ClipboardEntry>>>>,
    /// Write-through cache for SSH known hosts
    known_hosts_cache: Arc<Mutex<Option<HashMap<String, KnownHost>>>>,
    /// Write-through cache for trusted TLS certificates
    trusted_tls_cache: Arc<Mutex<Option<HashMap<String, TrustedTlsCert>>>>,
}

// --- データモデル ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardEntry {
    pub text: String,
    /// Unix timestamp in milliseconds
    pub timestamp: u64,
    /// "copy", "osc52", or "system"
    pub source: String,
}

const CLIPBOARD_MAX_ENTRIES: usize = 100;
const CLIPBOARD_MAX_TEXT_BYTES: usize = 10_240; // 10KB

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownHost {
    pub fingerprint: String,
    pub algorithm: String,
    /// Unix timestamp in milliseconds
    pub first_seen: u64,
    /// Unix timestamp in milliseconds
    pub last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedTlsCert {
    pub fingerprint: String,
    /// Unix timestamp in milliseconds
    pub first_seen: u64,
    /// Unix timestamp in milliseconds
    pub last_seen: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub auto_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SshAuthType {
    #[default]
    Password,
    Key,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshBookmark {
    pub label: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    pub auth_type: SshAuthType,
    #[serde(default)]
    pub key_path: Option<String>,
    #[serde(default)]
    pub initial_dir: Option<String>,
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenBookmark {
    pub label: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    pub use_relay: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_password: Option<String>,
}

/// Persisted session record for restart recovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<crate::pty::registry::SshSessionConfig>,
}

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
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub items: Option<Vec<KeybarButton>>,
    #[serde(default)]
    pub selected: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybarPosition {
    #[serde(default)]
    pub left: f64,
    #[serde(default)]
    pub top: f64,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default = "default_collapse_side")]
    pub collapse_side: String,
    #[serde(default)]
    pub secondary_visible: bool,
}

fn default_true() -> bool {
    true
}

fn default_collapse_side() -> String {
    "right".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_font_size")]
    pub font_size: u8,
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Valid range: 100–50000 (clamped server-side in put_settings)
    #[serde(default = "default_scrollback")]
    pub terminal_scrollback: u32,
    #[serde(default)]
    pub keybar_buttons: Option<Vec<KeybarButton>>,
    #[serde(default)]
    pub keybar_secondary_buttons: Option<Vec<KeybarButton>>,
    #[serde(default)]
    pub ssh_agent_forwarding: bool,
    #[serde(default)]
    pub keybar_position: Option<KeybarPosition>,
    #[serde(default)]
    pub snippets: Option<Vec<Snippet>>,
    #[serde(default)]
    pub ssh_bookmarks: Option<Vec<SshBookmark>>,
    #[serde(default)]
    pub den_bookmarks: Option<Vec<DenBookmark>>,
    #[serde(default)]
    pub sleep_prevention_mode: SleepPreventionMode,
    #[serde(default = "default_sleep_prevention_timeout")]
    pub sleep_prevention_timeout: u16,
    #[serde(default = "default_true")]
    pub group_remote_sessions: bool,
    #[serde(skip_deserializing, default)]
    pub version: String,
    #[serde(skip_deserializing, default)]
    pub hostname: String,
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
fn default_sleep_prevention_timeout() -> u16 {
    30
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_size: default_font_size(),
            theme: default_theme(),
            terminal_scrollback: default_scrollback(),
            keybar_buttons: None,
            keybar_secondary_buttons: None,
            ssh_agent_forwarding: false,
            keybar_position: None,
            snippets: None,
            ssh_bookmarks: None,
            den_bookmarks: None,
            sleep_prevention_mode: SleepPreventionMode::default(),
            sleep_prevention_timeout: default_sleep_prevention_timeout(),
            group_remote_sessions: true,
            version: String::new(),
            hostname: String::new(),
        }
    }
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
        fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            settings_cache: Arc::new(Mutex::new(None)),
            clipboard_cache: Arc::new(Mutex::new(None)),
            known_hosts_cache: Arc::new(Mutex::new(None)),
            trusted_tls_cache: Arc::new(Mutex::new(None)),
        })
    }

    // --- Settings ---

    pub fn load_settings(&self) -> Settings {
        if let Some(cached) = self.settings_cache.lock().unwrap().as_ref() {
            return cached.clone();
        }
        let settings = self.load_settings_from_disk();
        *self.settings_cache.lock().unwrap() = Some(settings.clone());
        settings
    }

    fn load_settings_from_disk(&self) -> Settings {
        let path = self.root.join("settings.json");
        match fs::read_to_string(&path) {
            Ok(content) => {
                // Detect and warn about legacy peer fields (removed in Quick Connect migration)
                if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&content)
                    && (raw.get("peer_name").is_some() || raw.get("peers").is_some())
                {
                    tracing::warn!(
                        "Legacy peer config fields found in settings.json \
                         — peer_name and peers will be dropped (removed in this version)"
                    );
                }
                serde_json::from_str(&content).unwrap_or_else(|e| {
                    tracing::warn!("Corrupt settings.json, using defaults: {e}");
                    Settings::default()
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => {
                tracing::warn!("Failed to read settings.json, using defaults: {e}");
                Settings::default()
            }
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> std::io::Result<()> {
        let path = self.root.join("settings.json");
        let json = serde_json::to_string_pretty(settings)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)?;
        *self.settings_cache.lock().unwrap() = Some(settings.clone());
        Ok(())
    }

    // --- Clipboard History ---

    pub fn load_clipboard_history(&self) -> Vec<ClipboardEntry> {
        let mut cache = self.clipboard_cache.lock().unwrap();
        if let Some(cached) = cache.as_ref() {
            return cached.clone();
        }
        let entries = self.load_clipboard_from_disk();
        *cache = Some(entries.clone());
        entries
    }

    fn load_clipboard_from_disk(&self) -> Vec<ClipboardEntry> {
        let path = self.root.join("clipboard-history.json");
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Corrupt clipboard-history.json, using empty: {e}");
                Vec::new()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::warn!("Failed to read clipboard-history.json: {e}");
                Vec::new()
            }
        }
    }

    pub fn add_clipboard_entry(
        &self,
        text: String,
        source: String,
    ) -> std::io::Result<Vec<ClipboardEntry>> {
        // Truncate FIRST (F005: before dedup, F001: UTF-8 safe)
        let text = if text.len() > CLIPBOARD_MAX_TEXT_BYTES {
            text[..text.floor_char_boundary(CLIPBOARD_MAX_TEXT_BYTES)].to_string()
        } else {
            text
        };

        // Hold lock across entire read-modify-write (F002)
        let mut cache = self.clipboard_cache.lock().unwrap();
        let mut entries = cache
            .take()
            .unwrap_or_else(|| self.load_clipboard_from_disk());

        // Remove duplicate (same text) if exists
        entries.retain(|e| e.text != text);

        // Prepend new entry
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        entries.insert(
            0,
            ClipboardEntry {
                text,
                timestamp: now,
                source,
            },
        );

        // Enforce max entries
        entries.truncate(CLIPBOARD_MAX_ENTRIES);

        // Write to disk (without re-locking cache)
        let path = self.root.join("clipboard-history.json");
        let json = serde_json::to_string(&entries).map_err(std::io::Error::other)?;
        fs::write(path, json)?;

        *cache = Some(entries.clone());
        Ok(entries)
    }

    pub fn clear_clipboard_history(&self) -> std::io::Result<()> {
        let mut cache = self.clipboard_cache.lock().unwrap();
        let path = self.root.join("clipboard-history.json");
        let json =
            serde_json::to_string(&Vec::<ClipboardEntry>::new()).map_err(std::io::Error::other)?;
        fs::write(path, json)?;
        *cache = Some(Vec::new());
        Ok(())
    }

    // --- Session Order ---

    pub fn load_session_order(&self) -> Vec<String> {
        let path = self.root.join("session-order.json");
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Corrupt session-order.json, using empty: {e}");
                Vec::new()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::warn!("Failed to read session-order.json: {e}");
                Vec::new()
            }
        }
    }

    pub fn save_session_order(&self, order: &[String]) -> std::io::Result<()> {
        let path = self.root.join("session-order.json");
        let json = serde_json::to_string(order).map_err(std::io::Error::other)?;
        fs::write(path, json)
    }

    // --- Session Records ---

    pub fn load_sessions(&self) -> Vec<SessionRecord> {
        let path = self.root.join("sessions.json");
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Corrupt sessions.json, using empty: {e}");
                Vec::new()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::warn!("Failed to read sessions.json: {e}");
                Vec::new()
            }
        }
    }

    pub fn save_sessions(&self, sessions: &[SessionRecord]) -> std::io::Result<()> {
        let path = self.root.join("sessions.json");
        let json = serde_json::to_string_pretty(sessions).map_err(std::io::Error::other)?;
        fs::write(path, json)
    }

    // --- SSH Known Hosts ---

    pub fn load_known_hosts(&self) -> HashMap<String, KnownHost> {
        let mut cache = self.known_hosts_cache.lock().unwrap();
        if let Some(cached) = cache.as_ref() {
            return cached.clone();
        }
        let hosts = self.load_known_hosts_from_disk();
        *cache = Some(hosts.clone());
        hosts
    }

    fn load_known_hosts_from_disk(&self) -> HashMap<String, KnownHost> {
        let path = self.root.join("ssh-known-hosts.json");
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Corrupt ssh-known-hosts.json, using empty: {e}");
                HashMap::new()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => {
                tracing::warn!("Failed to read ssh-known-hosts.json: {e}");
                HashMap::new()
            }
        }
    }

    pub fn get_known_host(&self, host_port: &str) -> Option<KnownHost> {
        let mut cache = self.known_hosts_cache.lock().unwrap();
        if cache.is_none() {
            *cache = Some(self.load_known_hosts_from_disk());
        }
        cache.as_ref().unwrap().get(host_port).cloned()
    }

    pub fn save_known_host(&self, host_port: &str, entry: KnownHost) -> std::io::Result<()> {
        let mut cache = self.known_hosts_cache.lock().unwrap();
        let mut hosts = cache
            .take()
            .unwrap_or_else(|| self.load_known_hosts_from_disk());

        // Preserve first_seen if entry already exists
        let entry = if let Some(existing) = hosts.get(host_port) {
            KnownHost {
                first_seen: existing.first_seen,
                ..entry
            }
        } else {
            entry
        };

        hosts.insert(host_port.to_string(), entry);

        let path = self.root.join("ssh-known-hosts.json");
        let json = serde_json::to_string(&hosts).map_err(std::io::Error::other)?;
        if let Err(e) = fs::write(path, &json) {
            // Restore cache before returning error
            *cache = Some(hosts);
            return Err(e);
        }

        *cache = Some(hosts);
        Ok(())
    }

    /// Update last_seen timestamp (cache-only, best-effort disk write on next save)
    pub fn update_known_host_last_seen(&self, host_port: &str) {
        let mut cache = self.known_hosts_cache.lock().unwrap();
        if cache.is_none() {
            *cache = Some(self.load_known_hosts_from_disk());
        }
        if let Some(entry) = cache.as_mut().unwrap().get_mut(host_port) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            entry.last_seen = now;
        }
    }

    pub fn remove_known_host(&self, host_port: &str) -> std::io::Result<()> {
        let mut cache = self.known_hosts_cache.lock().unwrap();
        let mut hosts = cache
            .take()
            .unwrap_or_else(|| self.load_known_hosts_from_disk());

        hosts.remove(host_port);

        let path = self.root.join("ssh-known-hosts.json");
        let json = serde_json::to_string(&hosts).map_err(std::io::Error::other)?;
        if let Err(e) = fs::write(path, &json) {
            *cache = Some(hosts);
            return Err(e);
        }

        *cache = Some(hosts);
        Ok(())
    }

    // --- Trusted TLS Certificates ---

    pub fn load_trusted_tls(&self) -> HashMap<String, TrustedTlsCert> {
        let mut cache = self.trusted_tls_cache.lock().unwrap();
        if let Some(cached) = cache.as_ref() {
            return cached.clone();
        }
        let certs = self.load_trusted_tls_from_disk();
        *cache = Some(certs.clone());
        certs
    }

    fn load_trusted_tls_from_disk(&self) -> HashMap<String, TrustedTlsCert> {
        let path = self.root.join("trusted-tls-certs.json");
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Corrupt trusted-tls-certs.json, using empty: {e}");
                HashMap::new()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => {
                tracing::warn!("Failed to read trusted-tls-certs.json: {e}");
                HashMap::new()
            }
        }
    }

    pub fn get_trusted_tls_cert(&self, host_port: &str) -> Option<TrustedTlsCert> {
        let mut cache = self.trusted_tls_cache.lock().unwrap();
        if cache.is_none() {
            *cache = Some(self.load_trusted_tls_from_disk());
        }
        cache.as_ref().unwrap().get(host_port).cloned()
    }

    pub fn save_trusted_tls_cert(
        &self,
        host_port: &str,
        entry: TrustedTlsCert,
    ) -> std::io::Result<()> {
        let mut cache = self.trusted_tls_cache.lock().unwrap();
        let mut certs = cache
            .take()
            .unwrap_or_else(|| self.load_trusted_tls_from_disk());

        let entry = if let Some(existing) = certs.get(host_port) {
            TrustedTlsCert {
                first_seen: existing.first_seen,
                display_name: entry.display_name.or_else(|| existing.display_name.clone()),
                ..entry
            }
        } else {
            entry
        };

        certs.insert(host_port.to_string(), entry);

        let path = self.root.join("trusted-tls-certs.json");
        let json = serde_json::to_string(&certs).map_err(std::io::Error::other)?;
        if let Err(e) = fs::write(path, &json) {
            *cache = Some(certs);
            return Err(e);
        }

        *cache = Some(certs);
        Ok(())
    }

    pub fn update_trusted_tls_display_name(
        &self,
        host_port: &str,
        display_name: Option<String>,
    ) -> std::io::Result<bool> {
        let mut cache = self.trusted_tls_cache.lock().unwrap();
        let mut certs = cache
            .take()
            .unwrap_or_else(|| self.load_trusted_tls_from_disk());

        let Some(entry) = certs.get_mut(host_port) else {
            *cache = Some(certs);
            return Ok(false);
        };
        entry.display_name = display_name;

        let path = self.root.join("trusted-tls-certs.json");
        let json = serde_json::to_string(&certs).map_err(std::io::Error::other)?;
        if let Err(e) = fs::write(path, &json) {
            *cache = Some(certs);
            return Err(e);
        }

        *cache = Some(certs);
        Ok(true)
    }

    pub fn remove_trusted_tls_cert(&self, host_port: &str) -> std::io::Result<()> {
        let mut cache = self.trusted_tls_cache.lock().unwrap();
        let mut certs = cache
            .take()
            .unwrap_or_else(|| self.load_trusted_tls_from_disk());

        certs.remove(host_port);

        let path = self.root.join("trusted-tls-certs.json");
        let json = serde_json::to_string(&certs).map_err(std::io::Error::other)?;
        if let Err(e) = fs::write(path, &json) {
            *cache = Some(certs);
            return Err(e);
        }

        *cache = Some(certs);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        store.save_settings(&settings).unwrap();
        let loaded = store.load_settings();
        assert_eq!(loaded.font_size, 18);
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

    #[test]
    fn from_data_dir_creates_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        let store = Store::from_data_dir(&nested.to_string_lossy()).unwrap();
        assert!(nested.exists());
        // settings should return defaults for a fresh store
        let settings = store.load_settings();
        assert_eq!(settings.font_size, 14);
    }

    #[test]
    fn settings_save_and_load_with_keybar() {
        let (store, _tmp) = temp_store();
        let settings = Settings {
            keybar_buttons: Some(vec![KeybarButton {
                label: "Tab".to_string(),
                send: "\t".to_string(),
                btn_type: Some("key".to_string()),
                mod_key: None,
                action: None,
                display: None,
                items: None,
                selected: None,
            }]),
            ..Settings::default()
        };
        store.save_settings(&settings).unwrap();
        let loaded = store.load_settings();
        let buttons = loaded.keybar_buttons.unwrap();
        assert_eq!(buttons.len(), 1);
        assert_eq!(buttons[0].label, "Tab");
        assert_eq!(buttons[0].send, "\t");
    }

    #[test]
    fn settings_stack_button_roundtrip() {
        let (store, _tmp) = temp_store();
        let settings = Settings {
            keybar_buttons: Some(vec![KeybarButton {
                label: String::new(),
                send: String::new(),
                btn_type: Some("stack".to_string()),
                mod_key: None,
                action: None,
                display: None,
                items: Some(vec![
                    KeybarButton {
                        label: "Sc↑".to_string(),
                        send: String::new(),
                        btn_type: Some("action".to_string()),
                        mod_key: None,
                        action: Some("scroll-page-up".to_string()),
                        display: Some("Scroll page up".to_string()),
                        items: None,
                        selected: None,
                    },
                    KeybarButton {
                        label: "Sc↓".to_string(),
                        send: String::new(),
                        btn_type: Some("action".to_string()),
                        mod_key: None,
                        action: Some("scroll-page-down".to_string()),
                        display: Some("Scroll page down".to_string()),
                        items: None,
                        selected: None,
                    },
                ]),
                selected: Some(1),
            }]),
            ..Settings::default()
        };
        store.save_settings(&settings).unwrap();
        // Clear cache to force disk read
        *store.settings_cache.lock().unwrap() = None;
        let loaded = store.load_settings();
        let buttons = loaded.keybar_buttons.unwrap();
        assert_eq!(buttons.len(), 1);
        assert_eq!(buttons[0].btn_type.as_deref(), Some("stack"));
        assert_eq!(buttons[0].selected, Some(1));
        let items = buttons[0].items.as_ref().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "Sc↑");
        assert_eq!(items[0].action.as_deref(), Some("scroll-page-up"));
        assert_eq!(items[1].label, "Sc↓");
    }

    #[test]
    fn settings_snippet_roundtrip() {
        let (store, _tmp) = temp_store();
        let settings = Settings {
            snippets: Some(vec![
                Snippet {
                    label: "workspace".to_string(),
                    command: "cd d:\\workspace".to_string(),
                    auto_run: true,
                },
                Snippet {
                    label: "status".to_string(),
                    command: "git status".to_string(),
                    auto_run: false,
                },
            ]),
            ..Settings::default()
        };
        store.save_settings(&settings).unwrap();
        *store.settings_cache.lock().unwrap() = None;
        let loaded = store.load_settings();
        let snippets = loaded.snippets.unwrap();
        assert_eq!(snippets.len(), 2);
        assert_eq!(snippets[0].label, "workspace");
        assert_eq!(snippets[0].command, "cd d:\\workspace");
        assert!(snippets[0].auto_run);
        assert_eq!(snippets[1].label, "status");
        assert!(!snippets[1].auto_run);
    }

    #[test]
    fn settings_snippet_auto_run_defaults_to_false() {
        let (store, tmp) = temp_store();
        // auto_run omitted from JSON — should default to false
        fs::write(
            tmp.path().join("settings.json"),
            r#"{"snippets":[{"label":"foo","command":"bar"}]}"#,
        )
        .unwrap();
        let settings = store.load_settings();
        let snippets = settings.snippets.unwrap();
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].label, "foo");
        assert!(!snippets[0].auto_run);
    }

    #[test]
    fn settings_empty_json_uses_all_defaults() {
        let (store, tmp) = temp_store();
        fs::write(tmp.path().join("settings.json"), "{}").unwrap();
        let settings = store.load_settings();
        assert_eq!(settings.font_size, 14);
        assert_eq!(settings.theme, "dark");
        assert_eq!(settings.terminal_scrollback, 1000);
        assert!(settings.keybar_buttons.is_none());
        assert!(!settings.ssh_agent_forwarding);
    }

    // --- Clipboard History tests ---

    #[test]
    fn clipboard_empty_when_missing() {
        let (store, _tmp) = temp_store();
        let entries = store.load_clipboard_history();
        assert!(entries.is_empty());
    }

    #[test]
    fn clipboard_add_and_load() {
        let (store, _tmp) = temp_store();
        let entries = store
            .add_clipboard_entry("hello".to_string(), "copy".to_string())
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "hello");
        assert_eq!(entries[0].source, "copy");

        // Load from cache
        let loaded = store.load_clipboard_history();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].text, "hello");
    }

    #[test]
    fn clipboard_dedup_moves_to_front() {
        let (store, _tmp) = temp_store();
        store
            .add_clipboard_entry("first".to_string(), "copy".to_string())
            .unwrap();
        store
            .add_clipboard_entry("second".to_string(), "copy".to_string())
            .unwrap();
        let entries = store
            .add_clipboard_entry("first".to_string(), "osc52".to_string())
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "first");
        assert_eq!(entries[0].source, "osc52");
        assert_eq!(entries[1].text, "second");
    }

    #[test]
    fn clipboard_max_entries() {
        let (store, _tmp) = temp_store();
        for i in 0..110 {
            store
                .add_clipboard_entry(format!("entry-{i}"), "copy".to_string())
                .unwrap();
        }
        let entries = store.load_clipboard_history();
        assert_eq!(entries.len(), CLIPBOARD_MAX_ENTRIES);
        assert_eq!(entries[0].text, "entry-109");
    }

    #[test]
    fn clipboard_clear() {
        let (store, _tmp) = temp_store();
        store
            .add_clipboard_entry("hello".to_string(), "copy".to_string())
            .unwrap();
        store.clear_clipboard_history().unwrap();
        let entries = store.load_clipboard_history();
        assert!(entries.is_empty());
    }

    #[test]
    fn clipboard_corrupt_json_returns_empty() {
        let (store, tmp) = temp_store();
        fs::write(tmp.path().join("clipboard-history.json"), "NOT JSON!!!").unwrap();
        let entries = store.load_clipboard_history();
        assert!(entries.is_empty());
    }

    #[test]
    fn clipboard_reload_from_disk() {
        let (store, _tmp) = temp_store();
        store
            .add_clipboard_entry("hello".to_string(), "copy".to_string())
            .unwrap();
        // Clear cache to force disk read
        *store.clipboard_cache.lock().unwrap() = None;
        let entries = store.load_clipboard_history();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "hello");
    }

    #[test]
    fn clipboard_truncate_multibyte_utf8() {
        let (store, _tmp) = temp_store();
        // "あ" is 3 bytes; create text exceeding CLIPBOARD_MAX_TEXT_BYTES
        let text = "あ".repeat(5000); // 15000 bytes > 10240
        let entries = store.add_clipboard_entry(text, "copy".to_string()).unwrap();
        assert_eq!(entries.len(), 1);
        // Should be truncated to at most CLIPBOARD_MAX_TEXT_BYTES
        assert!(entries[0].text.len() <= CLIPBOARD_MAX_TEXT_BYTES);
        // Must be valid UTF-8 (no panic, no partial char)
        assert!(entries[0].text.is_char_boundary(entries[0].text.len()));
    }

    // --- Known Hosts tests ---

    #[test]
    fn known_hosts_empty_when_missing() {
        let (store, _tmp) = temp_store();
        let hosts = store.load_known_hosts();
        assert!(hosts.is_empty());
    }

    #[test]
    fn known_hosts_save_and_get() {
        let (store, _tmp) = temp_store();
        let entry = KnownHost {
            fingerprint: "SHA256:abc123".to_string(),
            algorithm: "ssh-ed25519".to_string(),
            first_seen: 1000,
            last_seen: 1000,
        };
        store.save_known_host("example.com:22", entry).unwrap();
        let loaded = store.get_known_host("example.com:22");
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.fingerprint, "SHA256:abc123");
        assert_eq!(loaded.algorithm, "ssh-ed25519");
    }

    #[test]
    fn known_hosts_preserves_first_seen_on_update() {
        let (store, _tmp) = temp_store();
        let entry = KnownHost {
            fingerprint: "SHA256:abc123".to_string(),
            algorithm: "ssh-ed25519".to_string(),
            first_seen: 1000,
            last_seen: 1000,
        };
        store.save_known_host("example.com:22", entry).unwrap();

        let updated = KnownHost {
            fingerprint: "SHA256:def456".to_string(),
            algorithm: "ssh-ed25519".to_string(),
            first_seen: 2000,
            last_seen: 2000,
        };
        store.save_known_host("example.com:22", updated).unwrap();

        let loaded = store.get_known_host("example.com:22").unwrap();
        assert_eq!(loaded.fingerprint, "SHA256:def456");
        assert_eq!(loaded.first_seen, 1000); // preserved
        assert_eq!(loaded.last_seen, 2000);
    }

    #[test]
    fn known_hosts_remove() {
        let (store, _tmp) = temp_store();
        let entry = KnownHost {
            fingerprint: "SHA256:abc123".to_string(),
            algorithm: "ssh-ed25519".to_string(),
            first_seen: 1000,
            last_seen: 1000,
        };
        store.save_known_host("example.com:22", entry).unwrap();
        store.remove_known_host("example.com:22").unwrap();
        assert!(store.get_known_host("example.com:22").is_none());
    }

    #[test]
    fn known_hosts_corrupt_json_returns_empty() {
        let (store, tmp) = temp_store();
        fs::write(tmp.path().join("ssh-known-hosts.json"), "NOT JSON!!!").unwrap();
        let hosts = store.load_known_hosts();
        assert!(hosts.is_empty());
    }

    #[test]
    fn known_hosts_disk_roundtrip() {
        let (store, _tmp) = temp_store();
        let entry = KnownHost {
            fingerprint: "SHA256:abc123".to_string(),
            algorithm: "ssh-ed25519".to_string(),
            first_seen: 1000,
            last_seen: 1000,
        };
        store.save_known_host("example.com:22", entry).unwrap();
        // Clear cache to force disk read
        *store.known_hosts_cache.lock().unwrap() = None;
        let loaded = store.get_known_host("example.com:22").unwrap();
        assert_eq!(loaded.fingerprint, "SHA256:abc123");
    }

    #[test]
    fn trusted_tls_empty_when_missing() {
        let (store, _tmp) = temp_store();
        let certs = store.load_trusted_tls();
        assert!(certs.is_empty());
    }

    #[test]
    fn trusted_tls_save_and_get() {
        let (store, _tmp) = temp_store();
        let entry = TrustedTlsCert {
            fingerprint: "SHA256:deadbeef".to_string(),
            first_seen: 1000,
            last_seen: 1000,
            display_name: None,
        };
        store
            .save_trusted_tls_cert("example.com:8443", entry)
            .unwrap();
        let loaded = store.get_trusted_tls_cert("example.com:8443").unwrap();
        assert_eq!(loaded.fingerprint, "SHA256:deadbeef");
    }

    #[test]
    fn trusted_tls_preserves_first_seen_on_update() {
        let (store, _tmp) = temp_store();
        let entry = TrustedTlsCert {
            fingerprint: "SHA256:deadbeef".to_string(),
            first_seen: 1000,
            last_seen: 1000,
            display_name: None,
        };
        store
            .save_trusted_tls_cert("example.com:8443", entry)
            .unwrap();

        let updated = TrustedTlsCert {
            fingerprint: "SHA256:beadfeed".to_string(),
            first_seen: 2000,
            last_seen: 3000,
            display_name: None,
        };
        store
            .save_trusted_tls_cert("example.com:8443", updated)
            .unwrap();

        let loaded = store.get_trusted_tls_cert("example.com:8443").unwrap();
        assert_eq!(loaded.first_seen, 1000);
        assert_eq!(loaded.last_seen, 3000);
        assert_eq!(loaded.fingerprint, "SHA256:beadfeed");
    }

    #[test]
    fn trusted_tls_remove() {
        let (store, _tmp) = temp_store();
        let entry = TrustedTlsCert {
            fingerprint: "SHA256:deadbeef".to_string(),
            first_seen: 1000,
            last_seen: 1000,
            display_name: None,
        };
        store
            .save_trusted_tls_cert("example.com:8443", entry)
            .unwrap();
        store.remove_trusted_tls_cert("example.com:8443").unwrap();
        assert!(store.get_trusted_tls_cert("example.com:8443").is_none());
    }

    // --- Session Order tests ---

    #[test]
    fn session_order_empty_when_missing() {
        let (store, _tmp) = temp_store();
        let order = store.load_session_order();
        assert!(order.is_empty());
    }

    #[test]
    fn session_order_roundtrip() {
        let (store, _tmp) = temp_store();
        let order = vec!["b".to_string(), "a".to_string(), "c".to_string()];
        store.save_session_order(&order).unwrap();
        let loaded = store.load_session_order();
        assert_eq!(loaded, order);
    }
}
