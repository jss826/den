use den::config::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let config = Config::from_env();
    let port = config.port;

    // env-filter 対応の tracing 初期化
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level)),
        )
        .init();

    tracing::info!("Den v0.2 starting on port {} ({})", port, config.env);
    tracing::info!("Shell: {}", config.shell);
    tracing::info!(
        "Password: {}",
        if config.password == "den" {
            "(default: den)"
        } else {
            "(custom)"
        }
    );

    let app = den::create_app(config);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("Failed to bind port");

    tracing::info!("Listening on http://0.0.0.0:{}", port);

    axum::serve(listener, app).await.unwrap();
}
