//! NBT (Named Binary Tag): the binary format of world files, level.dat, player data and
//! some network fields.
//!
//! Files hold a named root compound ([`from_bytes`], [`to_bytes`]); the network form
//! drops the root name ([`from_network`], [`to_network`]). Strings use Java's modified
//! UTF-8. Gzip helpers cover level.dat and player data.

mod mutf8;

use std::fmt;
use std::io::{Read, Write};

/// Nesting deeper than this is rejected, as vanilla does.
const MAX_DEPTH: usize = 512;

const END: u8 = 0;
const BYTE: u8 = 1;
const SHORT: u8 = 2;
const INT: u8 = 3;
const LONG: u8 = 4;
const FLOAT: u8 = 5;
const DOUBLE: u8 = 6;
const BYTE_ARRAY: u8 = 7;
const STRING: u8 = 8;
const LIST: u8 = 9;
const COMPOUND: u8 = 10;
const INT_ARRAY: u8 = 11;
const LONG_ARRAY: u8 = 12;

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    /// Every element has the same type.
    List(Vec<Tag>),
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => BYTE,
            Tag::Short(_) => SHORT,
            Tag::Int(_) => INT,
            Tag::Long(_) => LONG,
            Tag::Float(_) => FLOAT,
            Tag::Double(_) => DOUBLE,
            Tag::ByteArray(_) => BYTE_ARRAY,
            Tag::String(_) => STRING,
            Tag::List(_) => LIST,
            Tag::Compound(_) => COMPOUND,
            Tag::IntArray(_) => INT_ARRAY,
            Tag::LongArray(_) => LONG_ARRAY,
        }
    }

    /// Any numeric tag as an integer (floats are truncated), like vanilla's getters.
    pub fn as_i64(&self) -> Option<i64> {
        Some(match *self {
            Tag::Byte(v) => i64::from(v),
            Tag::Short(v) => i64::from(v),
            Tag::Int(v) => i64::from(v),
            Tag::Long(v) => v,
            Tag::Float(v) => v as i64,
            Tag::Double(v) => v as i64,
            _ => return None,
        })
    }

    /// Any numeric tag as a float.
    pub fn as_f64(&self) -> Option<f64> {
        Some(match *self {
            Tag::Float(v) => f64::from(v),
            Tag::Double(v) => v,
            _ => self.as_i64()? as f64,
        })
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Tag::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_compound(&self) -> Option<&Compound> {
        match self {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }
}

/// A compound tag: named tags, kept in insertion order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compound(Vec<(String, Tag)>);

impl Compound {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `key`, replacing any existing value.
    pub fn insert(&mut self, key: impl Into<String>, tag: impl Into<Tag>) {
        let key = key.into();
        let tag = tag.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = tag,
            None => self.0.push((key, tag)),
        }
    }

    /// Builder form of [`Compound::insert`].
    pub fn with(mut self, key: impl Into<String>, tag: impl Into<Tag>) -> Self {
        self.insert(key, tag);
        self
    }

    pub fn get(&self, key: &str) -> Option<&Tag> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, t)| t)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Tag)> {
        self.0.iter().map(|(k, t)| (k.as_str(), t))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key)?.as_i64()
    }

    pub fn float(&self, key: &str) -> Option<f64> {
        self.get(key)?.as_f64()
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    pub fn list(&self, key: &str) -> Option<&[Tag]> {
        self.get(key)?.as_list()
    }

    pub fn compound(&self, key: &str) -> Option<&Compound> {
        self.get(key)?.as_compound()
    }

    pub fn long_array(&self, key: &str) -> Option<&[i64]> {
        match self.get(key)? {
            Tag::LongArray(a) => Some(a),
            _ => None,
        }
    }

    pub fn byte_array(&self, key: &str) -> Option<&[i8]> {
        match self.get(key)? {
            Tag::ByteArray(a) => Some(a),
            _ => None,
        }
    }

    pub fn int_array(&self, key: &str) -> Option<&[i32]> {
        match self.get(key)? {
            Tag::IntArray(a) => Some(a),
            _ => None,
        }
    }
}

macro_rules! from_impls {
    ($($ty:ty => $variant:ident),*) => {
        $(impl From<$ty> for Tag {
            fn from(v: $ty) -> Tag {
                Tag::$variant(v.into())
            }
        })*
    };
}

from_impls!(
    i8 => Byte, i16 => Short, i32 => Int, i64 => Long, f32 => Float, f64 => Double,
    String => String, &str => String, Compound => Compound, Vec<Tag> => List,
    Vec<i8> => ByteArray, Vec<i32> => IntArray, Vec<i64> => LongArray
);

impl From<bool> for Tag {
    fn from(v: bool) -> Tag {
        Tag::Byte(i8::from(v))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    UnexpectedEof,
    InvalidTagType(u8),
    InvalidString,
    TooDeep,
    RootNotCompound,
    MixedList,
    TrailingData,
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnexpectedEof => f.write_str("NBT data ended unexpectedly"),
            Error::InvalidTagType(t) => write!(f, "invalid NBT tag type {t}"),
            Error::InvalidString => f.write_str("invalid modified UTF-8 string"),
            Error::TooDeep => write!(f, "NBT nested deeper than {MAX_DEPTH}"),
            Error::RootNotCompound => f.write_str("NBT root is not a compound"),
            Error::MixedList => f.write_str("NBT list elements have different types"),
            Error::TrailingData => f.write_str("unexpected data after the NBT root"),
            Error::Io(e) => write!(f, "NBT I/O error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for std::io::Error {
    fn from(e: Error) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    }
}

/// Reads a file-form NBT document: a named root compound. Returns the root name too.
pub fn from_bytes(data: &[u8]) -> Result<(String, Compound), Error> {
    let mut r = Reader { data };
    if r.u8()? != COMPOUND {
        return Err(Error::RootNotCompound);
    }
    let name = r.string()?;
    let root = r.compound(0)?;
    if !r.data.is_empty() {
        return Err(Error::TrailingData);
    }
    Ok((name, root))
}

/// Writes a file-form NBT document with a named root compound.
pub fn to_bytes(name: &str, root: &Compound) -> Vec<u8> {
    let mut out = vec![COMPOUND];
    write_string(&mut out, name);
    write_compound(&mut out, root);
    out
}

/// Reads network-form NBT: a root tag without a name.
pub fn from_network(data: &[u8]) -> Result<Tag, Error> {
    let mut r = Reader { data };
    let id = r.u8()?;
    let tag = r.payload(id, 0)?;
    if !r.data.is_empty() {
        return Err(Error::TrailingData);
    }
    Ok(tag)
}

/// Writes network-form NBT: a root tag without a name.
pub fn to_network(tag: &Tag) -> Vec<u8> {
    let mut out = vec![tag.id()];
    write_payload(&mut out, tag);
    out
}

/// Reads a gzip-compressed file-form document (level.dat, player data).
pub fn from_gzip(data: &[u8]) -> Result<(String, Compound), Error> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(data)
        .read_to_end(&mut raw)
        .map_err(|e| Error::Io(e.to_string()))?;
    from_bytes(&raw)
}

/// Writes a gzip-compressed file-form document.
pub fn to_gzip(name: &str, root: &Compound) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&to_bytes(name, root))
        .expect("writing to a Vec cannot fail");
    encoder.finish().expect("writing to a Vec cannot fail")
}

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.data.len() < n {
            return Err(Error::UnexpectedEof);
        }
        let (head, tail) = self.data.split_at(n);
        self.data = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.take(N)?.try_into().expect("take returned N bytes"))
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    /// A length prefix, checked against the bytes left so corrupt data cannot make us
    /// allocate huge buffers.
    fn len(&mut self, element_size: usize) -> Result<usize, Error> {
        let len = i32::from_be_bytes(self.array()?);
        let len = usize::try_from(len).map_err(|_| Error::UnexpectedEof)?;
        if len.saturating_mul(element_size) > self.data.len() {
            return Err(Error::UnexpectedEof);
        }
        Ok(len)
    }

    fn string(&mut self) -> Result<String, Error> {
        let len = u16::from_be_bytes(self.array()?);
        mutf8::decode(self.take(usize::from(len))?).ok_or(Error::InvalidString)
    }

    fn compound(&mut self, depth: usize) -> Result<Compound, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        let mut compound = Compound::new();
        loop {
            let id = self.u8()?;
            if id == END {
                return Ok(compound);
            }
            let name = self.string()?;
            let tag = self.payload(id, depth + 1)?;
            compound.0.push((name, tag));
        }
    }

    fn payload(&mut self, id: u8, depth: usize) -> Result<Tag, Error> {
        Ok(match id {
            BYTE => Tag::Byte(self.u8()? as i8),
            SHORT => Tag::Short(i16::from_be_bytes(self.array()?)),
            INT => Tag::Int(i32::from_be_bytes(self.array()?)),
            LONG => Tag::Long(i64::from_be_bytes(self.array()?)),
            FLOAT => Tag::Float(f32::from_be_bytes(self.array()?)),
            DOUBLE => Tag::Double(f64::from_be_bytes(self.array()?)),
            BYTE_ARRAY => {
                let len = self.len(1)?;
                Tag::ByteArray(self.take(len)?.iter().map(|&b| b as i8).collect())
            }
            STRING => Tag::String(self.string()?),
            LIST => {
                if depth > MAX_DEPTH {
                    return Err(Error::TooDeep);
                }
                let element = self.u8()?;
                let len = self.len(1)?;
                if element == END && len > 0 {
                    return Err(Error::InvalidTagType(END));
                }
                let mut list = Vec::with_capacity(len);
                for _ in 0..len {
                    list.push(self.payload(element, depth + 1)?);
                }
                Tag::List(list)
            }
            COMPOUND => Tag::Compound(self.compound(depth)?),
            INT_ARRAY => {
                let len = self.len(4)?;
                let bytes = self.take(len * 4)?;
                Tag::IntArray(
                    bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|&c| i32::from_be_bytes(c))
                        .collect(),
                )
            }
            LONG_ARRAY => {
                let len = self.len(8)?;
                let bytes = self.take(len * 8)?;
                Tag::LongArray(
                    bytes
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|&c| i64::from_be_bytes(c))
                        .collect(),
                )
            }
            other => return Err(Error::InvalidTagType(other)),
        })
    }
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    let encoded = mutf8::encode(s);
    // Strings longer than the 16-bit length allows are cut, never written corrupt.
    let len = encoded.len().min(usize::from(u16::MAX));
    let len = mutf8::boundary(&encoded, len);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(&encoded[..len]);
}

fn write_compound(out: &mut Vec<u8>, compound: &Compound) {
    for (name, tag) in &compound.0 {
        out.push(tag.id());
        write_string(out, name);
        write_payload(out, tag);
    }
    out.push(END);
}

fn write_len(out: &mut Vec<u8>, len: usize) {
    out.extend_from_slice(&(len as i32).to_be_bytes());
}

fn write_payload(out: &mut Vec<u8>, tag: &Tag) {
    match tag {
        Tag::Byte(v) => out.push(*v as u8),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::ByteArray(a) => {
            write_len(out, a.len());
            out.extend(a.iter().map(|&b| b as u8));
        }
        Tag::String(s) => write_string(out, s),
        Tag::List(list) => {
            let element = list.first().map_or(END, Tag::id);
            out.push(element);
            // Elements of another type than the first cannot be represented; skip them.
            let items: Vec<&Tag> = list.iter().filter(|t| t.id() == element).collect();
            write_len(out, items.len());
            for item in items {
                write_payload(out, item);
            }
        }
        Tag::Compound(c) => write_compound(out, c),
        Tag::IntArray(a) => {
            write_len(out, a.len());
            for v in a {
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
        Tag::LongArray(a) => {
            write_len(out, a.len());
            for v in a {
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Compound {
        Compound::new()
            .with("byte", 1i8)
            .with("short", -2i16)
            .with("int", 300_000i32)
            .with("long", -5_000_000_000i64)
            .with("float", 1.5f32)
            .with("double", -2.25f64)
            .with("bytes", vec![1i8, -1])
            .with("text", "héllo \0 \u{1F600}")
            .with("list", vec![Tag::Int(1), Tag::Int(2)])
            .with("empty", Vec::<Tag>::new())
            .with("nested", Compound::new().with("x", 7i32))
            .with("ints", vec![1i32, -1])
            .with("longs", vec![i64::MIN, i64::MAX])
    }

    #[test]
    fn file_round_trip() {
        let bytes = to_bytes("root", &sample());
        let (name, root) = from_bytes(&bytes).unwrap();
        assert_eq!(name, "root");
        assert_eq!(root, sample());
        assert_eq!(root.str("text"), Some("héllo \0 \u{1F600}"));
        assert_eq!(root.int("short"), Some(-2));
        assert_eq!(root.compound("nested").unwrap().int("x"), Some(7));
    }

    #[test]
    fn network_and_gzip_round_trip() {
        let tag = Tag::Compound(sample());
        assert_eq!(from_network(&to_network(&tag)).unwrap(), tag);
        assert_eq!(from_gzip(&to_gzip("", &sample())).unwrap().1, sample());
        assert_eq!(to_network(&Tag::String("hi".into())), [8, 0, 2, b'h', b'i']);
    }

    #[test]
    fn known_encoding() {
        // { "a": 1b } in file form with an empty root name.
        let bytes = to_bytes("", &Compound::new().with("a", 1i8));
        assert_eq!(bytes, [10, 0, 0, 1, 0, 1, b'a', 1, 0]);
    }

    #[test]
    fn rejects_corrupt_data() {
        assert_eq!(from_bytes(&[]), Err(Error::UnexpectedEof));
        assert_eq!(from_bytes(&[8, 0, 0]), Err(Error::RootNotCompound));
        // A long array claiming 2^31-1 elements in a tiny buffer.
        let huge = [10, 0, 0, 12, 0, 1, b'x', 0x7F, 0xFF, 0xFF, 0xFF, 0];
        assert_eq!(from_bytes(&huge), Err(Error::UnexpectedEof));
        let mut deep = vec![10, 0, 0];
        for _ in 0..600 {
            deep.extend_from_slice(&[10, 0, 0]);
        }
        assert_eq!(from_bytes(&deep), Err(Error::TooDeep));
    }
}
