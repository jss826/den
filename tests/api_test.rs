use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use den::auth::generate_token;
use den::config::{Config, Environment};
use den::pty::registry::SessionRegistry;
use den::store::SleepPreventionMode;
use http_body_util::BodyExt;
use tower::ServiceExt;

use std::sync::atomic::{AtomicU32, Ordering};

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

const TEST_HMAC_SECRET: &[u8] = b"test-secret-key-for-integration!";

fn test_config() -> Config {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("den-test-{}-{}", std::process::id(), id));
    Config {
        port: 0,
        password: "testpass".to_string(),
        shell: "powershell.exe".to_string(),
        env: Environment::Development,
        log_level: "debug".to_string(),
        data_dir: tmp.to_string_lossy().to_string(),
        bind_address: "127.0.0.1".to_string(),
        ssh_port: None,
    }
}

fn test_app() -> axum::Router {
    test_app_with_state().0
}

fn test_app_with_state() -> (axum::Router, std::sync::Arc<den::AppState>) {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        std::sync::Arc::new(den::peer::PeerRegistry::new()),
    )
}

fn auth_header() -> String {
    format!("Bearer {}", generate_token("testpass", TEST_HMAC_SECRET))
}

// --- POST /api/login ---

#[tokio::test]
async fn login_correct_password() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"testpass"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify Set-Cookie headers are present
    let cookies: Vec<&str> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(cookies.iter().any(|c| c.starts_with("den_token=")));
    assert!(cookies.iter().any(|c| c.starts_with("den_logged_in=")));

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["ok"], true);
    // Token is no longer in response body (HttpOnly Cookie only)
    assert!(json.get("token").is_none());
}

#[tokio::test]
async fn login_wrong_password() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"wrong"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_no_body() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // axum returns 422 for deserialization failure
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn login_rate_limit() {
    let app = test_app();

    // 成功ログインはカウントされないことを検証（3回成功）
    for _ in 0..3 {
        let req = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"password":"testpass"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // 5回の失敗試行（MAX_LOGIN_ATTEMPTS = 5）— すべて 401
    for _ in 0..5 {
        let req = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"password":"wrong"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // 6回目 — レートリミットで 429
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"wrong"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

    // 正しいパスワードでも 429
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"testpass"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

// --- Auth middleware ---

#[tokio::test]
async fn auth_middleware_no_token() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_middleware_invalid_token() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, "Bearer invalidtoken")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_middleware_valid_token() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn ws_endpoint_requires_auth() {
    // /api/ws is protected by auth_middleware (Cookie / Authorization header).
    // Without auth, returns UNAUTHORIZED before WS upgrade.
    let app = test_app();
    let req = Request::builder()
        .uri("/api/ws")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- Static files ---

#[tokio::test]
async fn static_index() {
    let app = test_app();
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/html"));
}

#[tokio::test]
async fn static_js() {
    let app = test_app();
    let req = Request::builder()
        .uri("/js/app.js")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("javascript"));
}

#[tokio::test]
async fn static_css() {
    let app = test_app();
    let req = Request::builder()
        .uri("/css/style.css")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("css"));
}

#[tokio::test]
async fn static_404() {
    let app = test_app();
    let req = Request::builder()
        .uri("/nonexistent.xyz")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// --- Settings API ---

#[tokio::test]
async fn settings_get_default() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["font_size"], 14);
    assert_eq!(json["theme"], "dark");
    assert_eq!(json["terminal_scrollback"], 1000);
}

#[tokio::test]
async fn settings_put_and_get() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) = den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        std::sync::Arc::new(den::peer::PeerRegistry::new()),
    );

    // PUT
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"font_size":20,"theme":"dark","terminal_scrollback":2000}"#,
        ))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // GET
    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["font_size"], 20);
    assert_eq!(json["terminal_scrollback"], 2000);
}

#[tokio::test]
async fn settings_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- Settings API: edge cases ---

#[tokio::test]
async fn settings_put_invalid_json() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from("not json"))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn settings_put_partial_json() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) = den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        std::sync::Arc::new(den::peer::PeerRegistry::new()),
    );

    // PUT with only some fields — serde should use defaults for missing fields
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"font_size":18}"#))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    // If Settings has serde defaults, this should succeed (200)
    // If not, it should be 422 (missing fields)
    let status = resp.status();
    assert!(status == StatusCode::OK || status == StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_put_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"font_size":20,"theme":"dark","terminal_scrollback":2000}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- SSH Bookmarks validation ---

#[tokio::test]
async fn settings_ssh_bookmarks_roundtrip() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) = den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        std::sync::Arc::new(den::peer::PeerRegistry::new()),
    );

    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"ssh_bookmarks":[{"label":"myserver","host":"example.com","port":22,"username":"user","auth_type":"key","key_path":"~/.ssh/id_rsa"}]}"#,
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let bookmarks = json["ssh_bookmarks"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0]["label"], "myserver");
    assert_eq!(bookmarks[0]["host"], "example.com");
    assert_eq!(bookmarks[0]["auth_type"], "key");
    assert_eq!(bookmarks[0]["key_path"], "~/.ssh/id_rsa");
}

#[tokio::test]
async fn settings_ssh_bookmarks_too_many() {
    let app = test_app();
    let bookmarks: Vec<serde_json::Value> = (0..51)
        .map(|i| {
            serde_json::json!({
                "label": format!("host-{i}"),
                "host": "example.com",
                "username": "user",
                "auth_type": "password"
            })
        })
        .collect();
    let body = serde_json::json!({ "ssh_bookmarks": bookmarks }).to_string();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_empty_label() {
    let app = test_app();
    let body = r#"{"ssh_bookmarks":[{"label":"","host":"example.com","username":"user","auth_type":"password"}]}"#;
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_label_too_long() {
    let app = test_app();
    let long_label = "a".repeat(51);
    let body = serde_json::json!({
        "ssh_bookmarks": [{"label": long_label, "host": "example.com", "username": "user", "auth_type": "password"}]
    })
    .to_string();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_host_too_long() {
    let app = test_app();
    let long_host = "a".repeat(256);
    let body = serde_json::json!({
        "ssh_bookmarks": [{"label": "test", "host": long_host, "username": "user", "auth_type": "password"}]
    })
    .to_string();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_invalid_auth_type() {
    let app = test_app();
    let body = r#"{"ssh_bookmarks":[{"label":"test","host":"example.com","username":"user","auth_type":"invalid"}]}"#;
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // auth_type is now an enum — invalid values are rejected by serde (422)
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_username_too_long() {
    let app = test_app();
    let long_username = "u".repeat(256);
    let body = serde_json::json!({
        "ssh_bookmarks": [{"label": "test", "host": "example.com", "username": long_username, "auth_type": "password"}]
    })
    .to_string();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// --- Terminal REST API ---

#[tokio::test]
async fn terminal_sessions_list_empty() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/terminal/sessions")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn terminal_sessions_create_invalid_name() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/terminal/sessions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"name":"../invalid"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn terminal_sessions_destroy_nonexistent() {
    let app = test_app();
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/terminal/sessions/nonexistent")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // destroy is idempotent — returns 204 even if not found
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn terminal_sessions_rename_invalid_name() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/terminal/sessions/old-name")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"name":"bad name!"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn terminal_sessions_rename_not_found() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/terminal/sessions/nonexistent")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"name":"new-name"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn terminal_sessions_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/terminal/sessions")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- POST /api/logout ---

#[tokio::test]
async fn logout_clears_cookies() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/logout")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let cookies: Vec<&str> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        cookies
            .iter()
            .any(|c| c.contains("den_token=") && c.contains("Max-Age=0"))
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.contains("den_logged_in=") && c.contains("Max-Age=0"))
    );
}

#[tokio::test]
async fn logout_without_auth() {
    // logout は認証不要（無効クッキー削除は無害）
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/logout")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

// --- SFTP API ---

#[tokio::test]
async fn sftp_status_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/status")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["connected"], false);
    assert!(json["host"].is_null());
    assert!(json["username"].is_null());
}

#[tokio::test]
async fn sftp_disconnect_when_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/disconnect")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn sftp_list_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/list?path=/&show_hidden=false")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_read_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/read?path=/tmp/test.txt")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_write_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/sftp/write")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"path":"/tmp/test.txt","content":"hello"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_mkdir_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/mkdir")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"path":"/tmp/newdir"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_rename_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/rename")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"from":"/tmp/a","to":"/tmp/b"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_delete_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/sftp/delete?path=/tmp/test.txt")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_download_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/download?path=/tmp/test.txt")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_search_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/search?path=/&query=test&content=false")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_connect_missing_fields() {
    let app = test_app();
    // host と username は必須
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"auth_type":"password"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // axum deserialization error → 422
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn sftp_connect_invalid_auth_type() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"host":"example.com","username":"user","auth_type":"invalid"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sftp_connect_password_missing() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"host":"example.com","username":"user","auth_type":"password"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sftp_connect_key_path_missing() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"host":"example.com","username":"user","auth_type":"key"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sftp_requires_auth() {
    let app = test_app();
    // 全 SFTP エンドポイントは認証必須
    for uri in [
        "/api/sftp/status",
        "/api/sftp/list?path=/&show_hidden=false",
        "/api/sftp/read?path=/test",
        "/api/sftp/search?path=/&query=test&content=false",
    ] {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "GET {} should require auth",
            uri
        );
    }

    for uri in [
        "/api/sftp/connect",
        "/api/sftp/disconnect",
        "/api/sftp/mkdir",
        "/api/sftp/rename",
        "/api/sftp/upload",
    ] {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "POST {} should require auth",
            uri
        );
    }

    // PUT
    let req = Request::builder()
        .method("PUT")
        .uri("/api/sftp/write")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "PUT /api/sftp/write should require auth"
    );

    // DELETE
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/sftp/delete?path=/test")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "DELETE /api/sftp/delete should require auth"
    );

    // GET download
    let req = Request::builder()
        .uri("/api/sftp/download?path=/test")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "GET /api/sftp/download should require auth"
    );
}

#[tokio::test]
async fn sftp_upload_not_connected() {
    let app = test_app();
    let boundary = "----TestBoundary";
    let body = format!(
        "------TestBoundary\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\n/tmp\r\n------TestBoundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n------TestBoundary--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/upload")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={}", boundary),
        )
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_write_empty_path() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/sftp/write")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"path":"","content":"hello"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sftp_write_null_byte_path() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/sftp/write")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"path":"/tmp/\u0000evil.txt","content":"hello"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// --- Clipboard History API ---

#[tokio::test]
async fn clipboard_history_get_empty() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/clipboard-history")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn clipboard_history_post_and_get() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) = den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        std::sync::Arc::new(den::peer::PeerRegistry::new()),
    );

    // POST
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"hello world","source":"copy"}"#))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["text"], "hello world");
    assert_eq!(arr[0]["source"], "copy");

    // GET
    let req = Request::builder()
        .uri("/api/clipboard-history")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn clipboard_history_dedup() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) = den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        std::sync::Arc::new(den::peer::PeerRegistry::new()),
    );

    // Add two entries
    for text in ["first", "second"] {
        let req = Request::builder()
            .method("POST")
            .uri("/api/clipboard-history")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, auth_header())
            .body(Body::from(format!(
                r#"{{"text":"{text}","source":"copy"}}"#
            )))
            .unwrap();
        app.clone().oneshot(req).await.unwrap();
    }

    // Add "first" again — should deduplicate
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"first","source":"osc52"}"#))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["text"], "first");
    assert_eq!(arr[0]["source"], "osc52");
    assert_eq!(arr[1]["text"], "second");
}

#[tokio::test]
async fn clipboard_history_delete() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) = den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        std::sync::Arc::new(den::peer::PeerRegistry::new()),
    );

    // Add an entry
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"hello","source":"copy"}"#))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    // DELETE
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/clipboard-history")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // GET should be empty
    let req = Request::builder()
        .uri("/api/clipboard-history")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn clipboard_history_requires_auth() {
    let app = test_app();

    // GET
    let req = Request::builder()
        .uri("/api/clipboard-history")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // POST
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"text":"test","source":"copy"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // DELETE
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/clipboard-history")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn clipboard_history_post_empty_text_rejected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"","source":"copy"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn clipboard_history_post_invalid_source_rejected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"test","source":"invalid"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// --- SFTP Agent auth ---

#[tokio::test]
async fn sftp_connect_agent_unavailable() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"host":"127.0.0.1","port":1,"username":"user","auth_type":"agent"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // Agent may or may not be running; either way connection to 127.0.0.1:1 will fail
    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "Expected error status, got {}",
        resp.status()
    );
}

// --- Keep Awake API ---

#[tokio::test]
async fn keep_awake_get_default() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/keep-awake")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], false);
}

#[tokio::test]
async fn keep_awake_put_and_get() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) = den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        std::sync::Arc::new(den::peer::PeerRegistry::new()),
    );

    // PUT true — response body should confirm the state
    let req = Request::builder()
        .method("PUT")
        .uri("/api/keep-awake")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"enabled":true}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], true);

    // GET — should be true
    let req = Request::builder()
        .uri("/api/keep-awake")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], true);

    // PUT false — response body should confirm the state
    let req = Request::builder()
        .method("PUT")
        .uri("/api/keep-awake")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"enabled":false}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], false);

    // GET — should be false
    let req = Request::builder()
        .uri("/api/keep-awake")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], false);
}

#[tokio::test]
async fn keep_awake_requires_auth() {
    let app = test_app();

    // GET
    let req = Request::builder()
        .uri("/api/keep-awake")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // PUT
    let req = Request::builder()
        .method("PUT")
        .uri("/api/keep-awake")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"enabled":true}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- Peer API tests ---

#[tokio::test]
async fn peers_requires_auth() {
    let app = test_app();

    let req = Request::builder()
        .uri("/api/peers")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/invite")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/join")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"code":"abc","peer_url":"http://x"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn peers_invite_generates_code() {
    let app = test_app();

    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/invite")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let code = json["code"].as_str().unwrap();
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
    assert_eq!(json["expires_in_secs"], 300);
}

#[tokio::test]
async fn peers_pair_invalid_code_rejected() {
    let app = test_app();

    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/pair")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"code":"badcode","name":"test-peer","url":"http://peer:8080","token":"tok123"}"#,
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn peers_pair_valid_code_succeeds() {
    let (app, state) = test_app_with_state();

    let (code, _token) = state.peer_registry.create_invite();

    let body = serde_json::json!({
        "code": code,
        "name": "remote-den",
        "url": "http://192.168.1.10:8080",
        "token": "remote-token-abc"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/pair")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp_body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert!(json["name"].as_str().is_some());
    let token_str = json["token"].as_str().unwrap();
    assert!(!token_str.is_empty());

    let settings = state.store.load_settings();
    let peers = settings.peers.unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].name, "remote-den");
    assert_eq!(peers[0].url, "http://192.168.1.10:8080");
    assert_eq!(peers[0].token, "remote-token-abc");
}

#[tokio::test]
async fn peers_pair_code_consumed_once() {
    let (app, state) = test_app_with_state();

    let (code, _token) = state.peer_registry.create_invite();

    let body = serde_json::json!({
        "code": code,
        "name": "peer-a",
        "url": "http://a:8080",
        "token": "tok-a"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/pair")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = serde_json::json!({
        "code": code,
        "name": "peer-b",
        "url": "http://b:8080",
        "token": "tok-b"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/pair")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn peers_list_empty() {
    let app = test_app();

    let req = Request::builder()
        .uri("/api/peers")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn peers_list_after_pair() {
    let (app, state) = test_app_with_state();

    let (code, _token) = state.peer_registry.create_invite();
    let body = serde_json::json!({
        "code": code,
        "name": "my-peer",
        "url": "http://peer:8080",
        "token": "peer-tok"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/pair")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .uri("/api/peers")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let peers = json.as_array().unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0]["name"], "my-peer");
    assert_eq!(peers[0]["url"], "http://peer:8080");
    assert!(peers[0]["status"].as_str().is_some());
}

#[tokio::test]
async fn peers_delete() {
    let (app, state) = test_app_with_state();

    let (code, _token) = state.peer_registry.create_invite();
    let body = serde_json::json!({
        "code": code,
        "name": "del-peer",
        "url": "http://peer:8080",
        "token": "peer-tok"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/pair")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .method("DELETE")
        .uri("/api/peers/del-peer")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let settings = state.store.load_settings();
    let peers = settings.peers.unwrap_or_default();
    assert!(peers.is_empty());
}

#[tokio::test]
async fn peers_delete_nonexistent() {
    let app = test_app();

    let req = Request::builder()
        .method("DELETE")
        .uri("/api/peers/no-such-peer")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn peers_pair_invalid_name_rejected() {
    let (app, state) = test_app_with_state();

    let (code, _token) = state.peer_registry.create_invite();

    let body = serde_json::json!({
        "code": code,
        "name": "invalid name!",
        "url": "http://peer:8080",
        "token": "tok"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/pair")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn peer_token_authenticates_to_protected_routes() {
    let (app, state) = test_app_with_state();

    let (code, _token) = state.peer_registry.create_invite();
    let body = serde_json::json!({
        "code": code,
        "name": "auth-peer",
        "url": "http://peer:8080",
        "token": "my-secret-peer-token"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/pair")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    let req = Request::builder()
        .uri("/api/system/version")
        .header(header::AUTHORIZATION, "Bearer my-secret-peer-token")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn peer_token_invalid_rejected() {
    let app = test_app();

    let req = Request::builder()
        .uri("/api/system/version")
        .header(header::AUTHORIZATION, "Bearer not-a-valid-peer-token")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn peers_join_bad_url_returns_bad_gateway() {
    let app = test_app();

    let body = serde_json::json!({
        "code": "abc123",
        "peer_url": "http://127.0.0.1:1"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/join")
        .header(header::AUTHORIZATION, auth_header())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn settings_peer_name_roundtrip() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) = den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        std::sync::Arc::new(den::peer::PeerRegistry::new()),
    );

    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"peer_name":"my-den"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["peer_name"], "my-den");
}

#[tokio::test]
async fn peers_full_pairing_e2e() {
    let (app_a, state_a) = test_app_with_state();

    // Step 1: Den A generates invite
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/invite")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app_a.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let invite: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let code = invite["code"].as_str().unwrap().to_string();

    // Step 2: Den B pairs via /api/peers/pair
    let body = serde_json::json!({
        "code": code,
        "name": "den-b",
        "url": "http://den-b:8080",
        "token": "b-token-for-a"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/pair")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app_a.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let pair_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let a_name = pair_resp["name"].as_str().unwrap();
    let a_token = pair_resp["token"].as_str().unwrap();
    assert!(!a_name.is_empty());
    assert!(!a_token.is_empty());

    // Step 3: Den A has Den B in peers
    let settings_a = state_a.store.load_settings();
    let peers_a = settings_a.peers.unwrap();
    assert_eq!(peers_a.len(), 1);
    assert_eq!(peers_a[0].name, "den-b");
    assert_eq!(peers_a[0].token, "b-token-for-a");

    // Step 4: Den B's token authenticates to Den A
    let req = Request::builder()
        .uri("/api/system/version")
        .header(header::AUTHORIZATION, "Bearer b-token-for-a")
        .body(Body::empty())
        .unwrap();
    let resp = app_a.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Step 5: Token Den A returned is valid hex (32 bytes = 64 hex chars)
    assert_eq!(a_token.len(), 64);
    assert!(a_token.chars().all(|c| c.is_ascii_hexdigit()));
}

// --- Peer Terminal Proxy API ---

#[tokio::test]
async fn proxy_list_sessions_unknown_peer_returns_404() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/peers/unknown/terminal/sessions")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn proxy_list_sessions_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/peers/some-peer/terminal/sessions")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn proxy_create_session_unknown_peer_returns_404() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/peers/unknown/terminal/sessions")
        .header(header::AUTHORIZATION, auth_header())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"test"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn proxy_rename_session_unknown_peer_returns_404() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/peers/unknown/terminal/sessions/test")
        .header(header::AUTHORIZATION, auth_header())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"new-name"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn proxy_delete_session_unknown_peer_returns_404() {
    let app = test_app();
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/peers/unknown/terminal/sessions/test")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn proxy_ws_relay_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/peers/some-peer/ws?session=test")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn proxy_list_sessions_with_registered_peer_returns_bad_gateway() {
    // When the peer exists but is unreachable, proxy returns 502
    let (app, state) = test_app_with_state();

    // Register a fake peer with unreachable URL
    let peer = den::store::PeerConfig {
        name: "fake-peer".to_string(),
        url: "http://127.0.0.1:1".to_string(), // unreachable port
        token: "fake-token".to_string(),
    };
    let mut settings = state.store.load_settings();
    settings.peers = Some(vec![peer]);
    state.store.save_settings(&settings).unwrap();

    let req = Request::builder()
        .uri("/api/peers/fake-peer/terminal/sessions")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}
