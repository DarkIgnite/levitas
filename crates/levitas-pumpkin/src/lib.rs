//! Vanilla-style terrain from Pumpkin's world generator.
//!
//! [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin) (GPL-3.0) reimplements Minecraft's
//! noise terrain, biomes, carvers, features, structures and lighting in Rust. Levitas uses
//! its `pumpkin-world` crate as a library and translates each generated chunk. Its block
//! states and biomes are matched to 26.3's by name and properties.

use levitas_nbt::{Compound, Tag};
use levitas_registry::{block, BlockInfo, SyncedRegistry};
use levitas_world::chunk::{Section, SECTIONS};
use levitas_world::light::LIGHT_SECTIONS;
pub use levitas_world::Generated;
use levitas_world::{
    is_air, Chunk, ChunkLight, Kind, LightLayer, PalettedContainer, Span, MAX_Y, MIN_Y,
};
use pumpkin_config::lighting::LightingEngineConfig;
use pumpkin_data::chunk::Biome;
use pumpkin_data::dimension::Dimension;
use pumpkin_data::structures::StructureSet;
use pumpkin_data::{BlockStateId, Mirror, Rotation};
use pumpkin_util::world_seed::Seed;
use pumpkin_world::chunk::format::LightContainer;
use pumpkin_world::chunk_system::generation_cache::Cache;
use pumpkin_world::chunk_system::{Chunk as PumpkinChunk, StagedChunkEnum};
use pumpkin_world::generation::generator::structure_finder::{
    find_nearest_structure, StructureSearch,
};
use pumpkin_world::generation::generator::WorldGenerator;
use pumpkin_world::generation::get_world_gen;
use pumpkin_world::generation::proto_chunk::ProtoChunk;
use pumpkin_world::world::{BlockAccessor, WorldPortalExt};

/// Chunks per side of the square areas generated together.
pub const AREA: i32 = 8;

/// How far around an area each step must run so the area's own chunks come out
/// complete: features (trees, ores, structures) of the chunks next to it reach into it,
/// those features need their neighbors' terrain, and terrain surfaces need their
/// neighbors' biomes.
const STEPS: [(StagedChunkEnum, i32, i32); 8] = [
    // (step, margin around the area, radius of chunks the step reads and writes)
    (StagedChunkEnum::Biomes, 3, 0),
    (StagedChunkEnum::StructureStart, 3, 0),
    (StagedChunkEnum::StructureReferences, 3, 0),
    (StagedChunkEnum::Noise, 2, 0),
    (StagedChunkEnum::Surface, 2, 1),
    (StagedChunkEnum::Carvers, 2, 0),
    (StagedChunkEnum::Features, 1, 1),
    (StagedChunkEnum::Lighting, 0, 1),
];
const MARGIN: i32 = 3;

/// Terrain of one dimension for one seed.
pub struct PumpkinTerrain {
    generator: Box<WorldGenerator>,
    seed: i64,
    /// The sections the dimension's terrain fills; the rest stay empty.
    span: Span,
    /// The 26.3 state of every Pumpkin block state, by Pumpkin state ID.
    states: Box<[u16]>,
    /// The biome registry index of every Pumpkin biome, by Pumpkin biome ID.
    biomes: Box<[u16]>,
    biome_kind: Kind,
}

impl PumpkinTerrain {
    /// Overworld terrain.
    pub fn new(seed: i64) -> Self {
        Self::of(seed, Dimension::OVERWORLD, Span::OVERWORLD)
    }

    /// Nether terrain, from y = 0 to 255.
    pub fn nether(seed: i64) -> Self {
        Self::of(seed, Dimension::THE_NETHER, Span::NETHER)
    }

    /// End terrain, from y = 0 to 255.
    pub fn end(seed: i64) -> Self {
        Self::of(seed, Dimension::THE_END, Span::NETHER)
    }

    /// The nearest stronghold to block (`x`, `z`), as vanilla's eye of ender finds it:
    /// `Ok(None)` if there is none, `Err(())` while their places are still being
    /// worked out (on first use), so the caller can try again shortly.
    #[allow(clippy::result_unit_err)]
    pub fn stronghold_near(&self, x: i32, z: i32) -> Result<Option<[i32; 3]>, ()> {
        let Some(cache) = self.generator.global_structure_cache() else {
            return Ok(None);
        };
        let Some(strongholds) = StructureSet::get("strongholds") else {
            return Ok(None);
        };
        match find_nearest_structure(
            pumpkin_util::math::position::BlockPos::new(x, 64, z),
            &[&strongholds.placement],
            100,
            self.seed,
            cache,
        ) {
            StructureSearch::Found(pos) => Ok(Some([pos.0.x, pos.0.y, pos.0.z])),
            StructureSearch::NotFound => Ok(None),
            StructureSearch::Pending => Err(()),
        }
    }

    fn of(seed: i64, dimension: Dimension, span: Span) -> Self {
        let generator = get_world_gen(
            Seed(seed as u64),
            dimension,
            false,
            Vec::new(),
            String::new(),
        );
        let registry = SyncedRegistry::get("minecraft:worldgen/biome").expect("biome registry");
        let plains = registry.id_of("minecraft:plains").expect("plains biome") as u16;
        let biomes = (0..=u8::MAX)
            .map(|id| {
                Biome::from_id(id)
                    .and_then(|b| registry.id_of(&format!("minecraft:{}", b.registry_id)))
                    .map_or(plains, |i| i as u16)
            })
            .collect();
        let states = (0..BlockStateId::COUNT)
            .map(|id| map_state(BlockStateId::new_or_air(id)).unwrap_or_else(|s| s))
            .collect();
        Self {
            generator,
            seed,
            span,
            states,
            biomes,
            biome_kind: Kind::biomes(registry.entries.len()),
        }
    }

    /// Generates the area of [`AREA`] x [`AREA`] chunks with index (`ax`, `az`), the
    /// one holding chunks `ax * AREA..(ax + 1) * AREA` along x and likewise along z.
    pub fn generate_area(&self, ax: i32, az: i32) -> Vec<Generated> {
        self.generate_region(ax * AREA, az * AREA, AREA)
    }

    /// Generates one chunk, together with the rest of its area.
    pub fn generate(&self, x: i32, z: i32) -> Chunk {
        let area = self.generate_area(x.div_euclid(AREA), z.div_euclid(AREA));
        area.into_iter()
            .find(|g| g.pos == (x, z))
            .expect("the area holds the chunk")
            .chunk
    }

    /// Generates the `size` x `size` chunks starting at chunk (`x0`, `z0`).
    fn generate_region(&self, x0: i32, z0: i32, size: i32) -> Vec<Generated> {
        let mut grid = Grid {
            x0: x0 - MARGIN,
            z0: z0 - MARGIN,
            size: size + 2 * MARGIN,
            chunks: Vec::new(),
        };
        for x in grid.x0..grid.x0 + grid.size {
            for z in grid.z0..grid.z0 + grid.size {
                let proto = ProtoChunk::new(x, z, &self.generator);
                grid.chunks.push(Some(PumpkinChunk::Proto(Box::new(proto))));
            }
        }
        for (step, margin, radius) in STEPS {
            for x in x0 - margin..x0 + size + margin {
                for z in z0 - margin..z0 + size + margin {
                    self.advance(&mut grid, x, z, step, radius);
                }
            }
        }
        let mut out = Vec::with_capacity((size * size) as usize);
        for x in x0..x0 + size {
            for z in z0..z0 + size {
                let i = grid.index(x, z);
                match grid.chunks[i].take() {
                    Some(PumpkinChunk::Proto(mut proto)) => out.push(Generated {
                        pos: (x, z),
                        chunk: self.translate(&proto),
                        block_entities: proto.pending_block_entities.iter().map(compound).collect(),
                        entities: proto
                            .take_pending_structure_entities()
                            .iter()
                            .map(compound)
                            .collect(),
                    }),
                    _ => unreachable!("generation stops before the full stage"),
                }
            }
        }
        out
    }

    /// Runs one generation step on chunk (`x`, `z`), lending it the chunks around it.
    fn advance(&self, grid: &mut Grid, x: i32, z: i32, step: StagedChunkEnum, radius: i32) {
        let side = 2 * radius + 1;
        let mut cache = Cache::new(x - radius, z - radius, side);
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let i = grid.index(x + dx, z + dz);
                cache
                    .chunks
                    .push(grid.chunks[i].take().expect("chunk is in the grid"));
            }
        }
        cache.advance(
            step,
            &self.generator,
            &Portals,
            &LightingEngineConfig::Default,
        );
        for (n, chunk) in cache.chunks.into_iter().enumerate() {
            let (dx, dz) = (n as i32 / side - radius, n as i32 % side - radius);
            let i = grid.index(x + dx, z + dz);
            grid.chunks[i] = Some(chunk);
        }
    }

    /// A generated Pumpkin chunk as a 26.3 chunk.
    fn translate(&self, proto: &ProtoChunk) -> Chunk {
        let mut blocks = vec![0u16; 4096];
        let mut biomes = [0u16; 64];
        let span = self.span;
        let sections = (0..SECTIONS)
            .map(|index| {
                // Sections outside the dimension's height stay empty, with the biome of
                // the nearest section that has one.
                let s = index.clamp(span.first, span.first + span.count - 1) - span.first;
                let inside = (span.first..span.first + span.count).contains(&index);
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let id = if inside {
                                proto.get_block_state_raw(x, (s * 16 + y) as i32, z)
                            } else {
                                BlockStateId::AIR
                            };
                            blocks[(y * 16 + z as usize) * 16 + x as usize] =
                                self.states[usize::from(id.as_u16())];
                        }
                    }
                }
                // Biome y counts in 4-block cells from the bottom of the world.
                for y in 0..4 {
                    for z in 0..4 {
                        for x in 0..4 {
                            let cell_y = (span.min_y() >> 2) + (s * 4 + y) as i32;
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
        chunk.light = light(&proto.light.sky_light, &proto.light.block_light, span);
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

/// Chunks being generated, in a square around an area.
struct Grid {
    x0: i32,
    z0: i32,
    size: i32,
    /// By x, then z; a chunk is out while a step borrows it.
    chunks: Vec<Option<PumpkinChunk>>,
}

impl Grid {
    fn index(&self, x: i32, z: i32) -> usize {
        ((x - self.x0) * self.size + (z - self.z0)) as usize
    }
}

/// Pumpkin keeps light for the world's sections; Levitas also has one below and one
/// above, which stay dark and fully lit.
fn light(sky: &[LightContainer], block: &[LightContainer], span: Span) -> ChunkLight {
    fn layer(container: &LightContainer) -> LightLayer {
        match container {
            LightContainer::Empty(level) => LightLayer::Uniform(*level),
            LightContainer::Full(nibbles) => LightLayer::from_nibbles(nibbles),
        }
    }
    let mut light = ChunkLight::full_sky();
    if span != Span::OVERWORLD {
        // A dimension without a sky (the Nether) has no sky light at all.
        light.sky = vec![LightLayer::Uniform(0); LIGHT_SECTIONS];
        for (i, container) in block.iter().enumerate().take(span.count + 2) {
            light.block[span.first + i] = layer(container);
        }
        return light;
    }
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

/// The 26.3 state for a Pumpkin state: the block of the same name with the same
/// property values. `Err` holds a guess for blocks with no
/// match: stone if it has a collision box, air otherwise.
fn map_state(id: BlockStateId) -> Result<u16, u16> {
    let pumpkin = id.to_block();
    let name = format!("minecraft:{}", pumpkin.name);
    let Some(info) = BlockInfo::by_name(&name) else {
        return Err(guess(id));
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

/// What Pumpkin's feature placement asks of the world: whether a plant may grow where a
/// feature wants it. Levitas spawns no mobs during generation yet.
struct Portals;

impl WorldPortalExt for Portals {
    // Adapted from Pumpkin's `TestBlockRegistry` (pumpkin-world,
    // generation/proto_chunk_test/stage_parity.rs), Copyright the Pumpkin contributors,
    // GPL-3.0. Levitas adds the upper halves of tall plants, the water next to sugar cane
    // and the free sides of cacti.
    fn can_place_at(
        &self,
        block: &pumpkin_data::Block,
        state: &pumpkin_data::BlockState,
        world: &dyn BlockAccessor,
        pos: &pumpkin_util::math::position::BlockPos,
    ) -> bool {
        use pumpkin_data::tag::{Block as Tag, Taggable};
        use pumpkin_data::Block;

        let below = world.get_block(&pos.down());
        let upper_half = block
            .properties(state.id)
            .map(|p| p.to_props())
            .unwrap_or_default()
            .iter()
            .any(|p| p.key_str() == "half" && p.value_str() == "upper");
        if upper_half && !block.has_tag(&Tag::MINECRAFT_DOORS) {
            return below == block;
        }
        let vegetation = [
            &Block::SHORT_GRASS,
            &Block::FERN,
            &Block::SWEET_BERRY_BUSH,
            &Block::TALL_GRASS,
            &Block::LARGE_FERN,
            &Block::SUNFLOWER,
            &Block::LILAC,
            &Block::ROSE_BUSH,
            &Block::PEONY,
            &Block::WILDFLOWERS,
            &Block::PINK_PETALS,
            &Block::BUSH,
            &Block::FIREFLY_BUSH,
        ];
        if vegetation.contains(&block)
            || block.has_tag(&Tag::MINECRAFT_SMALL_FLOWERS)
            || block.has_tag(&Tag::MINECRAFT_SAPLINGS)
        {
            return below.has_tag(&Tag::MINECRAFT_SUPPORTS_VEGETATION);
        }
        if block == &Block::DEAD_BUSH
            || block == &Block::SHORT_DRY_GRASS
            || block == &Block::TALL_DRY_GRASS
        {
            return below.has_tag(&Tag::MINECRAFT_SUPPORTS_DRY_VEGETATION);
        }
        if block == &Block::CACTUS {
            let sides = [pos.north(), pos.south(), pos.east(), pos.west()];
            return below.has_tag(&Tag::MINECRAFT_SUPPORTS_CACTUS)
                && sides
                    .iter()
                    .all(|side| world.get_block_state(side).collision_shapes.is_empty());
        }
        if block == &Block::SUGAR_CANE {
            if below == &Block::SUGAR_CANE {
                return true;
            }
            let ground = pos.down();
            let sides = [ground.north(), ground.south(), ground.east(), ground.west()];
            return below.has_tag(&Tag::MINECRAFT_SUPPORTS_SUGAR_CANE)
                && sides.iter().any(|side| {
                    let b = world.get_block(side);
                    b == &Block::WATER || b == &Block::FROSTED_ICE
                });
        }
        if block == &Block::BAMBOO || block == &Block::BAMBOO_SAPLING {
            return below.has_tag(&Tag::MINECRAFT_SUPPORTS_BAMBOO);
        }
        if block == &Block::BROWN_MUSHROOM || block == &Block::RED_MUSHROOM {
            return below.has_tag(&Tag::MINECRAFT_OVERRIDES_MUSHROOM_LIGHT_REQUIREMENT)
                || world.get_block_state(&pos.down()).is_solid_render();
        }
        if block == &Block::LILY_PAD {
            return below == &Block::WATER || below.has_tag(&Tag::MINECRAFT_SUPPORTS_LILY_PAD);
        }
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

/// A Pumpkin NBT compound in Levitas's form.
fn compound(c: &pumpkin_nbt::compound::NbtCompound) -> Compound {
    let mut out = Compound::new();
    for (name, value) in &c.child_tags {
        if let Some(tag) = tag(value) {
            out.insert(name.to_string(), tag);
        }
    }
    out
}

fn tag(t: &pumpkin_nbt::tag::NbtTag) -> Option<Tag> {
    use pumpkin_nbt::tag::NbtTag;
    Some(match t {
        NbtTag::End => return None,
        NbtTag::Byte(v) => Tag::Byte(*v),
        NbtTag::Short(v) => Tag::Short(*v),
        NbtTag::Int(v) => Tag::Int(*v),
        NbtTag::Long(v) => Tag::Long(*v),
        NbtTag::Float(v) => Tag::Float(*v),
        NbtTag::Double(v) => Tag::Double(*v),
        NbtTag::ByteArray(v) => Tag::ByteArray(v.to_vec()),
        NbtTag::String(v) => Tag::String(v.to_string()),
        NbtTag::List(v) => Tag::List(v.iter().filter_map(tag).collect()),
        NbtTag::Compound(v) => Tag::Compound(compound(v)),
        NbtTag::IntArray(v) => Tag::IntArray(v.clone()),
        NbtTag::LongArray(v) => Tag::LongArray(v.clone()),
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_end_has_its_main_island() {
        let end = PumpkinTerrain::end(1);
        let chunk = end.generate(0, 0);
        let end_stone = (0..256)
            .filter(|&y| BlockInfo::of(chunk.get(8, y, 8)).name == "minecraft:end_stone")
            .count();
        assert!(end_stone > 10, "{end_stone}");
    }

    #[test]
    fn strongholds_lie_in_the_first_ring() {
        let overworld = PumpkinTerrain::new(1);
        let start = std::time::Instant::now();
        let found = loop {
            match overworld.stronghold_near(0, 0) {
                Ok(found) => break found,
                Err(()) => {
                    assert!(start.elapsed().as_secs() < 120, "still working them out");
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        };
        let [x, _, z] = found.expect("a stronghold");
        let distance = f64::from(x).hypot(f64::from(z));
        assert!((1000.0..3200.0).contains(&distance), "{distance}");
    }

    #[test]
    fn nether_has_a_bedrock_floor_and_roof_and_netherrack_between() {
        let nether = PumpkinTerrain::nether(1);
        let chunk = nether.generate(0, 0);
        let name = |y: i32| BlockInfo::of(chunk.get(3, y, 3)).name;
        assert_eq!(name(0), "minecraft:bedrock");
        assert_eq!(name(127), "minecraft:bedrock");
        assert!(is_air(chunk.get(3, -10, 3)), "nothing below the Nether");
        let netherrack = (1..127)
            .filter(|&y| name(y) == "minecraft:netherrack")
            .count();
        assert!(netherrack > 10, "{netherrack}");
    }

    use super::*;

    #[test]
    fn every_pumpkin_state_has_a_match() {
        let unmatched: Vec<String> = (0..BlockStateId::COUNT)
            .filter_map(BlockStateId::new)
            .filter(|&id| map_state(id).is_err())
            .map(|id| id.to_block().name.to_owned())
            .collect();
        assert!(unmatched.is_empty(), "no 26.3 block for {unmatched:?}");
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
    fn plants_grow_only_on_ground() {
        let terrain = PumpkinTerrain::new(-4254893504593453935);
        let plants = [
            "minecraft:short_grass",
            "minecraft:fern",
            "minecraft:dandelion",
            "minecraft:poppy",
            "minecraft:red_mushroom",
            "minecraft:brown_mushroom",
        ];
        let mut checked = 0;
        for Generated {
            pos: (cx, cz),
            chunk,
            ..
        } in terrain.generate_area(0, 0)
        {
            {
                for x in 0..16 {
                    for z in 0..16 {
                        for y in MIN_Y + 1..=MAX_Y {
                            let name = BlockInfo::of(chunk.get(x, y, z)).name;
                            if !plants.contains(&name) {
                                continue;
                            }
                            checked += 1;
                            let below = chunk.get(x, y - 1, z);
                            assert!(
                                !is_air(below) && below != block::WATER,
                                "{name} on {} at chunk ({cx}, {cz}) {x} {y} {z}",
                                BlockInfo::of(below).name
                            );
                        }
                    }
                }
            }
        }
        assert!(checked > 0, "no plants generated");
    }

    /// Trees are whole even where they cross a chunk border: right below the top of an
    /// oak or birch trunk, leaves surround the log on all four sides.
    #[test]
    fn trees_are_whole_across_chunk_borders() {
        let terrain = PumpkinTerrain::new(-4254893504593453935);
        let chunks: std::collections::HashMap<(i32, i32), Chunk> = terrain
            .generate_area(0, -6)
            .into_iter()
            .map(|g| (g.pos, g.chunk))
            .collect();
        let at = |x: i32, y: i32, z: i32| {
            chunks
                .get(&(x.div_euclid(16), z.div_euclid(16)))
                .map(|c| BlockInfo::of(c.get((x & 15) as usize, y, (z & 15) as usize)).name)
        };
        let (mut tops, mut cut) = (0, Vec::new());
        // A wooded area of this seed.
        let (x0, z0, edge) = (0, -6 * AREA * 16, AREA * 16);
        for x in x0 + 1..x0 + edge - 1 {
            for z in z0 + 1..z0 + edge - 1 {
                for y in 0..MAX_Y {
                    let log = at(x, y, z).unwrap();
                    let leaves = match log {
                        "minecraft:oak_log" => "minecraft:oak_leaves",
                        "minecraft:birch_log" => "minecraft:birch_leaves",
                        _ => continue,
                    };
                    // The top log of a trunk, with its crown above.
                    if at(x, y + 1, z) != Some(leaves) || at(x, y - 1, z) != Some(log) {
                        continue;
                    }
                    tops += 1;
                    // Crowns of neighboring trees mix; big oaks branch out sideways.
                    let sides: Vec<Option<&str>> = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .map(|(dx, dz)| at(x + dx, y, z + dz))
                        .collect();
                    if sides.contains(&Some(log)) {
                        continue;
                    }
                    if sides.iter().flatten().any(|b| !b.ends_with("_leaves")) {
                        cut.push((x, y, z));
                    }
                }
            }
        }
        assert!(tops > 0, "no trees in the area");
        assert!(
            cut.is_empty(),
            "{} of {tops} trees cut at {cut:?}",
            cut.len()
        );
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

    /// Structures and dungeons leave their block entities with the chunk: chests naming
    /// the loot table they are to be filled from.
    #[test]
    fn generated_chests_name_their_loot_table() {
        let terrain = PumpkinTerrain::new(-4254893504593453935);
        let mut tables = Vec::new();
        for (ax, az) in [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1), (1, 1)] {
            for g in terrain.generate_area(ax, az) {
                for nbt in &g.block_entities {
                    let (x, z) = (nbt.int("x").unwrap(), nbt.int("z").unwrap());
                    assert_eq!(((x >> 4) as i32, (z >> 4) as i32), g.pos);
                    if let Some(table) = nbt.str("LootTable") {
                        let block = g.chunk.get(
                            (x & 15) as usize,
                            nbt.int("y").unwrap() as i32,
                            (z & 15) as usize,
                        );
                        assert_ne!(block, 0, "{table} in thin air");
                        tables.push(table.to_owned());
                    }
                }
            }
            if !tables.is_empty() {
                break;
            }
        }
        assert!(!tables.is_empty(), "no loot chests in six areas");
        eprintln!("{tables:?}");
    }

    #[test]
    #[ignore]
    fn print_structure_entities() {
        let terrain = PumpkinTerrain::new(-4254893504593453935);
        for (ax, az) in [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1), (1, 1)] {
            for g in terrain.generate_area(ax, az) {
                for e in &g.entities {
                    eprintln!("{:?} {:?}", g.pos, e);
                }
            }
        }
    }
}
