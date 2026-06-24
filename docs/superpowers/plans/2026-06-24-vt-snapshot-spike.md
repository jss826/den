# Terminal VT Snapshot Reconnect — Phase 0 Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Verify whether the `vt100` crate (fallback `avt`) can produce a faithful server-side terminal snapshot good enough to replace Den's raw-byte reconnect replay, and record the decisions (crate / scrollback strategy D-1 vs D-2 / soft-wrap reflow #3 feasibility) that Phase 1 depends on.

**Architecture:** This is a **throwaway spike**, not production wiring. A standalone Rust integration test (`tests/vt_snapshot_spike.rs`) feeds representative raw-PTY byte streams into a headless VT parser and inspects what it can reproduce. No `src/` integration happens in Phase 0 — that is Phase 1, planned in a follow-up once this spike resolves the open questions. The durable deliverable is a findings document; the test harness exists to produce evidence for it.

**Tech Stack:** Rust (edition 2024), `vt100` crate (dev-dependency), Den's existing test runner.

## Global Constraints

- Quality gates (run before each commit): `cargo fmt -- --check` / `cargo clippy -- -D warnings` / `cargo test --target-dir target-test`. Do NOT use `--all-targets` for clippy (a pre-existing lint at `store.rs:749` fails it — match the project gate exactly).
- `--target-dir target-test` is required for test runs so a running dev server's binary lock is avoided.
- New crate addition is gated on user consent; the spec approval (2026-06-24) covers adding `vt100` (or `avt` as fallback) for this spike. Add it as a **dev-dependency** in Phase 0 (the spike only needs it under `tests/`); Phase 1 will promote it to `[dependencies]` if it passes.
- Spike test code lives only under `tests/` and may use `unwrap()`/`expect()` freely (the no-unwrap rule is for production `src/`).
- The spike feeds bytes WITHOUT spawning a PTY, so the `tests/registry_test.rs` ConPTY rules (no `#[tokio::test]`, conhost zombies) do NOT apply here — these are plain `#[test]` functions with no tokio and no child process.
- Long `cargo` commands run via `run_in_background` and are read back with `TaskOutput` (per the bash-tool rules), since the dev server may be running.

## Scope note

This plan is **Phase 0 only**. It deliberately does not write any `src/` integration, protocol frames, or `terminal.js` changes — those are Phase 1/2 and cannot be specified until the spike below resolves: (a) which crate, (b) D-1 vs D-2 scrollback strategy, (c) whether #3 reflow is feasible. Task 6 ends with a decision gate that hands off to a Phase 1 planning session.

## File Structure

- **Modify:** `Cargo.toml` — add `vt100` under `[dev-dependencies]`.
- **Create:** `tests/vt_snapshot_spike.rs` — throwaway spike harness. One module, grouped by the spec's five verification items. Each test either asserts a capability (when the API is documented and we expect it) or prints an observation for the findings doc (when the question is open). Self-contained: no shared state with other test files.
- **Create:** `docs/superpowers/specs/2026-06-24-vt-snapshot-spike-findings.md` — the durable output: chosen crate, D-1/D-2 decision, #3 feasibility, CPU/mem estimate, and the go/no-go for Phase 1.

The spike harness mirrors how Den feeds the parser in production (`src/pty/registry.rs:595-628`): raw PTY bytes arrive in arbitrary ≤4096-byte chunks and are written to the replay buffer under a lock. Phase 1 will feed the same `&data` slice to a VT parser in that same locked section. The spike therefore must prove the parser survives **escape sequences split across chunk boundaries**, since 4096-byte reads cut sequences arbitrarily.

---

### Task 1: Add `vt100` and prove faithful visible-screen round-trip

This is the go/no-go for the crate. If `vt100` cannot reproduce a styled visible screen (colors, attributes, cursor position) — including when escape sequences are split across feed chunks — it fails the core requirement and Task 6 switches to `avt`.

**Files:**
- Modify: `Cargo.toml` (add `vt100` to `[dev-dependencies]`)
- Create: `tests/vt_snapshot_spike.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: a working spike harness file and a confirmed `vt100` parsing API for later tasks. The API this task pins down for Tasks 2–5: `vt100::Parser::new(rows: u16, cols: u16, scrollback_len: usize)`, `parser.process(&[u8])`, `parser.screen() -> &vt100::Screen`, `screen.contents_formatted() -> Vec<u8>`, `screen.contents() -> String`, `screen.cursor_position() -> (u16, u16)`, `screen.cell(row, col) -> Option<&vt100::Cell>`.

- [ ] **Step 1: Add the dev-dependency**

Run: `cargo add vt100 --dev`
Expected: `Cargo.toml` gains a `vt100` line under `[dev-dependencies]`; note the resolved version in the commit message.

- [ ] **Step 2: Write the failing round-trip test**

Create `tests/vt_snapshot_spike.rs`:

```rust
//! Phase 0 spike: verify a headless VT parser can faithfully snapshot a
//! terminal screen well enough to replace raw-byte reconnect replay.
//! THROWAWAY — delete or fold into Phase 1 once decisions are recorded in
//! docs/superpowers/specs/2026-06-24-vt-snapshot-spike-findings.md.

/// Feed `bytes` to a fresh parser one `chunk`-sized slice at a time, mirroring
/// Den's ≤4096-byte PTY reads (src/pty/registry.rs:595). Returns the parser so
/// callers can inspect the resulting screen.
fn feed_chunked(rows: u16, cols: u16, scrollback: usize, bytes: &[u8], chunk: usize) -> vt100::Parser {
    let mut parser = vt100::Parser::new(rows, cols, scrollback);
    for slice in bytes.chunks(chunk.max(1)) {
        parser.process(slice);
    }
    parser
}

#[test]
fn item1_visible_screen_roundtrip() {
    // A screen with text, a foreground color, and bold — the minimum a
    // faithful snapshot must preserve.
    let input = b"plain \x1b[31mred\x1b[0m \x1b[1mbold\x1b[0m done";

    // Snapshot the source screen.
    let src = feed_chunked(24, 80, 0, input, 4096);
    let snapshot = src.screen().contents_formatted();

    // Re-render the snapshot into a fresh parser. A faithful snapshot must
    // reproduce the same logical screen.
    let mut restored = vt100::Parser::new(24, 80, 0);
    restored.process(&snapshot);

    assert_eq!(
        restored.screen().contents(),
        src.screen().contents(),
        "snapshot text content must match the source screen"
    );
    assert_eq!(
        restored.screen().contents_formatted(),
        snapshot,
        "snapshot must be idempotent when re-rendered"
    );
}
```

- [ ] **Step 3: Run the test to verify it compiles and passes**

Run (background): `cargo test --target-dir target-test --test vt_snapshot_spike item1`
Expected: PASS. If it fails to compile, the assumed API (Step 2 interfaces) is wrong for the resolved version — adjust call sites to the actual `vt100` API and record the corrected signatures in the findings doc. If it compiles but the assertions fail, that is a finding (record it) — but `contents_formatted` round-tripping is documented behavior, so a failure here is a strong signal to fall back to `avt` in Task 6.

- [ ] **Step 4: Add the split-escape-across-chunks test**

Append to `tests/vt_snapshot_spike.rs`:

```rust
#[test]
fn item1_escape_split_across_chunks() {
    // Den reads in ≤4096-byte chunks, so an escape sequence can be cut in
    // half between process() calls. The parser MUST buffer the partial
    // sequence rather than print garbage.
    let input = b"a\x1b[31mb"; // SGR "\x1b[31m" will be split by chunk=2

    let chunked = feed_chunked(24, 80, 0, input, 2);
    let whole = feed_chunked(24, 80, 0, input, 4096);

    assert_eq!(
        chunked.screen().contents_formatted(),
        whole.screen().contents_formatted(),
        "splitting an escape sequence across feed chunks must not change the screen"
    );
    // Sanity: the SGR must not have leaked into visible text.
    assert!(
        !chunked.screen().contents().contains('['),
        "escape introducer must be consumed, not rendered"
    );
}
```

- [ ] **Step 5: Run to verify it passes**

Run (background): `cargo test --target-dir target-test --test vt_snapshot_spike item1_escape_split`
Expected: PASS. A failure means the parser does not buffer partial escapes across `process()` calls — a blocker for Den's chunked feed; record it and treat as crate-disqualifying in Task 6.

- [ ] **Step 6: Quality gate + commit**

Run: `cargo fmt -- --check` then `cargo clippy -- -D warnings`
Expected: clean.

```bash
git add Cargo.toml tests/vt_snapshot_spike.rs
git commit -m "spike: verify vt100 faithful visible-screen snapshot + chunk-split safety"
```

---

### Task 2: Verify alt-screen current-frame snapshot (claude/vim case)

The #1 symptom (claude's bottom line missing on reconnect) is an **alt-screen** problem: TUIs switch to the alternate screen buffer and the last frame must be snapshotted. This task confirms the parser tracks alt-screen entry and that `contents_formatted` reflects the *current* (alt) frame, including tolerance for claude's DEC synchronized-output wrapper (`?2026`).

**Files:**
- Modify: `tests/vt_snapshot_spike.rs`

**Interfaces:**
- Consumes: `feed_chunked` from Task 1.
- Produces: confirmation (recorded in findings) that alt-screen frames snapshot correctly — required for Phase 1 to claim the claude bottom-line fix.

- [ ] **Step 1: Write the alt-screen snapshot test**

Append to `tests/vt_snapshot_spike.rs`:

```rust
#[test]
fn item3_alt_screen_current_frame() {
    // Enter alt screen (?1049h), wrap a draw in DEC synchronized output
    // (?2026h ... ?2026l) the way claude does, draw a TUI frame whose LAST
    // line is the bit Den currently drops on reconnect.
    let mut input: Vec<u8> = Vec::new();
    input.extend_from_slice(b"main screen line\r\n");
    input.extend_from_slice(b"\x1b[?1049h"); // enter alternate screen
    input.extend_from_slice(b"\x1b[2J\x1b[H"); // clear + home
    input.extend_from_slice(b"\x1b[?2026h"); // begin synchronized update
    input.extend_from_slice(b"TUI top\r\n");
    input.extend_from_slice(b"\x1b[24;1HTUI bottom line"); // last row content
    input.extend_from_slice(b"\x1b[?2026l"); // end synchronized update

    let parser = feed_chunked(24, 80, 0, &input, 4096);
    let text = parser.screen().contents();

    // The snapshot must reflect the ALT screen (TUI), not the main screen.
    assert!(text.contains("TUI top"), "alt-screen content must be present");
    assert!(
        text.contains("TUI bottom line"),
        "the LAST alt-screen line (claude's missing bottom row) must be in the snapshot"
    );
    assert!(
        !text.contains("main screen line"),
        "main-screen content must NOT leak into the alt-screen snapshot"
    );

    // And the formatted snapshot must restore to the same screen.
    let snapshot = parser.screen().contents_formatted();
    let mut restored = vt100::Parser::new(24, 80, 0);
    restored.process(&snapshot);
    assert_eq!(restored.screen().contents(), text);
}
```

- [ ] **Step 2: Run to verify it passes**

Run (background): `cargo test --target-dir target-test --test vt_snapshot_spike item3_alt`
Expected: PASS. If the snapshot includes the main-screen line or drops the bottom row, record exactly what was produced — this is the central evidence for whether Phase 1 actually fixes the claude bottom-line bug.

- [ ] **Step 3: Record the synchronized-output observation**

In the findings doc (created in Task 6), note whether `?2026h/l` affected the result at all (vt100 may treat it as a no-op pass-through, which is fine — the final frame is what matters). No code change; this is an observation captured while the test is fresh.

- [ ] **Step 4: Quality gate + commit**

Run: `cargo fmt -- --check` then `cargo clippy -- -D warnings`
Expected: clean.

```bash
git add tests/vt_snapshot_spike.rs
git commit -m "spike: verify alt-screen current-frame snapshot incl. claude bottom line"
```

---

### Task 3: Probe scrollback serialization → decide D-1 vs D-2

`contents_formatted()` serializes only the **visible** screen. The spec's D-1 (seamless: snapshot includes history) requires the parser to also serialize **scrollback**; if it can't, Phase 1 falls back to D-2 (prepend the existing byte ring for history, VT snapshot for the visible screen). This task determines which.

**Files:**
- Modify: `tests/vt_snapshot_spike.rs`

**Interfaces:**
- Consumes: `feed_chunked` from Task 1.
- Produces: the **D-1 vs D-2 decision** for the findings doc and Phase 1.

- [ ] **Step 1: Write the scrollback probe**

Append to `tests/vt_snapshot_spike.rs`. This is an **investigation**, not a pass/fail assertion: it prints what the parser exposes so the decision can be recorded.

```rust
#[test]
fn item2_scrollback_serialization_probe() {
    // Push more lines than fit on screen so older lines go to scrollback.
    let mut input = String::new();
    for i in 0..50 {
        input.push_str(&format!("line {i}\r\n"));
    }
    // Give the parser a scrollback budget (matches Den's xterm scrollback = 5000).
    let parser = feed_chunked(24, 80, 5000, input.as_bytes(), 4096);
    let screen = parser.screen();

    // (a) Does contents_formatted include scrollback, or only the visible 24 rows?
    let formatted = screen.contents_formatted();
    let mut restored = vt100::Parser::new(24, 80, 5000);
    restored.process(&formatted);
    let restored_text = restored.screen().contents();
    let restored_has_old = restored_text.contains("line 0");

    // (b) Can we reach scrollback at all to serialize it ourselves (D-1 via
    //     manual walk)? vt100 exposes set_scrollback(n) to scroll the VIEW back.
    //     Walk the view back and collect any rows above the live screen.
    // NOTE: the exact API may differ by version; if set_scrollback / cell access
    // is not available, that itself is the finding (→ D-2). Adjust to the real
    // API and record it.
    println!("SCROLLBACK PROBE:");
    println!("  contents_formatted restores old 'line 0'? {restored_has_old}");
    println!("  visible screen text:\n{}", screen.contents());

    // ASSERT the part that MUST hold regardless of the D-1/D-2 outcome: the
    // VISIBLE screen always round-trips faithfully. "line 49" is the last line
    // written and must survive snapshot → restore.
    assert!(
        restored_text.contains("line 49"),
        "the visible screen must round-trip: last line 'line 49' missing after restore"
    );
    // The UNKNOWN (does scrollback serialize → D-1 vs D-2) stays as printed
    // evidence above; `restored_has_old` is the boolean to record in findings.
    // It is intentionally NOT asserted — its value IS the open question.
    let _ = restored_has_old;
}
```

- [ ] **Step 2: Run with output captured**

Run (background): `cargo test --target-dir target-test --test vt_snapshot_spike item2_scrollback -- --nocapture`
Expected: PASS, and the `SCROLLBACK PROBE:` block printed. Read the output via `TaskOutput`.

- [ ] **Step 3: Decide D-1 vs D-2 from the evidence**

- If `contents_formatted` restored `line 0` (scrollback is included or reconstructable) → **D-1 is viable**.
- If only the visible 24 rows survived AND no API exposes scrollback rows for serialization → **D-2** (Phase 1 prepends the byte ring for history). Confirm whether `vt100::Screen` offers any scrollback row/cell accessor; if `set_scrollback` exists but only scrolls the view without exposing rows for export, that is still effectively D-2 unless rows can be read at each scroll offset.

Record the conclusion (and the precise reason) in the findings doc. No code beyond the probe.

- [ ] **Step 4: Quality gate + commit**

Run: `cargo fmt -- --check` then `cargo clippy -- -D warnings`
Expected: clean. (If the probe prints unused-variable warnings, prefix with `_` or `let _ =` to satisfy `-D warnings`.)

```bash
git add tests/vt_snapshot_spike.rs
git commit -m "spike: probe vt100 scrollback serialization (D-1 vs D-2 decision)"
```

---

### Task 4: Probe soft-wrap info → decide #3 (reflow) feasibility

Phase 2 (#3 reflow) is only attempted if the snapshot can carry **logical-line / autowrap** information so xterm can recompute wrapping on resize. This task determines whether `vt100` exposes whether a row was wrapped (continued) vs hard-newline-terminated.

**Files:**
- Modify: `tests/vt_snapshot_spike.rs`

**Interfaces:**
- Consumes: `feed_chunked` from Task 1.
- Produces: the **#3 feasibility decision** for the findings doc.

- [ ] **Step 1: Write the wrap-info probe**

Append to `tests/vt_snapshot_spike.rs`:

```rust
#[test]
fn item4_wrap_info_probe() {
    // Write a single logical line longer than the 80-col width so it soft-wraps.
    let long: String = "X".repeat(100); // 100 chars on an 80-col screen → wraps
    let parser = feed_chunked(24, 80, 0, long.as_bytes(), 4096);
    let screen = parser.screen();

    // Question: can we tell that row 0 was soft-wrapped into row 1 (vs a hard
    // newline)? Try whatever the version exposes — a row `wrapped()` flag, or a
    // cell-level continuation marker. If NEITHER exists, #3 is NOT feasible with
    // this crate and Phase 2 is dropped (or avt is considered ONLY if D-1/#3
    // both matter enough; otherwise D-2 + no-reflow is acceptable per spec §5).
    let row0 = row_text(screen, 0);
    let row1 = row_text(screen, 1);
    println!("WRAP PROBE:");
    println!("  row0: {row0:?}");
    println!("  row1: {row1:?}");
    // Record here whether a wrapped/continuation accessor was found in the API.

    // ASSERT the part that MUST hold: a 100-char logical line on an 80-col
    // screen physically occupies row 0 (full) and overflows onto row 1. This
    // proves the content is present across rows; whether the crate EXPOSES a
    // "wrapped" flag (the #3-deciding unknown) stays a recorded finding.
    assert_eq!(row0.trim_end().len(), 80, "row 0 must be full of the wrapped line");
    assert!(
        row1.starts_with('X'),
        "the 81st+ chars must overflow onto row 1 (soft-wrap), got {row1:?}"
    );
}

/// Collect the visible text of one row, cell by cell.
fn row_text(screen: &vt100::Screen, row: u16) -> String {
    let (_, cols) = screen.size();
    (0..cols)
        .filter_map(|c| screen.cell(row, c).map(|cell| cell.contents()))
        .collect()
}
```

- [ ] **Step 2: Run with output captured**

Run (background): `cargo test --target-dir target-test --test vt_snapshot_spike item4_wrap -- --nocapture`
Expected: PASS, `WRAP PROBE:` printed. Inspect via `TaskOutput`, and inspect the `vt100` docs/source for any `wrapped()` / continuation accessor on the row or cell.

- [ ] **Step 3: Decide #3 feasibility**

- If a wrapped/continuation flag is available → **#3 feasible**, Phase 2 stays on the roadmap (Phase 1 still ships first).
- If not → record **#3 not feasible with vt100**. Per spec §5, Phase 1 (#1) still delivers the core value; reflow of restored history is simply not attempted. Note whether `avt` (reflow is its specialty) would be worth a separate spike IF the user later prioritizes #3.

Record in the findings doc.

- [ ] **Step 4: Quality gate + commit**

Run: `cargo fmt -- --check` then `cargo clippy -- -D warnings`
Expected: clean.

```bash
git add tests/vt_snapshot_spike.rs
git commit -m "spike: probe vt100 soft-wrap info (#3 reflow feasibility)"
```

---

### Task 5: Estimate double-processing CPU/memory cost

Phase 1 feeds every PTY chunk to BOTH the ring buffer and the VT parser. This task estimates the added cost so we know it is acceptable at Den's `MAX_SESSIONS = 50`.

**Files:**
- Modify: `tests/vt_snapshot_spike.rs`

**Interfaces:**
- Consumes: `feed_chunked` (or an inline loop) from Task 1.
- Produces: a per-session and 50-session CPU/memory estimate for the findings doc.

- [ ] **Step 1: Write the throughput probe**

Append to `tests/vt_snapshot_spike.rs`:

```rust
#[test]
fn item5_double_processing_cost() {
    use std::time::Instant;

    // Simulate a busy session: ~8 MB of mixed text + SGR churn, fed in 4096-byte
    // chunks like the real read loop.
    let mut blob: Vec<u8> = Vec::with_capacity(8 * 1024 * 1024);
    while blob.len() < 8 * 1024 * 1024 {
        blob.extend_from_slice(b"\x1b[32msome colored output line with text\x1b[0m\r\n");
    }

    let mut parser = vt100::Parser::new(24, 80, 5000);
    let start = Instant::now();
    for slice in blob.chunks(4096) {
        parser.process(slice);
    }
    let elapsed = start.elapsed();

    let mb = blob.len() as f64 / (1024.0 * 1024.0);
    let mb_per_s = mb / elapsed.as_secs_f64();
    println!("COST PROBE:");
    println!("  processed {mb:.1} MB in {elapsed:?} → {mb_per_s:.0} MB/s per session");
    println!("  snapshot size: {} bytes", parser.screen().contents_formatted().len());
    println!("  (parser holds 24x80 grid + up to 5000 scrollback rows)");

    // Sanity floor so the test fails loudly if the parser is pathologically slow
    // (e.g. < 5 MB/s would make double-processing untenable). Generous bound.
    assert!(mb_per_s > 5.0, "VT parsing too slow for double-processing: {mb_per_s:.0} MB/s");
}
```

- [ ] **Step 2: Run with output captured**

Run (background): `cargo test --target-dir target-test --test vt_snapshot_spike item5 -- --nocapture`
Expected: PASS with `COST PROBE:` printed. Read via `TaskOutput`. Run on the dev machine (Windows) so the estimate reflects the real target.

- [ ] **Step 3: Record the estimate**

In the findings doc, capture MB/s, snapshot byte size, and a back-of-envelope 50-session worst case (e.g. throughput × concurrent busy sessions, plus per-parser memory ≈ grid + 5000-row scrollback). Note if coalescing VT feeds would be needed (spec §9 says initial design has none).

- [ ] **Step 4: Quality gate + commit**

Run: `cargo fmt -- --check` then `cargo clippy -- -D warnings`
Expected: clean.

```bash
git add tests/vt_snapshot_spike.rs
git commit -m "spike: estimate VT double-processing CPU/memory cost"
```

---

### Task 6: Write findings doc + Phase 1 decision gate

Consolidate all evidence into the durable findings document and make the four decisions Phase 1 depends on. If any crate-disqualifying result appeared (Task 1 round-trip or chunk-split failed, or Task 2 alt-screen dropped the bottom line), this task switches the candidate to `avt` and re-runs Tasks 1–4 before finalizing.

**Files:**
- Create: `docs/superpowers/specs/2026-06-24-vt-snapshot-spike-findings.md`

**Interfaces:**
- Consumes: printed output and pass/fail from Tasks 1–5.
- Produces: the go/no-go and parameters for the Phase 1 plan (crate, D-1/D-2, #3 yes/no, cost verdict).

- [ ] **Step 1: Write the findings document**

Create `docs/superpowers/specs/2026-06-24-vt-snapshot-spike-findings.md` with these sections, filled from the test evidence (no placeholders — paste the actual observed values):

```markdown
# VT Snapshot Spike — Findings (2026-06-24)

Spike for spec `2026-06-24-terminal-vt-snapshot-reconnect-design.md`.
Harness: `tests/vt_snapshot_spike.rs` (throwaway).

## Decisions
- **Crate:** vt100 vX.Y  (or: avt vX.Y — switched because <reason>)
- **Scrollback strategy:** D-1 (seamless) | D-2 (byte-ring prepend) — because <evidence>
- **#3 reflow feasible:** yes | no — because <wrap-info evidence>
- **Cost verdict:** <MB/s> per session; 50-session worst case <estimate>; coalescing needed? yes/no

## Evidence (per spec §7 items)
1. Visible-screen round-trip + chunk-split safety: PASS/FAIL + notes
2. Scrollback serialization: <what contents_formatted restored; API available?>
3. Alt-screen / claude bottom line: PASS/FAIL + exact snapshot observation
4. Soft-wrap info: <accessor found? name; or none>
5. Double-processing cost: <numbers>

## Go/No-Go for Phase 1
- Phase 1 (#1 reconnect clean-up): GO/NO-GO + which strategy (D-1/D-2)
- Phase 2 (#3 reflow): ON ROADMAP / DROPPED

## Follow-ups for the Phase 1 plan
- VT parser hook point: `src/pty/registry.rs` read_task (line ~595-628),
  fed `&data` in the same locked section as `replay_buf.write`, atomic with seq.
- Snapshot API: atomically return (contents_formatted bytes, total_written seq)
  under one lock, per spec §4 step 2.
- New protocol frame `{"type":"snapshot"}` + client reset→write in terminal.js.
- Promote vt100 to `[dependencies]`.
```

- [ ] **Step 2: If a crate-disqualifying result appeared, switch to `avt` and re-run**

Only if Task 1 round-trip/chunk-split FAILED or Task 2 alt-screen FAILED:
- Run `cargo remove vt100 --dev` then `cargo add avt --dev`.
- Adapt the Task 1–4 tests to the `avt` API (its screen/snapshot accessors differ; `avt` is asciinema's reflow-oriented VT).
- Re-run Tasks 1–4 and refill the findings doc with `avt` results.
- If `avt` also fails the core round-trip, STOP and escalate to the user: the additive VT-snapshot approach is not viable with either candidate (spec §9 max risk realized).

If no disqualifying result appeared, skip this step.

- [ ] **Step 3: Commit the findings**

```bash
git add docs/superpowers/specs/2026-06-24-vt-snapshot-spike-findings.md
git commit -m "spike: record VT snapshot findings + Phase 1 decisions"
```

- [ ] **Step 4: Decision gate — hand off to Phase 1 planning**

Report to the user: chosen crate, D-1/D-2, #3 yes/no, cost verdict, and GO/NO-GO. On GO, start a follow-up `writing-plans` session to produce the Phase 1 implementation plan (VT parser integration in `registry.rs`, atomic snapshot+seq API, `{"type":"snapshot"}` protocol frame, `terminal.js` reset→write, promote crate to `[dependencies]`). Do NOT begin Phase 1 coding inside this spike branch without that plan.

---

## Self-Review

**Spec coverage (against `2026-06-24-terminal-vt-snapshot-reconnect-design.md` §7 Phase 0):**
- §7 item 1 (faithful visible re-render: colors/attrs/cursor) → Task 1.
- §7 item 2 (scrollback serialization → D-1/D-2) → Task 3.
- §7 item 3 (alt-screen current frame) → Task 2.
- §7 item 4 (wrap info → #3) → Task 4.
- §7 item 5 (CPU/mem of double-processing, 50 sessions) → Task 5.
- §7 "new crate added this phase" → Task 1 Step 1 (vt100 dev-dep), with avt fallback in Task 6 Step 2.
- §7 "deliverable: findings doc (crate / D-1 or D-2 / #3)" → Task 6.
- Chunk-boundary robustness (not an explicit §7 item but required by Den's 4096-byte feed, registry.rs:596) → Task 1 Step 4.

Phase 1 / Phase 2 of the spec are intentionally NOT covered here — they are gated on this spike's output and get their own plan (Task 6 Step 4 hands off).

**Placeholder scan:** The findings-doc template (Task 6 Step 1) contains `<...>` fill-ins by design — those are the spike's *outputs*, recorded at execution time from real evidence, not plan placeholders. All test code is complete and runnable.

**Type/name consistency:** `feed_chunked(rows, cols, scrollback, bytes, chunk)` defined in Task 1 is used unchanged in Tasks 2–5. `row_text(screen, row)` defined in Task 4. Parser API (`process`, `screen`, `contents_formatted`, `contents`, `cursor_position`, `cell`, `size`) used consistently; Task 1 Step 3 explicitly flags that mismatches with the resolved `vt100` version must be corrected at first compile and the real signatures recorded.
