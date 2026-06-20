# Multiplexer UX Enhancements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** zellij/tmux session backend の運用摩擦 5 点を解消する（コマンドバー復活 / ＋メニュー再設計 / セッション管理モーダル / tmux 下部欠け）。

**Architecture:** ① zellij は起動引数から `-l den-bare.kdl` を外し（＝デフォルトレイアウト＝バー有り）、tmux は `den.conf` から `set -g status off` を消すだけでバーを復活させる。`default_shell`/`clear-defaults`/`prefix None` の「キー無効化」は維持＝`56567a6` の反転ではない。② ＋メニューをマシン単位でグループ化し backend をアイコン区別。③ kill/delete/rename API ＋ Den ローカルエイリアス（mux 実体は不変）＋ Sessions モーダル。remote 透過は既存 `multiplexer/` allowlist で追加変更なし。④ tmux 下部欠けは別トラックで remote nix 調査。

**Tech Stack:** Rust (axum + portable-pty), 素の HTML/CSS/JS, rust-embed, Playwright e2e。

## Global Constraints

- **`DEN_DATA_DIR=./data-dev` 厳守**（`./data` は本番。上書き禁止）。
- テストは **`cargo test --target-dir target-test`**（dev サーバー実行中のロック回避）。
- clippy は **`cargo clippy -- -D warnings`**（`--all-targets` は使わない＝`store.rs:749` の既存 lint で落ちる）。
- 本番コードで `unwrap()` 禁止（`expect()` or エラーハンドリング。`main.rs` 起動時のみ可）。
- コード内コメント・コミットメッセージは英語（Conventional Commits）。CLAUDE.md/.claude/docs は日本語可。
- CSP: inline `onclick` 禁止 → `addEventListener` を使う。inline SVG 要素は可。
- 新規 IIFE グローバルモジュール追加時は `eslint.config.mjs` の globals と varsIgnorePattern に登録。
- UI 変更時は `npx playwright test tests/e2e/filer-ui.e2e.ts` を実行。UI 規約は `frontend/DESIGN.md` が canonical。
- 変更対象ファイルは必ず Read してから編集。
- セッション名バリデーション: 英数字 + `-` のみ、最大 64 文字（`is_valid_session_name` と同一規則）。
- **`56567a6` のキー方針（`clear-defaults` / `prefix None` / `default_shell` 注入）は維持・反転しない。** 変えるのは「バーの表示」だけ。

---

## File Structure

| ファイル | 責務 | 変更種別 |
|---|---|---|
| `frontend/layouts/den-bare.kdl` | zellij bare layout | **削除** |
| `frontend/layouts/den.conf` | tmux config | 変更（`status off` 削除） |
| `src/assets.rs` | `ensure_mux_layouts`（layout 書き出し）＋テスト | 変更（`den-bare.kdl` 書き出し撤去・`status off` テスト反転） |
| `src/pty/backend.rs` | `MuxConfig` / `build_launch_command` / `kill_mux_session` / `delete_mux_session` / `is_valid_mux_name` | 変更（`zellij_layout` 撤去・`-l` 撤去・操作関数追加） |
| `src/store.rs` | `Settings.mux_aliases` ＋ Store エイリアスヘルパ | 変更（フィールド・メソッド追加） |
| `src/multiplexer_api.rs` | `status`（aliases 付与）＋ `kill`/`delete`/`rename` ハンドラ | 変更 |
| `src/lib.rs` | 新ルート 3 本 | 変更 |
| `src/remote.rs` | allowlist テスト追加（本体は変更不要） | 変更（テストのみ） |
| `frontend/index.html` | `#sessions-modal` markup ＋ backend アイコン SVG `<symbol>` | 変更 |
| `frontend/js/terminal.js` | ＋メニュー再設計 ＋ Sessions モーダル ＋ alias 反映 | 変更 |
| `frontend/css/*` | `.new-session-menu-group` / `.new-session-menu-backend` / Sessions モーダル ＋ backend 識別色 | 変更 |
| `frontend/DESIGN.md` | backend 識別色トークン追記 | 変更 |
| `tests/e2e/multiplexer-menu.e2e.ts` | グループヘッダ/backend アイコン assert | 新規 or 変更 |
| `tests/e2e/sessions-modal.e2e.ts` | Sessions モーダル一覧/rename/copy/Kill フロー | 新規 |

---

## Task 1: ① Native 化（バー復活・キー無効維持）

**Files:**
- Delete: `frontend/layouts/den-bare.kdl`
- Modify: `frontend/layouts/den.conf:5`（`set -g status off` 削除）
- Modify: `src/pty/backend.rs`（`MuxConfig.zellij_layout` 撤去・`build_launch_command` の `-l` 撤去・テスト更新）
- Modify: `src/assets.rs`（`ensure_mux_layouts` の `zellij_layout` 書き出し撤去・テスト更新）
- Test: `src/pty/backend.rs`（`#[cfg(test)] mod tests`）/ `src/assets.rs`（`mod mux_layout_tests`）

**Interfaces:**
- Produces:
  - `pub struct MuxConfig { pub zellij_config: String, pub tmux_conf: String }`（`zellij_layout` フィールド削除）
  - `pub fn build_launch_command(backend: SessionBackend, shell: &str, name: &str, mux: &MuxConfig) -> (String, Vec<String>)` — zellij は `zellij --config <cfg> attach -c <name>`（`-l` 無し）、tmux は `tmux -f <conf> new-session -A -s <name>`。

- [ ] **Step 1: backend.rs のテストを新 argv（`-l` 無し）へ更新**

`src/pty/backend.rs` の `mod tests` を以下のように書き換える。`mux()` ヘルパは引数 2 個（zellij_config, tmux_conf）に変更:

```rust
    fn mux(zellij_config: &str, tmux_conf: &str) -> MuxConfig {
        MuxConfig {
            zellij_config: zellij_config.to_string(),
            tmux_conf: tmux_conf.to_string(),
        }
    }

    #[test]
    fn shell_backend_uses_shell_with_no_args() {
        let (prog, args) =
            build_launch_command(SessionBackend::Shell, "powershell.exe", "work", &mux("C.kdl", "t.conf"));
        assert_eq!(prog, "powershell.exe");
        assert!(args.is_empty());
    }

    #[test]
    fn zellij_backend_attach_or_create_with_config_no_layout() {
        // Native 化: -l を付けない（デフォルトレイアウト＝バー有り）。--config は維持。
        let (prog, args) =
            build_launch_command(SessionBackend::Zellij, "powershell.exe", "work", &mux("C.kdl", "t.conf"));
        assert_eq!(prog, "zellij");
        assert_eq!(args, vec!["--config", "C.kdl", "attach", "-c", "work"]);
    }

    #[test]
    fn tmux_backend_attach_or_create_with_conf() {
        let (prog, args) =
            build_launch_command(SessionBackend::Tmux, "powershell.exe", "work", &mux("C.kdl", "t.conf"));
        assert_eq!(prog, "tmux");
        assert_eq!(args, vec!["-f", "t.conf", "new-session", "-A", "-s", "work"]);
    }

    #[test]
    fn zellij_backend_without_config_omits_flag() {
        let (prog, args) =
            build_launch_command(SessionBackend::Zellij, "powershell.exe", "work", &mux("", ""));
        assert_eq!(prog, "zellij");
        assert_eq!(args, vec!["attach", "-c", "work"]);
    }

    #[test]
    fn tmux_backend_without_conf_omits_flag() {
        let (prog, args) =
            build_launch_command(SessionBackend::Tmux, "powershell.exe", "work", &mux("", ""));
        assert_eq!(prog, "tmux");
        assert_eq!(args, vec!["new-session", "-A", "-s", "work"]);
    }

    #[test]
    fn hyphenated_name_is_not_confused_with_flag() {
        let (_, zargs) =
            build_launch_command(SessionBackend::Zellij, "powershell.exe", "-l", &mux("", ""));
        assert_eq!(zargs, vec!["attach", "-c", "-l"]);
        let (_, targs) =
            build_launch_command(SessionBackend::Tmux, "powershell.exe", "-f", &mux("", ""));
        assert_eq!(targs, vec!["new-session", "-A", "-s", "-f"]);
    }
```

既存の `zellij_backend_config_without_layout` テストは概念が消える（layout が無くなる）ため削除する。`backend_default_is_shell` / `parse_*` 系テストは残す。

- [ ] **Step 2: テストを走らせ、コンパイルエラー（`zellij_layout` 参照）で失敗を確認**

Run: `cargo test --target-dir target-test backend:: 2>&1 | head -40`
Expected: コンパイルエラー — `MuxConfig` に `zellij_layout` が無い / `build_launch_command` がまだ `-l` を付ける。

- [ ] **Step 3: `MuxConfig` から `zellij_layout` を削除**

`src/pty/backend.rs:17-25` を:

```rust
/// multiplexer 起動に使う materialized なファイルパス群。
/// `ensure_mux_layouts` が `data_dir` へ書き出した絶対パスを保持する。
/// 各フィールドが空文字列 = 書き出し失敗（`build_launch_command` は該当フラグを省略）。
#[derive(Debug, Clone, Default)]
pub struct MuxConfig {
    /// zellij config（`--config`）: default_shell ＋ keybinds clear-defaults
    pub zellij_config: String,
    /// tmux conf（`-f`）
    pub tmux_conf: String,
}
```

- [ ] **Step 4: `build_launch_command` の zellij 分岐から `-l` 付与を撤去**

`src/pty/backend.rs:50-65` の `SessionBackend::Zellij` 分岐を:

```rust
        SessionBackend::Zellij => {
            let mut args = Vec::new();
            // --config はグローバルオプション。サブコマンド前・long form で渡す。
            // Native 化: -l は付けない（デフォルトレイアウト＝tab/status bar 有り）。
            if !mux.zellij_config.is_empty() {
                args.push("--config".to_string());
                args.push(mux.zellij_config.clone());
            }
            args.push("attach".to_string());
            args.push("-c".to_string());
            args.push(name.to_string());
            ("zellij".to_string(), args)
        }
```

doc コメント（`src/pty/backend.rs:31-41` 付近）の zellij argv 説明から `-l <layout>` への言及を削除し `zellij --config <cfg> attach -c <name>` に直す。

- [ ] **Step 5: backend.rs テストが通ることを確認**

Run: `cargo test --target-dir target-test backend:: 2>&1 | tail -20`
Expected: PASS（全 backend テスト）。

- [ ] **Step 6: `frontend/layouts/den-bare.kdl` を削除**

```bash
git rm frontend/layouts/den-bare.kdl
```

- [ ] **Step 7: `frontend/layouts/den.conf` から `status off` を削除**

`frontend/layouts/den.conf:1-6` を:

```
# Den multiplexer config (tmux). Native bar is shown (status line on).
# window-size latest -> clamp to the most-recently-active client (PC<->iPad).
# See PoC findings doc §6/§7.
set -g window-size latest
```

（`set -g status off` 行を削除。`default-command` / `prefix None` / `unbind C-b` / `window-size latest` は維持）

- [ ] **Step 8: `assets.rs` の `ensure_mux_layouts` から `den-bare.kdl` 書き出しを撤去**

`src/assets.rs:138-148` の戻り値構築を:

```rust
    crate::pty::backend::MuxConfig {
        zellij_config: write_templated(
            data_dir,
            "layouts/den-zellij.kdl",
            "den-zellij.kdl",
            &shell_escaped,
        ),
        tmux_conf: write_templated(data_dir, "layouts/den.conf", "den.conf", &shell_escaped),
    }
```

`write_embedded` ヘルパ（`src/assets.rs:99-101`）は他に呼び出し元が無くなるため削除する（clippy `dead_code` 回避）。

- [ ] **Step 9: `assets.rs` のテストを Native 化に合わせて更新**

`src/assets.rs` の `mux_layout_tests`:
- `ensure_mux_layouts_writes_files`: `mux.zellij_layout` への assert を削除。`conf_body.contains("status off")` を **`assert!(!conf_body.contains("status off"))`** に反転。`assert!(conf_body.contains("set -g window-size latest"))` を追加。`zellij_config`/`tmux_conf` の存在 assert は維持。zellij kdl の `pane` assert（旧 `zellij_layout` 経由）は削除。
- `ensure_mux_layouts_escapes_backslashes_in_shell` / `ensure_mux_layouts_strips_control_chars_from_shell`: `zellij_config` を見るので変更不要。

具体的に `ensure_mux_layouts_writes_files` を:

```rust
    #[test]
    fn ensure_mux_layouts_writes_files() {
        let dir = std::env::temp_dir().join("den-mux-layout-test");
        let _ = std::fs::create_dir_all(&dir);
        let mux = ensure_mux_layouts(&dir, "powershell.exe");
        assert!(std::path::Path::new(&mux.zellij_config).exists());
        assert!(std::path::Path::new(&mux.tmux_conf).exists());

        let conf_body = std::fs::read_to_string(&mux.tmux_conf).expect("conf readable");
        // Native 化: status line は出す（status off を書かない）
        assert!(!conf_body.contains("status off"));
        assert!(conf_body.contains("set -g window-size latest"));
        // tmux: shell 展開 ＋ prefix 解放は維持
        assert!(conf_body.contains("default-command \"powershell.exe\""));
        assert!(conf_body.contains("set -g prefix None"));

        let cfg_body = std::fs::read_to_string(&mux.zellij_config).expect("cfg readable");
        // zellij: default_shell 展開 ＋ keybinds clear-defaults は維持
        assert!(cfg_body.contains("default_shell \"powershell.exe\""));
        assert!(cfg_body.contains("clear-defaults=true"));
        assert!(!cfg_body.contains("__DEN_SHELL__"));

        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 10: assets.rs テストが通ることを確認**

Run: `cargo test --target-dir target-test mux_layout 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 11: registry.rs に `MuxConfig.zellij_layout` 参照が無いか確認**

Run: `cargo build --target-dir target-test 2>&1 | tail -20`
Expected: ビルド成功。もし `zellij_layout` 参照が残れば該当箇所を削除（registry.rs:235 付近のフィールド渡しを確認）。

- [ ] **Step 12: fmt / clippy / 全テスト**

Run: `cargo fmt -- --check && cargo clippy -- -D warnings && cargo test --target-dir target-test 2>&1 | tail -20`
Expected: いずれも成功・全テスト PASS。

- [ ] **Step 13: Commit**

```bash
git add src/pty/backend.rs src/assets.rs frontend/layouts/den.conf
git rm frontend/layouts/den-bare.kdl 2>/dev/null; git add -A frontend/layouts
git commit -m "feat(multiplexer): restore native command bar (drop bare layout / tmux status off)"
```

---

## Task 2: ③ backend 操作関数（kill / delete / name 検証）

**Files:**
- Modify: `src/pty/backend.rs`（`is_valid_mux_name` / `kill_mux_session` / `delete_mux_session` 追加 ＋ テスト）

**Interfaces:**
- Consumes: `SessionBackend`（Task 1）。
- Produces:
  - `pub fn is_valid_mux_name(name: &str) -> bool` — 英数字 + `-`、1–64 文字。
  - `pub fn kill_mux_session(backend: SessionBackend, name: &str) -> Result<(), String>` — blocking。`Ok(())` = kill 成功、`Err(msg)` = 検証失敗 / 実行失敗 / 非ゼロ終了の stderr。
  - `pub fn delete_mux_session(backend: SessionBackend, name: &str) -> Result<(), String>` — zellij `delete-session --force <name>`。tmux は `Err("delete is not supported for tmux".into())`。

- [ ] **Step 1: name 検証＋操作関数の argv テストを書く**

`src/pty/backend.rs` の `mod tests` に追加。CLI 実行は環境依存なので **argv 構築を切り出した純関数** をテストする。まず argv ビルダ `kill_argv` / `delete_argv` を介する設計にする想定でテストを書く:

```rust
    #[test]
    fn is_valid_mux_name_accepts_alnum_hyphen() {
        assert!(is_valid_mux_name("work"));
        assert!(is_valid_mux_name("work-1"));
        assert!(!is_valid_mux_name(""));
        assert!(!is_valid_mux_name("a b"));
        assert!(!is_valid_mux_name("../x"));
        assert!(!is_valid_mux_name(&"x".repeat(65)));
    }

    #[test]
    fn kill_argv_per_backend() {
        assert_eq!(kill_argv(SessionBackend::Zellij, "work"), Some(("zellij".into(), vec!["kill-session".to_string(), "work".to_string()])));
        assert_eq!(kill_argv(SessionBackend::Tmux, "work"), Some(("tmux".into(), vec!["kill-session".to_string(), "-t".to_string(), "work".to_string()])));
        assert_eq!(kill_argv(SessionBackend::Shell, "work"), None);
    }

    #[test]
    fn delete_argv_zellij_force_tmux_none() {
        assert_eq!(delete_argv(SessionBackend::Zellij, "work"), Some(("zellij".into(), vec!["delete-session".to_string(), "--force".to_string(), "work".to_string()])));
        assert_eq!(delete_argv(SessionBackend::Tmux, "work"), None);
        assert_eq!(delete_argv(SessionBackend::Shell, "work"), None);
    }
```

- [ ] **Step 2: テストがコンパイル失敗することを確認**

Run: `cargo test --target-dir target-test backend:: 2>&1 | head -20`
Expected: `is_valid_mux_name` / `kill_argv` / `delete_argv` 未定義でコンパイルエラー。

- [ ] **Step 3: 検証関数と argv ビルダ・実行関数を実装**

`src/pty/backend.rs`（`list_mux_sessions` の後ろ、`#[cfg(test)]` の前）に追加:

```rust
/// mux セッション名バリデーション: 英数字 + `-`、1–64 文字。
/// argv 直渡し（シェル経由でない）だが多層防御として検証する。
pub fn is_valid_mux_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// kill-session の (program, args)。Shell は None。
fn kill_argv(backend: SessionBackend, name: &str) -> Option<(String, Vec<String>)> {
    match backend {
        SessionBackend::Shell => None,
        SessionBackend::Zellij => Some(("zellij".into(), vec!["kill-session".into(), name.into()])),
        SessionBackend::Tmux => {
            Some(("tmux".into(), vec!["kill-session".into(), "-t".into(), name.into()]))
        }
    }
}

/// delete-session の (program, args)。zellij のみ（`--force` で kill+delete）。
/// tmux は delete 概念が無い（kill が兼ねる）→ None。
fn delete_argv(backend: SessionBackend, name: &str) -> Option<(String, Vec<String>)> {
    match backend {
        SessionBackend::Zellij => {
            Some(("zellij".into(), vec!["delete-session".into(), "--force".into(), name.into()]))
        }
        SessionBackend::Shell | SessionBackend::Tmux => None,
    }
}

/// 共通: argv を実行し、非ゼロ終了は stderr を Err で返す（blocking）。
fn run_mux_command(prog: &str, args: &[String]) -> Result<(), String> {
    match Command::new(prog).args(args).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            Err(if stderr.trim().is_empty() {
                format!("{prog} exited with status {}", o.status)
            } else {
                stderr.trim().to_string()
            })
        }
        Err(e) => Err(format!("failed to run {prog}: {e}")),
    }
}

/// 実行中の mux セッションを終了する（blocking → 呼び出し側で spawn_blocking）。
pub fn kill_mux_session(backend: SessionBackend, name: &str) -> Result<(), String> {
    if !is_valid_mux_name(name) {
        return Err("invalid session name".into());
    }
    let (prog, args) = kill_argv(backend, name).ok_or("kill is not supported for this backend")?;
    run_mux_command(&prog, &args)
}

/// 終了済み（resurrect 状態）の zellij セッションを掃除する（blocking）。
pub fn delete_mux_session(backend: SessionBackend, name: &str) -> Result<(), String> {
    if !is_valid_mux_name(name) {
        return Err("invalid session name".into());
    }
    let (prog, args) =
        delete_argv(backend, name).ok_or("delete is not supported for this backend")?;
    run_mux_command(&prog, &args)
}
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test --target-dir target-test backend:: 2>&1 | tail -20`
Expected: PASS（`is_valid_mux_name` / `kill_argv` / `delete_argv` 新規 3 件含む）。

- [ ] **Step 5: clippy**

Run: `cargo clippy -- -D warnings 2>&1 | tail -10`
Expected: warning 無し。

- [ ] **Step 6: Commit**

```bash
git add src/pty/backend.rs
git commit -m "feat(multiplexer): add kill/delete session backend operations"
```

---

## Task 3: ③ エイリアス永続化（Settings.mux_aliases ＋ Store ヘルパ）

**Files:**
- Modify: `src/store.rs`（`Settings.mux_aliases` フィールド ＋ `Default` ＋ Store メソッド ＋ テスト）

**Interfaces:**
- Consumes: 既存 `Store::load_settings` / `save_settings` パターン。
- Produces:
  - `Settings.mux_aliases: Option<HashMap<String, String>>`（キー `"<backend>:<name>"`、値=エイリアス）。
  - `pub fn load_mux_aliases(&self) -> HashMap<String, String>` — None を空 map に正規化して返す。
  - `pub fn set_mux_alias(&self, key: &str, alias: &str) -> std::io::Result<()>` — `alias` 空文字列ならキー削除、非空なら upsert。settings を load → mutate → save。

- [ ] **Step 1: Store エイリアスヘルパのテストを書く**

`src/store.rs` の `#[cfg(test)] mod tests`（既存）にテスト追加。一時ディレクトリで Store を作るパターンは既存テストに倣う（`store.rs` 内の既存 test helper を確認して合わせる）:

```rust
    #[test]
    fn mux_alias_set_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("den-mux-alias-test-1");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(dir.clone()).unwrap();
        assert!(store.load_mux_aliases().is_empty());

        store.set_mux_alias("zellij:work", "My Work").unwrap();
        let aliases = store.load_mux_aliases();
        assert_eq!(aliases.get("zellij:work").map(String::as_str), Some("My Work"));

        // empty alias removes the key
        store.set_mux_alias("zellij:work", "").unwrap();
        assert!(store.load_mux_aliases().get("zellij:work").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: テストがコンパイル失敗することを確認**

Run: `cargo test --target-dir target-test mux_alias 2>&1 | head -20`
Expected: `load_mux_aliases` / `set_mux_alias` 未定義でコンパイルエラー。

- [ ] **Step 3: `Settings` に `mux_aliases` フィールドを追加**

`src/store.rs:226-228`（`default_backend` の後）に追加:

```rust
    /// Default session backend for new local sessions: "shell" | "zellij" | "tmux"
    #[serde(default)]
    pub default_backend: Option<String>,
    /// Den-local aliases for mux sessions. Key = "<backend>:<name>", value = display alias.
    /// Separate from SessionRecord so externally-created sessions can be aliased too.
    #[serde(default)]
    pub mux_aliases: Option<std::collections::HashMap<String, String>>,
```

`impl Default for Settings`（`src/store.rs:268` 付近、`default_backend: None,` の後）に `mux_aliases: None,` を追加。

- [ ] **Step 4: Store メソッドを実装**

`src/store.rs` の `impl Store` 内（`load_settings` 系の近く）に追加。`save_settings` の正確な名前は既存実装を Read して合わせる（settings の保存メソッド名・引数）:

```rust
    /// mux エイリアスマップを返す（None は空 map に正規化）。
    pub fn load_mux_aliases(&self) -> std::collections::HashMap<String, String> {
        self.load_settings().mux_aliases.unwrap_or_default()
    }

    /// mux エイリアスを upsert（alias 空文字列ならキー削除）し永続化する。
    pub fn set_mux_alias(&self, key: &str, alias: &str) -> std::io::Result<()> {
        let mut settings = self.load_settings();
        let mut map = settings.mux_aliases.take().unwrap_or_default();
        if alias.is_empty() {
            map.remove(key);
        } else {
            map.insert(key.to_string(), alias.to_string());
        }
        settings.mux_aliases = if map.is_empty() { None } else { Some(map) };
        self.save_settings(&settings)
    }
```

> 注: `save_settings` の実シグネチャ（`&self, settings: &Settings) -> io::Result<()>` か `Settings` 値渡しか）は `src/store.rs` を Read して合わせる。キャッシュ無効化が `save_settings` 内で行われているかも確認し、必要なら `set_mux_alias` 後にキャッシュ更新する。

- [ ] **Step 5: テストが通ることを確認**

Run: `cargo test --target-dir target-test mux_alias 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 6: 既存 settings テストの回帰確認**

Run: `cargo test --target-dir target-test store:: 2>&1 | tail -20`
Expected: 既存 store テスト全 PASS（`mux_aliases` は `#[serde(default)]` なので旧 JSON もデシリアライズ可）。

- [ ] **Step 7: Commit**

```bash
git add src/store.rs
git commit -m "feat(multiplexer): persist Den-local session aliases in settings"
```

---

## Task 4: ③ API ハンドラ（kill / delete / rename）＋ status に aliases ＋ ルート

**Files:**
- Modify: `src/multiplexer_api.rs`（`status` を State 付き化＋aliases 付与・`kill`/`delete`/`rename` ハンドラ追加・テスト）
- Modify: `src/lib.rs:150`（ルート 3 本追加）
- Modify: `src/remote.rs`（allowlist テスト追加。本体は変更不要）

**Interfaces:**
- Consumes: `kill_mux_session` / `delete_mux_session` / `is_valid_mux_name`（Task 2）、`Store::load_mux_aliases` / `set_mux_alias`（Task 3）、`SessionBackend`。
- Produces:
  - `GET /api/multiplexer/status` → `MultiplexerStatus { zellij: BackendStatus, tmux: BackendStatus }`、`BackendStatus { available: bool, sessions: Vec<String>, aliases: HashMap<String, String> }`（aliases は当該 backend の name→alias）。
  - `POST /api/multiplexer/kill` body `{ backend: String, name: String }` → `{ ok: bool, message: Option<String> }`。
  - `POST /api/multiplexer/delete` 同上。
  - `POST /api/multiplexer/rename` body `{ backend: String, name: String, alias: String }` → `{ ok: bool, message: Option<String> }`（mux CLI は叩かず Store エイリアスのみ更新）。

- [ ] **Step 1: API のリクエスト/レスポンス型とハンドラのユニットテストを書く**

`src/multiplexer_api.rs` の `mod tests` に追加（payload シリアライズ＋backend 文字列パース）:

```rust
    #[test]
    fn parse_backend_str_maps_known_values() {
        assert_eq!(parse_backend("zellij"), Some(SessionBackend::Zellij));
        assert_eq!(parse_backend("tmux"), Some(SessionBackend::Tmux));
        assert_eq!(parse_backend("shell"), None); // shell は kill/delete 不可
        assert_eq!(parse_backend("bogus"), None);
    }

    #[test]
    fn op_result_serializes_ok_and_error() {
        let ok = OpResult { ok: true, message: None };
        assert!(serde_json::to_string(&ok).unwrap().contains("\"ok\":true"));
        let err = OpResult { ok: false, message: Some("boom".into()) };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("\"ok\":false"));
        assert!(s.contains("boom"));
    }

    #[test]
    fn status_backend_status_includes_aliases() {
        let bs = BackendStatus {
            available: true,
            sessions: vec!["work".into()],
            aliases: std::collections::HashMap::from([("work".to_string(), "My Work".to_string())]),
        };
        let json = serde_json::to_string(&bs).unwrap();
        assert!(json.contains("\"aliases\""));
        assert!(json.contains("My Work"));
    }
```

- [ ] **Step 2: コンパイル失敗を確認**

Run: `cargo test --target-dir target-test multiplexer_api 2>&1 | head -20`
Expected: `parse_backend` / `OpResult` / `BackendStatus.aliases` 未定義でエラー。

- [ ] **Step 3: 型・ヘルパ・ハンドラを実装**

`src/multiplexer_api.rs` を全面的に拡張。先頭の use とAppState 取得を追加（`store_api` の `State<Arc<AppState>>` パターンに合わせる。`AppState` のパスは `crate::AppState` を確認）:

```rust
use crate::pty::backend::{
    SessionBackend, delete_mux_session, is_valid_mux_name, kill_mux_session, list_mux_sessions,
    probe_available,
};
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

#[derive(Serialize)]
pub struct BackendStatus {
    pub available: bool,
    pub sessions: Vec<String>,
    /// name -> Den-local alias（このバックエンド分のみ）
    pub aliases: HashMap<String, String>,
}

#[derive(Serialize)]
pub struct MultiplexerStatus {
    pub zellij: BackendStatus,
    pub tmux: BackendStatus,
}

#[derive(Deserialize)]
pub struct SessionOp {
    pub backend: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct RenameOp {
    pub backend: String,
    pub name: String,
    pub alias: String,
}

#[derive(Serialize)]
pub struct OpResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// "zellij"/"tmux" のみ受理（shell は kill/delete 対象外）。
fn parse_backend(s: &str) -> Option<SessionBackend> {
    match s {
        "zellij" => Some(SessionBackend::Zellij),
        "tmux" => Some(SessionBackend::Tmux),
        _ => None,
    }
}

/// settings の mux_aliases から指定 backend 分（プレフィックス除去済み name→alias）を取り出す。
fn aliases_for(all: &HashMap<String, String>, backend: &str) -> HashMap<String, String> {
    let prefix = format!("{backend}:");
    all.iter()
        .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|n| (n.to_string(), v.clone())))
        .collect()
}
```

`availability()` は既存のまま。`status` を State 付きに:

```rust
/// GET /api/multiplexer/status
pub async fn status(State(state): State<Arc<crate::AppState>>) -> Json<MultiplexerStatus> {
    let (zellij_ok, tmux_ok) = *availability();
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
    let store = state.store.clone();
    let all_aliases = tokio::task::spawn_blocking(move || store.load_mux_aliases())
        .await
        .unwrap_or_default();
    Json(MultiplexerStatus {
        zellij: BackendStatus {
            available: zellij_ok,
            sessions: zellij_sessions,
            aliases: aliases_for(&all_aliases, "zellij"),
        },
        tmux: BackendStatus {
            available: tmux_ok,
            sessions: tmux_sessions,
            aliases: aliases_for(&all_aliases, "tmux"),
        },
    })
}
```

kill/delete/rename ハンドラ:

```rust
/// POST /api/multiplexer/kill
pub async fn kill(
    State(_state): State<Arc<crate::AppState>>,
    Json(op): Json<SessionOp>,
) -> Json<OpResult> {
    Json(run_session_op(&op.backend, &op.name, kill_mux_session).await)
}

/// POST /api/multiplexer/delete
pub async fn delete(
    State(_state): State<Arc<crate::AppState>>,
    Json(op): Json<SessionOp>,
) -> Json<OpResult> {
    Json(run_session_op(&op.backend, &op.name, delete_mux_session).await)
}

/// kill/delete 共通の検証＋spawn_blocking 実行。
async fn run_session_op(
    backend: &str,
    name: &str,
    op: fn(SessionBackend, &str) -> Result<(), String>,
) -> OpResult {
    let Some(be) = parse_backend(backend) else {
        return OpResult { ok: false, message: Some("unknown backend".into()) };
    };
    if !is_valid_mux_name(name) {
        return OpResult { ok: false, message: Some("invalid session name".into()) };
    }
    let name = name.to_string();
    match tokio::task::spawn_blocking(move || op(be, &name)).await {
        Ok(Ok(())) => OpResult { ok: true, message: None },
        Ok(Err(msg)) => OpResult { ok: false, message: Some(msg) },
        Err(e) => OpResult { ok: false, message: Some(format!("task panicked: {e}")) },
    }
}

/// POST /api/multiplexer/rename — Den ローカルエイリアスのみ更新（mux CLI は叩かない）。
pub async fn rename(
    State(state): State<Arc<crate::AppState>>,
    Json(op): Json<RenameOp>,
) -> Json<OpResult> {
    if parse_backend(&op.backend).is_none() {
        return Json(OpResult { ok: false, message: Some("unknown backend".into()) });
    }
    if !is_valid_mux_name(&op.name) {
        return Json(OpResult { ok: false, message: Some("invalid session name".into()) });
    }
    let key = format!("{}:{}", op.backend, op.name);
    let alias = op.alias.clone();
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.set_mux_alias(&key, &alias)).await {
        Ok(Ok(())) => Json(OpResult { ok: true, message: None }),
        Ok(Err(e)) => Json(OpResult { ok: false, message: Some(e.to_string()) }),
        Err(e) => Json(OpResult { ok: false, message: Some(format!("task panicked: {e}")) }),
    }
}
```

> 注: `crate::AppState` のフィールド名（`store`）と公開パスは `store_api.rs` / `lib.rs` を Read して合わせる。`AppState` が `pub` か、`store` フィールドが `pub` かを確認。

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test --target-dir target-test multiplexer_api 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 5: ルートを `lib.rs` に追加**

`src/lib.rs:150` の status ルートの直後に追加:

```rust
        // Multiplexer (tmux/zellij) availability + session list
        .route("/api/multiplexer/status", get(multiplexer_api::status))
        .route("/api/multiplexer/kill", post(multiplexer_api::kill))
        .route("/api/multiplexer/delete", post(multiplexer_api::delete))
        .route("/api/multiplexer/rename", post(multiplexer_api::rename))
```

`post` が import 済みか確認（既に `.post(...)` を多用しているので import 済みのはず）。

- [ ] **Step 6: remote allowlist テストを追加**

`src/remote.rs:939` の `allowlist_permits_multiplexer` テストに行を追加（本体 `path_in_allowlist` は `multiplexer/` プレフィックスで既に全て許可済み＝変更不要）:

```rust
        assert!(path_in_allowlist("multiplexer/kill"));
        assert!(path_in_allowlist("multiplexer/delete"));
        assert!(path_in_allowlist("multiplexer/rename"));
```

- [ ] **Step 7: ビルド＋全テスト＋clippy**

Run: `cargo build --target-dir target-test 2>&1 | tail -10 && cargo test --target-dir target-test 2>&1 | tail -20 && cargo clippy -- -D warnings 2>&1 | tail -10`
Expected: ビルド成功・全テスト PASS・warning 無し。

- [ ] **Step 8: Commit**

```bash
git add src/multiplexer_api.rs src/lib.rs src/remote.rs
git commit -m "feat(multiplexer): add kill/delete/rename endpoints and aliases in status"
```

---

## Task 5: ② ＋メニュー再設計（マシン単位グループ化＋backend アイコン）

**Files:**
- Modify: `frontend/index.html`（backend アイコン用 inline SVG `<symbol>` を `<svg>` sprite に追加）
- Modify: `frontend/js/terminal.js`（`buildNewSessionMenu` / `buildBackendSubmenu` をグループ構造へ）
- Modify: `frontend/css/`（`.new-session-menu-group` / `.new-session-menu-backend` ＋ backend 識別色）
- Modify: `frontend/DESIGN.md`（backend 識別色トークン追記）

**Interfaces:**
- Consumes: `fetchMuxStatus(connId)`（既存）が返す `{ zellij:{available,sessions,aliases}, tmux:{...} }`、`createSession(name, ssh, connId, backend)`（既存）、`DenSettings.get('default_backend')`。
- Produces: グループ化された `#new-session-menu` DOM。E2E が当てるセレクタ: `.new-session-menu-group`（グループヘッダ）、`.new-session-menu-backend`（backend 行）、`.backend-icon[data-backend="zellij|tmux|shell"]`。

- [ ] **Step 1: backend アイコン sprite を `index.html` に追加**

`frontend/index.html` の既存 SVG sprite（`<svg ... style="display:none">` の `<symbol>` 群）を探し、無ければ `<body>` 直下に追加。3 アイコンを定義（CSP 上 inline `<symbol>` は可。`onclick` は使わない）:

```html
<svg xmlns="http://www.w3.org/2000/svg" style="display:none" aria-hidden="true">
  <symbol id="ic-backend-shell" viewBox="0 0 16 16"><path d="M2 3l4 5-4 5M8 13h6" fill="none" stroke="currentColor" stroke-width="1.5"/></symbol>
  <symbol id="ic-backend-zellij" viewBox="0 0 16 16"><path d="M8 1l6 3.5v7L8 15 2 11.5v-7L8 1z" fill="none" stroke="currentColor" stroke-width="1.5"/></symbol>
  <symbol id="ic-backend-tmux" viewBox="0 0 16 16"><rect x="2" y="3" width="12" height="10" rx="1" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M2 7h12" stroke="currentColor" stroke-width="1.5"/></symbol>
</svg>
```

既存の sprite が別 ID 命名規則なら合わせる（`frontend/index.html` を Read して確認）。

- [ ] **Step 2: `buildBackendSubmenu` をグループ内 backend 行構造へ書き換え**

`frontend/js/terminal.js:1531-1577` を、backend ごとに「アイコン＋ラベル＋既存セッションチップ＋New」を 1 つの `.new-session-menu-backend` 行に出す形へ。alias 表示も反映:

```js
  /**
   * Append backend (zellij/tmux) rows under the current machine group.
   * status = { zellij:{available,sessions,aliases}, tmux:{available,sessions,aliases} }
   */
  function buildBackendSubmenu(menu, status, remoteConnId, closeMenu) {
    if (!status) return;
    for (const kind of ['zellij', 'tmux']) {
      const bs = status[kind];
      if (!bs || !bs.available) continue;

      const row = document.createElement('div');
      row.className = 'new-session-menu-backend';

      const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      icon.setAttribute('class', 'backend-icon');
      icon.dataset.backend = kind;
      const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
      use.setAttribute('href', `#ic-backend-${kind}`);
      icon.appendChild(use);
      row.appendChild(icon);

      const label = document.createElement('span');
      label.className = 'new-session-menu-backend-label';
      label.textContent = kind;
      row.appendChild(label);

      const chips = document.createElement('span');
      chips.className = 'new-session-menu-chips';
      const aliases = bs.aliases || {};
      for (const name of bs.sessions) {
        const chip = document.createElement('button');
        chip.type = 'button';
        chip.className = 'new-session-menu-chip';
        chip.textContent = aliases[name] || name;
        chip.title = aliases[name] ? `${aliases[name]} (${name})` : name;
        chip.addEventListener('click', async () => {
          closeMenu();
          const res = await createSession(name, null, remoteConnId, kind);
          if (!res.ok) { Toast.error(res.message || 'Failed to attach session'); return; }
          lastSessionsKey = '';
          await refreshSessionList();
          switchSession(name, remoteConnId || undefined);
        });
        chips.appendChild(chip);
      }
      // New (+) chip
      const plus = document.createElement('button');
      plus.type = 'button';
      plus.className = 'new-session-menu-chip new-session-menu-chip-new';
      plus.textContent = '+';
      plus.title = `New ${kind} session`;
      plus.addEventListener('click', async () => {
        closeMenu();
        const name = await Toast.prompt('Session name:');
        if (!name || !name.trim()) return;
        const trimmed = name.trim();
        const validationError = validateSessionName(trimmed);
        if (validationError) { Toast.error(validationError); return; }
        const res = await createSession(trimmed, null, remoteConnId, kind);
        if (!res.ok) { Toast.error(res.message || 'Failed to create session'); return; }
        lastSessionsKey = '';
        await refreshSessionList();
        switchSession(trimmed, remoteConnId || undefined);
      });
      chips.appendChild(plus);
      row.appendChild(chips);

      menu.appendChild(row);
    }
  }
```

- [ ] **Step 3: `buildNewSessionMenu` をマシン単位グループへ再構成**

`frontend/js/terminal.js:1595-1716` の本体を、`new-session-menu-separator`（フラット）を `new-session-menu-group`（ヘッダ）に置き換える。グループ＝「This Den (local)」「Remote: <name>」「SSH」、最下部に「Quick Connect Den…」。ヘルパを 1 つ足す:

```js
  function appendGroupHeader(menu, iconId, text) {
    const h = document.createElement('div');
    h.className = 'new-session-menu-group';
    const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    icon.setAttribute('class', 'group-icon');
    const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
    use.setAttribute('href', `#${iconId}`);
    icon.appendChild(use);
    h.appendChild(icon);
    const span = document.createElement('span');
    span.textContent = text;
    h.appendChild(span);
    menu.appendChild(h);
    return h;
  }
```

そして:
- ローカル: `appendGroupHeader(menu, 'ic-machine-local', 'This Den (local)')` → `localItem`（Local Terminal、既存ロジック維持）→ `buildBackendSubmenu(menu, localStatus, null, ...)`。
- 各リモート: ループ内 `appendGroupHeader(menu, 'ic-machine-remote', 'Remote: ' + (info.displayName || stripPort(info.hostPort) || connId))` → New Terminal 行 → `buildBackendSubmenu(menu, remoteStatusById[connId], connId, ...)`。既存 separator 生成（`new-session-menu-separator`）は削除。
- SSH: bookmarks があれば `appendGroupHeader(menu, 'ic-machine-ssh', 'SSH')` → 既存 bookmark 行。
- Quick Connect は最下部に常設（既存 `quickItem` をグループ群の後・SSH の後ろに移動。現状は SSH より前なので順序を「local → remotes → SSH → Quick Connect」に並べ替え）。

`ic-machine-local`/`ic-machine-remote`/`ic-machine-ssh` の `<symbol>` も Step 1 の sprite に追加（🖥/🌐/🔑 相当の簡易パス。無ければ絵文字テキストでも可だが DOM セレクタ統一のため SVG 推奨）。

- [ ] **Step 4: CSS を追加**

`frontend/css/`（メニュー関連 CSS のあるファイルを Grep `new-session-menu` で特定）に追加。`frontend/DESIGN.md` のトークンを使う:

```css
.new-session-menu-group {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 12px 4px;
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--text-muted, #888);
}
.new-session-menu-group .group-icon { width: 14px; height: 14px; }
.new-session-menu-backend {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 12px 4px 20px;
}
.new-session-menu-backend .backend-icon { width: 14px; height: 14px; flex: 0 0 auto; }
.new-session-menu-backend .backend-icon[data-backend="zellij"] { color: var(--backend-zellij, #7aa2f7); }
.new-session-menu-backend .backend-icon[data-backend="tmux"] { color: var(--backend-tmux, #9ece6a); }
.new-session-menu-backend .backend-icon[data-backend="shell"] { color: var(--backend-shell, #888); }
.new-session-menu-backend-label { font-size: 12px; color: var(--text-muted, #888); min-width: 46px; }
.new-session-menu-chips { display: flex; flex-wrap: wrap; gap: 4px; }
.new-session-menu-chip {
  font: inherit; font-size: 12px;
  padding: 2px 8px; border-radius: 10px;
  border: 1px solid var(--border, #333);
  background: var(--surface-2, #1a1a1a); color: var(--text, #ddd);
  cursor: pointer;
}
.new-session-menu-chip:hover { background: var(--surface-hover, #2a2a2a); }
.new-session-menu-chip-new { font-weight: 700; }
```

> 実際のトークン変数名は `frontend/DESIGN.md` / 既存 CSS を Read して合わせる（`--text-muted` 等が無ければ既存の近いトークンに置換）。

- [ ] **Step 5: `frontend/DESIGN.md` に backend 識別色を追記**

`frontend/DESIGN.md` のトークン節に backend 識別色 3 つ（`--backend-zellij` / `--backend-tmux` / `--backend-shell`）と用途（＋メニュー / Sessions モーダルの backend アイコン）を 1 ブロック追記。

- [ ] **Step 6: ESLint**

Run: `npx eslint frontend/js/terminal.js 2>&1 | tail -20`
Expected: 0 errors（新規 helper の未使用変数等なし）。

- [ ] **Step 7: 手動スモーク（dev サーバー）**

Run（background）: `$env:DEN_PASSWORD="test"; $env:DEN_DATA_DIR="./data-dev"; cargo run`
ブラウザで ＋ボタン → メニューに「This Den (local)」グループヘッダ、zellij/tmux available なら backend 行＋アイコン、Quick Connect が最下部に出ることを目視。終了時にサーバー停止。

- [ ] **Step 8: Commit**

```bash
git add frontend/index.html frontend/js/terminal.js frontend/css frontend/DESIGN.md
git commit -m "feat(multiplexer): redesign new-session menu with machine groups and backend icons"
```

---

## Task 6: ③ フロント Sessions モーダル

**Files:**
- Modify: `frontend/index.html`（`#sessions-modal` markup）
- Modify: `frontend/js/terminal.js`（モーダル開閉・描画・rename/copy/Kill/Delete 配線・入口エントリ）
- Modify: `frontend/css/`（Sessions モーダルのスタイル）

**Interfaces:**
- Consumes: `fetchMuxStatus(connId)`（aliases 付き）、`FilerRemote.getDenConnections()`、新 API `POST /api/multiplexer/{kill,delete,rename}`（local は `/api`、remote は `/api/remote/<connId>`）、`Toast.confirm` / `Toast.error`、`navigator.clipboard`。
- Produces: `#sessions-modal`（`allModals` 登録）。E2E セレクタ: `#sessions-modal`、`.sessions-row`、`.sessions-row-name`、`[data-action="rename"|"copy"|"kill"|"delete"]`。

- [ ] **Step 1: `#sessions-modal` の markup を追加**

`frontend/index.html` の既存モーダル群（`confirm-modal` 等の近く）に追加。`hidden` 属性で初期非表示、構造は他モーダルに倣う:

```html
<div id="sessions-modal" class="modal" hidden>
  <div class="modal-content sessions-modal-content">
    <div class="modal-header"><h2>Sessions</h2></div>
    <div id="sessions-modal-body" class="sessions-modal-body"><!-- rows injected --></div>
    <div class="modal-footer">
      <button type="button" id="sessions-modal-close" class="btn">Close</button>
    </div>
  </div>
</div>
```

- [ ] **Step 2: モーダルの開閉と描画を `terminal.js` に実装**

`frontend/js/terminal.js` に関数を追加（IIFE 内）。`allModals` への登録は既存パターンに合わせる（`confirm-modal`/`prompt-modal` の登録箇所を Grep `allModals` で確認。**`escModals` には入れない**＝Promise 未解決防止の既存方針）:

```js
  async function openSessionsModal() {
    const modal = document.getElementById('sessions-modal');
    const body = document.getElementById('sessions-modal-body');
    body.innerHTML = '';
    modal.hidden = false;
    await renderSessionsModal(body);
  }

  function muxApiBase(connId) {
    return connId ? `/api/remote/${connId}` : '/api';
  }

  async function renderSessionsModal(body) {
    body.innerHTML = '<div class="sessions-loading">Loading…</div>';
    const denConns = typeof FilerRemote !== 'undefined' ? FilerRemote.getDenConnections() : {};
    const connIds = Object.keys(denConns);
    const [localStatus, ...remoteStatuses] = await Promise.all([
      fetchMuxStatus(null),
      ...connIds.map(id => fetchMuxStatus(id)),
    ]);
    body.innerHTML = '';
    renderSessionsGroup(body, 'This Den (local)', localStatus, null);
    connIds.forEach((id, i) => {
      const info = denConns[id];
      const title = 'Remote: ' + (info.displayName || stripPort(info.hostPort) || id);
      renderSessionsGroup(body, title, remoteStatuses[i], id);
    });
    if (!body.children.length) {
      body.innerHTML = '<div class="sessions-empty">No multiplexer sessions.</div>';
    }
  }

  function renderSessionsGroup(body, title, status, connId) {
    if (!status) return;
    let any = false;
    const header = document.createElement('div');
    header.className = 'sessions-group-header';
    header.textContent = title;
    body.appendChild(header);
    for (const kind of ['zellij', 'tmux']) {
      const bs = status[kind];
      if (!bs || !bs.available || !bs.sessions.length) continue;
      any = true;
      const sub = document.createElement('div');
      sub.className = 'sessions-backend-header';
      sub.textContent = kind;
      body.appendChild(sub);
      const aliases = bs.aliases || {};
      for (const name of bs.sessions) {
        body.appendChild(buildSessionRow(kind, name, aliases[name], connId));
      }
    }
    if (!any) header.remove();
  }

  function buildSessionRow(kind, name, alias, connId) {
    const row = document.createElement('div');
    row.className = 'sessions-row';
    row.dataset.backend = kind;
    row.dataset.name = name;

    const nameEl = document.createElement('span');
    nameEl.className = 'sessions-row-name';
    nameEl.textContent = alias ? `${alias} (${name})` : name;
    row.appendChild(nameEl);

    const actions = document.createElement('span');
    actions.className = 'sessions-row-actions';

    const mk = (action, text) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'btn-sm';
      b.dataset.action = action;
      b.textContent = text;
      actions.appendChild(b);
      return b;
    };

    mk('rename', 'rename').addEventListener('click', async () => {
      const next = await Toast.prompt('Alias (empty to clear):', alias || '');
      if (next === null) return;
      const res = await muxOp(connId, 'rename', { backend: kind, name, alias: next.trim() });
      if (!res.ok) { Toast.error(res.message || 'Rename failed'); return; }
      await renderSessionsModal(document.getElementById('sessions-modal-body'));
    });

    mk('copy', 'copy attach').addEventListener('click', async () => {
      const cmd = kind === 'zellij' ? `zellij attach ${name}` : `tmux attach -t ${name}`;
      try { await navigator.clipboard.writeText(cmd); Toast.show('Copied'); }
      catch (_) { Toast.error('Copy failed'); }
    });

    mk('kill', 'Kill').addEventListener('click', async () => {
      const ok = await Toast.confirm(`Kill session "${name}"?`);
      if (!ok) return;
      const res = await muxOp(connId, 'kill', { backend: kind, name });
      if (!res.ok) { Toast.error(res.message || 'Kill failed'); return; }
      await renderSessionsModal(document.getElementById('sessions-modal-body'));
    });

    if (kind === 'zellij') {
      mk('delete', 'Delete').addEventListener('click', async () => {
        const ok = await Toast.confirm(`Delete (purge) session "${name}"?`);
        if (!ok) return;
        const res = await muxOp(connId, 'delete', { backend: kind, name });
        if (!res.ok) { Toast.error(res.message || 'Delete failed'); return; }
        await renderSessionsModal(document.getElementById('sessions-modal-body'));
      });
    }

    row.appendChild(actions);
    return row;
  }

  async function muxOp(connId, op, payload) {
    try {
      const resp = await fetch(`${muxApiBase(connId)}/multiplexer/${op}`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
      if (!resp.ok) return { ok: false, message: `HTTP ${resp.status}` };
      return await resp.json();
    } catch (e) {
      return { ok: false, message: String(e) };
    }
  }
```

> `Toast.prompt` の第 2 引数（初期値）・`Toast.show` / `Toast.confirm` の実シグネチャは既存実装を Read して合わせる（`buildBackendSubmenu` で `Toast.prompt('Session name:')` を使っているので prompt は存在。confirm/show は要確認。無ければ `confirm-modal` を直接使う）。

- [ ] **Step 3: 入口エントリと Close 配線**

設定メニュー（or ＋メニュー脇）に「Manage sessions」エントリを追加して `openSessionsModal()` を呼ぶ。`sessions-modal-close` と背景クリックで `modal.hidden = true`。`allModals` への登録は既存の集中管理に合わせる（Grep `allModals` で配列定義を見つけ `'sessions-modal'` を追加。`escModals` には入れない）。入口の具体位置は既存の設定メニュー構造を Read して決定（例: ＋ボタン横のケバブ or 設定モーダル内のリンク）。

- [ ] **Step 4: CSS を追加**

`frontend/css/`（モーダル CSS のファイル）に Sessions モーダルのスタイルを追加（行・アクションボタンの並び、group/backend ヘッダ）。トークンは DESIGN.md に従う。`btn-sm` が既存に無ければ定義。

- [ ] **Step 5: ESLint**

Run: `npx eslint frontend/js/terminal.js 2>&1 | tail -20`
Expected: 0 errors。

- [ ] **Step 6: 手動スモーク**

dev サーバーを起動し、入口から Sessions モーダルを開く → 一覧表示・rename 入力・copy（クリップボード）・Kill 確認フローを目視。終了時にサーバー停止。

- [ ] **Step 7: Commit**

```bash
git add frontend/index.html frontend/js/terminal.js frontend/css
git commit -m "feat(multiplexer): add Sessions management modal (list/rename/copy/kill/delete)"
```

---

## Task 7: E2E テスト（＋メニューグループ ＋ Sessions モーダル）

**Files:**
- Create/Modify: `tests/e2e/multiplexer-menu.e2e.ts`
- Create: `tests/e2e/sessions-modal.e2e.ts`

**Interfaces:**
- Consumes: Task 5/6 のセレクタ（`.new-session-menu-group` / `.new-session-menu-backend` / `.backend-icon[data-backend]` / `#sessions-modal` / `.sessions-row` / `[data-action]`）、`page.route` で `**/api/multiplexer/status` 等をモック。

- [ ] **Step 1: 既存 e2e のセットアップ（ログイン・ルートモック）を確認**

`tests/e2e/multiplexer-menu.e2e.ts` が既にあるか確認（メモリでは Task 13 として計画されていた）。あれば既存パターン、無ければ `tests/e2e/filer-ui.e2e.ts` の beforeEach（ログイン・baseURL）を流用。`page.route('**/api/multiplexer/status', ...)` で zellij available + sessions + aliases を返すモックを定義。

- [ ] **Step 2: ＋メニューのグループ/アイコン assert を書く**

```ts
test('new-session menu groups by machine and shows backend icons', async ({ page }) => {
  await page.route('**/api/multiplexer/status', (route) =>
    route.fulfill({ json: {
      zellij: { available: true, sessions: ['work'], aliases: { work: 'My Work' } },
      tmux: { available: false, sessions: [], aliases: {} },
    }}));
  // open the new-session menu (＋ button selector — match existing test helper)
  await page.click('#new-session-btn'); // 実セレクタは既存に合わせる
  await expect(page.locator('.new-session-menu-group').first()).toContainText('This Den');
  await expect(page.locator('.new-session-menu-backend .backend-icon[data-backend="zellij"]')).toBeVisible();
  await expect(page.locator('.new-session-menu-chip', { hasText: 'My Work' })).toBeVisible();
});
```

- [ ] **Step 3: Sessions モーダルの一覧/rename/copy/Kill フローを書く**

`tests/e2e/sessions-modal.e2e.ts`:

```ts
test('sessions modal lists, renames and kills sessions', async ({ page }) => {
  await page.route('**/api/multiplexer/status', (route) =>
    route.fulfill({ json: {
      zellij: { available: true, sessions: ['work'], aliases: {} },
      tmux: { available: false, sessions: [], aliases: {} },
    }}));
  let renameBody: any = null;
  await page.route('**/api/multiplexer/rename', async (route) => {
    renameBody = route.request().postDataJSON();
    await route.fulfill({ json: { ok: true } });
  });
  await page.route('**/api/multiplexer/kill', (route) => route.fulfill({ json: { ok: true } }));

  // open modal via entry point (match implemented selector)
  await page.click('#manage-sessions-btn');
  await expect(page.locator('#sessions-modal')).toBeVisible();
  await expect(page.locator('.sessions-row[data-name="work"]')).toBeVisible();

  // rename → prompt は実装の Toast.prompt をモックするか、prompt UI を操作
  // (実装の prompt が <input> ベースなら入力して確定する手順に置換)
});
```

> prompt/confirm が `window.prompt`/`window.confirm` でなくカスタム UI（Toast / `prompt-modal`）の場合、e2e はその UI 要素を操作する。実装（Task 6）で使った UI に合わせて手順を確定。

- [ ] **Step 4: e2e を実行**

Run（background 推奨）: `npx playwright test tests/e2e/multiplexer-menu.e2e.ts tests/e2e/sessions-modal.e2e.ts`
Expected: 全 PASS。サーバー起動が必要なら既存 e2e の webServer 設定に従う。

- [ ] **Step 5: filer-ui 回帰（CSS/hidden 影響確認）**

Run（background）: `npx playwright test tests/e2e/filer-ui.e2e.ts`
Expected: 全 PASS（新モーダル markup が既存レイアウトを壊していない）。

- [ ] **Step 6: Commit**

```bash
git add tests/e2e/multiplexer-menu.e2e.ts tests/e2e/sessions-modal.e2e.ts
git commit -m "test(e2e): cover new-session menu groups and Sessions modal"
```

---

## Task 8: ④ tmux 下部欠けバグ（別トラック・remote nix）

> このタスクは ①②③ のリリースをブロックしない。tmux は Windows 非対応のため **remote nix / WSL** で調査する。systematic-debugging スキルで進める。ローカルタスク #31 と対応。
>
> **ユーザー追加観測（2026-06-20）**: 下部欠けは **PC で顕著・iPad では目立たない**が、**iPad でも欠けている可能性あり**。少なくとも **iPad の方が PC より 1 行多く表示できている**。
> → 症状は「クライアント viewport ごとに描画行数が異なり、PC が 1 行少ない」。spec §④ 仮説 1（SIGWINCH/resize 同期ズレ＝Den xterm 行数 vs tmux window 行数の不一致）と整合。PC と iPad で `term.rows` の算出（行高・DPR・キーボード有無）が異なり、tmux の window-size 勘定とズレている線が濃厚。調査時は **PC と iPad の両方で `term.rows` と `tmux display -p '#{window_height}'` を突き合わせ、1 行差の出どころ（status 行勘定 or fit 計算の floor 差）を特定**する。

**Files:**
- 調査結果次第（候補: `src/pty/manager.rs` の resize 経路、`frontend/layouts/den.conf` の `window-size`、`frontend/js/terminal.js` の cols/rows 送出）

- [ ] **Step 1: systematic-debugging で再現環境を用意**

remote nix Den（v3.5.1+）に Den からアタッチし tmux セッションを開く。下部 N 行が描画されない症状を再現・録画。`tmux display -p '#{window_height}'` と Den 側 xterm `term.rows` を突き合わせ、差分を測る。

- [ ] **Step 2: 仮説を 1 つずつ検証**

spec §④ の仮説（1: SIGWINCH/resize 同期ズレ / 2: `-A` attach 時の `-f` 無視で window-size 他クライアント基準 / 3: Native status 行の行数勘定ズレ）を、resize イベントログ・`window-size` 設定変更で切り分け。**1 度に 1 つだけ変える。**

- [ ] **Step 3: 最小修正 → 実機確認 → Commit**

真因確定後に最小修正。remote nix で下部が出ることを確認してコミット。`docs/superpowers/specs/` の spec か `memory/patterns.md` に真因と修正を記録。

> 真因が実環境調査前のため、このタスクの Step 2-3 は調査結果で具体化する（プレースホルダではなく「調査タスク」として意図的に開放）。

---

## Task 9: 全ゲート・renderer スモーク・レビュー・実機（①②③ 出荷）

**Files:** なし（検証のみ）

- [ ] **Step 1: 品質ゲート一括**

Run: `cargo fmt -- --check && cargo clippy -- -D warnings && cargo test --target-dir target-test 2>&1 | tail -20`
Expected: 全成功。

- [ ] **Step 2: ESLint ＋ UI e2e**

Run: `npx eslint frontend/ 2>&1 | tail -10`
Run（background）: `npx playwright test tests/e2e/filer-ui.e2e.ts tests/e2e/multiplexer-menu.e2e.ts tests/e2e/sessions-modal.e2e.ts`
Expected: ESLint 0 err・e2e 全 PASS。

- [ ] **Step 3: renderer 切替スモーク（restty / wterm）**

Native 化で起動引数が変わるため、`.claude/rules/workflow.md` に従い chrome-cdp で restty / wterm に切替え、初期描画遅延なし・CJK・theme 反映・入力エコーを確認（手順は `memory/patterns.md`「chrome-cdp で renderer 切替 + WASM ready 検証」）。

- [ ] **Step 4: `/code-review`（high）**

remote/session 境界を含むため high。finding は `.claude/rules/review-judgement.md` で対応判断（価値・リスクで判断、スコープ内外・工数は考慮しない）。

- [ ] **Step 5: `/security-review`**

新エンドポイント（kill/delete/rename）＋ remote 露出を対象に実施。spec §セキュリティ考慮（kill は terminal/filer フルシェルの部分集合＝権限昇格なし、name 検証、argv 配列渡し）を確認。`src/auth.rs`/`src/tls.rs` は無改変であることも確認。

- [ ] **Step 6: 実機スモーク**

iPad で zellij Native セッションを新規作成 → コマンドバー（tab/status bar）が出る／Den クライアントで Ctrl+R 等が干渉しない（`clear-defaults` 維持）／Sessions モーダルで Kill・rename。**既存 bare セッションはバー無しのまま残る**（作り直し案内）。

- [ ] **Step 7: finishing-a-development-branch → master squash merge**

全ゲート緑・レビュー対応後、`finishing-a-development-branch` スキルで master へ squash merge（マージ先 = master）。リリースは flow 範囲外 → `/release`。リリースノートに「コマンドバー復活は `56567a6` のキー方針の反転ではない／既存 bare セッションは作り直し要」を明記。

---

## Self-Review

**1. Spec coverage:**
- ① Native 化 → Task 1 ✓（zellij `-l` 撤去・tmux `status off` 削除・den-bare.kdl 削除・テスト反転）
- ② ＋メニュー再設計 → Task 5 ✓（グループ化・backend アイコン・DESIGN.md トークン）
- ③ セッション管理: backend ops → Task 2 ✓ / alias 永続化 → Task 3 ✓ / API → Task 4 ✓ / モーダル → Task 6 ✓
- ④ tmux 下部欠け → Task 8 ✓（別トラック・調査タスク）
- remote allowlist → Task 4 Step 6 ✓（本体不変・テスト追加）
- セキュリティ（name 検証・argv 配列・remote 露出）→ Task 2/4 ＋ Task 9 Step 5 ✓
- テスト各レイヤ（unit backend/assets/multiplexer_api/remote・e2e menu/modal・renderer smoke・実機）→ Task 1-4/7/9 ✓
- 品質ゲート → Task 9 ✓

**2. Placeholder scan:** Task 8 の Step 2-3 は「調査結果で具体化」と明記した意図的な調査タスク（真因が実環境調査前のため spec も仮説段階）。それ以外のコードステップは実コードを記載。フロント実装で「既存実装を Read して合わせる」と注記した箇所（`save_settings` シグネチャ・`Toast.confirm`/`show` の有無・`allModals` 登録・CSS トークン名・入口位置・e2e セレクタ）は、コードベース固有の不確定点であり実装時に Read で確定する明示指示。

**3. Type consistency:**
- `MuxConfig { zellij_config, tmux_conf }`（Task 1）— `zellij_layout` を全タスクで参照しない ✓
- `build_launch_command` シグネチャ不変・argv のみ変更 ✓
- `is_valid_mux_name` / `kill_mux_session` / `delete_mux_session`（Task 2）→ Task 4 で同名利用 ✓
- `BackendStatus { available, sessions, aliases }`（Task 4）→ フロント `bs.aliases`（Task 5/6）一致 ✓
- `OpResult { ok, message }`（Task 4）→ フロント `res.ok` / `res.message`（Task 5/6）一致 ✓
- alias キー `"<backend>:<name>"`（Task 3）↔ `aliases_for` の prefix strip（Task 4）一致 ✓
- API パス `multiplexer/{kill,delete,rename}`（Task 4）↔ `muxOp` / allowlist（Task 6/Task 4）一致 ✓
