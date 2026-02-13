pub mod assets;
pub mod auth;
pub mod claude;
pub mod config;
pub mod filer;
pub mod pty;
pub mod store;
pub mod store_api;
pub mod ws;

use axum::{
    Router, middleware,
    routing::{get, post, put},
};
use config::Config;
use std::sync::Arc;
use store::Store;

pub struct AppState {
    pub config: Config,
    pub store: Store,
}

/// アプリケーション Router を構築（テストからも利用可能）
pub fn create_app(config: Config) -> Router {
    let store = Store::from_data_dir(&config.data_dir).expect("Failed to initialize data store");

    let state = Arc::new(AppState { config, store });

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
        .route("/api/sessions/{id}", get(store_api::get_session))
        .route(
            "/api/sessions/{id}/events",
            get(store_api::get_session_events),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::auth_middleware,
        ));

    Router::new()
        .merge(protected_routes)
        .merge(public_routes)
        .with_state(state)
}
