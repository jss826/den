# Den - CLAUDE.md

## 概要
iPad mini からブラウザ経由で自宅 Windows PC を操作する個人用ワークステーション。

## 技術スタック
- バックエンド: Rust (axum + portable-pty + tokio)
- フロントエンド: 素の HTML/CSS/JS + xterm.js v6
- 静的ファイル: rust-embed でバイナリ埋め込み

## ビルド & 実行
```bash
cargo build
DEN_PASSWORD=your_password cargo run
```

## 環境変数
| 変数 | デフォルト | 説明 |
|------|-----------|------|
| DEN_PORT | 8080 | リッスンポート |
| DEN_PASSWORD | den | ログインパスワード |
| DEN_SHELL | cmd.exe (Win) / $SHELL | シェル |

## バージョン計画
- v0.1: Web ターミナル + タッチキーバー + 認証
- v0.2: Claude Code 専用 UI (streaming-json)
- v0.3: ファイラ (ツリー + エディタ)
