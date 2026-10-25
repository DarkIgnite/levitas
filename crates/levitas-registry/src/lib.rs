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
    pub tags: &'static [(&'static str, &'static [i64])],
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
