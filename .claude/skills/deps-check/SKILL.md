---
name: deps-check
description: Rust crate と frontend vendor ライブラリのバージョンを確認し、更新推奨を報告する。「依存関係チェック」「ライブラリ更新」「deps update」「outdated」で使用。
---

# deps-check

プロジェクトの依存関係バージョンを確認し、更新推奨レポートを生成する。

## 対象

| カテゴリ | ソース | チェック方法 |
|---------|--------|------------|
| Rust crates | `Cargo.toml` | WebSearch で crates.io の最新版を確認 |
| Frontend vendor | `frontend/vendor/*.js` | WebSearch で npm の最新版を確認 |

## 実行手順

### 1. 現在のバージョン収集

**Rust**: `Cargo.toml` の `[dependencies]` と `[dev-dependencies]` を Read。

**Frontend**: `frontend/vendor/` のファイル一覧を取得。バージョンはファイルヘッダーやコミット履歴から推定。現在のバージョン:
- xterm.js v6 系（`@xterm/xterm`, `@xterm/addon-fit`, `@xterm/addon-canvas`, `@xterm/addon-webgl`）
- CodeMirror v6 系（`codemirror` メタパッケージ）

### 2. 最新バージョン確認

Task(general-purpose) で WebSearch を使い、各ライブラリの最新安定版を並列調査。

確認ポイント:
- メジャーバージョンの変更があるか
- セキュリティアドバイザリがあるか
- 破壊的変更の有無

### 3. レポート生成

以下の形式で報告:

```
## 依存関係レポート

### 要更新（メジャーバージョンアップ）
| ライブラリ | 現在 | 最新 | 変更点 | 移行難度 |
|-----------|------|------|--------|---------|

### 最新（対応不要）
| ライブラリ | 現在 | 最新 |
|-----------|------|------|

### 注意事項
- 互換性制約（例: portable-pty が windows-sys の特定バージョンに依存）
- 破壊的変更の概要
```

### 4. 更新実行（ユーザー承認後）

**Rust crates**:
1. `Cargo.toml` のバージョン指定を更新
2. `cargo update` で Cargo.lock を再生成
3. `cargo check && cargo clippy -- -D warnings && cargo test --target-dir target-test`

**Frontend vendor**:
1. npm/CDN から最新ビルドを取得
2. `frontend/vendor/` のファイルを置換
3. ブラウザ動作確認が必要な旨を案内

## 制約

- メジャーバージョンアップは1つずつ承認を取る
- `portable-pty` と `windows-sys` は互換性を確認してからセットで更新
- vendor JS は minified ファイルのため、ビルド済みバンドルの取得方法を案内
