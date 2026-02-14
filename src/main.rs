use den::config::Config;
use den::pty::registry::SessionRegistry;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let config = Config::from_env();
    let port = config.port;
    let ssh_port = config.ssh_port;

    // env-filter 対応の tracing 初期化
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level)),
        )
        .init();

    let bind_address = config.bind_address.clone();

    tracing::info!("Den v0.4 starting on port {} ({})", port, config.env);
    tracing::info!("SSH port: {}", ssh_port);
    tracing::info!("Shell: {}", config.shell);
    tracing::info!("Password: (custom)");

    // SessionRegistry 生成
    let registry = SessionRegistry::new(config.shell.clone());

    // SSH サーバー（バックグラウンド: 失敗しても HTTP は継続）
    let ssh_registry = std::sync::Arc::clone(&registry);
    let ssh_password = config.password.clone();
    let ssh_data_dir = config.data_dir.clone();
    let ssh_bind = config.bind_address.clone();
    tokio::spawn(async move {
        if let Err(e) =
            den::ssh::server::run(ssh_registry, ssh_password, ssh_port, ssh_data_dir, ssh_bind)
                .await
        {
            tracing::error!("SSH server error: {e}");
        }
    });

    // HTTP サーバー（メイン）
    let app = den::create_app(config, registry);

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", bind_address, port))
        .await
        .expect("Failed to bind port");

    tracing::info!("Listening on http://{}:{}", bind_address, port);

    axum::serve(listener, app).await.unwrap();
}
