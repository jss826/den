# Multiplexer Session Backend (tmux / zellij) — Design

- 日付: 2026-06-20
- ステータス: 設計確定（実装前）
- 関連: #115（per-session scrollback, K=2）, `src/pty/manager.rs`, `src/pty/registry.rs`, `src/remote.rs`, `frontend/js/terminal.js`

## 背景・目的

Den は現在、リモートから接続するための独自セッション管理（registry の PTY、自前 scrollback、Den 再起動でセッション消滅）を持つ。PC で始めた作業を iPad で引き継ぎ、また PC に戻す「セッション継続」を実現するため SSH サーバー機能を入れたが、**PC ローカルの作業でも一旦 Den に SSH してセッションを作る必要がある**という摩擦があった。

セッションの住処を「Den プロセス」から **tmux / zellij** に移すことで、

- PC ローカルは普通の端末で `zellij`/`tmux` を直接使える（Den 経由の ceremony 不要）
- iPad は Den のブラウザ窓口から同名セッションに attach（multiplayer 合流）
- Den 再起動でセッションが死なない（multiplexer が永続化を所有）
- 自前 scrollback 地獄（#115）から段階的に降りられる

を狙う。zellij は 0.44.0（2026-03）で **Windows ネイティブ対応**済みのため本番 `soon-pc`（Windows/PowerShell）でも候補になる。

### multiplexer の使い分け方針（人間 / AI エージェント）

- **人間の daily / 引き継ぎ → zellij に寄せる**（Windows+nix 統一、UI が今風）。
- **AI エージェント用途（tmux-for-ai-agents 系）→ tmux を残す**。`send-keys -t`/`capture-pane -p` の外部からの特定ペイン直叩きが tmux はネイティブで、エコシステムも tmux 前提。zellij でも `write-chars`+`dump-screen`+`--pane-id` で基本は可能だが、込み入った multi-pane オーケストレーションはプラグイン（zellij-send-keys）依存。
- Den は **両方を backend として持つ**ことで、この使い分けをコストゼロで成立させる。

## スコープ

| 入れる（v1） | 入れない（将来） |
|---|---|
| ローカルホストの zellij/tmux backend | AI エージェント向け send-keys/capture-pane 連携 |
| backend 選択メニュー＋既存セッション一覧＋既定 backend 設定 | zellij-send-keys プラグイン同梱 |
| bare layout（zellij/tmux）＋ latest-active サイズ | `ssh_config` と `SessionLaunch` への統合（案B） |
| saved record への backend 永続化 | mux セッション自体の削除 UI（v1 は detach のみ） |
| remote（denA→denB）の multiplexer 識別・attach | |

**触らない境界**: `ssh_config` / `create_with_ssh` / auth・ssh コードは無改変（案A の肝。security-review トリガーを回避）。

## 設計決定（ブレスト結論）

1. **セッションモデル**: 既定 = multiplexer（設定で zellij/tmux/素シェル 切替）。Den の registry セッションモデルは維持し、**起動コマンドを multiplexer でラップ**するだけ。#115 の per-session SessionTerm 保持（K=2）は据え置き、multiplexer が上に永続化を足す。
2. **backend 表現（案A）**: 追加の `SessionBackend` enum を `create` に渡す。`ssh_config` の仕組みは温存し並列に持つ。ssh/auth 境界に触れない低リスク案。
3. **検出・fallback**: 起動時に `zellij --version`/`tmux -V` を probe してキャッシュ。既定 backend 不在なら素シェルへ静かに fallback＋Toast。メニューは可用 backend のみ。セッション一覧（`ls`）はメニュー open 毎に取得（鮮度優先）。
4. **レイアウト**: bare 既定（zellij: tab/status bar 除去 layout、tmux: `status off`）。多重化 UI は Den のタブ（Ctrl+1/2/3）が持つ。設定追加なし（YAGNI）。
5. **命名**: 1:1 同名（Den セッション名 == multiplexer セッション名）。既存 attach = 同名 create-or-attach。同名を PC/iPad 両方から → multiplayer 合流。
6. **clamp 対策**: latest-active サイズ（tmux `set -g window-size latest` / zellij は同等機能を PoC で確認 TBD）。multiplayer（同時閲覧）は残す。clamp 実挙動の PoC を実装の初タスクにする。

## アーキテクチャ

Den の registry セッションモデルは維持。変えるのは「セッション起動時に実行するコマンド」だけ。`PtyManager::spawn` が起動するコマンドを backend に応じて差し替える。

```
[iPad/PC ブラウザ]
   │ WS attach (cols/rows)
   ▼
[Den registry session "work"]   ← 1:1 同名
   │ PTY
   ▼
[ zellij attach -c work ]  ← 永続化/scrollback/detach は zellij が所有
   │
   ▼
[ powershell.exe ]         ← 実シェル
```

引き継ぎ = 同名 `work` を PC と iPad の両 Den クライアントから attach → multiplayer で合流。Den 再起動でも saved record の backend で `attach -c work` し直せば復活。

### トポロジ（"local" と "remote" の意味）

- **PC↔iPad の引き継ぎ（主目的）は "local"**: iPad ブラウザは PC の Den（`soon-pc:3939`）に**直接**接続。iPad から見て PC の mux セッションは Den の **local** セッション。→ local multiplexer 一覧でカバー。
- **"remote" は denA→denB**: 例えば Windows の Den から nix 箱の Den に Quick Connect して、その箱の tmux セッションを識別・attach するケース。

## コンポーネント

### Rust（backend）

| コンポーネント | 内容 |
|---|---|
| `SessionBackend` enum（新規 `src/pty/backend.rs`） | `Shell` \| `Zellij` \| `Tmux`。serde（API＋永続化）。`Default = Shell`（後方互換） |
| `build_launch_command(backend, shell, name, layouts) -> (program, args)`（純関数） | Shell→`(shell, [])` / Zellij→`zellij --session <name> --layout <bare.kdl>`（attach-or-create、正確な形は PoC で確定）/ Tmux→`tmux -f <den.conf> new-session -A -s <name>` |

> **コマンド形の注記**: 以降のデータフロー例で `zellij attach -c work` と略記している箇所は、すべて上記 `build_launch_command` が返す正規形（PoC #1 で確定）の短縮表記。実装では起動コマンドは常に `build_launch_command` 一箇所から得る（データフロー例は読みやすさ優先の擬似表記）。
| `PtyManager::spawn` 拡張 | 現状 `shell: &str` 1 本 → program＋args を受ける形に拡張（既存呼び出しは `(shell, [])` で不変） |
| `Registry::create_with_backend(name, cols, rows, backend)` | `create_with_ssh` の兄弟。ssh パスは触らない |
| `SessionRecord` 拡張 | `backend: Option<SessionBackend>` 追加・永続化。再起動後に正しい backend で attach |
| 起動時 probe | `zellij --version` / `tmux -V` を spawn_blocking で 1 回 → `AppState` にキャッシュ |
| `list_mux_sessions(backend)` | `zellij ls` / `tmux ls` を spawn_blocking＋短タイムアウトで実行しセッション名をパース |
| layout 同梱 | `den-bare.kdl`（zellij）/ `den.conf`（tmux）を rust-embed → 起動時に `DEN_DATA_DIR` へ書き出し（mux はファイルパス参照のため） |

### API（axum）

- `GET /api/multiplexer/status` → `{ zellij:{available, sessions:[..]}, tmux:{available, sessions:[..]} }`。availability＝キャッシュ、sessions＝都度 `ls`。既存 auth middleware 配下。
- `POST /api/terminal/sessions` の body に `backend?: "shell"|"zellij"|"tmux"` 追加（省略時 Shell＝後方互換）。
- `src/remote.rs` の `remote_proxy_catch_all` allowlist に `multiplexer/` 追加（remote 識別）。create は既存の `terminal/sessions` 経路でそのまま body プロキシされる。

### Frontend

- **Settings**: "Default terminal backend" select（Shell/Zellij/tmux）→ DenSettings `default_backend`。
- **`showNewSessionMenu`**（`frontend/js/terminal.js`）: `/api/multiplexer/status`（local）＋ remote 各 `/api/remote/{id}/multiplexer/status` を menu open 時取得。
  - `Local Terminal` → `default_backend` 解決 → `createSession(name, {backend})`。
  - `Zellij ▸` / `tmux ▸` submenu（available 時のみ）= 既存セッション一覧（tap→同名 create-or-attach）＋`(new…)`（prompt→create）。
  - Remote セクションも同じ `buildBackendSubmenu()` を流用。既定 backend 設定は**直接ログインした Den の "Local Terminal" のみ**に適用、remote は明示 backend submenu で選ぶ（cross-Den の既定混乱を回避）。
- **`createSession`** に `backend` を追加。既存 Den registry セッションが同名で在れば create はスキップして `switchSession`（1:1 同名 attach の表れ）。

### レイアウト / 設定ファイル中身

- zellij `den-bare.kdl`: `pane_frames false`、tab/status bar plugin 無しの単一ペイン（正確な KDL は PoC）。
- tmux `den.conf`: `set -g status off` ＋ `set -g window-size latest`（latest-active サイズ）。

### destroy = detach（kill しない）セマンティクス

Den の「セッション破棄」は registry セッション（=`zellij attach`/`tmux attach` の子プロセス）を kill するが、**子プロセスの kill = mux への detach** であって **mux セッション自体は生き残る**。これは引き継ぎの肝（iPad でタブを閉じても PC の作業セッションは死なない）。mux セッション自体の削除は v1 では提供せず、必要なら mux 側（`zellij delete-session` 等）で。この「kill が detach になる」挙動は PoC で確認（クリーンに detach するか）。

## データフロー

### ① 新規作成（local・既定 backend = zellij）
```
iPad: "Local Terminal" tap
 → createSession("work", {backend:"zellij"})
 → POST /api/terminal/sessions {name:"work", backend:"zellij"}
 → Registry::create_with_backend → build_launch_command
 → PtyManager::spawn("zellij", ["--session","work","--layout",".../den-bare.kdl"])
 → zellij が work を作成＋attach → 中で powershell 起動
 → saved record に backend=zellij 永続化 → WS attach → 描画
```

### ② 既存 mux セッションへ attach（一覧から）
```
menu open → GET /api/multiplexer/status → {zellij:{sessions:["main","work"]}}
 → "Zellij ▸ main" tap → createSession("main", {backend:"zellij"})
   ├ Den registry に "main" 無し → ①と同じ経路で zellij attach -c main
   └ Den registry に "main" 有り → create は AlreadyExists
       → frontend は switchSession("main") のみ（合流）
```

### ③ PC↔iPad 引き継ぎ（同名 multiplayer）
```
PC:   Den registry "work" → zellij attach (client A)
iPad: 同じ Den の "work" を開く → zellij 視点で client 増加 = multiplayer
      → latest-active サイズ（直近操作した側に画面追従）
```

### ④ Den 再起動後の復活
```
Den 再起動 → registry 空、saved record に {name:"work", backend:"zellij"}
 → "work" を開く → saved backend=zellij で再 spawn
 → zellij attach -c work（zellij 側は生存、scrollback ごと復活）
```

### ⑤ remote（denA→denB / 例: Windows→nix 箱）
```
menu open → GET /api/remote/{denB}/multiplexer/status（catch-all proxy）
 → denB が自分の {tmux:{sessions:["agent"]}} を応答
 → "Remote denB ▸ tmux ▸ agent" tap
 → POST /api/remote/{denB}/terminal/sessions {name:"agent", backend:"tmux"}
 → denB 側で tmux new-session -A -s agent
```

### ⑥ resize / clamp
```
クライアント resize → per-client PTY resize → mux にサイズ伝播
複数 attach 時: tmux=window-size latest で直近アクティブ client が駆動
              zellij=PoC で同等挙動を確認（TBD）
```

### ⑦ destroy = detach
```
タブを閉じる/破棄 → Den registry セッション削除 → attach 子プロセス kill
 = mux への detach（mux セッションは生存）
 → 次回 menu open で ls にまだ "work" が見える → 再 attach 可能
```

## エラー処理・fallback

| ケース | 挙動 |
|---|---|
| 既定 backend が PATH に無い | 起動時 probe で検知。create 時に Shell へ静かに fallback＋`Toast: "zellij not found, using PowerShell"`。メニューは可用 backend のみ |
| `zellij ls` / `tmux ls` 失敗・タイムアウト | 空 Vec を返す。submenu は `(new…)` のみ。status は `available:true, sessions:[]` |
| mux 起動失敗（layout 不正・mux 異常終了） | 既存 `RegistryError::SpawnFailed` 経路。`Toast.error('Failed to create session')`。registry へ挿入しない |
| 同名 create が既存と衝突 | `RegistryError::AlreadyExists` → frontend は switchSession のみ（create-or-attach、エラー表示しない） |
| remote status 取得失敗 | Remote セクションの submenu を出さない（local は通常表示）。既存 remote 接続失敗 UX 準拠 |
| saved backend が後で不可用化 | `attach -c` 失敗 → SpawnFailed → Toast。saved record はそのまま。v1 では自動 fallback しない（backend を勝手に変えると別物セッションになり 1:1 が壊れる） |
| layout ファイル書き出し失敗 | 起動時 warn ログ。`--layout` 省略で mux 既定 UI 起動（bare にならないが致命でない） |
| mux 名に使えない文字 | Den の `is_valid_session_name`（英数・`-`）は zellij/tmux 制約の部分集合。追加検証不要 |

### セキュリティ注意点（CLAUDE.md スロット②）

- `build_launch_command` はシェル文字列連結をしない。`CommandBuilder` に program＋args を配列で渡す（インジェクション防止）。session 名は `is_valid_session_name` で英数＋`-` に限定済みで argv に直接渡して安全。
- ssh/auth コードは無改変（案A）。security-review 対象差分に該当しない見込みだが、`remote.rs` allowlist 変更は触るので `/code-review` で確認。

## テスト戦略

| レイヤー | テスト | CI 可否 |
|---|---|---|
| Rust unit | `build_launch_command` の argv assert（各 backend × 名前）。純関数 | ✅ |
| Rust unit | `SessionBackend` serde roundtrip ＋ `SessionRecord{backend}` 永続化 roundtrip（backend 欠落 JSON → Shell の後方互換） | ✅ |
| Rust unit | `zellij ls` / `tmux ls` 出力パーサ（固定文字列、実 mux 不要） | ✅ |
| Rust | `remote.rs` allowlist `multiplexer/` 追加（既存 `sanitize_proxy_path`/allowlist テスト様式で 1 ケース） | ✅ |
| Rust（registry_test） | `create_with_backend` の Shell 経路（既存 PTY 治具、`#[test]`＋手動ランタイム）。mux backend は CI に無いので Shell のみ | ✅（Shell のみ） |
| e2e（playwright） | メニュー構造を status モックで検証（Zellij▸/tmux▸ submenu・`(new…)`・既存一覧の描画と `[hidden]`） | ✅ |
| PoC（実装 #1・手動/chrome-cdp） | ① `zellij --session/--layout` の attach-or-create 正確形 ② bare layout 実描画（bar 消滅）③ clamp と latest-active 実挙動（zellij 同等機能 TBD）④ destroy=detach がクリーンか ⑤ restty/wterm 切替スモーク | ❌ 実機 |
| 手動スモーク | PC↔iPad 引き継ぎ（同名合流）／Den 再起動後の復活／remote denA→denB 識別＋attach | ❌ 実機 |

**品質ゲート（CLAUDE.md スロット①）**: `cargo fmt --check` / `cargo clippy -D warnings` / `cargo test --target-dir target-test` / UI 変更ありのため `npx playwright test tests/e2e/filer-ui.e2e.ts`。

**CI の限界**: zellij/tmux は CI ランナーに無い前提 → mux backend の起動経路は CI で検証不可。PoC＋実機スモークで担保する。

## 未解決事項（PoC で確定する TBD）

1. `zellij --session <name> --layout <path>` の attach-or-create 正確な挙動（既存セッションに layout 指定で attach した時の振る舞い）。fallback は `zellij attach -c <name>`（bare layout 無し）。
2. zellij の latest-active サイズ相当機能の有無。無ければ clamp 挙動を spec に追記し、必要なら逐次 attach（旧 client detach）へ方針変更を検討。
3. `zellij attach`/`tmux attach` 子プロセス kill がクリーンに detach するか（mux セッション破損の有無）。
4. zellij/tmux 2 インスタンス同時稼働時の restty/wterm 切替スモーク（vendor bump ではないが描画確認）。
