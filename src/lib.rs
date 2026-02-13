pub mod assets;
pub mod auth;
pub mod claude;
pub mod config;
pub mod filer;
pub mod pty;
pub mod ws;

use axum::{
    Router, middleware,
    routing::{get, post},
};
use config::Config;
use std::sync::Arc;

pub struct AppState {
    pub config: Config,
}

/// アプリケーション Router を構築（テストからも利用可能）
pub fn create_app(config: Config) -> Router {
    let state = Arc::new(AppState { config });

    // 認証不要のルート
    let public_routes = Router::new()
        .route("/api/login", post(auth::login))
        .route("/", get(assets::serve_index))
        .route("/{*path}", get(assets::serve_static));

    // 認証必要のルート
    let protected_routes = Router::new()
        .route("/api/ws", get(ws::ws_handler))
        .route("/api/claude/ws", get(claude::ws::ws_handler))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::auth_middleware,
        ));

    Router::new()
        .merge(protected_routes)
        .merge(public_routes)
        .with_state(state)
}
