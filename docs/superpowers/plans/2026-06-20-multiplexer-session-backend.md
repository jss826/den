# Multiplexer Session Backend (tmux / zellij) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Den のセッション起動コマンドを tmux/zellij でラップできるようにし、PC↔iPad 引き継ぎと Den 再起動跨ぎの永続化を multiplexer に委譲する。

**Architecture:** registry セッションモデルは維持し、`PtyManager::spawn` が起動するコマンドだけを backend に応じて差し替える（案A: 追加の `SessionBackend` enum、ssh パスは無改変）。Den セッション名 = multiplexer セッション名（1:1）。bare layout 既定、latest-active サイズ、remote(denA→denB) はプロキシ allowlist 1 行で対応。

**Tech Stack:** Rust (axum + portable-pty + tokio + rust-embed + serde) / 素の HTML/CSS/JS / playwright e2e。外部 multiplexer: zellij 0.44+（Windows ネイティブ）, tmux。

**設計 spec:** `docs/superpowers/specs/2026-06-20-multiplexer-session-backend-design.md`

## Global Constraints

- 新規 crate 追加は事前相談（`.claude/rules/development.md`）。本プランは新規 crate 不要。
- 本番コードで `unwrap()` 禁止（`expect()`/エラーハンドリング、`main.rs` の起動時のみ許容）。
- 変更対象ファイルは編集前に必ず Read。
- セッション名は `is_valid_session_name`（英数＋`-`、最大 64 文字）で限定済み。これは zellij/tmux のセッション名制約の部分集合。
- `build_launch_command` はシェル文字列連結をしない。`CommandBuilder` に program＋args を配列で渡す。
- ssh/auth コード（`create_with_ssh` / `validate_ssh_fields` / `build_ssh_command` / `SshSessionConfig`）は**無改変**。
- 品質ゲート（`.claude/CLAUDE.md` スロット①）: `cargo fmt -- --check` / `cargo clippy -- -D warnings` / `cargo test --target-dir target-test` / UI 変更ありのため `npx playwright test tests/e2e/filer-ui.e2e.ts`。
- 長時間 cargo コマンドは `run_in_background: true` で起動し `TaskOutput` で結果取得（`.claude/rules/bash-tool.md`）。
- PTY テストは `#[tokio::test]` 禁止。`#[test]`＋手動ランタイム＋`rt.shutdown_timeout(3s)`（`.claude/rules/development.md`）。
- DEN_DATA_DIR は開発時 `./data-dev` 厳守（`./data` 上書き禁止）。
- 新規 IIFE グローバルモジュールを追加したら `eslint.config.mjs` の globals に登録（本プランは新規モジュール追加なし）。
- コミットメッセージは英語 Conventional Commits。Co-Authored-By トレーラを付ける。
- マージ先 `master`、squash。本ブランチ `feat/multiplexer-session-backend`。

---

## File Structure

**新規:**
- `src/pty/backend.rs` — `SessionBackend` enum ＋ `build_launch_command` 純関数 ＋ availability probe ＋ `list_mux_sessions` パーサ。
- `frontend/layouts/den-bare.kdl` — zellij bare layout（rust-embed）。
- `frontend/layouts/den.conf` — tmux 設定（status off ＋ window-size latest）。
- `docs/superpowers/specs/2026-06-20-multiplexer-session-backend-poc-findings.md` — Task 1 PoC の確定値置き場。

**変更:**
- `src/pty/manager.rs` — `spawn` を program＋args 受け取りに拡張。
- `src/pty/mod.rs` — `pub mod backend;` 追加。
- `src/pty/registry.rs` — `create_with_backend`、`upsert_saved_record`/`load_saved_record` の backend 対応、layout パス保持。
- `src/store.rs:113` — `SessionRecord` に `backend` フィールド追加。
- `src/ws.rs:239,319` — `CreateSessionRequest.backend`、`create_session` ハンドラで `create_with_backend` 呼び出し。
- `src/multiplexer_api.rs`（新規） — `GET /api/multiplexer/status` ハンドラ。
- `src/lib.rs:122,140` — `/api/multiplexer/status` ルート追加、layout 書き出し起動処理。
- `src/remote.rs:431` — catch-all allowlist に `multiplexer/` 追加。
- `frontend/index.html:346` 付近 — Settings に `setting-default-backend` select 追加。
- `frontend/js/settings.js` — `default_backend` 既定値＋保存/復元。
- `frontend/js/terminal.js:1074,1461` — `createSession` に backend、`showNewSessionMenu` に submenu。
- `tests/e2e/` — メニュー構造 e2e（status モック）。

---

## Task 1: PoC — multiplexer 実挙動の確証（手動・実機）

**目的:** 後続タスクが依存する「正確なコマンド形」「layout 中身」「clamp/detach 挙動」を実機で確定する。コードは書かず、確定値を findings doc に記録する。

**Files:**
- Create: `docs/superpowers/specs/2026-06-20-multiplexer-session-backend-poc-findings.md`

**実機要件:** zellij 0.44+ と tmux が入ったホスト（主作業ホスト）＋ iPad（同時 attach 検証）。

- [ ] **Step 1: zellij の attach-or-create + bare layout 形を確定**

主作業ホストの素の端末で順に試し、どの形が「存在すれば attach・無ければ作成、かつ status/tab bar 非表示」になるか確認する:

```bash
# 候補A（layout 付き作成 + 既存時 attach）
zellij --session den-poc --layout ./frontend/layouts/den-bare.kdl
# 別端末から同名で再実行 → attach（合流）になるか
zellij --session den-poc --layout ./frontend/layouts/den-bare.kdl
# 候補B（layout 無し attach-or-create のフォールバック）
zellij attach -c den-poc
```

確認: ①2 回目が新規作成でなく attach になる ②status bar / tab bar が消えている ③`zellij ls` に `den-poc` が出る。

findings doc に「Zellij 起動コマンドの正規形 = （確定した形）」を記録。

- [ ] **Step 2: tmux の形を確定**

```bash
tmux -f ./frontend/layouts/den.conf new-session -A -s den-poc
# status off と window-size latest が効くか
tmux ls
```

確認: ①`-A` で attach-or-create になる ②status 行が消えている。findings doc に記録。

- [ ] **Step 3: clamp と latest-active サイズを実測**

zellij/tmux それぞれで、PC（大）と iPad（小）から同名セッションへ同時 attach し、画面サイズの clamp 挙動を観察:

- tmux: `den.conf` に `set -g window-size latest` を入れた状態で、直近操作した側にサイズが追従するか。
- zellij: 同等の挙動があるか（無ければ最小 clamp のはず）。

findings doc に「tmux=latest 追従 / zellij=（実測結果）」を記録。zellij に latest 相当が無い場合は「zellij は最小 clamp。引き継ぎ（片方ずつ）では実害小。同時閲覧時は縮む」と明記し、spec の TBD #2 を更新。

- [ ] **Step 4: destroy=detach がクリーンか実測**

```bash
# attach プロセスを kill（Den の destroy 相当）して mux セッションが生存するか
zellij attach den-poc &   # 別端末
kill %1                   # attach プロセスだけ kill
zellij ls                 # den-poc がまだ生きているか
```

tmux も同様（`tmux attach -t den-poc` を kill）。findings doc に「attach 子プロセス kill = クリーン detach、mux セッション生存（Yes/No）」を記録。No なら spec のエラー処理に追記。

- [ ] **Step 5: layout ファイル中身を確定して findings に記録**

Step 1/2 で bar 非表示になった `den-bare.kdl` と `den.conf` の最終中身を findings doc に貼る（Task 4 がこれを使う）。

- [ ] **Step 6: PoC findings を commit**

```bash
git add docs/superpowers/specs/2026-06-20-multiplexer-session-backend-poc-findings.md
git commit -m "docs: record multiplexer backend PoC findings"
```

---

## Task 2: SessionBackend enum と build_launch_command（純関数・TDD）

**Files:**
- Create: `src/pty/backend.rs`
- Modify: `src/pty/mod.rs`

**Interfaces:**
- Produces:
  - `pub enum SessionBackend { Shell, Zellij, Tmux }`（`#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]`, `#[serde(rename_all = "lowercase")]`, `#[derive(Default)]` で `Shell` を default）
  - `pub fn build_launch_command(backend: SessionBackend, shell: &str, name: &str, zellij_layout: &str, tmux_conf: &str) -> (String, Vec<String>)` — `(program, args)` を返す。

- [ ] **Step 1: 失敗するテストを書く**

`src/pty/backend.rs` 末尾に追加:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_backend_uses_shell_with_no_args() {
        let (prog, args) = build_launch_command(
            SessionBackend::Shell, "powershell.exe", "work", "L.kdl", "t.conf");
        assert_eq!(prog, "powershell.exe");
        assert!(args.is_empty());
    }

    #[test]
    fn zellij_backend_attach_or_create_with_layout() {
        let (prog, args) = build_launch_command(
            SessionBackend::Zellij, "powershell.exe", "work", "L.kdl", "t.conf");
        assert_eq!(prog, "zellij");
        assert_eq!(args, vec!["--session", "work", "--layout", "L.kdl"]);
    }

    #[test]
    fn tmux_backend_attach_or_create_with_conf() {
        let (prog, args) = build_launch_command(
            SessionBackend::Tmux, "powershell.exe", "work", "L.kdl", "t.conf");
        assert_eq!(prog, "tmux");
        assert_eq!(args, vec!["-f", "t.conf", "new-session", "-A", "-s", "work"]);
    }

    #[test]
    fn backend_default_is_shell() {
        assert_eq!(SessionBackend::default(), SessionBackend::Shell);
    }
}
```

> **注:** zellij の argv（`--session/--layout`）と tmux の argv（`-f/new-session/-A/-s`）は **Task 1 PoC findings の確定形を使う**。上のコードは PoC で確定する想定形。findings が別の形（例: `zellij attach -c <name>`）なら、テストと実装の argv をその形に合わせる。

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test --target-dir target-test backend:: 2>&1`（`run_in_background: true`、`TaskOutput` で取得）
Expected: コンパイルエラー（`build_launch_command` 未定義）。

- [ ] **Step 3: 最小実装**

`src/pty/backend.rs` 冒頭:

```rust
use serde::{Deserialize, Serialize};

/// セッション起動の backend 種別
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionBackend {
    #[default]
    Shell,
    Zellij,
    Tmux,
}

/// backend に応じた起動コマンド (program, args) を組み立てる。
/// シェル文字列連結はしない（argv 配列で CommandBuilder に渡す）。
/// name は is_valid_session_name で英数＋`-` に限定済みの前提。
pub fn build_launch_command(
    backend: SessionBackend,
    shell: &str,
    name: &str,
    zellij_layout: &str,
    tmux_conf: &str,
) -> (String, Vec<String>) {
    match backend {
        SessionBackend::Shell => (shell.to_string(), Vec::new()),
        SessionBackend::Zellij => (
            "zellij".to_string(),
            vec![
                "--session".to_string(),
                name.to_string(),
                "--layout".to_string(),
                zellij_layout.to_string(),
            ],
        ),
        SessionBackend::Tmux => (
            "tmux".to_string(),
            vec![
                "-f".to_string(),
                tmux_conf.to_string(),
                "new-session".to_string(),
                "-A".to_string(),
                "-s".to_string(),
                name.to_string(),
            ],
        ),
    }
}
```

`src/pty/mod.rs` に追加:

```rust
pub mod backend;
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test --target-dir target-test backend:: 2>&1`（background）
Expected: 4 tests PASS。

- [ ] **Step 5: commit**

```bash
git add src/pty/backend.rs src/pty/mod.rs
git commit -m "feat(pty): add SessionBackend enum and build_launch_command"
```

---

## Task 3: PtyManager::spawn を program+args 受け取りに拡張

**Files:**
- Modify: `src/pty/manager.rs:18-67`
- Modify: `src/pty/registry.rs:646-654`（呼び出し側）

**Interfaces:**
- Consumes: `build_launch_command` の `(String, Vec<String>)`。
- Produces: `PtyManager::spawn(program: &str, args: &[String], cols: u16, rows: u16, instance_id: &str) -> Result<PtySession, ...>`。

- [ ] **Step 1: 呼び出し側を新シグネチャに合わせて失敗させる**

`src/pty/registry.rs:646-654` の spawn 呼び出しを編集（この時点で manager 未変更なのでコンパイル失敗する）:

```rust
let pty = tokio::task::spawn_blocking({
    let shell = self.shell.clone();
    let instance_id = self.instance_id.clone();
    move || PtyManager::spawn(&shell, &[], cols, rows, &instance_id)
})
```

- [ ] **Step 2: コンパイル失敗を確認**

Run: `cargo build --target-dir target-test 2>&1`（background）
Expected: FAIL（`spawn` の引数不一致）。

- [ ] **Step 3: manager.rs を拡張**

`src/pty/manager.rs` の `spawn` シグネチャと cmd 構築部を変更:

```rust
pub fn spawn(
    program: &str,
    args: &[String],
    cols: u16,
    rows: u16,
    instance_id: &str,
) -> Result<PtySession, Box<dyn std::error::Error + Send + Sync>> {
    let pty_system = native_pty_system();

    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };

    #[cfg(windows)]
    let pids_before = snapshot_openconsole_pids();

    let pair = pty_system.openpty(size)?;

    let mut cmd = CommandBuilder::new(program);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.env("DEN_INSTANCE", instance_id);
    cmd.env("TERM", "xterm-256color");
    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        cmd.cwd(home);
    }

    let child = pair.slave.spawn_command(cmd)?;
    // ...（以降の drop/job/reader/writer は変更なし）
}
```

- [ ] **Step 4: ビルドと既存テスト**

Run: `cargo build --target-dir target-test 2>&1`（background）→ PASS
Run: `cargo test --target-dir target-test --test registry_test 2>&1`（background）
Expected: 既存 PTY テスト（Shell 経路）が PASS（spawn の引数変更のみ、挙動不変）。

- [ ] **Step 5: commit**

```bash
git add src/pty/manager.rs src/pty/registry.rs
git commit -m "refactor(pty): spawn accepts program and args vec"
```

---

## Task 4: layout ファイルの同梱と起動時書き出し

**Files:**
- Create: `frontend/layouts/den-bare.kdl`（Task 1 findings の中身）
- Create: `frontend/layouts/den.conf`（Task 1 findings の中身）
- Modify: `src/lib.rs`（起動時に DEN_DATA_DIR へ書き出す関数追加）

**Interfaces:**
- Produces: `pub fn ensure_mux_layouts(data_dir: &std::path::Path) -> (String, String)` — `(zellij_layout_path, tmux_conf_path)` を返し、ファイルを書き出す。

- [ ] **Step 1: layout ファイルを作成（PoC findings の中身）**

`frontend/layouts/den.conf`（PoC で bar 非表示が確認できた最終形。下は想定）:

```
set -g status off
set -g window-size latest
```

`frontend/layouts/den-bare.kdl`（PoC で確定した zellij bare layout。下は想定）:

```kdl
layout {
    pane
}
default_tab_template {
    children
}
```

> PoC findings の中身で上書きする。bar 非表示にならない形は使わない。

- [ ] **Step 2: 失敗するテストを書く**

`src/lib.rs` のテストモジュール（無ければ追加）:

```rust
#[cfg(test)]
mod mux_layout_tests {
    use super::*;

    #[test]
    fn ensure_mux_layouts_writes_files() {
        let dir = std::env::temp_dir().join("den-mux-layout-test");
        let _ = std::fs::create_dir_all(&dir);
        let (kdl, conf) = ensure_mux_layouts(&dir);
        assert!(std::path::Path::new(&kdl).exists());
        assert!(std::path::Path::new(&conf).exists());
        assert!(std::fs::read_to_string(&conf).unwrap().contains("status off"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 3: テスト失敗を確認**

Run: `cargo test --target-dir target-test ensure_mux_layouts 2>&1`（background）
Expected: FAIL（`ensure_mux_layouts` 未定義）。

- [ ] **Step 4: 実装**

`src/lib.rs` に rust-embed の既存 Asset を使って書き出す関数を追加（既存の embed 構造体名に合わせる。例 `Asset`）:

```rust
/// multiplexer 用 layout/conf を DEN_DATA_DIR に書き出し、絶対パスを返す。
/// 書き出し失敗時は warn ログを出し、空文字列パスを返す（呼び出し側は --layout 省略にフォールバック）。
pub fn ensure_mux_layouts(data_dir: &std::path::Path) -> (String, String) {
    fn write_embedded(data_dir: &std::path::Path, embedded: &str, out_name: &str) -> String {
        let path = data_dir.join(out_name);
        match Asset::get(embedded) {
            Some(file) => match std::fs::write(&path, file.data.as_ref()) {
                Ok(()) => path.to_string_lossy().into_owned(),
                Err(e) => {
                    tracing::warn!("Failed to write {out_name}: {e}");
                    String::new()
                }
            },
            None => {
                tracing::warn!("Embedded asset {embedded} missing");
                String::new()
            }
        }
    }
    let kdl = write_embedded(data_dir, "layouts/den-bare.kdl", "den-bare.kdl");
    let conf = write_embedded(data_dir, "layouts/den.conf", "den.conf");
    (kdl, conf)
}
```

> `Asset::get` のキー（`layouts/den-bare.kdl`）は rust-embed の `#[folder = "frontend"]` 設定に合わせる。既存の embed 設定（folder ルート）を Read で確認し、キーのプレフィックスを調整すること。

起動処理（`run`/`serve` 関数内、registry 構築前後）で呼び出し、結果を registry に渡す（Task 6 で使用）:

```rust
let (zellij_layout, tmux_conf) = ensure_mux_layouts(&data_dir);
```

- [ ] **Step 5: テストが通ることを確認**

Run: `cargo test --target-dir target-test ensure_mux_layouts 2>&1`（background）→ PASS

- [ ] **Step 6: commit**

```bash
git add frontend/layouts/den-bare.kdl frontend/layouts/den.conf src/lib.rs
git commit -m "feat(pty): embed and materialize tmux/zellij layout files"
```

---

## Task 5: SessionRecord.backend と永続化の backend 対応

**Files:**
- Modify: `src/store.rs:113-117`
- Modify: `src/pty/registry.rs:330-352`（upsert）, `:344-345`（push）

**Interfaces:**
- Consumes: `SessionBackend`（Task 2）。
- Produces: `SessionRecord { name, ssh, backend: Option<SessionBackend> }`、`upsert_saved_record(&self, name, ssh, backend)`。

- [ ] **Step 1: 失敗するテスト（serde 後方互換）**

`src/store.rs` のテストに追加:

```rust
#[test]
fn session_record_backend_defaults_to_none_when_absent() {
    // backend キーが無い旧 JSON でもデシリアライズできる
    let json = r#"{"name":"work"}"#;
    let rec: SessionRecord = serde_json::from_str(json).unwrap();
    assert_eq!(rec.name, "work");
    assert!(rec.backend.is_none());
}

#[test]
fn session_record_backend_roundtrips() {
    let rec = SessionRecord {
        name: "work".to_string(),
        ssh: None,
        backend: Some(crate::pty::backend::SessionBackend::Zellij),
    };
    let json = serde_json::to_string(&rec).unwrap();
    let back: SessionRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back.backend, Some(crate::pty::backend::SessionBackend::Zellij));
}
```

- [ ] **Step 2: テスト失敗を確認**

Run: `cargo test --target-dir target-test session_record_backend 2>&1`（background）
Expected: FAIL（`backend` フィールド無し）。

- [ ] **Step 3: SessionRecord にフィールド追加**

`src/store.rs:113`:

```rust
pub struct SessionRecord {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<crate::pty::registry::SshSessionConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<crate::pty::backend::SessionBackend>,
}
```

- [ ] **Step 4: upsert_saved_record を backend 対応に**

`src/pty/registry.rs:330` のシグネチャと中身:

```rust
async fn upsert_saved_record(
    &self,
    name: &str,
    ssh: Option<SshSessionConfig>,
    backend: Option<crate::pty::backend::SessionBackend>,
) -> Result<(), String> {
    let Some(ref store) = self.store else { return Ok(()); };
    let store = store.clone();
    let name = name.to_string();
    tokio::task::spawn_blocking(move || {
        let mut records = store.load_sessions();
        if let Some(record) = records.iter_mut().find(|r| r.name == name) {
            record.ssh = ssh;
            record.backend = backend;
        } else {
            records.push(crate::store::SessionRecord { name, ssh, backend });
        }
        store.save_sessions(&records)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}
```

既存の呼び出し（`create_with_ssh` 内 `:709-711` と sync ループ `:1072` 付近）を一旦 `None` backend で更新してコンパイルを通す（Task 6 で create_with_backend が正しい backend を渡す）:

```rust
self.upsert_saved_record(name, session.ssh_config.clone(), None)
```

`rename_saved_record` は backend を保持（name だけ変更なので既存ロジックで `record.backend` は触らず温存される）。

- [ ] **Step 5: ビルド＆テスト**

Run: `cargo test --target-dir target-test session_record_backend 2>&1`（background）→ PASS
Run: `cargo build --target-dir target-test 2>&1`（background）→ PASS

- [ ] **Step 6: commit**

```bash
git add src/store.rs src/pty/registry.rs
git commit -m "feat(store): persist session backend in SessionRecord"
```

---

## Task 6: Registry::create_with_backend

**Files:**
- Modify: `src/pty/registry.rs`（`create_with_backend` 追加、registry に layout パス保持、`new` 拡張）

**Interfaces:**
- Consumes: `build_launch_command`（Task 2）, `PtyManager::spawn(program,args,...)`（Task 3）, layout パス（Task 4）。
- Produces: `pub async fn create_with_backend(&self, name: &str, cols: u16, rows: u16, backend: SessionBackend) -> Result<(Arc<SharedSession>, broadcast::Receiver<Vec<u8>>), RegistryError>`。

- [ ] **Step 1: registry に layout パスフィールドを追加**

`SessionRegistry` struct に追加（`shell` の隣）:

```rust
zellij_layout: String,
tmux_conf: String,
```

`new` のシグネチャに `zellij_layout: String, tmux_conf: String` を追加し、struct 初期化に含める。呼び出し側（`src/lib.rs` の `SessionRegistry::new(...)`）に Task 4 の `(zellij_layout, tmux_conf)` を渡す。

- [ ] **Step 2: Shell 経路の create_with_backend を検証するテスト**

`tests/registry_test.rs` に既存治具（`build_test_runtime` / `#[test]`）に倣って追加:

```rust
#[test]
fn create_with_backend_shell_spawns_session() {
    let rt = build_test_runtime();
    rt.block_on(async {
        let reg = test_registry(); // 既存ヘルパに準拠（store=None でよい）
        let res = reg
            .create_with_backend("becktest", 80, 24,
                den::pty::backend::SessionBackend::Shell)
            .await;
        assert!(res.is_ok());
        // クリーンアップ
        let _ = reg.destroy("becktest").await;
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
}
```

> 既存 `tests/registry_test.rs` のヘルパ名（registry 構築・destroy）に合わせて調整。crate 名は `den`（要確認）。

- [ ] **Step 3: テスト失敗を確認**

Run: `cargo test --target-dir target-test --test registry_test create_with_backend_shell 2>&1`（background）
Expected: FAIL（`create_with_backend` 未定義）。

- [ ] **Step 4: create_with_backend を実装**

`create_with_ssh` を複製せず、内部の spawn 部分を backend 対応にする。`create_with_ssh` は無改変のまま、新メソッドを追加:

```rust
pub async fn create_with_backend(
    &self,
    name: &str,
    cols: u16,
    rows: u16,
    backend: crate::pty::backend::SessionBackend,
) -> Result<(Arc<SharedSession>, broadcast::Receiver<Vec<u8>>), RegistryError> {
    if !is_valid_session_name(name) {
        return Err(RegistryError::InvalidName(name.to_string()));
    }
    {
        let sessions = self.sessions.read().await;
        if sessions.contains_key(name) {
            return Err(RegistryError::AlreadyExists(name.to_string()));
        }
        if sessions.len() >= MAX_SESSIONS {
            return Err(RegistryError::LimitExceeded);
        }
    }

    let (program, args) = crate::pty::backend::build_launch_command(
        backend, &self.shell, name, &self.zellij_layout, &self.tmux_conf);

    // layout パスが空（書き出し失敗）かつ mux backend のときは --layout/-f を外す
    let (program, args) = sanitize_missing_layout(backend, program, args,
        &self.zellij_layout, &self.tmux_conf);

    let pty = tokio::task::spawn_blocking({
        let instance_id = self.instance_id.clone();
        move || PtyManager::spawn(&program, &args, cols, rows, &instance_id)
    })
    .await
    .map_err(|e| RegistryError::SpawnFailed(e.to_string()))?
    .map_err(|e| RegistryError::SpawnFailed(e.to_string()))?;

    let (session, first_rx, monitor_handle) = Self::setup_pty_session(
        name, pty.reader, pty.writer, pty.master, pty.child,
        #[cfg(windows)] pty.job,
        Arc::clone(&self.last_activity), None);
    session.inner.lock().await.monitor_handle = Some(monitor_handle);

    // 権威的挿入（create_with_ssh と同一の TOCTOU 防止ブロックを踏襲）
    let session_count = {
        let mut sessions = self.sessions.write().await;
        let race_err = if sessions.contains_key(name) {
            Some(RegistryError::AlreadyExists(name.to_string()))
        } else if sessions.len() >= MAX_SESSIONS {
            Some(RegistryError::LimitExceeded)
        } else { None };
        if let Some(err) = race_err {
            session.alive.store(false, Ordering::Release);
            let (resize_handle, monitor_handle) = {
                let mut inner = session.inner.lock().await;
                if let Some(mut child) = inner.child.take() {
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = child.kill();
                        let _ = child.wait();
                    }).await;
                }
                inner.pty_writer = Box::new(std::io::sink());
                inner.resize_tx.take();
                (inner.resize_handle.take(), inner.monitor_handle.take())
            };
            if let Some(h) = monitor_handle {
                let _ = tokio::time::timeout(TASK_JOIN_TIMEOUT, h).await;
            }
            if let Some(h) = resize_handle {
                let _ = tokio::time::timeout(TASK_JOIN_TIMEOUT, h).await;
            }
            return Err(err);
        }
        sessions.insert(name.to_string(), Arc::clone(&session));
        sessions.len()
    };

    self.evaluate_sleep_prevention(session_count);
    tracing::info!("Session created: {name} (backend={backend:?})");
    if let Err(e) = self.upsert_saved_record(name, None, Some(backend)).await {
        tracing::warn!("Failed to persist saved session '{name}': {e}");
    }
    Ok((session, first_rx))
}
```

`sanitize_missing_layout` ヘルパ（layout パスが空なら mux の `--layout`/`-f <conf>` を除去）:

```rust
fn sanitize_missing_layout(
    backend: crate::pty::backend::SessionBackend,
    program: String,
    args: Vec<String>,
    zellij_layout: &str,
    tmux_conf: &str,
) -> (String, Vec<String>) {
    use crate::pty::backend::SessionBackend;
    let args = match backend {
        SessionBackend::Zellij if zellij_layout.is_empty() => {
            // --layout <path> を除去
            strip_pair(args, "--layout")
        }
        SessionBackend::Tmux if tmux_conf.is_empty() => {
            // -f <conf> を除去
            strip_pair(args, "-f")
        }
        _ => args,
    };
    (program, args)
}

fn strip_pair(args: Vec<String>, flag: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut skip = false;
    for a in args {
        if skip { skip = false; continue; }
        if a == flag { skip = true; continue; }
        out.push(a);
    }
    out
}
```

- [ ] **Step 5: テスト＆ゲート**

Run: `cargo test --target-dir target-test --test registry_test create_with_backend_shell 2>&1`（background）→ PASS
Run: `cargo test --target-dir target-test 2>&1`（background）→ 全 PASS

- [ ] **Step 6: commit**

```bash
git add src/pty/registry.rs src/lib.rs
git commit -m "feat(pty): add create_with_backend for multiplexer sessions"
```

---

## Task 7: availability probe と list_mux_sessions パーサ

**Files:**
- Modify: `src/pty/backend.rs`

**Interfaces:**
- Produces:
  - `pub fn probe_available(backend: SessionBackend) -> bool`（`--version`/`-V` を実行）
  - `pub fn parse_zellij_ls(output: &str) -> Vec<String>`
  - `pub fn parse_tmux_ls(output: &str) -> Vec<String>`
  - `pub fn list_mux_sessions(backend: SessionBackend) -> Vec<String>`（probe→ls 実行→パース、失敗時空）

- [ ] **Step 1: パーサの失敗テスト**

`src/pty/backend.rs` の tests に追加:

```rust
#[test]
fn parse_zellij_ls_extracts_names() {
    // zellij ls は "name [Created ...] (current)" 等の行。ANSI/装飾は除去前提のサンプル。
    let out = "main [Created 1h ago]\nwork [Created 2m ago] (EXITED - attach to resurrect)\n";
    let names = parse_zellij_ls(out);
    assert_eq!(names, vec!["main", "work"]);
}

#[test]
fn parse_tmux_ls_extracts_names() {
    // tmux ls は "name: N windows (created ...) ..." 形式
    let out = "main: 1 windows (created Sat) [80x24]\nagent: 2 windows (created Sat)\n";
    let names = parse_tmux_ls(out);
    assert_eq!(names, vec!["main", "agent"]);
}

#[test]
fn parse_handles_empty() {
    assert!(parse_zellij_ls("").is_empty());
    assert!(parse_tmux_ls("").is_empty());
}
```

> zellij/tmux の実 `ls` 出力形は Task 1 PoC findings で確認したサンプルに合わせる（ANSI エスケープが付く場合は実装でストリップ）。

- [ ] **Step 2: テスト失敗を確認**

Run: `cargo test --target-dir target-test backend::tests::parse 2>&1`（background）
Expected: FAIL（パーサ未定義）。

- [ ] **Step 3: パーサと probe を実装**

```rust
use std::process::Command;

/// "name: ..." 形式（tmux）からセッション名を抽出
pub fn parse_tmux_ls(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split(':').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// 行頭トークン（zellij）からセッション名を抽出。ANSI と装飾を落とす。
pub fn parse_zellij_ls(output: &str) -> Vec<String> {
    output
        .lines()
        .map(strip_ansi)
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .filter(|s| !s.is_empty())
        .collect()
}

fn strip_ansi(line: &str) -> String {
    // 簡易 ANSI ストリップ（ESC[...m）
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC シーケンスを 'm' まで読み飛ばす
            for n in chars.by_ref() {
                if n == 'm' { break; }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn probe_available(backend: SessionBackend) -> bool {
    let (prog, flag) = match backend {
        SessionBackend::Shell => return true,
        SessionBackend::Zellij => ("zellij", "--version"),
        SessionBackend::Tmux => ("tmux", "-V"),
    };
    Command::new(prog)
        .arg(flag)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn list_mux_sessions(backend: SessionBackend) -> Vec<String> {
    let (prog, args): (&str, &[&str]) = match backend {
        SessionBackend::Shell => return Vec::new(),
        SessionBackend::Zellij => ("zellij", &["list-sessions", "--no-formatting"]),
        SessionBackend::Tmux => ("tmux", &["ls"]),
    };
    let output = match Command::new(prog).args(args).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return Vec::new(),
    };
    match backend {
        SessionBackend::Zellij => parse_zellij_ls(&output),
        SessionBackend::Tmux => parse_tmux_ls(&output),
        SessionBackend::Shell => Vec::new(),
    }
}
```

> `zellij list-sessions --no-formatting` の正確なフラグは PoC findings に合わせる（装飾無効化フラグが別名なら調整。無ければ `strip_ansi` で吸収）。

- [ ] **Step 4: テスト通過確認**

Run: `cargo test --target-dir target-test backend::tests::parse 2>&1`（background）→ PASS

- [ ] **Step 5: commit**

```bash
git add src/pty/backend.rs
git commit -m "feat(pty): probe multiplexer availability and parse session lists"
```

---

## Task 8: GET /api/multiplexer/status エンドポイント

**Files:**
- Create: `src/multiplexer_api.rs`
- Modify: `src/lib.rs`（`mod multiplexer_api;` ＋ ルート追加）

**Interfaces:**
- Consumes: `probe_available`, `list_mux_sessions`（Task 7）。
- Produces: `GET /api/multiplexer/status` → `{ "zellij": {"available": bool, "sessions": [String]}, "tmux": {...} }`。probe は `OnceLock` でキャッシュ、sessions は都度。

- [ ] **Step 1: probe キャッシュと availability の単体テスト**

`src/multiplexer_api.rs` の tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_payload_serializes() {
        let payload = MultiplexerStatus {
            zellij: BackendStatus { available: true, sessions: vec!["main".into()] },
            tmux: BackendStatus { available: false, sessions: vec![] },
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"available\":true"));
        assert!(json.contains("\"main\""));
    }
}
```

- [ ] **Step 2: テスト失敗を確認**

Run: `cargo test --target-dir target-test multiplexer_api 2>&1`（background）
Expected: FAIL（型未定義）。

- [ ] **Step 3: ハンドラ実装**

`src/multiplexer_api.rs`:

```rust
use crate::pty::backend::{list_mux_sessions, probe_available, SessionBackend};
use axum::Json;
use serde::Serialize;
use std::sync::OnceLock;

#[derive(Serialize)]
pub struct BackendStatus {
    pub available: bool,
    pub sessions: Vec<String>,
}

#[derive(Serialize)]
pub struct MultiplexerStatus {
    pub zellij: BackendStatus,
    pub tmux: BackendStatus,
}

/// availability は起動後不変とみなしキャッシュ（lazy）
fn availability() -> &'static (bool, bool) {
    static AVAIL: OnceLock<(bool, bool)> = OnceLock::new();
    AVAIL.get_or_init(|| {
        (
            probe_available(SessionBackend::Zellij),
            probe_available(SessionBackend::Tmux),
        )
    })
}

/// GET /api/multiplexer/status
pub async fn status() -> Json<MultiplexerStatus> {
    let (zellij_ok, tmux_ok) = *availability();
    // ls は blocking。spawn_blocking で。
    let zellij_sessions = if zellij_ok {
        tokio::task::spawn_blocking(|| list_mux_sessions(SessionBackend::Zellij))
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let tmux_sessions = if tmux_ok {
        tokio::task::spawn_blocking(|| list_mux_sessions(SessionBackend::Tmux))
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Json(MultiplexerStatus {
        zellij: BackendStatus { available: zellij_ok, sessions: zellij_sessions },
        tmux: BackendStatus { available: tmux_ok, sessions: tmux_sessions },
    })
}
```

`src/lib.rs`: `mod multiplexer_api;` 追加、protected_routes に追加:

```rust
.route("/api/multiplexer/status", get(multiplexer_api::status))
```

- [ ] **Step 4: テスト＆ビルド**

Run: `cargo test --target-dir target-test multiplexer_api 2>&1`（background）→ PASS
Run: `cargo build --target-dir target-test 2>&1`（background）→ PASS

- [ ] **Step 5: commit**

```bash
git add src/multiplexer_api.rs src/lib.rs
git commit -m "feat(api): add GET /api/multiplexer/status endpoint"
```

---

## Task 9: create_session ハンドラに backend を配線

**Files:**
- Modify: `src/ws.rs:239-242`（`CreateSessionRequest`）, `:319-379`（`create_session`）

**Interfaces:**
- Consumes: `create_with_backend`（Task 6）, `SessionBackend`（Task 2）。
- Produces: `POST /api/terminal/sessions { name, ssh?, backend? }`。

- [ ] **Step 1: backend 経路を検証する最小テスト（リクエストのデシリアライズ）**

`src/ws.rs` の tests に追加:

```rust
#[test]
fn create_session_request_parses_backend() {
    let json = r#"{"name":"work","backend":"zellij"}"#;
    let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.backend, Some(crate::pty::backend::SessionBackend::Zellij));
}

#[test]
fn create_session_request_backend_absent_is_none() {
    let json = r#"{"name":"work"}"#;
    let req: CreateSessionRequest = serde_json::from_str(json).unwrap();
    assert!(req.backend.is_none());
}
```

- [ ] **Step 2: テスト失敗を確認**

Run: `cargo test --target-dir target-test create_session_request 2>&1`（background）
Expected: FAIL（`backend` フィールド無し）。

- [ ] **Step 3: CreateSessionRequest に backend 追加 ＋ ハンドラ分岐**

`src/ws.rs:239`:

```rust
#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub name: String,
    pub ssh: Option<CreateSessionSsh>,
    #[serde(default)]
    pub backend: Option<crate::pty::backend::SessionBackend>,
}
```

`create_session` ハンドラ冒頭で ssh と backend の優先順位を決める。ssh 指定があれば従来どおり `create_with_ssh`、無ければ backend（省略時 Shell）で `create_with_backend`:

```rust
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    // SSH 指定時は従来の ssh 経路（無改変）
    if req.ssh.is_some() {
        // ...（既存 ssh 経路をそのまま。ssh_config 構築〜inject まで現状コードを維持）...
        // 既存実装をこのブロックに移す
        return create_session_ssh(state, req).await;
    }

    // backend 経路
    let backend = req.backend.unwrap_or_default();
    match state.registry.create_with_backend(&req.name, 80, 24, backend).await {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(RegistryError::LimitExceeded) => {
            (StatusCode::TOO_MANY_REQUESTS, "Session limit exceeded").into_response()
        }
        Err(RegistryError::AlreadyExists(_)) => {
            // 1:1 同名 create-or-attach: 既存なら 200 を返し frontend は switch のみ
            StatusCode::OK.into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}
```

既存の ssh 処理本体を `async fn create_session_ssh(state, req) -> Response` に切り出す（ロジック不変、ssh パス無改変の原則を守る）。

> **AlreadyExists → 200** にする理由: frontend の `createSession` は `resp.ok || 201` を成功扱い。既存セッション attach 時は 200 を返すことで「作成済み＝合流」を成功として扱える。201=新規作成、200=既存合流、と区別。

- [ ] **Step 4: テスト＆ゲート**

Run: `cargo test --target-dir target-test create_session_request 2>&1`（background）→ PASS
Run: `cargo test --target-dir target-test 2>&1`（background）→ 全 PASS
Run: `cargo clippy --target-dir target-test -- -D warnings 2>&1`（background）→ 0 warnings

- [ ] **Step 5: commit**

```bash
git add src/ws.rs
git commit -m "feat(api): wire backend field into session creation"
```

---

## Task 10: remote.rs allowlist に multiplexer/ 追加

**Files:**
- Modify: `src/remote.rs:430-435`

**Interfaces:**
- Produces: `/api/remote/{id}/multiplexer/status` が denB の `/api/multiplexer/status` にプロキシされる。

- [ ] **Step 1: allowlist テストを追加（失敗）**

`src/remote.rs` の tests に、allowlist ロジックを純関数化して検証。まず `path_in_allowlist(rest: &str) -> bool` を切り出す前提のテスト:

```rust
#[test]
fn allowlist_permits_multiplexer() {
    assert!(path_in_allowlist("multiplexer/status"));
    assert!(path_in_allowlist("terminal/sessions"));
    assert!(path_in_allowlist("filer/list"));
    assert!(!path_in_allowlist("settings"));
}
```

- [ ] **Step 2: テスト失敗を確認**

Run: `cargo test --target-dir target-test allowlist_permits_multiplexer 2>&1`（background）
Expected: FAIL（`path_in_allowlist` 未定義）。

- [ ] **Step 3: allowlist を純関数化して multiplexer/ を許可**

`src/remote.rs`:

```rust
fn path_in_allowlist(rest: &str) -> bool {
    rest.starts_with("terminal/")
        || rest.starts_with("filer/")
        || rest.starts_with("multiplexer/")
}
```

`remote_proxy_catch_all` の allowlist 部を置換:

```rust
let path = if path_in_allowlist(&rest) {
    format!("/api/{rest}")
} else {
    return Err(StatusCode::FORBIDDEN);
};
```

- [ ] **Step 4: テスト通過**

Run: `cargo test --target-dir target-test allowlist 2>&1`（background）→ PASS

- [ ] **Step 5: commit**

```bash
git add src/remote.rs
git commit -m "feat(remote): proxy multiplexer endpoints to remote Den"
```

---

## Task 11: Settings に default_backend select

**Files:**
- Modify: `frontend/index.html`（`setting-terminal-renderer` の近く）
- Modify: `frontend/js/settings.js`（既定値＋保存/復元）

**Interfaces:**
- Produces: `DenSettings.get('default_backend')`（`'shell'|'zellij'|'tmux'`、既定 `'shell'`）。

- [ ] **Step 1: index.html に select 追加**

`frontend/index.html` の `setting-terminal-renderer`（行 346 付近）の直後に、同じ `.settings-row` パターンで:

```html
<div class="settings-row">
  <label for="setting-default-backend">Default backend</label>
  <select id="setting-default-backend" class="settings-input">
    <option value="shell">Shell</option>
    <option value="zellij">Zellij</option>
    <option value="tmux">tmux</option>
  </select>
</div>
```

> 既存の `setting-terminal-renderer` 行の `.settings-row`/label 構造を Read で確認し、完全に同じマークアップに合わせる。

- [ ] **Step 2: settings.js に既定値追加**

`frontend/js/settings.js` の defaults オブジェクト（行 12-23 付近、`terminal_renderer: null` の隣）:

```js
default_backend: 'shell',
```

- [ ] **Step 3: 保存/復元の配線**

`settings.js` の「モーダルを開くとき値を反映」箇所（`setting-theme` を value 設定している行 613 付近）に追加:

```js
const backendSelect = document.getElementById('setting-default-backend');
if (backendSelect) backendSelect.value = current.default_backend || 'shell';
```

保存ハンドラ（他の select を `current.x = el.value` で集めている箇所）に追加:

```js
const backendEl = document.getElementById('setting-default-backend');
if (backendEl) next.default_backend = backendEl.value;
```

> 既存の save ロジックの変数名（`next`/`updated` 等）に合わせる。Read で確認。

- [ ] **Step 4: ESLint**

Run: `npx eslint frontend/js/settings.js 2>&1`（background）
Expected: 0 errors。

- [ ] **Step 5: commit**

```bash
git add frontend/index.html frontend/js/settings.js
git commit -m "feat(settings): add default terminal backend selector"
```

---

## Task 12: showNewSessionMenu に backend submenu

**Files:**
- Modify: `frontend/js/terminal.js:1074`（`createSession`）, `:1461`（`showNewSessionMenu`）

**Interfaces:**
- Consumes: `GET /api/multiplexer/status`（Task 8）, `default_backend`（Task 11）。
- Produces: `createSession(name, opts)` の `opts.backend`、`buildBackendSubmenu(status, remoteConnId)`。

- [ ] **Step 1: createSession に backend を追加**

`frontend/js/terminal.js:1074` の `createSession` を opts 対応に拡張（後方互換: 第2引数が object なら opts、ssh は opts.ssh）。最小変更で backend を載せる:

```js
async function createSession(name, sshConfig, remote, backend) {
  try {
    const body = { name };
    if (sshConfig) body.ssh = sshConfig;
    if (backend && backend !== 'shell') body.backend = backend;
    const base = sessionApiBase(remote);
    const resp = await fetch(`${base}/terminal/sessions`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    return resp.ok || resp.status === 201;
  } catch (_) {
    return false;
  }
}
```

> 既存呼び出し（`createSession(trimmed)` 等）は引数追加だけなので不変。

- [ ] **Step 2: status 取得ヘルパ**

`showNewSessionMenu` 内で local/remote の status を取る関数を追加:

```js
async function fetchMuxStatus(remoteConnId) {
  try {
    const base = remoteConnId ? `/api/remote/${remoteConnId}` : '/api';
    const resp = await fetch(`${base}/multiplexer/status`, { credentials: 'same-origin' });
    if (!resp.ok) return null;
    return await resp.json();
  } catch (_) { return null; }
}
```

- [ ] **Step 3: submenu ビルダ**

```js
// status = { zellij:{available,sessions}, tmux:{available,sessions} }
function buildBackendSubmenu(menu, status, remoteConnId, closeMenu) {
  if (!status) return;
  for (const kind of ['zellij', 'tmux']) {
    const bs = status[kind];
    if (!bs || !bs.available) continue;

    const sep = document.createElement('div');
    sep.className = 'new-session-menu-separator';
    sep.textContent = kind === 'zellij' ? 'Zellij' : 'tmux';
    menu.appendChild(sep);

    // 既存セッション（attach）
    for (const name of bs.sessions) {
      const item = document.createElement('div');
      item.className = 'new-session-menu-item';
      item.textContent = name;
      item.addEventListener('click', async () => {
        closeMenu();
        const ok = await createSession(name, null, remoteConnId, kind);
        if (!ok) { Toast.error('Failed to attach session'); return; }
        lastSessionsKey = '';
        await refreshSessionList();
        switchSession(name, remoteConnId || undefined);
      });
      menu.appendChild(item);
    }

    // 新規作成
    const newItem = document.createElement('div');
    newItem.className = 'new-session-menu-item';
    newItem.textContent = `New ${kind} session…`;
    newItem.addEventListener('click', async () => {
      closeMenu();
      const name = await Toast.prompt('Session name:');
      if (!name || !name.trim()) return;
      const trimmed = name.trim();
      const validationError = validateSessionName(trimmed);
      if (validationError) { Toast.error(validationError); return; }
      const ok = await createSession(trimmed, null, remoteConnId, kind);
      if (!ok) { Toast.error('Failed to create session'); return; }
      lastSessionsKey = '';
      await refreshSessionList();
      switchSession(trimmed, remoteConnId || undefined);
    });
    menu.appendChild(newItem);
  }
}
```

- [ ] **Step 4: showNewSessionMenu に組み込む**

`Local Terminal` の click を default_backend 解決に変更:

```js
localItem.addEventListener('click', async () => {
  closeMenu();
  const name = await Toast.prompt('Session name:');
  if (!name || !name.trim()) return;
  const trimmed = name.trim();
  const validationError = validateSessionName(trimmed);
  if (validationError) { Toast.error(validationError); return; }
  const backend = (typeof DenSettings !== 'undefined'
    ? DenSettings.get('default_backend') : 'shell') || 'shell';
  const ok = await createSession(trimmed, null, null, backend);
  if (!ok) { Toast.error('Failed to create session'); return; }
  lastSessionsKey = '';
  await refreshSessionList();
  switchSession(trimmed);
});
```

`localItem` 追加直後に local の backend submenu を挿入:

```js
const localStatus = await fetchMuxStatus(null);
buildBackendSubmenu(menu, localStatus, null, closeMenu);
```

各 Remote セクション（`denConns` ループ内、`New Terminal` の後）に remote の submenu:

```js
const remoteStatus = await fetchMuxStatus(connId);
buildBackendSubmenu(menu, remoteStatus, connId, closeMenu);
```

> `closeMenu` は後方で定義される `let closeMenu;` に依存。既存コード同様、click ハンドラ実行時には代入済みなのでクロージャ参照で OK。

- [ ] **Step 5: ESLint＆手動確認**

Run: `npx eslint frontend/js/terminal.js 2>&1`（background）→ 0 errors

dev サーバ（別ポート）で起動し、メニューに backend submenu が出ることを目視（mux 無し環境では submenu 非表示＝可用 backend のみ）:

```powershell
$env:DEN_PASSWORD="test"; $env:DEN_DATA_DIR="./data-dev"; $env:DEN_PORT="3940"; cargo run
```

- [ ] **Step 6: commit**

```bash
git add frontend/js/terminal.js
git commit -m "feat(terminal): backend submenu in new-session menu (local + remote)"
```

---

## Task 13: e2e — メニュー構造（status モック）

**Files:**
- Create/Modify: `tests/e2e/multiplexer-menu.e2e.ts`（新規）または `filer-ui.e2e.ts` に追記

**Interfaces:**
- Consumes: `/api/multiplexer/status` をルートインターセプトでモック。

- [ ] **Step 1: e2e テストを書く**

`/api/multiplexer/status` を `page.route` でモックし、submenu が描画されることを検証:

```ts
test('new-session menu shows backend submenus when available (#multiplexer)', async ({ page }) => {
  await page.route('**/api/multiplexer/status', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        zellij: { available: true, sessions: ['main', 'work'] },
        tmux: { available: false, sessions: [] },
      }),
    }));
  await login(page); // 既存ヘルパ
  await page.click('#session-new-btn');
  const menu = page.locator('#new-session-menu');
  await expect(menu).toBeVisible();
  // zellij セクションと既存セッションが出る
  await expect(menu.getByText('Zellij', { exact: true })).toBeVisible();
  await expect(menu.getByText('main', { exact: true })).toBeVisible();
  // tmux は available:false なので出ない
  await expect(menu.getByText('tmux', { exact: true })).toHaveCount(0);
});
```

> 既存 e2e（`tests/e2e/filer-ui.e2e.ts`）の login ヘルパ・サーバ起動の仕組みに合わせる。

- [ ] **Step 2: テスト失敗を確認**

Run: `npx playwright test tests/e2e/multiplexer-menu.e2e.ts 2>&1`（background）
Expected: FAIL（submenu まだ未実装なら、または前タスク完了済みなら PASS。未完なら描画されず FAIL）。

- [ ] **Step 3: 必要なら frontend を微修正して通す**

Task 12 が正しければ通る。通らなければセレクタ/描画タイミングを調整（status fetch は menu open 時 await なので、`page.click` 後に menu が出るまで待つ）。

- [ ] **Step 4: 既存 e2e リグレッション**

Run: `npx playwright test tests/e2e/filer-ui.e2e.ts 2>&1`（background）→ 全 PASS

- [ ] **Step 5: commit**

```bash
git add tests/e2e/multiplexer-menu.e2e.ts
git commit -m "test(e2e): verify backend submenu rendering with mocked status"
```

---

## Task 14: 全ゲート ＋ 実機スモーク（手動）

**Files:** なし（検証のみ）

- [ ] **Step 1: 品質ゲート一括**

Run（それぞれ background → TaskOutput）:
- `cargo fmt -- --check`
- `cargo clippy --target-dir target-test -- -D warnings`
- `cargo test --target-dir target-test`
- `npx playwright test tests/e2e/filer-ui.e2e.ts tests/e2e/multiplexer-menu.e2e.ts`

すべて緑を確認。

- [ ] **Step 2: 実機スモーク（主作業ホスト＋iPad）**

findings に沿って確認:
- PC で `Local Terminal`（既定 zellij）→ セッション作成 → iPad の同名 attach で合流（multiplayer）。
- PC↔iPad で交互に操作 → latest-active サイズ追従（tmux）/ zellij は findings の結論どおり。
- Den を再起動 → 同名セッションを開く → zellij/tmux 側が生きていてスクロールバックごと復活。
- iPad でタブを閉じる（destroy）→ PC 側セッションが死なない（detach のみ）。

- [ ] **Step 3: renderer 切替スモーク（restty/wterm）**

`.claude/rules/workflow.md` に従い、mux セッションを restty / wterm で開いて初期描画・CJK・theme・入力エコーを確認（vendor bump ではないが 2 インスタンス同時稼働の描画確認）。chrome-cdp 手順は `memory/patterns.md`。

- [ ] **Step 4: remote スモーク**

別ホストの Den（例: nix 箱 tmux）に Quick Connect → Remote セクションに tmux submenu が出る → 既存セッション attach → denB で動作。

- [ ] **Step 5: spec の TBD を確定値で更新 ＋ commit**

PoC findings と実機スモークの結果で spec の「未解決事項」を確定値に更新（特に zellij の clamp 挙動）。

```bash
git add docs/superpowers/specs/2026-06-20-multiplexer-session-backend-design.md
git commit -m "docs: resolve multiplexer design TBDs with PoC and smoke results"
```

- [ ] **Step 6: コードレビュー（`/code-review`）**

`remote.rs` allowlist・session 作成境界に触れるため effort: high。finding の対応判断は `.claude/rules/review-judgement.md`。`/security-review` も併用（auth/session 境界の確認、ただし ssh パスは無改変）。

---

## Self-Review（プラン作成者によるチェック結果）

**Spec coverage:** spec の各要素 → タスク対応:
- ①検出/fallback → Task 7（probe/list）, Task 8（status）, Task 6（sanitize_missing_layout fallback）, Task 9（ssh無し→backend, AlreadyExists→200）
- レイアウト bare → Task 1（PoC 確定）, Task 4（embed/write）
- ②関係（既定=mux/設定切替） → Task 11（設定）, Task 12（Local Terminal が default_backend で wrap）
- ③1:1 同名 → Task 9（AlreadyExists→200）, Task 12（attach=同名 createSession）
- ④clamp latest-active → Task 1（PoC）, Task 4（tmux window-size latest）
- remote → Task 10（allowlist）, Task 12（remote submenu）
- backend 永続化 → Task 5
- destroy=detach → Task 1（PoC で確認）、コード変更不要（既存 child.kill = attach detach）
- テスト戦略 → Task 2/4/5/6/7/8/9/10（unit）, Task 13（e2e）, Task 1/14（実機）

**Placeholder scan:** zellij/tmux の正確な argv・ls 出力形・layout 中身は Task 1 PoC findings に依存。これは「empirical gate」であり TODO ではない。各依存箇所に「PoC findings の確定形を使う」と明記済み。それ以外に TBD/TODO 無し。

**Type consistency:** `SessionBackend`（Task 2）→ `SessionRecord.backend`（Task 5）→ `create_with_backend`（Task 6）→ `CreateSessionRequest.backend`（Task 9）で一貫。`build_launch_command(backend, shell, name, zellij_layout, tmux_conf)` の引数順は Task 2 定義と Task 6 呼び出しで一致。`list_mux_sessions`/`probe_available`（Task 7）→ `multiplexer_api`（Task 8）で一致。`path_in_allowlist`（Task 10）一貫。

**既知の実装時確認項目（コードを読んで合わせる）:**
- rust-embed の Asset 構造体名と folder ルート（Task 4 のキー prefix）
- `tests/registry_test.rs` のヘルパ名・crate 名（Task 6）
- `settings.js` の save ロジック変数名（Task 11）
- e2e の login ヘルパ・サーバ起動（Task 13）
