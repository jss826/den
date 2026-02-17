/// 固定容量のリングバッファ（リプレイ用）
pub struct RingBuffer {
    buf: Vec<u8>,
    write_pos: usize,
    len: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0u8; capacity],
            write_pos: 0,
            len: 0,
        }
    }

    pub fn write(&mut self, data: &[u8]) {
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
}
