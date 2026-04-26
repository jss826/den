# Den UI Design

Den のフロントエンド (`frontend/`) のデザインシステム規約書。**`frontend/` 配下を編集するときは、コード変更前にこのドキュメントを参照する**こと（`.claude/rules/development.md` から trigger される）。

実装が DESIGN.md と乖離した場合、原則は実装側を直して整合させる（DESIGN.md を後から書き換えるのは「仕様変更を意図したとき」だけ）。

> このドキュメントは Den の UI に関する north-star。CLAUDE.md / `.claude/rules/development.md` からも参照される開発者向け内部ドキュメントなので日本語で書く。コミットメッセージや UI 文言は CLAUDE.md の言語規約に従う（英語）。

## 目次

1. [デザイン原則](#1-デザイン原則)
2. [デザイントークン](#2-デザイントークン) — color / state-tint / spacing / radius / font-size / z-index / breakpoint
3. [テーマ](#3-テーマ) — 13 テーマ、追加ルール
4. [レイアウト構造](#4-レイアウト構造)
5. [コンポーネント](#5-コンポーネント) — Tab / Pane / Modal / Keybar / Float / Filer / Chat / Buttons / Scrollbar
6. [規約まとめ（チェックリスト）](#6-規約まとめチェックリスト)
7. [既知の Drift（リファクタ候補）](#7-既知の-driftリファクタ候補)
8. [更新ルール](#8-更新ルール)
- [付録 A. ファイル参照](#付録-a-ファイル参照)
- [付録 B. 関連ドキュメント](#付録-b-関連ドキュメント)

---

## 1. デザイン原則

Den は **開発者向けターミナル/ファイラ/チャット**。LP のような editorial デザインは目指さない。

1. **キーボードファースト**: ショートカット (`Ctrl+1/2/3` でタブ切替等) が常に優先。マウス/タップは補助。
2. **iPad/タッチ両対応**: タッチターゲット **44-48px 最低**。`@media (hover: none) and (pointer: coarse)` 内で必ず拡大版を定義する。
3. **テーマは色のみで切り替える**: レイアウトはテーマ非依存。13 テーマ全てで同一の構造が成立する。
4. **素の HTML/CSS/JS を維持**: フレームワーク (React/Vue/Tailwind 等) 導入禁止。CSS 変数 + IIFE モジュール構成を貫く。
5. **`[hidden]` で表示制御を統一**: 動的に出し入れする要素は `element.hidden = true/false` で操作し、CSS 側で `[hidden] { display: none; }` を必ず併記する（ID セレクタの `display: flex` が打ち消すため）。
6. **静的バンドル**: rust-embed で配信されるため、ファイル分割は最小限（CSS は `frontend/css/style.css` 1 本に統一）。

---

## 2. デザイントークン

すべてのトークンは `:root` (`frontend/css/style.css:2-36`) に定義し、各テーマで上書きする。**新規トークン追加時は全 13 テーマに同時追加すること**（テーマごとに値の意味が変わるため、デフォルト継承では破綻する）。

> **例外**: フォント・spacing 等のテーマ非依存トークン（`--font-mono`, `--keybar-vertical-top` 等）は `:root` のみで定義する。

### 2.1 カラートークン

| トークン | 役割 | 使用例 |
|---|---|---|
| `--bg` | アプリ全体の背景 | `body`, `.tab.active` |
| `--fg` | 標準テキスト | 全テキスト要素 |
| `--surface` | ペイン・カード・モーダルの面 | `.modal-content`, `.pane` |
| `--border` | 仕切り線 | `border` 1px solid |
| `--accent` | プライマリ操作 | active タブ、送信ボタン、フォーカスリング |
| `--success` `--warn` `--error` | ステータス | バッジ、トースト、diff |
| `--muted` | 補助テキスト | breadcrumb、status bar |

### 2.2 State Tint（透過色レイヤー）

ホバー・フォーカス・選択状態は **必ず tint トークンを使う**（直接 `rgba(...)` を書かない）。

| トークン | 用途 |
|---|---|
| `--hover-subtle` (3%) | リスト項目の hover |
| `--hover` (5%) | 標準ボタン hover |
| `--hover-strong` (10%) | 強調 hover、選択中項目の bg |
| `--accent-tint` ~ `--accent-tint-4` | accent 色の透過 5 段階（0.1, 0.15, 0.2, 0.3, 0.6） |
| `--error-tint` `--warn-tint` (15%) | エラー/警告の bg |
| `--code-bg` | inline code, code block |
| `--inset-bg` `--overlay-bg` `--overlay-bg-strong` | 沈み込み・モーダル背景 |
| `--diff-add-bg` `--diff-del-bg` | diff hunks |

### 2.3 Spacing Scale

**4px ベース**。`4 / 8 / 12 / 16 / 24 / 32` を基本とする。`6 / 10` は密集 UI（keybar、tab 内部）に限定。

> **Drift**: 現状 1px / 2px / 3px の微調整値が散在（`.breadcrumb-segment` 等）。今後の修正時に 4px ベースに寄せる。CSS 変数化は未着手だが、新規コードでは数値を直接書いて構わない。**ルールは「4 の倍数を選ぶ」だけ**で守れる。

### 2.4 Border Radius

| 値 | 用途 |
|---|---|
| `4px` | 微小要素（chip, badge, snippet item） |
| `6px` | ボタン、タブ、入力フィールド |
| `8px` | カード、モーダル要素、フローティングターミナル |
| `12px` | モーダル本体 |

> **Drift**: `.session-bar-btn` (6px) と `.filer-tool-btn` (4px) のように同じ「ツールバーアイコンボタン」で揃っていない箇所がある。新規ボタンは **6px** を選ぶ。

### 2.5 Font Size

`rem` ベースで以下の bucket を使う。**新しいフォントサイズを増やさない**。

| サイズ | 用途 |
|---|---|
| `0.65rem` | バッジ、極小ラベル |
| `0.7rem` | breadcrumb、ファイラのメタ情報 |
| `0.75rem` | status bar、補助テキスト |
| `0.8rem` / `0.85rem` | タブ、ボタン、ツールバー |
| `0.9rem` / `1rem` | 本文、見出し |
| `1.1rem` | アイコン |
| `2.5rem` | ログイン画面のタイトル（特殊） |

### 2.6 Z-index バンド

階層を**バンド単位**で確保する。新規要素は所属バンドの値だけを使う。

| バンド | 範囲 | 例 |
|---|---|---|
| ローカル | `1` ~ `10` | `.float-resize`, `.terminal-empty-state`, `.select-mode-overlay` |
| ペイン内オーバーレイ | `50` ~ `100` | `.spinner-overlay`, `.snippet-popup`, `.clipboard-history-popup` |
| サイドバー expand (mobile) | `140` ~ `150` | `.sidebar-overlay`, `.float-terminal`, `.chat-sidebar.sidebar-expanded` |
| グローバル UI | `160` | `#keybar` |
| モーダル系 | `200` | `.modal`, `.stack-popup`, `.text-input-history-popup` |
| トースト | `400` | `#toast-container` |
| ドロップダウン | `1000` | `.new-session-menu` |
| ツールチップ | `9999` | `#den-tooltip` |

> **Drift**: float-terminal (150) と keybar (160) が近接。意図通り (keybar が float の上) だが、**float の中にモーダルを出すと keybar の下に隠れる**ため、float 内モーダルは要設計（現状そのケースは無い）。

### 2.7 ブレークポイント

| 値 | 用途 |
|---|---|
| `768px` | sidebar 折りたたみ閾値（filer / chat） |
| `(hover: none) and (pointer: coarse)` | タッチデバイス検出。**幅ベースより優先**してこちらを使う |
| `(prefers-reduced-motion: reduce)` | アニメーション無効化 |

> **Drift**: 過去の修正で `600px` (modal) と `640px` (TLS Trust grid) が混入。**今後は 768px に統一**、必要なら新規 bucket を追加する前にここに記載する。

---

## 3. テーマ

13 テーマ（dark 9 / light 4）。`<html data-theme="...">` で切替。

| テーマ | 種別 | 行 |
|---|---|---|
| (default) | dark | `style.css:2-35` |
| `light` | light | `:37-73` |
| `solarized-dark` / `solarized-light` | dark / light | `:76-146` |
| `monokai` | dark | `:149-180` |
| `nord` | dark | `:183-214` |
| `dracula` | dark | `:217-248` |
| `gruvbox-dark` / `gruvbox-light` | dark / light | `:251-321` |
| `catppuccin` | dark | `:324-355` |
| `one-dark` | dark | `:358-389` |
| `github-light` / `one-light` | light | `:392-467` |

**ルール**:

- light テーマは `[data-theme="..."], [data-theme="..."] body { color-scheme: light; }` を必ず付ける（フォーム要素のネイティブダーク化を抑制）。
- 新テーマ追加時は §2.1〜§2.5 のトークンを**すべて埋める**。デフォルト継承に頼らない（dark/light で意味が変わるため）。
- CodeMirror のライト時補正は `style.css:470-494` で別管理。新ライトテーマ追加時はここにも追加。

---

## 4. レイアウト構造

```
<body>
  #login-screen | #main-screen
    .tab-bar (height: 40px)
    .pane (#terminal-pane | #filer-pane | #chat-pane)
      ※ position: absolute; top: 40px; bottom: env(safe-area-inset-bottom)
    #keybar (position: fixed; z-index: 160)
    .float-terminal (position: fixed; z-index: 150)
    #toast-container (z-index: 400)
    #den-tooltip (z-index: 9999)
```

**規約**:

- **ペイン高さ**: `top: 40px` (tab-bar) + `bottom: env(safe-area-inset-bottom, 0px)`。iOS の home indicator を避ける。
- **`#filer-pane` に `display: flex` を追加してはいけない**。absolute positioning + `.filer-layout { height: 100% }` で動作している。`flex` を入れると Safari でレイアウト崩壊。
- **`.pane` の子要素 (#terminal-pane / #chat-pane) には ID セレクタで `display: flex` を当てている**。そのため `[hidden]` が効かないので `要素[hidden] { display: none; }` を必ずセットで書く（既存箇所: `:629-631`）。
- **keybar は orientation 切替**: `data-orientation="horizontal"` (画面下) / `"vertical"` (画面右、ドラッグでリサイズ可)。

---

## 5. コンポーネント

### 5.1 Tab Bar（メインタブ）
- 高さ 40px、タブ padding `0.5rem 1rem`、border-radius `6px 6px 0 0`
- `.tab.active` は `--bg` を使ってペインと一体化
- タッチデバイスで `min-height: 44px`

### 5.2 Pane
- `position: absolute; top: 40px; left: 0; right: 0; bottom: env(safe-area-inset-bottom, 0px)`
- 表示制御は `[hidden]` 属性。CSS 側に `[hidden] { display: none }` を必ず併記
- `#filer-pane` は **flex 禁止**

### 5.3 Modal
- `position: fixed; z-index: 200`、背景 `rgba(0,0,0,0.6)`
- `.modal-content`: `max-width: 480px; padding: 24px; border-radius: 12px`
- 600px 以下で `max-width: 95%; padding: 16px` に縮小
- **`confirm-modal` / `prompt-modal` は `escModals` に含めない**（Promise 未解決防止、CLAUDE.md memory より）
- **API 失敗時は必ず `modal.hidden = true`**（空モーダル残留防止）

### 5.4 Keybar
- 水平: `position: fixed; bottom: 0; flex-direction: row; height: 40px (touch: 44px)`
- 垂直: `top: var(--keybar-vertical-top); flex-direction: column; cursor: ew-resize`
- `#keybar` は ID で `display: flex`、表示制御は `#keybar:not([hidden])` でガード
- `.key-btn`: `min-height: 44px` (touch)、padding `8px 12px`、radius `6px`

### 5.5 Float Terminal
- `position: fixed; z-index: 150; min-width: 320px; min-height: 200px; border-radius: 8px`
- 8 方向 resize handle (`.float-resize[data-dir="..."]`) はタッチで 12-20px に拡大
- DenTerminal とは `den:sessions-changed` CustomEvent 経由で通信（循環依存回避）

### 5.6 Filer
- サイドバー `260px (min 200px)`、collapsed 時 `width: 0`
- breadcrumb: monospace, `0.7rem`
- タブ: `padding 6px 12px; max-width 180px`
- ステータスバー: `0.75rem`, padding `2px 10px`, border-top
- 768px 以下でサイドバーは overlay 表示
- **HTML プレビュー** (`.filer-html-preview-frame`): iframe で **`background: #fff` 固定**（プレビュー対象 HTML の作者意図に揃えるため、テーマ非依存）。Markdown プレビューは `--bg` を使う

### 5.7 Chat
- サイドバー仕様は Filer と揃える（260px / collapse / overlay）
- メッセージ: user `max-width: 85%`, assistant `max-width: 95%`
- `padding 8px 12px; border-radius: 8px; gap: 8px`
- **`#chat-pane[hidden] { display: none }` 必須**
- Markdown は `DenMarkdown.renderMarkdown()` + `sanitize()` を使う（`render` メソッドは存在しない）
- **Advanced Section** (`.chat-advanced-section`): `<details><summary>` を使った折りたたみ。summary は `cursor: pointer; font-weight: 500; padding: 4px 0`、`[open]` 時に `margin-bottom: 6px`。中の `.chat-tool-list` は monospace `0.85rem`、`resize: vertical; min-height: 2.4em`

### 5.8 Buttons
標準ボタンクラスは未確立。新規ボタンは以下のいずれかに合わせる:

| クラス | 用途 | サイズ |
|---|---|---|
| `.modal-btn` (`.primary`) | モーダル内の確定/キャンセル | padding `8px 16px`, radius `8px` |
| `.text-input-send-btn` | 入力フィールド付随送信 | padding `0.4rem 1rem`, accent bg |
| `.session-bar-btn` | ツールバーのアイコンボタン | `min 32px`（**touch 拡大未対応 = drift**） |
| `.key-btn` | keybar 個別キー | padding `8px 12px`, radius `6px`, touch 44px |

> **Drift**: `.btn` という汎用クラスが無く、似た見た目のボタンが個別に定義されている。共通 base class への抽出は未着手。新規追加時は**最も近い既存クラスを再利用**し、新クラスを増やさないこと。

### 5.9 Scrollbar
- WebKit: `width/height: 8px` (desktop) / `16px` (touch)
- thumb: `var(--border)`, hover `var(--muted)`, radius `4px`
- Firefox: `scrollbar-width: thin; scrollbar-color: var(--border) transparent`

---

## 6. 規約まとめ（チェックリスト）

新規 UI を追加する/既存 UI を変更するときは以下を確認:

### 6.1 必須

- [ ] **`[hidden]` で表示制御するか？** → CSS で `display: flex` 等を当てるなら `要素[hidden] { display: none; }` を併記
- [ ] **タッチターゲットが 44-48px 以上か？** → `@media (hover: none) and (pointer: coarse)` 内に `min-height/min-width` 追加
- [ ] **色は CSS 変数を使っているか？** → 直接 `#hex` / `rgba()` を書かない
- [ ] **z-index は §2.6 のバンドに収まるか？** → 新バンドが必要なら DESIGN.md に追記してから使う
- [ ] **新規フォントサイズ・新規 spacing 値を増やしていないか？** → §2.3 / §2.5 の bucket から選ぶ
- [ ] **13 テーマ全てで動くか？** → トークン追加時は全テーマで定義
- [ ] **CSP に違反しないか？** → inline `onclick` 禁止、`addEventListener` を使う
- [ ] **E2E テストを実行したか？** → `npx playwright test` で CSS レイアウト変更を必ず検証 (`.claude/rules/workflow.md`)

### 6.2 推奨

- [ ] hover/active/focus/disabled の状態を明示
- [ ] `prefers-reduced-motion: reduce` でアニメーションを無効化
- [ ] iOS Safari の `safe-area-inset-bottom` を考慮
- [ ] `position: fixed` は本当に必要か？ペイン内 `absolute` で済まないか確認

---

## 7. 既知の Drift（リファクタ候補）

DESIGN.md 制定時点で実装が規約からずれている箇所。今後修正対象とする。

| # | Drift | 影響 | 対応方針 |
|---|---|---|---|
| D1 | `.session-bar-btn` `.modal-btn` がタッチ拡大未対応 | iPad で押しにくい | touch media query に `min-height: 44px` 追加 |
| D2 | border-radius `4px`/`6px` がツールバーボタンで混在 | 見た目の一貫性 | `6px` に統一 |
| D3 | `.breadcrumb-segment` 等で 1-3px の微調整 padding | spacing scale 違反 | 4px ベースに寄せる |
| D4 | ブレークポイントが `600px` / `640px` / `768px` 混在 | レスポンシブ予測困難 | `768px` に統一 |
| D5 | `--viewport-height` が実質常に 100% | dead variable 候補 | 削除可否を確認 |
| D6 | `.btn` 共通基底クラスが無い | ボタン定義が散乱 | base class 抽出は中期課題 |
| D7 | rem ベース padding と px ベース padding が混在 | 数値計算が不揃い | px に統一（rem は font-size のみ）|

> **Drift 対応の進め方**: 単独で「Drift を直すだけの PR」は作らない。関連箇所を触る PR の中で **ついでに直す** 方針（review-judgement.md の「修正の価値 vs 放置のリスク」）。重要度が上がったら個別 Issue 化。
>
> **解消済み（参考）**: `var(--undefined-token, fallback)` パターンは Issue #105 で全スキャン済み。`--pane-bg` `--font-mono` `--danger` `--danger-hover` `--accent-fg` `--fg-muted` `--dim` `--text-muted` は全て既存の定義済みトークン（`--surface` `--error` `--muted` 等）に統合、`--font-mono` は `:root` で定義。残る defensive fallback は `--keybar-layout-width`（JS が動的設定する runtime variable）のみで意図通り。

---

## 8. 更新ルール

### このドキュメントを更新するタイミング

1. **新トークン・新コンポーネント・新規約を追加した**: 実装と同じ PR で DESIGN.md を更新
2. **既存規約を変える必要が生じた**: まず DESIGN.md の改定提案を出し、ユーザー承認後に実装と同期
3. **Drift を解消した**: §7 の該当行を削除
4. **新しい Drift を見つけた**: §7 に追記（即修正でなくてもよい）

### 実装と DESIGN.md がずれていたら

- **原則**: 実装を直して DESIGN.md に揃える
- **例外**: DESIGN.md 自体が現実離れしているとき → ユーザーに「規約を変える/緩める提案」を出してから両方更新（`workflow.md` の「ワークフロー遵守」原則と同じ）

---

## 付録 A. ファイル参照

- メイン CSS: `frontend/css/style.css` (4321 行)
- wterm 上書き: `frontend/vendor/wterm/wterm.den.css`
- xterm.js: `frontend/vendor/xterm.css`
- HTML 構造: `frontend/index.html`
- 主要 JS モジュール:
  - `frontend/js/terminal.js` (DenTerminal, セッション管理)
  - `frontend/js/float-terminal.js` (フローティングターミナル)
  - `frontend/js/filer/` (ファイラ各機能)
  - `frontend/js/chat.js` (Chat タブ)
  - `frontend/js/markdown.js` (DenMarkdown)

## 付録 B. 関連ドキュメント

- `CLAUDE.md` — プロジェクト全体の規約（CSS 注意点 §29-33 はここでメンテ、詳細は DESIGN.md を参照する形に整理予定）
- `.claude/rules/development.md` — 開発ルール全般（フロントエンド規約あり）
- `.claude/rules/workflow.md` — Phase 4 検証の厳守
- `.claude/rules/review-judgement.md` — レビュー指摘の判断基準
