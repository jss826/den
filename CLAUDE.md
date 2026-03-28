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

## CSS 注意点

- `.pane` の子要素（`#terminal-pane` 等）に ID セレクタで `display: flex` を指定しているため、`hidden` 属性が効かない。`#terminal-pane[hidden] { display: none }` で明示的にオーバーライドが必要
- `#filer-pane` は `display: flex` を追加してはいけない。absolute positioning + `.filer-layout { height: 100% }` で正しく動作している
- E2E テスト（Playwright）で CSS レイアウト変更を必ず検証すること: `npx playwright test tests/e2e/filer-ui.e2e.ts`

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
