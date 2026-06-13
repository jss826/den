# 設計メモ: セッション切替とスクロールバック保持 (#115)

- 日付: 2026-06-13
- 対象 Issue: #115（切替でスクロールバック喪失）
- 関連: #114（切替で前セッション混在・修正済み e2a485a）、#116（スクロール/再描画不安定 iPad）、#107（viewport 下端隠れ）

## 1. 現状アーキテクチャ（確証済み）

- `frontend/js/terminal.js` は **単一の `term` インスタンス**（module-level `let term`、`init()` で1回だけ生成し `#terminal-container` に `term.open()`）を全セッションで使い回す。
- セッション切替 `switchSession()` は `disconnect()` → `term.reset()` → `doConnect()`（#114 修正後）。`term.reset()` で **client 側スクロールバック（既定 `terminal_scrollback = 1000` 行）を全消去**。
- 再接続時、サーバーは per-session の replay ring buffer（`src/pty/registry.rs` `REPLAY_CAPACITY = 64 * 1024` = 64KB）を `read_all()` で送出（registry.rs:757）。これが切替後の唯一の履歴復元手段。
- 結果: **64KB を超える古い出力は復元されず、切替の度に消える**。深いスクロールバックは保持されない。

> #114 と #115 は同一の構造的根（単一 term + 切替時 reset + replay 依存）に由来する。#114 は「reset 直前の旧接続流入」を塞いだだけで、reset 自体は残っている。

## 2. 制約・前提

- レンダラーは xterm / restty / wterm の3種（`TerminalAdapter`）。**restty/wterm は WASM + canvas**。インスタンス生成は xterm より重く、特に iPad mini で初期化コストが大きい（memory: restty WASM ready, canvas first-paint 問題）。
- `MAX_SESSIONS = 50`（`src/pty/registry.rs`）。サーバーは1セッションに複数クライアント attach 可（broadcast）。
- 既存の `#terminal-container` は単一 DOM。
- 切替は頻繁な操作。体感速度が重要。

## 3. 選択肢

### 案1 — replay 上限の引き上げ（サーバー、最小）

`REPLAY_CAPACITY` を 64KB → 256KB〜1MB 程度に増やす。

- **Pro**: 数行。即効性。リスク極小。
- **Con**:
  - メモリ = cap × アクティブセッション数（最悪 1MB × 50 = 50MB）。
  - 依然 **cap 上限あり** → 深いスクロールバックは結局失われる（緩和であって根治でない）。
  - `term.reset()` + 全 replay 再描画は残るので、切替毎の **再描画フリッカ / CPU コスト**（特に大 replay × iPad）はむしろ悪化方向。
  - #114 系の構造問題には無関係。
- **位置づけ**: 案2 とは独立の「いつでも入れられる小さな緩和」。単独で #115 を閉じる解にはしない。

### 案2 — per-session の client term 保持（本命・大）

セッション毎に term インスタンスを生成して保持し、切替は **reset せず表示/非表示の切替**にする（hidden で残す）。

- **Pro**:
  - client スクロールバック（1000行/セッション）が切替を跨いで**保持**される → #115 の根治。
  - 共有 term が無くなるため **#114 系の混在を構造的に排除**。
  - 切替時に reset も full replay 再描画も不要 → **切替が即時**、フリッカなし。
- **Con / 要設計**:
  - **メモリ**: N term + N canvas。restty/wterm は WASM/canvas が重い。→ **保持数を LRU で上限 K（例 K=5〜8）** にし、溢れたら破棄（再表示時は replay から復元、現状と同等にデグレード）。
  - **WS 接続モデル**（最重要の設計点）:
    - (a) **背景セッションも WS 維持**: hidden でも term が live 更新。最もスクロールバックが正確。コスト = N ソケット + 背景描画 CPU。
    - (b) **表示時のみ接続 / 非表示で切断**: 省リソースだが、再表示時 replay と**保持済み term の内容が重複**する恐れ → サーバーが per-client offset を持つ or 「reset しない場合は replay 抑止し delta のみ」等の対応が必要。
    - 推奨: まず (a) を K 個までに限定（LRU 内のセッションだけ WS 維持）。実装が単純で重複問題が無い。
  - **DOM**: `#terminal-container` 配下に per-session の子コンテナを作り show/hide（`[hidden]` + ID セレクタの `display:flex` 落とし穴に注意 — DESIGN.md / CLAUDE.md）。
  - **テーマ/フォント/リサイズ**: 設定変更時は保持中の全 term に適用。リサイズは表示中 term のみ（非表示は表示時に relayout）。
  - **adapter ライフサイクル**: restty/wterm の dispose を確実に（破棄時 WASM/canvas リーク防止）。

### 案2-lite（現実的な初手）

案2 の構造（per-session term + LRU K + WS 維持 (a)）を K を小さく（例 3）して導入。多くのユースケース（直近数セッションを行き来）をカバーしつつメモリを抑える。

## 4. 推奨

1. **#115 の根治は案2**（per-session term, LRU 上限, WS 維持 (a)）。これは #114 系の再発も構造的に防ぐ。
2. ただし restty/wterm の WASM/canvas コストと iPad mini 実機の挙動が未知数 → **最初に「N=2 で2セッション保持」の PoC** を作り、iPad mini で初期化コスト・メモリ・切替体感を計測してから K を決める。
3. **案1（replay bump）は案2 と独立の小緩和**として、#116 の調査と合わせて入れてよい（ただし cap × 50 のメモリは見ておく）。
4. 影響範囲が大きいため、PoC 計測結果を見て **#115 を「案2本実装」と「PoC/計測」にサブ Issue 分割**する可能性あり。

## 5. 未確定・要計測（PoC で潰す）

- restty/wterm を1ページに **2インスタンス同時生成**した時の WASM/canvas メモリと初期化時間（iPad mini 実機）。
- 背景セッションの WS 維持による CPU（特に高頻度出力セッションを hidden 保持した場合）。
- LRU で破棄→再表示した時の replay 復元が現状と同等に動くか。
- `[hidden]` 子コンテナ show/hide と各 adapter の relayout/refresh の相性（#116 と重複領域）。

## 6. 次アクション

- [x] この方向性（案2 + PoC 先行）をユーザー承認
- [x] PoC ブランチで2セッション保持を実装（`feat/115-multiterm-poc`）— デスクトップ検証済み
- [ ] **iPad mini 実機で restty/wterm の WASM/canvas 初期化コスト・メモリ・切替体感を計測**（PoC のハンドオフ先）
- [ ] 計測結果で K と WS モデルを確定 → 本実装 or サブ Issue 分割
- [ ] （任意・独立）案1 replay bump の可否判断

## 7. PoC 実装結果（2026-06-13, `feat/115-multiterm-poc`）

案2 の最小形（K=2 固定・WS モデル(a)）を `frontend/js/terminal.js` + `frontend/css/style.css` に実装。

### 採用した構造

- **SessionTerm**（`createSessionTerm`）: セッション毎に term + fitAddon + WS + generation/ping/reconnect を保持。`#terminal-container` 配下の `.term-session-host`（`position:absolute; inset:0`、非アクティブは `[hidden]`）に `term.open()`。
- `term`/`fitAddon` は **active のミラー**（fit / select mode / context menu / focus / theme / `getTerminal()` は単一 `term` 参照のまま動作）。接続状態は per-SessionTerm に移動。
- **切替 = `activateSession()`**: `active.host.hidden=true` → 新 host を show + fit + focus。**`term.reset()` も full replay もしない** → client scrollback 保持（#115 根治）。共有 term が無いため #114 系の混在も構造的に排除。
- **LRU 上限 `MAX_RETAINED=2`**: 溢れたら `stDispose`（WS close + `term.dispose()` + host 除去）。再表示時は replay 復元（現状デグレード）。active は決して evict しない。
- **WS モデル(a)**: 保持中セッションは背景でも WS 維持。背景 term への書き込みは rAF を介さず直接 flush（hidden 要素の rAF スロットル回避）。背景 term は入力を送らない（`active!==st` ガード）。
- 設定変更の追従: theme 変更は全保持 term に即時適用、font/scrollback は `showSessionTerm()` で表示時に再適用。

### デスクトップ検証（全 PASS）

- cargo fmt / clippy(-D warnings) / test（71+40+5）: PASS
- ESLint（0 errors）/ frontend unit（89/89）: PASS
- e2e terminal/sessions/filer-ui: 25 passed / 2 skipped
- **新規 e2e `scrollback is preserved when switching sessions (#115)`**（`tests/e2e/sessions.e2e.ts`）: A の term にクライアント側マーカーを書き込み → B へ切替 → A へ戻ると **xterm バッファにマーカーが残存**することを assert（旧実装の reset-on-switch では消える）。PASS。
  - 副次的に `#terminal-container .xterm` が 2 要素に解決する（A/B の term が DOM 共存）ことを確認 = PoC の保持構造が実際に効いている証跡。

### iPad mini 計測で潰す未確定（§5 と同じ、PoC で土台は用意済み）

- restty/wterm を **2インスタンス同時生成**した時の WASM/canvas メモリ・初期化時間（iPad mini 実機）。
- 背景セッション WS 維持の CPU（高頻度出力セッションを hidden 保持した場合）。
- LRU 破棄→再表示の replay 復元が現状と同等か。
- `[hidden]` host show/hide と各 adapter の relayout/refresh の相性（#116 と重複領域）。

> 注: デスクトップ e2e は **xterm レンダラーのみ**。restty/wterm 固有の初期描画・canvas コストは iPad mini 実機計測が本番（design 方針どおり PoC のハンドオフ先）。
