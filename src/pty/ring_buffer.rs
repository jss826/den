/// リプレイ片: クライアントへ送るデータと、その性質を表す。
pub struct ReplaySlice {
    /// 送出するバイト列（古い順）。
    pub data: Vec<u8>,
    /// true の場合、クライアントは適用前に term をリセットすべき
    /// （新規接続、またはクライアントがバッファ窓より後れて差分を出せないとき）。
    pub full: bool,
    /// `data` の末尾に対応する絶対シーケンス（= これまでに書き込まれた総バイト数）。
    pub end_seq: u64,
}

/// 固定容量のリングバッファ（リプレイ用）
///
/// `total_written` で「これまでに書き込まれた総バイト数」を絶対シーケンスとして
/// 追跡する。クライアントは最後に受信した seq を覚えておき、再接続時に
/// `replay_since(Some(seq))` で差分のみを受け取ることで、重複なく復帰できる。
pub struct RingBuffer {
    buf: Vec<u8>,
    write_pos: usize,
    len: usize,
    /// これまでに write された総バイト数（絶対シーケンス）。
    total_written: u64,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0u8; capacity],
            write_pos: 0,
            len: 0,
            total_written: 0,
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        // total_written は容量に関係なく常に進める（seq の連続性のため）。
        self.total_written = self.total_written.saturating_add(data.len() as u64);

        let cap = self.buf.len();
        if cap == 0 {
            return;
        }

        for &byte in data {
            self.buf[self.write_pos] = byte;
            self.write_pos = (self.write_pos + 1) % cap;
        }
        self.len = (self.len + data.len()).min(cap);
    }

    /// これまでに書き込まれた総バイト数（絶対シーケンス）。
    pub fn total_written(&self) -> u64 {
        self.total_written
    }

    /// バッファ内のデータを古い順に返す
    pub fn read_all(&self) -> Vec<u8> {
        if self.len == 0 {
            return Vec::new();
        }

        let cap = self.buf.len();
        let start = if self.len < cap {
            0
        } else {
            self.write_pos // write_pos が最も古いデータの位置
        };

        let mut result = Vec::with_capacity(self.len);
        // 2 スライスコピー: リングバッファの連続領域を直接 extend
        let first_len = (cap - start).min(self.len);
        result.extend_from_slice(&self.buf[start..start + first_len]);
        if first_len < self.len {
            result.extend_from_slice(&self.buf[..self.len - first_len]);
        }
        result
    }

    /// 末尾 n バイトを古い順に返す（`n <= len` 前提、呼び出し側が保証する）。
    fn read_last(&self, n: usize) -> Vec<u8> {
        if n == 0 {
            return Vec::new();
        }
        let cap = self.buf.len();
        // 末尾 n バイトは [write_pos - n, write_pos)（mod cap）の範囲。
        let start = (self.write_pos + cap - n) % cap;
        let mut result = Vec::with_capacity(n);
        let first_len = (cap - start).min(n);
        result.extend_from_slice(&self.buf[start..start + first_len]);
        if first_len < n {
            result.extend_from_slice(&self.buf[..n - first_len]);
        }
        result
    }

    /// クライアントの `since` シーケンスに基づくリプレイ片を返す。
    ///
    /// - `since = Some(s)` かつ `s` がバッファ窓 `[oldest_seq, total_written]` 内:
    ///   `[s, total_written)` の差分のみを `full = false` で返す（重複なし）。
    /// - それ以外（新規接続 = None、または窓より後れた s）:
    ///   バッファ全体を `full = true` で返す。クライアントは適用前にリセットする。
    ///   全体リプレイは先頭の壊れたエスケープシーケンスを避けるため行境界に揃える。
    pub fn replay_since(&self, since: Option<u64>) -> ReplaySlice {
        let end = self.total_written;
        let oldest = end - self.len as u64; // len <= total_written なので安全

        if let Some(s) = since
            && s >= oldest
            && s <= end
        {
            // 差分: バッファ末尾の (end - s) バイトのみコピー（read_all を避ける）。
            // ライブ経路が毎回呼ぶため、64KB 全コピーにならないようにする。
            let take = (end - s) as usize;
            return ReplaySlice {
                data: self.read_last(take),
                full: false,
                end_seq: end,
            };
        }

        // 全体リプレイ: 先頭を行境界に揃える（バッファが一周している場合のみ意味を持つ）。
        // `len == cap` ではなく `total_written > cap` で判定する。前者はちょうど満杯
        // （total_written == cap, 先頭がまだ正規データ）でも真になり、正当な先頭行を
        // 誤って捨ててしまうため。
        let all = self.read_all();
        let data = if self.total_written > self.buf.len() as u64 {
            trim_to_line_start(all)
        } else {
            all
        };
        ReplaySlice {
            data,
            full: true,
            end_seq: end,
        }
    }
}

/// 先頭の部分行（最初の改行より前）を捨てて行境界から始める。
/// 一周したリングバッファの先頭は途中のエスケープ/マルチバイト境界になりがちで、
/// xterm に渡すと再同期するまで化けるため。改行が無ければそのまま返す。
fn trim_to_line_start(data: Vec<u8>) -> Vec<u8> {
    match data.iter().position(|&b| b == b'\n') {
        Some(nl) if nl + 1 < data.len() => data[nl + 1..].to_vec(),
        _ => data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer() {
        let buf = RingBuffer::new(64);
        assert!(buf.read_all().is_empty());
    }

    #[test]
    fn simple_write_read() {
        let mut buf = RingBuffer::new(64);
        buf.write(b"hello");
        assert_eq!(buf.read_all(), b"hello");
    }

    #[test]
    fn wrap_around() {
        let mut buf = RingBuffer::new(8);
        buf.write(b"12345678"); // 満杯
        buf.write(b"AB"); // 先頭を上書き
        assert_eq!(buf.read_all(), b"345678AB");
    }

    #[test]
    fn multiple_writes() {
        let mut buf = RingBuffer::new(16);
        buf.write(b"aaa");
        buf.write(b"bbb");
        assert_eq!(buf.read_all(), b"aaabbb");
    }

    #[test]
    fn overwrite_multiple_times() {
        let mut buf = RingBuffer::new(4);
        buf.write(b"abcdef"); // 4バイトを超える
        assert_eq!(buf.read_all(), b"cdef");
        buf.write(b"gh");
        assert_eq!(buf.read_all(), b"efgh");
    }

    #[test]
    fn zero_capacity() {
        let mut buf = RingBuffer::new(0);
        buf.write(b"test");
        assert!(buf.read_all().is_empty());
    }

    // ── seq tracking / replay_since ──────────────────────────────

    #[test]
    fn total_written_counts_all_bytes_even_when_wrapped() {
        let mut buf = RingBuffer::new(4);
        buf.write(b"abcdef"); // 6 bytes, buffer holds last 4
        assert_eq!(buf.total_written(), 6);
        assert_eq!(buf.read_all(), b"cdef");
    }

    #[test]
    fn replay_since_returns_delta_when_in_window() {
        let mut buf = RingBuffer::new(64);
        buf.write(b"hello");
        buf.write(b"world"); // total = 10
        // Client already has the first 5 bytes ("hello"), wants the rest.
        let r = buf.replay_since(Some(5));
        assert!(!r.full);
        assert_eq!(r.end_seq, 10);
        assert_eq!(r.data, b"world");
    }

    #[test]
    fn replay_since_caught_up_returns_empty_delta() {
        let mut buf = RingBuffer::new(64);
        buf.write(b"hello");
        let r = buf.replay_since(Some(5)); // client is fully caught up
        assert!(!r.full);
        assert_eq!(r.end_seq, 5);
        assert!(r.data.is_empty());
    }

    #[test]
    fn replay_since_none_returns_full() {
        let mut buf = RingBuffer::new(64);
        buf.write(b"hello");
        let r = buf.replay_since(None);
        assert!(r.full);
        assert_eq!(r.end_seq, 5);
        assert_eq!(r.data, b"hello");
    }

    #[test]
    fn replay_since_too_old_falls_back_to_full() {
        let mut buf = RingBuffer::new(4);
        buf.write(b"abcdefgh"); // total = 8, window covers [4, 8] = "efgh"
        // Client's seq (2) is older than the oldest retained byte (4) → full.
        let r = buf.replay_since(Some(2));
        assert!(r.full);
        assert_eq!(r.end_seq, 8);
    }

    #[test]
    fn replay_since_delta_after_wrap() {
        let mut buf = RingBuffer::new(8);
        buf.write(b"12345678"); // total = 8, window [0,8]
        buf.write(b"AB"); // total = 10, window [2,10] = "345678AB"
        // Client had up to seq 8, wants the new "AB".
        let r = buf.replay_since(Some(8));
        assert!(!r.full);
        assert_eq!(r.end_seq, 10);
        assert_eq!(r.data, b"AB");
    }

    #[test]
    fn full_replay_trims_to_line_boundary_when_wrapped() {
        // Capacity 8, wrapped so the head is a partial line "cde" before the \n.
        let mut buf = RingBuffer::new(8);
        buf.write(b"abcde\nXY"); // exactly fills: "abcde\nXY"
        buf.write(b"Z"); // wraps: drops 'a' → "bcde\nXYZ"
        let r = buf.replay_since(None);
        assert!(r.full);
        // Head partial line "bcde" trimmed; starts after the first newline.
        assert_eq!(r.data, b"XYZ");
    }

    #[test]
    fn full_replay_no_trim_when_not_wrapped() {
        let mut buf = RingBuffer::new(64);
        buf.write(b"abc\ndef");
        let r = buf.replay_since(None);
        // Not wrapped → head is authentic, no trimming.
        assert_eq!(r.data, b"abc\ndef");
    }

    #[test]
    fn full_replay_no_trim_when_exactly_full_but_not_wrapped() {
        // Exactly fills the buffer (total_written == cap, write_pos back to 0)
        // but has NOT wrapped: the head is still the authentic first byte.
        // len == cap is true here, so the old check wrongly trimmed the head.
        let mut buf = RingBuffer::new(8);
        buf.write(b"abcde\nXY"); // total = 8 == cap, no overwrite yet
        let r = buf.replay_since(None);
        assert!(r.full);
        assert_eq!(r.end_seq, 8);
        // First line must be preserved — nothing has been overwritten.
        assert_eq!(r.data, b"abcde\nXY");
    }
}
