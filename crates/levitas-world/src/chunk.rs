//! Chunk columns: 24 sections of 16x16x16 blocks spanning y = -64..320.

use levitas_protocol::varint;
use std::sync::OnceLock;

use levitas_registry::{block, BlockInfo, BLOCK_STATE_COUNT};

use crate::light::ChunkLight;
use crate::paletted::{Kind, PalettedContainer};

pub const MIN_Y: i32 = -64;
pub const SECTIONS: usize = 24;
pub const MAX_Y: i32 = MIN_Y + (SECTIONS as i32) * 16 - 1;

/// The sections a dimension shows clients and saves, out of the 24 every chunk keeps:
/// all of them in the overworld, the 16 from y = 0 to 255 in the Nether. Chunks keep the
/// same layout everywhere; only what goes out is cut down to the dimension's height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Index of the first section shown.
    pub first: usize,
    pub count: usize,
}

impl Span {
    pub const OVERWORLD: Span = Span {
        first: 0,
        count: SECTIONS,
    };
    pub const NETHER: Span = Span {
        first: 4,
        count: 16,
    };

    /// The lowest block y of the span.
    pub fn min_y(self) -> i32 {
        MIN_Y + 16 * self.first as i32
    }

    pub fn max_y(self) -> i32 {
        self.min_y() + 16 * self.count as i32 - 1
    }

    /// Light sections of the span: one below and one above its block sections too.
    pub fn light(self) -> std::ops::Range<usize> {
        self.first..self.first + self.count + 2
    }
}

pub fn is_air(state: u16) -> bool {
    matches!(state, block::AIR | block::CAVE_AIR | block::VOID_AIR)
}

/// Whether a block state holds a fluid: water or lava themselves, anything
/// waterlogged, and plants that only grow under water.
pub fn holds_fluid(state: u16) -> bool {
    static FLUID: OnceLock<Vec<bool>> = OnceLock::new();
    let table = FLUID.get_or_init(|| {
        (0..BLOCK_STATE_COUNT)
            .map(|s| {
                let s = s as u16;
                let info = BlockInfo::of(s);
                matches!(
                    info.name,
                    "minecraft:water"
                        | "minecraft:lava"
                        | "minecraft:bubble_column"
                        | "minecraft:kelp"
                        | "minecraft:kelp_plant"
                        | "minecraft:seagrass"
                        | "minecraft:tall_seagrass"
                ) || info.property(s, "waterlogged") == Some("true")
            })
            .collect()
    });
    table.get(usize::from(state)).copied().unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct Section {
    /// Non-air blocks, sent to the client so it can skip empty sections.
    block_count: i16,
    /// Blocks holding a fluid, which the client is told too.
    fluid_count: i16,
    blocks: PalettedContainer,
    biomes: PalettedContainer,
}

impl Section {
    pub fn empty(biome: u16, biome_kind: Kind) -> Self {
        Self {
            block_count: 0,
            fluid_count: 0,
            blocks: PalettedContainer::single(Kind::blocks(), block::AIR),
            biomes: PalettedContainer::single(biome_kind, biome),
        }
    }

    /// A section from its block and biome containers.
    pub fn from_parts(blocks: PalettedContainer, biomes: PalettedContainer) -> Self {
        let block_count = blocks.values().iter().filter(|&&s| !is_air(s)).count() as i16;
        let fluid_count = blocks.values().iter().filter(|&&s| holds_fluid(s)).count() as i16;
        Self {
            block_count,
            fluid_count,
            blocks,
            biomes,
        }
    }

    /// Whether the section holds nothing but air.
    pub fn is_empty(&self) -> bool {
        self.block_count == 0
    }

    pub fn blocks(&self) -> &PalettedContainer {
        &self.blocks
    }

    pub fn biomes(&self) -> &PalettedContainer {
        &self.biomes
    }

    fn index(x: usize, y: usize, z: usize) -> usize {
        (y * 16 + z) * 16 + x
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> u16 {
        self.blocks.get(Self::index(x, y, z))
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, state: u16) -> u16 {
        let old = self.blocks.set(Self::index(x, y, z), state);
        self.block_count += i16::from(!is_air(state)) - i16::from(!is_air(old));
        self.fluid_count += i16::from(holds_fluid(state)) - i16::from(holds_fluid(old));
        old
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.block_count.to_be_bytes());
        out.extend_from_slice(&self.fluid_count.to_be_bytes());
        self.blocks.encode(out);
        self.biomes.encode(out);
    }
}

/// A newly generated chunk: its position, its blocks, and the block entities and
/// entities generation placed in it (structure chests naming their loot table, dungeon
/// spawners naming their mob, villagers in villages) in their saved NBT form.
pub struct Generated {
    pub pos: (i32, i32),
    pub chunk: Chunk,
    pub block_entities: Vec<levitas_nbt::Compound>,
    pub entities: Vec<levitas_nbt::Compound>,
}

impl Generated {
    pub fn new(pos: (i32, i32), chunk: Chunk) -> Self {
        Self {
            pos,
            chunk,
            block_entities: Vec::new(),
            entities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    sections: Vec<Section>,
    pub light: ChunkLight,
}

impl Chunk {
    pub fn empty(biome: u16, biome_kind: Kind) -> Self {
        Self {
            sections: vec![Section::empty(biome, biome_kind); SECTIONS],
            light: ChunkLight::full_sky(),
        }
    }

    /// A chunk from its sections, bottom to top; there must be [`SECTIONS`] of them.
    /// Light starts as full daylight.
    pub fn from_sections(sections: Vec<Section>) -> Self {
        assert_eq!(sections.len(), SECTIONS);
        Self {
            sections,
            light: ChunkLight::full_sky(),
        }
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Local x/z in 0..16, world y in MIN_Y..=MAX_Y.
    pub fn get(&self, x: usize, y: i32, z: usize) -> u16 {
        match section_of(y) {
            Some((s, ly)) => self.sections[s].get(x, ly, z),
            None => block::AIR,
        }
    }

    /// Sets a block and returns the previous state; out-of-range y is ignored.
    pub fn set(&mut self, x: usize, y: i32, z: usize, state: u16) -> u16 {
        match section_of(y) {
            Some((s, ly)) => self.sections[s].set(x, ly, z, state),
            None => block::AIR,
        }
    }

    /// The section data of a chunk packet: a length-prefixed byte array of all sections.
    pub fn encode_sections(&self, out: &mut Vec<u8>, span: Span) {
        let mut data = Vec::new();
        for section in &self.sections[span.first..span.first + span.count] {
            section.encode(&mut data);
        }
        varint::write(out, data.len() as i32);
        out.extend_from_slice(&data);
    }
}

impl Chunk {
    /// A heightmap: for each column (index `z * 16 + x`), how many blocks above the
    /// bottom of the world the first block from the top accepted by `counts` ends, or
    /// 0 if the column has none.
    pub fn heightmap(&self, counts: impl Fn(u16) -> bool) -> [u16; 256] {
        let mut heights = [0u16; 256];
        let mut missing = 256;
        for (s, section) in self.sections.iter().enumerate().rev() {
            if section.block_count == 0 {
                continue;
            }
            for (i, height) in heights.iter_mut().enumerate() {
                if *height != 0 {
                    continue;
                }
                let (x, z) = (i % 16, i / 16);
                if let Some(y) = (0..16).rev().find(|&y| counts(section.get(x, y, z))) {
                    *height = (s * 16 + y + 1) as u16;
                    missing -= 1;
                }
            }
            if missing == 0 {
                break;
            }
        }
        heights
    }
}

/// Packs a heightmap into longs as the protocol and Anvil store it: 9 bits per column
/// (enough for the 385 possible heights), 7 per long, none split across longs.
pub fn pack_heightmap(heights: &[u16; 256]) -> Vec<i64> {
    const BITS: usize = 9;
    const PER_LONG: usize = 64 / BITS;
    let mut longs = vec![0i64; heights.len().div_ceil(PER_LONG)];
    for (i, &h) in heights.iter().enumerate() {
        longs[i / PER_LONG] |= (i64::from(h) & ((1 << BITS) - 1)) << ((i % PER_LONG) * BITS);
    }
    longs
}

fn section_of(y: i32) -> Option<(usize, usize)> {
    if !(MIN_Y..=MAX_Y).contains(&y) {
        return None;
    }
    let rel = (y - MIN_Y) as usize;
    Some((rel / 16, rel % 16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk() -> Chunk {
        Chunk::empty(0, Kind::biomes(64))
    }

    #[test]
    fn set_and_get_across_sections() {
        let mut c = chunk();
        assert_eq!(c.set(3, MIN_Y, 4, block::STONE), block::AIR);
        c.set(15, MAX_Y, 15, block::DIRT);
        assert_eq!(c.get(3, MIN_Y, 4), block::STONE);
        assert_eq!(c.get(15, MAX_Y, 15), block::DIRT);
        assert_eq!(c.get(0, 0, 0), block::AIR);
        assert_eq!(c.set(0, MAX_Y + 1, 0, block::STONE), block::AIR);
    }

    #[test]
    fn heightmaps_find_the_top_block() {
        let mut c = chunk();
        c.set(0, MIN_Y, 0, block::BEDROCK);
        c.set(0, 70, 0, block::STONE);
        c.set(5, 100, 2, block::GLASS);
        let heights = c.heightmap(|s| s != block::AIR && s != block::GLASS);
        assert_eq!(heights[0], (70 - MIN_Y + 1) as u16);
        assert_eq!(heights[2 * 16 + 5], 0);
        let packed = pack_heightmap(&heights);
        assert_eq!(packed.len(), 37);
        assert_eq!(packed[0] & 0x1FF, i64::from(heights[0]));
    }

    #[test]
    fn block_count_tracks_air() {
        let mut c = chunk();
        c.set(0, 0, 0, block::STONE);
        c.set(1, 0, 0, block::STONE);
        c.set(0, 0, 0, block::CAVE_AIR);
        assert_eq!(c.sections[section_of(0).unwrap().0].block_count, 1);
    }
}
