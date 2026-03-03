use den::config::Config;
use den::pty::registry::SessionRegistry;
use den::store::Store;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() {
    // Load .env from the executable's directory (if present)
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        let _ = dotenvy::from_path(exe_dir.join(".env"));
    }

    let config = Config::from_env();
    let port = config.port;
    let ssh_port = config.ssh_port;

    // tracing 初期化: console (stderr) + file (data_dir/logs/)
    // stdout は ConPTY (OpenConsole.exe) のカーソル制御シーケンスに干渉されるため
    // stderr に明示的に出力する。
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));
    let console_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    let log_dir = std::path::Path::new(&config.data_dir).join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::daily(&log_dir, "den.log");
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(file_appender);

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
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

    // クリップボード監視（Windows: システムクリップボード変更を検知）
    den::clipboard_monitor::start(store.clone());

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
    let app = den::create_app(config, registry, store);

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", bind_address, port))
        .await
        .expect("Failed to bind port");

    tracing::info!("Listening on http://{}:{}", bind_address, port);

    axum::serve(listener, app).await.unwrap();
}
