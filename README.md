# Den

iPad mini からブラウザ経由で自宅 Windows PC を操作する個人用ワークステーション。

## Architecture

```
┌─────────────────────────────────────────┐
│  Browser (iPad mini / Desktop)          │
│  ┌─────────────┐  ┌──────────────────┐  │
│  │  Terminal    │  │  Claude Code UI  │  │
│  │  (xterm.js) │  │  (streaming-json)│  │
│  └──────┬──────┘  └────────┬─────────┘  │
└─────────┼──────────────────┼────────────┘
          │ WebSocket        │ WebSocket
┌─────────┼──────────────────┼────────────┐
│  Axum   │                  │            │
│  ┌──────┴──────┐  ┌───────┴──────────┐  │
│  │  PTY (shell)│  │  PTY (claude CLI)│  │
│  └─────────────┘  └──────────────────┘  │
│  Static files (rust-embed)              │
│  Store (JSON file persistence)          │
└─────────────────────────────────────────┘
```

## Quick Start

[just](https://github.com/casey/just) task runner を使用。

```powershell
cargo install just

# .env にパスワードを設定（初回のみ）
echo 'DEN_PASSWORD=your_password' > .env

# 開発
just dev              # debug ビルド＆起動 (localhost:3939)
just watch            # ホットリロード開発 (cargo-watch)

# 本番
just prod             # release ビルド＆起動 (0.0.0.0:8080)
just prod strongpw    # パスワード上書き指定も可
```

開発ビルドでは `rust-embed` がファイルシステムから直接読むため、`frontend/` の変更はブラウザリロードだけで反映される。

### 全コマンド

| Command | Description |
|---------|-------------|
| `just dev` | 開発ビルド＆起動 |
| `just prod [pw]` | 本番ビルド＆起動 |
| `just watch` | ホットリロード開発 |
| `just check` | fmt + clippy + test |
| `just build` | ビルドのみ |
| `just test` | cargo test |
| `just e2e` | E2E テスト |
| `just fmt` | コード整形 |
| `just ps` | OpenConsole プロセス一覧 |
| `just clean` | ビルド成果物削除 |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DEN_PASSWORD` | **(required)** | Login password |
| `DEN_ENV` | `development` | `development` / `production` |
| `DEN_PORT` | `3939` (dev) / `8080` (prod) | Listen port |
| `DEN_BIND_ADDRESS` | `127.0.0.1` (dev) / `0.0.0.0` (prod) | Bind address |
| `DEN_SHELL` | `cmd.exe` (Win) / `$SHELL` | Shell for terminal |
| `DEN_LOG_LEVEL` | `debug` (dev) / `info` (prod) | Log level filter |
| `DEN_DATA_DIR` | `./data` | Data persistence directory |
| `DEN_SSH_PORT` | *(disabled)* | SSH server port (opt-in) |

## Features

- **Web Terminal** - xterm.js v6 with touch-friendly keybar
- **Claude Code UI** - streaming-json chat, multi-session, SSH support
- **Server-side Persistence** - settings and session history saved to JSON files
- **Authentication** - HMAC-SHA256 token with 24h expiry
- **Built-in SSH Server** - attach to terminal sessions via SSH

## SSH Server

`DEN_SSH_PORT` を設定すると SSH サーバーが有効になる（opt-in）。

```powershell
$env:DEN_SSH_PORT="2222"
```

接続:

```powershell
$SSH_OPTS = @("-o", "PubkeyAuthentication=no", "-o", "IdentityAgent=none")

# セッション一覧
ssh -p 2222 @SSH_OPTS den@localhost list

# セッションに接続（なければ作成） — -t で PTY 割当が必要
ssh -t -p 2222 @SSH_OPTS den@localhost attach default

# 新規セッション作成
ssh -t -p 2222 @SSH_OPTS den@localhost new mysession
```

- ユーザー名は任意（パスワード認証のみ、`DEN_PASSWORD` と同じ）
- `attach` / `new` は対話セッションなので **`-t`（PTY 割当）が必須**
- `-o PubkeyAuthentication=no -o IdentityAgent=none` で SSH agent をバイパス
- ホストキーは初回起動時に `DEN_DATA_DIR/ssh_host_key` に自動生成

## Project Structure

```
den/
├── src/                    # Rust backend
│   ├── lib.rs              # App builder (create_app)
│   ├── main.rs             # Entrypoint
│   ├── config.rs           # Config + Environment
│   ├── auth.rs             # HMAC token auth + middleware
│   ├── store.rs            # JSON file persistence
│   ├── store_api.rs        # Settings/Sessions REST API
│   ├── claude/             # Claude Code integration
│   └── pty/                # PTY management
├── frontend/               # Browser UI
│   ├── js/                 # App modules (IIFE pattern)
│   ├── css/
│   └── vendor/             # xterm.js v6
├── data/                   # Runtime data (gitignored)
└── tests/                  # Integration tests
```

## Version Roadmap

- **v0.1** Web terminal + touch keybar + auth
- **v0.2** Claude Code UI + persistence + security
- **v0.3** File manager (tree + editor + upload/download + search)
