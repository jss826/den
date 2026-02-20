pub mod assets;
pub mod auth;
pub mod config;
pub mod filer;
pub mod pty;
pub mod sftp;
pub mod ssh;
pub mod store;
pub mod store_api;
pub mod ws;

use axum::{
    Router, middleware,
    routing::{delete, get, post, put},
};
use config::Config;
use pty::registry::SessionRegistry;
use std::sync::Arc;
use store::Store;

pub struct AppState {
    pub config: Config,
    pub store: Store,
    pub registry: Arc<SessionRegistry>,
    pub hmac_secret: Vec<u8>,
    pub rate_limiter: auth::LoginRateLimiter,
    pub sftp_manager: sftp::client::SftpManager,
}

/// アプリケーション Router を構築（テストからも利用可能）
pub fn create_app(config: Config, registry: Arc<SessionRegistry>) -> Router {
    // 起動ごとにランダムな HMAC シークレットを生成
    // 再起動で全トークンが無効化される（セキュリティ上望ましい）
    let hmac_secret: Vec<u8> = rand::random::<[u8; 32]>().to_vec();
    create_app_with_secret(config, registry, hmac_secret)
}

/// テスト用: 固定シークレットで Router を構築
pub fn create_app_with_secret(
    config: Config,
    registry: Arc<SessionRegistry>,
    hmac_secret: Vec<u8>,
) -> Router {
    let store = Store::from_data_dir(&config.data_dir).expect("Failed to initialize data store");

    // NOTE: 永続化状態を追加する場合は、ここでスタートアップ時の整合性チェックを実装すること。
    // 例: 前回の異常終了で中断状態のままのリソースをリセットする（orphaned state cleanup）。
    // 以前はセッション永続化に対して store.cleanup_stale_running_sessions() を呼んでいた。

    let state = Arc::new(AppState {
        config,
        store,
        registry,
        hmac_secret,
        rate_limiter: auth::LoginRateLimiter::new(),
        sftp_manager: sftp::client::SftpManager::new(),
    });

    // 認証不要のルート
    let public_routes = Router::new()
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/", get(assets::serve_index))
        .route("/{*path}", get(assets::serve_static));

    // 認証必要のルート（Cookie / Authorization ヘッダーで認証）
    let protected_routes = Router::new()
        .route("/api/settings", get(store_api::get_settings))
        .route("/api/settings", put(store_api::put_settings))
        // WebSocket: Cookie 認証（ブラウザが自動で Cookie を送信）
        .route("/api/ws", get(ws::ws_handler))
        // Terminal session management API
        .route(
            "/api/terminal/sessions",
            get(ws::list_sessions).post(ws::create_session),
        )
        .route("/api/terminal/sessions/{name}", delete(ws::destroy_session))
        // Filer API
        .route("/api/filer/list", get(filer::api::list))
        .route("/api/filer/read", get(filer::api::read))
        .route("/api/filer/write", put(filer::api::write))
        .route("/api/filer/mkdir", post(filer::api::mkdir))
        .route("/api/filer/rename", post(filer::api::rename))
        .route("/api/filer/delete", delete(filer::api::delete))
        .route("/api/filer/download", get(filer::api::download))
        .route("/api/filer/upload", post(filer::api::upload))
        .route("/api/filer/search", get(filer::api::search))
        // SFTP API
        .route("/api/sftp/connect", post(sftp::api::connect))
        .route("/api/sftp/status", get(sftp::api::status))
        .route("/api/sftp/disconnect", post(sftp::api::disconnect))
        .route("/api/sftp/list", get(sftp::api::list))
        .route("/api/sftp/read", get(sftp::api::read))
        .route("/api/sftp/write", put(sftp::api::write))
        .route("/api/sftp/mkdir", post(sftp::api::mkdir))
        .route("/api/sftp/rename", post(sftp::api::rename))
        .route("/api/sftp/delete", delete(sftp::api::delete))
        .route("/api/sftp/download", get(sftp::api::download))
        .route("/api/sftp/upload", post(sftp::api::upload))
        .route("/api/sftp/search", get(sftp::api::search))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::auth_middleware,
        ));

    Router::new()
        .merge(protected_routes)
        .merge(public_routes)
        // CSP ヘッダーを全レスポンスに付与（XSS 防止）
        .layer(middleware::from_fn(auth::csp_middleware))
        .with_state(state)
}
