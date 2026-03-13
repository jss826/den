//! System clipboard monitoring
//!
//! PTY 内で動作するプログラム（yazi, vim 等）がシステムクリップボード経由で
//! コピーした場合にも clipboard history に記録する。
//! 2秒間隔でクリップボードのシーケンス番号をチェックし、
//! テキスト内容が変更された場合のみ Store に追加する。

use crate::store::Store;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const CLIPBOARD_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const CLIPBOARD_MAX_TEXT_BYTES: usize = 10_240;

/// Handle to stop the clipboard monitor on shutdown.
#[derive(Clone)]
pub struct ClipboardMonitorHandle {
    stop: Arc<AtomicBool>,
}

impl ClipboardMonitorHandle {
    /// Signal the monitor to stop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(windows)]
mod win32 {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::FALSE;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    const CF_UNICODETEXT: u32 = 13;

    /// クリップボードの変更カウンタを取得（軽量、クリップボードを開かない）
    pub fn get_sequence_number() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    /// クリップボードからテキストを読み取る
    pub fn get_clipboard_text() -> Option<String> {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == FALSE {
                return None;
            }
            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() {
                CloseClipboard();
                return None;
            }
            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() {
                CloseClipboard();
                return None;
            }
            // null terminator までの長さを計算
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let text = OsString::from_wide(slice).to_string_lossy().to_string();
            GlobalUnlock(handle);
            CloseClipboard();
            if text.is_empty() { None } else { Some(text) }
        }
    }
}

#[cfg(not(windows))]
mod desktop {
    pub enum ClipboardRead {
        Text(String),
        NoTextContent,
        Busy,
        Unavailable(String),
    }

    fn classify_error(err: arboard::Error) -> ClipboardRead {
        match err {
            arboard::Error::ContentNotAvailable | arboard::Error::ConversionFailure => {
                ClipboardRead::NoTextContent
            }
            arboard::Error::ClipboardOccupied => ClipboardRead::Busy,
            other => ClipboardRead::Unavailable(other.to_string()),
        }
    }

    /// Read text from the desktop clipboard when the platform backend is available.
    pub fn read_clipboard(clipboard: &mut arboard::Clipboard) -> ClipboardRead {
        match clipboard.get_text() {
            Ok(text) if text.is_empty() => ClipboardRead::NoTextContent,
            Ok(text) => ClipboardRead::Text(text),
            Err(err) => classify_error(err),
        }
    }

    /// Try to create a clipboard instance.
    pub fn new_clipboard() -> Result<arboard::Clipboard, String> {
        arboard::Clipboard::new().map_err(|e| e.to_string())
    }
}

fn should_track_text(text: &str, last_text: &str) -> bool {
    !text.is_empty() && text != last_text && text.len() <= CLIPBOARD_MAX_TEXT_BYTES
}

async fn record_clipboard_text(store: &Store, text: String, last_text: &mut String) {
    if !should_track_text(&text, last_text) {
        return;
    }

    // add_clipboard_entry internally deduplicates via retain, so no pre-check needed
    let store2 = store.clone();
    let text2 = text.clone();
    match tokio::task::spawn_blocking(move || {
        store2.add_clipboard_entry(text2, "system".to_string())
    })
    .await
    {
        Ok(Ok(_)) => {
            // Update last_text only after successful save to allow retry on failure
            *last_text = text;
        }
        Ok(Err(e)) => tracing::warn!("Clipboard monitor: failed to add entry: {e}"),
        Err(e) => tracing::warn!("Clipboard monitor: task panicked: {e}"),
    }
}

/// クリップボード監視を開始（バックグラウンド tokio タスク）
#[cfg(windows)]
pub fn start(store: Store) -> ClipboardMonitorHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let handle = ClipboardMonitorHandle { stop: stop.clone() };
    tokio::spawn(async move {
        let mut last_seq = win32::get_sequence_number();
        let mut last_text = String::new();
        // 初回: 現在のクリップボード内容を記録（履歴に追加はしない）
        if let Some(text) = tokio::task::spawn_blocking(win32::get_clipboard_text)
            .await
            .ok()
            .flatten()
        {
            last_text = text;
        }

        let mut interval = tokio::time::interval(CLIPBOARD_POLL_INTERVAL);
        loop {
            interval.tick().await;
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let current_seq = win32::get_sequence_number();
            if current_seq == last_seq {
                continue;
            }
            last_seq = current_seq;

            let text = match tokio::task::spawn_blocking(win32::get_clipboard_text).await {
                Ok(Some(t)) => t,
                _ => continue,
            };

            record_clipboard_text(&store, text, &mut last_text).await;
        }
    });
    handle
}

#[cfg(not(windows))]
pub fn start(store: Store) -> ClipboardMonitorHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let handle = ClipboardMonitorHandle { stop: stop.clone() };
    // Clipboard operations must stay on a single OS thread (Wayland/X11 connection affinity).
    // Use spawn_blocking to pin to a dedicated thread and reuse the Clipboard instance.
    tokio::task::spawn_blocking(move || {
        let mut clipboard = match desktop::new_clipboard() {
            Ok(c) => c,
            Err(e) => {
                tracing::info!("Clipboard monitor unavailable on this environment: {e}");
                return;
            }
        };

        let mut last_text = String::new();
        // Capture initial clipboard content without recording it
        if let desktop::ClipboardRead::Text(text) = desktop::read_clipboard(&mut clipboard) {
            last_text = text;
        }

        let rt = tokio::runtime::Handle::current();
        loop {
            std::thread::sleep(CLIPBOARD_POLL_INTERVAL);
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match desktop::read_clipboard(&mut clipboard) {
                desktop::ClipboardRead::Text(text) => {
                    rt.block_on(record_clipboard_text(&store, text, &mut last_text));
                }
                desktop::ClipboardRead::Unavailable(e) => {
                    tracing::info!("Clipboard monitor backend lost: {e}");
                    return;
                }
                desktop::ClipboardRead::NoTextContent | desktop::ClipboardRead::Busy => {}
            }
        }
    });
    handle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_new_non_empty_text() {
        assert!(should_track_text("hello", ""));
    }

    #[test]
    fn skips_duplicate_text() {
        assert!(!should_track_text("hello", "hello"));
    }

    #[test]
    fn skips_empty_text() {
        assert!(!should_track_text("", ""));
    }

    #[test]
    fn skips_large_text() {
        let large = "a".repeat(CLIPBOARD_MAX_TEXT_BYTES + 1);
        assert!(!should_track_text(&large, ""));
    }
}
