//! Chunk columns: 24 sections of 16x16x16 blocks spanning y = -64..320.

use levitas_protocol::varint;
use levitas_registry::block;

use crate::paletted::{Kind, PalettedContainer};

pub const MIN_Y: i32 = -64;
pub const SECTIONS: usize = 24;
pub const MAX_Y: i32 = MIN_Y + (SECTIONS as i32) * 16 - 1;

pub fn is_air(state: u16) -> bool {
    matches!(state, block::AIR | block::CAVE_AIR | block::VOID_AIR)
}

#[derive(Debug, Clone)]
pub struct Section {
    /// Non-air blocks, sent to the client so it can skip empty sections.
    block_count: i16,
    blocks: PalettedContainer,
    biomes: PalettedContainer,
}

impl Section {
    pub fn empty(biome: u16, biome_kind: Kind) -> Self {
        Self {
            block_count: 0,
            blocks: PalettedContainer::single(Kind::blocks(), block::AIR),
            biomes: PalettedContainer::single(biome_kind, biome),
        }
    }

    /// A section from its block and biome containers.
    pub fn from_parts(blocks: PalettedContainer, biomes: PalettedContainer) -> Self {
        let block_count = blocks.values().iter().filter(|&&s| !is_air(s)).count() as i16;
        Self {
            block_count,
            blocks,
            biomes,
        }
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
        old
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.block_count.to_be_bytes());
        self.blocks.encode(out);
        self.biomes.encode(out);
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    sections: Vec<Section>,
}

impl Chunk {
    pub fn empty(biome: u16, biome_kind: Kind) -> Self {
        Self {
            sections: vec![Section::empty(biome, biome_kind); SECTIONS],
        }
    }

    /// A chunk from its sections, bottom to top; there must be [`SECTIONS`] of them.
    pub fn from_sections(sections: Vec<Section>) -> Self {
        assert_eq!(sections.len(), SECTIONS);
        Self { sections }
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
    pub fn encode_sections(&self, out: &mut Vec<u8>) {
        let mut data = Vec::new();
        for section in &self.sections {
            section.encode(&mut data);
        }
        varint::write(out, data.len() as i32);
        out.extend_from_slice(&data);
    }
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
    fn block_count_tracks_air() {
        let mut c = chunk();
        c.set(0, 0, 0, block::STONE);
        c.set(1, 0, 0, block::STONE);
        c.set(0, 0, 0, block::CAVE_AIR);
        assert_eq!(c.sections[section_of(0).unwrap().0].block_count, 1);
    }
}
