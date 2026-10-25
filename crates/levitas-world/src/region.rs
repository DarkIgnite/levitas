//! Anvil region files (`r.<x>.<z>.mca`): 32x32 chunks per file, stored in 4 KiB sectors.
//!
//! The file starts with two 4 KiB tables: 1024 chunk locations (3-byte sector offset,
//! 1-byte sector count) and 1024 modification times. Each chunk is a 4-byte length, a
//! compression byte and the compressed NBT. Chunks over 255 sectors go to an external
//! `c.<x>.<z>.mcc` file and set the compression byte's high bit.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use levitas_nbt::Compound;

const SECTOR: usize = 4096;
const HEADER_SECTORS: usize = 2;
const MAX_SECTORS_PER_CHUNK: usize = 255;
const GZIP: u8 = 1;
const ZLIB: u8 = 2;
const UNCOMPRESSED: u8 = 3;
const LZ4: u8 = 4;
const EXTERNAL: u8 = 0x80;

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub struct RegionFile {
    file: File,
    dir: PathBuf,
    region: (i32, i32),
    locations: [u32; 1024],
    timestamps: [u32; 1024],
    /// Which sectors of the file are in use.
    used: Vec<bool>,
}

impl RegionFile {
    /// Opens (or creates) the region file for region `(rx, rz)` in `dir`.
    pub fn open(dir: &Path, rx: i32, rz: i32) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("r.{rx}.{rz}.mca"));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let len = file.metadata()?.len() as usize;
        if len < HEADER_SECTORS * SECTOR {
            file.set_len((HEADER_SECTORS * SECTOR) as u64)?;
        }
        let mut header = vec![0u8; HEADER_SECTORS * SECTOR];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut header)?;
        let word = |i: usize| u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());

        let sectors = file.metadata()?.len() as usize / SECTOR;
        let mut region = Self {
            file,
            dir: dir.to_owned(),
            region: (rx, rz),
            locations: [0; 1024],
            timestamps: [0; 1024],
            used: vec![false; sectors.max(HEADER_SECTORS)],
        };
        region.used[..HEADER_SECTORS].fill(true);
        for i in 0..1024 {
            region.timestamps[i] = word(1024 + i);
            let location = word(i);
            let (offset, count) = ((location >> 8) as usize, (location & 0xFF) as usize);
            // Entries pointing into the header, past the end or onto another chunk are
            // treated as missing chunks rather than trusted.
            let valid = location != 0
                && offset >= HEADER_SECTORS
                && count > 0
                && offset + count <= sectors
                && !region.used[offset..offset + count].iter().any(|&u| u);
            if valid {
                region.locations[i] = location;
                region.used[offset..offset + count].fill(true);
            }
        }
        Ok(region)
    }

    fn index(lx: usize, lz: usize) -> usize {
        (lx & 31) + (lz & 31) * 32
    }

    fn external_path(&self, lx: usize, lz: usize) -> PathBuf {
        let x = self.region.0 * 32 + lx as i32;
        let z = self.region.1 * 32 + lz as i32;
        self.dir.join(format!("c.{x}.{z}.mcc"))
    }

    /// The uncompressed NBT of a chunk (local coordinates 0..32), if it is stored.
    pub fn read(&mut self, lx: usize, lz: usize) -> io::Result<Option<Vec<u8>>> {
        let location = self.locations[Self::index(lx, lz)];
        if location == 0 {
            return Ok(None);
        }
        let (offset, count) = ((location >> 8) as usize, (location & 0xFF) as usize);
        let mut sectors = vec![0u8; count * SECTOR];
        self.file.seek(SeekFrom::Start((offset * SECTOR) as u64))?;
        self.file.read_exact(&mut sectors)?;
        let length = u32::from_be_bytes(sectors[..4].try_into().unwrap()) as usize;
        if length == 0 || length + 4 > sectors.len() {
            return Err(invalid(format!(
                "chunk {lx},{lz} has invalid length {length}"
            )));
        }
        let kind = sectors[4];
        let data = if kind & EXTERNAL != 0 {
            fs::read(self.external_path(lx, lz))?
        } else {
            sectors[5..4 + length].to_vec()
        };
        decompress(kind & !EXTERNAL, &data).map(Some)
    }

    /// Stores a chunk's uncompressed NBT, zlib-compressed.
    pub fn write(&mut self, lx: usize, lz: usize, nbt: &[u8], timestamp: u32) -> io::Result<()> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(nbt)?;
        let compressed = encoder.finish()?;

        let external = 5 + compressed.len() > MAX_SECTORS_PER_CHUNK * SECTOR;
        let mut record = Vec::new();
        if external {
            fs::write(self.external_path(lx, lz), &compressed)?;
            record.extend_from_slice(&1u32.to_be_bytes());
            record.push(ZLIB | EXTERNAL);
        } else {
            record.extend_from_slice(&(compressed.len() as u32 + 1).to_be_bytes());
            record.push(ZLIB);
            record.extend_from_slice(&compressed);
        }
        let count = record.len().div_ceil(SECTOR);
        record.resize(count * SECTOR, 0);

        // Write to fresh sectors first and only then point the header at them and free
        // the old ones, so a crash mid-write leaves the previous copy intact.
        let offset = self.allocate(count);
        self.file.seek(SeekFrom::Start((offset * SECTOR) as u64))?;
        self.file.write_all(&record)?;

        let index = Self::index(lx, lz);
        let old = self.locations[index];
        self.locations[index] = ((offset as u32) << 8) | count as u32;
        self.timestamps[index] = timestamp;
        self.file.seek(SeekFrom::Start((index * 4) as u64))?;
        self.file.write_all(&self.locations[index].to_be_bytes())?;
        self.file
            .seek(SeekFrom::Start((SECTOR + index * 4) as u64))?;
        self.file.write_all(&timestamp.to_be_bytes())?;
        self.file.flush()?;

        if old != 0 {
            let (o, c) = ((old >> 8) as usize, (old & 0xFF) as usize);
            self.used[o..o + c].fill(false);
        }
        if !external {
            // A smaller copy replaced an oversized one.
            let _ = fs::remove_file(self.external_path(lx, lz));
        }
        Ok(())
    }

    /// Finds `count` free sectors (first fit), growing the file when needed.
    fn allocate(&mut self, count: usize) -> usize {
        let mut run = 0;
        for i in HEADER_SECTORS..self.used.len() {
            run = if self.used[i] { 0 } else { run + 1 };
            if run == count {
                let start = i + 1 - count;
                self.used[start..=i].fill(true);
                return start;
            }
        }
        let start = self.used.len() - run;
        self.used.resize(start + count, false);
        self.used[start..].fill(true);
        start
    }
}

fn decompress(kind: u8, data: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    match kind {
        GZIP => {
            flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
        }
        ZLIB => {
            flate2::read::ZlibDecoder::new(data).read_to_end(&mut out)?;
        }
        UNCOMPRESSED => out.extend_from_slice(data),
        LZ4 => {
            return Err(invalid(
                "LZ4-compressed chunks are not supported yet".into(),
            ))
        }
        other => return Err(invalid(format!("unknown chunk compression {other}"))),
    }
    Ok(out)
}

/// The region files of one dimension, opened on demand.
pub struct RegionStorage {
    dir: PathBuf,
    files: HashMap<(i32, i32), RegionFile>,
}

impl RegionStorage {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            files: HashMap::new(),
        }
    }

    fn file(&mut self, cx: i32, cz: i32) -> io::Result<&mut RegionFile> {
        let region = (cx >> 5, cz >> 5);
        if !self.files.contains_key(&region) {
            let file = RegionFile::open(&self.dir, region.0, region.1)?;
            self.files.insert(region, file);
        }
        Ok(self.files.get_mut(&region).expect("just inserted"))
    }

    /// The NBT of chunk `(cx, cz)`, if it has been saved.
    pub fn read(&mut self, cx: i32, cz: i32) -> io::Result<Option<Compound>> {
        let Some(bytes) = self
            .file(cx, cz)?
            .read((cx & 31) as usize, (cz & 31) as usize)?
        else {
            return Ok(None);
        };
        Ok(Some(levitas_nbt::from_bytes(&bytes)?.1))
    }

    pub fn write(&mut self, cx: i32, cz: i32, nbt: &Compound) -> io::Result<()> {
        let bytes = levitas_nbt::to_bytes("", nbt);
        let timestamp = std::time::UNIX_EPOCH
            .elapsed()
            .unwrap_or_default()
            .as_secs() as u32;
        self.file(cx, cz)?
            .write((cx & 31) as usize, (cz & 31) as usize, &bytes, timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("levitas-region-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn chunks_survive_reopening() {
        let dir = temp_dir("reopen");
        let mut storage = RegionStorage::new(&dir);
        for (x, z) in [(0, 0), (31, 31), (-1, -1), (40, -70)] {
            let nbt = Compound::new().with("x", x).with("z", z);
            storage.write(x, z, &nbt).unwrap();
        }
        // Overwrite one chunk with a bigger copy.
        let big = Compound::new().with("data", (0..20_000i64).collect::<Vec<_>>());
        storage.write(0, 0, &big).unwrap();
        drop(storage);

        let mut storage = RegionStorage::new(&dir);
        assert_eq!(storage.read(0, 0).unwrap(), Some(big));
        assert_eq!(storage.read(31, 31).unwrap().unwrap().int("x"), Some(31));
        assert_eq!(storage.read(40, -70).unwrap().unwrap().int("z"), Some(-70));
        assert_eq!(storage.read(5, 5).unwrap(), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oversized_chunks_go_to_external_files() {
        let dir = temp_dir("external");
        let mut storage = RegionStorage::new(&dir);
        // Pseudo-random longs barely compress, so this is well over 1 MiB compressed.
        let mut x = 1u64;
        let noise: Vec<i64> = (0..200_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as i64
            })
            .collect();
        let nbt = Compound::new().with("noise", noise);
        storage.write(3, 4, &nbt).unwrap();
        assert!(dir.join("c.3.4.mcc").exists());
        drop(storage);
        assert_eq!(RegionStorage::new(&dir).read(3, 4).unwrap(), Some(nbt));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn freed_sectors_are_reused() {
        let dir = temp_dir("reuse");
        let mut region = RegionFile::open(&dir, 0, 0).unwrap();
        for _ in 0..10 {
            region.write(0, 0, &[1; 100], 0).unwrap();
        }
        // Rewriting one small chunk keeps the file at header + at most two sectors.
        assert!(region.used.len() <= HEADER_SECTORS + 2);
        fs::remove_dir_all(dir).unwrap();
    }
}
