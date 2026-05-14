# ワークフロー遵守ルール

## 原則

定義済みワークフロー（/develop, /ship, CI, Issue 駆動, 品質ゲート等）は忠実に従う。

## 不都合が生じた場合

1. なし崩しに無視・省略・ワークアラウンドしない
2. 「このルールをこう変えたい。理由は〜」と変更提案を明示する
3. ユーザーの承認を得てから CLAUDE.md / .claude/ を修正する

## Phase 4（検証）の厳守

UI 変更を含む場合、e2e テストを必ずローカルで実行してから出荷する。CSS セレクタや hidden 属性のオーバーライド問題は実行しないと検出できない。

### terminal renderer の vendor bump / adapter 修正時は切替スモーク必須

`frontend/vendor/restty/` や `frontend/vendor/wterm/` の **version bump（npm bump 含む）または adapter (`*-xterm-adapter.js`) のロジック修正**を含む release は、e2e に加えて **renderer 切替スモークを必ず実施** してから出荷する。

- e2e のデフォルト renderer (xterm) では restty / wterm 固有の regression が拾えない
- 切替後に最低限確認: 初期描画が遅延しないこと、CJK レンダリング、theme 反映、入力エコー
- chrome-cdp で自動化可能（手順は `memory/patterns.md` の「chrome-cdp で renderer 切替 + WASM ready 検証」を参照）
- v3.3.3 で restty 0.1.35 化したが、xterm e2e は全通過する一方で restty に切り替えると初期描画が破綻していた前例あり（v3.3.4 で hotfix → 実は v3.3.4 自体も誤判断で v3.3.5 で再修正）

#### adapter 修正時の必須チェックリスト

過去 hotfix を逆方向に修正してしまった v3.3.4 → v3.3.5 の教訓:

1. **`*-xterm-adapter.js` のコードを変更したら、`frontend/js/terminal-adapter.js` 内の `import('/vendor/<x>/xxx-adapter.js?v=N')` の `N` を必ず bump**。ESM dynamic import は hard reload (`Page.reload --ignoreCache`) でもキャッシュが残り、旧モジュールが返るケースあり
2. **動作確認は `chrome-cdp shot` 必須**: `eval` で `term._wasmReady === true` を確認しても fallback timer で立てたフリの可能性がある。canvas は `shot` でないと実描画を判別できない（`snap` は canvas を読めない）
3. **vendor の API 形を確認するときは memory の旧記述を鵜呑みにせず、`frontend/vendor/<x>/chunk-*.js` を直接 grep**。callbacks の destructure 経路は version 間で drift する（前回そうだった = 今回もそう、ではない）
4. **restty 特有**: console に `[restty] WASM ready timeout — flushing write buffer` が出たら、`onBackend` が正規ルートで届いていない疑い → fallback で `_wasmReady=true` 強制された状態 = 文字は出ても WASM 状態と乖離。callbacks 経路が壊れているので adapter を再点検する

詳細手順: `memory/patterns.md` の「vendor adapter 修正の検証手順」と「restty 0.1.35 の callbacks 経路」

## 具体例

- /develop の Phase が冗長に感じる → 「Phase 2 と 3 を統合したい。小さい Issue では設計と実装を分ける必要がないため」
- テストが書けない状況 → 「E2E テストは後回しにしたい。CI にサーバー起動の仕組みがまだないため」
- CI が通らない → 原因を調査して修正。「CI をスキップして push」は提案しない
