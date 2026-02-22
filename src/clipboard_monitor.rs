//! Windows システムクリップボード監視
//!
//! PTY 内で動作するプログラム（yazi, vim 等）がシステムクリップボード経由で
//! コピーした場合にも clipboard history に記録する。
//! 2秒間隔でクリップボードのシーケンス番号をチェックし、
//! テキスト内容が変更された場合のみ Store に追加する。

use crate::store::Store;

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

/// クリップボード監視を開始（バックグラウンド tokio タスク）
#[cfg(windows)]
pub fn start(store: Store) {
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

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            let current_seq = win32::get_sequence_number();
            if current_seq == last_seq {
                continue;
            }
            last_seq = current_seq;

            let text = match tokio::task::spawn_blocking(win32::get_clipboard_text).await {
                Ok(Some(t)) => t,
                _ => continue,
            };

            // 前回と同じテキスト、または 10KB 超はスキップ
            if text == last_text || text.len() > 10_240 {
                continue;
            }
            last_text = text.clone();

            // add_clipboard_entry internally deduplicates via retain, so no pre-check needed
            let store2 = store.clone();
            match tokio::task::spawn_blocking(move || {
                store2.add_clipboard_entry(text, "system".to_string())
            })
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!("Clipboard monitor: failed to add entry: {e}"),
                Err(e) => tracing::warn!("Clipboard monitor: task panicked: {e}"),
            }
        }
    });
}

#[cfg(not(windows))]
pub fn start(_store: Store) {
    // 非 Windows 環境ではクリップボード監視は未実装
}
