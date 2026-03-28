---
description: GitHub Issue を1件実装する開発ワークフロー
argument-hint: "Issue #<番号> （auto モード: auto #<番号>）"
allowed-tools: Bash, Read, Write, Edit, Grep, Glob, Task, TaskGet, TaskUpdate, TaskList, TaskCreate, AskUserQuestion, EnterPlanMode
---

# 開発ワークフロー

GitHub Issue を1件実装するフェーズ型ワークフロー。

## 実行モード

引数に `auto` を含む場合（例: `/develop auto #54`）、**auto モード**で実行する。

### 承認モデル

| フェーズ | 通常モード | auto モード |
|---------|-----------|------------|
| Phase 1: 要件確認 | **承認待ち** | 自律実行 |
| Phase 2: 設計 | **承認待ち** | 自律実行 |
| Phase 3〜4: 実装・検証 | 自律実行 | 自律実行 |
| Phase 5: 出荷 | **5-4 のみ承認待ち** | 自律実行 |
| Phase 6: 振り返り | 自律実行 | 自律実行 |

「承認待ち」と明記されていないステップでは人間に判断を求めない。
外部スキル（ai-review 等）を呼び出す場合も、呼び出し元フェーズの承認モデルに従う。

### auto モードの追加ルール

- **push 失敗時**: リトライせずスキップして先に進む（1Password ロック解除等、ユーザー操作が必要な場合がある）
- **ダイジェスト出力**: 各 Phase 完了時に `/tmp/den-develop-<issue番号>-digest.md` に決定事項を追記する（後から参照可能）

### ダイジェストファイルの書式

`/tmp/den-develop-<issue番号>-digest.md` に以下の形式で追記する:

```markdown
# Issue #<番号>: <タイトル>

## Phase 1: 要件確認
- 影響範囲: <変更対象ファイル一覧（簡潔に）>

## Phase 2: 設計
- 方針: <採用したアプローチの要約（1-3行）>

## Phase 4: 検証
- fmt: PASS/FAIL
- clippy: PASS/FAIL
- test: PASS/FAIL
- e2e: PASS/FAIL/SKIP（該当する場合）

## Phase 5: 出荷
- コミット: <コミットメッセージ>
- push: OK/SKIP（理由）
- ai-review findings: 即時修正 N件 / Issue登録 N件 / 見送り N件
- master マージ: OK/SKIP

## Phase 6: 振り返り
- <改善提案があれば記載、なければ「特になし」>
```

## Phase 1: 要件確認

1. 対象 Issue の内容を確認（引数で Issue 番号が渡される場合は `gh issue view` で取得）
2. 関連コードを Glob/Grep/Read で調査
   - `src/` の関連モジュール
   - `frontend/` の関連ファイル
3. 以下をまとめてユーザーに提示:
   - **やること**: 要件の箇条書き
   - **影響範囲**: 変更が必要なファイル一覧
   - **制約・注意点**: 既存機能への影響
   - **不明点**: あれば AskUserQuestion で確認（auto モードでは最善の判断で進む）

→ 通常モード: **承認待ち**
→ auto モード: 影響範囲をダイジェストファイルに書き出し、そのまま Phase 2 へ進む

## Phase 2: 設計

1. 実装アプローチを検討（複数案がある場合は比較表を作成）
2. 以下を設計してユーザーに提示:
   - **データ構造**: 型・構造体の追加/変更
   - **API 設計**: エンドポイント・WebSocket メッセージの追加/変更
   - **フロントエンド**: JS モジュール・UI の変更
   - **ファイル一覧**: 追加・変更するファイルと概要
3. 変更が大きい場合はサブタスクに分割（TaskCreate）

→ 通常モード: **承認待ち**
→ auto モード: 設計方針をダイジェストファイルに書き出し、そのまま Phase 3 へ進む

## Phase 3: 実装

1. ブランチ作成: `feat/<issue番号>-<短い説明>` or `fix/<issue番号>-<短い説明>`
2. 仕様に基づいて段階的に実装
3. 各ステップでビルド確認: `cargo check`
4. コンパイルエラーがあれば即修正（最大3回試行）
5. 実装完了後、変更差分の概要をユーザーに提示

## Phase 4: 検証

checker エージェントで自動検証を実行。

1. Task ツールで checker エージェントを起動
2. 失敗がある場合は修正して再検証（最大3回）
3. フロントエンド変更がある場合は e2e テストも実行:
   ```
   npx playwright test --project=chromium
   ```
   - 今回の変更に起因する失敗は修正する
   - 既存の `test.fixme` テストの失敗は無視してよい

### 検証結果のエビデンス保存

**各コマンドの実行結果（stdout の最終行付近）をダイジェストに引用すること。** 自己申告ではなく、実際の出力をエビデンスとして記録する。

エビデンスの形式（ダイジェストの Phase 4 セクション）:

```markdown
## Phase 4: 検証
- fmt: PASS
  ```
  0 files changed
  ```
- clippy: PASS
  ```
  0 warnings
  ```
- test: PASS
  ```
  test result: ok. 42 passed; 0 failed
  ```
- e2e: PASS
  ```
  5 passed (8.2s)
  ```
```

**禁止事項**:
- 実行せずに PASS と記録してはならない
- エラーで実行できなかった場合は `SKIP（理由）` と記録し、PASS と書いてはならない
- エビデンス（stdout 引用）のないPASS/FAIL は無効とする

### フィードバックループ

| 失敗項目 | 対応 | 上限 |
|---------|------|------|
| cargo fmt | `cargo fmt` で自動修正 | — |
| clippy 警告 | 警告箇所を修正 | 3回 |
| テスト失敗 | 失敗テストを分析して修正 | 3回 |
| E2E 失敗 | 失敗テストのみ再実行して原因特定 | 3回 |

**修正上限に達した場合**: ユーザーに報告してエスカレーション。

### ステータス JSON の書き出し

Phase 4 の全チェック完了後、`/tmp/den-develop-<issue番号>-status.json` に機械可読なステータスを書き出す。loop 等の呼び出し側がパースして検証に使う。

```json
{
  "issue": 54,
  "status": "completed",
  "checks": {
    "fmt":     { "result": "pass", "evidence": "0 files changed" },
    "clippy":  { "result": "pass", "evidence": "0 warnings" },
    "test":    { "result": "pass", "evidence": "test result: ok. 42 passed" },
    "e2e":     { "result": "pass", "evidence": "5 passed (8.2s)" }
  },
  "warnings": []
}
```

**ルール**:
- `result` は `"pass"` / `"fail"` / `"skip"` のいずれか
- `evidence` は実際の stdout 出力（必須。空文字禁止）
- `"skip"` の場合は `"reason"` フィールドを追加
- `status` は全 check が pass なら `"completed"`、skip が 1 つでもあれば `"partial"`、fail があれば `"failed"`
- `warnings` には非致命的な問題を列挙（push スキップ、E2E スキップ等）

## Phase 5: 出荷

Claude 主導で以下を連続実行する。承認待ちはコミット前のみ。

### 5-1: コミット・プッシュ

1. 変更内容に基づいてコミットメッセージを生成（英語、Conventional Commits）
2. 関連ファイルを `git add`（個別指定、`-A` は使わない）
3. コミット（末尾に `Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>`）
4. feature branch にプッシュ: `git push -u origin <ブランチ名>`
   - SSH 失敗時は HTTPS フォールバック: `GH_TOKEN=$(gh auth token) && git push https://<owner>:${GH_TOKEN}@github.com/<owner>/<repo>.git <branch>`

### 5-2: AI レビュー

`/ai-review:review auto` を実行（承認なしでトリアージまで自動完了）。
- AI: `.claude/ai-review.local.md` の設定に従う
- 観点: 全て
- ai-review の修正フェーズは実行しない — 修正判断は 5-3 で行う

### 5-3: Finding 対応判断

`.ai-review/findings.md` を読み、各 Finding を以下の基準で分類・対応する。Claude が自律判断する。

#### 判断基準

各 Finding を「修正の価値」と「放置のリスク」で評価する。
今回の Issue スコープ内外は考慮しない。工数は判断を覆す要因にしない。

| 判断 | 基準 | アクション |
|------|------|-----------|
| **即時修正** | 価値が明確 or 放置リスクあり | その場で修正、再検証、追加コミット |
| **Issue 登録** | 価値はあるが設計判断を伴う | `gh issue create --label ai-review` |
| **見送り** | 価値も放置リスクも低い | findings.md にチェックを入れてスキップ |

#### 判断の指針

- **セキュリティ**（injection, IDOR, 認証バイパス）→ severity によらず即時修正
- **データ整合性・正確性**のリスク → 即時修正を優先
- **可観測性・ログ不足** → 障害調査を阻むなら即時修正
- **パフォーマンス** → ボトルネック顕在化の蓋然性で判断
- **confidence 1 かつ実害の根拠が薄い** → 見送り候補
- Issue 登録時は本文に対応優先度を記載: `next` / `backlog`
- 工数が大きくても価値・リスクが高ければ即時修正する。分割が必要なら最小限の修正を即時 + 残りを Issue 化

#### 即時修正を行った場合

1. 修正コミット追加（`fix: address ai-review findings for ...`）
2. checker エージェントで再検証
3. プッシュ

### 5-4: master マージ

1. `git switch master && git pull origin master`
2. `git merge --squash <feature-branch>`
3. コミットメッセージ作成（Issue タイトル + `Closes #<issue番号>`）
4. `git push origin master`（SSH 失敗時は HTTPS フォールバック）
5. feature branch 削除: `git branch -D <feature-branch> && git push origin --delete <feature-branch>`
   （squash merge は merge commit を作らないため `-d` では「not fully merged」エラーになる）

→ 通常モード: **承認待ち**（マージ内容の最終確認）。承認を得た後にマージ・プッシュを実行する。
→ auto モード: そのままマージ・プッシュを実行。push 失敗時はスキップしてダイジェストに記録。

### 5-5: 対応サマリー

ai-review の対応結果を報告:
- 即時修正: N 件（内容の要約）
- Issue 登録: N 件（Issue URL 一覧）
- 見送り: N 件

### 5-6: ステータス JSON 最終更新

Phase 4 で書き出した `/tmp/den-develop-<issue番号>-status.json` を Read し、トップレベルに以下のフィールドを追加して Write で上書きする:

```json
{
  "push": "ok",
  "ai_review": { "fix": 2, "issue": 0, "dismiss": 1 },
  "merged_to_master": true
}
```

push スキップ、マージスキップ等があれば既存の `warnings` 配列に追加する。

## Phase 6: 振り返り

この Issue の実装を通じてワークフロー・ルールに不都合や改善点を感じたか振り返る。

確認項目:
- /develop のフェーズ構成は適切だったか（冗長 or 不足）
- CLAUDE.md / rules のルールで実態と合わなかったものはないか
- CI / テスト戦略で問題はなかったか
- 次の Issue で活かせる知見はあるか

該当があれば変更提案を提示する。なければ「特になし」で完了。

## エスカレーション

- コンパイル/ビルドエラーは最大 **3回** 修正を試みる
- 3回で解決しない場合はエラー内容をユーザーに報告し判断を仰ぐ
- 仕様の不明点は Phase 中でも AskUserQuestion で即確認する

## 完了条件

- [ ] `cargo fmt` 差分なし
- [ ] `cargo clippy` 警告なし
- [ ] `cargo test` 通過
- [ ] `npx playwright test` 通過（フロントエンド変更がある場合）
- [ ] コミット・プッシュ完了
- [ ] ai-review 完了（Finding 対応済み）
- [ ] 関連 Issue クローズ（該当時）
