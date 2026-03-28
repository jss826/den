---
description: バージョンタグを付けて GitHub Release を作成する
argument-hint: バージョン番号（例: 0.2.0）省略時は自動インクリメント
allowed-tools: Bash, Read, Write, Edit, Grep, Glob
---

バージョンタグを付けて GitHub Release を作成する。
バイナリビルドは CI（`.github/workflows/release.yml`）が自動で行う。

## Phase 1: 準備

1. `git status` で未コミットの変更がないか確認（あればユーザーに報告して停止）
2. タグ確認:
   - `git tag -l --sort=-v:refname` でローカルタグ一覧
3. 既存リリース確認: `gh release list`
4. バージョン番号を決定:
   - 引数ありならそれを使用（`v` prefix 付与: `0.2.0` → `v0.2.0`）
   - 引数なしなら最新タグから変更規模に応じてバージョンを提案
   - タグが無ければ `v0.1.0` から開始
5. 前回タグからの変更一覧を `git log --oneline <前回タグ>..HEAD` で取得
6. リリースノートを生成（英語、カテゴリ分け: Features / Fixes / Other）
7. バージョン番号とリリースノートをユーザーに提示

→ **承認待ち**

## Phase 2: リリース

1. `Cargo.toml` の `version` フィールドを新バージョン番号に更新（`v` prefix なし）
2. `cargo generate-lockfile` で Cargo.lock を更新（Cargo.toml の version 変更を反映）
3. バージョン更新をコミット＆プッシュ: `git add Cargo.toml Cargo.lock && git commit -m "chore: bump version to <version>" && git push`
   - SSH 失敗時は HTTPS フォールバック: `GH_TOKEN=$(gh auth token) && git push https://<owner>:${GH_TOKEN}@github.com/<owner>/<repo>.git <branch>`
   - **push が成功したことを確認してから次のステップへ進む**（失敗した場合、リリース作成でタグが古いコミットを指す）
4. `gh` のアクティブアカウントがリポジトリオーナーと一致することを確認: `gh auth status` でアクティブアカウントを確認し、不一致なら `gh auth switch -u <owner>` で切り替え
5. `gh release create <version> --title "<version>" --notes "<リリースノート>"` で GitHub Release を作成（タグも自動作成される）
6. `git fetch --tags` でリモートタグをローカルに同期（次回リリース時のタグ参照に必要）
7. CI がトリガーされたことを確認: `gh run list --limit 1`
8. CI 完了を待つ: `gh run watch <run_id>`
9. リリースにバイナリが添付されたことを確認: `gh release view <version>`
10. 結果を報告（タグ名 + リリースURL + 含まれるコミット数 + CI ステータス）

## ルール

- 未コミットの変更がある場合は Phase 1 で停止する
- 既存タグと重複する場合はエラーで止まる
- リリースノートは英語で書く (Categories: Features / Fixes / Other)
- `gh` コマンド失敗時はエラー内容を報告して止まる
- バイナリビルドはローカルで行わない（CI に任せる）

## 完了条件

- [ ] Cargo.toml の version が更新・コミットされている
- [ ] GitHub Release が作成された
- [ ] CI が成功し、Windows + Linux バイナリが添付されている
- [ ] リリースURLが報告された
