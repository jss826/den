# Den - CLAUDE.md

## ビルド & 実行

```powershell
$env:DEN_PASSWORD="test"; $env:DEN_DATA_DIR="./data-dev"; cargo run
```

**`DEN_DATA_DIR` は必ず `./data-dev` を指定すること。**
`./data` は本番環境が使用中。開発・テストで上書き厳禁。

## テスト

```powershell
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test --target-dir target-test
```

**`--target-dir target-test`**: dev サーバー実行中でもバイナリロックを回避してテスト可能。

## 開発フロー（flow 前提）

Den の標準開発入口は **`/flow`**。

- `/flow <やりたいこと>` — 司会者層（受付→コンサル→分類→委譲→報告）
- `/flow #N` — Issue を 1 件実装（対話モード、7 フェーズ）
- `/flow auto #N` — headless 実装（`/loop` 用、status JSON 出力）

flow の実装エンジンはプロジェクト固有事項を以下の **4 スロット**から読む。詳細は各 rules を参照。

| スロット | Den の内容 |
|---|---|
| ① 品質ゲート | `cargo fmt -- --check` / `cargo clippy -- -D warnings` / `cargo test --target-dir target-test` /（UI 変更時）`npx playwright test tests/e2e/filer-ui.e2e.ts` |
| ② security-review 対象差分 | `src/auth.rs`・`src/tls.rs`・`src/remote.rs` 等、認証/セッション/トークン/TLS 境界に触れる差分は `/security-review` を併用 |
| ③ 実行上の罠 | `DEN_DATA_DIR=./data-dev` 厳守（`./data` 上書き禁止）/ `--target-dir target-test` でロック回避 / 長時間コマンドは background 実行 / ConPTY conhost ゾンビの後始末 / PTY テストは `#[tokio::test]` 禁止（`.claude/rules/development.md`）/ 本番 :3939 と並行時は別ポート |
| ④ フェーズ内追加チェック | 設計時=`frontend/DESIGN.md` が UI の正（先に更新）/ UI 変更時=e2e 必須＋vendor bump・adapter 修正なら renderer 切替スモーク（`.claude/rules/workflow.md`） |

**flow デフォルト（`main`）の Den 向け上書き:**

- ブランチ命名: `feat|fix|chore/<N>-<説明>`
- マージ先 = **`master`**（`main` ではない）、**squash merge**
- Phase 6 コードレビュー = **`/code-review`**（effort: 軽微 medium / 通常 high / 本丸 max）。finding の対応判断は `.claude/rules/review-judgement.md`
- リリース（tag + GitHub Release）は flow 範囲外 → **`/release`**
- Issue 外の単発コミット → **`/ship`**

## 技術スタック

- バックエンド: Rust (axum + portable-pty + tokio)
- フロントエンド: 素の HTML/CSS/JS + xterm.js v6
- 静的ファイル: rust-embed でバイナリ埋め込み
- 永続化: JSON ファイル (`./data/` 配下)

## UI / CSS

UI デザインの規約は **`frontend/DESIGN.md`** が canonical（トークン・テーマ・コンポーネント・z-index バンド・ブレークポイント・既知 Drift など全て）。新規 UI 追加・既存 UI 変更時はそちらを参照する。

特に違反しやすい 2 点だけ手元に残す:

- **`[hidden]` + ID セレクタ `display: flex` の落とし穴**: `.pane` の子要素 (`#terminal-pane` 等) は ID セレクタで `display: flex` を当てているため `[hidden]` が効かない。**新規 ID で `display: flex` を使うときは必ず `要素[hidden] { display: none; }` を併記**
- **`#filer-pane` に `display: flex` を追加してはいけない**: absolute positioning + `.filer-layout { height: 100% }` で動作中。flex 化すると Safari でレイアウト崩壊

UI 変更後は **E2E テストを必ず実行**: `npx playwright test tests/e2e/filer-ui.e2e.ts`（CSS セレクタ・hidden 問題は実行しないと検出できない、`.claude/rules/workflow.md`）

## CSP 注意点

- CSP は `src/auth.rs` の `csp_middleware` で定義
- vendor ライブラリ（restty 等）が WASM・外部フォント等を必要とするため、CSP 変更時は vendor の要件を壊さないこと
- 現在の CSP: `script-src 'self' 'wasm-unsafe-eval'`、`connect-src 'self' ws: wss: https://cdn.jsdelivr.net`
- inline イベントハンドラ（`onclick` 等）は CSP で禁止されている。`addEventListener` を使うこと

## 言語規約

- コミットメッセージ: 英語 (Conventional Commits: feat/fix/chore/refactor/docs/perf/test)
- リリースノート: 英語 (Categories: Features / Fixes / Other)
- コード内コメント: 英語
- CLAUDE.md / .claude/: 日本語OK（開発者向け内部ドキュメント）
