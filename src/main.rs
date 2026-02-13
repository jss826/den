mod assets;
mod auth;
mod config;
mod ws;
mod pty;
mod claude;
mod filer;

use axum::{
    Router,
    middleware,
    routing::{get, post},
};
use config::Config;
use std::sync::Arc;

pub struct AppState {
    pub config: Config,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let port = config.port;

    tracing::info!("Den v0.1 starting on port {}", port);
    tracing::info!("Shell: {}", config.shell);
    tracing::info!(
        "Password: {}",
        if config.password == "den" {
            "(default: den)"
        } else {
            "(custom)"
        }
    );

    let state = Arc::new(AppState { config });

    // 認証不要のルート
    let public_routes = Router::new()
        .route("/api/login", post(auth::login))
        .route("/", get(assets::serve_index))
        .route("/{*path}", get(assets::serve_static));

    // 認証必要のルート
    let protected_routes = Router::new()
        .route("/api/ws", get(ws::ws_handler))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::auth_middleware,
        ));

    let app = Router::new()
        .merge(protected_routes)
        .merge(public_routes)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("Failed to bind port");

    tracing::info!("Listening on http://0.0.0.0:{}", port);

    axum::serve(listener, app).await.unwrap();
}
