//! Terminal sequence filters shared between WebSocket and SSH paths.
//!
//! ConPTY emits private mode sequences and query sequences that cause problems
//! when forwarded to remote or browser-based terminals.  These filters strip
//! the problematic sequences from both the output path (PTY → client) and the
//! input path (client → PTY).

use std::borrow::Cow;

/// ConPTY private mode sequences to strip from output sent to terminals.
///
/// - `ESC[?9001h/l` — Win32 input mode: the client terminal does not understand
///   this, and enabling it would change all input to `CSI … _` format.
/// - `ESC[?1004h/l` — Focus reporting: the client terminal sends `ESC[I`/`ESC[O`
///   back as PTY input, causing stray characters in applications.
const CONPTY_BLOCKED_MODES: &[&[u8]] = &[
    b"\x1b[?9001h",
    b"\x1b[?9001l",
    b"\x1b[?1004h",
    b"\x1b[?1004l",
];

/// Strip ConPTY private mode sequences from PTY output before sending to the
/// client terminal (browser xterm.js or SSH client).
pub fn filter_conpty_private_modes(data: &[u8]) -> Cow<'_, [u8]> {
    // Fast path: no ESC → nothing to filter
    if !data.contains(&0x1b) {
        return Cow::Borrowed(data);
    }

    if !CONPTY_BLOCKED_MODES
        .iter()
        .any(|seq| data.windows(seq.len()).any(|w| w == *seq))
    {
        return Cow::Borrowed(data);
    }

    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;

    while i < data.len() {
        let remaining = &data[i..];
        if let Some(seq) = CONPTY_BLOCKED_MODES
            .iter()
            .find(|s| remaining.starts_with(s))
        {
            i += seq.len();
        } else {
            result.push(data[i]);
            i += 1;
        }
    }

    Cow::Owned(result)
}

/// Filter terminal response sequences from client input before forwarding to
/// the PTY.
///
/// When the replay buffer (or live output) contains terminal query sequences
/// (DSR, DA, DECRQSS, etc.), xterm.js / SSH client terminals respond to them.
/// These responses must be stripped to prevent them from appearing as literal
/// input in the shell.
///
/// CPR (`ESC[n;mR`) is kept because ConPTY needs it for cursor tracking.
/// Private-prefix CSI (DA, DECRQM, etc.), DCS, and OSC responses are removed.
pub fn filter_terminal_responses(data: &[u8]) -> Cow<'_, [u8]> {
    // Fast path: no ESC → nothing to filter
    if !data.contains(&0x1b) {
        return Cow::Borrowed(data);
    }

    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;

    while i < data.len() {
        if data[i] != 0x1b {
            result.push(data[i]);
            i += 1;
            continue;
        }

        // ESC found
        if i + 1 >= data.len() {
            // Trailing ESC → keep
            result.push(data[i]);
            i += 1;
            continue;
        }

        match data[i + 1] {
            b'[' => {
                // CSI sequence: ESC [
                let start = i;
                i += 2;

                // Private prefix: ? > =
                // Note: `<` is NOT included — SGR mouse reports use CSI < ... M/m
                let has_private_prefix =
                    i < data.len() && (data[i] == b'?' || data[i] == b'>' || data[i] == b'=');
                if has_private_prefix {
                    i += 1;
                }

                // Parameter bytes: 0x30-0x3F (digits, ;, :, etc.)
                while i < data.len() && (0x30..=0x3f).contains(&data[i]) {
                    i += 1;
                }

                // Intermediate bytes: 0x20-0x2F ($, !, ", space, etc.)
                while i < data.len() && (0x20..=0x2f).contains(&data[i]) {
                    i += 1;
                }

                // Final byte: 0x40-0x7E
                if i < data.len() && (0x40..=0x7e).contains(&data[i]) {
                    i += 1;

                    if has_private_prefix {
                        // Private prefix CSI → filter (DA, DECRQM, DECSET responses, etc.)
                        continue;
                    }

                    result.extend_from_slice(&data[start..i]);
                } else {
                    // Incomplete CSI → keep as-is
                    result.extend_from_slice(&data[start..i]);
                }
            }

            // DCS (ESC P), SOS (ESC X), PM (ESC ^), APC (ESC _)
            b'P' | b'X' | b'^' | b'_' => {
                let end = skip_string_sequence(data, i);
                if end > i {
                    i = end; // Terminated → filter
                } else {
                    // Unterminated → keep ESC, advance 1 (rest follows as plain bytes)
                    result.push(data[i]);
                    i += 1;
                }
            }

            // OSC (ESC ])
            b']' => {
                let end = skip_osc_sequence(data, i);
                if end > i {
                    i = end; // Terminated → filter
                } else {
                    // Unterminated → keep ESC, advance 1
                    result.push(data[i]);
                    i += 1;
                }
            }

            _ => {
                // Other ESC sequences (e.g. ESC O for SS3) → keep
                result.push(data[i]);
                i += 1;
            }
        }
    }

    if result.len() == data.len() {
        Cow::Borrowed(data)
    } else {
        Cow::Owned(result)
    }
}

/// Skip a ST-terminated string sequence (DCS, SOS, PM, APC).
fn skip_string_sequence(data: &[u8], start: usize) -> usize {
    let mut i = start + 2; // skip ESC + introducer
    while i < data.len() {
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
            return i + 2; // consume ST
        }
        i += 1;
    }
    // Unterminated → keep bytes as-is to avoid losing subsequent input
    start
}

/// Skip a BEL- or ST-terminated OSC sequence.
pub(crate) fn skip_osc_sequence(data: &[u8], start: usize) -> usize {
    let mut i = start + 2; // skip ESC ]
    while i < data.len() {
        if data[i] == 0x07 {
            return i + 1; // consume BEL
        }
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
            return i + 2; // consume ST
        }
        i += 1;
    }
    // Unterminated → keep bytes as-is to avoid losing subsequent input
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── filter_conpty_private_modes ─────────────────────────────

    #[test]
    fn modes_strip_win32_input() {
        assert!(filter_conpty_private_modes(b"\x1b[?9001h").is_empty());
        assert!(filter_conpty_private_modes(b"\x1b[?9001l").is_empty());
    }

    #[test]
    fn modes_strip_focus_reporting() {
        assert!(filter_conpty_private_modes(b"\x1b[?1004h").is_empty());
        assert!(filter_conpty_private_modes(b"\x1b[?1004l").is_empty());
    }

    #[test]
    fn modes_keep_other_sequences() {
        let data = b"\x1b[?25h"; // show cursor
        assert_eq!(filter_conpty_private_modes(data), &data[..]);
    }

    #[test]
    fn modes_mixed() {
        let data = b"\x1b[?9001h\x1b[?1004hHello\x1b[?25h";
        assert_eq!(filter_conpty_private_modes(data), &b"Hello\x1b[?25h"[..]);
    }

    #[test]
    fn modes_conpty_init() {
        let data = b"\x1b[?9001h\x1b[?1004h\x1b[6n";
        assert_eq!(filter_conpty_private_modes(data), &b"\x1b[6n"[..]);
    }

    #[test]
    fn modes_no_esc_fast_path() {
        let data = b"plain text";
        assert_eq!(filter_conpty_private_modes(data), &data[..]);
    }

    // ── filter_terminal_responses ───────────────────────────────

    #[test]
    fn resp_keep_cpr() {
        let data = b"\x1b[1;1R";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_keep_cpr_large() {
        let data = b"\x1b[24;80R";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_filter_da1() {
        assert!(filter_terminal_responses(b"\x1b[?1;2c").is_empty());
    }

    #[test]
    fn resp_filter_da2() {
        assert!(filter_terminal_responses(b"\x1b[>0;136;0c").is_empty());
    }

    #[test]
    fn resp_keep_arrow_keys() {
        let data = b"\x1b[A\x1b[B\x1b[C\x1b[D";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_keep_function_keys() {
        let data = b"\x1b[15~"; // F5
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_keep_plain_text() {
        let data = b"hello world";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_filter_da_mixed() {
        let data = b"\x1b[?1;2chello";
        assert_eq!(filter_terminal_responses(data), &b"hello"[..]);
    }

    #[test]
    fn resp_keep_cpr_filter_da() {
        let data = b"\x1b[24;80R\x1b[?1;2c";
        assert_eq!(filter_terminal_responses(data), &b"\x1b[24;80R"[..]);
    }

    #[test]
    fn resp_filter_decrqm() {
        assert!(filter_terminal_responses(b"\x1b[?1;1$y").is_empty());
    }

    #[test]
    fn resp_filter_dcs_xtversion() {
        assert!(filter_terminal_responses(b"\x1bP>|xterm(388)\x1b\\").is_empty());
    }

    #[test]
    fn resp_filter_dcs_decrqss() {
        assert!(filter_terminal_responses(b"\x1bP1$r0m\x1b\\").is_empty());
    }

    #[test]
    fn resp_filter_osc_bel() {
        assert!(filter_terminal_responses(b"\x1b]10;rgb:ff/ff/ff\x07").is_empty());
    }

    #[test]
    fn resp_filter_osc_st() {
        assert!(filter_terminal_responses(b"\x1b]11;rgb:00/00/00\x1b\\").is_empty());
    }

    #[test]
    fn resp_mixed_all() {
        // DA + DECRQM + CPR + DCS → CPR only
        let data = b"\x1b[?1;2c\x1b[?1;1$y\x1b[24;80R\x1bP>|term\x1b\\";
        assert_eq!(filter_terminal_responses(data), &b"\x1b[24;80R"[..]);
    }

    #[test]
    fn resp_keep_incomplete_csi() {
        let data = b"\x1b[1";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_keep_unterminated_dcs() {
        let data = b"\x1bPsome data without terminator";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_keep_unterminated_osc() {
        let data = b"\x1b]10;rgb:ff/ff/ff";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_keep_sgr_mouse() {
        let data = b"\x1b[<0;35;5M";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_keep_trailing_esc() {
        let data = b"hello\x1b";
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_keep_ss3() {
        let data = b"\x1bOP"; // SS3 F1
        assert_eq!(filter_terminal_responses(data), &data[..]);
    }

    #[test]
    fn resp_filter_dcs_with_text() {
        let data = b"before\x1bP>|ver\x1b\\after";
        assert_eq!(filter_terminal_responses(data), &b"beforeafter"[..]);
    }
}
