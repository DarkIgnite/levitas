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

pub struct BlockInfo {
    pub name: &'static str,
    pub first_state: u16,
    pub default_state: u16,
    /// Property names with their possible values, in state ID order.
    pub properties: &'static [(&'static str, &'static [&'static str])],
}

impl BlockInfo {
    /// The block a state belongs to.
    pub fn of(state: u16) -> &'static BlockInfo {
        &BLOCKS[usize::from(STATE_BLOCK[usize::from(state)])]
    }

    /// Value of property `name` in `state`, which must be a state of this block.
    pub fn property(&self, state: u16, name: &str) -> Option<&'static str> {
        // State IDs count through property values with the last property fastest.
        let mut index = usize::from(state - self.first_state);
        for (prop, values) in self.properties.iter().rev() {
            let value = values[index % values.len()];
            if *prop == name {
                return Some(value);
            }
            index /= values.len();
        }
        None
    }

    /// The block called `name` (e.g. `minecraft:oak_stairs`).
    pub fn by_name(name: &str) -> Option<&'static BlockInfo> {
        let i = BLOCKS_BY_NAME
            .binary_search_by_key(&name, |(n, _)| n)
            .ok()?;
        Some(BlockInfo::of(BLOCKS_BY_NAME[i].1))
    }

    /// The state of this block with the given property values. Properties not listed
    /// keep their default value; unknown properties or values are ignored.
    pub fn state_with<'a>(&self, properties: impl IntoIterator<Item = (&'a str, &'a str)>) -> u16 {
        let mut state = self.default_state;
        for (name, value) in properties {
            state = self.with(state, name, value).unwrap_or(state);
        }
        state
    }

    /// Property names and values of `state`, in property order.
    pub fn properties_of(&self, state: u16) -> Vec<(&'static str, &'static str)> {
        self.properties
            .iter()
            .filter_map(|(name, _)| Some((*name, self.property(state, name)?)))
            .collect()
    }

    /// `state` with property `name` set to `value`, or `None` if the block has no such
    /// property or value.
    pub fn with(&self, state: u16, name: &str, value: &str) -> Option<u16> {
        let mut index = usize::from(state - self.first_state);
        let mut stride = 1;
        for (prop, values) in self.properties.iter().rev() {
            let current = index / stride % values.len();
            if *prop == name {
                let new = values.iter().position(|v| *v == value)?;
                index = index + new * stride - current * stride;
                return Some(self.first_state + index as u16);
            }
            stride *= values.len();
        }
        None
    }
}

pub struct ItemInfo {
    pub name: &'static str,
    pub max_stack: u8,
    /// Default state of the block this item places, or air (0).
    pub block: u16,
    pub food: Option<Food>,
    pub tool_rules: &'static [ToolRule],
}

impl ItemInfo {
    pub fn get(id: i32) -> Option<&'static ItemInfo> {
        usize::try_from(id).ok().and_then(|i| ITEMS.get(i))
    }

    pub fn id_of(name: &str) -> Option<i32> {
        ITEMS.iter().position(|i| i.name == name).map(|i| i as i32)
    }

    /// Whether this item, used as a tool, makes `state` drop its loot. `None` if no rule
    /// of the tool mentions the block.
    pub fn correct_for_drops(&self, state: u16) -> Option<bool> {
        let block = i64::from(STATE_BLOCK[usize::from(state)]);
        self.tool_rules
            .iter()
            .find(|r| r.blocks.binary_search(&block).is_ok())
            .and_then(|r| r.correct_for_drops)
    }
}

pub struct Food {
    pub nutrition: i32,
    pub saturation: f32,
    pub can_always_eat: bool,
}

pub struct ToolRule {
    /// Block registry IDs, sorted.
    pub blocks: &'static [i64],
    pub correct_for_drops: Option<bool>,
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
    fn block_properties() {
        let info = BlockInfo::of(block::GRASS_BLOCK);
        assert_eq!(info.name, "minecraft:grass_block");
        assert_eq!(info.property(block::GRASS_BLOCK, "snowy"), Some("false"));
        let wheat = BlockInfo::of(block::WHEAT);
        assert_eq!(wheat.property(wheat.first_state + 7, "age"), Some("7"));

        let furnace = BlockInfo::of(block::FURNACE);
        let lit = furnace.with(block::FURNACE, "lit", "true").unwrap();
        assert_eq!(furnace.property(lit, "lit"), Some("true"));
        assert_eq!(
            furnace.property(lit, "facing"),
            furnace.property(block::FURNACE, "facing")
        );
        assert_eq!(furnace.with(lit, "lit", "false"), Some(block::FURNACE));
        assert_eq!(furnace.with(lit, "lit", "maybe"), None);
        assert!(RECIPES_JSON.contains("minecraft:crafting_table"));

        let stairs = BlockInfo::by_name("minecraft:oak_stairs").unwrap();
        let east = stairs.state_with([("facing", "east"), ("half", "top")]);
        assert_eq!(stairs.property(east, "facing"), Some("east"));
        assert_eq!(stairs.property(east, "half"), Some("top"));
        assert_eq!(stairs.properties_of(east).len(), stairs.properties.len());
        assert!(BlockInfo::by_name("minecraft:not_a_block").is_none());
        assert_eq!(DATA_VERSION, 4440);
        assert_ne!(menu::GENERIC_9X3, menu::CRAFTING);
    }

    #[test]
    fn items() {
        let pickaxe = ItemInfo::get(ItemInfo::id_of("minecraft:wooden_pickaxe").unwrap()).unwrap();
        assert_eq!(pickaxe.max_stack, 1);
        assert_eq!(pickaxe.correct_for_drops(block::STONE), Some(true));
        assert_eq!(pickaxe.correct_for_drops(block::IRON_ORE), Some(false));
        let bread = ItemInfo::get(ItemInfo::id_of("minecraft:bread").unwrap()).unwrap();
        assert_eq!(bread.food.as_ref().unwrap().nutrition, 5);
        assert!(BLOCK_LOOT_JSON.contains("minecraft:cobblestone"));
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
