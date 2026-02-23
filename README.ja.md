# Den

[English](README.md) | **日本語**

タブレットやスマートフォンからアクセスできるセルフホスト型ウェブワークステーション。
内蔵 SSH サーバーにより、デバイス間でターミナルセッションをシームレスに引き継げます。

## 機能

- **Web ターミナル** — xterm.js v6 + タッチ対応キーバー (Shift, Ctrl, F1–F12 等)
- **フローティングターミナル** — ドラッグ＆リサイズ可能なオーバーレイターミナル (Ctrl+\` またはタブバーボタン)
- **ファイルマネージャ** — ツリー表示、CodeMirror 6 エディタ、アップロード/ダウンロード、検索、画像/Markdown プレビュー
- **SFTP リモートファイル** — russh-sftp 経由でリモート SSH ホストに接続し、ファイルを閲覧・編集
- **SSH サーバー内蔵** — russh ベース、パスワード＋公開鍵認証、セッション attach/create
- **12 テーマ** — Dark, Light, Solarized Dark/Light, Monokai, Nord, Dracula, Gruvbox Dark/Light, Catppuccin Mocha, One Dark, System
- **スニペット** — カスタマイズ可能なリストからワンクリックでコマンド入力
- **クリップボード履歴** — システムクリップボード監視による自動追跡
- **認証** — HttpOnly Cookie (HMAC-SHA256 トークン, 24時間有効期限) + レートリミット + CSP
- **サーバーサイド永続化** — 設定とセッション履歴を JSON ファイルに保存
- **アクセシビリティ** — ARIA 属性、focus-visible、キーボードナビゲーション、prefers-reduced-motion
- **モバイル対応** — サイドバートグル、iPad キーボードレイアウト、HTTP LAN 用クリップボードフォールバック

## クイックスタート

[just](https://github.com/casey/just) タスクランナーが必要です。

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

開発ビルドでは `rust-embed` がファイルシステムから直接読むため、`frontend/` の変更はブラウザリロードだけで反映されます。

### 全コマンド

| コマンド | 説明 |
|---------|------|
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

## 環境変数

| 変数 | `just dev` | `just prod` | 説明 |
|------|-----------|-------------|------|
| `DEN_PASSWORD` | `.env` から読込 | `.env` or 引数指定 | ログインパスワード **（必須）** |
| `DEN_ENV` | `development` | `production` | 環境モード |
| `DEN_PORT` | `3939` | `8080` | リッスンポート |
| `DEN_BIND_ADDRESS` | `127.0.0.1` | `0.0.0.0` | バインドアドレス |
| `DEN_DATA_DIR` | `./data-dev` | `./data` | データ永続化ディレクトリ |
| `DEN_LOG_LEVEL` | `debug` | `info` | ログレベル |
| `DEN_SHELL` | `powershell.exe` (Win) / `$SHELL` | 同左 | ターミナルのシェル |
| `DEN_SSH_PORT` | *（無効）* | *（無効）* | SSH サーバーポート（opt-in） |

## SSH サーバー

`DEN_SSH_PORT` を設定すると SSH サーバーが有効になります（opt-in）。

```sh
# Linux/macOS
export DEN_SSH_PORT=2222

# Windows PowerShell
$env:DEN_SSH_PORT="2222"
```

### 接続方法

```sh
# セッション一覧
ssh -p 2222 den@localhost list

# セッションに接続（なければ作成） — -t で PTY 割当が必要
ssh -t -p 2222 den@localhost attach default

# 新規セッション作成
ssh -t -p 2222 den@localhost new mysession
```

- ユーザー名は任意（パスワード認証のみ、`DEN_PASSWORD` と同じ）
- `attach` / `new` は対話セッションなので **`-t`（PTY 割当）が必須**
- ホストキーは初回起動時に `DEN_DATA_DIR/ssh_host_key` に自動生成

### 公開鍵認証

`DEN_DATA_DIR/ssh/authorized_keys` に公開鍵を配置すると、パスワード不要で接続できます。

```sh
# 開発環境の場合（just dev → DEN_DATA_DIR=./data-dev）
mkdir -p ./data-dev/ssh
cat ~/.ssh/id_ed25519.pub >> ./data-dev/ssh/authorized_keys
```

鍵認証が有効な場合、パスワードプロンプトなしで接続されます。
鍵が未設定の場合はパスワード認証にフォールバックします。

## アーキテクチャ

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

## プロジェクト構成

```
den/
├── src/                    # Rust バックエンド
│   ├── lib.rs              # App builder (create_app)
│   ├── main.rs             # エントリポイント
│   ├── config.rs           # 設定 + 環境変数
│   ├── auth.rs             # HMAC トークン認証 + ミドルウェア
│   ├── ws.rs               # ターミナル WebSocket ハンドラ
│   ├── store.rs            # JSON ファイル永続化
│   ├── store_api.rs        # 設定 REST API
│   ├── assets.rs           # 静的ファイル配信 (rust-embed)
│   ├── filer/              # ファイルマネージャ API
│   │   └── api.rs          # ツリー, 読取, 書込, 検索, アップロード, ダウンロード
│   ├── sftp/               # SFTP リモートファイル操作
│   │   ├── api.rs          # 12 SFTP REST エンドポイント
│   │   └── client.rs       # SSH/SFTP 接続マネージャ (russh-sftp)
│   ├── pty/                # PTY 管理
│   │   ├── manager.rs      # PTY 作成 + OpenConsole 検出
│   │   ├── registry.rs     # SessionRegistry (broadcast, ring buffer)
│   │   ├── session.rs      # セッションメタデータ + 永続化
│   │   ├── ring_buffer.rs  # 64KB 出力リングバッファ
│   │   └── job.rs          # Windows Job Object (ゾンビプロセス防止)
│   └── ssh/                # 内蔵 SSH サーバー
│       ├── server.rs       # russh ハンドラ + ターミナル出力フィルタ
│       └── keys.rs         # ホストキー生成 + authorized_keys
├── frontend/               # ブラウザ UI
│   ├── index.html
│   ├── js/                 # アプリモジュール (IIFE パターン)
│   │   ├── app.js          # メインアプリコントローラ
│   │   ├── terminal.js     # xterm.js ターミナル
│   │   ├── filer.js        # ファイルマネージャ UI
│   │   ├── filer-tree.js   # ツリービューコンポーネント
│   │   ├── filer-editor.js # CodeMirror 6 エディタ
│   │   ├── markdown.js     # Markdown レンダラー
│   │   ├── float-terminal.js # フローティングターミナル
│   │   ├── filer-remote.js # SFTP リモート接続 UI
│   │   ├── keybar.js       # タッチキーバー
│   │   ├── settings.js     # 設定モーダル
│   │   ├── toast.js        # Toast + confirm/prompt モーダル
│   │   ├── icons.js        # SVG アイコンモジュール
│   │   ├── spinner.js      # ローディングスピナー
│   │   └── auth.js         # ログイン/ログアウトハンドラ
│   ├── css/style.css       # スタイル + テーマ定義
│   └── vendor/             # xterm.js v6, CodeMirror 6
├── tests/                  # 統合テスト + SSH テスト
├── data/                   # ランタイムデータ (gitignored)
└── justfile                # タスクランナーレシピ
```

## ライセンス

MIT License。詳細は [LICENSE](LICENSE) を参照してください。
