# Den 開発フローを flow 前提にする Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Den の標準開発入口を `/flow` に一本化し、flow の実装エンジンが Den 固有事項（4 スロット）を指示ファイルから拾える状態にする。

**Architecture:** コード非変更。CLAUDE.md に flow の 4 スロット + master/code-review 規約を新設し、`/develop` コマンド本体を削除、関連参照（workflow.md / memory / review-judgement.md）を flow 前提に更新、不用資産（ai-review.local.md / .orch-develop/）を削除する。

**Tech Stack:** Markdown（CLAUDE.md / `.claude/rules/` / memory）、`.gitignore`、Git。

参照 spec: `docs/superpowers/specs/2026-06-13-flow-development-workflow-design.md`

---

## File Structure

| ファイル | 操作 | 責務 |
|---|---|---|
| `CLAUDE.md` | Modify | flow の 4 スロット + master/code-review 規約を定義（flow が読む中核） |
| `.claude/commands/develop.md` | Delete | `/develop` スキル本体を廃止 |
| `.claude/rules/workflow.md` | Modify | `/develop` 参照を flow に更新 |
| `.claude/rules/review-judgement.md` | Modify | 例示をツール非依存（/code-review）に修正 |
| `.claude/ai-review.local.md` | Delete | `/code-review` 一本化で未使用化 |
| `.orch-develop/` | Delete | 過去の orch:develop 実行ゴミ（untracked） |
| `.gitignore` | Modify | `.orch-develop/` 再発防止 |
| `MEMORY.md`（user auto-memory） | Modify | `/develop`・`/orch:develop` 前提を flow 前提に書き換え |

> 注: 全タスク完了まで **コミットしない**。ブランチ/コミット方針は実行完了後にユーザーへ確認する（spec の方針どおり）。各タスクの「Commit」ステップは保留扱いとし、最終 Task でまとめて扱う。

---

## Task 1: CLAUDE.md に「開発フロー（flow 前提）」セクションを新設

**Files:**
- Modify: `CLAUDE.md`（`## テスト` セクション直後、`## 技術スタック` の前に挿入）

- [ ] **Step 1: 挿入位置を確認**

Read `CLAUDE.md` の冒頭〜30 行。`## テスト` セクションは以下で終わる:

```
**`--target-dir target-test`**: dev サーバー実行中でもバイナリロックを回避してテスト可能。
```

その直後に `## 技術スタック` が来る。この 2 つの間に新セクションを挿入する。

- [ ] **Step 2: flow セクションを挿入**

`CLAUDE.md` の以下の行（`## テスト` の末尾）:

```markdown
**`--target-dir target-test`**: dev サーバー実行中でもバイナリロックを回避してテスト可能。

## 技術スタック
```

を、次に置き換える:

```markdown
**`--target-dir target-test`**: dev サーバー実行中でもバイナリロックを回避してテスト可能。

## 開発フロー（flow 前提）

Den の標準開発入口は **`/flow`**。

- `/flow <やりたいこと>` — 司会者層（受付→コンサル→分類→委譲→報告）
- `/flow #N` — Issue を 1 件実装（対話モード、7 フェーズ）
- `/flow auto #N` — headless 実装（`/loop` 用、status JSON 出力）

flow の実装エンジンはプロジェクト固有事項を以下の **4 スロット**から読む。詳細は各 rules を参照。

| スロット | Den の内容 |
|---|---|
| ① 品質ゲート | `cargo fmt -- --check` / `cargo clippy -- -D warnings` / `cargo test --target-dir target-test` /（UI 変更時）`npx playwright test tests/e2e/filer-ui.e2e.ts` |
| ② security-review 対象差分 | `src/auth.rs`・`src/tls.rs`・`src/remote.rs` 等、認証/セッション/トークン/TLS 境界に触れる差分は `/security-review` を併用 |
| ③ 実行上の罠 | `DEN_DATA_DIR=./data-dev` 厳守（`./data` 上書き禁止）/ `--target-dir target-test` でロック回避 / 長時間コマンドは background 実行 / ConPTY conhost ゾンビの後始末 / PTY テストは `#[tokio::test]` 禁止（`.claude/rules/development.md`）/ 本番 :3939 と並行時は別ポート |
| ④ フェーズ内追加チェック | 設計時=`frontend/DESIGN.md` が UI の正（先に更新）/ UI 変更時=e2e 必須＋vendor bump・adapter 修正なら renderer 切替スモーク（`.claude/rules/workflow.md`） |

**flow デフォルト（`main`）の Den 向け上書き:**

- ブランチ命名: `feat|fix|chore/<N>-<説明>`
- マージ先 = **`master`**（`main` ではない）、**squash merge**
- Phase 6 コードレビュー = **`/code-review`**（effort: 軽微 medium / 通常 high / 本丸 max）。finding の対応判断は `.claude/rules/review-judgement.md`
- リリース（tag + GitHub Release）は flow 範囲外 → **`/release`**
- Issue 外の単発コミット → **`/ship`**

## 技術スタック
```

- [ ] **Step 3: 検証**

Grep で挿入を確認する。

Run: Grep `pattern="開発フロー（flow 前提）"` `path="CLAUDE.md"` `output_mode="content"`
Expected: 1 件ヒット（`## 開発フロー（flow 前提）`）

Run: Grep `pattern="マージ先 = \\*\\*\`master\`"` `path="CLAUDE.md"` `output_mode="files_with_matches"`
Expected: `CLAUDE.md` がヒット（master 上書き規約が入っている）

---

## Task 2: `/develop` コマンド本体を削除

**Files:**
- Delete: `.claude/commands/develop.md`

- [ ] **Step 1: 削除**

Run: `git rm .claude/commands/develop.md`
Expected: `rm '.claude/commands/develop.md'`（ステージ済み削除）

- [ ] **Step 2: 検証**

Run: Glob `pattern=".claude/commands/*.md"`
Expected: `ship.md` と `release.md` のみ（`develop.md` が消えている）

---

## Task 3: `.claude/rules/workflow.md` の `/develop` 参照を更新

**Files:**
- Modify: `.claude/rules/workflow.md:5`, `.claude/rules/workflow.md:39`

- [ ] **Step 1: L5 の定義済みワークフロー列挙を更新**

`.claude/rules/workflow.md` の以下:

```markdown
定義済みワークフロー（/develop, /ship, CI, Issue 駆動, 品質ゲート等）は忠実に従う。
```

を:

```markdown
定義済みワークフロー（/flow, /ship, /release, CI, Issue 駆動, 品質ゲート等）は忠実に従う。
```

- [ ] **Step 2: L39 の具体例を flow の Phase 表現に差し替え**

`.claude/rules/workflow.md` の以下:

```markdown
- /develop の Phase が冗長に感じる → 「Phase 2 と 3 を統合したい。小さい Issue では設計と実装を分ける必要がないため」
```

を:

```markdown
- /flow の Phase が冗長に感じる → 「Phase 2（設計）と 3（実装）を統合したい。小さい Issue では設計と実装を分ける必要がないため」
```

- [ ] **Step 3: 検証**

Run: Grep `pattern="/develop"` `path=".claude/rules/workflow.md"` `output_mode="count"`
Expected: 0 件（`/develop` が残っていない）

Run: Grep `pattern="/flow"` `path=".claude/rules/workflow.md"` `output_mode="count"`
Expected: 2 件以上

---

## Task 4: `.claude/rules/review-judgement.md` をツール非依存に修正

**Files:**
- Modify: `.claude/rules/review-judgement.md:3`

- [ ] **Step 1: 例示を /code-review に修正**

`.claude/rules/review-judgement.md` の以下:

```markdown
コードレビュー（ai-review、PR レビュー等）の指摘対応は「修正の価値」と「放置のリスク」で判断する。
```

を:

```markdown
コードレビュー（`/code-review`、PR レビュー等）の指摘対応は「修正の価値」と「放置のリスク」で判断する。
```

- [ ] **Step 2: 検証**

Run: Grep `pattern="ai-review"` `path=".claude/rules/review-judgement.md"` `output_mode="count"`
Expected: 0 件

---

## Task 5: 不用資産を削除（ai-review.local.md / .orch-develop/）

**Files:**
- Delete: `.claude/ai-review.local.md`
- Delete: `.orch-develop/`（untracked ディレクトリ）

- [ ] **Step 1: ai-review.local.md を削除**

Run: `git rm .claude/ai-review.local.md`
Expected: `rm '.claude/ai-review.local.md'`

- [ ] **Step 2: .orch-develop/ を削除（untracked なので rm）**

Run: `rm -rf .orch-develop`
Expected: 出力なし（成功）

- [ ] **Step 3: 検証**

Run: Glob `pattern=".claude/ai-review.local.md"`
Expected: ヒットなし

Run: `git status --porcelain .orch-develop`
Expected: 出力なし（追跡対象でも作業ツリーにも存在しない）

---

## Task 6: `.gitignore` に `.orch-develop/` を追加（再発防止）

**Files:**
- Modify: `.gitignore`

- [ ] **Step 1: 既存の `.gitignore` に `.orch-develop/` があるか確認**

Run: Grep `pattern="orch-develop"` `path=".gitignore"` `output_mode="count"`
Expected: 0 件なら Step 2 へ。1 件以上なら Task 6 をスキップ。

- [ ] **Step 2: `.gitignore` 末尾に追記**

`.gitignore` の末尾に以下の行を追加する（Edit で既存末尾行の後ろに足す。`Read` で現末尾を確認してから追記）:

```
# orch:develop の作業ゴミ（flow 移行で廃止）
.orch-develop/
```

- [ ] **Step 3: 検証**

Run: Grep `pattern="\\.orch-develop/"` `path=".gitignore"` `output_mode="count"`
Expected: 1 件

---

## Task 7: memory (MEMORY.md) を flow 前提に書き換え

**Files:**
- Modify: `C:\Users\soon7\.claude\projects\D--Documents-git-den\memory\MEMORY.md`

> memory はリポジトリ外（user auto-memory）。git 管理対象ではないため commit 不要。

- [ ] **Step 1: 現状の develop/orch 記述を把握**

Run: Grep `pattern="/develop|orch:develop|orch-develop"` `path="C:\\Users\\soon7\\.claude\\projects\\D--Documents-git-den\\memory\\MEMORY.md"` `output_mode="content"` `-n=true`
Expected: 「セッション引き継ぎ」セクション周辺で複数ヒット（大規模削除 → orch:develop sequential パターン等）。

- [ ] **Step 2: flow 前提に書き換え**

ヒットした各箇所を以下の方針で Edit:
- 過去の実績記述（「v3.4.0 を /orch:develop sequential で実装した」等）は **履歴として残す**（事実なので改変しない）。
- 「今後の開発フロー」を述べている箇所・前提としている箇所は **`/flow` に置換**。
- 「未完了・次にやること」等に、次の 1 行を追記する:

```markdown
- **[開発フロー変更]** 2026-06-13 に開発フローを `/flow` に一本化（`/develop` 廃止、Phase 6 は `/code-review`）。今後の Issue 実装は `/flow #N`、headless は `/flow auto #N` を `/loop` で回す。詳細: `docs/superpowers/specs/2026-06-13-flow-development-workflow-design.md`
```

- [ ] **Step 3: 検証**

Run: Grep `pattern="/flow"` `path="C:\\Users\\soon7\\.claude\\projects\\D--Documents-git-den\\memory\\MEMORY.md"` `output_mode="count"`
Expected: 1 件以上（flow 前提の追記が入っている）

---

## Task 8: 全体整合チェックとコミット方針確認

**Files:** なし（検証のみ）

- [ ] **Step 1: リポジトリ全体に `/develop` の生き残り参照がないか確認**

Run: Grep `pattern="/develop\\b|orch:develop"` `glob="*.md"` `output_mode="content"` `-n=true`
Expected: ヒットは `docs/superpowers/specs/...` と `docs/superpowers/plans/...`（本 spec/plan 自身が history として言及）のみ。`.claude/` 配下・`CLAUDE.md` にヒットが無いこと。

- [ ] **Step 2: ai-review 残存参照の確認**

Run: Grep `pattern="ai-review"` `glob="*.md"` `output_mode="files_with_matches"`
Expected: spec/plan ドキュメント以外でヒットしないこと（`.claude/ai-review.local.md` 削除済み、review-judgement.md 修正済み）。

- [ ] **Step 3: 変更サマリーを提示しコミット方針を確認**

Run: `git status --short`
変更一覧（CLAUDE.md / develop.md 削除 / workflow.md / review-judgement.md / ai-review.local.md 削除 / .gitignore / spec / plan）をユーザーに提示し、以下を確認:
- master 直コミット or ブランチを切る（spec の方針: markdown のみなので master 直も可）
- コミットメッセージ案（英語 / Conventional Commits）:
  ```
  docs: adopt flow as the canonical dev workflow, retire /develop
  ```

→ **承認待ち**（コミット/プッシュはユーザー依頼時のみ実行）。

---

## Self-Review メモ

- **Spec coverage:** 変更 1（CLAUDE.md 4 スロット）=Task 1、変更 2（/develop 廃止 + 参照更新）=Task 2/3/7、変更 3（ai-review 整理）=Task 4/5、変更 4（.orch-develop 後始末）=Task 5/6。やらないこと（/ship・/release・flow 本体・CI）はタスク化せず。全カバー。
- **Placeholder scan:** 各 Edit に before/after の実テキストを明記。プレースホルダなし。
- **Type consistency:** ファイルパス・行番号・Grep パターンは現行ファイルの実テキストに基づく。`master` / `/code-review` / `/flow` の表記をプラン全体で統一。
