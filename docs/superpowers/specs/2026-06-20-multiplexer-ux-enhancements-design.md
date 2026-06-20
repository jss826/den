# Multiplexer UX 強化（Native 化 / ＋メニュー再設計 / セッション管理 / tmux 下部欠け）— Design

- 日付: 2026-06-20
- ステータス: 設計確定（実装前）
- 関連 spec: `2026-06-20-multiplexer-session-backend-design.md`（本 spec はその直接の続き）
- 関連コード: `src/assets.rs`(`ensure_mux_layouts`) / `src/pty/backend.rs`(`build_launch_command`) / `src/multiplexer_api.rs` / `src/pty/registry.rs` / `src/store.rs`(`SessionRecord`) / `src/remote.rs`(`path_in_allowlist`) / `frontend/js/terminal.js` / `frontend/layouts/*`
- 前提リリース: v3.5.1（multiplexer session backend 実装済み）

## 背景・目的

v3.5.0 で zellij/tmux を session backend として組み込んだが、運用してみて以下の摩擦が判明した（ユーザー報告）。

1. **PC のネイティブ端末（`zellij`/`tmux` 直叩き）で attach するとコマンドバーが出ない。** Den は bare layout（`den-bare.kdl`）＋ `keybinds clear-defaults` で「Den が multiplexing UI を所有する」設計にしていたが、コマンドバーは **レイアウト＝セッション側状態**（後述）なので全クライアントで消え、PC でも復元できない。
2. **PC で開始したセッションに Den で attach すると Ctrl+R が効かない。** そのセッションのシェルは PC の zellij が起動した時点のもの（`default_shell` 未設定だと Windows は cmd.exe＝PSReadLine なし）。Den が注入する `__DEN_SHELL__` は **Den 自身が作るセッションにしか効かない**。
3. **セッションの管理手段が無い。** 一覧は ＋メニューにあるが、削除・リネーム・PC からの attach 補助が無い。
4. **＋メニューが平坦で、環境（local / 各リモート Den / backend）と Quick Connect 先の見分けがつきにくい。**
5. **tmux セッションに Den で attach すると端末下部が表示されない**（zellij では出ない tmux 限定症状）。

本 spec はこの 5 点を **4 本立て**で解消する。

### 前提知識: 何が "セッション側" / "クライアント側" か（cross-attach モデル）

`zellij setup --dump-layout default` の実出力:

```
layout {
    pane size=1 borderless=true { plugin location="tab-bar" }    // コマンドバー（上）
    pane
    pane size=1 borderless=true { plugin location="status-bar" } // コマンドバー（下）
}
```

| 項目 | 属する層 | 帰結 |
|---|---|---|
| コマンドバー（tab/status bar） | **セッション側**（レイアウト＝作成時に確定・全クライアント共有） | 作った側の設定で全員固定。後から attach しても変わらない |
| 動いているシェル | **セッション側**（作成時に spawn・共有） | 後から attach してもシェルは差し替え不可 |
| キーバインド / テーマ / ペイン枠 | **クライアント側**（attach ごとに config 適用） | Den は `--config` を毎 attach 適用 |

→ **バーの有無はセッション共通**なので「Den では消す / PC では出す」は不可能。よってバーは**全クライアントに出す**（セッションを Native レイアウトで作る）。一方**キーバインドはクライアントごと**なので、Den クライアントだけ `clear-defaults` でキーを無効化し、PC クライアントはネイティブのまま——を同一セッションで両立できる（決定 ①＝ハイブリッド C）。

## スコープ

| 入れる（本 spec） | 入れない（将来） |
|---|---|
| ① mux のコマンドバーを復活（zellij `-l bare` を外す / tmux `status off` を削除）。Den クライアントのキー無効（`clear-defaults` / `prefix None`）と `default_shell` 注入は維持 | bare/Den-managed プロファイルの per-session 選択（2-profile 案は不採用） |
| ② ＋メニュー再設計（ソース＝マシン単位でグループ化、backend をアイコン区別） | tmux 実 rename の併用トグル（zellij は外部 rename 不可なので alias に統一） |
| ③ セッション管理モーダル（一覧 / Kill / Delete / リネーム=Den エイリアス / attach コマンドコピー、local＋remote 横断） | mux セッションの自動 GC / TTL |
| ④ tmux 下部欠けバグの調査・修正（実環境） | AI エージェント向け send-keys/capture-pane 連携 |

**触らない境界**: `src/auth.rs` / `src/tls.rs` の認証・TLS コードは無改変。`ssh_config` / `create_with_ssh` も無改変。

## 決定事項（ブレスト結論）

1. **mux デフォルト = ハイブリッド C「バーは出す / Den クライアントのキーは無効のまま」**（ユーザー選択）。コマンドバー（zellij tab/status-bar、tmux status line）を復活させるが、**キーバインドはクライアント側に効く**ので Den の `--config clear-defaults`（tmux は `prefix None`）は**そのまま維持**する。
   - 具体的変更は最小: **zellij は起動から `-l den-bare.kdl` を外す**（デフォルトレイアウト＝バー有り）。**tmux は `den.conf` の `set -g status off` を削除**（status line を出す）。それ以外（`default_shell` / `clear-defaults` / `prefix None` / `window-size latest`）は不変。
   - 結果: バーは全クライアントで表示（報告①解消）。**Den クライアントはキー干渉ゼロのまま**＝ `56567a6` の方針は**維持・反転しない**。PC クライアントは自分の config（ネイティブキーバインド）で attach する。**キーバインドはクライアント側なので、同一セッションでも Den（無効）と PC（ネイティブ）が独立して共存できる**。
   - `default_shell` 注入は維持 → Den 作成セッションは PowerShell/PSReadLine で Ctrl+R が効く。
   - 報告② は「PC 作成セッションのシェル」が原因で **Den コードでは直せない**。PC 側 `~/.config/zellij/config.kdl` に `default_shell` を入れる運用ガイドをドキュメントに記載（コード対応外）。
   - **tmux の `prefix` はサーバーグローバルで per-client 分離不可** → `prefix None`（全キー素通し）を維持。tmux は主に AI エージェント（send-keys/capture-pane＝prefix 非依存）用途のため interactive prefix を諦めても影響小。zellij のような Den/PC 分離は tmux では効かない点に注意（バー表示だけ復活、キーは全クライアント素通し）。
   - 補足: config に焼かず一時的にキーを無効化したい場合の逃げ道として **locked モード（`Ctrl+g`）** がある（クライアント単位のトグル）。ただし `zellij action switch-mode locked` は all-clients に効くため Den 限定の自動化には使わない。
2. **リネーム = Den ローカルのエイリアス**。zellij は外部からデタッチ中セッションの rename 不可（`rename-session` は `zellij action` 下＝アタッチ中のみ）。tmux は `rename-session -t` で可能だが、**backend 非依存で統一する**ため実 rename はせず、Den が持つエイリアスのみ変更する。実体の mux 名は不変 → attach コマンドは本名を表示。
3. **Kill / Delete は backend CLI を外部実行**（両 backend とも名前引数を取れることを実測確認済み）。
4. **tmux 下部欠けは別トラックの bug fix**（tmux は Windows 非対応のため remote nix / WSL で systematic-debugging）。本体 UI 機能のリリースを待たせない。

### backend 操作能力（zellij 0.44.3 / tmux 実測ベース）

| 操作 | zellij | tmux |
|---|---|---|
| 一覧 | `list-sessions --short --no-formatting` | `ls` |
| Kill（実行中を終了） | `kill-session <name>` | `kill-session -t <name>` |
| Delete（終了済み resurrect 状態を掃除） | `delete-session <name>`（`--force` で kill+delete） | 該当概念なし（Kill が兼ねる） |
| Rename（外部・デタッチ中） | **不可**（`action rename-session` はアタッチ中のみ） | `rename-session -t <old> <new>`（可だが本 spec では未使用） |

## ① mux Native 化（バーだけ出す・キーは Den クライアント無効を維持）

**最小変更**。`frontend/layouts/` の config 本体（`den-zellij.kdl` / `den.conf`）の `default_shell` / `clear-defaults` / `prefix None` は**いじらない**。やるのは「バーを出す」だけ。

### 変更点

- **zellij: 起動から `-l den-bare.kdl` を外す**。
  - デフォルトレイアウト（tab-bar + status-bar 付き）でセッションが作られ、バーが**全クライアントに**出る（バーの有無はセッション共通のため）。
  - `den-zellij.kdl`（`default_shell` + `keybinds clear-defaults` + `pane_frames false`）は**そのまま `--config` で渡す** → Den クライアントはキー干渉ゼロのまま。
  - `MuxConfig.zellij_layout` フィールドと `build_launch_command` の `-l` 付与を撤去。`ensure_mux_layouts` から `den-bare.kdl` 書き出しを削除（asset 自体も削除）。
- **tmux: `den.conf` から `set -g status off` を削除**。
  - status line（バー）が出る。`default-command` / `prefix None` / `unbind C-b` / `window-size latest` は**維持**。
  - tmux の `prefix` はサーバーグローバルで per-client 分離不可のため、`prefix None`（全キー素通し）を維持＝バーは出るがキーは全クライアント素通し（zellij のような Den/PC 分離は tmux ではできない）。
  - 注: `-f <conf>` は tmux サーバー初回起動時のみ適用。既存サーバー（PC 起点）への attach では無視＝既存挙動どおり。

### `build_launch_command`（`src/pty/backend.rs`）

```
zellij --config <den-zellij.kdl> attach -c <name>   // -l を付けない（デフォルトレイアウト＝バー有り）
tmux   -f <den.conf> new-session -A -s <name>        // den.conf は status off を除いた版
```

`MuxConfig` から `zellij_layout` を削除。既存テスト（`zellij_backend_attach_or_create_with_config_and_layout` / `zellij_backend_config_without_layout` 等）は `-l` 無しの新 argv に更新。

### `__DEN_SHELL__` のエスケープ

既存の制御文字除去＋`\`/`"` エスケープ（`assets.rs:92-97`）を維持。config 本体は不変なので注入境界も不変。

### 移行上の注意

- **既に bare で作成済みの zellij セッション**は、バー無しのまま残る（レイアウト＝セッション側状態）。新規作成分からバーが出る。「バーを出したい既存セッションは作り直しが必要」と案内。
- **これは `56567a6`（Den クライアントのキー素通し）の反転では無い**。キー方針は維持。変わるのは「バーの表示」だけ。release ノートにもそう記す。

## ② ＋メニュー再設計

### 現状（`frontend/js/terminal.js` `buildNewSessionMenu`）

`new-session-menu-item` のフラットな行に薄い `new-session-menu-separator` が混ざる構造。ローカル / 各リモート Den / backend / SSH / Quick Connect が視覚的に同列。

### 新構造: ソース（マシン）でグループ化

```
┌─ New session ─────────────────┐
│ 🖥  This Den (local)           │   ← グループヘッダ（アイコン＋ラベル、非クリック）
│     Local Terminal            │
│     ▸ zellij   work, agent  + │   ← backend サブグループ（既存 + 一覧 + New）
│     ▸ tmux     main         + │
│                               │
│ 🌐  Remote: macbook           │   ← Quick Connect 先ごと
│     New Terminal              │
│     ▸ zellij   build        + │
│                               │
│ 🔑  SSH                       │
│     prod-web                  │
│                               │
│ ＋  Quick Connect Den…         │   ← 新規接続（最下部・常設）
└───────────────────────────────┘
```

- **グループヘッダ**（新クラス `new-session-menu-group`）: アイコン＋マシン名。`This Den (local)` / `Remote: <displayName>` / `SSH`。
- **backend 行**（新クラス `new-session-menu-backend`）: 左にアイコン（shell/zellij/tmux を色・字形で区別）。zellij=⬡ 系、tmux=▢ 系、shell=❯ など。アイコンは絵文字 or インライン SVG（CSP 上 inline `onclick` は不可だが SVG 要素は可）。
- **セッション名チップ**: 既存 mux セッションは横並びチップで列挙、末尾に `+`（New）。
- backend が available でないグループは backend 行を出さない（既存 `buildBackendSubmenu` の available 判定を流用）。
- 配色/アイコンは `frontend/DESIGN.md` のトークンに追加（backend 識別色を新規定義）。

### 振る舞い

- 既存の `fetchMuxStatus(connId)` 並列プリフェッチ（2s timeout）と `createSession(name, ssh, connId, backend)` はそのまま流用。グルーピングは DOM 構造の変更のみで、API は不変。
- E2E（`tests/e2e/multiplexer-menu.e2e.ts`）にグループヘッダ・backend アイコンのセレクタ assert を追加。

## ③ セッション管理モーダル

### UI

新モーダル `#sessions-modal`（`allModals` に登録、`confirm-modal` パターン準拠）。タイトル「Sessions」。

```
Sessions
─────────────────────────────────────────────
This Den (local)
  zellij
    ◦ work     [rename] [copy attach] [Kill] [Delete]
    ◦ agent    [rename] [copy attach] [Kill] [Delete]
  tmux
    ◦ main     [rename] [copy attach] [Kill]
Remote: macbook
  zellij
    ◦ build    [rename] [copy attach] [Kill] [Delete]
─────────────────────────────────────────────
                                       [Close]
```

- **行**: 実体 mux 名（＋ Den エイリアスがあれば `alias (real)` 表示）。
- **rename**: インライン入力 → Den エイリアスを更新（後述 API）。実体名は不変。
- **copy attach**: クリップボードへ `zellij attach <name>` / `tmux attach -t <name>` をコピー（`navigator.clipboard`）。Toast で「Copied」表示。
- **Kill**: `confirm-modal` で確認 → `POST .../multiplexer/kill`。
- **Delete**（zellij のみ表示）: 確認 → `POST .../multiplexer/delete`。
- 操作後は mux status を再 fetch して再描画。Den が attach 中だったセッションを Kill した場合、その Den PTY（`zellij attach`）が終了し、既存の dead-session クリーンアップで Den 側セッションも消える。
- 入口: 設定メニュー or ＋メニュー脇に「Manage sessions」エントリ。

### API（`src/multiplexer_api.rs`）

既存 `GET /api/multiplexer/status` に加え:

| メソッド/パス | body | 動作 |
|---|---|---|
| `POST /api/multiplexer/kill` | `{ backend, name }` | `kill-session` を spawn_blocking 実行。成功/失敗を JSON で返す |
| `POST /api/multiplexer/delete` | `{ backend, name }` | zellij `delete-session`（`--force` 付き）。tmux は kill にフォールバック or 400 |
| `POST /api/multiplexer/rename` | `{ backend, name, alias }` | Den ローカルのエイリアスマップを更新（mux CLI は叩かない） |

- **name バリデーション必須**: `is_valid_session_name`（英数＋`-`）で弾く。argv 直渡し（シェル経由でない）だが防御的に検証。
- backend 操作は `src/pty/backend.rs` に `kill_mux_session(backend, name)` / `delete_mux_session(backend, name)` を追加（`list_mux_sessions` と同じく blocking → `spawn_blocking`）。
- **remote 透過**: `path_in_allowlist`（`src/remote.rs:459`）は既に `multiplexer/` 始まりを許可済み → `multiplexer/kill` 等は **追加変更なしで denA→denB プロキシ可能**。

### エイリアス永続化（`src/store.rs`）

- 新規 `mux_aliases: Option<HashMap<String, String>>`（キー `"<backend>:<name>"`、値=エイリアス）を `Settings` か専用ファイルに追加。backend の live 一覧（`list_mux_sessions`）と join して表示。
- SessionRecord は触らない（mux 名と Den セッション名は現状一致のため、エイリアスは別レイヤで持つ方が外部作成セッションにも効く）。
- v1 ではエイリアスはモーダル＋＋メニューの attach 一覧に反映。**Den のタブラベルへの反映は将来**（タブは現状 mux 名表示のまま）。

## ④ tmux 下部欠けバグ（別トラック）

- **症状**: Den→tmux attach で端末最下部が描画されない（zellij では再現せず）。
- **仮説（有力順）**:
  1. tmux はフルスクリーン TUI で「tmux ウィンドウ行数」ぴったりにしか描画しない。Den の xterm 行数と tmux ウィンドウ行数の不一致で下部が欠ける（SIGWINCH/resize 同期ズレ）。
  2. `-A` で既存セッションに attach する際、サーバー既存だと `-f den.conf` が無視され、`window-size` が他クライアント基準のまま。
  3. Native 化でステータス行が最下行に復活 → サイズ勘定がズレると最下部（ステータス行）が欠ける。
- **調査手段**: tmux は Windows 非対応のため **remote nix / WSL** で `systematic-debugging`。Den の PTY resize 経路（cols/rows 送出 → tmux SIGWINCH）を実測。
- **本体 UI 機能（①②③）のリリースをブロックしない**（独立して進める）。

## エラー処理

- mux CLI 実行失敗（非ゼロ終了 / 実行ファイル無し）→ API は `{ ok:false, message }` を返し、フロントは Toast.error。
- Kill 対象が既に消えている → 成功扱い（冪等）にするか、stderr を message で返す。実装時に確定。
- remote プロキシ越しの kill/delete でリモート Den 到達不可 → 既存 remote proxy のエラー伝播に従う。

## セキュリティ考慮

- **subprocess 実行**: name は `is_valid_session_name` で英数＋`-` に限定。argv 配列で渡す（シェル文字列連結しない）ためインジェクション面は無いが、多層防御として検証。
- **remote 経由 kill/delete**: `multiplexer/` allowlist により、接続中の denA が denB の mux セッションを Kill/Delete 可能になる。これは **Quick Connect が既に terminal/filer の完全制御（＝フルシェル）を許可している**ことの部分集合（kill-session はそれ以下の権限）なので新たな権限昇格ではない。ただし spec として明記し、`/security-review` を新エンドポイント＋remote 露出に対して実施する。
- `default_shell` 注入のエスケープは既存実装を維持。

## テスト

| レイヤ | 内容 |
|---|---|
| unit (`backend.rs`) | `build_launch_command` の新 argv（`-l` 無し）、`kill_mux_session`/`delete_mux_session` の argv 構築、name バリデーション |
| unit (`assets.rs`) | `den.conf` に `status off` が**含まれない**こと、`den-zellij.kdl` に `clear-defaults` と `default_shell` が**維持**されていること、`den-bare.kdl` を書き出さないこと、制御文字除去 |
| unit (`multiplexer_api.rs`) | kill/delete/rename ハンドラの payload・エラー JSON |
| unit (`remote.rs`) | `path_in_allowlist("multiplexer/kill")` 等が許可されること |
| e2e (`multiplexer-menu.e2e.ts`) | ＋メニューのグループヘッダ/backend アイコン、`page.route` で status モック |
| e2e (新規) | Sessions モーダルの一覧表示・rename 入力・copy ボタン・Kill 確認フロー（mux API はモック） |
| renderer 切替スモーク | Native 化で起動引数が変わるため、restty/wterm で初期描画・CJK・theme・入力エコーを確認（`.claude/rules/workflow.md`） |
| 実機 | iPad で zellij Native セッションのコマンドバー表示、Kill/rename。tmux は remote nix で下部欠け検証（④） |

## 品質ゲート

`cargo fmt -- --check` / `cargo clippy -- -D warnings`（`--all-targets` は使わない）/ `cargo test --target-dir target-test` / `npx playwright test tests/e2e/filer-ui.e2e.ts`（UI 変更のため）/ ESLint。

## 未確定・リスク

- **④ tmux 下部欠けの真因**は実環境調査前のため仮説段階（①②③ と独立）。
- **キー衝突は Den では起きない**（`clear-defaults` 維持）。zellij は PC クライアントが自分の config（ネイティブ）で attach するため PC 側のキーは PC の責任。**tmux のみ** `prefix None` 維持で全クライアント素通し＝PC でも Ctrl+b は tmux に取られない（interactive tmux prefix を PC で使いたい人には制約。AI エージェント用途では無関係）。
- **バー中身の per-client レンダリング**（モード/キーヒントがクライアントごとに出るか）は実機未確認。設計はバーの「有無＝共有」にのみ依存するため影響なし。
- **エイリアスの保存先**（`Settings` 内 vs 専用 JSON）は実装時に確定。外部作成セッションにも効くよう SessionRecord とは分離する方針は固定。
- **既存 bare セッションの混在**: Native 移行後も旧セッションは bare のまま残る（作り直し案内）。

## 実装順序（目安）

1. ① Native 化（`assets.rs` テンプレート / `backend.rs` argv / 既存テスト更新）。
2. ③ バックエンド（kill/delete/rename API＋`backend.rs` 操作関数＋エイリアス永続化）。
3. ② ＋メニュー再設計（DOM/CSS）。
4. ③ フロント（Sessions モーダル）。
5. ④ tmux 下部欠け（別トラック・remote nix）。
6. 全ゲート → renderer 切替スモーク → `/code-review`(high) → `/security-review` → 実機。
