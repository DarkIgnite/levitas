//! Java's modified UTF-8: like UTF-8, but NUL is two bytes (C0 80) and characters outside
//! the Basic Multilingual Plane are stored as two 3-byte surrogates (CESU-8 style).

pub fn encode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for unit in s.encode_utf16() {
        match unit {
            0x0001..=0x007F => out.push(unit as u8),
            0x0000 | 0x0080..=0x07FF => {
                out.push(0xC0 | (unit >> 6) as u8);
                out.push(0x80 | (unit & 0x3F) as u8);
            }
            _ => {
                out.push(0xE0 | (unit >> 12) as u8);
                out.push(0x80 | ((unit >> 6) & 0x3F) as u8);
                out.push(0x80 | (unit & 0x3F) as u8);
            }
        }
    }
    out
}

pub fn decode(bytes: &[u8]) -> Option<String> {
    let mut units = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let cont = |j: usize| bytes.get(j).copied().filter(|c| c & 0xC0 == 0x80);
        let unit = match b {
            0x01..=0x7F => {
                i += 1;
                u16::from(b)
            }
            0xC0..=0xDF => {
                let c = cont(i + 1)?;
                i += 2;
                (u16::from(b & 0x1F) << 6) | u16::from(c & 0x3F)
            }
            0xE0..=0xEF => {
                let (c1, c2) = (cont(i + 1)?, cont(i + 2)?);
                i += 3;
                (u16::from(b & 0x0F) << 12) | (u16::from(c1 & 0x3F) << 6) | u16::from(c2 & 0x3F)
            }
            _ => return None,
        };
        units.push(unit);
    }
    // Lone surrogates written by other software are replaced rather than rejected.
    Some(
        char::decode_utf16(units)
            .map(|c| c.unwrap_or('\u{FFFD}'))
            .collect(),
    )
}

/// The largest length `<= len` that does not split a character in `encoded`.
pub fn boundary(encoded: &[u8], mut len: usize) -> usize {
    while len > 0 && len < encoded.len() && encoded[len] & 0xC0 == 0x80 {
        len -= 1;
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_encodings() {
        assert_eq!(encode("\0"), [0xC0, 0x80]);
        assert_eq!(encode("é"), [0xC3, 0xA9]);
        // U+1F600 is the surrogate pair D83D DE00, each as three bytes.
        assert_eq!(encode("\u{1F600}"), [0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]);
    }

    #[test]
    fn round_trips() {
        for s in ["", "plain", "a\0b", "日本語", "\u{1F600}x\u{10FFFF}"] {
            assert_eq!(decode(&encode(s)).as_deref(), Some(s));
        }
    }

    #[test]
    fn rejects_invalid() {
        assert_eq!(decode(&[0xFF]), None);
        assert_eq!(decode(&[0xC3]), None);
        assert_eq!(decode(&[0x00]), None);
    }
}
