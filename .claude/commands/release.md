---
description: バージョンタグを付けて GitHub Release を作成する
argument-hint: バージョン番号（例: 0.2.0）省略時は自動インクリメント
allowed-tools: Bash, Read, Write, Edit, Grep, Glob
---

バージョンタグを付けて GitHub Release を作成する。

## Phase 1: 準備

1. `git status` で未コミットの変更がないか確認（あればユーザーに報告して停止）
2. タグ確認（ローカル＋リモート両方）:
   - `git fetch --tags` でリモートタグを同期
   - `git tag -l --sort=-v:refname` で一覧表示
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
2. バージョン更新をコミット＆プッシュ: `git add Cargo.toml Cargo.lock && git commit -m "chore: bump version to <version>" && git push`
3. タグを作成してプッシュ: `git tag <version> && git push origin <version>`
4. `gh release create <version> --title "<version>" --notes "<リリースノート>"` で GitHub Release を作成
5. `git fetch --tags` でリモートタグをローカルに同期
6. 結果を報告（タグ名 + リリースURL + 含まれるコミット数）

## ルール

- 未コミットの変更がある場合は Phase 1 で停止する
- 既存タグと重複する場合はエラーで止まる
- リリースノートは英語で書く (Categories: Features / Fixes / Other)
- `gh` コマンド失敗時はエラー内容を報告して止まる

## 完了条件

- [ ] Cargo.toml の version が更新・コミットされている
- [ ] GitHub Release が作成された
- [ ] ローカルタグがリモートと同期されている
- [ ] リリースURLが報告された
