pub mod assets;
pub mod auth;
pub mod clipboard_api;
pub mod clipboard_monitor;
pub mod config;
pub mod filer;
pub mod peer;
pub mod port_detection;
pub mod port_forward;
pub mod port_monitor;
pub mod pty;
pub mod sftp;
pub mod ssh;
pub mod store;
pub mod store_api;
pub mod update;
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
    pub peer_registry: Arc<peer::PeerRegistry>,
    pub port_monitor: Arc<port_monitor::PortMonitor>,
}

/// アプリケーション Router を構築（テストからも利用可能）
pub fn create_app(
    config: Config,
    registry: Arc<SessionRegistry>,
    store: Store,
    peer_registry: Arc<peer::PeerRegistry>,
) -> (Router, Arc<AppState>) {
    // 起動ごとにランダムな HMAC シークレットを生成
    // 再起動で全トークンが無効化される（セキュリティ上望ましい）
    let hmac_secret: Vec<u8> = rand::random::<[u8; 32]>().to_vec();
    create_app_with_secret(config, registry, hmac_secret, store, peer_registry)
}

/// テスト用: 固定シークレットで Router を構築
pub fn create_app_with_secret(
    config: Config,
    registry: Arc<SessionRegistry>,
    hmac_secret: Vec<u8>,
    store: Store,
    peer_registry: Arc<peer::PeerRegistry>,
) -> (Router, Arc<AppState>) {
    // NOTE: 永続化状態を追加する場合は、ここでスタートアップ時の整合性チェックを実装すること。
    // 例: 前回の異常終了で中断状態のままのリソースをリセットする（orphaned state cleanup）。
    // 以前はセッション永続化に対して store.cleanup_stale_running_sessions() を呼んでいた。

    let sftp_manager = sftp::client::SftpManager::new(store.clone());

    let port_monitor = Arc::new(port_monitor::PortMonitor::new());
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
        peer_registry,
        port_monitor,
    });

    // 認証不要のルート
    let public_routes = Router::new()
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        // Peer pairing endpoint: authenticated by invite code, not user token
        .route("/api/peers/pair", post(peer::pair))
        .route("/", get(assets::serve_index))
        .route("/{*path}", get(assets::serve_static));

    // 認証必要のルート（Cookie / Authorization ヘッダー / peer token で認証）
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
        // Peer management API
        .route("/api/peers/invite", post(peer::create_invite))
        .route("/api/peers/join", post(peer::join))
        .route("/api/peers", get(peer::list_peers))
        .route("/api/peers/{name}", delete(peer::delete_peer))
        // Peer terminal proxy API
        .route(
            "/api/peers/{name}/terminal/sessions",
            get(peer::proxy_list_sessions).post(peer::proxy_create_session),
        )
        .route(
            "/api/peers/{name}/terminal/sessions/{session}",
            put(peer::proxy_rename_session).delete(peer::proxy_delete_session),
        )
        .route("/api/peers/{name}/ws", get(peer::ws_relay_handler))
        // Peer filer proxy API
        .route("/api/peers/{name}/filer/list", get(peer::proxy_filer_list))
        .route("/api/peers/{name}/filer/read", get(peer::proxy_filer_read))
        .route(
            "/api/peers/{name}/filer/write",
            put(peer::proxy_filer_write),
        )
        .route(
            "/api/peers/{name}/filer/upload",
            post(peer::proxy_filer_upload),
        )
        .route(
            "/api/peers/{name}/filer/download",
            get(peer::proxy_filer_download),
        )
        .route(
            "/api/peers/{name}/filer/mkdir",
            post(peer::proxy_filer_mkdir),
        )
        .route(
            "/api/peers/{name}/filer/rename",
            post(peer::proxy_filer_rename),
        )
        .route(
            "/api/peers/{name}/filer/delete",
            delete(peer::proxy_filer_delete),
        )
        .route(
            "/api/peers/{name}/filer/search",
            get(peer::proxy_filer_search),
        )
        // Port detection API (system-level + PTY combined)
        .route("/api/ports", get(ws::list_all_ports))
        // Peer port proxy API
        .route("/api/peers/{name}/ports", get(peer::proxy_list_ports))
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
        // Remote peer port proxy (HTTP + WebSocket)
        .route(
            "/fwd/peer/{peer}/{port}",
            get(port_forward::fwd_peer_proxy_root)
                .post(port_forward::fwd_peer_proxy_root)
                .put(port_forward::fwd_peer_proxy_root)
                .delete(port_forward::fwd_peer_proxy_root)
                .patch(port_forward::fwd_peer_proxy_root),
        )
        .route(
            "/fwd/peer/{peer}/{port}/{*path}",
            get(port_forward::fwd_peer_proxy)
                .post(port_forward::fwd_peer_proxy)
                .put(port_forward::fwd_peer_proxy)
                .delete(port_forward::fwd_peer_proxy)
                .patch(port_forward::fwd_peer_proxy),
        )
        .route(
            "/fwd-ws/peer/{peer}/{port}",
            get(port_forward::fwd_peer_ws_proxy_root),
        )
        .route(
            "/fwd-ws/peer/{peer}/{port}/{*path}",
            get(port_forward::fwd_peer_ws_proxy),
        )
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
        .merge(protected_routes)
        .merge(public_routes)
        // CSP ヘッダーを全レスポンスに付与（XSS 防止）
        .layer(middleware::from_fn(auth::csp_middleware))
        .with_state(Arc::clone(&state));

    (router, state)
}
