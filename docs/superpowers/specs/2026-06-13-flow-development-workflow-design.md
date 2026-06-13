# Den 開発フローを flow 前提にする — 設計

- 日付: 2026-06-13
- ステータス: 承認待ち（spec レビュー）
- 種別: 開発ワークフロー / 指示ファイルのリファクタ（コード非変更）

## 背景

Den には現在、独自の `/develop`（Den 専用・6 フェーズ実装コマンド）、`/ship`、`/release`、
および過去に使った `/orch:develop` を前提とした記述（`.claude/rules/workflow.md`・memory）が混在している。

一方、汎用の `flow` スキル（`ccplugins/flow`）は「司会者層（Phase 0-4）+ 実装エンジン（Phase 1-7）」
で構成され、**プロジェクト固有事項を指示ファイル（CLAUDE.md / `.claude/rules/`）に 4 スロットで委譲**する設計になっている。
flow の実装エンジンは `/develop` の上位互換であり、Den 固有事項を指示ファイル側で吸収できる。

本変更で Den の標準開発入口を `/flow` に一本化する。

## ゴール（検証可能）

- Den の標準開発入口が `/flow`（`/flow <やりたいこと>` / `/flow #N` / `/flow auto #N`）になる。
- flow の実装エンジンが Den 固有事項（品質ゲート・実行上の罠・レビュー・master 運用）を
  指示ファイルから正しく拾える。
- Den 専用 `/develop`・`/orch:develop` 参照が排除され、矛盾する記述が残らない。
- 完了基準: `/flow #<実在 Issue>` を 1 件流し、Phase 1-7 が
  Den の cargo ゲート（`--target-dir target-test`）・master squash merge・`/code-review` で完走できる状態。

## 決定事項

| 論点 | 決定 |
|---|---|
| `/develop` の扱い | flow に一本化（`/develop` 廃止） |
| Phase 6 レビューツール | flow ネイティブ `/code-review` に合わせる |
| memory (MEMORY.md) | 本作業で flow 前提に書き換える |
| 不用資産 | `.claude/ai-review.local.md` と `.orch-develop/` を削除 |
| マージ先 | `master`（flow デフォルトの `main` を指示ファイルで上書き）、squash merge |
| flow プラグイン本体 | 編集しない（Den 側は指示ファイルで override） |

## 変更内容

### 変更 1: CLAUDE.md に「開発フロー（flow 前提）」セクションを新設（中核）

flow の実装エンジンが開始時に読む **4 スロット**を、常時ロードされる CLAUDE.md に明示する。
詳細は既存 rules（`development.md` 等）へポインタで委譲する。

| スロット | Den の内容 |
|---|---|
| ① 品質ゲート | `cargo fmt -- --check` / `cargo clippy -- -D warnings` / `cargo test --target-dir target-test` /（UI 変更時）`npx playwright test tests/e2e/filer-ui.e2e.ts` |
| ② security-review 対象差分 | `src/auth.rs`・`src/tls.rs`・`src/remote.rs` 等、認証/セッション/トークン/TLS 境界に触れる差分 |
| ③ 実行上の罠 | `DEN_DATA_DIR=./data-dev` 厳守・`./data` 上書き禁止 / `--target-dir target-test`（dev サーバー実行中のロック回避） / 長時間コマンドは background 実行 / ConPTY conhost ゾンビの後始末 / PTY テストは `#[tokio::test]` 禁止 / 本番 :3939 と並行時は別ポート |
| ④ フェーズ内追加チェック | 設計時=`frontend/DESIGN.md` が UI の正（先に更新）/ UI 変更時=e2e 必須＋vendor bump・adapter 修正なら renderer 切替スモーク |

加えて flow のデフォルトを上書きする規約を明記する:

- ブランチ命名: `feat|fix|chore/<N>-<説明>`
- マージ先 = **master**（main ではない）、**squash merge**
- Phase 6 レビュー = **`/code-review`**（effort: 軽微 medium / 通常 high / 本丸 max）
- リリースは flow 範囲外 → `/release`、Issue 外の単発コミット → `/ship`

### 変更 2: `/develop` 廃止と参照更新

- 削除: `.claude/commands/develop.md`（= `/develop` スキル本体）。
- `.claude/rules/workflow.md` 更新:
  - L5 `定義済みワークフロー（/develop, /ship, CI, ...）` → `（/flow, /ship, /release, CI, ...）`
  - L39 `/develop の Phase が冗長…` の具体例 → flow の Phase 表現に差し替え。
  - Phase 4（検証）厳守・renderer 切替スモークの記述は**維持**（flow のスロット③/④が参照する Den 固有知見）。
- memory `MEMORY.md` 更新: `/orch:develop`・`/develop` 前提の記述を flow 前提に書き換え。
  「大規模削除→orch:develop sequential」パターンは履歴として残し、「今後は `/flow auto #N` を `/loop` で回す」と追記。

### 変更 3: ai-review 資産の整理

- 削除: `.claude/ai-review.local.md`（`/code-review` 一本化で未使用化）。
- `.claude/rules/review-judgement.md`: 判断基準（価値×リスク、スコープ内外を問わない等）は維持。
  冒頭の「ai-review、PR レビュー等」の例示のみ「`/code-review` 等のコードレビュー」に修正してツール非依存にする。
- flow の Phase 6-3（finding を価値×リスクで分類）は `review-judgement.md` と整合する。

### 変更 4: `.orch-develop/` の後始末

- 削除: `.orch-develop/`（過去の orch:develop 実行の未追跡ゴミ。untracked なので git 履歴に影響なし）。
- `.gitignore` に `.orch-develop/` が無ければ追加（再発防止）。
  flow の成果物（`/tmp/<repo>-flow-*`）は `/tmp` 配下なので gitignore 不要。

## やらないこと（スコープ外）

- `/ship`・`/release` のロジック変更（flow と役割分担済み、そのまま残す）。
- flow プラグイン本体（`ccplugins/flow`）の編集（プラグインはソースリポジトリで編集する規約）。
- CI 設定・cargo/playwright の実体変更。

## 影響・移行リスク

- 低。コード非変更（markdown / 設定のみ）。
- flow は `disable-model-invocation: true` のため自動暴発なし、`/flow` 明示起動のみ。
- 動作確認ポイント: 移行後に `/flow #<実在 Issue>` を 1 件流し、
  Phase 5 で Den の cargo ゲート（`--target-dir target-test`）・Phase 6 で `/code-review`・master squash が想定通り動くこと。
