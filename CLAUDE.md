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
