# Den

**English** | [日本語](README.ja.md)

A self-hosted web workstation accessible from tablets and phones.
Built-in SSH server enables seamless terminal session handoff across devices.

## Features

- **Web Terminal** — xterm.js v6 with touch-friendly keybar (Shift, Ctrl, F1–F12, etc.)
- **File Manager** — tree view, CodeMirror 6 editor, upload/download, search, image/Markdown preview
- **SSH Bookmark Sessions** — one-click SSH terminal creation from saved bookmarks with auto-connect
- **SFTP Remote Files** — connect to remote SSH hosts and browse/edit files via russh-sftp
- **Built-in SSH Server** — russh-based, password + public key auth, session attach/create
- **12 Themes** — Dark, Light, Solarized Dark/Light, Monokai, Nord, Dracula, Gruvbox Dark/Light, Catppuccin Mocha, One Dark, System
- **Text Input** — resizable command input box for mobile/tablet (Ctrl+J), with command history
- **Snippets** — one-click command input from customizable snippet list
- **Clipboard History** — automatic clipboard tracking with system clipboard monitoring where available
- **Quick Connect** — connect to another Den instance's terminal and files through TLS-secured proxy
- **Self-Signed TLS** — optional HTTPS/WSS with auto-generated certificates and fingerprint-based trust
- **Authentication** — HttpOnly Cookie (HMAC-SHA256 token, 24h expiry) + rate limiting + CSP
- **Self-Update** — check for updates and apply from the Settings panel (downloads from GitHub Releases)
- **Session Persistence** — terminal sessions survive restarts; SSH bookmark sessions auto-reconnect
- **Session Tab Reordering** — drag-and-drop to reorder terminal session tabs, order persisted server-side
- **Server-side Persistence** — settings and session history saved to JSON files
- **Accessibility** — ARIA attributes, focus-visible, keyboard navigation, prefers-reduced-motion
- **Mobile Support** — sidebar toggle, iPad keyboard layout, clipboard fallback for HTTP LAN access

## Install

Single binary, no dependencies.

### Quick Install

**Linux / macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/jss826/den/master/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/jss826/den/master/install.ps1 | iex
```

Then run:

```bash
DEN_PASSWORD=your_password den
```

### Manual Install

<details>
<summary>Windows</summary>

```powershell
curl -Lo den.zip https://github.com/jss826/den/releases/latest/download/den-x86_64-pc-windows-msvc.zip
Expand-Archive den.zip -DestinationPath . ; Remove-Item den.zip

$env:DEN_PASSWORD="your_password"
.\den.exe
```

</details>

<details>
<summary>Linux</summary>

```bash
curl -Lo den.tar.gz https://github.com/jss826/den/releases/latest/download/den-x86_64-unknown-linux-gnu.tar.gz
tar xzf den.tar.gz && rm den.tar.gz && chmod +x den

DEN_PASSWORD=your_password ./den
```

</details>

Open `http://localhost:3939` in your browser.

> **Tip:** Set `DEN_PASSWORD` in a `.env` file to avoid typing it every time.
>
> | Platform | `.env` location | Data directory |
> |----------|-----------------|----------------|
> | Windows | `%LOCALAPPDATA%\den\.env` | `%LOCALAPPDATA%\den\data\` |
> | Linux / macOS | `~/.config/den/.env` | `~/.local/share/den/` |
>
> Override with `DEN_DATA_DIR` environment variable.

### Development (with just)

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
| `DEN_DATA_DIR` | `./data-dev` | *(see below)* | Data persistence directory |
| `DEN_LOG_LEVEL` | `debug` | `info` | Log level filter |
| `DEN_SHELL` | `powershell.exe` (Win) / `$SHELL` | same | Shell for terminal |
| `DEN_SSH_PORT` | *(disabled)* | *(disabled)* | SSH server port (opt-in) |
| `DEN_TLS` | `false` | `false` | Enable HTTPS/WSS (`1`, `true`, `yes`, `on`) |
| `DEN_TLS_CERT_PATH` | *(auto-generate)* | *(auto-generate)* | Server certificate path (DER) |
| `DEN_TLS_KEY_PATH` | *(auto-generate)* | *(auto-generate)* | Private key path (PKCS#8 DER) |
| `DEN_TLS_SAN` | *(none)* | *(none)* | Subject Alternative Names (comma-separated) |

When `DEN_DATA_DIR` is not set, the default depends on the platform:
- **Windows:** `<exe directory>\data` (e.g. `%LOCALAPPDATA%\den\data`)
- **Linux / macOS:** `$XDG_DATA_HOME/den` (default `~/.local/share/den`)

## TLS

Set `DEN_TLS=true` to serve over HTTPS/WSS. If no certificate is provided, a self-signed certificate is auto-generated in `DEN_DATA_DIR/tls/`.

```powershell
$env:DEN_TLS="true"
$env:DEN_TLS_SAN="den-a,10.0.0.2"  # optional SANs for the self-signed cert
```

The server's TLS fingerprint is shown in Settings. When connecting to a remote Den, the fingerprint is presented for confirmation on first use (trust-on-first-use model). A fingerprint change triggers a warning.

## Quick Connect

Connect to another Den instance's terminal and files from your browser. Requires TLS on the remote Den.

1. Open the **Remote** dropdown in the file manager (or session bar)
2. Select **Quick Connect Den**
3. Enter the remote Den's URL and password
4. Confirm the TLS fingerprint on first connection
5. Terminal sessions and files from the remote Den appear alongside local ones

All connections are proxied through the local Den — your browser only talks to localhost.

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
- Host key is auto-generated at `DEN_DATA_DIR/ssh_host_key` on first start (no user action needed — deleting it will trigger host key warnings on clients)

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
┌──────────────────────────────────────────────────────────┐
│  Browser (iPad mini / Desktop)                            │
│  ┌──────────────────────┐ ┌────────────┐ ┌─────────────┐ │
│  │ Terminal + Floating   │ │File Manager│ │ SFTP / Quick│ │
│  │ (xterm.js)            │ │(CM6 + tree)│ │   Connect   │ │
│  └──────────┬────────────┘ └─────┬──────┘ └──────┬──────┘ │
└─────────────┼────────────────────┼───────────────┼────────┘
              │ WebSocket          │ REST API       │ REST
┌─────────────┼────────────────────┼───────────────┼────────┐
│  Axum (HTTP or HTTPS/WSS)        │               │        │
│  ┌──────────┴──────────┐  ┌──────┴──────┐  ┌────┴──────┐ │
│  │ PTY (shell, ConPTY) │  │  Filer API  │  │ SFTP API  │ │
│  └─────────────────────┘  └─────────────┘  └───────────┘ │
│  ┌──────────────────────────────────────────────────────┐ │
│  │ Quick Connect  →  Remote Den (HTTPS proxy)            │ │
│  │ (terminal + filer + WS proxy)                        │ │
│  └──────────────────────────────────────────────────────┘ │
│  Static files (rust-embed)    TLS (self-signed / custom)  │
│  Store (JSON persistence)     SSH Server (russh)          │
│  Job Object (ConPTY cleanup)  SFTP Client (russh-sftp)    │
└──────────────────────────────────────────────────────────┘
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
│   ├── remote.rs           # Quick Connect proxy (terminal, filer, WS)
│   ├── tls.rs              # TLS setup, fingerprint trust API
│   ├── update.rs           # Self-update from GitHub Releases
│   ├── clipboard_api.rs    # Clipboard REST API
│   ├── clipboard_monitor.rs # System clipboard monitoring
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
│       ├── keys.rs         # Host key generation + authorized_keys
│       └── loopback.rs     # SSH self-connection detection
├── frontend/               # Browser UI
│   ├── index.html
│   ├── js/                 # App modules (IIFE pattern)
│   │   ├── app.js          # Main app controller
│   │   ├── terminal.js     # xterm.js terminal
│   │   ├── filer.js        # File manager UI
│   │   ├── filer-tree.js   # Tree view component
│   │   ├── filer-editor.js # CodeMirror 6 editor
│   │   ├── markdown.js     # Markdown renderer
│   │   ├── filer-remote.js # SFTP remote connection UI
│   │   ├── keybar.js       # Touch keyboard bar
│   │   ├── settings.js     # Settings modal
│   │   ├── text-input.js   # Mobile-friendly command input box
│   │   ├── tls-trust.js    # TLS fingerprint trust UI
│   │   ├── snippet.js      # Snippet manager
│   │   ├── clipboard.js    # Clipboard utilities
│   │   ├── clipboard-history.js # Clipboard history UI
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

