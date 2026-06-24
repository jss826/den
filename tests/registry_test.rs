use std::sync::Arc;

use serial_test::serial;
use tokio::sync::broadcast;

use den::pty::registry::{ClientKind, OutputChunk, RegistryError, SessionRegistry, SharedSession};
use den::store::SleepPreventionMode;

fn new_registry() -> Arc<SessionRegistry> {
    SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
        den::pty::backend::MuxConfig::default(),
    )
}

fn session_name(test: &str) -> String {
    format!(
        "test-{}-{}",
        test,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            % 100000
    )
}

/// ConPTY の DSR (`ESC[6n`) に CPR で応答し、シェルが起動するまで待つ。
/// シェルが初期化前に死亡した場合は panic する。
async fn init_shell(session: &Arc<SharedSession>, rx: &mut broadcast::Receiver<Arc<OutputChunk>>) {
    let overall = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut buf = Vec::new();

    // Phase 1: DSR を検出して CPR を返す
    loop {
        match tokio::time::timeout_at(overall, rx.recv()).await {
            Ok(Ok(chunk)) => {
                buf.extend_from_slice(&chunk.data);
                if buf.windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = session.write_input(b"\x1b[1;1R").await;
                    break;
                }
            }
            _ => {
                assert!(
                    session.is_alive(),
                    "Shell died during init (DSR phase). Received {} bytes but no DSR.",
                    buf.len()
                );
                return;
            }
        }
    }

    // Phase 2: 出力が落ち着くまで待つ（1秒間新データなし → 完了）
    loop {
        let idle = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        match tokio::time::timeout_at(std::cmp::min(idle, overall), rx.recv()).await {
            Ok(Ok(_)) => continue,
            _ => break,
        }
    }

    assert!(
        session.is_alive(),
        "Shell died during init (idle-wait phase)"
    );
}

/// exit 後にセッションが dead になるまでポーリング
async fn wait_for_death(session: &Arc<SharedSession>, timeout_secs: u64) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    while session.is_alive() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// テスト用ランタイムを構築する。
/// ConPTY の read_task は子プロセス終了後もパイプが閉じないため、
/// spawn_blocking が永久ブロックする。shutdown_timeout で強制終了する。
fn build_test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
}

// ============================================================
// PTY 不要テスト（即座に完了）
// ============================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_invalid_name() {
    let reg = new_registry();
    assert!(matches!(
        reg.create("../bad", 80, 24).await,
        Err(RegistryError::InvalidName(_))
    ));
    assert!(matches!(
        reg.create("", 80, 24).await,
        Err(RegistryError::InvalidName(_))
    ));
    assert!(matches!(
        reg.create("has space", 80, 24).await,
        Err(RegistryError::InvalidName(_))
    ));
    let long_name = "a".repeat(65);
    assert!(matches!(
        reg.create(&long_name, 80, 24).await,
        Err(RegistryError::InvalidName(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_nonexistent_returns_not_found() {
    let reg = new_registry();
    let result = reg
        .attach("nonexistent-session", ClientKind::WebSocket, 80, 24, None)
        .await;
    assert!(matches!(result, Err(RegistryError::NotFound(_))));
}

// ============================================================
// PTY テスト（非対話）: init_shell 不要、高速
// shutdown_timeout で残存 read_task を強制終了
// ============================================================

#[test]
#[serial]
fn pty_non_interactive() {
    let rt = build_test_runtime();
    rt.block_on(async {
        // --- create → alive → destroy → dead ---
        {
            let reg = new_registry();
            let name = session_name("alive");

            let (session, _rx) = reg.create(&name, 80, 24).await.unwrap();
            assert!(session.is_alive());
            assert!(reg.exists(&name).await);
            reg.destroy(&name).await;
            assert!(!reg.exists(&name).await);
            assert!(!session.is_alive());
        }

        // --- create duplicate → AlreadyExists ---
        {
            let reg = new_registry();
            let name = session_name("dup");

            let (_s, _rx) = reg.create(&name, 80, 24).await.unwrap();
            assert!(matches!(
                reg.create(&name, 80, 24).await,
                Err(RegistryError::AlreadyExists(_))
            ));
            reg.destroy(&name).await;
        }

        // --- attach → replay non-empty ---
        {
            let reg = new_registry();
            let name = session_name("attach");

            let (_s, _rx) = reg.create(&name, 80, 24).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let (_s, _rx, replay, _cid) = reg
                .attach(&name, ClientKind::WebSocket, 80, 24, None)
                .await
                .unwrap();
            assert!(
                !replay.data.is_empty(),
                "Replay should contain DSR sequences"
            );
            reg.destroy(&name).await;
        }

        // --- detach → session persists ---
        {
            let reg = new_registry();
            let name = session_name("detach");

            let (_s, _rx) = reg.create(&name, 80, 24).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let (_s, _rx, _replay, cid) = reg
                .attach(&name, ClientKind::WebSocket, 80, 24, None)
                .await
                .unwrap();
            reg.detach(&name, cid).await;
            assert!(reg.exists(&name).await);
            reg.destroy(&name).await;
        }

        // --- get_or_create (new) ---
        {
            let reg = new_registry();
            let name = session_name("goc-new");

            let (session, _rx, _replay, _cid) = reg
                .get_or_create(&name, ClientKind::WebSocket, 80, 24, None)
                .await
                .unwrap();
            assert!(session.is_alive());
            reg.destroy(&name).await;
        }

        // --- get_or_create (existing) ---
        {
            let reg = new_registry();
            let name = session_name("goc-exist");

            let (_s, _rx) = reg.create(&name, 80, 24).await.unwrap();
            let (session, _rx, _replay, _cid) = reg
                .get_or_create(&name, ClientKind::WebSocket, 80, 24, None)
                .await
                .unwrap();
            assert!(session.is_alive());
            reg.destroy(&name).await;
        }

        // --- list sessions ---
        {
            let reg = new_registry();
            let n1 = session_name("list1");
            let n2 = session_name("list2");

            let (_s1, _rx1) = reg.create(&n1, 80, 24).await.unwrap();
            let (_s2, _rx2) = reg.create(&n2, 80, 24).await.unwrap();
            let list = reg.list().await;
            let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
            assert!(names.contains(&n1.as_str()));
            assert!(names.contains(&n2.as_str()));
            reg.destroy(&n1).await;
            reg.destroy(&n2).await;
        }

        // --- exists / get ---
        {
            let reg = new_registry();
            let name = session_name("exists");

            assert!(!reg.exists(&name).await);
            assert!(reg.get(&name).await.is_none());

            let (_s, _rx) = reg.create(&name, 80, 24).await.unwrap();
            assert!(reg.exists(&name).await);
            let got = reg.get(&name).await;
            assert!(got.is_some());
            assert_eq!(got.unwrap().name, name);

            reg.destroy(&name).await;
            assert!(!reg.exists(&name).await);
        }

        // --- write_input_from: アクティブ切り替え + dead session エラー ---
        {
            let reg = new_registry();
            let name = session_name("wif");

            let (_s, _rx) = reg.create(&name, 80, 24).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let (s, _rx1, _rp1, id1) = reg
                .attach(&name, ClientKind::WebSocket, 120, 40, None)
                .await
                .unwrap();
            let (_s2, _rx2, _rp2, id2) = reg
                .attach(&name, ClientKind::Ssh, 80, 24, None)
                .await
                .unwrap();

            // 登録済みクライアントからの書き込みは成功する
            assert!(s.write_input_from(id1, b"test1").await.is_ok());
            // 別クライアントに切り替えても成功する
            assert!(s.write_input_from(id2, b"test2").await.is_ok());
            // 既にアクティブなクライアントの再書き込みも成功する
            assert!(s.write_input_from(id2, b"test3").await.is_ok());
            // 未登録クライアント ID でも書き込み自体は成功する（アクティブ切替はスキップ）
            assert!(s.write_input_from(99999, b"test4").await.is_ok());

            // destroy 後は dead → エラー
            reg.destroy(&name).await;
            assert!(s.write_input_from(id1, b"test5").await.is_err());
        }

        // --- resize (multiple clients) ---
        {
            let reg = new_registry();
            let name = session_name("resize");

            let (_s, _rx) = reg.create(&name, 80, 24).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            let (s1, _rx1, _rp1, id1) = reg
                .attach(&name, ClientKind::WebSocket, 120, 40, None)
                .await
                .unwrap();
            let (s2, _rx2, _rp2, id2) = reg
                .attach(&name, ClientKind::Ssh, 80, 24, None)
                .await
                .unwrap();
            s1.resize(id1, 100, 30).await;
            s2.resize(id2, 90, 25).await;
            reg.destroy(&name).await;
        }
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
}

// ============================================================
// PTY テスト（対話）: init_shell + broadcast/write/replay
// ============================================================

#[test]
#[serial]
fn pty_interactive() {
    let rt = build_test_runtime();
    rt.block_on(async {
        let reg = new_registry();
        let name = session_name("interactive");

        let (session, mut rx) = reg.create(&name, 80, 24).await.unwrap();
        init_shell(&session, &mut rx).await;

        // --- broadcast: echo → output 受信 ---
        while rx.try_recv().is_ok() {}
        session
            .write_input(b"echo BROADCAST_MARKER_99\r\n")
            .await
            .unwrap();

        let mut output = String::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(data)) => {
                    output.push_str(&String::from_utf8_lossy(&data.data));
                    if output.contains("BROADCAST_MARKER_99") {
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(
            output.contains("BROADCAST_MARKER_99"),
            "Expected marker in broadcast output"
        );

        // --- write_input: 別マーカー ---
        while rx.try_recv().is_ok() {}
        session
            .write_input(b"echo WRITE_MARKER_77\r\n")
            .await
            .unwrap();

        let mut output2 = String::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(data)) => {
                    output2.push_str(&String::from_utf8_lossy(&data.data));
                    if output2.contains("WRITE_MARKER_77") {
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(
            output2.contains("WRITE_MARKER_77"),
            "Expected marker in write output"
        );

        // --- replay: attach して replay に内容が含まれるか ---
        let (_s, _rx2, replay, _cid) = reg
            .attach(&name, ClientKind::WebSocket, 80, 24, None)
            .await
            .unwrap();
        let replay_text = String::from_utf8_lossy(&replay.data);
        assert!(!replay.data.is_empty(), "Replay should contain data");
        assert!(
            replay_text.contains("BROADCAST_MARKER_99")
                || replay_text.contains("WRITE_MARKER_77")
                || replay_text.contains("PowerShell")
                || replay_text.contains("❯")
                || replay_text.contains("PS ")
                || replay_text.contains(">"),
            "Replay should contain shell output: {:?}",
            &replay_text[..replay_text.len().min(500)]
        );

        reg.destroy(&name).await;
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
}

#[test]
#[serial]
fn reconnect_full_replay_includes_visible_snapshot() {
    let rt = build_test_runtime();
    rt.block_on(async {
        let reg = new_registry();
        let (session, mut rx) = reg.create("snaptest", 80, 24).await.unwrap();
        init_shell(&session, &mut rx).await;

        // Produce a distinctive line on screen.
        session.write_input(b"echo SNAP_VT_MARKER\r").await.unwrap();
        // Let the shell echo + render.
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;

        // A brand-new client (since = None) gets a FULL replay → snapshot present.
        let slice = session.replay_since(None);
        assert!(slice.full, "new client must get a full replay");
        let snap = slice
            .snapshot
            .expect("full replay must carry a VT snapshot");

        // The snapshot, re-rendered, reproduces the marker on the visible screen.
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(&snap);
        assert!(
            p.screen().contents().contains("SNAP_VT_MARKER"),
            "snapshot must reflect the current screen, got:\n{}",
            p.screen().contents()
        );

        reg.destroy("snaptest").await;
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
}

// ============================================================
// PTY テスト（exit）: init_shell + exit → dead 検出 → 再作成
// ============================================================

#[test]
#[serial]
fn pty_exit_and_recreate() {
    let rt = build_test_runtime();
    rt.block_on(async {
        let reg = new_registry();
        let name = session_name("exit");

        let (session, mut rx) = reg.create(&name, 80, 24).await.unwrap();
        init_shell(&session, &mut rx).await;

        // exit 送信 → dead 検出
        session.write_input(b"exit\r\n").await.unwrap();
        wait_for_death(&session, 10).await;
        assert!(!session.is_alive(), "Session should be dead after exit");

        // dead session の subscribe → Closed
        let mut dead_rx = session.subscribe();
        let result = dead_rx.recv().await;
        assert!(result.is_err(), "Subscribe on dead session → Closed");

        // destroy → 消える
        assert!(reg.exists(&name).await);
        reg.destroy(&name).await;
        assert!(!reg.exists(&name).await);

        // get_or_create → 再作成
        let (new_session, _rx, _replay, _cid) = reg
            .get_or_create(&name, ClientKind::WebSocket, 80, 24, None)
            .await
            .unwrap();
        assert!(new_session.is_alive());

        reg.destroy(&name).await;
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
}

#[test]
#[serial]
fn create_with_backend_shell_spawns_session() {
    let rt = build_test_runtime();
    rt.block_on(async {
        let reg = new_registry();
        let name = session_name("backend-shell");
        let res = reg
            .create_with_backend(&name, 80, 24, den::pty::backend::SessionBackend::Shell)
            .await;
        assert!(res.is_ok(), "Shell backend create should succeed");
        assert!(reg.exists(&name).await);
        reg.destroy(&name).await;
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
}

#[test]
#[serial]
fn create_with_backend_rejects_same_name_different_backend() {
    // 同名・別 backend は BackendMismatch（別種同名への誤 attach を防ぐ）、
    // 同名・同 backend は従来どおり AlreadyExists。
    // 既存判定は spawn 前の fast-check なので zellij/tmux 未インストールでも成立する。
    let rt = build_test_runtime();
    rt.block_on(async {
        let reg = new_registry();
        let name = session_name("backend-conflict");
        reg.create_with_backend(&name, 80, 24, den::pty::backend::SessionBackend::Shell)
            .await
            .expect("initial shell session should be created");

        // Ok 側は Arc<SharedSession>（Debug 未実装）なので () に潰してから検査する
        let mismatch = reg
            .create_with_backend(&name, 80, 24, den::pty::backend::SessionBackend::Zellij)
            .await
            .map(|_| ());
        assert!(
            matches!(mismatch, Err(RegistryError::BackendMismatch(_))),
            "different backend on same name must be BackendMismatch, got {mismatch:?}"
        );

        let same = reg
            .create_with_backend(&name, 80, 24, den::pty::backend::SessionBackend::Shell)
            .await
            .map(|_| ());
        assert!(
            matches!(same, Err(RegistryError::AlreadyExists(_))),
            "same backend on same name must be AlreadyExists, got {same:?}"
        );

        reg.destroy(&name).await;
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
}
