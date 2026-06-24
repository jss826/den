# ターミナル再接続のクリーン化 — サーバー側 VT snapshot 設計

- 日付: 2026-06-24
- ステータス: Phase 1 実装済み（master `3d20343`、v3.7.0）+ transient 改修（master `375ead2`、v3.7.1）。**Phase 2（#3 reflow）は検証の結果「Phase 1 で達成済み」と判明**（自前シリアライザ不要、§5 参照）。決定打 = iPad 実機確認のみ。
- 関連: v3.5.0 #117（seq 差分 replay）, v3.6.0（iPad replay 非破壊追記＋窓 2MB）, restty-refresh-noop
- 着想元: getpaseo/paseo のサーバー側ヘッドレス VT + snapshot/restore（`memory/paseo-architecture-comparison.md`）

## 1. 背景と問題

Den のターミナルは **生 PTY バイトを ring buffer に貯め、WebSocket で seq 前置のバイナリフレームとして差分配信**している（`src/pty/ring_buffer.rs` / `src/ws.rs` / `frontend/js/terminal.js`）。再接続時はクライアントの `?since=N` に基づき差分 or 全体 replay を返す。

この「生バイト窓の replay」方式は、**iPad/Safari のように頻繁に WS 再接続する環境で繰り返し破綻**してきた:

- #117（v3.5.0）: 再接続毎の全量再送による重複を seq 差分で解消。
- v3.6.0: full replay 時の `term.reset()` を廃止し非破壊追記＋GAP マーカー化、窓を 512KB→2MB に拡大。

**それでも iPad 実機では重複・保持不足が残る**（ユーザー実機確認、2026-06-24）。生バイト窓の整合（どこから再生すれば画面が正しく復元するか）に依存する限り、窓ずれ・行/エスケープ境界・リサイズ跨ぎで破綻し続ける。これは個別パッチの問題ではなく **「生バイト replay という方式そのものの限界」**。

Paseo は**サーバー側でヘッドレス VT（`@xterm/headless`）を走らせ、再接続時に画面状態の snapshot を再描画 ANSI として送る**ことで、この問題を構造的に回避している。Den も同じ原理を Rust で取り入れる。

## 2. 目標 / 非目標

### 目標
- **#1（最優先）**: 再接続時の重複・保持不足・リサイズ崩れ・claude 等 TUI の最下行欠け（alt 画面の最終フレーム）を**構造的に解消**する。
- **#3**: リサイズ時の soft-wrap reflow（復元後コンテンツが折り返し再計算される）。
- 既存のライブ配信路（生バイト → ring buffer → 差分配信）の体感性能・挙動を劣化させない。

### 非目標（今回スコープ外）
- **#2 slot 多重化 / 1 WS 集約 / output coalescer**: 優先度最下位。別トラック。
- VT を唯一の source of truth にする全面置換（Paseo 完全踏襲）。リスク大のため採らない。
- E2EE relay 等、ターミナル描画と無関係の Paseo 知見。

## 3. 採用アプローチ: 追加型（再接続 snapshot だけ VT 化）

3 案（追加型 / 全面置換 / VT 無しで現行継続）を比較し、**追加型**を採用。理由 = Den はこの領域を何度も壊してきたため、実績あるライブ路を温存しつつ「壊れている再接続 replay の 1 点だけ」を外科的に差し替えるのが最小リスク。

### 構成
- **ライブ経路は無改変**: 生 PTY バイト → `RingBuffer` → broadcast 起床 → `replay_since` 差分配信（`src/ws.rs` の `pty_to_ws` ループ）。
- **追加**: セッションごとに**サーバー側 VT パーサ**を 1 個持ち、PTY の同じ出力バイトを食わせて「現在の画面状態（＋可能なら scrollback）」を保持する。
- **置換は (再)接続時の初期 replay 1 点のみ**: 現状の「生バイト窓 replay」を、**VT が生成するクリーンな再描画 snapshot**に差し替える。

## 4. データフロー（再接続時）

1. クライアントが WS 接続（初期 replay 要求）。`?since=N` は不要化（後述の互換）。
2. サーバーが**同一ロック下で原子的に** (a) VT から snapshot（画面＋可能なら scrollback の再描画 ANSI）と (b) その時点の絶対 seq `S` を取得する。
   - 原子性が重要: snapshot 生成と seq 取得の間に新しい出力が混ざると、snapshot に含まれた内容を差分で二重送出してしまう。
3. クライアントは受信 snapshot を **`term.reset()` してから write**（= 現在状態のクリーン再描画。生バイト窓の整合に依存しない＝重複なし、alt 画面ごと正しい最終フレーム、claude 最下行も復元）。
4. 以降は **seq `S` から既存の差分ライブ路**（`replay_since(Some(S))`）で追従。

### プロトコル
- 新しい制御フレーム（例 `{"type":"snapshot"}`）で「次のバイナリは reset 後に write する完全再描画」であることを伝える。既存の `{"type":"sync","mode":"full"}` と排他/置換関係を整理する（snapshot 経路では full byte replay は使わない）。
- ライブ差分フレーム（8B seq 前置バイナリ）は**従来どおり**。

## 5. #3 reflow の扱い（Phase 2 → Phase 1 で達成済みと判明）

- snapshot を **論理行＋autowrap（行 wrap 情報付き）**で直列化できれば、復元後も xterm がリサイズで折り返しを再計算する（Paseo 方式: continuation 行を full width で出して次行先頭で auto-wrap を誘発）。
- **ライブ中のリサイズ reflow は現状どおり xterm が処理**するため、Phase 1 のみでも体感劣化はしない（reflow が劣化するのは「再接続で復元した過去行」に限られる）。

### Phase 2 検証結果（2026-06-24）: 自前シリアライザは不要

当初は「論理行＋autowrap の直列化を自前実装する Phase 2」を想定していたが、vt100 0.16.2 のソース実読＋throwaway spike（`tests/vt_reflow_spike.rs`、検証後削除）で **Phase 1 が使う `Screen::state_formatted()` が既に wrap 対応**であることが判明した:

- `grid::write_contents_formatted` は各行の `row.wrapped()` を追跡し、継続行には**絶対座標移動（CUP）を出さず**内容を連続ストリームとして流す（`row.rs:226-237` のガード）。これはまさに「continuation 行で auto-wrap を誘発」する Paseo 方式そのもの。
- spike の決定的アサーション: snapshot を**まっさらな同寸パーサに再描画すると、そのパーサが独立に `row_wrapped(0/1)==true / row_wrapped(2)==false` を報告**。soft-wrap が auto-wrap 経由で忠実に伝送されている。xterm.js は同じ auto-wrap → `isWrapped` セマンティクスなので、同じバイトを受ければ継続行を soft-wrap と記録しリサイズで reflow する。
- よって **#3 は Phase 1（`state_formatted()`）で構造的に達成済み**。Phase 2 のコード作業は「この性質を将来の直列化変更から守る回帰テスト」（`src/pty/replay_state.rs` の `snapshot_preserves_soft_wrap_for_reflow` / `snapshot_keeps_hard_newlines_unwrapped`）の追加のみ。
- 残る検証は **xterm 実描画での reflow 確認＝iPad 実機**（Rust 側 precondition は証明済み）。

## 6. スクロールバック/保持の戦略（spike で確定）

snapshot にどこまで履歴を含めるかで 2 案。Phase 0 spike の結果で選ぶ。

- **D-1（理想・継ぎ目なし）**: VT crate が scrollback を保持・直列化できる → snapshot 一発で画面＋履歴。保持量は VT の scrollback 容量で決まる（xterm 側 scrollback と整合させる、現状 5000 行）。
- **D-2（フォールバック）**: VT は可視画面のみ。履歴は**現状の byte ring（行境界トリム）を前置**し `[履歴バイト][VT 画面 snapshot]` を送る。可視画面のクリーン化（dup/alt 画面/claude 下部）は確実に得られるが、履歴バイトと VT 画面の**継ぎ目で二重化が出ないよう**、履歴は「画面に表示中の行より前」だけに限定する等の境界処理が要る。

D-1 が取れない場合でも #1 の核（再接続のクリーン化）は D-2 で達成できる。

## 7. 段階リリース（リスク順。やる順のこだわりは無い前提）

### Phase 0 — spike（crate 検証）
- **第一候補 = `vt100`**（`contents_formatted()` による可視画面再描画を doc 確認済、pure Rust、枯れている）。**フォールバック候補 = `avt`**（asciinema 製、reflow が本職）。
- 検証項目:
  1. 可視画面の忠実な再描画（前景/背景色・bold/italic/underline 等の属性・カーソル位置/形状・関連 DEC モード）。
  2. **scrollback の直列化可否** → D-1/D-2 を確定。
  3. **alt 画面**（claude/vim 等）の現在フレームを正しく snapshot できるか。
  4. **行 wrap 情報の有無** → #3（Phase 2）の可否を確定。
  5. 全 PTY チャンク二重処理時の CPU/メモリ概算（最大 50 セッション）。
- **新 crate 追加はこの Phase で 1 個**（`vt100` または `avt`）。`development.md`「新 crate はユーザー相談」に従い、本 spec 承認をもって候補追加の合意とする（最終確定は spike 結果で報告）。
- 成果物: spike 所見ドキュメント（採用 crate / D-1 か D-2 か / #3 可否）。

### Phase 1 — #1 解消（本丸）
- セッションに VT パーサを追加し、PTY 出力を ring と VT の両方へ供給。
- (再)接続時の初期 replay を **VT snapshot に置換**（§4）。snapshot 生成＋seq 取得を原子化。
- ライブ差分路・ring buffer は維持。
- クライアント（`terminal.js`）: snapshot 制御フレーム受信 → `term.reset()` → write → seq 同期。`?since` 経路の整理。
- これで iPad の重複・保持不足・claude 下部欠けが消えることを実機確認。

### Phase 2 — #3 reflow（Phase 0 で wrap 情報取得が確認できた場合のみ）
- snapshot を wrap-aware 直列化に変更（論理行＋autowrap）。
- 復元後コンテンツのリサイズ reflow を確認。

### スコープ外
- #2 slot 多重化 / coalescer。

## 8. 影響範囲（ファイル）

- `src/pty/`（VT パーサの保持先。`session.rs`/`registry.rs` 近辺。snapshot 生成 API と seq の原子取得）。
- `src/pty/ring_buffer.rs`（D-2 採用時の履歴前置との整合。基本は無改変）。
- `src/ws.rs`（初期 replay の分岐を snapshot 経路へ。snapshot 制御フレーム送出）。
- `frontend/js/terminal.js`（snapshot 制御フレーム処理、reset→write、seq 同期、`?since` 整理）。
- `Cargo.toml`（新 crate 1 個、Phase 0）。

## 9. リスクと対策

- **最大リスク = crate が scrollback/wrap を出せるか**。→ Phase 0 spike を最初に置き、ダメなら D-2／独自直列化に倒す明確な分岐点を設ける。
- **二重処理の CPU/メモリ**。→ spike で概算、必要なら VT 供給のコアレッシングを検討（ただし初手は無し）。
- **alt 画面/同期出力（claude の `?2026`）の取りこぼし**。→ spike で claude 実出力を流して検証。
- **snapshot と差分の継ぎ目重複**。→ snapshot 生成と seq 取得の原子化で防ぐ。回帰テスト（snapshot 直後の差分が二重化しない）を入れる。
- **vendor renderer（restty/wterm）依存**。snapshot は通常の ANSI を write するだけなので renderer 非依存だが、`workflow.md` に従い切替スモークを出荷前に実施。

## 10. 検証（品質ゲート）

- Phase 共通: `cargo fmt -- --check` / `cargo clippy -- -D warnings` / `cargo test --target-dir target-test`。
- ring_buffer/ws の新規ユニットテスト（snapshot＋seq 原子性、snapshot 直後の差分非重複）。
- UI 変更を含むため e2e（`tests/e2e/filer-ui.e2e.ts` および terminal/sessions e2e）。
- renderer 切替スモーク（restty/wterm、`workflow.md`）。
- **決定打 = iPad 実機**: 実 claude を流し、リサイズ/再接続を繰り返して (a) 重複が出ない (b) 保持が十分 (c) claude 最下行が出る、を確認。

## 11. 未解決（spike で確定）

- 採用 crate（vt100 / avt）。
- 保持戦略（D-1 / D-2）。
- #3 reflow の実現可否（行 wrap 情報の有無）。
