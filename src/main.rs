use den::config::Config;
use den::pty::registry::SessionRegistry;
use den::store::Store;
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

    tracing::info!(
        "Den v{} starting on port {} ({})",
        env!("CARGO_PKG_VERSION"),
        port,
        config.env
    );
    if let Some(sp) = ssh_port {
        tracing::info!("SSH port: {}", sp);
    } else {
        tracing::info!("SSH server: disabled (set DEN_SSH_PORT to enable)");
    }
    tracing::info!("Shell: {}", config.shell);
    tracing::info!("Password: (custom)");

    // Settings から初期設定を読み込み、SessionRegistry を生成
    let store = Store::from_data_dir(&config.data_dir).expect("Failed to initialize data store");
    let settings = store.load_settings();
    let registry = SessionRegistry::new(
        config.shell.clone(),
        settings.sleep_prevention_mode,
        settings.sleep_prevention_timeout,
    );

    // SSH サーバー（opt-in: DEN_SSH_PORT 設定時のみ起動）
    if let Some(ssh_port) = ssh_port {
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
    }

    // HTTP サーバー（メイン）
    let app = den::create_app(config, registry);

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", bind_address, port))
        .await
        .expect("Failed to bind port");

    tracing::info!("Listening on http://{}:{}", bind_address, port);

    axum::serve(listener, app).await.unwrap();
}
