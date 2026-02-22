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
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new("powershell.exe".to_string(), SleepPreventionMode::Off, 30);
    den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store)
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
    let registry = SessionRegistry::new("powershell.exe".to_string(), SleepPreventionMode::Off, 30);
    let app = den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store);

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
    let registry = SessionRegistry::new("powershell.exe".to_string(), SleepPreventionMode::Off, 30);
    let app = den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store);

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
    let registry = SessionRegistry::new("powershell.exe".to_string(), SleepPreventionMode::Off, 30);
    let app = den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store);

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
    let registry = SessionRegistry::new("powershell.exe".to_string(), SleepPreventionMode::Off, 30);
    let app = den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store);

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
    let registry = SessionRegistry::new("powershell.exe".to_string(), SleepPreventionMode::Off, 30);
    let app = den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store);

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
