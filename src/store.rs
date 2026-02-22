use serde::{Deserialize, Serialize};
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
}

// --- データモデル ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardEntry {
    pub text: String,
    /// Unix timestamp in milliseconds
    pub timestamp: u64,
    /// "copy" or "osc52"
    pub source: String,
}

const CLIPBOARD_MAX_ENTRIES: usize = 100;
const CLIPBOARD_MAX_TEXT_BYTES: usize = 10_240; // 10KB

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub auto_run: bool,
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
    pub sleep_prevention_mode: SleepPreventionMode,
    #[serde(default = "default_sleep_prevention_timeout")]
    pub sleep_prevention_timeout: u16,
    #[serde(skip_deserializing, default)]
    pub version: String,
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
            sleep_prevention_mode: SleepPreventionMode::default(),
            sleep_prevention_timeout: default_sleep_prevention_timeout(),
            version: String::new(),
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
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                tracing::warn!("Corrupt settings.json, using defaults: {e}");
                Settings::default()
            }),
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
}
