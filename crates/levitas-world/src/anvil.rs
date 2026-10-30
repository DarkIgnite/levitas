//! Chunks in the vanilla on-disk format (the NBT stored in region files): sections with
//! block state and biome palettes, plus the chunk's block entities.

use levitas_nbt::{Compound, Tag};
use levitas_registry::{block, BlockInfo, SyncedRegistry, DATA_VERSION};

use crate::chunk::{Chunk, Section, Span, MIN_Y, SECTIONS};
use crate::light::{ChunkLight, LightLayer, LIGHT_SECTIONS};
use crate::paletted::{bits_for, Kind, PalettedContainer};

/// A chunk read from disk.
pub struct LoadedChunk {
    pub chunk: Chunk,
    /// Block entity NBT as stored (`id`, `x`, `y`, `z` and type-specific fields).
    pub block_entities: Vec<Compound>,
    /// Pending block ticks (`i` block name, `x`, `y`, `z`, `t` ticks left, `p` priority).
    pub block_ticks: Vec<Tag>,
    /// Pending fluid ticks, in the same form with a fluid name in `i`.
    pub fluid_ticks: Vec<Tag>,
}

fn biomes() -> &'static SyncedRegistry {
    SyncedRegistry::get("minecraft:worldgen/biome").expect("biome registry")
}

fn biome_kind() -> Kind {
    Kind::biomes(biomes().entries.len())
}

/// Serializes a chunk as vanilla 26.3 does, so vanilla and Leaf can read it.
#[allow(clippy::too_many_arguments)]
pub fn to_nbt(
    chunk: &Chunk,
    cx: i32,
    cz: i32,
    block_entities: Vec<Compound>,
    block_ticks: Vec<Compound>,
    fluid_ticks: Vec<Compound>,
    game_time: i64,
    span: Span,
) -> Compound {
    let mut sections = Vec::new();
    let nibbles = |layer: &LightLayer| -> Vec<i8> {
        layer.to_nibbles().into_iter().map(|b| b as i8).collect()
    };
    // Light covers a section below and above the world, which hold nothing else.
    for i in span.light() {
        let mut entry = Compound::new()
            .with("Y", (FIRST_LIGHT_Y + i as i32) as i8)
            .with("SkyLight", nibbles(&chunk.light.sky[i]));
        if chunk.light.block[i] != LightLayer::Uniform(0) {
            entry.insert("BlockLight", nibbles(&chunk.light.block[i]));
        }
        let in_span = i > span.first && i <= span.first + span.count;
        if let Some(section) = i
            .checked_sub(1)
            .filter(|_| in_span)
            .and_then(|s| chunk.sections().get(s))
        {
            add_blocks(&mut entry, section);
        }
        sections.push(Tag::Compound(entry));
    }
    Compound::new()
        .with("DataVersion", DATA_VERSION)
        .with("xPos", cx)
        .with("zPos", cz)
        .with("yPos", span.min_y() >> 4)
        .with("Status", "minecraft:full")
        .with("LastUpdate", game_time)
        .with("InhabitedTime", 0i64)
        .with("isLightOn", true)
        .with("sections", sections)
        .with(
            "block_entities",
            block_entities
                .into_iter()
                .map(Tag::Compound)
                .collect::<Vec<_>>(),
        )
        .with(
            "block_ticks",
            block_ticks
                .into_iter()
                .map(Tag::Compound)
                .collect::<Vec<_>>(),
        )
        .with(
            "fluid_ticks",
            fluid_ticks
                .into_iter()
                .map(Tag::Compound)
                .collect::<Vec<_>>(),
        )
        .with(
            "structures",
            Compound::new()
                .with("starts", Compound::new())
                .with("References", Compound::new()),
        )
}

/// Section Y of the light-only section below the world.
const FIRST_LIGHT_Y: i32 = (MIN_Y >> 4) - 1;

/// Adds a section's block states and biomes to its saved compound.
fn add_blocks(saved: &mut Compound, section: &Section) {
    let states = palette_and_data(&section.blocks().values(), 4, |state| {
        let info = BlockInfo::of(state);
        let mut entry = Compound::new().with("Name", info.name);
        if !info.properties.is_empty() {
            let mut props = Compound::new();
            for (name, value) in info.properties_of(state) {
                props.insert(name, value);
            }
            entry.insert("Properties", props);
        }
        Tag::Compound(entry)
    });
    let biome_names = biomes().entries;
    let biomes = palette_and_data(&section.biomes().values(), 1, |biome| {
        Tag::String(biome_names[usize::from(biome)].to_owned())
    });
    saved.insert("block_states", states);
    saved.insert("biomes", biomes);
}

/// A palette (first-seen order) and the packed indices; `data` is left out when the
/// palette has a single entry, as vanilla does.
fn palette_and_data(values: &[u16], min_bits: u8, entry: impl Fn(u16) -> Tag) -> Compound {
    let mut palette: Vec<u16> = Vec::new();
    let mut indices = Vec::with_capacity(values.len());
    for &v in values {
        let slot = palette.iter().position(|&p| p == v).unwrap_or_else(|| {
            palette.push(v);
            palette.len() - 1
        });
        indices.push(slot as u64);
    }
    let mut out = Compound::new().with(
        "palette",
        palette.iter().map(|&v| entry(v)).collect::<Vec<_>>(),
    );
    if palette.len() > 1 {
        let bits = bits_for(palette.len()).max(min_bits) as usize;
        let per_long = 64 / bits;
        let mut data = vec![0i64; values.len().div_ceil(per_long)];
        for (i, &slot) in indices.iter().enumerate() {
            data[i / per_long] |= (slot << ((i % per_long) * bits)) as i64;
        }
        out.insert("data", data);
    }
    out
}

/// Unpacks palette indices; `None` if the data does not fit the palette.
fn unpack(
    data: Option<&[i64]>,
    palette_len: usize,
    entries: usize,
    min_bits: u8,
) -> Option<Vec<usize>> {
    if palette_len <= 1 {
        return Some(vec![0; entries]);
    }
    let data = data?;
    let bits = bits_for(palette_len).max(min_bits) as usize;
    let per_long = 64 / bits;
    if data.len() != entries.div_ceil(per_long) {
        return None;
    }
    let mask = (1u64 << bits) - 1;
    (0..entries)
        .map(|i| {
            let slot = ((data[i / per_long] as u64) >> ((i % per_long) * bits)) & mask;
            (slot < palette_len as u64).then_some(slot as usize)
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChunkError {
    /// Saved by a newer Minecraft version than Levitas supports.
    TooNew(i64),
    Corrupt(&'static str),
}

/// Reads a chunk saved by vanilla, Leaf or Levitas. Returns `Ok(None)` for chunks that
/// were never fully generated. Unknown blocks become air and unknown biomes plains;
/// corrupt sections are emptied, like vanilla does.
pub fn from_nbt(nbt: &Compound) -> Result<Option<LoadedChunk>, ChunkError> {
    let version = nbt.int("DataVersion").unwrap_or(0);
    if version > i64::from(DATA_VERSION) {
        return Err(ChunkError::TooNew(version));
    }
    let status = nbt.str("Status").unwrap_or("");
    if status != "minecraft:full" && status != "full" {
        return Ok(None);
    }
    let biomes = biomes();
    let plains = biomes.id_of("minecraft:plains").expect("plains biome") as u16;
    let mut sections: Vec<Section> = (0..SECTIONS)
        .map(|_| Section::empty(plains, biome_kind()))
        .collect();
    // Chunks saved without light get full daylight until a light engine relights them.
    let lit = nbt.int("isLightOn") == Some(1);
    let mut light = ChunkLight::full_sky();

    for tag in nbt.list("sections").unwrap_or_default() {
        let Some(section) = tag.as_compound() else {
            return Err(ChunkError::Corrupt("section is not a compound"));
        };
        let y = section.int("Y").unwrap_or(i64::MIN);
        if let Some(i) = usize::try_from(y - i64::from(FIRST_LIGHT_Y))
            .ok()
            .filter(|&i| lit && i < LIGHT_SECTIONS)
        {
            if let Some(sky) = section.byte_array("SkyLight") {
                light.sky[i] = LightLayer::from_nibbles(&to_bytes(sky));
            }
            if let Some(block) = section.byte_array("BlockLight") {
                light.block[i] = LightLayer::from_nibbles(&to_bytes(block));
            }
        }
        let Some(index) = usize::try_from(y - i64::from(MIN_Y >> 4))
            .ok()
            .filter(|&i| i < SECTIONS)
        else {
            continue; // light-only sections above and below the world
        };

        let blocks = section.compound("block_states").and_then(|states| {
            let palette: Vec<u16> = states
                .list("palette")?
                .iter()
                .map(|entry| block_state(entry.as_compound()))
                .collect();
            let slots = unpack(states.long_array("data"), palette.len(), 4096, 4)?;
            let values: Vec<u16> = slots.iter().map(|&s| palette[s]).collect();
            Some(PalettedContainer::from_values(Kind::blocks(), &values))
        });
        let biome_container = section.compound("biomes").and_then(|b| {
            let palette: Vec<u16> = b
                .list("palette")?
                .iter()
                .map(|name| {
                    name.as_str()
                        .and_then(|n| biomes.id_of(n))
                        .map_or(plains, |id| id as u16)
                })
                .collect();
            let slots = unpack(b.long_array("data"), palette.len(), 64, 1)?;
            let values: Vec<u16> = slots.iter().map(|&s| palette[s]).collect();
            Some(PalettedContainer::from_values(biome_kind(), &values))
        });
        sections[index] = Section::from_parts(
            blocks.unwrap_or_else(|| PalettedContainer::single(Kind::blocks(), block::AIR)),
            biome_container.unwrap_or_else(|| PalettedContainer::single(biome_kind(), plains)),
        );
    }

    let block_entities = nbt
        .list("block_entities")
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t.as_compound().cloned())
        .collect();
    let mut chunk = Chunk::from_sections(sections);
    chunk.light = light;
    Ok(Some(LoadedChunk {
        chunk,
        block_entities,
        block_ticks: nbt.list("block_ticks").unwrap_or_default().to_vec(),
        fluid_ticks: nbt.list("fluid_ticks").unwrap_or_default().to_vec(),
    }))
}

fn to_bytes(array: &[i8]) -> Vec<u8> {
    array.iter().map(|&b| b as u8).collect()
}

/// A palette entry `{Name, Properties}` as a block state ID; unknown blocks are air.
fn block_state(entry: Option<&Compound>) -> u16 {
    let Some(info) = entry
        .and_then(|e| e.str("Name"))
        .and_then(BlockInfo::by_name)
    else {
        return block::AIR;
    };
    let props = entry.and_then(|e| e.compound("Properties"));
    info.state_with(
        props
            .into_iter()
            .flat_map(|p| p.iter())
            .filter_map(|(name, value)| Some((name, value.as_str()?))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_chunk() -> Chunk {
        let mut chunk = Chunk::empty(0, biome_kind());
        chunk.light.sky[1] = LightLayer::Uniform(0);
        chunk.light.sky[6].set(1, 2, 3, 7);
        chunk.light.block[6].set(4, 5, 6, 14);
        let stairs = BlockInfo::by_name("minecraft:oak_stairs").unwrap();
        let east = stairs.state_with([("facing", "east"), ("waterlogged", "true")]);
        chunk.set(0, MIN_Y, 0, block::BEDROCK);
        chunk.set(5, 70, 9, east);
        // A section with hundreds of different states needs direct storage.
        for i in 0..600u16 {
            chunk.set(
                usize::from(i % 16),
                100 + i32::from(i / 256),
                usize::from((i / 16) % 16),
                i + 1,
            );
        }
        chunk
    }

    #[test]
    fn round_trip_keeps_every_block() {
        let chunk = sample_chunk();
        let entity = Compound::new()
            .with("id", "minecraft:chest")
            .with("x", 1)
            .with("y", 2)
            .with("z", 3);
        let nbt = to_nbt(
            &chunk,
            4,
            -7,
            vec![entity.clone()],
            Vec::new(),
            Vec::new(),
            123,
            Span::OVERWORLD,
        );
        // Through the binary form too, as it would go to disk.
        let nbt = levitas_nbt::from_bytes(&levitas_nbt::to_bytes("", &nbt))
            .unwrap()
            .1;
        assert_eq!(nbt.int("xPos"), Some(4));
        let loaded = from_nbt(&nbt).unwrap().unwrap();
        for (a, b) in chunk.sections().iter().zip(loaded.chunk.sections()) {
            assert_eq!(a.blocks().values(), b.blocks().values());
            assert_eq!(a.biomes().values(), b.biomes().values());
        }
        assert_eq!(loaded.block_entities, vec![entity]);
        assert_eq!(loaded.chunk.light, chunk.light);
    }

    #[test]
    fn vanilla_style_palette_entries() {
        let nbt = to_nbt(
            &sample_chunk(),
            0,
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            0,
            Span::OVERWORLD,
        );
        let sections = nbt.list("sections").unwrap();
        // y = 70 is section 4 (y 64..80); the list starts with the light-only section -5.
        let section = sections[9].as_compound().unwrap();
        assert_eq!(section.int("Y"), Some(4));
        let palette = section
            .compound("block_states")
            .unwrap()
            .list("palette")
            .unwrap();
        let stairs = palette[1].as_compound().unwrap();
        assert_eq!(stairs.str("Name"), Some("minecraft:oak_stairs"));
        let props = stairs.compound("Properties").unwrap();
        assert_eq!(props.str("facing"), Some("east"));
        assert_eq!(props.str("waterlogged"), Some("true"));
        // A single-entry palette has no data.
        let top = sections[24]
            .as_compound()
            .unwrap()
            .compound("block_states")
            .unwrap();
        assert!(top.get("data").is_none());
    }

    #[test]
    fn unfinished_too_new_and_unknown() {
        let partial = Compound::new().with("Status", "minecraft:features");
        assert!(from_nbt(&partial).unwrap().is_none());
        let newer = Compound::new()
            .with("DataVersion", 99_999)
            .with("Status", "minecraft:full");
        assert_eq!(from_nbt(&newer).err(), Some(ChunkError::TooNew(99_999)));

        let palette = vec![Tag::Compound(
            Compound::new().with("Name", "minecraft:future_block"),
        )];
        let section = Compound::new()
            .with("Y", 0i8)
            .with("block_states", Compound::new().with("palette", palette));
        let nbt = Compound::new()
            .with("Status", "minecraft:full")
            .with("sections", vec![Tag::Compound(section)]);
        let loaded = from_nbt(&nbt).unwrap().unwrap();
        assert_eq!(loaded.chunk.get(0, 0, 0), block::AIR);
    }

    #[test]
    fn nether_chunks_save_only_their_sections() {
        let mut chunk = sample_chunk();
        chunk.set(0, 10, 0, block::STONE);
        let nbt = to_nbt(
            &chunk,
            0,
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            0,
            Span::NETHER,
        );
        assert_eq!(nbt.int("yPos"), Some(0));
        let ys: Vec<i64> = nbt
            .list("sections")
            .unwrap()
            .iter()
            .map(|s| s.as_compound().unwrap().int("Y").unwrap())
            .collect();
        assert_eq!(ys.first(), Some(&-1));
        assert_eq!(ys.last(), Some(&16));
        let loaded = from_nbt(&nbt).unwrap().unwrap();
        assert_eq!(loaded.chunk.get(0, 10, 0), block::STONE);
    }
}
