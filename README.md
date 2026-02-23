# Den

**English** | [日本語](README.ja.md)

A self-hosted web workstation accessible from tablets and phones.
Built-in SSH server enables seamless terminal session handoff across devices.

## Features

- **Web Terminal** — xterm.js v6 with touch-friendly keybar (Shift, Ctrl, F1–F12, etc.)
- **Floating Terminal** — draggable/resizable overlay terminal (Ctrl+\` or tab bar button)
- **File Manager** — tree view, CodeMirror 6 editor, upload/download, search, image/Markdown preview
- **SFTP Remote Files** — connect to remote SSH hosts and browse/edit files via russh-sftp
- **Built-in SSH Server** — russh-based, password + public key auth, session attach/create
- **12 Themes** — Dark, Light, Solarized Dark/Light, Monokai, Nord, Dracula, Gruvbox Dark/Light, Catppuccin Mocha, One Dark, System
- **Snippets** — one-click command input from customizable snippet list
- **Clipboard History** — automatic clipboard tracking with system clipboard monitoring
- **Authentication** — HttpOnly Cookie (HMAC-SHA256 token, 24h expiry) + rate limiting + CSP
- **Server-side Persistence** — settings and session history saved to JSON files
- **Accessibility** — ARIA attributes, focus-visible, keyboard navigation, prefers-reduced-motion
- **Mobile Support** — sidebar toggle, iPad keyboard layout, clipboard fallback for HTTP LAN access

## Quick Start

Requires [just](https://github.com/casey/just) task runner.

```sh
cargo install just

# Set password in .env (first time only)
echo 'DEN_PASSWORD=your_password' > .env

# Development
just dev              # debug build & run (localhost:3939)
just watch            # hot-reload development (cargo-watch)

# Production
just prod             # release build & run (0.0.0.0:8080)
just prod strongpw    # override password
```

In development builds, `rust-embed` reads directly from the filesystem — changes to `frontend/` are reflected with a browser reload.

### All Commands

| Command | Description |
|---------|-------------|
| `just dev` | Development build & run |
| `just prod [pw]` | Production build & run |
| `just watch` | Hot-reload development |
| `just check` | fmt + clippy + test |
| `just build` | Build only |
| `just test` | cargo test |
| `just e2e` | E2E tests |
| `just fmt` | Code formatting |
| `just ps` | List OpenConsole processes |
| `just clean` | Clean build artifacts |

## Environment Variables

| Variable | `just dev` | `just prod` | Description |
|----------|-----------|-------------|-------------|
| `DEN_PASSWORD` | from `.env` | `.env` or argument | Login password **(required)** |
| `DEN_ENV` | `development` | `production` | Environment mode |
| `DEN_PORT` | `3939` | `8080` | Listen port |
| `DEN_BIND_ADDRESS` | `127.0.0.1` | `0.0.0.0` | Bind address |
| `DEN_DATA_DIR` | `./data-dev` | `./data` | Data persistence directory |
| `DEN_LOG_LEVEL` | `debug` | `info` | Log level filter |
| `DEN_SHELL` | `powershell.exe` (Win) / `$SHELL` | same | Shell for terminal |
| `DEN_SSH_PORT` | *(disabled)* | *(disabled)* | SSH server port (opt-in) |

## SSH Server

Set `DEN_SSH_PORT` to enable the built-in SSH server (opt-in).

```sh
# Linux/macOS
export DEN_SSH_PORT=2222

# Windows PowerShell
$env:DEN_SSH_PORT="2222"
```

### Usage

```sh
# List sessions
ssh -p 2222 den@localhost list

# Attach to a session (creates if not found) — requires -t for PTY allocation
ssh -t -p 2222 den@localhost attach default

# Create a new session
ssh -t -p 2222 den@localhost new mysession
```

- Username can be anything (password auth only, same as `DEN_PASSWORD`)
- `attach` / `new` are interactive sessions — **`-t` (PTY allocation) is required**
- Host key is auto-generated at `DEN_DATA_DIR/ssh_host_key` on first start

### Public Key Authentication

Place public keys in `DEN_DATA_DIR/ssh/authorized_keys` to enable passwordless login.

```sh
# Example for development (just dev → DEN_DATA_DIR=./data-dev)
mkdir -p ./data-dev/ssh
cat ~/.ssh/id_ed25519.pub >> ./data-dev/ssh/authorized_keys
```

When key auth is configured, password prompts are skipped.
Falls back to password auth when no keys are set up.

## Architecture

```
┌──────────────────────────────────────────────────────┐
│  Browser (iPad mini / Desktop)                        │
│  ┌──────────────────────┐ ┌────────────┐ ┌─────────┐ │
│  │ Terminal + Floating   │ │File Manager│ │  SFTP   │ │
│  │ (xterm.js)            │ │(CM6 + tree)│ │ connect │ │
│  └──────────┬────────────┘ └─────┬──────┘ └────┬────┘ │
└─────────────┼────────────────────┼─────────────┼──────┘
              │ WebSocket          │ REST API     │ REST
┌─────────────┼────────────────────┼─────────────┼──────┐
│  Axum       │                    │             │      │
│  ┌──────────┴──────────┐  ┌──────┴──────┐  ┌──┴────┐ │
│  │ PTY (shell, ConPTY) │  │  Filer API  │  │ SFTP  │ │
│  └─────────────────────┘  └─────────────┘  │ API   │ │
│                                            └──┬────┘ │
│  Static files (rust-embed)              ┌─────┴────┐ │
│  Store (JSON persistence)               │ russh +  │ │
│  SSH Server (russh)                     │ russh-   │ │
│  Job Object (ConPTY cleanup)            │ sftp     │ │
│                                         └──────────┘ │
└──────────────────────────────────────────────────────┘
```

## Project Structure

```
den/
├── src/                    # Rust backend
│   ├── lib.rs              # App builder (create_app)
│   ├── main.rs             # Entrypoint
│   ├── config.rs           # Config + Environment
│   ├── auth.rs             # HMAC token auth + middleware
│   ├── ws.rs               # Terminal WebSocket handler
│   ├── store.rs            # JSON file persistence
│   ├── store_api.rs        # Settings REST API
│   ├── assets.rs           # Static file serving (rust-embed)
│   ├── filer/              # File manager API
│   │   └── api.rs          # Tree, read, write, search, upload, download
│   ├── sftp/               # SFTP remote file operations
│   │   ├── api.rs          # 12 SFTP REST endpoints
│   │   └── client.rs       # SSH/SFTP connection manager (russh-sftp)
│   ├── pty/                # PTY management
│   │   ├── manager.rs      # PTY creation + OpenConsole detection
│   │   ├── registry.rs     # SessionRegistry (broadcast, ring buffer)
│   │   ├── session.rs      # Session metadata + persistence
│   │   ├── ring_buffer.rs  # 64KB output ring buffer
│   │   └── job.rs          # Windows Job Object (zombie prevention)
│   └── ssh/                # Built-in SSH server
│       ├── server.rs       # russh handler + terminal output filter
│       └── keys.rs         # Host key generation + authorized_keys
├── frontend/               # Browser UI
│   ├── index.html
│   ├── js/                 # App modules (IIFE pattern)
│   │   ├── app.js          # Main app controller
│   │   ├── terminal.js     # xterm.js terminal
│   │   ├── filer.js        # File manager UI
│   │   ├── filer-tree.js   # Tree view component
│   │   ├── filer-editor.js # CodeMirror 6 editor
│   │   ├── markdown.js     # Markdown renderer
│   │   ├── float-terminal.js # Floating terminal overlay
│   │   ├── filer-remote.js # SFTP remote connection UI
│   │   ├── keybar.js       # Touch keyboard bar
│   │   ├── settings.js     # Settings modal
│   │   ├── toast.js        # Toast + confirm/prompt modals
│   │   ├── icons.js        # SVG icon module
│   │   ├── spinner.js      # Loading spinner
│   │   └── auth.js         # Login/logout handler
│   ├── css/style.css       # Styles + theme definitions
│   └── vendor/             # xterm.js v6, CodeMirror 6
├── tests/                  # Integration + SSH tests
├── data/                   # Runtime data (gitignored)
└── justfile                # Task runner recipes
```

## License

MIT License. See [LICENSE](LICENSE) for details.

