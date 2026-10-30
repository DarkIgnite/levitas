//! Reading and writing packet fields.

use std::fmt;
use std::io;

use crate::types::BlockPos;
use crate::varint::{self, VarIntError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnexpectedEof,
    VarIntTooLong,
    StringTooLong { max: usize },
    InvalidUtf8,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => f.write_str("packet ended unexpectedly"),
            Self::VarIntTooLong => f.write_str("VarInt longer than 5 bytes"),
            Self::StringTooLong { max } => write!(f, "string longer than {max} characters"),
            Self::InvalidUtf8 => f.write_str("string is not valid UTF-8"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<DecodeError> for io::Error {
    fn from(e: DecodeError) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, e)
    }
}

/// Reads fields from the body of one packet.
pub struct Reader<'a> {
    buf: &'a [u8],
}

/// Low-precision vectors: the largest quantized value, the bit marking a scale that
/// goes on in a VarInt, and the scale bits kept in the first byte.
const LP_MAX: f64 = 32766.0;
const LP_CONTINUATION: u64 = 0x04;
const LP_SCALE_BITS: u64 = 0x03;

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.buf.len() < n {
            return Err(DecodeError::UnexpectedEof);
        }
        let (head, tail) = self.buf.split_at(n);
        self.buf = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        Ok(self.take(N)?.try_into().expect("take returned N bytes"))
    }

    /// The next `n` raw bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        self.take(n)
    }

    pub fn varint(&mut self) -> Result<i32, DecodeError> {
        let (value, len) = varint::read(self.buf).map_err(|e| match e {
            VarIntError::Incomplete => DecodeError::UnexpectedEof,
            VarIntError::TooLong => DecodeError::VarIntTooLong,
        })?;
        self.buf = &self.buf[len..];
        Ok(value)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        self.array().map(u8::from_be_bytes)
    }

    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        self.u8().map(|b| b != 0)
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        self.array().map(u16::from_be_bytes)
    }

    pub fn i16(&mut self) -> Result<i16, DecodeError> {
        self.array().map(i16::from_be_bytes)
    }

    pub fn position(&mut self) -> Result<BlockPos, DecodeError> {
        self.i64().map(BlockPos::from_packed)
    }

    pub fn i32(&mut self) -> Result<i32, DecodeError> {
        self.array().map(i32::from_be_bytes)
    }

    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        self.array().map(i64::from_be_bytes)
    }

    pub fn f32(&mut self) -> Result<f32, DecodeError> {
        self.array().map(f32::from_be_bytes)
    }

    pub fn f64(&mut self) -> Result<f64, DecodeError> {
        self.array().map(f64::from_be_bytes)
    }

    pub fn uuid(&mut self) -> Result<u128, DecodeError> {
        self.array().map(u128::from_be_bytes)
    }

    pub fn varlong(&mut self) -> Result<i64, DecodeError> {
        let mut value: u64 = 0;
        for shift in (0..70).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value as i64);
            }
        }
        Err(DecodeError::VarIntTooLong)
    }

    /// A low-precision vector (velocities): see [`Writer::lp_vec3`].
    pub fn lp_vec3(&mut self) -> Result<[f64; 3], DecodeError> {
        let first = u64::from(self.u8()?);
        if first == 0 {
            return Ok([0.0; 3]);
        }
        let second = u64::from(self.u8()?);
        let rest = u64::from(self.i32()? as u32);
        let packed = (rest << 16) | (second << 8) | first;
        let mut scale = first & LP_SCALE_BITS;
        if first & LP_CONTINUATION != 0 {
            scale |= u64::from(self.varint()? as u32) << 2;
        }
        let scale = scale as f64;
        let unpack = |v: u64| ((v & 0x7FFF) as f64).min(LP_MAX) * 2.0 / LP_MAX - 1.0;
        Ok([
            unpack(packed >> 3) * scale,
            unpack(packed >> 18) * scale,
            unpack(packed >> 33) * scale,
        ])
    }

    /// Reads a string of at most `max` characters (counted in UTF-16 code units, as the
    /// protocol specifies).
    pub fn string(&mut self, max: usize) -> Result<&'a str, DecodeError> {
        let len = self.varint()?;
        let len = usize::try_from(len).map_err(|_| DecodeError::UnexpectedEof)?;
        // A UTF-16 code unit takes at most 3 bytes in UTF-8, so this bounds the read
        // before decoding anything.
        if len > max * 3 {
            return Err(DecodeError::StringTooLong { max });
        }
        let s = std::str::from_utf8(self.take(len)?).map_err(|_| DecodeError::InvalidUtf8)?;
        if s.encode_utf16().count() > max {
            return Err(DecodeError::StringTooLong { max });
        }
        Ok(s)
    }
}

/// Builds one outgoing packet.
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn packet(id: i32) -> Self {
        let mut buf = Vec::new();
        varint::write(&mut buf, id);
        Self { buf }
    }

    pub fn varint(mut self, value: i32) -> Self {
        varint::write(&mut self.buf, value);
        self
    }

    /// A 64-bit value in the same seven-bits-a-byte form as a VarInt.
    pub fn varlong(mut self, value: i64) -> Self {
        let mut v = value as u64;
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                return self;
            }
            self.buf.push(byte | 0x80);
        }
    }

    pub fn bool(mut self, value: bool) -> Self {
        self.buf.push(u8::from(value));
        self
    }

    pub fn u8(mut self, value: u8) -> Self {
        self.buf.push(value);
        self
    }

    pub fn i8(self, value: i8) -> Self {
        self.u8(value as u8)
    }

    pub fn u16(mut self, value: u16) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn i16(mut self, value: i16) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn position(self, pos: BlockPos) -> Self {
        self.i64(pos.packed())
    }

    pub fn i32(mut self, value: i32) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn i64(mut self, value: i64) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn f32(mut self, value: f32) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn f64(mut self, value: f64) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// A low-precision vector, as 26.x sends velocities: the three parts scaled by
    /// their largest (rounded up) into 15 bits each, with the scale in the low bits,
    /// in six bytes (two little-endian, then a big-endian int), or one zero byte for
    /// next to nothing.
    pub fn lp_vec3(self, v: [f64; 3]) -> Self {
        let max = v[0].abs().max(v[1].abs()).max(v[2].abs());
        if max.is_nan() || max < 1.0 / LP_MAX {
            return self.u8(0);
        }
        let scale = max.ceil() as u64;
        let continued = scale & LP_SCALE_BITS != scale;
        let low = if continued {
            (scale & LP_SCALE_BITS) | LP_CONTINUATION
        } else {
            scale
        };
        let pack = |x: f64| ((x / scale as f64 * 0.5 + 0.5) * LP_MAX).round() as u64;
        let packed = (pack(v[2]) << 33) | (pack(v[1]) << 18) | (pack(v[0]) << 3) | low;
        let w = self
            .u8(packed as u8)
            .u8((packed >> 8) as u8)
            .i32((packed >> 16) as u32 as i32);
        if continued {
            w.varint((scale >> 2) as i32)
        } else {
            w
        }
    }

    /// Raw bytes with no length prefix.
    pub fn bytes(mut self, value: &[u8]) -> Self {
        self.buf.extend_from_slice(value);
        self
    }

    /// A plain-text chat component, encoded as network NBT (a nameless string tag).
    pub fn text(mut self, value: &str) -> Self {
        const TAG_STRING: u8 = 8;
        self.buf.push(TAG_STRING);
        // NBT uses Java's modified UTF-8, which matches UTF-8 except for NUL and
        // characters outside the BMP. Those are replaced rather than re-encoded.
        let clean: String = value
            .chars()
            .map(|c| {
                if c == '\0' || c > '\u{FFFF}' {
                    '\u{FFFD}'
                } else {
                    c
                }
            })
            .collect();
        let mut end = clean.len().min(usize::from(u16::MAX));
        while !clean.is_char_boundary(end) {
            end -= 1;
        }
        self.buf.extend_from_slice(&(end as u16).to_be_bytes());
        self.buf.extend_from_slice(&clean.as_bytes()[..end]);
        self
    }

    pub fn uuid(mut self, value: u128) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn string(mut self, value: &str) -> Self {
        varint::write(&mut self.buf, value.len() as i32);
        self.buf.extend_from_slice(value.as_bytes());
        self
    }

    /// Prefixes the packet with its length, ready to send on an uncompressed connection.
    pub fn into_frame(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.buf.len() + varint::MAX_LEN);
        varint::write(&mut out, self.buf.len() as i32);
        out.extend_from_slice(&self.buf);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The examples the protocol documentation gives.
    #[test]
    fn low_precision_vectors_match_the_documented_bytes() {
        let bytes = |v: [f64; 3]| Writer { buf: Vec::new() }.lp_vec3(v).buf;
        assert_eq!(bytes([0.0; 3]), [0]);
        assert_eq!(
            bytes([1.0, 0.0, -1.0]),
            [0xF1, 0xFF, 0x00, 0x00, 0xFF, 0xFF]
        );
        assert_eq!(
            bytes([10.0, 0.2, -5.0]),
            [0xF6, 0xFF, 0x40, 0x01, 0x05, 0x1F, 0x02]
        );
        assert_eq!(
            bytes([123457.0, 15.071, 0.0]),
            [0xF5, 0xFF, 0x7F, 0xFF, 0x00, 0x07, 0x90, 0xF1, 0x01]
        );
        let long = |v: i64| Writer { buf: Vec::new() }.varlong(v).buf;
        assert_eq!(long(0), [0]);
        assert_eq!(long(300), [0xAC, 0x02]);
        assert_eq!(
            long(-1),
            [0xFF; 9].into_iter().chain([0x01]).collect::<Vec<_>>()
        );
        let back = Reader::new(&bytes([0.25, -0.5, 1.5])).lp_vec3().unwrap();
        for (a, b) in back.iter().zip([0.25, -0.5, 1.5]) {
            assert!((a - b).abs() < 1e-3, "{back:?}");
        }
    }

    #[test]
    fn frame_round_trip() {
        let frame = Writer::packet(0x01).string("Levitas").i64(-42).into_frame();
        let mut r = Reader::new(&frame);
        assert_eq!(r.varint().unwrap() as usize, frame.len() - 1);
        assert_eq!(r.varint().unwrap(), 0x01);
        assert_eq!(r.string(16).unwrap(), "Levitas");
        assert_eq!(r.i64().unwrap(), -42);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn string_limit_counts_utf16_units() {
        let frame = Writer::packet(0).string("ééé").into_frame();
        let mut r = Reader::new(&frame[2..]);
        assert_eq!(r.string(2), Err(DecodeError::StringTooLong { max: 2 }));
        let mut r = Reader::new(&frame[2..]);
        assert_eq!(r.string(3).unwrap(), "ééé");
    }

    #[test]
    fn truncated_field_is_eof() {
        assert_eq!(Reader::new(&[0, 1]).i64(), Err(DecodeError::UnexpectedEof));
    }
}
