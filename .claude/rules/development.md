---
paths: ['src/**/*.rs', 'frontend/**', 'tests/**']
---

# 開発ルール

## Rust

- 変更対象ファイルを必ず Read してから編集する
- `unwrap()` は本番コードで使わない（`expect()` またはエラーハンドリング）
- `main.rs` の `unwrap()` は起動時フェイルファストとして許容
- 新しい crate 追加時はユーザーに相談する（勝手に追加しない）

## フロントエンド

- 素の HTML/CSS/JS を維持（フレームワーク導入禁止）
- iPad タッチターゲット 48px 以上
- xterm.js 関連は `frontend/vendor/` に配置
- `[hidden]` 属性で表示制御する要素に `display: flex` 等を指定する場合、`要素[hidden] { display: none; }` を必ず併記する（CSS の `display` 指定が `[hidden]` のデフォルト挙動を上書きするため）
- 新しい IIFE グローバルモジュールを追加したら `eslint.config.mjs` の globals と varsIgnorePattern に登録する

## プロセス管理

- `cargo run` でサーバーを起動した場合、作業完了時に必ずプロセスを停止する
- 停止し忘れるとポート占有やディレクトリロックの原因になる

## PTY テスト（tests/registry_test.rs）

- `#[tokio::test]` は使わない。ConPTY の read パイプは子プロセス終了後も閉じないため、`spawn_blocking` の read_task が永久ブロックし、tokio ランタイムが終了できない
- 代わりに `#[test]` + 手動ランタイム（`build_test_runtime()`）+ `rt.shutdown_timeout(3s)` を使う
- ConPTY は DSR (`ESC[6n`) に CPR 応答がないと出力をブロックする → シェルと対話するテストでは `init_shell()` で CPR を返してからコマンドを送る
- テストが中断（Ctrl+C / taskkill）すると conhost.exe ゾンビが残る → `tasklist //FI "IMAGENAME eq conhost.exe"` で確認し、テスト由来のプロセス（6-7MB）を手動 kill する

## タスク管理

- タスク番号（#54 など）は Claude Code の TaskCreate/TaskUpdate で管理するローカル番号
- GitHub Issues は使用していない。`gh issue` コマンドや GitHub API を叩かないこと

## エスカレーション

- コンパイルエラーは最大3回修正を試みる
- 3回失敗したらユーザーに報告して判断を仰ぐ
