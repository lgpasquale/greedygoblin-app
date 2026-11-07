//! A minimal CBOR encoder: maps, text strings and integers only. That is all
//! the data add-on needs, and the add-on decodes it with `DeserializeCBOR`.

#[derive(Default)]
pub struct Encoder {
    buf: Vec<u8>,
}

const MAJOR_UNSIGNED: u8 = 0;
const MAJOR_NEGATIVE: u8 = 1;
const MAJOR_TEXT: u8 = 3;
const MAJOR_MAP: u8 = 5;

impl Encoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    fn head(&mut self, major: u8, n: u64) {
        let major = major << 5;
        if n < 24 {
            self.buf.push(major | n as u8);
        } else if n <= u8::MAX as u64 {
            self.buf.extend_from_slice(&[major | 24, n as u8]);
        } else if n <= u16::MAX as u64 {
            self.buf.push(major | 25);
            self.buf.extend_from_slice(&(n as u16).to_be_bytes());
        } else if n <= u32::MAX as u64 {
            self.buf.push(major | 26);
            self.buf.extend_from_slice(&(n as u32).to_be_bytes());
        } else {
            self.buf.push(major | 27);
            self.buf.extend_from_slice(&n.to_be_bytes());
        }
    }

    /// Starts a map of `len` pairs; the caller then writes `len` keys and values.
    pub fn map(&mut self, len: usize) {
        self.head(MAJOR_MAP, len as u64);
    }

    pub fn text(&mut self, s: &str) {
        self.head(MAJOR_TEXT, s.len() as u64);
        self.buf.extend_from_slice(s.as_bytes());
    }

    pub fn int(&mut self, n: i64) {
        if n >= 0 {
            self.head(MAJOR_UNSIGNED, n as u64);
        } else {
            // CBOR stores -1 - n for negative integers.
            self.head(MAJOR_NEGATIVE, (-1 - n) as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(n: i64) -> Vec<u8> {
        let mut e = Encoder::new();
        e.int(n);
        e.into_bytes()
    }

    // Expected bytes from RFC 8949, Appendix A.
    #[test]
    fn integers() {
        assert_eq!(int(0), [0x00]);
        assert_eq!(int(23), [0x17]);
        assert_eq!(int(24), [0x18, 0x18]);
        assert_eq!(int(100), [0x18, 0x64]);
        assert_eq!(int(1000), [0x19, 0x03, 0xe8]);
        assert_eq!(int(1_000_000), [0x1a, 0x00, 0x0f, 0x42, 0x40]);
        assert_eq!(
            int(1_000_000_000_000),
            [0x1b, 0x00, 0x00, 0x00, 0xe8, 0xd4, 0xa5, 0x10, 0x00]
        );
        assert_eq!(int(-1), [0x20]);
        assert_eq!(int(-100), [0x38, 0x63]);
        assert_eq!(int(-1000), [0x39, 0x03, 0xe7]);
    }

    #[test]
    fn text_and_map() {
        let mut e = Encoder::new();
        e.map(2);
        e.text("a");
        e.int(1);
        e.text("IETF");
        e.text("");
        assert_eq!(
            e.into_bytes(),
            [0xa2, 0x61, 0x61, 0x01, 0x64, 0x49, 0x45, 0x54, 0x46, 0x60]
        );
    }
}
