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

## Quick Start (Production)

### PowerShell

```powershell
# ビルド
cargo build --release

# 環境変数を設定して起動
$env:DEN_ENV = "production"
$env:DEN_PASSWORD = "your_password"
cargo run --release
```

ブラウザで `http://localhost:8080` を開く。

> 環境変数はセッション内のみ有効。ターミナルを閉じるとリセットされる。

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

## Features

- **Web Terminal** - xterm.js v6 with touch-friendly keybar
- **Claude Code UI** - streaming-json chat, multi-session, SSH support
- **Server-side Persistence** - settings and session history saved to JSON files
- **Authentication** - HMAC-SHA256 token with 24h expiry

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
- **v0.3** File manager (tree + editor) *(planned)*
