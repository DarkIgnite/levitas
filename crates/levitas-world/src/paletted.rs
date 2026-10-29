//! Paletted containers: the compact storage the protocol (and Anvil) use for block states
//! and biomes. A section with one kind of block costs a few bytes; a varied section stores
//! small palette indices instead of full state IDs.

use levitas_protocol::varint;

/// Sizing rules for one kind of container.
#[derive(Debug, Clone, Copy)]
pub struct Kind {
    /// Number of entries: 4096 blocks or 64 biomes per section.
    pub entries: usize,
    /// Smallest bits per entry for the indirect (palette) form.
    pub min_bits: u8,
    /// Largest bits per entry before switching to direct (global ID) storage.
    pub max_indirect_bits: u8,
    /// Bits per entry for direct storage: enough for every possible value.
    pub direct_bits: u8,
}

impl Kind {
    pub fn blocks() -> Self {
        Self {
            entries: 4096,
            min_bits: 4,
            max_indirect_bits: 8,
            direct_bits: bits_for(levitas_registry::BLOCK_STATE_COUNT),
        }
    }

    pub fn biomes(biome_count: usize) -> Self {
        Self {
            entries: 64,
            min_bits: 1,
            max_indirect_bits: 3,
            direct_bits: bits_for(biome_count),
        }
    }
}

/// Bits needed to store values `0..count`.
pub fn bits_for(count: usize) -> u8 {
    (usize::BITS - (count.max(2) - 1).leading_zeros()) as u8
}

#[derive(Debug, Clone)]
pub struct PalettedContainer {
    kind: Kind,
    storage: Storage,
}

#[derive(Debug, Clone)]
enum Storage {
    Single(u16),
    Indirect {
        palette: Vec<u16>,
        data: PackedArray,
    },
    Direct(PackedArray),
}

impl PalettedContainer {
    pub fn single(kind: Kind, value: u16) -> Self {
        Self {
            kind,
            storage: Storage::Single(value),
        }
    }

    pub fn get(&self, index: usize) -> u16 {
        match &self.storage {
            Storage::Single(value) => *value,
            Storage::Indirect { palette, data } => palette[data.get(index) as usize],
            Storage::Direct(data) => data.get(index) as u16,
        }
    }

    /// Sets an entry and returns the previous value.
    pub fn set(&mut self, index: usize, value: u16) -> u16 {
        let old = self.get(index);
        if old == value {
            return old;
        }
        let kind = self.kind;
        match &mut self.storage {
            Storage::Single(current) => {
                let palette = vec![*current, value];
                let mut data = PackedArray::new(kind.min_bits, kind.entries);
                data.set(index, 1);
                self.storage = Storage::Indirect { palette, data };
            }
            Storage::Indirect { palette, data } => {
                let slot = match palette.iter().position(|&p| p == value) {
                    Some(slot) => slot,
                    None => {
                        palette.push(value);
                        palette.len() - 1
                    }
                };
                if slot < 1 << data.bits {
                    data.set(index, slot as u64);
                } else if data.bits < kind.max_indirect_bits {
                    *data = data.repack(data.bits + 1);
                    data.set(index, slot as u64);
                } else {
                    let mut direct = PackedArray::new(kind.direct_bits, kind.entries);
                    for i in 0..kind.entries {
                        direct.set(i, u64::from(palette[data.get(i) as usize]));
                    }
                    direct.set(index, u64::from(value));
                    self.storage = Storage::Direct(direct);
                }
            }
            Storage::Direct(data) => data.set(index, u64::from(value)),
        }
        old
    }

    /// Whether `value` might be in the container: a quick check of the palette, which
    /// may still list values since replaced.
    pub fn may_contain(&self, value: u16) -> bool {
        match &self.storage {
            Storage::Single(v) => *v == value,
            Storage::Indirect { palette, .. } => palette.contains(&value),
            Storage::Direct(_) => true,
        }
    }

    /// Builds a container holding `values` (one per entry), in the most compact form.
    pub fn from_values(kind: Kind, values: &[u16]) -> Self {
        assert_eq!(values.len(), kind.entries);
        let mut palette: Vec<u16> = Vec::new();
        let mut indices = Vec::with_capacity(values.len());
        for &v in values {
            let slot = match palette.iter().position(|&p| p == v) {
                Some(slot) => slot,
                None => {
                    palette.push(v);
                    palette.len() - 1
                }
            };
            indices.push(slot as u64);
        }
        let storage = if palette.len() == 1 {
            Storage::Single(palette[0])
        } else {
            let bits = bits_for(palette.len()).max(kind.min_bits);
            if bits <= kind.max_indirect_bits {
                let mut data = PackedArray::new(bits, kind.entries);
                for (i, &slot) in indices.iter().enumerate() {
                    data.set(i, slot);
                }
                Storage::Indirect { palette, data }
            } else {
                let mut data = PackedArray::new(kind.direct_bits, kind.entries);
                for (i, &v) in values.iter().enumerate() {
                    data.set(i, u64::from(v));
                }
                Storage::Direct(data)
            }
        };
        Self { kind, storage }
    }

    /// Every entry's value, in index order.
    pub fn values(&self) -> Vec<u16> {
        (0..self.kind.entries).map(|i| self.get(i)).collect()
    }

    /// Network form: bits per entry, palette, then the packed longs (no length prefix
    /// since 1.21.5).
    pub fn encode(&self, out: &mut Vec<u8>) {
        match &self.storage {
            Storage::Single(value) => {
                out.push(0);
                varint::write(out, i32::from(*value));
            }
            Storage::Indirect { palette, data } => {
                out.push(data.bits);
                varint::write(out, palette.len() as i32);
                for &p in palette {
                    varint::write(out, i32::from(p));
                }
                data.encode(out);
            }
            Storage::Direct(data) => {
                out.push(data.bits);
                data.encode(out);
            }
        }
    }
}

/// Fixed-width entries packed into longs, lowest bits first. Entries never straddle two
/// longs; leftover high bits are padding.
#[derive(Debug, Clone)]
struct PackedArray {
    bits: u8,
    entries: usize,
    longs: Vec<u64>,
}

impl PackedArray {
    fn new(bits: u8, entries: usize) -> Self {
        let per_long = 64 / bits as usize;
        Self {
            bits,
            entries,
            longs: vec![0; entries.div_ceil(per_long)],
        }
    }

    fn per_long(&self) -> usize {
        64 / self.bits as usize
    }

    fn mask(&self) -> u64 {
        (1 << self.bits) - 1
    }

    fn get(&self, index: usize) -> u64 {
        let per_long = self.per_long();
        let shift = (index % per_long) * self.bits as usize;
        (self.longs[index / per_long] >> shift) & self.mask()
    }

    fn set(&mut self, index: usize, value: u64) {
        let per_long = self.per_long();
        let shift = (index % per_long) * self.bits as usize;
        let mask = self.mask();
        let long = &mut self.longs[index / per_long];
        *long = (*long & !(mask << shift)) | ((value & mask) << shift);
    }

    fn repack(&self, bits: u8) -> Self {
        let mut packed = Self::new(bits, self.entries);
        for i in 0..self.entries {
            packed.set(i, self.get(i));
        }
        packed
    }

    fn encode(&self, out: &mut Vec<u8>) {
        for long in &self.longs {
            out.extend_from_slice(&long.to_be_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_bits_cover_all_block_states() {
        let kind = Kind::blocks();
        assert!(1usize << kind.direct_bits >= levitas_registry::BLOCK_STATE_COUNT);
        assert_eq!(bits_for(64), 6);
        assert_eq!(bits_for(65), 7);
    }

    #[test]
    fn grows_from_single_to_indirect_to_direct() {
        let mut c = PalettedContainer::single(Kind::blocks(), 0);
        assert_eq!(c.set(5, 7), 0);
        assert_eq!(c.get(5), 7);
        assert_eq!(c.get(6), 0);

        // 300 distinct values overflow an 8-bit palette and force direct storage.
        for i in 0..300u16 {
            c.set(usize::from(i) * 13, i + 1);
        }
        assert!(matches!(c.storage, Storage::Direct(_)));
        for i in 0..300u16 {
            assert_eq!(c.get(usize::from(i) * 13), i + 1);
        }
    }

    #[test]
    fn long_count_matches_entries_at_every_width() {
        let mut c = PalettedContainer::single(Kind::blocks(), 0);
        // 17 values need 5 bits: 12 per long, so ceil(4096 / 12) = 342 longs.
        for i in 0..17u16 {
            c.set(usize::from(i), i + 1);
        }
        let Storage::Indirect { data, .. } = &c.storage else {
            panic!("expected indirect storage");
        };
        assert_eq!(data.bits, 5);
        assert_eq!(data.longs.len(), 342);
    }

    #[test]
    fn from_values_picks_the_compact_form() {
        let kind = Kind::blocks();
        assert!(matches!(
            PalettedContainer::from_values(kind, &[3; 4096]).storage,
            Storage::Single(3)
        ));
        let mixed: Vec<u16> = (0..4096).map(|i| (i % 20) as u16).collect();
        let c = PalettedContainer::from_values(kind, &mixed);
        assert!(matches!(&c.storage, Storage::Indirect { data, .. } if data.bits == 5));
        assert_eq!(c.values(), mixed);
        let varied: Vec<u16> = (0..4096).map(|i| (i % 1000) as u16).collect();
        let c = PalettedContainer::from_values(kind, &varied);
        assert!(matches!(c.storage, Storage::Direct(_)));
        assert_eq!(c.values(), varied);
    }

    #[test]
    fn encoded_sizes() {
        let mut out = Vec::new();
        PalettedContainer::single(Kind::blocks(), 9).encode(&mut out);
        assert_eq!(out, [0, 9]);

        let mut c = PalettedContainer::single(Kind::blocks(), 0);
        c.set(0, 1);
        out.clear();
        c.encode(&mut out);
        // bits, palette length, two palette entries, 4096 * 4 bits = 256 longs.
        assert_eq!(out.len(), 1 + 1 + 2 + 256 * 8);
        assert_eq!(out[0], 4);
    }
}
