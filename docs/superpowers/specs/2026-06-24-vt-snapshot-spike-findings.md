# VT Snapshot Spike — Findings (2026-06-24)

Spike for spec `2026-06-24-terminal-vt-snapshot-reconnect-design.md`.
Harness: `tests/vt_snapshot_spike.rs` (throwaway).
Branch: `chore/terminal-vt-snapshot-spec`
Commits: `782b9dd`..`afca116` (Tasks 1–5 evidence).

## Decisions

- **Crate:** vt100 v0.16.2. GO. No avt fallback needed — all five probe tasks passed with no disqualifying result.
- **Scrollback strategy:** D-2 (byte-ring prepend). `contents_formatted()` at offset 0 serializes only the live visible viewport (Task 3: `restored_has_old = false`). Scrollback rows are reachable only via `set_scrollback(n)` + re-feeding raw bytes into a fresh parser (`Parser` is not `Clone`) — equivalent to byte replay. No single-call scrollback-dump API exists. Phase 1 keeps the existing byte ring for history and uses the VT snapshot exclusively for the visible screen on reconnect.
- **#3 reflow feasible:** YES. `Screen::row_wrapped(row: u16) -> bool` (vt100 0.16.2, `screen.rs:540`) returns `true` for soft-wrapped rows, `false` for hard-newline rows. Phase 2 reflow can walk rows and recover logical-line boundaries from this flag. `avt` is not needed. Phase 1 ships first; #3 is Phase 2.
- **Cost verdict:** Release ≈ 61–67 MB/s per session; debug ≈ 2–4 MB/s (30× slower, unoptimized). Visible snapshot ≈ 851 bytes. 50-session worst case at 2 MB/s sustained burst each ≈ 1.65 CPU cores — acceptable on any multi-core box. Each 4096-byte chunk ≈ 64 µs in release. Per-parser memory realistic 2–5 MB → 50 sessions ≈ 100–250 MB (comparable to existing ring buffer). No coalescing needed. **Caveat for Phase 1:** a fully-populated 5000-row scrollback × 50 sessions could reach ~2.5 GB worst case → Phase 1 must expose a configurable scrollback cap.

## Evidence (per spec §7 items)

### 1. Visible-screen round-trip + chunk-split safety: PASS

Task 1 confirmed vt100 v0.16.2 faithfully round-trips a 24×80 visible screen including SGR color attributes (`Color::Idx(u8)` for 256-color) and bold. An SGR escape sequence (`\x1b[31m`) deliberately cut in half by a tiny chunk size (fed 2 bytes at a time vs. whole) produced an identical screen — the parser buffers the partial escape across `process()` calls rather than rendering garbage, and the SGR introducer never leaked into visible text. This proves chunk-boundary safety for Den's ≤4096-byte PTY reads (`registry.rs:595`), where an escape can be split arbitrarily. `contents_formatted()` output fed into a fresh parser reproduced an identical screen. Commit range `782b9dd`..`09f8581`.

Confirmed public API:
- `Parser::new(rows: u16, cols: u16, scrollback: usize) -> Parser`
- `Parser::process(&mut self, bytes: &[u8])`
- `Parser::screen(&self) -> &Screen`
- `Screen::contents() -> String`
- `Screen::contents_formatted() -> Vec<u8>`
- `Screen::cursor_position() -> (u16, u16)`
- `Screen::cell(row: u16, col: u16) -> Option<&Cell>`
- `Screen::size() -> (u16, u16)`
- `Cell::fgcolor() -> Color` (`Color::Idx(u8)` for 256-color palette)
- `Cell::contents() -> String`
- `Screen::row_wrapped(row: u16) -> bool` (screen.rs:540)
- `Screen::set_scrollback(n: usize)` / `Screen::scrollback() -> usize` (current offset, not ring size)

Helper `feed_chunked(rows, cols, scrollback, bytes, chunk)` defined in Task 1 used by all subsequent tasks.

`Parser` is NOT `Clone`.

### 2. Scrollback serialization: D-2 (byte ring for history)

Task 3 probe output (commit `790326e`):

```
SCROLLBACK PROBE:
  contents_formatted (offset=0) restores old 'line 0'? false
  scrollback_offset before set_scrollback: 0
  actual scrollback rows held by parser (clamped usize::MAX): 27
  scrollback page at max offset contains 'line 0'? true
  walking all offsets in screen-height steps contains 'line 0'? true
  visible screen text (last ~24 lines):
line 27
line 28
...
line 49
test item2_scrollback_serialization_probe ... ok
```

`contents_formatted()` at offset 0 returns only the 24 live visible rows. Scrollback content ("line 0") is accessible only by calling `set_scrollback(n)` + `contents()`, and only one viewport-height window at a time. Because `Parser` is not `Clone`, walking the full scrollback ring requires re-feeding the original raw bytes into a fresh parser — equivalent to the byte replay already provided by the existing ring buffer. There is no single-call API that exports the entire scrollback ring as escape sequences.

**Conclusion:** `set_scrollback` is a GUI scroll utility, not a serialization API. D-1 (seamless, snapshot includes history) is not viable. D-2 is correct: existing byte ring for history, `contents_formatted()` for visible screen.

### 3. Alt-screen / claude bottom line: PASS

Task 2 (commit `339c7a8`) asserted all four conditions:

- `text.contains("TUI top")` — alt-screen content present: PASS
- `text.contains("TUI bottom line")` — last alt-screen row (the exact claude bottom-line case) captured: PASS
- `!text.contains("main screen line")` — main-screen content does NOT leak into alt-screen snapshot: PASS
- `restored.screen().contents() == text` — `contents_formatted()` round-trip faithful: PASS

vt100 v0.16.x correctly tracks `?1049h` (`DECSET 1049`, enter alt screen) and snapshots the current alt-screen frame including the last row. The "claude bottom line missing on reconnect" bug is therefore a Den replay-path problem (raw bytes are replayed from the ring buffer rather than a VT snapshot), not a vt100 parser deficiency. Switching to snapshot-based reconnect replay will capture the last line.

**`?2026h/l` (DEC synchronized output) observation:** Both sequences were treated as no-ops by vt100 v0.16.2 — no visible effect on `contents()` or `contents_formatted()`. Phase 1 does not need to strip or specially handle `?2026` sequences before feeding bytes to the parser.

### 4. Soft-wrap info: `Screen::row_wrapped(row: u16) -> bool` — found, public, correct

Task 4 (commit `f8bbb4b`) probe output:

```
WRAP PROBE:
  row0: "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
  row1: "XXXXXXXXXXXXXXXXXXXX"
  screen.row_wrapped(0) = true  ← FINDING: public API exists
  screen.row_wrapped(1) = false
  API: Screen::row_wrapped(row: u16) -> bool  [screen.rs:540]
  Row::wrapped() -> bool also exists but Row is not pub-exported [row.rs:82]
  #3 FEASIBILITY VERDICT: FEASIBLE — wrap flag is available via public API
test item4_wrap_info_probe ... ok
```

Hard assert `screen.row_wrapped(0) == true` passed. The backing `Row::wrapped` field is set by the parser's text-drawing path whenever autowrap fires. `Row` itself is not in `lib.rs`'s `pub use` list; only `Screen::row_wrapped` is needed from outside. Cell-level continuation markers do not exist — wrapping is tracked per row only, which is sufficient for Phase 2 reflow.

### 5. Double-processing cost: acceptable, no coalescing needed

Task 5 (commits `2cd29b2`, `afca116`):

**Release build:**
```
COST PROBE:
  processed 8.0 MB in 130.3144ms → 61 MB/s per session
  snapshot size: 851 bytes
  (parser holds 24x80 grid + up to 5000 scrollback rows)
test item5_double_processing_cost ... ok
```

**Debug build (unoptimized):**
```
COST PROBE:
  processed 8.0 MB in 1.8411369s → 4 MB/s per session
  snapshot size: 851 bytes
test item5_double_processing_cost ... ok
```

(Debug runs with the profile-aware floor of 0.5 MB/s; commit `afca116` lowered the assert floor for `cfg(debug_assertions)` so the project's standard `cargo test --target-dir target-test` gate passes.)

50-session CPU worst case: 50 × (2 MB/s ÷ 61 MB/s) ≈ 1.65 cores. Per-session memory realistic 2–5 MB; 50 sessions ≈ 100–250 MB. Visible snapshot compact at 851 bytes. Each 4096-byte PTY chunk costs ~64 µs in release. Coalescing would only become relevant below ~5 MB/s sustained — 12× headroom at 61 MB/s.

## Go/No-Go for Phase 1

- **Phase 1 (#1 reconnect clean-up): GO**, strategy **D-2**. vt100 v0.16.2 passes all five spec §7 items. The visible-screen snapshot fixes the claude bottom-line bug at its root. No avt fallback needed.
- **Phase 2 (#3 reflow): ON ROADMAP.** Feasible via `Screen::row_wrapped(row: u16) -> bool`. Implement after Phase 1 ships.

## Follow-ups for the Phase 1 plan

- **VT parser hook point:** `src/pty/registry.rs` `read_task` (~line 595–628). Feed the same `&data` slice to the vt100 parser in the same locked section as `replay_buf.write`, atomic with the seq counter. One `Parser` instance per session, held in the session state alongside the ring buffer.
- **Snapshot API:** Atomically return `(contents_formatted_bytes: Vec<u8>, total_written_seq: u64)` under one lock, per spec §4 step 2. This is the payload for the new reconnect frame.
- **New protocol frame:** `{"type":"snapshot", "data": <base64 or raw bytes>}` on reconnect. Client (`terminal.js`) receives this frame, resets the xterm instance, and writes the snapshot bytes — replacing the current full raw-byte replay for the visible screen. Ring-buffer replay still provides scrollback history (prepended before the snapshot, D-2).
- **Promote vt100 to `[dependencies]`:** Currently a `dev-dependency` (spike). Phase 1 integration moves it to the main dependency section in `Cargo.toml`.
- **~~Expose a configurable scrollback cap~~ — OBVIATED (Phase 1 implemented).** This follow-up assumed the per-session parser would hold scrollback. Phase 1 (D-2) constructs every parser with `vt100::Parser::new(rows, cols, 0)` — scrollback `0`. History comes exclusively from the existing byte ring; the parser only ever serializes the visible grid (`state_formatted()`). With no scrollback rows held, the 2.5 GB worst case is structurally impossible, so no `vt_scrollback` setting was added. (Spike `item1`/`item3` already verified scrollback `0` round-trips the visible screen.)

---

## Phase 1 implementation status (2026-06-24)

**Implemented** on branch `feat/vt-snapshot-reconnect` via subagent-driven-development (6 tasks, all review clean). Summary:
- vt100 v0.16.2 promoted to `[dependencies]`.
- New `src/pty/replay_state.rs`: `ReplayState` = byte ring + headless `vt100::Parser` (scrollback 0), fed the same bytes under one lock in `read_task`; `replay_since` attaches a `Some(snapshot)` iff the slice is full. Snapshot = `?1049h` (when on alt-screen) + `state_formatted()`.
- `SharedSession.replay_buf` → `replay_state: Arc<Mutex<ReplayState>>`; resize_task follows VT geometry.
- Server (`src/ws.rs`): on a full slice the WS sends a Text control frame `{"type":"snapshot"}` then one Binary frame `[8-byte be seq][filter(history ++ snapshot)]`; the old `{"type":"sync","mode":"full"}` frame is retired. Snapshot fires for any full slice (new connection `since=None` OR window-miss).
- Client (`terminal.js`): on `{"type":"snapshot"}` it `term.reset()`s then writes the bytes (history then clean redraw) — eliminating overlap (no dup), rebuilding scrollback from the ring (retention), stamping the authoritative viewport (claude bottom line). `GAP_MARKER`/`pendingReset` removed.
- e2e: raw-WS protocol assertion (the `{"type":"snapshot"}` + binary contract); throwaway `tests/vt_snapshot_spike.rs` removed.

**Decisive verification = iPad real device** (real claude, resize + reconnect: dup gone, retention up, claude bottom line present) — pending. Phase 2 (#3 reflow via `row_wrapped`) is next.
