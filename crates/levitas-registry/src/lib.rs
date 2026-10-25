//! Game registries (packets, blocks, data-driven registries, tags) generated at build time
//! from the official data generator's output. See `tools/datagen.py`.

pub struct SyncedRegistry {
    pub name: &'static str,
    pub entries: &'static [&'static str],
}

impl SyncedRegistry {
    pub fn get(name: &str) -> Option<&'static SyncedRegistry> {
        SYNCED_REGISTRIES.iter().find(|r| r.name == name)
    }

    /// Network ID of `entry`, which is its position in the list sent to the client.
    pub fn id_of(&self, entry: &str) -> Option<i32> {
        self.entries
            .iter()
            .position(|e| *e == entry)
            .map(|i| i as i32)
    }
}

pub struct TagRegistry {
    pub registry: &'static str,
    /// Tag names with their member IDs, sorted ascending.
    pub tags: &'static [(&'static str, &'static [i64])],
}

impl TagRegistry {
    pub fn get(registry: &str) -> Option<&'static TagRegistry> {
        TAGS.iter().find(|t| t.registry == registry)
    }

    /// Members of `tag`, or an empty slice if there is no such tag.
    pub fn members(&self, tag: &str) -> &'static [i64] {
        self.tags
            .iter()
            .find(|(name, _)| *name == tag)
            .map_or(&[], |(_, ids)| ids)
    }
}

/// Whether the block of `state` is in the block tag `tag`.
pub fn block_has_tag(state: u16, tag: &str) -> bool {
    let Some(&block) = STATE_BLOCK.get(usize::from(state)) else {
        return false;
    };
    TagRegistry::get("minecraft:block")
        .is_some_and(|blocks| blocks.members(tag).binary_search(&i64::from(block)).is_ok())
}

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ids() {
        assert_eq!(block::AIR, 0);
        assert_eq!(packet::status::clientbound::STATUS_RESPONSE, 0);
        let biomes = SyncedRegistry::get("minecraft:worldgen/biome").unwrap();
        assert!(biomes.id_of("minecraft:plains").is_some());
        let dims = SyncedRegistry::get("minecraft:dimension_type").unwrap();
        assert!(dims.id_of("minecraft:overworld").is_some());
    }

    #[test]
    fn replaceable_tag() {
        assert!(block_has_tag(block::AIR, "minecraft:replaceable"));
        assert!(block_has_tag(block::SHORT_GRASS, "minecraft:replaceable"));
        assert!(!block_has_tag(block::STONE, "minecraft:replaceable"));
    }

    #[test]
    fn tags_are_resolved() {
        let blocks = TAGS
            .iter()
            .find(|t| t.registry == "minecraft:block")
            .unwrap();
        let (_, logs) = blocks
            .tags
            .iter()
            .find(|(n, _)| *n == "minecraft:logs")
            .unwrap();
        assert!(!logs.is_empty());
    }
}
