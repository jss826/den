use den::config::Config;
use den::pty::registry::SessionRegistry;
use den::store::Store;
use std::sync::Arc;
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
        Some(store.clone()),
    );

    // Restore sessions from previous run
    restore_sessions(&registry, &store).await;

    // クリップボード監視（Windows: システムクリップボード変更を検知）
    den::clipboard_monitor::start(store.clone());

    // PeerRegistry を先に作成（SSH サーバーと HTTP サーバーで共有）
    let peer_registry = Arc::new(den::peer::PeerRegistry::new());
    {
        let settings = store.load_settings();
        if let Some(peers) = &settings.peers {
            peer_registry.init_health_states(peers);
        }
    }

    // SSH サーバー（opt-in: DEN_SSH_PORT 設定時のみ起動）
    if let Some(ssh_port) = ssh_port {
        let ssh_registry = Arc::clone(&registry);
        let ssh_password = config.password.clone();
        let ssh_data_dir = config.data_dir.clone();
        let ssh_bind = config.bind_address.clone();
        let ssh_store = store.clone();
        let ssh_peer_registry = Arc::clone(&peer_registry);
        tokio::spawn(async move {
            if let Err(e) = den::ssh::server::run(
                ssh_registry,
                ssh_password,
                ssh_port,
                ssh_data_dir,
                ssh_bind,
                ssh_store,
                ssh_peer_registry,
            )
            .await
            {
                tracing::error!("SSH server error: {e}");
            }
        });
    }

    // HTTP サーバー（メイン）+ graceful shutdown
    let shutdown_registry = Arc::clone(&registry);
    let (app, app_state) = den::create_app(config, registry, store, peer_registry);

    // Start peer health check background task
    den::peer::spawn_health_check(Arc::clone(&app_state));

    let listener = tokio::net::TcpListener::bind(format!("{}:{}", bind_address, port))
        .await
        .expect("Failed to bind port");

    tracing::info!("Listening on http://{}:{}", bind_address, port);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_registry))
        .await
        .unwrap();
}

/// Restore sessions from sessions.json on startup.
async fn restore_sessions(registry: &Arc<SessionRegistry>, store: &Store) {
    let records = store.load_sessions();
    if records.is_empty() {
        return;
    }

    tracing::info!("Restoring {} session(s) from previous run", records.len());

    for record in records {
        let ssh_config = record.ssh.clone();

        // Validate SSH fields if present
        if let Some(ref ssh) = ssh_config
            && let Err(msg) = den::ws::validate_ssh_fields(ssh)
        {
            tracing::warn!("Skipping session '{}': {msg}", record.name);
            continue;
        }

        match registry
            .create_with_ssh(&record.name, 80, 24, ssh_config.clone())
            .await
        {
            Ok((session, _rx)) => {
                // Inject SSH command if configured
                if let Some(ref ssh) = ssh_config {
                    let ssh_cmd = den::ws::build_ssh_command(ssh);
                    let inject = format!("{}\r", ssh_cmd);
                    if let Err(e) = session.write_input(inject.as_bytes()).await {
                        tracing::warn!(
                            "Failed to inject SSH command for session '{}': {e}",
                            record.name
                        );
                    }

                    // For key/agent auth: inject cd after delay
                    if ssh.auth_type != den::store::SshAuthType::Password
                        && let Some(ref dir) = ssh.initial_dir
                    {
                        let dir = dir.clone();
                        let session = Arc::clone(&session);
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            let cd_cmd = format!("cd '{}'\r", dir);
                            if let Err(e) = session.write_input(cd_cmd.as_bytes()).await {
                                tracing::warn!("Failed to inject cd command: {e}");
                            }
                        });
                    }
                }
                tracing::info!("Restored session: {}", record.name);
            }
            Err(e) => {
                tracing::warn!("Failed to restore session '{}': {e}", record.name);
            }
        }
    }
}

/// Wait for shutdown signal (Ctrl+C) and persist sessions.
async fn shutdown_signal(registry: Arc<SessionRegistry>) {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("Shutdown signal received, persisting sessions...");
    registry.persist_sessions().await;
    tracing::info!("Sessions persisted. Shutting down.");
}
