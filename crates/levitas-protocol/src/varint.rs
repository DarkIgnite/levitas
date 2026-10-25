//! VarInt: a 32-bit two's-complement integer stored 7 bits per byte, least significant
//! group first, with the high bit of each byte marking that another byte follows.

/// A VarInt never takes more than five bytes.
pub const MAX_LEN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarIntError {
    /// The buffer ended before the last byte of the VarInt.
    Incomplete,
    /// More than [`MAX_LEN`] bytes had the continuation bit set.
    TooLong,
}

/// Appends `value` to `out`.
pub fn write(out: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        if v & !0x7F == 0 {
            out.push(v as u8);
            return;
        }
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

/// Decodes a VarInt from the start of `buf`, returning the value and how many bytes it used.
pub fn read(buf: &[u8]) -> Result<(i32, usize), VarIntError> {
    let mut value = 0u32;
    for (i, &byte) in buf.iter().take(MAX_LEN).enumerate() {
        value |= u32::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value as i32, i + 1));
        }
    }
    if buf.len() >= MAX_LEN {
        Err(VarIntError::TooLong)
    } else {
        Err(VarIntError::Incomplete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (255, &[0xff, 0x01]),
        (25565, &[0xdd, 0xc7, 0x01]),
        (2_097_151, &[0xff, 0xff, 0x7f]),
        (i32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x07]),
        (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        (i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x08]),
    ];

    #[test]
    fn encodes_known_values() {
        for &(value, bytes) in CASES {
            let mut out = Vec::new();
            write(&mut out, value);
            assert_eq!(out, bytes, "encoding {value}");
        }
    }

    #[test]
    fn decodes_known_values() {
        for &(value, bytes) in CASES {
            assert_eq!(read(bytes), Ok((value, bytes.len())), "decoding {value}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(read(&[]), Err(VarIntError::Incomplete));
        assert_eq!(read(&[0x80, 0x80]), Err(VarIntError::Incomplete));
        assert_eq!(read(&[0x80; 6]), Err(VarIntError::TooLong));
    }
}
