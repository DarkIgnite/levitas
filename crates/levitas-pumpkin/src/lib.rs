//! Vanilla-style terrain from Pumpkin's world generator.
//!
//! [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin) (GPL-3.0) reimplements Minecraft's
//! noise terrain, biomes, carvers, features, structures and lighting in Rust. Levitas uses
//! its `pumpkin-world` crate as a library and translates each generated chunk. Pumpkin
//! targets a newer Minecraft version, so its block states and biomes are matched to
//! 1.21.8 by name and properties, and the blocks 1.21.8 does not have yet become their
//! closest older relative (see [`older_name`]).

use levitas_registry::{block, BlockInfo, SyncedRegistry};
use levitas_world::chunk::{Section, SECTIONS};
use levitas_world::light::LIGHT_SECTIONS;
use levitas_world::{is_air, Chunk, ChunkLight, Kind, LightLayer, PalettedContainer, MAX_Y, MIN_Y};
use pumpkin_data::chunk::Biome;
use pumpkin_data::dimension::Dimension;
use pumpkin_data::{BlockStateId, Mirror, Rotation};
use pumpkin_util::world_seed::Seed;
use pumpkin_world::chunk::format::LightContainer;
use pumpkin_world::chunk_system::{generate_single_chunk, Chunk as PumpkinChunk, StagedChunkEnum};
use pumpkin_world::generation::generator::WorldGenerator;
use pumpkin_world::generation::get_world_gen;
use pumpkin_world::world::{BlockAccessor, WorldPortalExt};

/// Overworld terrain for one seed.
pub struct PumpkinTerrain {
    generator: Box<WorldGenerator>,
    /// The 1.21.8 state of every Pumpkin block state, by Pumpkin state ID.
    states: Box<[u16]>,
    /// The biome registry index of every Pumpkin biome, by Pumpkin biome ID.
    biomes: Box<[u16]>,
    biome_kind: Kind,
}

impl PumpkinTerrain {
    pub fn new(seed: i64) -> Self {
        let generator = get_world_gen(
            Seed(seed as u64),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let registry = SyncedRegistry::get("minecraft:worldgen/biome").expect("biome registry");
        let plains = registry.id_of("minecraft:plains").expect("plains biome") as u16;
        let biomes = (0..=u8::MAX)
            .map(|id| {
                Biome::from_id(id)
                    .and_then(|b| {
                        registry.id_of(&format!("minecraft:{}", older_biome(b.registry_id)))
                    })
                    .map_or(plains, |i| i as u16)
            })
            .collect();
        let states = (0..BlockStateId::COUNT)
            .map(|id| map_state(BlockStateId::new_or_air(id)).unwrap_or_else(|s| s))
            .collect();
        Self {
            generator,
            states,
            biomes,
            biome_kind: Kind::biomes(registry.entries.len()),
        }
    }

    /// Generates chunk (`x`, `z`) with its features, structures and light.
    pub fn generate(&self, x: i32, z: i32) -> Chunk {
        let proto =
            match generate_single_chunk(&self.generator, &Portals, x, z, StagedChunkEnum::Lighting)
            {
                PumpkinChunk::Proto(proto) => proto,
                PumpkinChunk::Level(_) => unreachable!("generation stops before the full stage"),
            };

        let mut blocks = vec![0u16; 4096];
        let mut biomes = [0u16; 64];
        let sections = (0..SECTIONS)
            .map(|s| {
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let id = proto.get_block_state_raw(x, (s * 16 + y) as i32, z);
                            blocks[(y * 16 + z as usize) * 16 + x as usize] =
                                self.states[usize::from(id.as_u16())];
                        }
                    }
                }
                // Biome y counts in 4-block cells from the bottom of the world.
                for y in 0..4 {
                    for z in 0..4 {
                        for x in 0..4 {
                            let cell_y = (MIN_Y >> 2) + (s * 4 + y) as i32;
                            let id = proto.get_biome_id(x, cell_y, z);
                            biomes[(y * 4 + z as usize) * 4 + x as usize] =
                                self.biomes[usize::from(id)];
                        }
                    }
                }
                Section::from_parts(
                    PalettedContainer::from_values(Kind::blocks(), &blocks),
                    PalettedContainer::from_values(self.biome_kind, &biomes),
                )
            })
            .collect();

        let mut chunk = Chunk::from_sections(sections);
        chunk.light = light(&proto.light.sky_light, &proto.light.block_light);
        chunk
    }

    /// Where players first spawn: the spot Pumpkin's spawn finder picks, standing on the
    /// highest block there that is not part of a tree.
    pub fn spawn_point(&self) -> [i32; 3] {
        let pos = self.generator.find_spawn_position().0;
        let chunk = self.generate(pos.x >> 4, pos.z >> 4);
        let (lx, lz) = ((pos.x & 15) as usize, (pos.z & 15) as usize);
        let ground = (MIN_Y..=MAX_Y).rev().find(|&y| {
            let state = chunk.get(lx, y, lz);
            !is_air(state)
                && !levitas_registry::block_has_tag(state, "minecraft:leaves")
                && !levitas_registry::block_has_tag(state, "minecraft:logs")
        });
        [pos.x, ground.map_or(64, |y| y + 1), pos.z]
    }
}

/// Pumpkin keeps light for the world's sections; Levitas also has one below and one
/// above, which stay dark and fully lit.
fn light(sky: &[LightContainer], block: &[LightContainer]) -> ChunkLight {
    fn layer(container: &LightContainer) -> LightLayer {
        match container {
            LightContainer::Empty(level) => LightLayer::Uniform(*level),
            LightContainer::Full(nibbles) => LightLayer::from_nibbles(nibbles),
        }
    }
    let mut light = ChunkLight::full_sky();
    let offset = usize::from(sky.len() < LIGHT_SECTIONS);
    if offset == 1 {
        light.sky[0] = LightLayer::Uniform(0);
    }
    for (i, container) in sky.iter().enumerate().take(LIGHT_SECTIONS - offset) {
        light.sky[i + offset] = layer(container);
    }
    for (i, container) in block.iter().enumerate().take(LIGHT_SECTIONS - offset) {
        light.block[i + offset] = layer(container);
    }
    light
}

/// The 1.21.8 state for a Pumpkin state: the block of the same name (or its older
/// relative) with the same property values. `Err` holds a guess for blocks with no
/// match: stone if it has a collision box, air otherwise.
fn map_state(id: BlockStateId) -> Result<u16, u16> {
    let pumpkin = id.to_block();
    let name = format!("minecraft:{}", pumpkin.name);
    let info = match BlockInfo::by_name(&name) {
        Some(info) => info,
        None => match older_name(pumpkin.name) {
            Some("air") => return Ok(block::AIR),
            Some(older) => match BlockInfo::by_name(&format!("minecraft:{older}")) {
                Some(info) => info,
                None => return Err(guess(id)),
            },
            None => return Err(guess(id)),
        },
    };
    let props = pumpkin
        .properties(id)
        .map(|p| p.to_props())
        .unwrap_or_default();
    Ok(info.state_with(props.iter().map(|p| (p.key_str(), p.value_str()))))
}

fn guess(id: BlockStateId) -> u16 {
    if id.to_state().collision_shapes.is_empty() {
        block::AIR
    } else {
        block::STONE
    }
}

/// The 1.21.8 block standing in for one added after it (names without the namespace).
fn older_name(name: &str) -> Option<&'static str> {
    let exact = match name {
        "golden_dandelion" => "dandelion",
        "potted_golden_dandelion" => "potted_dandelion",
        "shelf_mushroom" => "air",
        "straw_bed" => "hay_block",
        "iron_chain" => "chain",
        "sulfur_spike" => "pointed_dripstone",
        "sulfur" | "potent_sulfur" => "yellow_terracotta",
        "sulfur_slab" => "sandstone_slab",
        "sulfur_stairs" | "sulfur_brick_stairs" => "sandstone_stairs",
        "sulfur_wall" | "polished_sulfur_wall" | "sulfur_brick_wall" => "sandstone_wall",
        "polished_sulfur" => "smooth_sandstone",
        "polished_sulfur_slab" => "smooth_sandstone_slab",
        "polished_sulfur_stairs" => "smooth_sandstone_stairs",
        "sulfur_bricks" => "cut_sandstone",
        "sulfur_brick_slab" => "cut_sandstone_slab",
        "chiseled_sulfur" => "chiseled_sandstone",
        "cinnabar" => "granite",
        "cinnabar_slab" => "granite_slab",
        "cinnabar_stairs" => "granite_stairs",
        "cinnabar_wall" | "polished_cinnabar_wall" | "cinnabar_brick_wall" => "granite_wall",
        "polished_cinnabar" | "cinnabar_bricks" | "chiseled_cinnabar" => "polished_granite",
        "polished_cinnabar_slab" | "cinnabar_brick_slab" => "polished_granite_slab",
        "polished_cinnabar_stairs" | "cinnabar_brick_stairs" => "polished_granite_stairs",
        _ => "",
    };
    if !exact.is_empty() {
        return Some(exact);
    }
    if name.ends_with("_shelf") {
        return Some("air");
    }
    if name.ends_with("_poplar_leaves") {
        return Some("birch_leaves");
    }
    if name.contains("poplar") {
        return BlockInfo::by_name(&format!("minecraft:{}", name.replace("poplar", "birch")))
            .map(|info| &info.name["minecraft:".len()..]);
    }
    if name.ends_with("_shrub") {
        return Some("bush");
    }
    // Wool and concrete slabs and stairs become the full block.
    if let Some(base) = name
        .strip_suffix("_slab")
        .or_else(|| name.strip_suffix("_stairs"))
        .filter(|base| base.ends_with("_wool") || base.ends_with("_concrete"))
    {
        return BlockInfo::by_name(&format!("minecraft:{base}"))
            .map(|info| &info.name["minecraft:".len()..]);
    }
    // Copper versions of older blocks, in any oxidation stage, waxed or not.
    let copper = name.strip_prefix("waxed_").unwrap_or(name);
    let copper = ["exposed_", "weathered_", "oxidized_"]
        .iter()
        .find_map(|stage| copper.strip_prefix(stage))
        .unwrap_or(copper);
    match copper {
        "copper_chain" => Some("chain"),
        "copper_bars" => Some("iron_bars"),
        "copper_chest" => Some("chest"),
        "copper_lantern" => Some("lantern"),
        "copper_torch" => Some("torch"),
        "copper_wall_torch" => Some("wall_torch"),
        "lightning_rod" => Some("lightning_rod"),
        "copper_golem_statue" => Some("air"),
        _ => None,
    }
}

/// Biomes added after 1.21.8 become the closest older one.
fn older_biome(name: &str) -> &str {
    match name {
        "dappled_forest" => "forest",
        "sulfur_caves" => "dripstone_caves",
        other => other,
    }
}

/// What Pumpkin's feature placement asks of the world. Levitas places every block a
/// feature wants and spawns no mobs during generation yet.
struct Portals;

impl WorldPortalExt for Portals {
    fn can_place_at(
        &self,
        _block: &pumpkin_data::Block,
        _state: &pumpkin_data::BlockState,
        _world: &dyn BlockAccessor,
        _pos: &pumpkin_util::math::position::BlockPos,
    ) -> bool {
        true
    }

    fn mirror(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        mirror: Mirror,
    ) -> &'static pumpkin_data::BlockState {
        block.mirror(state_id, mirror)
    }

    fn rotate(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static pumpkin_data::BlockState {
        block.rotate(state_id, rotation)
    }

    fn spawn_mobs_for_chunk_generation(
        &self,
        _cache: &mut dyn pumpkin_world::generation::proto_chunk::GenerationCache,
        _biome: &'static Biome,
        _chunk_x: i32,
        _chunk_z: i32,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pumpkin_state_has_a_match() {
        let unmatched: Vec<String> = (0..BlockStateId::COUNT)
            .filter_map(BlockStateId::new)
            .filter(|&id| map_state(id).is_err())
            .map(|id| id.to_block().name.to_owned())
            .collect();
        assert!(unmatched.is_empty(), "no 1.21.8 block for {unmatched:?}");
    }

    #[test]
    fn states_keep_their_properties() {
        let stairs: Vec<BlockStateId> = (0..BlockStateId::COUNT)
            .filter_map(BlockStateId::new)
            .filter(|id| id.to_block().name == "oak_stairs")
            .collect();
        assert_eq!(stairs.len(), 80);
        for id in stairs {
            let ours = map_state(id).unwrap();
            let info = BlockInfo::of(ours);
            assert_eq!(info.name, "minecraft:oak_stairs");
            for p in id.to_block().properties(id).unwrap().to_props() {
                assert_eq!(info.property(ours, p.key_str()), Some(p.value_str()));
            }
        }
    }

    #[test]
    fn generates_terrain_with_light() {
        let terrain = PumpkinTerrain::new(12345);
        let chunk = terrain.generate(0, 0);
        assert_eq!(chunk.get(0, MIN_Y, 0), block::BEDROCK);
        let top = (MIN_Y..=MAX_Y)
            .rev()
            .find(|&y| !is_air(chunk.get(8, y, 8)))
            .unwrap();
        assert!(top > 0, "surface at y {top}");
        // Daylight above the surface, darkness in the bedrock.
        let sky =
            |y: i32| chunk.light.sky[((y - MIN_Y) >> 4) as usize + 1].get(8, (y & 15) as usize, 8);
        assert_eq!(sky(top + 1), 15);
        assert_eq!(sky(MIN_Y), 0);
        assert_eq!(chunk.light.sky[LIGHT_SECTIONS - 1], LightLayer::Uniform(15));
    }
}
