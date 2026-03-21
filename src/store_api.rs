// テスト: tests/api_test.rs の Settings API セクションで統合テスト済み
// （GET/PUT 正常系・認証必須・不正JSON・部分JSON）
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::store::Settings;

// --- Bookmark password encryption (AES-256-GCM with HMAC-derived key) ---

fn derive_bookmark_key(master_password: &str) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(b"den-bookmark-encryption-key").expect("HMAC key length");
    mac.update(master_password.as_bytes());
    let result = mac.finalize().into_bytes();
    let mut key = [0u8; 32];
    key.copy_from_slice(&result);
    key
}

fn encrypt_password(plain: &str, key: &[u8; 32]) -> String {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    use base64::Engine;
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES key length");
    let nonce_bytes: [u8; 12] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plain.as_bytes())
        .expect("AES-GCM encrypt");
    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    base64::engine::general_purpose::STANDARD.encode(&combined)
}

fn decrypt_password(encrypted: &str, key: &[u8; 32]) -> Result<String, String> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    use base64::Engine;
    let combined = base64::engine::general_purpose::STANDARD
        .decode(encrypted)
        .map_err(|e| format!("base64 decode: {e}"))?;
    if combined.len() < 29 {
        // 12 nonce + 1 plaintext min + 16 tag
        return Err("encrypted data too short".into());
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES key length");
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "decryption failed (wrong password?)")?;
    String::from_utf8(plaintext).map_err(|e| format!("utf8: {e}"))
}

/// Encrypt plaintext bookmark passwords for disk storage
fn encrypt_den_bookmarks(settings: &mut Settings, key: &[u8; 32]) {
    if let Some(ref mut bookmarks) = settings.den_bookmarks {
        for b in bookmarks.iter_mut() {
            if let Some(ref pw) = b.password
                && !pw.is_empty()
            {
                b.password = Some(encrypt_password(pw, key));
            }
            if let Some(ref pw) = b.relay_password
                && !pw.is_empty()
            {
                b.relay_password = Some(encrypt_password(pw, key));
            }
        }
    }
}

/// Decrypt bookmark passwords for API response (best-effort: leave encrypted on failure)
fn decrypt_den_bookmarks(settings: &mut Settings, key: &[u8; 32]) {
    if let Some(ref mut bookmarks) = settings.den_bookmarks {
        for b in bookmarks.iter_mut() {
            if let Some(ref pw) = b.password
                && !pw.is_empty()
                && let Ok(plain) = decrypt_password(pw, key)
            {
                b.password = Some(plain);
            }
            if let Some(ref pw) = b.relay_password
                && !pw.is_empty()
                && let Ok(plain) = decrypt_password(pw, key)
            {
                b.relay_password = Some(plain);
            }
        }
    }
}

/// GET /api/settings
pub async fn get_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.load_settings()).await {
        Ok(mut settings) => {
            settings.version = env!("CARGO_PKG_VERSION").to_string();
            settings.hostname = gethostname::gethostname().to_string_lossy().into_owned();
            // Decrypt bookmark passwords for API response
            let key = derive_bookmark_key(&state.config.password);
            decrypt_den_bookmarks(&mut settings, &key);
            Json(settings).into_response()
        }
        Err(e) => {
            tracing::error!("load_settings task panicked: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// PUT /api/settings
pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(mut settings): Json<Settings>,
) -> impl IntoResponse {
    // Server-side validation: clamp to match frontend constraints (100–50000)
    settings.terminal_scrollback = settings.terminal_scrollback.clamp(100, 50000);
    // Validate keybar_position: reject NaN/Infinity to prevent persistent layout breakage
    if let Some(ref mut pos) = settings.keybar_position {
        if !pos.left.is_finite() {
            pos.left = 0.0;
        }
        if !pos.top.is_finite() {
            pos.top = 0.0;
        }
        // F011: Clamp bounds — generous enough for multi-monitor setups (8K×3 ≈ 23040px)
        pos.left = pos.left.clamp(-10000.0, 100000.0);
        pos.top = pos.top.clamp(-10000.0, 100000.0);
        // Validate collapse_side: only "left" or "right" allowed
        if pos.collapse_side != "left" && pos.collapse_side != "right" {
            pos.collapse_side = "right".to_string();
        }
        // Validate orientation: only "horizontal" or "vertical" allowed
        if pos.orientation != "horizontal" && pos.orientation != "vertical" {
            pos.orientation = "horizontal".to_string();
        }
    }
    // Validate snippets: limit count, label/command length, reject empty
    if let Some(ref snips) = settings.snippets {
        if snips.len() > 100 {
            return (StatusCode::UNPROCESSABLE_ENTITY, "too many snippets").into_response();
        }
        for s in snips {
            if s.label.trim().is_empty() || s.command.trim().is_empty() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "snippet label/command required",
                )
                    .into_response();
            }
            if s.label.chars().count() > 50 {
                return (StatusCode::UNPROCESSABLE_ENTITY, "snippet label too long")
                    .into_response();
            }
            if s.command.len() > 10_000 {
                return (StatusCode::UNPROCESSABLE_ENTITY, "snippet command too long")
                    .into_response();
            }
        }
    }
    // Validate ssh_bookmarks (auth_type is enum — invalid values rejected by serde)
    if let Some(ref bookmarks) = settings.ssh_bookmarks {
        if bookmarks.len() > 50 {
            tracing::warn!("ssh_bookmarks validation: too many bookmarks");
            return (StatusCode::UNPROCESSABLE_ENTITY, "too many ssh bookmarks").into_response();
        }
        for b in bookmarks {
            if b.label.trim().is_empty() || b.host.trim().is_empty() || b.username.trim().is_empty()
            {
                tracing::warn!("ssh_bookmarks validation: empty required field");
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "bookmark label/host/username required",
                )
                    .into_response();
            }
            if b.label.chars().count() > 50 {
                return (StatusCode::UNPROCESSABLE_ENTITY, "bookmark label too long")
                    .into_response();
            }
            if b.host.len() > 255 {
                return (StatusCode::UNPROCESSABLE_ENTITY, "bookmark host too long")
                    .into_response();
            }
            if b.username.len() > 255 {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "bookmark username too long",
                )
                    .into_response();
            }
            if b.key_path.as_deref().is_some_and(|p| p.len() > 4096) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "bookmark key_path too long",
                )
                    .into_response();
            }
            if b.initial_dir.as_deref().is_some_and(|d| d.len() > 4096) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "bookmark initial_dir too long",
                )
                    .into_response();
            }
        }
    }
    // Validate den_bookmarks
    if let Some(ref bookmarks) = settings.den_bookmarks {
        if bookmarks.len() > 50 {
            return (StatusCode::UNPROCESSABLE_ENTITY, "too many den bookmarks").into_response();
        }
        for b in bookmarks {
            if b.url.trim().is_empty() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "den bookmark url required",
                )
                    .into_response();
            }
            if b.url.len() > 2048 {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "den bookmark url too long",
                )
                    .into_response();
            }
            if b.relay_url.as_deref().is_some_and(|u| u.len() > 2048) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "den bookmark relay_url too long",
                )
                    .into_response();
            }
        }
    }
    // Validate mcp_servers
    if let Some(ref servers) = settings.mcp_servers {
        if servers.len() > 20 {
            return (StatusCode::UNPROCESSABLE_ENTITY, "too many MCP servers").into_response();
        }
        let mut seen_names = std::collections::HashSet::new();
        for srv in servers {
            if srv.name.trim().is_empty() || srv.command.trim().is_empty() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "MCP server name/command required",
                )
                    .into_response();
            }
            if srv.name.len() > 64
                || !srv
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return (StatusCode::UNPROCESSABLE_ENTITY, "invalid MCP server name")
                    .into_response();
            }
            if srv.name.starts_with("den-") {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "MCP server name must not start with 'den-'",
                )
                    .into_response();
            }
            if !seen_names.insert(&srv.name) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "duplicate MCP server name",
                )
                    .into_response();
            }
            if srv.command.len() > 4096 {
                return (StatusCode::UNPROCESSABLE_ENTITY, "MCP command too long").into_response();
            }
            if srv.args.len() > 64 || srv.args.iter().any(|a| a.len() > 4096) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "MCP args too many or too long",
                )
                    .into_response();
            }
            if srv.env.len() > 50 {
                return (StatusCode::UNPROCESSABLE_ENTITY, "too many MCP env vars").into_response();
            }
            for (k, v) in &srv.env {
                if k.is_empty()
                    || k.len() > 256
                    || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    || k.starts_with(|c: char| c.is_ascii_digit())
                {
                    return (StatusCode::UNPROCESSABLE_ENTITY, "invalid MCP env key")
                        .into_response();
                }
                if v.len() > 4096 {
                    return (StatusCode::UNPROCESSABLE_ENTITY, "MCP env value too long")
                        .into_response();
                }
            }
        }
    }
    // sleep_prevention_mode: enum 化により serde が不正値を拒否（422 を返す）
    settings.sleep_prevention_timeout = settings.sleep_prevention_timeout.clamp(1, 480);

    // Encrypt bookmark passwords before saving to disk
    let key = derive_bookmark_key(&state.config.password);
    encrypt_den_bookmarks(&mut settings, &key);

    let store = state.store.clone();
    let sleep_mode = settings.sleep_prevention_mode;
    let sleep_timeout = settings.sleep_prevention_timeout;
    match tokio::task::spawn_blocking(move || store.save_settings(&settings)).await {
        Ok(Ok(())) => {
            state
                .registry
                .update_sleep_config(sleep_mode, sleep_timeout)
                .await;
            StatusCode::OK.into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to save settings: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            tracing::error!("save_settings task panicked: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// --- Keep Awake API ---

#[derive(Deserialize)]
pub struct KeepAwakeRequest {
    pub enabled: bool,
}

#[derive(Serialize)]
struct KeepAwakeResponse {
    enabled: bool,
}

/// GET /api/keep-awake
pub async fn get_keep_awake(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(KeepAwakeResponse {
        enabled: state.registry.is_force_awake(),
    })
}

/// PUT /api/keep-awake
pub async fn put_keep_awake(
    State(state): State<Arc<AppState>>,
    Json(req): Json<KeepAwakeRequest>,
) -> impl IntoResponse {
    tracing::info!(enabled = req.enabled, "keep-awake toggled via API");
    state.registry.set_force_awake(req.enabled).await;
    Json(KeepAwakeResponse {
        enabled: req.enabled,
    })
}
