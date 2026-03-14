---
description: 変更をコミットしてプッシュする
argument-hint: 省略可（特定ファイルのみコミットしたい場合にパス指定）
allowed-tools: Bash, Read, Grep, Glob
---

変更をコミットしてプッシュする。

## Phase 1: 確認

1. `git status` と `git diff --stat` で変更内容を確認
2. `git log --oneline -3` でコミットメッセージのスタイルを確認
3. untracked ファイルにゴミ（`nul`, `.DS_Store` 等）があれば警告する
4. 変更内容に基づいてコミットメッセージを生成（英語、Conventional Commits format: feat/fix/chore/docs/refactor/perf/test）
5. コミット対象ファイルとメッセージをユーザーに提示

→ **承認待ち**

## Phase 2: 実行

1. 関連ファイルを `git add`（個別指定、`-A` は使わない）
2. コミット（末尾に `Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>`）
3. `git push`
4. `git status` でワーキングツリーがクリーンか確認
5. 結果を報告（コミットハッシュ + 変更概要）

## Phase 3: Issue クローズ（該当時）

1. コミットメッセージ・会話コンテキストから関連 Issue 番号を推定
2. 該当 Issue があれば `gh issue view <num> --json state` で状態確認
3. OPEN なら「Issue #N をクローズしますか？」と確認

→ **承認待ち**

4. 承認後 `gh issue close <num>`

## ルール

- .env や credentials 系ファイルはコミットしない
- target/ ディレクトリはコミットしない
- プッシュ失敗時はエラー内容を報告してユーザーに対処方法を確認する（勝手にフォールバックしない）

## 完了条件

- [ ] コミットが作成された
- [ ] リモートにプッシュされた
