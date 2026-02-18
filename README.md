# Den

iPad mini からブラウザ経由で自宅 Windows PC を操作する個人用ワークステーション。

## Architecture

```
┌──────────────────────────────────────────────────────┐
│  Browser (iPad mini / Desktop)                       │
│  ┌───────────┐  ┌────────────────┐  ┌─────────────┐ │
│  │ Terminal   │  │ Claude Code UI │  │ File Manager│ │
│  │ (xterm.js)│  │ (interactive)  │  │ (CodeMirror)│ │
│  └─────┬─────┘  └───────┬────────┘  └──────┬──────┘ │
└────────┼─────────────────┼──────────────────┼────────┘
         │ WebSocket       │ WebSocket        │ REST API
┌────────┼─────────────────┼──────────────────┼────────┐
│  Axum  │                 │                  │        │
│  ┌─────┴─────┐  ┌───────┴────────┐  ┌──────┴──────┐ │
│  │PTY (shell)│  │PTY (claude CLI)│  │ Filer API   │ │
│  └───────────┘  └────────────────┘  └─────────────┘ │
│  Static files (rust-embed)    SSH Server (russh)     │
│  Store (JSON persistence)     Job Object (ConPTY)    │
└──────────────────────────────────────────────────────┘
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

| Variable | `just dev` | `just prod` | Description |
|----------|-----------|-------------|-------------|
| `DEN_PASSWORD` | `.env` から読込 | `.env` or 引数指定 | Login password **(required)** |
| `DEN_ENV` | `development` | `production` | Environment mode |
| `DEN_PORT` | `3939` | `8080` | Listen port |
| `DEN_BIND_ADDRESS` | `127.0.0.1` | `0.0.0.0` | Bind address |
| `DEN_DATA_DIR` | `./data-dev` | `./data` | Data persistence directory |
| `DEN_LOG_LEVEL` | `debug` | `info` | Log level filter |
| `DEN_SHELL` | `powershell.exe` (Win) / `$SHELL` | same | Shell for terminal |
| `DEN_SSH_PORT` | *(disabled)* | *(disabled)* | SSH server port (opt-in) |

## Features

- **Web Terminal** - xterm.js v6 with touch-friendly keybar (Shift, Ctrl, F1-F12 etc.)
- **Claude Code UI** - interactive mode (persistent process), multi-session, thinking spinner
- **File Manager** - tree view, CodeMirror 6 editor, upload/download, search, image/Markdown preview
- **7 Themes** - Dark, Light, Solarized Dark/Light, Monokai, Nord, System
- **Server-side Persistence** - settings and session history saved to JSON files
- **Authentication** - HMAC-SHA256 token with 24h expiry
- **Built-in SSH Server** - russh-based, password + public key auth, session attach/create
- **Accessibility** - ARIA attributes, focus-visible, keyboard navigation, prefers-reduced-motion
- **Mobile Support** - sidebar toggle, iPad keyboard layout, drag & drop upload

## SSH Server

`DEN_SSH_PORT` を設定すると SSH サーバーが有効になる（opt-in）。

```powershell
$env:DEN_SSH_PORT="2222"
```

接続:

```powershell
# セッション一覧
ssh -p 2222 den@localhost list

# セッションに接続（なければ作成） — -t で PTY 割当が必要
ssh -t -p 2222 den@localhost attach default

# 新規セッション作成
ssh -t -p 2222 den@localhost new mysession
```

- ユーザー名は任意（パスワード認証のみ、`DEN_PASSWORD` と同じ）
- `attach` / `new` は対話セッションなので **`-t`（PTY 割当）が必須**
- 公開鍵認証なしでも `-o PubkeyAuthentication=no` は不要（拒否が即座に完了しパスワードにフォールバック）
- ホストキーは初回起動時に `DEN_DATA_DIR/ssh_host_key` に自動生成

### 公開鍵認証

`DEN_DATA_DIR/ssh/authorized_keys` に公開鍵を配置すると、パスワード不要で接続できる。

```powershell
# 開発環境の場合（just dev → DEN_DATA_DIR=./data-dev）
mkdir ./data-dev/ssh
Add-Content "./data-dev/ssh/authorized_keys" (Get-Content ~/.ssh/id_ed25519.pub)
```

鍵認証が有効な場合、パスワードプロンプトなしで接続される。
鍵が未設定の場合はパスワード認証にフォールバックする。

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
│   ├── store_api.rs        # Settings/Sessions REST API
│   ├── assets.rs           # Static file serving (rust-embed)
│   ├── claude/             # Claude Code integration
│   │   ├── ws.rs           # Claude WebSocket (interactive mode)
│   │   ├── session.rs      # Claude process management
│   │   ├── connection.rs   # SSH connection config
│   │   └── ssh_config.rs   # SSH config parser
│   ├── filer/              # File manager API
│   │   └── api.rs          # Tree, read, write, search, upload, download
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
│   │   ├── claude.js       # Claude Code UI
│   │   ├── claude-parser.js # Streaming JSON parser
│   │   ├── filer.js        # File manager UI
│   │   ├── filer-tree.js   # Tree view component
│   │   ├── filer-editor.js # CodeMirror 6 editor
│   │   ├── keybar.js       # Touch keyboard bar
│   │   ├── settings.js     # Settings modal
│   │   ├── toast.js        # Toast + confirm/prompt modals
│   │   ├── icons.js        # SVG icon module
│   │   └── spinner.js      # Loading spinner
│   ├── css/style.css       # Styles + theme definitions
│   └── vendor/             # xterm.js v6, CodeMirror 6
├── tests/                  # Integration + SSH tests
├── data/                   # Runtime data (gitignored)
└── justfile                # Task runner recipes
```

## Version History

- **v0.1** Web terminal + touch keybar + auth
- **v0.2** Claude Code UI + persistence + security
- **v0.3** File manager (tree + editor + upload/download + search)
- **v0.3.1** iPad keyboard layout + settings path browser + drive list
- **v0.4** Built-in SSH server + SessionRegistry + session persistence
- **v0.4+** UI/UX improvements (themes, accessibility, Claude interactive mode, file preview, performance optimization)
