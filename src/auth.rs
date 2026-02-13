use axum::{
    Json,
    extract::State,
    http::{Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

/// トークン有効期限（秒）: 24時間
const TOKEN_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
}

/// パスワードと発行時刻からトークンを生成（HMAC-SHA256 + タイムスタンプ）
/// フォーマット: "{issued_at_unix_hex}.{hmac_hex}"
pub fn generate_token(password: &str) -> String {
    let issued_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();
    generate_token_at(password, issued_at)
}

/// 指定時刻でトークン生成（テスト用にも公開）
pub fn generate_token_at(password: &str, issued_at: u64) -> String {
    let timestamp_hex = format!("{:x}", issued_at);
    let sig = compute_hmac(password, issued_at);
    format!("{}.{}", timestamp_hex, sig)
}

/// トークンを検証（HMAC チェック + 有効期限チェック）
pub fn validate_token(token: &str, password: &str) -> bool {
    let Some((timestamp_hex, sig)) = token.split_once('.') else {
        return false;
    };

    let Ok(issued_at) = u64::from_str_radix(timestamp_hex, 16) else {
        return false;
    };

    // 有効期限チェック
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();

    if now.saturating_sub(issued_at) > TOKEN_TTL_SECS {
        return false;
    }

    // HMAC 検証
    let expected = compute_hmac(password, issued_at);
    constant_time_eq(sig, &expected)
}

fn compute_hmac(password: &str, issued_at: u64) -> String {
    let mut mac =
        HmacSha256::new_from_slice(b"den-secret-key").expect("HMAC accepts any key length");
    mac.update(password.as_bytes());
    mac.update(&issued_at.to_be_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// 定数時間比較（タイミング攻撃対策）
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// ログイン API
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    if req.password == state.config.password {
        let token = generate_token(&state.config.password);
        Ok(Json(LoginResponse { token }))
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// トークン認証ミドルウェア
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // Authorization ヘッダーからトークンを取得
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        // クエリパラメータからも取得（WebSocket 用）
        .or_else(|| {
            req.uri()
                .query()
                .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("token=")))
                .map(|s| s.to_string())
        });

    match token {
        Some(t) if validate_token(&t, &state.config.password) => next.run(req).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip() {
        let token = generate_token("password");
        assert!(validate_token(&token, "password"));
    }

    #[test]
    fn token_wrong_password_fails() {
        let token = generate_token("password");
        assert!(!validate_token(&token, "wrong"));
    }

    #[test]
    fn token_format() {
        let token = generate_token("test");
        assert!(token.contains('.'));
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 2);
        // timestamp part is hex
        assert!(u64::from_str_radix(parts[0], 16).is_ok());
        // signature part is hex
        assert!(parts[1].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(parts[1].len(), 64); // HMAC-SHA256 = 64 hex chars
    }

    #[test]
    fn token_expired() {
        // 25時間前のトークン
        let old_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 25 * 60 * 60;
        let token = generate_token_at("password", old_time);
        assert!(!validate_token(&token, "password"));
    }

    #[test]
    fn token_not_yet_expired() {
        // 23時間前のトークン（まだ有効）
        let recent_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 23 * 60 * 60;
        let token = generate_token_at("password", recent_time);
        assert!(validate_token(&token, "password"));
    }

    #[test]
    fn token_tampered_signature() {
        let mut token = generate_token("test");
        // 署名の末尾を改ざん
        let last = token.pop().unwrap();
        let replacement = if last == '0' { '1' } else { '0' };
        token.push(replacement);
        assert!(!validate_token(&token, "test"));
    }

    #[test]
    fn token_tampered_timestamp() {
        let token = generate_token("test");
        let parts: Vec<&str> = token.split('.').collect();
        // タイムスタンプを改ざん
        let tampered = format!("ff{}.{}", parts[0], parts[1]);
        assert!(!validate_token(&tampered, "test"));
    }

    #[test]
    fn token_invalid_format() {
        assert!(!validate_token("not-a-token", "password"));
        assert!(!validate_token("", "password"));
        assert!(!validate_token("abc.def.ghi", "password"));
    }
}
