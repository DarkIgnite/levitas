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

/// Face numbering used by the protocol and by [`SHAPE_STURDY`]: down, up, north, south,
/// west, east.
pub const FACE_DOWN: usize = 0;
pub const FACE_UP: usize = 1;

/// Collision boxes of a block state, as `[min_x, min_y, min_z, max_x, max_y, max_z]` in
/// block units.
pub fn collision_boxes(state: u16) -> &'static [[f32; 6]] {
    SHAPES[usize::from(STATE_SHAPE[usize::from(state)])]
}

/// Whether a block state's face (protocol numbering) is fully solid, so blocks such as
/// torches, ladders and buttons can be attached to it.
pub fn face_sturdy(state: u16, face: usize) -> bool {
    SHAPE_STURDY[usize::from(STATE_SHAPE[usize::from(state)])] & (1 << face) != 0
}

/// Whether a block state is a full, solid cube.
pub fn is_full_cube(state: u16) -> bool {
    SHAPE_STURDY[usize::from(STATE_SHAPE[usize::from(state)])] == 0b11_1111
}

/// The ID of a sound event such as `minecraft:block.wooden_door.open`.
pub fn sound_id(name: &str) -> Option<i32> {
    let i = SOUND_EVENTS.binary_search_by_key(&name, |(n, _)| n).ok()?;
    Some(SOUND_EVENTS[i].1)
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
    /// Mining hardness; -1 for unbreakable blocks such as bedrock.
    pub hardness: f32,
    /// Explosion resistance.
    pub resistance: f32,
    /// Light level the block gives off (0-15).
    pub light_emission: u8,
    /// How much light the block absorbs (0-15).
    pub light_filter: u8,
    /// Whether the block drops nothing unless mined with a correct tool (stone without a
    /// pickaxe, for example).
    pub requires_tool: bool,
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
    /// How the item is eaten or drunk, for items that can be.
    pub consumable: Option<Consumable>,
    /// How well the item takes enchantments at an enchanting table; 0 if it cannot.
    pub enchantability: u8,
    /// What repairs the item in an anvil: an item name, or `#tag` of items.
    pub repair_with: Option<&'static str>,
    pub tool: Option<Tool>,
    /// Durability; 0 for items that do not wear out.
    pub max_damage: i32,
    /// The equipment slot the item is worn in (`head`, `chest`, `legs`, `feet`, ...).
    pub equip_slot: Option<&'static str>,
    /// Attack damage of a player holding this item (1 for a fist).
    pub attack_damage: f32,
    /// Attacks per second at full strength when holding this item (4 for a fist).
    pub attack_speed: f32,
    /// Armor points and armor toughness given while worn.
    pub armor: f32,
    pub toughness: f32,
    /// The armor trim material this item gives at a smithing table.
    pub trim_material: Option<&'static str>,
}

impl ItemInfo {
    pub fn get(id: i32) -> Option<&'static ItemInfo> {
        usize::try_from(id).ok().and_then(|i| ITEMS.get(i))
    }

    pub fn id_of(name: &str) -> Option<i32> {
        ITEMS.iter().position(|i| i.name == name).map(|i| i as i32)
    }

    /// Whether this item, used as a tool, makes `state` drop its loot. `None` if no rule
    /// of the tool decides it.
    pub fn correct_for_drops(&self, state: u16) -> Option<bool> {
        let block = i64::from(STATE_BLOCK[usize::from(state)]);
        self.tool?
            .rules
            .iter()
            .filter(|r| r.correct_for_drops.is_some())
            .find(|r| r.blocks.binary_search(&block).is_ok())
            .and_then(|r| r.correct_for_drops)
    }

    /// How fast this item mines `state` (1 for a bare hand or a non-tool).
    pub fn mining_speed(&self, state: u16) -> f32 {
        let Some(tool) = self.tool else {
            return 1.0;
        };
        let block = i64::from(STATE_BLOCK[usize::from(state)]);
        tool.rules
            .iter()
            .filter_map(|r| Some((r, r.speed?)))
            .find(|(r, _)| r.blocks.binary_search(&block).is_ok())
            .map_or(tool.default_speed, |(_, speed)| speed)
    }
}

/// Eating or drinking an item: how long it takes, what it does, and what is left.
pub struct Consumable {
    pub ticks: u32,
    pub effects: &'static [OnConsume],
    /// The item left in hand afterwards (a bowl, a bucket, a bottle).
    pub remainder: Option<&'static str>,
}

/// Something that happens on eating or drinking.
pub enum OnConsume {
    /// Gives status effects, all at once with this probability.
    Apply {
        probability: f32,
        effects: &'static [Effect],
    },
    ClearAll,
    /// Removes these status effects.
    Remove(&'static [i32]),
    TeleportRandomly,
}

/// A status effect to give: its ID, ticks, and level less one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    pub id: i32,
    pub duration: i32,
    pub amplifier: u8,
}

pub struct Food {
    pub nutrition: i32,
    pub saturation: f32,
    pub can_always_eat: bool,
}

#[derive(Clone, Copy)]
pub struct Tool {
    pub rules: &'static [ToolRule],
    /// Speed on blocks no rule mentions.
    pub default_speed: f32,
    /// Durability used up per block mined.
    pub damage_per_block: i32,
}

pub struct ToolRule {
    /// Block registry IDs, sorted.
    pub blocks: &'static [i64],
    pub correct_for_drops: Option<bool>,
    pub speed: Option<f32>,
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
    fn registries_26_3_clients_need() {
        // A vanilla 26.3 server sends these too; without them the client cannot build
        // item components (shovels reference minecraft:block_transformer/shovel).
        let transformers = SyncedRegistry::get("minecraft:block_transformer").unwrap();
        assert!(transformers.id_of("minecraft:shovel").is_some());
        assert_eq!(
            SyncedRegistry::get("minecraft:decorated_pot_pattern")
                .unwrap()
                .entries
                .len(),
            23
        );
        assert_eq!(
            SyncedRegistry::get("minecraft:worldgen/block_state_provider")
                .unwrap()
                .entries
                .len(),
            8
        );
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
        assert_eq!(DATA_VERSION, 5023, "26.3");
        // Blocks 26.3 added take hardness and shapes from their older relatives.
        let poplar = BlockInfo::by_name("minecraft:poplar_planks").unwrap();
        assert_eq!(poplar.hardness, BlockInfo::of(block::OAK_PLANKS).hardness);
        let wool_stairs = BlockInfo::by_name("minecraft:white_wool_stairs").unwrap();
        assert_eq!(
            wool_stairs.hardness,
            BlockInfo::of(block::WHITE_WOOL).hardness
        );
        assert_eq!(
            collision_boxes(wool_stairs.default_state),
            collision_boxes(block::OAK_STAIRS)
        );

        assert_eq!(BlockInfo::of(block::STONE).hardness, 1.5);
        assert_eq!(BlockInfo::of(block::BEDROCK).hardness, -1.0);
        assert_eq!(BlockInfo::of(block::GLOWSTONE).light_emission, 15);
        assert!(is_full_cube(block::STONE));
        assert!(collision_boxes(block::AIR).is_empty());
        // A bottom slab is solid underneath only; stairs facing north have a solid back.
        let slab = block::OAK_SLAB;
        assert!(face_sturdy(slab, FACE_DOWN) && !face_sturdy(slab, FACE_UP));
        let stairs = BlockInfo::by_name("minecraft:oak_stairs").unwrap();
        let north = stairs.state_with([
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
        ]);
        assert!(
            face_sturdy(north, 2) && !face_sturdy(north, 3),
            "north face solid, south not"
        );
        assert!(sound_id("minecraft:block.wooden_door.open").is_some());
        assert_ne!(menu::GENERIC_9X3, menu::CRAFTING);
    }

    #[test]
    fn items() {
        let pickaxe = ItemInfo::get(ItemInfo::id_of("minecraft:wooden_pickaxe").unwrap()).unwrap();
        assert_eq!(pickaxe.max_stack, 1);
        assert_eq!(pickaxe.correct_for_drops(block::STONE), Some(true));
        assert_eq!(pickaxe.correct_for_drops(block::IRON_ORE), Some(false));
        assert_eq!(pickaxe.mining_speed(block::STONE), 2.0);
        assert_eq!(pickaxe.mining_speed(block::DIRT), 1.0);
        assert_eq!(pickaxe.max_damage, 59);
        let sword = ItemInfo::get(ItemInfo::id_of("minecraft:diamond_sword").unwrap()).unwrap();
        assert_eq!(sword.tool.unwrap().damage_per_block, 2);
        assert_eq!(sword.correct_for_drops(block::COBWEB), Some(true));
        let helmet = ItemInfo::get(ItemInfo::id_of("minecraft:iron_helmet").unwrap()).unwrap();
        assert_eq!(helmet.equip_slot, Some("head"));
        assert!(BlockInfo::of(block::STONE).requires_tool);
        assert!(!BlockInfo::of(block::DIRT).requires_tool);
        assert_eq!(component::DAMAGE, 3);
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
