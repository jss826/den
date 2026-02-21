use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// サーバーサイド永続化ストア
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    /// Write-through cache for settings (updated on save, avoids file I/O on read)
    settings_cache: Arc<Mutex<Option<Settings>>>,
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
}

fn default_true() -> bool {
    true
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
    pub ssh_agent_forwarding: bool,
    #[serde(default)]
    pub keybar_position: Option<KeybarPosition>,
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
            keybar_buttons: None,
            ssh_agent_forwarding: false,
            keybar_position: None,
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
            Err(_) => Settings::default(),
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
    fn settings_unknown_fields_ignored() {
        // 旧バージョンの settings.json に残っているフィールド（例: claude_default_dir）が
        // デシリアライズ時にエラーにならないことを確認（後方互換性）
        let (store, tmp) = temp_store();
        fs::write(
            tmp.path().join("settings.json"),
            r#"{"font_size": 16, "claude_default_dir": "/old", "unknown_field": true}"#,
        )
        .unwrap();
        let settings = store.load_settings();
        assert_eq!(settings.font_size, 16);
        assert_eq!(settings.theme, "dark");
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
}
