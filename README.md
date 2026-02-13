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
└─────────────────────────────────────────┘
```

## Quick Start

```bash
# Build
cargo build

# Run (development)
DEN_PASSWORD=your_password cargo run

# Run (production)
DEN_ENV=production DEN_PASSWORD=your_password cargo run
```

ブラウザで `http://localhost:8080` (dev) または `http://localhost:3000` (prod) を開く。

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DEN_ENV` | `development` | `development` / `production` |
| `DEN_PORT` | `8080` (dev) / `3000` (prod) | Listen port |
| `DEN_PASSWORD` | `den` | Login password |
| `DEN_SHELL` | `cmd.exe` (Win) / `$SHELL` | Shell for terminal |
| `DEN_LOG_LEVEL` | `debug` (dev) / `info` (prod) | Log level filter |

## Development

### Prerequisites

- Rust (edition 2024)
- Node.js 22+

### Test & Lint

```bash
# Rust
cargo fmt -- --check   # Format check
cargo clippy            # Lint
cargo test              # Unit (~34) + Integration (~11) tests

# Frontend
npm install
npm run lint            # ESLint
npm test                # node:test
npm run check           # lint + test
```

## Project Structure

```
den/
├── src/
│   ├── lib.rs              # App builder (create_app)
│   ├── main.rs             # Entrypoint
│   ├── config.rs           # Config + Environment
│   ├── auth.rs             # Login + token auth middleware
│   ├── assets.rs           # Static file serving (rust-embed)
│   ├── ws.rs               # Terminal WebSocket handler
│   ├── claude/
│   │   ├── ws.rs           # Claude WebSocket handler
│   │   ├── session.rs      # Claude PTY session management
│   │   ├── connection.rs   # Local/SSH connection + directory listing
│   │   └── ssh_config.rs   # SSH config parser
│   ├── pty/
│   │   ├── manager.rs      # PTY spawn + session
│   │   └── session.rs      # PTY session types
│   └── filer/              # v0.3 (planned)
├── frontend/
│   ├── index.html
│   ├── css/style.css
│   ├── js/                 # App modules (IIFE pattern)
│   ├── vendor/             # xterm.js v6
│   └── test/               # Frontend tests
├── tests/
│   └── api_test.rs         # Integration tests
├── rustfmt.toml
├── eslint.config.mjs
└── package.json
```

## Version Roadmap

- **v0.1** Web terminal + touch keybar + auth
- **v0.2** Claude Code UI (streaming-json) + multi-session + SSH
- **v0.3** File manager (tree + editor) *(planned)*
