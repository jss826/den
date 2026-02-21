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
