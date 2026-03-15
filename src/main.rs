use den::config::Config;
use den::pty::registry::SessionRegistry;
use den::store::Store;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() {
    // Load .env: CWD first, then platform-specific config directory as fallback.
    // Later values do NOT override earlier ones, so CWD takes precedence.
    let _ = dotenvy::dotenv();
    if cfg!(windows) {
        // Windows: exe directory (e.g. AppData\Local\den\.env)
        if let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        {
            let _ = dotenvy::from_path(exe_dir.join(".env"));
        }
    } else {
        // Linux/macOS: XDG_CONFIG_HOME/den/.env (default ~/.config/den/.env)
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| std::path::PathBuf::from(h).join(".config"))
            });
        if let Some(dir) = config_dir {
            let _ = dotenvy::from_path(dir.join("den").join(".env"));
        }
    }

    let config = Config::from_env();
    let port = config.port;
    let ssh_port = config.ssh_port;
    let tls_runtime = den::tls::setup(&config).unwrap_or_else(|e| {
        eprintln!("ERROR: TLS setup failed: {e}");
        std::process::exit(1);
    });

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
        Some(store.clone()),
    );

    // クリップボード監視（システムクリップボード変更を検知）
    let clipboard_handle = den::clipboard_monitor::start(store.clone());

    // HTTP サーバー（メイン）+ graceful shutdown
    let shutdown_registry = Arc::clone(&registry);
    let (app, app_state) = den::create_app(config, registry, store, tls_runtime.as_ref());

    // SSH サーバー（opt-in: DEN_SSH_PORT 設定時のみ起動）
    if let Some(ssh_port) = ssh_port {
        let ssh_registry = Arc::clone(&app_state.registry);
        let ssh_password = app_state.config.password.clone();
        let ssh_data_dir = app_state.config.data_dir.clone();
        let ssh_bind = app_state.config.bind_address.clone();
        let ssh_store = app_state.store.clone();
        tokio::spawn(async move {
            if let Err(e) = den::ssh::server::run(
                ssh_registry,
                ssh_password,
                ssh_port,
                ssh_data_dir,
                ssh_bind,
                ssh_store,
            )
            .await
            {
                tracing::error!("SSH server error: {e}");
            }
        });
    }

    let listener = bind_with_retry(&bind_address, port)
        .await
        .expect("Failed to bind port");

    if let Some(tls_runtime) = tls_runtime {
        tracing::info!("TLS: enabled");
        tracing::info!("TLS fingerprint: {}", tls_runtime.info.fingerprint);
        tracing::info!(
            "TLS SANs: {}",
            tls_runtime.info.subject_alt_names.join(", ")
        );
        tracing::info!("Listening on https://{}:{}", bind_address, port);
        den::tls::serve(
            listener,
            app,
            tls_runtime.server_config,
            shutdown_signal(shutdown_registry, clipboard_handle.clone()),
        )
        .await
        .unwrap();
    } else {
        tracing::info!("Listening on http://{}:{}", bind_address, port);
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal(shutdown_registry, clipboard_handle))
            .await
            .unwrap();
    }
}

/// Bind a TCP listener with retries (handles port still held by previous process after update).
async fn bind_with_retry(addr: &str, port: u16) -> Result<tokio::net::TcpListener, std::io::Error> {
    const MAX_RETRIES: u32 = 10;
    const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

    let bind_addr = format!("{addr}:{port}");
    let mut last_err = None;

    for attempt in 0..MAX_RETRIES {
        match tokio::net::TcpListener::bind(&bind_addr).await {
            Ok(listener) => return Ok(listener),
            Err(e) => {
                if attempt > 0 {
                    tracing::warn!(
                        "Port {port} not yet available (attempt {}/{}): {e}",
                        attempt + 1,
                        MAX_RETRIES
                    );
                }
                last_err = Some(e);
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
        }
    }

    Err(last_err.unwrap())
}

/// Wait for shutdown signal (Ctrl+C) and persist sessions.
async fn shutdown_signal(
    registry: Arc<SessionRegistry>,
    clipboard_handle: den::clipboard_monitor::ClipboardMonitorHandle,
) {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("Shutdown signal received, persisting sessions...");
    clipboard_handle.stop();
    registry.persist_sessions().await;
    tracing::info!("Sessions persisted. Shutting down.");
}
