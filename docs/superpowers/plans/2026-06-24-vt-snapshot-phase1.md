# Terminal VT Snapshot Reconnect — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the broken raw-byte *full* reconnect replay with a server-side VT (vt100) snapshot of the visible screen, so reconnects on iPad/Safari stop duplicating output, retain scrollback, and restore the last line of TUIs like claude.

**Architecture:** Each PTY session runs a headless `vt100::Parser` fed the *same* output bytes as the existing byte ring, under one lock so the snapshot and the absolute sequence (`seq`) are read atomically. The live delta path (byte ring → broadcast wake → `replay_since`) is unchanged. Only the *full* replay branch (new connection or window-miss reconnect) changes: the server sends a clean, self-contained snapshot (`[history bytes][VT screen redraw]`) and the client does `term.reset()` before writing it — eliminating overlap (no dup), rebuilding scrollback from the 2 MB ring (retention), and stamping an authoritative current viewport (claude bottom line). This is the spec's **D-2** strategy: byte ring for history, VT for the visible screen only.

**Tech Stack:** Rust (axum + portable-pty + tokio), `vt100` 0.16.2 (promoted from dev-dependency), vanilla JS + xterm.js v6, Playwright e2e.

## Global Constraints

- **Crate:** `vt100` v0.16.2 only (decided in spike findings). No `avt`. Promote the existing `vt100 = "0.16"` from `[dev-dependencies]` to `[dependencies]` — do NOT add a new crate.
- **VT parser scrollback = 0.** D-2 means history comes from the byte ring, never from the VT parser; the snapshot is the *visible viewport only*. Giving the parser scrollback would risk the ~2.5 GB worst case the findings warned about for *no benefit*. Construct every parser with `vt100::Parser::new(rows, cols, 0)`. **This obviates the "configurable scrollback cap" follow-up from the findings — no new setting is added.** (Verified safe: spike `item1`/`item3` used scrollback `0`.)
- **`vt100::Screen::contents_formatted()` is self-contained for the viewport** (`\x1b[m\x1b[2J` + absolute per-row repaint + final cursor pos) but does NOT emit alt-screen entry. The snapshot builder MUST prepend `\x1b[?1049h` when `screen.alternate_screen()` is true, then append `screen.state_formatted()` (contents + input modes).
- **Atomicity:** snapshot bytes and `end_seq` MUST be produced under a single `replay_state` lock. A new write between them would re-send already-snapshotted bytes as a delta (dup) or skip bytes (gap).
- **`Parser` is not `Clone`.** Hold exactly one parser per session.
- Quality gates (run for every task that touches Rust/JS): `cargo fmt -- --check`, `cargo clippy -- -D warnings`, `cargo test --target-dir target-test`. `DEN_DATA_DIR=./data-dev` if a server is run. Long-running commands → background.
- PTY integration tests: `#[test]` + manual runtime (`build_test_runtime()` + `rt.shutdown_timeout`), never `#[tokio::test]` (`.claude/rules/development.md`).
- Commit messages in English (Conventional Commits). Branch: `feat/vt-snapshot-reconnect`. Merge target `master`, squash.

---

## File Structure

- **`Cargo.toml`** — move `vt100 = "0.16"` to `[dependencies]`.
- **`src/pty/ring_buffer.rs`** — `ReplaySlice` gains `snapshot: Option<Vec<u8>>`; existing constructors set `None`. Byte ring logic otherwise unchanged.
- **`src/pty/replay_state.rs`** *(new)* — `ReplayState { ring: RingBuffer, vt: vt100::Parser }`: owns the byte ring + VT parser, exposes `write`, `resize`, `total_written`, `replay_since` (attaches a snapshot iff the slice is full), and the private `snapshot_bytes` builder. All snapshot/seq atomicity lives here. New unit tests here.
- **`src/pty/mod.rs`** — register `mod replay_state;`.
- **`src/pty/registry.rs`** — `SharedSession.replay_buf: std::sync::Mutex<RingBuffer>` → `replay_state: Arc<std::sync::Mutex<ReplayState>>`; construct it in `setup_pty_session` *before* `resize_task` and clone into the resize task (so resizes follow the PTY); `read_task` feeds it; `setup_pty_session` gains `cols`/`rows` params; `replay_since` delegates to it; the create-path `ReplaySlice` literal gets `snapshot: None`.
- **`src/ws.rs`** — `handle_socket` initial replay + the live `pty_to_ws` loop send the snapshot protocol when a slice is full; retire `SYNC_FULL_MSG`. New `snapshot_control_frame`/send helper + unit test.
- **`frontend/js/terminal.js`** — handle `{"type":"snapshot"}` control frame → `term.reset()` then write; remove `SYNC_FULL`/`GAP_MARKER`/`pendingReset` path.
- **`tests/registry_test.rs`** — add snapshot-on-full integration assertion.
- **`tests/e2e/terminal.e2e.ts`** — snapshot-reconnect e2e (adapt to existing helpers).
- **`tests/vt_snapshot_spike.rs`** — deleted (throwaway, folded into `replay_state.rs` tests).
- **Docs:** spec + findings status bumps, `MEMORY.md` handover.

---

## Task 1: `ReplayState` module — byte ring + VT parser, snapshot builder

**Files:**
- Modify: `Cargo.toml:70-81` (move `vt100` to `[dependencies]`)
- Modify: `src/pty/ring_buffer.rs:1-11` (`ReplaySlice` + `snapshot` field), `:115-136` (set `snapshot: None`)
- Create: `src/pty/replay_state.rs`
- Modify: `src/pty/mod.rs` (`mod replay_state;`)
- Test: unit tests inside `src/pty/replay_state.rs`

**Interfaces:**
- Produces:
  - `ReplaySlice { pub data: Vec<u8>, pub full: bool, pub end_seq: u64, pub snapshot: Option<Vec<u8>> }`
  - `ReplayState::new(capacity: usize, rows: u16, cols: u16) -> ReplayState`
  - `ReplayState::write(&mut self, data: &[u8]) -> u64` (returns new `total_written`)
  - `ReplayState::resize(&mut self, cols: u16, rows: u16)`
  - `ReplayState::total_written(&self) -> u64`
  - `ReplayState::replay_since(&self, since: Option<u64>) -> ReplaySlice` (snapshot `Some` iff `full`)

- [ ] **Step 1: Move `vt100` to runtime dependencies**

In `Cargo.toml`, delete `vt100 = "0.16"` from `[dev-dependencies]` (line 81) and add it under `[dependencies]` (alongside the other runtime crates, keep alphabetical-ish ordering near the bottom of that block):

```toml
vt100 = "0.16"
```

- [ ] **Step 2: Add `snapshot` field to `ReplaySlice`**

In `src/pty/ring_buffer.rs`, extend the struct (top of file):

```rust
/// リプレイ片: クライアントへ送るデータと、その性質を表す。
pub struct ReplaySlice {
    /// 送出するバイト列（古い順）。
    pub data: Vec<u8>,
    /// true の場合、これは差分ではなくバッファ窓全体（新規接続、またはクライアントが
    /// バッファ窓より後れて差分を出せないとき）。
    pub full: bool,
    /// `data` の末尾に対応する絶対シーケンス（= これまでに書き込まれた総バイト数）。
    pub end_seq: u64,
    /// `full` のとき、可視画面のクリーンな再描画 ANSI（VT snapshot, D-2）。
    /// クライアントは reset 後に `data`（履歴）→ `snapshot` の順で書く。
    /// 差分（`full == false`）では常に `None`。RingBuffer 単体では常に `None` を入れ、
    /// VT を保持する `ReplayState` のみが `Some` を載せる。
    pub snapshot: Option<Vec<u8>>,
}
```

In the same file, set `snapshot: None` at both `ReplaySlice` constructions inside `RingBuffer::replay_since` (the delta branch ~line 115 and the full branch ~line 132):

```rust
            return ReplaySlice {
                data: self.read_last(take),
                full: false,
                end_seq: end,
                snapshot: None,
            };
```

```rust
        ReplaySlice {
            data,
            full: true,
            end_seq: end,
            snapshot: None,
        }
```

- [ ] **Step 3: Register the new module**

In `src/pty/mod.rs`, add alongside the other `mod` lines:

```rust
mod replay_state;
```

- [ ] **Step 4: Write the failing tests for `ReplayState`**

Create `src/pty/replay_state.rs` with ONLY the test module first (the `impl` comes next step), so the tests fail to compile/pass:

```rust
//! Per-session replay state: the existing byte ring plus a headless vt100
//! parser fed the same output bytes. On a *full* (re)connect we serve a clean
//! VT snapshot of the visible screen (spec D-2) instead of replaying raw bytes,
//! which fixes reconnect duplication, scrollback retention, and the dropped
//! last line of TUIs (claude) — see
//! docs/superpowers/specs/2026-06-24-terminal-vt-snapshot-reconnect-design.md.

use super::ring_buffer::{ReplaySlice, RingBuffer};

/// Byte ring (history, D-2) + vt100 parser (visible-screen snapshot).
pub struct ReplayState {
    ring: RingBuffer,
    vt: vt100::Parser,
}

// impl added in Step 5.

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-render `bytes` into a fresh parser and return its visible text.
    fn screen_text(rows: u16, cols: u16, bytes: &[u8]) -> String {
        let mut p = vt100::Parser::new(rows, cols, 0);
        p.process(bytes);
        p.screen().contents()
    }

    #[test]
    fn delta_replay_has_no_snapshot() {
        let mut rs = ReplayState::new(64, 24, 80);
        rs.write(b"hello");
        rs.write(b"world"); // total = 10
        let slice = rs.replay_since(Some(5)); // within window → delta
        assert!(!slice.full);
        assert_eq!(slice.end_seq, 10);
        assert_eq!(slice.data, b"world");
        assert!(slice.snapshot.is_none(), "delta must not carry a snapshot");
    }

    #[test]
    fn full_replay_carries_visible_snapshot() {
        let mut rs = ReplayState::new(1024, 24, 80);
        rs.write(b"echo SNAPSHOT_MARKER\r\n");
        let slice = rs.replay_since(None); // new connection → full
        assert!(slice.full);
        let snap = slice.snapshot.expect("full replay must carry a snapshot");
        // The snapshot, re-rendered into a blank terminal, reproduces the screen.
        assert!(
            screen_text(24, 80, &snap).contains("SNAPSHOT_MARKER"),
            "snapshot must redraw the visible screen content"
        );
    }

    #[test]
    fn snapshot_and_seq_are_consistent() {
        // The snapshot must reflect exactly the bytes counted by end_seq.
        let mut rs = ReplayState::new(1024, 24, 80);
        rs.write(b"abc");
        rs.write(b"def");
        let slice = rs.replay_since(None);
        assert_eq!(slice.end_seq, 6);
        let snap = slice.snapshot.unwrap();
        assert!(screen_text(24, 80, &snap).contains("abcdef"));
    }

    #[test]
    fn alt_screen_snapshot_prepends_enter() {
        let mut rs = ReplayState::new(4096, 24, 80);
        // Enter alt screen, draw a TUI whose LAST row is the claude-bottom case.
        rs.write(b"\x1b[?1049h\x1b[2J\x1b[H");
        rs.write(b"TUI top\r\n");
        rs.write(b"\x1b[24;1HTUI bottom line");
        let snap = rs.replay_since(None).snapshot.unwrap();
        assert!(
            snap.starts_with(b"\x1b[?1049h"),
            "alt-screen snapshot must re-enter the alternate screen first"
        );
        let text = screen_text(24, 80, &snap);
        assert!(text.contains("TUI bottom line"), "last alt-screen row must survive");
    }

    #[test]
    fn resize_changes_snapshot_geometry() {
        let mut rs = ReplayState::new(4096, 24, 80);
        rs.write(b"hi");
        rs.resize(40, 10); // cols=40, rows=10
        let snap = rs.replay_since(None).snapshot.unwrap();
        let mut p = vt100::Parser::new(10, 40, 0);
        p.process(&snap);
        assert_eq!(p.screen().size(), (10, 40), "snapshot must match resized geometry");
    }
}
```

- [ ] **Step 5: Run the tests to verify they fail**

Run: `cargo test --target-dir target-test replay_state`
Expected: FAIL — `ReplayState::new`/`write`/`resize`/`replay_since` not found (no `impl` yet).

- [ ] **Step 6: Implement `ReplayState`**

In `src/pty/replay_state.rs`, insert the `impl` between the struct and the `#[cfg(test)]` module:

```rust
impl ReplayState {
    /// `capacity` = byte-ring size; `rows`/`cols` = initial terminal geometry.
    /// VT scrollback is fixed at 0: history comes from the byte ring (D-2), the
    /// parser only ever serves the visible screen, so it needs no scrollback.
    pub fn new(capacity: usize, rows: u16, cols: u16) -> Self {
        Self {
            ring: RingBuffer::new(capacity),
            vt: vt100::Parser::new(rows, cols, 0),
        }
    }

    /// Feed one PTY output chunk to BOTH the byte ring and the VT parser, and
    /// return the new absolute sequence. Single call site (read_task), single
    /// lock — so a later `replay_since` reads a snapshot+seq that agree.
    pub fn write(&mut self, data: &[u8]) -> u64 {
        self.ring.write(data);
        self.vt.process(data);
        self.ring.total_written()
    }

    /// Track the PTY geometry so the snapshot matches the client's terminal.
    /// Note vt100 takes (rows, cols); our callers speak (cols, rows).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.vt.screen_mut().set_size(rows, cols);
    }

    pub fn total_written(&self) -> u64 {
        self.ring.total_written()
    }

    /// Like `RingBuffer::replay_since`, but when the result is a *full* window
    /// (new connection or window-miss) it also attaches a clean VT snapshot of
    /// the visible screen. Deltas are returned untouched (snapshot `None`).
    pub fn replay_since(&self, since: Option<u64>) -> ReplaySlice {
        let mut slice = self.ring.replay_since(since);
        if slice.full {
            slice.snapshot = Some(self.snapshot_bytes());
        }
        slice
    }

    /// Self-contained redraw of the current visible screen. `state_formatted`
    /// emits `\x1b[m\x1b[2J` + absolute per-row repaint + final cursor pos +
    /// input modes, but NOT the alt-screen entry — so prepend `?1049h` when the
    /// parser is on the alternate screen (claude/vim).
    fn snapshot_bytes(&self) -> Vec<u8> {
        let screen = self.vt.screen();
        let mut out = Vec::new();
        if screen.alternate_screen() {
            out.extend_from_slice(b"\x1b[?1049h");
        }
        out.extend_from_slice(&screen.state_formatted());
        out
    }
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --target-dir target-test replay_state`
Expected: PASS (all 5 tests). Then `cargo clippy --target-dir target-test -- -D warnings` and `cargo fmt -- --check`.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml src/pty/ring_buffer.rs src/pty/replay_state.rs src/pty/mod.rs
git commit -m "feat(pty): add ReplayState (byte ring + vt100 snapshot) for D-2 reconnect"
```

---

## Task 2: Wire `ReplayState` into the session (read/resize/replay)

**Files:**
- Modify: `src/pty/registry.rs` — `SharedSession` field (`:248`), `setup_pty_session` (`:534-663`), `read_task` (`:606-613`), `resize_task` (`:555-566`), `replay_since` (`:1474-1479`), create-path `ReplaySlice` literal (`:1014-1018`), and the two call sites that invoke `setup_pty_session` (pass `cols, rows`).
- Test: `tests/registry_test.rs` (PTY integration, manual-runtime pattern)

**Interfaces:**
- Consumes: `ReplayState` (Task 1).
- Produces: `SharedSession.replay_since(&self, since: Option<u64>) -> ReplaySlice` (unchanged signature; now snapshot-aware). VT parser tracks PTY resizes.

- [ ] **Step 1: Write the failing integration test**

In `tests/registry_test.rs`, add a test next to the existing interactive replay test (~line 316), reusing `build_test_runtime`/`init_shell` already in the file:

```rust
#[test]
fn reconnect_full_replay_includes_visible_snapshot() {
    let rt = build_test_runtime();
    rt.block_on(async {
        let reg = SessionRegistry::new_for_test();
        let (session, mut rx) = reg.create("snaptest", 80, 24).await.unwrap();
        init_shell(&session, &mut rx).await;

        // Produce a distinctive line on screen.
        session.write_input(b"echo SNAP_VT_MARKER\r").await.unwrap();
        // Let the shell echo + render.
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;

        // A brand-new client (since = None) gets a FULL replay → snapshot present.
        let slice = session.replay_since(None);
        assert!(slice.full, "new client must get a full replay");
        let snap = slice.snapshot.expect("full replay must carry a VT snapshot");

        // The snapshot, re-rendered, reproduces the marker on the visible screen.
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(&snap);
        assert!(
            p.screen().contents().contains("SNAP_VT_MARKER"),
            "snapshot must reflect the current screen, got:\n{}",
            p.screen().contents()
        );

        reg.destroy("snaptest").await;
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
}
```

Note: match the exact registry constructor used elsewhere in this file (`SessionRegistry::new_for_test()` or the existing helper — read the top of `registry_test.rs` and mirror it). `vt100` is available (now a normal dependency, also in dev scope).

- [ ] **Step 2: Run the test to verify it fails to compile**

Run: `cargo test --target-dir target-test reconnect_full_replay_includes_visible_snapshot`
Expected: FAIL — `slice.snapshot` exists but is always `None` (SharedSession still uses the plain `RingBuffer`, not `ReplayState`).

- [ ] **Step 3: Swap the session field to `ReplayState`**

In `src/pty/registry.rs`, change the field (`~:248`):

```rust
    /// リプレイ状態（byte ring + VT parser）。std::sync::Mutex: blocking context
    /// から常にアクセス可能。Arc で resize_task と共有し、リサイズを VT に追従させる。
    replay_state: std::sync::Arc<std::sync::Mutex<super::replay_state::ReplayState>>,
```

Update the imports near the top (`:12-13`) — drop the `RingBuffer` direct use if it becomes unused here, and add:

```rust
use super::replay_state::ReplayState;
```

(Keep `pub use super::ring_buffer::ReplaySlice;` — still the public return type.)

- [ ] **Step 4: Construct `ReplayState` before `resize_task` and clone into it**

In `setup_pty_session`, change the signature to accept geometry (it already has `#[allow(clippy::too_many_arguments)]`):

```rust
    fn setup_pty_session(
        name: &str,
        cols: u16,
        rows: u16,
        pty_reader: Box<dyn std::io::Read + Send>,
        // ...existing params unchanged...
    ) -> ( /* unchanged */ ) {
```

Immediately after the `broadcast::channel` / `resize_tx` setup (`~:550-551`), build the shared replay state:

```rust
        let replay_state = std::sync::Arc::new(std::sync::Mutex::new(
            ReplayState::new(REPLAY_CAPACITY, rows, cols),
        ));
        let replay_state_for_resize = std::sync::Arc::clone(&replay_state);
```

In `resize_task` (`:555-566`), after `master.resize(size)`, follow the VT parser to the same geometry:

```rust
        let resize_handle = tokio::task::spawn_blocking(move || {
            while let Ok((cols, rows)) = resize_rx.recv() {
                let size = PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                };
                let _ = master.resize(size);
                replay_state_for_resize
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .resize(cols, rows);
            }
            // master はここで drop → ClosePseudoConsole → OpenConsole.exe 終了
        });
```

In the `SharedSession` literal (`:568-589`), replace `replay_buf: std::sync::Mutex::new(RingBuffer::new(REPLAY_CAPACITY))` with:

```rust
            replay_state: std::sync::Arc::clone(&replay_state),
```

- [ ] **Step 5: Feed `ReplayState` from `read_task`**

In `read_task` (`:606-613`), replace the ring-only write with the combined write:

```rust
                        // replay state: byte ring + VT parser を同一ロックで更新。
                        // poison しても seq の連続性を保つため into_inner で復帰する。
                        let seq_end = {
                            let mut rs = session_for_read
                                .replay_state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            rs.write(&data)
                        };
```

- [ ] **Step 6: Delegate `replay_since` and fix the create-path literal**

In `SharedSession::replay_since` (`:1474-1479`):

```rust
    pub fn replay_since(&self, since: Option<u64>) -> ReplaySlice {
        self.replay_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replay_since(since)
    }
```

In the create path inside `get_or_create` (`:1014-1018`), add the new field to the empty slice:

```rust
                let replay = ReplaySlice {
                    data: Vec::new(),
                    full: false,
                    end_seq: 0,
                    snapshot: None,
                };
```

- [ ] **Step 7: Pass `cols, rows` at the two `setup_pty_session` call sites**

Search `Self::setup_pty_session(` in `src/pty/registry.rs` (two call sites — the shell/ssh spawn and the backend spawn). Insert `cols, rows` right after the `name` argument to match the new signature. The surrounding functions (`create_with_ssh`, `create_with_backend`) already have `cols`/`rows` in scope.

- [ ] **Step 8: Run the integration test + full build**

Run: `cargo test --target-dir target-test reconnect_full_replay_includes_visible_snapshot`
Expected: PASS.
Then: `cargo build --target-dir target-test` then `cargo clippy --target-dir target-test -- -D warnings` then `cargo fmt -- --check`. If `vt100::Parser` is not `Send` the build fails here — it is `Send`, but confirm the spawn_blocking closures compile.

- [ ] **Step 9: Run the existing PTY/registry suite (no regressions)**

Run (background per long-running rule): `cargo test --target-dir target-test`
Expected: PASS — existing `registry_test.rs` replay assertions still hold (delta path unchanged).

- [ ] **Step 10: Commit**

```bash
git add src/pty/registry.rs tests/registry_test.rs
git commit -m "feat(pty): feed vt100 parser from read_task, follow resizes, snapshot-aware replay"
```

---

## Task 3: Server — send the snapshot protocol on full replay

**Files:**
- Modify: `src/ws.rs` — initial replay (`:111-134`), live loop full branch (`:172-191`), remove `SYNC_FULL_MSG` (`:25`), add a send helper + unit test (`:587+`).
- Test: `#[cfg(test)] mod tests` in `src/ws.rs`

**Interfaces:**
- Consumes: `ReplaySlice { data, full, end_seq, snapshot }` (Tasks 1-2).
- Produces wire protocol: on a full slice with a snapshot, the server sends a Text control frame `{"type":"snapshot"}` immediately followed by one Binary frame `[8-byte be end_seq][filtered(history ++ snapshot)]`. Deltas are unchanged. The `{"type":"sync","mode":"full"}` frame is removed.

- [ ] **Step 1: Write the failing unit test for the frame builder**

In `src/ws.rs` `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn snapshot_binary_frame_concatenates_history_then_snapshot() {
        let history = b"HIST";
        let snapshot = b"SNAP";
        let frame = build_snapshot_binary(42, history, snapshot);
        // 8-byte big-endian seq prefix.
        assert_eq!(&frame[..8], &42u64.to_be_bytes());
        // history then snapshot, in order.
        assert_eq!(&frame[8..], b"HISTSNAP");
    }

    #[test]
    fn snapshot_control_frame_is_typed_json() {
        assert_eq!(SNAPSHOT_MSG, r#"{"type":"snapshot"}"#);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --target-dir target-test --lib ws::tests::snapshot`
Expected: FAIL — `build_snapshot_binary` / `SNAPSHOT_MSG` undefined.

- [ ] **Step 3: Add the control constant + frame builder, remove `SYNC_FULL_MSG`**

In `src/ws.rs`, replace the `SYNC_FULL_MSG` const (`:23-25`) with:

```rust
/// Snapshot control frame: the next binary frame is a full, self-contained
/// redraw (byte-ring history followed by a clean VT screen snapshot). The
/// client resets its terminal before applying it — so there is no overlap with
/// prior scrollback (no duplication) and the current viewport is authoritative.
const SNAPSHOT_MSG: &str = r#"{"type":"snapshot"}"#;

/// Build the snapshot binary frame: `[8-byte be seq][history ++ snapshot]`.
/// The combined buffer is run through `filter_conpty_private_modes`; the VT
/// snapshot never contains the blocked `?9001`/`?1004` modes, so filtering is a
/// no-op on its bytes and only scrubs the raw history portion.
fn build_snapshot_binary(end_seq: u64, history: &[u8], snapshot: &[u8]) -> Vec<u8> {
    let mut combined = Vec::with_capacity(history.len() + snapshot.len());
    combined.extend_from_slice(history);
    combined.extend_from_slice(snapshot);
    let filtered = filter_conpty_private_modes(&combined);
    seq_frame(end_seq, &filtered)
}
```

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test --target-dir target-test --lib ws::tests::snapshot`
Expected: PASS.

- [ ] **Step 5: Send the snapshot on the INITIAL replay**

In `handle_socket`, replace the initial-replay block (`:111-134`) with snapshot-aware logic:

```rust
    // 初期リプレイ。full かつ snapshot 付き → snapshot プロトコル（reset → 履歴 → snapshot）。
    // それ以外（差分）は従来どおり seq 前置バイナリを追記。
    let mut client_seq = replay.end_seq;
    if replay.full {
        if let Some(ref snapshot) = replay.snapshot {
            if ws_tx.send(Message::Text(SNAPSHOT_MSG.into())).await.is_err() {
                registry.detach(&session_name, client_id).await;
                return;
            }
            let frame = build_snapshot_binary(replay.end_seq, &replay.data, snapshot);
            if ws_tx.send(Message::Binary(frame.into())).await.is_err() {
                registry.detach(&session_name, client_id).await;
                return;
            }
        }
    } else if !replay.data.is_empty() {
        let filtered = filter_conpty_private_modes(&replay.data);
        if ws_tx
            .send(Message::Binary(seq_frame(replay.end_seq, &filtered).into()))
            .await
            .is_err()
        {
            registry.detach(&session_name, client_id).await;
            return;
        }
    }
```

(Note: a freshly-created session returns `full == false` with empty data, so it falls through harmlessly and relies on `first_rx`, exactly as before.)

- [ ] **Step 6: Send the snapshot on a live-loop full (window miss)**

In the `pty_to_ws` loop, replace the full/delta send block (`:173-191`) with:

```rust
            let slice = session_for_output.replay_since(Some(client_seq));
            if slice.end_seq != client_seq {
                if slice.full {
                    if let Some(ref snapshot) = slice.snapshot {
                        if ws_tx.send(Message::Text(SNAPSHOT_MSG.into())).await.is_err() {
                            break;
                        }
                        let frame = build_snapshot_binary(slice.end_seq, &slice.data, snapshot);
                        if ws_tx.send(Message::Binary(frame.into())).await.is_err() {
                            break;
                        }
                    }
                } else {
                    let filtered = filter_conpty_private_modes(&slice.data);
                    if ws_tx
                        .send(Message::Binary(seq_frame(slice.end_seq, &filtered).into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                client_seq = slice.end_seq;
            }
```

- [ ] **Step 7: Build + gates**

Run: `cargo build --target-dir target-test` → `cargo clippy --target-dir target-test -- -D warnings` → `cargo fmt -- --check` → `cargo test --target-dir target-test --lib ws`
Expected: PASS. (`SYNC_FULL_MSG` is gone; confirm no remaining references in `src/`.)

- [ ] **Step 8: Commit**

```bash
git add src/ws.rs
git commit -m "feat(ws): send VT snapshot protocol on full replay, retire sync/full frame"
```

---

## Task 4: Client — handle the snapshot frame, retire sync/full + gap marker

**Files:**
- Modify: `frontend/js/terminal.js` — control-frame handling (`:596-617`), binary handling (`:606-633`), `flushWrite` (`:565-573`), local flags (`:558-563`), `GAP_MARKER` const (`:22-27`).

**Interfaces:**
- Consumes: server protocol from Task 3 — Text `{"type":"snapshot"}` then Binary `[seq][bytes]`.
- Produces: on the snapshot frame, `term.reset()` then write the bytes; commit `lastSeq` only after the bytes are handed to the term (preserves the #117 deferred-commit invariant).

- [ ] **Step 1: Remove the `GAP_MARKER` constant**

In `frontend/js/terminal.js`, delete the `GAP_MARKER` declaration and its comment block (`:22-27`). (It is only referenced in the code we replace below.)

- [ ] **Step 2: Replace the per-connection sync flag with a snapshot flag**

In `stConnect`'s `attemptConnect` (`:558-563`), replace `let pendingReset = false;` and its comment with:

```js
      // Set when the server sends a {"type":"snapshot"} control frame: the next
      // binary frame is a full self-contained redraw (history + clean VT
      // snapshot). We reset the term before applying it so the authoritative
      // window replaces stale scrollback — no overlap, no duplication.
      let pendingSnapshot = false;
      let resetBeforeFlush = false;
```

- [ ] **Step 3: Reset inside `flushWrite` when a snapshot arrived**

Replace `flushWrite` (`:565-573`):

```js
      const flushWrite = () => {
        if (writeBuf.length === 0) return;
        const chunks = writeBuf;
        writeBuf = [];
        if (resetBeforeFlush) {
          resetBeforeFlush = false;
          st.term.reset();
        }
        st.term.write(chunks.length === 1 ? chunks[0] : mergeChunks(chunks));
        // Commit the seq only now that the bytes live in the term's buffer.
        st.lastSeq = pendingSeq;
      };
```

- [ ] **Step 4: Handle the `{"type":"snapshot"}` control frame**

In `ws.onmessage`, replace the `if (msg.type === 'sync') { ... }` block (`:596-601`) with:

```js
            if (msg.type === 'snapshot') {
              // Next binary frame is a full redraw: reset before applying it.
              pendingSnapshot = true;
              return;
            }
```

- [ ] **Step 5: Reset-and-supersede on the snapshot binary frame**

Replace the binary-frame `pendingReset` block (`:609-618`):

```js
          pendingSeq = new DataView(event.data).getBigUint64(0);
          if (pendingSnapshot) {
            pendingSnapshot = false;
            // Drop any not-yet-flushed deltas; the snapshot supersedes them.
            writeBuf = [];
            resetBeforeFlush = true;
          }
          writeBuf.push(new Uint8Array(event.data, 8));
```

- [ ] **Step 6: Lint**

Run: `npx eslint frontend/js/terminal.js`
Expected: 0 errors. (Confirm `GAP_MARKER`/`pendingReset` have no remaining references.)

- [ ] **Step 7: Commit**

```bash
git add frontend/js/terminal.js
git commit -m "feat(terminal): apply VT snapshot via reset on reconnect, drop sync/gap path"
```

---

## Task 5: e2e — snapshot reconnect + delete spike harness

**Files:**
- Modify: `tests/e2e/terminal.e2e.ts` (add a reconnect snapshot test; adapt existing reconnect/since assertions if any).
- Delete: `tests/vt_snapshot_spike.rs`.

- [ ] **Step 1: Read the existing terminal e2e to reuse helpers**

Read `tests/e2e/terminal.e2e.ts` and `tests/e2e/helpers.ts`. Identify the login/connect helper, the terminal selector, and how a session is created and typed into (mirror the existing patterns — do not invent new fixtures).

- [ ] **Step 2: Add the snapshot reconnect test**

Append to `tests/e2e/terminal.e2e.ts`, adapting selectors/helpers to the file's conventions:

```ts
test('reconnect restores a clean screen via VT snapshot (no duplication)', async ({ page }) => {
  // (use the file's existing login + terminal-ready helpers here)
  const term = page.locator('SELECTOR_FROM_FILE');

  // Type a unique marker and wait for it to render.
  await page.keyboard.type('echo SNAP_E2E_UNIQUE\n');
  await expect(term).toContainText('SNAP_E2E_UNIQUE');

  // Force a WS reconnect (close the socket; the client auto-reconnects with
  // ?since and falls into the full-replay → snapshot path when out of window).
  await page.evaluate(() => {
    // close every open terminal socket to trigger reconnect
    window.dispatchEvent(new Event('offline'));
  });
  // Simpler/robust alternative if the above is not wired: reload the page,
  // which always opens a fresh WS with the prior lastSeq absent → full replay.
  await page.reload();
  // (re-run the file's terminal-ready helper here)

  // The marker appears exactly once after the snapshot redraw.
  await expect(term).toContainText('SNAP_E2E_UNIQUE');
  const occurrences = (await term.innerText()).split('SNAP_E2E_UNIQUE').length - 1;
  expect(occurrences).toBe(1);
});
```

Note: prefer `page.reload()` (deterministic full replay on a fresh tab) if there is no clean hook to drop the socket. The assertion that matters: the marker is present **once** (snapshot redraw, no dup).

- [ ] **Step 3: Run the terminal e2e**

Stop any detached dev Den on the e2e port first (the e2e config self-spawns `cargo run` on its own HTTPS port). Run (background): `npx playwright test tests/e2e/terminal.e2e.ts`
Expected: PASS (existing tests + the new one). If an existing test asserted the old `sync`/gap behavior, update it to the snapshot protocol.

- [ ] **Step 4: Delete the spike harness**

```bash
git rm tests/vt_snapshot_spike.rs
```

Run: `cargo test --target-dir target-test` (background) to confirm nothing referenced it.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/e2e/terminal.e2e.ts
git commit -m "test: e2e snapshot reconnect; remove throwaway vt spike harness"
```

---

## Task 6: Full verification, renderer smoke, docs

**Files:**
- Modify: `docs/superpowers/specs/2026-06-24-terminal-vt-snapshot-reconnect-design.md` (status), `docs/superpowers/specs/2026-06-24-vt-snapshot-spike-findings.md` (mark Phase 1 done + note scrollback=0 decision), `MEMORY.md` (handover).

- [ ] **Step 1: Run all quality gates clean**

Run in order (long ones in background, read output files / use `--quiet`):
- `cargo fmt -- --check`
- `cargo clippy --target-dir target-test -- -D warnings`
- `cargo test --target-dir target-test`
- `npx eslint frontend/js/terminal.js`
- `npx playwright test tests/e2e/terminal.e2e.ts tests/e2e/sessions.e2e.ts tests/e2e/filer-ui.e2e.ts`

Expected: all PASS. Record the actual pass counts (no "should pass" — paste real output).

- [ ] **Step 2: Renderer switch smoke (restty / wterm)**

Per `.claude/rules/workflow.md`: the snapshot is plain ANSI written to the term, so it is renderer-agnostic, but verify anyway. Using chrome-cdp against a running dev Den (`DEN_DATA_DIR=./data-dev`), switch renderer to **restty** then **wterm**, run a TUI (e.g. a full-screen program or `claude`), trigger a reconnect (reload), and confirm with `chrome-cdp shot`: initial draw is not delayed, the last TUI row is present, theme/CJK/echo are intact. Note results in the commit/PR body.

- [ ] **Step 3: Update spec + findings status**

In the spec, change the status line to note **Phase 1 implemented** (link the branch). In the findings doc "Follow-ups for the Phase 1 plan" section, append a short note: the configurable scrollback-cap follow-up was **obviated** — D-2 runs the parser with scrollback `0`, so the per-session parser holds only the visible grid (no 2.5 GB risk), and no new setting was added.

- [ ] **Step 4: Update `MEMORY.md` handover**

Add a one-line index pointer and a handover note: Phase 1 (VT snapshot reconnect, D-2) implemented on `feat/vt-snapshot-reconnect`; vt100 promoted to a runtime dep; full replay now sends `{"type":"snapshot"}` + `[seq][history++snapshot]`, client resets then writes; `SYNC_FULL`/`GAP_MARKER` retired; **decision打 = iPad 実機**で重複消・保持増・claude 最下行を確認; Phase 2 (#3 reflow via `row_wrapped`) is next.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-06-24-terminal-vt-snapshot-reconnect-design.md docs/superpowers/specs/2026-06-24-vt-snapshot-spike-findings.md MEMORY.md
git commit -m "docs: mark VT snapshot Phase 1 done; record scrollback=0 (D-2) decision"
```

- [ ] **Step 6: Finish the branch**

Use `superpowers:finishing-a-development-branch` → Den default is `/code-review` (effort high; this touches the reconnect core) then `/security-review` (no auth/TLS/token boundary changed, but the WS replay path is in scope) → squash merge to `master`. Release (version bump + tag + production refresh) is out of scope here → `/release` afterward. **The decisive check is iPad real-device** (real claude, resize + reconnect: dup gone, retention up, claude bottom line present) before declaring #1 solved.

---

## Self-Review

**Spec coverage:**
- §3 追加型 (live path untouched, only full replay swapped) → Tasks 2-3 keep delta path verbatim, change only the `full` branch. ✓
- §4 atomic snapshot+seq, new `{"type":"snapshot"}` frame, client reset→write, delta follow-up → Tasks 1 (atomic in `ReplayState`), 3 (frame), 4 (client). ✓
- §6 D-2 (byte ring history + VT visible screen) → snapshot = `[history ++ state_formatted]`, parser scrollback 0. ✓
- §7 Phase 1 list (parser per session, both ring+VT fed, atomic snapshot, client reset, `?since` integration, promote vt100) → Tasks 1-4. `?since` is preserved (delta path unchanged); full path ignores it by design. ✓
- §8 影響範囲 files → all covered (`src/pty/*`, `src/ws.rs`, `terminal.js`, `Cargo.toml`). ✓
- §9 risks: crate scrollback/wrap (resolved by spike → scrollback 0), double-processing cost (accepted), alt-screen (explicit `?1049h` prepend + test), seam dup (reset() guarantees no overlap), renderer (Task 6 smoke). ✓
- §10 gates incl. atomicity/non-dup tests + e2e + renderer smoke + iPad → Tasks 1/2/5/6. ✓
- §11 unresolved: crate=vt100, D-2, #3 feasible — all resolved in findings; Phase 1 honors them. ✓
- Findings follow-ups: hook point (read_task) ✓ Task 2; snapshot API atomic ✓ Task 1; new frame ✓ Task 3; promote vt100 ✓ Task 1; **scrollback cap → obviated by scrollback=0, documented** ✓ Global Constraints + Task 6.

**Placeholder scan:** e2e selectors are intentionally deferred to "read the file and reuse helpers" (Task 5 Step 1) because the exact selector lives in `terminal.e2e.ts`; every Rust/JS step has concrete code. No TBD/TODO in shipped code.

**Type consistency:** `ReplaySlice` gains `snapshot: Option<Vec<u8>>` (Task 1) and every constructor across `ring_buffer.rs` (2) + `registry.rs` (1) is updated (Tasks 1-2). `ReplayState::resize(cols, rows)` vs vt100 `set_size(rows, cols)` ordering is called out explicitly. `replay_since` keeps its `(Option<u64>) -> ReplaySlice` shape end to end. `build_snapshot_binary(end_seq, history, snapshot)` / `SNAPSHOT_MSG` names match between Task 3 impl and tests. Client `pendingSnapshot`/`resetBeforeFlush`/`flushWrite` names consistent within Task 4.
