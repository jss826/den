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
        assert!(
            text.contains("TUI bottom line"),
            "last alt-screen row must survive"
        );
    }

    #[test]
    fn resize_changes_snapshot_geometry() {
        let mut rs = ReplayState::new(4096, 24, 80);
        rs.write(b"hi");
        rs.resize(40, 10); // cols=40, rows=10
        let snap = rs.replay_since(None).snapshot.unwrap();
        let mut p = vt100::Parser::new(10, 40, 0);
        p.process(&snap);
        assert_eq!(
            p.screen().size(),
            (10, 40),
            "snapshot must match resized geometry"
        );
    }

    // Reflow readiness (#3, Phase 2). A soft-wrapped logical line must be
    // serialized so the client records it as soft-wrapped and reflows it on a
    // later resize. `state_formatted()` already emits a wrapped line as one
    // continuous auto-wrapping run (no cursor-move between continuation rows),
    // so no custom serializer is needed; these tests guard that the snapshot
    // keeps that property. Proof is byte-level and renderer-independent: a fresh
    // parser fed the snapshot independently re-derives the wrap flags, which is
    // exactly the signal xterm.js uses to reflow.

    #[test]
    fn snapshot_preserves_soft_wrap_for_reflow() {
        let mut rs = ReplayState::new(4096, 24, 80);
        // 200 chars, no newline, on an 80-col screen wrap across 3 rows.
        rs.write(&vec![b'a'; 200]);
        let snap = rs.replay_since(None).snapshot.unwrap();

        let mut fresh = vt100::Parser::new(24, 80, 0);
        fresh.process(&snap);
        let screen = fresh.screen();
        assert!(screen.row_wrapped(0), "row 0 must stay soft-wrapped");
        assert!(screen.row_wrapped(1), "row 1 must stay soft-wrapped");
        assert!(
            !screen.row_wrapped(2),
            "logical-line end (row 2) must not be wrapped"
        );
    }

    #[test]
    fn snapshot_keeps_hard_newlines_unwrapped() {
        // Negative control: distinct hard-newline lines must not be merged into
        // a wrapped run, or unrelated lines would reflow together.
        let mut rs = ReplayState::new(4096, 24, 80);
        rs.write(b"alpha\r\nbeta\r\ngamma");
        let snap = rs.replay_since(None).snapshot.unwrap();

        let mut fresh = vt100::Parser::new(24, 80, 0);
        fresh.process(&snap);
        let screen = fresh.screen();
        assert!(!screen.row_wrapped(0), "hard line must not be wrapped");
        assert!(!screen.row_wrapped(1), "hard line must not be wrapped");
    }
}
