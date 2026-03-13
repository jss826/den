pub mod assets;
pub mod auth;
pub mod clipboard_api;
pub mod clipboard_monitor;
pub mod config;
pub mod filer;
pub mod port_detection;
pub mod port_forward;
pub mod port_monitor;
pub mod pty;
pub mod remote;
pub mod sftp;
pub mod ssh;
pub mod store;
pub mod store_api;
pub mod tls;
pub mod update;
pub mod ws;

use axum::{
    Router, middleware,
    routing::{any, delete, get, post, put},
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
    pub remote_manager: Arc<remote::RemoteManager>,
    pub relay_manager: remote::RelayManager,
    pub relay_client: remote::RelayClientManager,
    pub tls_info: Option<tls::TlsInfo>,
    pub tls_certificate_der: Option<Vec<u8>>,
    pub port_monitor: Arc<port_monitor::PortMonitor>,
}

/// アプリケーション Router を構築（テストからも利用可能）
pub fn create_app(
    config: Config,
    registry: Arc<SessionRegistry>,
    store: Store,
    tls_runtime: Option<&tls::TlsRuntime>,
) -> (Router, Arc<AppState>) {
    // 起動ごとにランダムな HMAC シークレットを生成
    // 再起動で全トークンが無効化される（セキュリティ上望ましい）
    let hmac_secret: Vec<u8> = rand::random::<[u8; 32]>().to_vec();
    create_app_with_secret(config, registry, hmac_secret, store, tls_runtime)
}

/// テスト用: 固定シークレットで Router を構築
pub fn create_app_with_secret(
    config: Config,
    registry: Arc<SessionRegistry>,
    hmac_secret: Vec<u8>,
    store: Store,
    tls_runtime: Option<&tls::TlsRuntime>,
) -> (Router, Arc<AppState>) {
    // NOTE: 永続化状態を追加する場合は、ここでスタートアップ時の整合性チェックを実装すること。
    // 例: 前回の異常終了で中断状態のままのリソースをリセットする（orphaned state cleanup）。
    // 以前はセッション永続化に対して store.cleanup_stale_running_sessions() を呼んでいた。

    let sftp_manager = sftp::client::SftpManager::new(store.clone());

    let port_monitor = Arc::new(port_monitor::PortMonitor::new());
    let remote_manager = Arc::new(remote::RemoteManager::default());
    let mut exclude_ports = vec![config.port];
    if let Some(ssh_port) = config.ssh_port {
        exclude_ports.push(ssh_port);
    }
    port_monitor.start(exclude_ports);

    let state = Arc::new(AppState {
        config,
        store,
        registry,
        hmac_secret,
        rate_limiter: auth::LoginRateLimiter::new(),
        sftp_manager,
        remote_manager,
        relay_manager: remote::RelayManager::default(),
        relay_client: remote::RelayClientManager::default(),
        tls_info: tls_runtime.map(|tls| tls.info.clone()),
        tls_certificate_der: tls_runtime.map(|tls| tls.certificate_der.clone()),
        port_monitor,
    });

    // 認証不要のルート
    let public_routes = Router::new()
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/system/tls", get(tls::status))
        .route("/api/system/tls/certificate", get(tls::certificate))
        .route("/", get(assets::serve_index))
        .route("/{*path}", get(assets::serve_static));

    let user_only_routes = Router::new()
        .route(
            "/api/system/tls/trusted",
            get(tls::list_trusted)
                .post(tls::trust)
                .delete(tls::remove_trusted),
        )
        .route("/api/remote/connect", post(remote::connect))
        .route("/api/remote/status", get(remote::status))
        .route("/api/remote/disconnect", post(remote::disconnect))
        .route(
            "/api/remote/terminal/sessions",
            get(remote::proxy_list_sessions).post(remote::proxy_create_session),
        )
        .route(
            "/api/remote/terminal/sessions/{session}",
            put(remote::proxy_rename_session).delete(remote::proxy_delete_session),
        )
        .route("/api/remote/ws", get(remote::ws_relay_handler))
        .route("/api/remote/filer/list", get(remote::proxy_filer_list))
        .route("/api/remote/filer/read", get(remote::proxy_filer_read))
        .route("/api/remote/filer/write", put(remote::proxy_filer_write))
        .route("/api/remote/filer/mkdir", post(remote::proxy_filer_mkdir))
        .route("/api/remote/filer/rename", post(remote::proxy_filer_rename))
        .route("/api/remote/filer/upload", post(remote::proxy_filer_upload))
        .route(
            "/api/remote/filer/download",
            get(remote::proxy_filer_download),
        )
        .route(
            "/api/remote/filer/delete",
            delete(remote::proxy_filer_delete),
        )
        .route("/api/remote/filer/search", get(remote::proxy_filer_search))
        // Relay routes
        .route("/api/relay/connect", post(remote::relay_connect))
        .route("/api/relay/status", get(remote::relay_status))
        .route("/api/relay/{id}/disconnect", post(remote::relay_disconnect))
        .route("/api/relay/{id}/ws", get(remote::relay_ws_handler))
        .route(
            "/api/relay/{id}/{*rest}",
            any(remote::relay_proxy_catch_all),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::user_auth_middleware,
        ));

    // 認証必要のルート（Cookie / Authorization ヘッダーで認証）
    let protected_routes = Router::new()
        .route("/api/settings", get(store_api::get_settings))
        .route("/api/settings", put(store_api::put_settings))
        .route(
            "/api/keep-awake",
            get(store_api::get_keep_awake).put(store_api::put_keep_awake),
        )
        // Clipboard history API
        .route(
            "/api/clipboard-history",
            get(clipboard_api::get_clipboard_history)
                .post(clipboard_api::add_clipboard_entry)
                .delete(clipboard_api::clear_clipboard_history),
        )
        // WebSocket: Cookie 認証（ブラウザが自動で Cookie を送信）
        .route("/api/ws", get(ws::ws_handler))
        // Terminal session management API
        .route(
            "/api/terminal/sessions",
            get(ws::list_sessions).post(ws::create_session),
        )
        .route("/api/terminal/sessions/order", put(ws::reorder_sessions))
        .route(
            "/api/terminal/sessions/{name}",
            put(ws::rename_session).delete(ws::destroy_session),
        )
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
        // Port detection API (system-level + PTY combined)
        .route("/api/ports", get(ws::list_all_ports))
        // Port forwarding API
        .route("/api/terminal/sessions/{name}/ports", get(ws::list_ports))
        .route(
            "/api/terminal/sessions/{name}/ports/{port}/forward",
            post(ws::start_forward).delete(ws::stop_forward),
        )
        // HTTP reverse proxy for forwarded ports (all methods)
        .route(
            "/fwd/{port}",
            get(port_forward::fwd_proxy_root)
                .post(port_forward::fwd_proxy_root)
                .put(port_forward::fwd_proxy_root)
                .delete(port_forward::fwd_proxy_root)
                .patch(port_forward::fwd_proxy_root),
        )
        .route(
            "/fwd/{port}/{*path}",
            get(port_forward::fwd_proxy)
                .post(port_forward::fwd_proxy)
                .put(port_forward::fwd_proxy)
                .delete(port_forward::fwd_proxy)
                .patch(port_forward::fwd_proxy),
        )
        // WebSocket proxy for forwarded ports
        .route("/fwd-ws/{port}", get(port_forward::fwd_ws_proxy_root))
        .route("/fwd-ws/{port}/{*path}", get(port_forward::fwd_ws_proxy))
        // System update API
        .route("/api/system/version", get(update::get_version))
        .route("/api/system/update", post(update::do_update))
        .route(
            "/api/sftp/known-hosts",
            get(sftp::api::list_known_hosts)
                .post(sftp::api::trust_host)
                .delete(sftp::api::remove_known_host),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::auth_middleware,
        ));

    let router = Router::new()
        .merge(user_only_routes)
        .merge(protected_routes)
        .merge(public_routes)
        // CSP ヘッダーを全レスポンスに付与（XSS 防止）
        .layer(middleware::from_fn(auth::csp_middleware))
        .with_state(Arc::clone(&state));

    (router, state)
}
