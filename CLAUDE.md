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

## 言語規約

- コミットメッセージ: 英語 (Conventional Commits: feat/fix/chore/refactor/docs/perf/test)
- リリースノート: 英語 (Categories: Features / Fixes / Other)
- コード内コメント: 英語
- CLAUDE.md / .claude/: 日本語OK（開発者向け内部ドキュメント）
