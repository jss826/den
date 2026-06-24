//! Phase 0 spike: verify a headless VT parser can faithfully snapshot a
//! terminal screen well enough to replace raw-byte reconnect replay.
//! THROWAWAY — delete or fold into Phase 1 once decisions are recorded in
//! docs/superpowers/specs/2026-06-24-vt-snapshot-spike-findings.md.

/// Feed `bytes` to a fresh parser one `chunk`-sized slice at a time, mirroring
/// Den's ≤4096-byte PTY reads (src/pty/registry.rs:595). Returns the parser so
/// callers can inspect the resulting screen.
fn feed_chunked(
    rows: u16,
    cols: u16,
    scrollback: usize,
    bytes: &[u8],
    chunk: usize,
) -> vt100::Parser {
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

    // The red cell at column 6 ("red") must carry fg=red after snapshot→restore.
    // Input "plain \x1b[31mred...": p-l-a-i-n-space = cols 0..5, so 'r' is col 6.
    let cell = restored.screen().cell(0, 6).expect("cell (0,6) exists");
    assert_eq!(
        cell.fgcolor(),
        vt100::Color::Idx(1),
        "SGR 31 → fg red must survive snapshot"
    );
}

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
    assert!(
        text.contains("TUI top"),
        "alt-screen content must be present"
    );
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
    // At this point scrollback_offset == 0 (normal view), so contents_formatted
    // captures only the live viewport (lines 26-49 approximately).
    let formatted = screen.contents_formatted();
    let mut restored = vt100::Parser::new(24, 80, 5000);
    restored.process(&formatted);
    let restored_text = restored.screen().contents();
    let restored_has_old = restored_text.contains("line 0");

    // (b) Probe whether set_scrollback lets us READ scrollback rows.
    // Real v0.16.2 API (verified from vt100-0.16.2/src/screen.rs + grid.rs):
    //   screen.set_scrollback(n: usize)   — shifts visible_rows() view back by n
    //                                        rows (clamped to actual scrollback len)
    //   screen.scrollback() -> usize       — current offset (0 = live viewport)
    //   screen.contents() / contents_formatted() — reflect the scrolled view
    //   screen.cell(row, col)              — returns cell at visible_rows()[row][col]
    // NOTE: Parser<CB> does NOT implement Clone; we re-feed from input bytes instead.
    let scrollback_size = screen.scrollback(); // 0 before any set_scrollback call

    // How many scrollback rows does the parser hold?
    // Re-feed into a fresh parser and scroll back as far as possible (clamped).
    let mut probe = feed_chunked(24, 80, 5000, input.as_bytes(), 4096);
    probe.screen_mut().set_scrollback(usize::MAX); // clamped to actual len
    let actual_scrollback_rows = probe.screen().scrollback();

    // Read the "oldest" page in scrollback.
    let scrollback_page_text = probe.screen().contents();
    let scrollback_has_line0 = scrollback_page_text.contains("line 0");

    // Can we reconstruct all scrollback rows by walking the offset?
    // Each set_scrollback(n) shifts visible_rows() so row 0 == scrollback[offset].
    // Walking in steps of `rows` height lets us tile all scrollback into snapshots.
    let (rows, _cols) = screen.size();
    let rows = usize::from(rows);
    let mut all_scrollback_text = String::new();
    let mut offset = actual_scrollback_rows;
    loop {
        let mut p2 = feed_chunked(24, 80, 5000, input.as_bytes(), 4096);
        p2.screen_mut().set_scrollback(offset);
        all_scrollback_text.push_str(&p2.screen().contents());
        all_scrollback_text.push('\n');
        if offset < rows {
            break;
        }
        offset = offset.saturating_sub(rows);
        if offset == 0 {
            // one last pass at offset 0 = live screen
            break;
        }
    }
    let walk_has_line0 = all_scrollback_text.contains("line 0");

    println!("SCROLLBACK PROBE:");
    println!("  contents_formatted (offset=0) restores old 'line 0'? {restored_has_old}");
    println!("  scrollback_offset before set_scrollback: {scrollback_size}");
    println!(
        "  actual scrollback rows held by parser (clamped usize::MAX): {actual_scrollback_rows}"
    );
    println!("  scrollback page at max offset contains 'line 0'? {scrollback_has_line0}");
    println!("  walking all offsets in screen-height steps contains 'line 0'? {walk_has_line0}");
    println!("  visible screen text (last ~24 lines):");
    println!("{}", screen.contents());

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
    let _ = walk_has_line0;
    let _ = scrollback_has_line0;
}

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
    println!(
        "  snapshot size: {} bytes",
        parser.screen().contents_formatted().len()
    );
    println!("  (parser holds 24x80 grid + up to 5000 scrollback rows)");

    // Sanity floor so the test fails loudly if the parser is pathologically slow
    // (e.g. < 5 MB/s would make double-processing untenable). Generous bound.
    #[cfg(debug_assertions)]
    const FLOOR: f64 = 0.5; // debug is ~30x slower than release; any forward progress is fine
    #[cfg(not(debug_assertions))]
    const FLOOR: f64 = 5.0; // production-meaningful: double-processing is untenable below this
    assert!(
        mb_per_s > FLOOR,
        "VT parsing too slow for double-processing: {mb_per_s:.0} MB/s"
    );
}

/// Collect the visible text of one row, cell by cell.
fn row_text(screen: &vt100::Screen, row: u16) -> String {
    let (_, cols) = screen.size();
    (0..cols)
        .filter_map(|c| screen.cell(row, c).map(|cell| cell.contents()))
        .collect()
}

#[test]
fn item4_wrap_info_probe() {
    // Write a single logical line longer than the 80-col width so it soft-wraps.
    let long: String = "X".repeat(100); // 100 chars on an 80-col screen → wraps
    let parser = feed_chunked(24, 80, 0, long.as_bytes(), 4096);
    let screen = parser.screen();

    let row0 = row_text(screen, 0);
    let row1 = row_text(screen, 1);

    // Does vt100 v0.16.2 expose a soft-wrap flag?
    // FINDING: Screen::row_wrapped(row: u16) -> bool is a public method
    // (screen.rs:540). It delegates to the internal Row::wrapped() (row.rs:82).
    // Row itself is not publicly exported, but the Screen-level accessor IS.
    // This is the authoritative public API for querying soft-wrap status.
    let row0_wrapped = screen.row_wrapped(0);
    let row1_wrapped = screen.row_wrapped(1);

    println!("WRAP PROBE:");
    println!("  row0: {row0:?}");
    println!("  row1: {row1:?}");
    println!("  screen.row_wrapped(0) = {row0_wrapped}  ← FINDING: public API exists");
    println!("  screen.row_wrapped(1) = {row1_wrapped}");
    println!("  API: Screen::row_wrapped(row: u16) -> bool  [screen.rs:540]");
    println!("  Row::wrapped() -> bool also exists but Row is not pub-exported [row.rs:82]");
    println!("  #3 FEASIBILITY VERDICT: FEASIBLE — wrap flag is available via public API");

    // ASSERT the part that MUST hold: a 100-char logical line on an 80-col
    // screen physically occupies row 0 (full) and overflows onto row 1.
    assert_eq!(
        row0.trim_end().len(),
        80,
        "row 0 must be full of the wrapped line"
    );
    assert!(
        row1.starts_with('X'),
        "the 81st+ chars must overflow onto row 1 (soft-wrap), got {row1:?}"
    );
    // ASSERT: row 0 must report as wrapped (the defining observable for #3).
    assert!(
        row0_wrapped,
        "screen.row_wrapped(0) must be true for a soft-wrapped row"
    );
}
