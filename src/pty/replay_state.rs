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
}
