//! LZ4 as vanilla's region files use it (`region-file-compression=lz4`): lz4-java's
//! block stream. Each block is the magic `LZ4Block`, a token (the method, raw or LZ4,
//! with the block size's level), the compressed and the decompressed lengths and an
//! xxHash32 checksum of the decompressed bytes (seed 0x9747b28c, top four bits
//! cleared), all little-endian, then the data; an empty raw block ends the stream.
//! The LZ4 block format itself is the public one: sequences of literals and matches.

use std::io;

const MAGIC: &[u8; 8] = b"LZ4Block";
const RAW: u8 = 0x10;
const COMPRESSED: u8 = 0x20;
const SEED: u32 = 0x9747_b28c;
/// Blocks of 64 KiB, as lz4-java writes them by default (level 6: 1 << (10 + 6)).
const BLOCK: usize = 1 << 16;
const LEVEL: u8 = 6;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("lz4: {message}"))
}

const P1: u32 = 2_654_435_761;
const P2: u32 = 2_246_822_519;
const P3: u32 = 3_266_489_917;
const P4: u32 = 668_265_263;
const P5: u32 = 374_761_393;

fn read32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// xxHash32 of `data`.
pub fn xxh32(data: &[u8], seed: u32) -> u32 {
    let round = |acc: u32, lane: u32| {
        acc.wrapping_add(lane.wrapping_mul(P2))
            .rotate_left(13)
            .wrapping_mul(P1)
    };
    let len = data.len();
    let mut i = 0;
    let mut h = if len >= 16 {
        let mut v = [
            seed.wrapping_add(P1).wrapping_add(P2),
            seed.wrapping_add(P2),
            seed,
            seed.wrapping_sub(P1),
        ];
        while i + 16 <= len {
            for (k, lane) in v.iter_mut().enumerate() {
                *lane = round(*lane, read32(data, i + k * 4));
            }
            i += 16;
        }
        v[0].rotate_left(1)
            .wrapping_add(v[1].rotate_left(7))
            .wrapping_add(v[2].rotate_left(12))
            .wrapping_add(v[3].rotate_left(18))
    } else {
        seed.wrapping_add(P5)
    };
    h = h.wrapping_add(len as u32);
    while i + 4 <= len {
        h = h
            .wrapping_add(read32(data, i).wrapping_mul(P3))
            .rotate_left(17)
            .wrapping_mul(P4);
        i += 4;
    }
    while i < len {
        h = h
            .wrapping_add(u32::from(data[i]).wrapping_mul(P5))
            .rotate_left(11)
            .wrapping_mul(P1);
        i += 1;
    }
    h ^= h >> 15;
    h = h.wrapping_mul(P2);
    h ^= h >> 13;
    h = h.wrapping_mul(P3);
    h ^ (h >> 16)
}

fn checksum(data: &[u8]) -> u32 {
    xxh32(data, SEED) & 0x0FFF_FFFF
}

/// Decompresses one LZ4 block into exactly `size` bytes.
fn decompress_block(src: &[u8], size: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(size);
    let mut i = 0;
    let length = |i: &mut usize, base: usize| -> io::Result<usize> {
        let mut n = base;
        if base == 15 {
            loop {
                let b = *src.get(*i).ok_or_else(|| invalid("truncated length"))?;
                *i += 1;
                n += usize::from(b);
                if b != 255 {
                    break;
                }
            }
        }
        Ok(n)
    };
    while i < src.len() {
        let token = src[i];
        i += 1;
        let literals = length(&mut i, usize::from(token >> 4))?;
        let end = i + literals;
        out.extend_from_slice(
            src.get(i..end)
                .ok_or_else(|| invalid("truncated literals"))?,
        );
        i = end;
        if i >= src.len() {
            break; // the last sequence has no match
        }
        let offset = usize::from(u16::from_le_bytes([
            src[i],
            *src.get(i + 1).ok_or_else(|| invalid("truncated offset"))?,
        ]));
        i += 2;
        if offset == 0 || offset > out.len() {
            return Err(invalid("bad match offset"));
        }
        let matched = length(&mut i, usize::from(token & 15))? + 4;
        let start = out.len() - offset;
        for k in 0..matched {
            let b = out[start + k];
            out.push(b);
        }
    }
    if out.len() != size {
        return Err(invalid("wrong decompressed size"));
    }
    Ok(out)
}

fn push_length(out: &mut Vec<u8>, mut n: usize) {
    while n >= 255 {
        out.push(255);
        n -= 255;
    }
    out.push(n as u8);
}

fn sequence(out: &mut Vec<u8>, literals: &[u8], matched: Option<(usize, usize)>) {
    let lit = literals.len();
    let m = matched.map_or(0, |(_, len)| len - 4);
    let token = ((lit.min(15) as u8) << 4)
        | if matched.is_some() {
            m.min(15) as u8
        } else {
            0
        };
    out.push(token);
    if lit >= 15 {
        push_length(out, lit - 15);
    }
    out.extend_from_slice(literals);
    if let Some((offset, _)) = matched {
        out.extend_from_slice(&(offset as u16).to_le_bytes());
        if m >= 15 {
            push_length(out, m - 15);
        }
    }
}

/// Compresses one block (greedy matching with a hash table of 4-byte sequences).
fn compress_block(src: &[u8]) -> Vec<u8> {
    const MIN_MATCH: usize = 4;
    // The format's rules: the last 5 bytes are literals, no match starts in the last 12.
    const LAST_LITERALS: usize = 5;
    const MATCH_LIMIT: usize = 12;
    let mut out = Vec::with_capacity(src.len() / 2 + 16);
    let mut table = vec![usize::MAX; 1 << 14];
    let hash = |v: u32| (v.wrapping_mul(2_654_435_761) >> 18) as usize;
    let mut anchor = 0;
    let mut i = 0;
    while src.len() >= MATCH_LIMIT && i + MATCH_LIMIT <= src.len() {
        let v = read32(src, i);
        let h = hash(v);
        let candidate = table[h];
        table[h] = i;
        if candidate != usize::MAX && i - candidate <= 0xFFFF && read32(src, candidate) == v {
            let mut len = MIN_MATCH;
            let limit = src.len() - LAST_LITERALS;
            while i + len < limit && src[candidate + len] == src[i + len] {
                len += 1;
            }
            sequence(&mut out, &src[anchor..i], Some((i - candidate, len)));
            i += len;
            anchor = i;
        } else {
            i += 1;
        }
    }
    sequence(&mut out, &src[anchor..], None);
    out
}

/// `data` as an lz4-java block stream.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    let header = |out: &mut Vec<u8>, method: u8, compressed: usize, raw: usize, check: u32| {
        out.extend_from_slice(MAGIC);
        out.push(method | LEVEL);
        out.extend_from_slice(&(compressed as u32).to_le_bytes());
        out.extend_from_slice(&(raw as u32).to_le_bytes());
        out.extend_from_slice(&check.to_le_bytes());
    };
    for block in data.chunks(BLOCK) {
        let packed = compress_block(block);
        let check = checksum(block);
        if packed.len() < block.len() {
            header(&mut out, COMPRESSED, packed.len(), block.len(), check);
            out.extend_from_slice(&packed);
        } else {
            header(&mut out, RAW, block.len(), block.len(), check);
            out.extend_from_slice(block);
        }
    }
    // The end of the stream: an empty raw block.
    header(&mut out, RAW, 0, 0, 0);
    out
}

/// Reads an lz4-java block stream back.
pub fn decompress(mut src: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    while !src.is_empty() {
        if src.len() < 21 || &src[..8] != MAGIC {
            return Err(invalid("bad block header"));
        }
        let method = src[8] & 0xF0;
        let compressed = read32(src, 9) as usize;
        let raw = read32(src, 13) as usize;
        let check = read32(src, 17);
        src = &src[21..];
        if raw == 0 && compressed == 0 {
            break;
        }
        let body = src
            .get(..compressed)
            .ok_or_else(|| invalid("truncated block"))?;
        let block = match method {
            RAW => body.to_vec(),
            COMPRESSED => decompress_block(body, raw)?,
            _ => return Err(invalid("unknown block method")),
        };
        if checksum(&block) != check {
            return Err(invalid("checksum mismatch"));
        }
        out.extend_from_slice(&block);
        src = &src[compressed..];
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xxhash_matches_the_reference() {
        // Reference values of xxHash32.
        assert_eq!(xxh32(b"", 0), 0x02CC_5D05);
        assert_eq!(xxh32(b"a", 0), 0x550D_7456);
        assert_eq!(xxh32(b"abc", 0), 0x32D1_53FF);
        assert_eq!(
            xxh32(b"Nobody inspects the spammish repetition", 0),
            0xE229_3B2F
        );
    }

    #[test]
    fn streams_round_trip() {
        let mut data = Vec::new();
        for i in 0..200_000u32 {
            data.extend_from_slice(&(i % 977).to_le_bytes());
            if i % 7 == 0 {
                data.push((i * 31) as u8);
            }
        }
        let packed = compress(&data);
        assert!(packed.len() < data.len() / 2, "it compresses");
        assert_eq!(decompress(&packed).unwrap(), data);
        assert_eq!(decompress(&compress(b"")).unwrap(), b"");
        assert_eq!(decompress(&compress(b"short")).unwrap(), b"short");
        // A raw block, as lz4-java writes incompressible data.
        let noise: Vec<u8> = (0..5000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        assert_eq!(decompress(&compress(&noise)).unwrap(), noise);
    }
}
