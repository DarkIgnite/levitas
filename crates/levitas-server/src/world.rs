//! A flat overworld for testing: bedrock, two layers of dirt and a layer of grass, like
//! vanilla's default superflat preset. Every chunk is identical, so the chunk payload is
//! encoded once and reused.

use levitas_protocol::{varint, Writer};
use levitas_registry::packet::play::clientbound;
use levitas_registry::{block, SyncedRegistry};

/// Lowest block Y in the overworld.
pub const MIN_Y: i32 = -64;
/// The overworld is 384 blocks tall.
const SECTIONS: usize = 24;
/// Light data covers one extra section below and above the world.
const LIGHT_SECTIONS: usize = SECTIONS + 2;
/// Y of the first air block above the grass, where players stand.
pub const SURFACE_Y: f64 = (MIN_Y + 4) as f64;

pub struct FlatWorld {
    /// Everything in a chunk packet after the chunk coordinates.
    chunk_body: Vec<u8>,
}

impl FlatWorld {
    pub fn new() -> Self {
        let plains = SyncedRegistry::get("minecraft:worldgen/biome")
            .and_then(|r| r.id_of("minecraft:plains"))
            .expect("plains biome is registered");

        let mut sections = Vec::new();
        bottom_section(&mut sections, plains);
        for _ in 1..SECTIONS {
            empty_section(&mut sections, plains);
        }

        let mut body = Vec::new();
        // Heightmaps: none; the client computes what it needs.
        varint::write(&mut body, 0);
        varint::write(&mut body, sections.len() as i32);
        body.extend_from_slice(&sections);
        // Block entities: none.
        varint::write(&mut body, 0);
        full_sky_light(&mut body);

        Self { chunk_body: body }
    }

    pub fn chunk_packet(&self, x: i32, z: i32) -> Writer {
        Writer::packet(clientbound::LEVEL_CHUNK_WITH_LIGHT)
            .i32(x)
            .i32(z)
            .bytes(&self.chunk_body)
    }
}

/// The lowest 16 blocks: bedrock at y=0, dirt at y=1..=2, grass at y=3, air above.
fn bottom_section(out: &mut Vec<u8>, biome: i32) {
    let layers = [block::BEDROCK, block::DIRT, block::DIRT, block::GRASS_BLOCK];
    out.extend_from_slice(&(16 * 16 * layers.len() as i16).to_be_bytes());

    // Indirect palette, 4 bits per block: index 0 is air, then the layer blocks.
    let palette = [block::AIR, block::BEDROCK, block::DIRT, block::GRASS_BLOCK];
    out.push(4);
    varint::write(out, palette.len() as i32);
    for id in palette {
        varint::write(out, i32::from(id));
    }
    // 4096 entries ordered y, z, x; 16 entries per long, so each long is one row of x.
    for y in 0..16 {
        let index = layers
            .get(y)
            .map_or(0, |b| palette.iter().position(|p| p == b).unwrap());
        let long = index as u64 * 0x1111_1111_1111_1111;
        for _row in 0..16 {
            out.extend_from_slice(&long.to_be_bytes());
        }
    }
    single_biome(out, biome);
}

fn empty_section(out: &mut Vec<u8>, biome: i32) {
    out.extend_from_slice(&0i16.to_be_bytes());
    // Single-value palette: 0 bits per entry, then the value, and no data array.
    out.push(0);
    varint::write(out, i32::from(block::AIR));
    single_biome(out, biome);
}

fn single_biome(out: &mut Vec<u8>, biome: i32) {
    out.push(0);
    varint::write(out, biome);
}

/// Full sky light everywhere and no block light.
fn full_sky_light(out: &mut Vec<u8>) {
    let all = (1u64 << LIGHT_SECTIONS) - 1;
    let bitset = |out: &mut Vec<u8>, bits: u64| {
        if bits == 0 {
            varint::write(out, 0);
        } else {
            varint::write(out, 1);
            out.extend_from_slice(&bits.to_be_bytes());
        }
    };
    bitset(out, all); // sky light mask
    bitset(out, 0); // block light mask
    bitset(out, 0); // empty sky light mask
    bitset(out, all); // empty block light mask
    varint::write(out, LIGHT_SECTIONS as i32);
    for _ in 0..LIGHT_SECTIONS {
        varint::write(out, 2048);
        out.resize(out.len() + 2048, 0xFF);
    }
    varint::write(out, 0); // block light arrays
}
