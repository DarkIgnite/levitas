//! Flat terrain like vanilla's default superflat preset: bedrock, two layers of dirt and
//! a layer of grass.

use levitas_registry::{block, SyncedRegistry};
use levitas_world::{Chunk, Kind, MIN_Y};

/// Bottom to top, starting at `MIN_Y`.
const LAYERS: [u16; 4] = [block::BEDROCK, block::DIRT, block::DIRT, block::GRASS_BLOCK];

/// Y of the first air block above the grass, where players stand.
pub const SURFACE_Y: f64 = (MIN_Y + LAYERS.len() as i32) as f64;

pub struct FlatGenerator {
    /// Every flat chunk is identical, so new chunks are copies of this one.
    template: Chunk,
}

impl FlatGenerator {
    pub fn new() -> Self {
        let biomes = SyncedRegistry::get("minecraft:worldgen/biome").expect("biome registry");
        let plains = biomes.id_of("minecraft:plains").expect("plains biome") as u16;
        let mut template = Chunk::empty(plains, Kind::biomes(biomes.entries.len()));
        for (dy, &state) in LAYERS.iter().enumerate() {
            for x in 0..16 {
                for z in 0..16 {
                    template.set(x, MIN_Y + dy as i32, z, state);
                }
            }
        }
        Self { template }
    }
}

impl crate::terrain::Terrain for FlatGenerator {
    fn generate(&self, _x: i32, _z: i32) -> Chunk {
        self.template.clone()
    }

    fn spawn_point(&self) -> [i32; 3] {
        [0, SURFACE_Y as i32, 0]
    }

    fn is_cheap(&self) -> bool {
        true
    }
}
