use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

/// トークン有効期限（秒）: 24時間
const TOKEN_TTL_SECS: u64 = 24 * 60 * 60;

/// レートリミット: ウィンドウ内の最大ログイン試行回数
const MAX_LOGIN_ATTEMPTS: usize = 5;
/// レートリミット: スライディングウィンドウ（秒）
const RATE_LIMIT_WINDOW_SECS: u64 = 60;

/// ログイン試行のグローバルレートリミッター（スライディングウィンドウ方式）
/// 単一パスワード認証のため、IP 単位ではなくグローバルで制限する。
pub struct LoginRateLimiter {
    attempts: Mutex<VecDeque<Instant>>,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl LoginRateLimiter {
    pub fn new() -> Self {
        Self {
            attempts: Mutex::new(VecDeque::new()),
        }
    }

    /// レートリミット内であれば true を返す（記録はしない）
    pub fn check(&self) -> bool {
        let mut attempts = self.attempts.lock().expect("rate limiter lock poisoned");
        let window = std::time::Duration::from_secs(RATE_LIMIT_WINDOW_SECS);
        let now = Instant::now();

        // ウィンドウ外の古いエントリを削除
        while let Some(front) = attempts.front() {
            if now.duration_since(*front) > window {
                attempts.pop_front();
            } else {
                break;
            }
        }

        attempts.len() < MAX_LOGIN_ATTEMPTS
    }

    /// 失敗した試行を記録する
    pub fn record_failure(&self) {
        let mut attempts = self.attempts.lock().expect("rate limiter lock poisoned");
        attempts.push_back(Instant::now());
    }
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginSuccess {
    pub ok: bool,
}

/// パスワードと発行時刻からトークンを生成（HMAC-SHA256 + タイムスタンプ）
/// フォーマット: "{issued_at_unix_hex}.{hmac_hex}"
pub fn generate_token(password: &str, secret: &[u8]) -> String {
    let issued_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();
    generate_token_at(password, secret, issued_at)
}

/// 指定時刻でトークン生成（テスト用にも公開）
pub fn generate_token_at(password: &str, secret: &[u8], issued_at: u64) -> String {
    let timestamp_hex = format!("{:x}", issued_at);
    let sig = compute_hmac(password, secret, issued_at);
    format!("{}.{}", timestamp_hex, sig)
}

/// トークンを検証（HMAC チェック + 有効期限チェック）
pub fn validate_token(token: &str, password: &str, secret: &[u8]) -> bool {
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
    let expected = compute_hmac(password, secret, issued_at);
    constant_time_eq(sig, &expected)
}

fn compute_hmac(password: &str, secret: &[u8], issued_at: u64) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(password.as_bytes());
    mac.update(&issued_at.to_be_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// 定数時間比較（タイミング攻撃対策）
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Cookie name for the auth token (HttpOnly)
const TOKEN_COOKIE: &str = "den_token";
/// Cookie name for the login flag (readable by JS for isLoggedIn check)
const LOGGED_IN_COOKIE: &str = "den_logged_in";

/// ログイン API
/// トークンは HttpOnly Cookie で設定。レスポンスボディは `{"ok": true}` のみ。
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, StatusCode> {
    if !state.rate_limiter.check() {
        tracing::warn!("Login rate limited");
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    if req.password == state.config.password {
        let token = generate_token(&state.config.password, &state.hmac_secret);
        tracing::info!("Login successful");

        let mut headers = HeaderMap::new();
        // HttpOnly Cookie: JS からアクセス不可（XSS 対策）
        let token_cookie = format!(
            "{}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
            TOKEN_COOKIE, token, TOKEN_TTL_SECS
        );
        headers.insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&token_cookie).expect("valid cookie value"),
        );
        // Flag Cookie: JS から isLoggedIn() チェック用（トークン値は含まない）
        let flag_cookie = format!(
            "{}=1; SameSite=Strict; Path=/; Max-Age={}",
            LOGGED_IN_COOKIE, TOKEN_TTL_SECS
        );
        headers.append(
            header::SET_COOKIE,
            HeaderValue::from_str(&flag_cookie).expect("valid cookie value"),
        );

        Ok((headers, Json(LoginSuccess { ok: true })).into_response())
    } else {
        state.rate_limiter.record_failure();
        tracing::warn!("Login failed: incorrect password");
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// ログアウト API
/// HttpOnly Cookie `den_token` と JS フラグ Cookie `den_logged_in` を削除する。
/// 認証不要（無効クッキーの削除は無害）。
pub async fn logout() -> Response {
    let mut headers = HeaderMap::new();
    let token_cookie = format!(
        "{}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        TOKEN_COOKIE
    );
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&token_cookie).expect("valid cookie value"),
    );
    let flag_cookie = format!("{}=; SameSite=Strict; Path=/; Max-Age=0", LOGGED_IN_COOKIE);
    headers.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&flag_cookie).expect("valid cookie value"),
    );
    (StatusCode::NO_CONTENT, headers).into_response()
}

/// Cookie ヘッダーから指定名の値を抽出
fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            let prefix = format!("{}=", name);
            cookies
                .split(';')
                .map(|c| c.trim())
                .find(|c| c.starts_with(&prefix))
                .map(|c| c[prefix.len()..].to_string())
        })
}

/// トークン認証ミドルウェア
/// 認証ソース（優先順）:
/// 1. Authorization: Bearer <token> ヘッダー（API クライアント・テスト用）
/// 2. den_token Cookie（ブラウザ用、HttpOnly）
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    // Authorization ヘッダーからトークンを取得（優先）
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        // フォールバック: Cookie からトークンを取得
        .or_else(|| extract_cookie(req.headers(), TOKEN_COOKIE));

    match token {
        Some(t) if validate_token(&t, &state.config.password, &state.hmac_secret) => {
            next.run(req).await
        }
        _ => {
            tracing::debug!("Auth rejected: {path}");
            StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

/// Content-Security-Policy ミドルウェア
/// script-src 'self' で外部スクリプト注入を防止し、XSS リスクを軽減する。
pub async fn csp_middleware(req: Request<axum::body::Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss:; img-src 'self' data: blob:",
        ),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &[u8] = b"test-secret-key-for-unit-tests!!";

    #[test]
    fn token_roundtrip() {
        let token = generate_token("password", TEST_SECRET);
        assert!(validate_token(&token, "password", TEST_SECRET));
    }

    #[test]
    fn token_wrong_password_fails() {
        let token = generate_token("password", TEST_SECRET);
        assert!(!validate_token(&token, "wrong", TEST_SECRET));
    }

    #[test]
    fn token_wrong_secret_fails() {
        let token = generate_token("password", TEST_SECRET);
        assert!(!validate_token(&token, "password", b"different-secret"));
    }

    #[test]
    fn token_format() {
        let token = generate_token("test", TEST_SECRET);
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
        let token = generate_token_at("password", TEST_SECRET, old_time);
        assert!(!validate_token(&token, "password", TEST_SECRET));
    }

    #[test]
    fn token_not_yet_expired() {
        // 23時間前のトークン（まだ有効）
        let recent_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 23 * 60 * 60;
        let token = generate_token_at("password", TEST_SECRET, recent_time);
        assert!(validate_token(&token, "password", TEST_SECRET));
    }

    #[test]
    fn token_tampered_signature() {
        let mut token = generate_token("test", TEST_SECRET);
        // 署名の末尾を改ざん
        let last = token.pop().unwrap();
        let replacement = if last == '0' { '1' } else { '0' };
        token.push(replacement);
        assert!(!validate_token(&token, "test", TEST_SECRET));
    }

    #[test]
    fn token_tampered_timestamp() {
        let token = generate_token("test", TEST_SECRET);
        let parts: Vec<&str> = token.split('.').collect();
        // タイムスタンプを改ざん
        let tampered = format!("ff{}.{}", parts[0], parts[1]);
        assert!(!validate_token(&tampered, "test", TEST_SECRET));
    }

    #[test]
    fn token_invalid_format() {
        assert!(!validate_token("not-a-token", "password", TEST_SECRET));
        assert!(!validate_token("", "password", TEST_SECRET));
        assert!(!validate_token("abc.def.ghi", "password", TEST_SECRET));
    }

    #[test]
    fn extract_cookie_single() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "den_token=abc123".parse().unwrap());
        assert_eq!(extract_cookie(&headers, "den_token"), Some("abc123".into()));
    }

    #[test]
    fn extract_cookie_multiple() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "other=x; den_token=abc123; den_logged_in=1"
                .parse()
                .unwrap(),
        );
        assert_eq!(extract_cookie(&headers, "den_token"), Some("abc123".into()));
        assert_eq!(extract_cookie(&headers, "den_logged_in"), Some("1".into()));
    }

    #[test]
    fn extract_cookie_missing() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "other=x".parse().unwrap());
        assert_eq!(extract_cookie(&headers, "den_token"), None);
    }

    #[test]
    fn extract_cookie_no_header() {
        let headers = HeaderMap::new();
        assert_eq!(extract_cookie(&headers, "den_token"), None);
    }

    #[test]
    fn rate_limiter_check_does_not_count() {
        let limiter = LoginRateLimiter::new();
        // check() を何度呼んでもカウントは増えない
        for _ in 0..10 {
            assert!(limiter.check());
        }
    }

    #[test]
    fn rate_limiter_record_failure_counts() {
        let limiter = LoginRateLimiter::new();
        // 5回失敗を記録 → check() が false になる
        for _ in 0..5 {
            assert!(limiter.check());
            limiter.record_failure();
        }
        assert!(!limiter.check());
    }
}
