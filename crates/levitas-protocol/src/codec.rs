//! Reading and writing packet fields.

use std::fmt;
use std::io;

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

    pub fn varint(&mut self) -> Result<i32, DecodeError> {
        let (value, len) = varint::read(self.buf).map_err(|e| match e {
            VarIntError::Incomplete => DecodeError::UnexpectedEof,
            VarIntError::TooLong => DecodeError::VarIntTooLong,
        })?;
        self.buf = &self.buf[len..];
        Ok(value)
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        self.array().map(u16::from_be_bytes)
    }

    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        self.array().map(i64::from_be_bytes)
    }

    pub fn uuid(&mut self) -> Result<u128, DecodeError> {
        self.array().map(u128::from_be_bytes)
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

    pub fn u16(mut self, value: u16) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn i64(mut self, value: i64) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
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
