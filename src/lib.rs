pub mod assets;
pub mod auth;
pub mod claude;
pub mod config;
pub mod filer;
pub mod pty;
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
}

/// アプリケーション Router を構築（テストからも利用可能）
pub fn create_app(config: Config, registry: Arc<SessionRegistry>) -> Router {
    let store = Store::from_data_dir(&config.data_dir).expect("Failed to initialize data store");

    let state = Arc::new(AppState {
        config,
        store,
        registry,
    });

    // 認証不要のルート
    let public_routes = Router::new()
        .route("/api/login", post(auth::login))
        .route("/", get(assets::serve_index))
        .route("/{*path}", get(assets::serve_static));

    // 認証必要のルート
    let protected_routes = Router::new()
        .route("/api/ws", get(ws::ws_handler))
        .route("/api/claude/ws", get(claude::ws::ws_handler))
        .route("/api/settings", get(store_api::get_settings))
        .route("/api/settings", put(store_api::put_settings))
        .route("/api/sessions", get(store_api::list_sessions))
        .route(
            "/api/sessions/{id}",
            get(store_api::get_session).delete(store_api::delete_session),
        )
        .route(
            "/api/sessions/{id}/events",
            get(store_api::get_session_events),
        )
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
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::auth_middleware,
        ));

    Router::new()
        .merge(protected_routes)
        .merge(public_routes)
        .with_state(state)
}
