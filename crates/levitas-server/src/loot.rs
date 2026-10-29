//! Loot tables: what a broken block or a killed mob drops. Tables are the vanilla JSON
//! from the data generator, evaluated with the context Levitas tracks so far: the block
//! state, the tool item and its enchantments (silk touch, fortune, looting) and a few
//! mob properties. Chest tables fill the containers structures generate with.

use std::collections::HashMap;

use levitas_registry::{
    BlockInfo, ItemInfo, TagRegistry, BLOCK_LOOT_JSON, CHEST_LOOT_JSON, ENTITY_LOOT_JSON,
};
use serde_json::Value;

use crate::enchanting::{self, Enchantments};
use crate::inventory::Stack;
use crate::rng::Rng;

pub struct LootTables {
    blocks: HashMap<String, Value>,
    /// Entity tables, keyed `minecraft:<entity>` (and `minecraft:sheep/<color>`).
    entities: HashMap<String, Value>,
    /// Chest tables, keyed `minecraft:<path under chests/>`.
    chests: HashMap<String, Value>,
    item_ids: HashMap<&'static str, i32>,
}

/// What a loot table is rolled with.
pub struct LootContext<'a> {
    pub state: u16,
    /// Item ID of the tool used, 0 for an empty hand.
    pub tool: i32,
    pub rng: &'a mut Rng,
    /// Enchantments of the tool, or of the weapon that killed the mob.
    pub enchants: Enchantments,
    /// The mob whose loot is rolled, for `entity_properties` conditions.
    pub entity: EntityProps,
}

/// What loot conditions may ask about the mob that died.
#[derive(Debug, Clone, Default)]
pub struct EntityProps {
    pub on_fire: bool,
    /// A sheep's wool colour (`white`, `black`, ...) and whether it was sheared.
    pub sheep_color: Option<&'static str>,
    pub sheared: bool,
    pub killed_by_player: bool,
}

impl LootTables {
    pub fn load() -> Self {
        let blocks: serde_json::Map<String, Value> =
            serde_json::from_str(BLOCK_LOOT_JSON).expect("block loot tables are valid JSON");
        let entities: serde_json::Map<String, Value> =
            serde_json::from_str(ENTITY_LOOT_JSON).expect("entity loot tables are valid JSON");
        let chests: serde_json::Map<String, Value> =
            serde_json::from_str(CHEST_LOOT_JSON).expect("chest loot tables are valid JSON");
        let item_ids = levitas_registry::ITEMS
            .iter()
            .enumerate()
            .map(|(id, item)| (item.name, id as i32))
            .collect();
        Self {
            blocks: blocks.into_iter().collect(),
            entities: entities.into_iter().collect(),
            chests: chests.into_iter().collect(),
            item_ids,
        }
    }

    /// Items dropped by breaking a block, as `(item ID, count)` pairs.
    pub fn block_drops(&self, ctx: &mut LootContext) -> Vec<(i32, i32)> {
        if !crate::rules::bool("doTileDrops") {
            return Vec::new();
        }
        let name = BlockInfo::of(ctx.state).name;
        // Wall-mounted variants (wall torches, wall signs, ...) have no table of their own
        // and drop like the standing block.
        let standing = name
            .replacen("_wall_", "_", 1)
            .replacen("minecraft:wall_", "minecraft:", 1);
        let Some(table) = self.blocks.get(name).or_else(|| self.blocks.get(&standing)) else {
            return Vec::new();
        };
        let mut drops = Vec::new();
        self.roll(table, ctx, &mut drops);
        pairs(drops)
    }

    /// Items dropped by a mob of `entity` type (e.g. `minecraft:cow`) when it dies.
    pub fn entity_drops(&self, entity: &str, ctx: &mut LootContext) -> Vec<(i32, i32)> {
        if !crate::rules::bool("doMobLoot") {
            return Vec::new();
        }
        let mut drops = Vec::new();
        if let Some(table) = self.entities.get(entity) {
            self.roll(table, ctx, &mut drops);
        }
        pairs(drops)
    }

    /// Fills a container from chest table `name` (`minecraft:chests/...`), rolled with
    /// `seed` (0 for any). As in vanilla, the items land in random empty slots, with
    /// some stacks split up to fill more of them. Returns whether the table exists.
    pub fn fill(&self, name: &str, seed: i64, slots: &mut [Stack]) -> bool {
        let Some(table) = name
            .strip_prefix("minecraft:chests/")
            .and_then(|path| self.chests.get(&format!("minecraft:{path}")))
        else {
            return false;
        };
        let mut rng = if seed == 0 {
            Rng::from_time()
        } else {
            Rng::new(seed as u64)
        };
        let mut stacks = Vec::new();
        self.roll(
            table,
            &mut LootContext {
                state: 0,
                tool: 0,
                rng: &mut rng,
                enchants: Enchantments::NONE,
                entity: EntityProps::default(),
            },
            &mut stacks,
        );
        spread(stacks, slots, &mut rng);
        true
    }

    /// The table a `minecraft:loot_table` entry refers to.
    fn table(&self, name: &str) -> Option<&Value> {
        let path = name.strip_prefix("minecraft:").unwrap_or(name);
        if let Some(entity) = path.strip_prefix("entities/") {
            return self.entities.get(&format!("minecraft:{entity}"));
        }
        if let Some(chest) = path.strip_prefix("chests/") {
            return self.chests.get(&format!("minecraft:{chest}"));
        }
        let block = path.strip_prefix("blocks/")?;
        self.blocks.get(&format!("minecraft:{block}"))
    }

    fn roll(&self, table: &Value, ctx: &mut LootContext, drops: &mut Vec<Stack>) {
        for pool in array(&table["pools"]) {
            if !conditions(pool, ctx) {
                continue;
            }
            let rolls = number(&pool["rolls"], ctx.rng).floor() as i32;
            for _ in 0..rolls {
                let mut candidates = Vec::new();
                for entry in array(&pool["entries"]) {
                    expand(entry, ctx, &mut candidates);
                }
                let total: u32 = candidates.iter().map(|(_, w)| w).sum();
                if total == 0 {
                    continue;
                }
                let mut pick = ctx.rng.below(total);
                let chosen = candidates.iter().find(|(_, w)| {
                    if pick < *w {
                        true
                    } else {
                        pick -= w;
                        false
                    }
                });
                let Some((entry, _)) = chosen else { continue };
                if entry["type"] == "minecraft:loot_table" {
                    if let Some(nested) = entry["value"].as_str().and_then(|n| self.table(n)) {
                        self.roll(nested, ctx, drops);
                    }
                    continue;
                }
                if entry["type"] != "minecraft:item" {
                    continue; // empty, or a kind of entry not supported yet
                }
                let Some(&item) = entry["name"].as_str().and_then(|n| self.item_ids.get(n)) else {
                    continue;
                };
                let mut stack = Stack {
                    item,
                    count: 1,
                    ..Stack::EMPTY
                };
                for function in array(&entry["functions"])
                    .iter()
                    .chain(array(&pool["functions"]))
                    .chain(array(&table["functions"]))
                {
                    if conditions(function, ctx) {
                        self.apply_to(function, &mut stack, ctx);
                    }
                }
                if stack.count > 0 {
                    drops.push(stack);
                }
            }
        }
    }
}

impl LootTables {
    /// Runs a loot function on a dropped stack.
    fn apply_to(&self, function: &Value, stack: &mut Stack, ctx: &mut LootContext) {
        match function["function"].as_str().unwrap_or_default() {
            "minecraft:set_damage" => {
                if let Some(info) = ItemInfo::get(stack.item).filter(|i| i.max_damage > 0) {
                    // The share of durability left.
                    let left = number(&function["damage"], ctx.rng).clamp(0.0, 1.0);
                    stack.damage = ((1.0 - left) * f64::from(info.max_damage)).round() as i32;
                }
            }
            "minecraft:enchant_randomly" => {
                let picked = enchanting::random_for_loot(ctx.rng, stack.item, &function["options"]);
                if let Some((id, level)) = picked {
                    self.enchant(stack, id, level);
                }
            }
            "minecraft:enchant_with_levels" => {
                let level = number(&function["levels"], ctx.rng).max(0.0) as u32;
                let options = &function["options"];
                for (id, l) in enchanting::select_for_loot(ctx.rng, stack.item, level, options) {
                    self.enchant(stack, id, l);
                }
            }
            _ => stack.count = apply(function, stack.count, ctx),
        }
    }

    /// Enchants a stack; a book becomes an enchanted book holding the enchantment.
    fn enchant(&self, stack: &mut Stack, id: i32, level: u32) {
        if ItemInfo::get(stack.item).is_some_and(|i| i.name == "minecraft:book") {
            if let Some(&book) = self.item_ids.get("minecraft:enchanted_book") {
                stack.item = book;
            }
        }
        stack.enchants.set(id, level);
    }
}

/// Dropped stacks as `(item ID, count)` pairs.
fn pairs(stacks: Vec<Stack>) -> Vec<(i32, i32)> {
    stacks.into_iter().map(|s| (s.item, s.count)).collect()
}

/// Puts rolled stacks into random empty slots, splitting oversized stacks, and then
/// splitting others while there are slots to spare.
fn spread(stacks: Vec<Stack>, slots: &mut [Stack], rng: &mut Rng) {
    let mut free: Vec<usize> = (0..slots.len()).filter(|&i| slots[i].count == 0).collect();
    shuffle(&mut free, rng);
    let mut piles = Vec::new();
    for mut stack in stacks {
        let max = ItemInfo::get(stack.item).map_or(64, |i| i32::from(i.max_stack.max(1)));
        while stack.count > max {
            piles.push(Stack {
                count: max,
                ..stack
            });
            stack.count -= max;
        }
        piles.push(stack);
    }
    while piles.len() < free.len() {
        let splittable: Vec<usize> = (0..piles.len()).filter(|&i| piles[i].count > 1).collect();
        if splittable.is_empty() {
            break;
        }
        let i = splittable[rng.below(splittable.len() as u32) as usize];
        let part = 1 + rng.below((piles[i].count / 2) as u32) as i32;
        piles[i].count -= part;
        piles.push(Stack {
            count: part,
            ..piles[i]
        });
    }
    shuffle(&mut piles, rng);
    for (pile, slot) in piles.into_iter().zip(free) {
        slots[slot] = pile;
    }
}

fn shuffle<T>(list: &mut [T], rng: &mut Rng) {
    for i in (1..list.len()).rev() {
        list.swap(i, rng.below(i as u32 + 1) as usize);
    }
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// Adds the entries `entry` expands to, with their weights. Returns whether its
/// conditions passed, which decides how `alternatives` and `sequence` continue.
fn expand<'a>(entry: &'a Value, ctx: &mut LootContext, out: &mut Vec<(&'a Value, u32)>) -> bool {
    if !conditions(entry, ctx) {
        return false;
    }
    match entry["type"].as_str().unwrap_or_default() {
        "minecraft:alternatives" => array(&entry["children"])
            .iter()
            .any(|child| expand(child, ctx, out)),
        "minecraft:group" => {
            for child in array(&entry["children"]) {
                expand(child, ctx, out);
            }
            true
        }
        "minecraft:sequence" => {
            for child in array(&entry["children"]) {
                if !expand(child, ctx, out) {
                    break;
                }
            }
            true
        }
        _ => {
            let weight = entry["weight"].as_u64().unwrap_or(1) as u32;
            out.push((entry, weight));
            true
        }
    }
}

fn conditions(holder: &Value, ctx: &mut LootContext) -> bool {
    array(&holder["conditions"])
        .iter()
        .all(|c| condition(c, ctx))
}

fn condition(c: &Value, ctx: &mut LootContext) -> bool {
    match c["condition"].as_str().unwrap_or_default() {
        "minecraft:survives_explosion" => true,
        "minecraft:killed_by_player" => ctx.entity.killed_by_player,
        "minecraft:match_tool" => match_tool(&c["predicate"], ctx.tool, &ctx.enchants),
        "minecraft:random_chance" => ctx.rng.f64() < number(&c["chance"], ctx.rng),
        "minecraft:random_chance_with_enchanted_bonus" => {
            let level = enchant_level(c, ctx);
            let chance = if level > 0 {
                leveled(&c["enchanted_chance"], level, ctx.rng)
            } else {
                number(&c["unenchanted_chance"], ctx.rng)
            };
            ctx.rng.f64() < chance
        }
        "minecraft:table_bonus" => {
            let chances = array(&c["chances"]);
            let level = enchant_level(c, ctx) as usize;
            let chance = chances
                .get(level.min(chances.len().saturating_sub(1)))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            ctx.rng.f64() < chance
        }
        "minecraft:block_state_property" => block_state_matches(&c["properties"], ctx.state),
        "minecraft:entity_properties" if c["entity"] == "this" => {
            entity_matches(&c["predicate"], &ctx.entity)
        }
        "minecraft:inverted" => !condition(&c["term"], ctx),
        "minecraft:any_of" => array(&c["terms"]).iter().any(|t| condition(t, ctx)),
        "minecraft:all_of" => array(&c["terms"]).iter().all(|t| condition(t, ctx)),
        _ => false,
    }
}

/// The level of the enchantment a condition or function names, on the tool or weapon.
fn enchant_level(holder: &Value, ctx: &LootContext) -> u32 {
    holder["enchantment"]
        .as_str()
        .map_or(0, |e| ctx.enchants.level_of(e))
}

/// A number provider that may depend on an enchantment level (`minecraft:linear`).
fn leveled(value: &Value, level: u32, rng: &mut Rng) -> f64 {
    if value["type"] == "minecraft:linear" {
        let base = value["base"].as_f64().unwrap_or(0.0);
        let per = value["per_level_above_first"].as_f64().unwrap_or(0.0);
        return base + per * f64::from(level.saturating_sub(1));
    }
    number(value, rng)
}

/// Matches an item predicate: the items, and enchantment requirements on them. Other
/// component predicates never match.
fn match_tool(predicate: &Value, tool: i32, enchants: &Enchantments) -> bool {
    if let Some(predicates) = predicate.get("predicates").and_then(Value::as_object) {
        for (key, wanted) in predicates {
            if key != "minecraft:enchantments" {
                return false;
            }
            let all = array(wanted).iter().all(|w| {
                let names: Vec<&str> = match &w["enchantments"] {
                    Value::String(s) => vec![s.as_str()],
                    Value::Array(l) => l.iter().filter_map(Value::as_str).collect(),
                    _ => Vec::new(),
                };
                let min = w["levels"]["min"].as_u64().unwrap_or(1) as u32;
                let max = w["levels"]["max"].as_u64().map_or(u32::MAX, |m| m as u32);
                names.iter().any(|n| {
                    let level = enchants.level_of(n);
                    level > 0 && (min..=max).contains(&level)
                })
            });
            if !all {
                return false;
            }
        }
    }
    let Some(items) = predicate.get("items") else {
        return true;
    };
    let name = ItemInfo::get(tool).map_or("minecraft:air", |i| i.name);
    let matches = |spec: &str| match spec.strip_prefix('#') {
        Some(tag) => TagRegistry::get("minecraft:item")
            .is_some_and(|t| t.members(tag).binary_search(&i64::from(tool)).is_ok()),
        None => spec == name,
    };
    match items {
        Value::String(s) => matches(s),
        Value::Array(list) => list.iter().filter_map(Value::as_str).any(matches),
        _ => false,
    }
}

/// Matches the parts of an entity predicate Levitas knows; anything else fails.
fn entity_matches(predicate: &Value, entity: &EntityProps) -> bool {
    let Some(fields) = predicate.as_object() else {
        return false;
    };
    fields.iter().all(|(key, wanted)| match key.as_str() {
        "flags" => wanted.as_object().is_some_and(|flags| {
            flags.iter().all(|(flag, value)| match flag.as_str() {
                "is_on_fire" => value.as_bool() == Some(entity.on_fire),
                _ => false,
            })
        }),
        "components" => wanted.as_object().is_some_and(|components| {
            components.iter().all(|(name, value)| {
                name == "minecraft:sheep/color" && value.as_str() == entity.sheep_color
            })
        }),
        "type_specific" => {
            wanted["type"] == "minecraft:sheep"
                && wanted
                    .get("sheared")
                    .is_none_or(|s| s.as_bool() == Some(entity.sheared))
        }
        _ => false,
    })
}

fn block_state_matches(properties: &Value, state: u16) -> bool {
    let block = BlockInfo::of(state);
    let Some(properties) = properties.as_object() else {
        return true;
    };
    properties.iter().all(|(name, wanted)| {
        let Some(actual) = block.property(state, name) else {
            return false;
        };
        match wanted {
            Value::String(s) => actual == s,
            Value::Object(range) => {
                let Ok(actual) = actual.parse::<i64>() else {
                    return false;
                };
                let min = range.get("min").and_then(Value::as_i64).unwrap_or(i64::MIN);
                let max = range.get("max").and_then(Value::as_i64).unwrap_or(i64::MAX);
                (min..=max).contains(&actual)
            }
            _ => false,
        }
    })
}

fn apply(function: &Value, count: i32, ctx: &mut LootContext) -> i32 {
    let level = enchant_level(function, ctx);
    let rng = &mut *ctx.rng;
    match function["function"].as_str().unwrap_or_default() {
        "minecraft:set_count" => {
            let n = number(&function["count"], rng).floor() as i32;
            if function["add"].as_bool() == Some(true) {
                count + n
            } else {
                n
            }
        }
        "minecraft:apply_bonus" => match function["formula"].as_str().unwrap_or_default() {
            "minecraft:binomial_with_bonus_count" => {
                let extra =
                    function["parameters"]["extra"].as_i64().unwrap_or(0) + i64::from(level);
                let p = function["parameters"]["probability"]
                    .as_f64()
                    .unwrap_or(0.0);
                count + (0..extra).filter(|_| rng.f64() < p).count() as i32
            }
            // Fortune multiplies ore drops by 1 to level + 1, favouring 1.
            "minecraft:ore_drops" if level > 0 => {
                let bonus = rng.below(level + 2) as i32 - 1;
                count * (bonus.max(0) + 1)
            }
            "minecraft:uniform_bonus_count" => {
                let multiplier = function["parameters"]["bonusMultiplier"]
                    .as_u64()
                    .unwrap_or(1) as u32;
                count + rng.below(multiplier * level + 1) as i32
            }
            _ => count,
        },
        // Looting: each level adds up to the given count.
        "minecraft:enchanted_count_increase" if level > 0 => {
            let per = number(&function["count"], rng);
            count + (per * f64::from(level)).round() as i32
        }
        "minecraft:limit_count" => {
            let limit = &function["limit"];
            let min = limit.get("min").map_or(i32::MIN, |v| number(v, rng) as i32);
            let max = limit.get("max").map_or(i32::MAX, |v| number(v, rng) as i32);
            count.clamp(min, max)
        }
        _ => count,
    }
}

/// Evaluates a number provider: a plain number, or a constant/uniform/binomial object.
fn number(value: &Value, rng: &mut Rng) -> f64 {
    if let Some(n) = value.as_f64() {
        return n;
    }
    match value["type"].as_str().unwrap_or("minecraft:constant") {
        "minecraft:uniform" => {
            let min = number(&value["min"], rng);
            let max = number(&value["max"], rng);
            min + rng.f64() * (max - min)
        }
        "minecraft:binomial" => {
            let n = number(&value["n"], rng) as i64;
            let p = number(&value["p"], rng);
            (0..n).filter(|_| rng.f64() < p).count() as f64
        }
        _ => value["value"].as_f64().unwrap_or(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use levitas_registry::block;

    fn drops(state: u16, tool: &str) -> Vec<(i32, i32)> {
        enchanted_drops(state, tool, Enchantments::NONE)
    }

    fn enchanted_drops(state: u16, tool: &str, enchants: Enchantments) -> Vec<(i32, i32)> {
        let tables = LootTables::load();
        let tool = ItemInfo::id_of(tool).unwrap_or(0);
        let mut rng = Rng::new(1);
        tables.block_drops(&mut LootContext {
            state,
            tool,
            rng: &mut rng,
            enchants,
            entity: Default::default(),
        })
    }

    fn item(name: &str) -> i32 {
        ItemInfo::id_of(name).unwrap()
    }

    #[test]
    fn stone_drops_cobblestone_without_silk_touch() {
        assert_eq!(
            drops(block::STONE, "minecraft:wooden_pickaxe"),
            [(item("minecraft:cobblestone"), 1)]
        );
    }

    fn with(name: &str, level: u32) -> Enchantments {
        let mut e = Enchantments::NONE;
        e.set(crate::enchanting::enchantment_id(name).unwrap(), level);
        e
    }

    #[test]
    fn silk_touch_keeps_blocks_whole() {
        let silk = with("minecraft:silk_touch", 1);
        assert_eq!(
            enchanted_drops(block::STONE, "minecraft:wooden_pickaxe", silk),
            [(item("minecraft:stone"), 1)]
        );
        let ore = levitas_registry::BlockInfo::by_name("minecraft:diamond_ore")
            .unwrap()
            .default_state;
        assert_eq!(
            enchanted_drops(ore, "minecraft:iron_pickaxe", silk),
            [(item("minecraft:diamond_ore"), 1)]
        );
    }

    #[test]
    fn fortune_multiplies_ore_drops() {
        let ore = levitas_registry::BlockInfo::by_name("minecraft:diamond_ore")
            .unwrap()
            .default_state;
        let fortune = with("minecraft:fortune", 3);
        let tables = LootTables::load();
        let mut rng = Rng::new(3);
        let total: i32 = (0..200)
            .flat_map(|_| {
                tables.block_drops(&mut LootContext {
                    state: ore,
                    tool: item("minecraft:iron_pickaxe"),
                    rng: &mut rng,
                    enchants: fortune,
                    entity: Default::default(),
                })
            })
            .map(|(_, n)| n)
            .sum();
        // Fortune III averages 2.2 diamonds an ore.
        assert!((350..550).contains(&total), "{total}");
    }

    #[test]
    fn grass_block_drops_dirt() {
        assert_eq!(
            drops(block::GRASS_BLOCK, "minecraft:air"),
            [(item("minecraft:dirt"), 1)]
        );
    }

    #[test]
    fn wall_variants_drop_like_standing_blocks() {
        let wall = |name: &str| BlockInfo::by_name(name).unwrap().default_state;
        assert_eq!(
            drops(wall("minecraft:wall_torch"), "minecraft:air"),
            [(item("minecraft:torch"), 1)]
        );
        assert_eq!(
            drops(wall("minecraft:oak_wall_sign"), "minecraft:air"),
            [(item("minecraft:oak_sign"), 1)]
        );
        assert_eq!(
            drops(wall("minecraft:spruce_wall_hanging_sign"), "minecraft:air"),
            [(item("minecraft:spruce_hanging_sign"), 1)]
        );
        assert!(drops(wall("minecraft:piston_head"), "minecraft:air").is_empty());
    }

    #[test]
    fn glass_needs_silk_touch() {
        assert!(drops(block::GLASS, "minecraft:air").is_empty());
    }

    #[test]
    fn mature_wheat_drops_wheat_and_seeds() {
        let wheat = BlockInfo::of(block::WHEAT);
        let ripe = drops(wheat.first_state + 7, "minecraft:air");
        assert!(ripe.contains(&(item("minecraft:wheat"), 1)));
        assert!(ripe
            .iter()
            .any(|&(i, _)| i == item("minecraft:wheat_seeds")));
        let young = drops(wheat.first_state, "minecraft:air");
        assert!(!young.iter().any(|&(i, _)| i == item("minecraft:wheat")));
    }

    fn mob_drops(entity: &str, props: EntityProps, seed: u64) -> Vec<(i32, i32)> {
        let tables = LootTables::load();
        let mut rng = Rng::new(seed);
        tables.entity_drops(
            entity,
            &mut LootContext {
                state: block::AIR,
                tool: 0,
                rng: &mut rng,
                enchants: Default::default(),
                entity: props,
            },
        )
    }

    #[test]
    fn cows_drop_raw_beef_and_sometimes_leather() {
        for seed in 0..20 {
            let drops = mob_drops("minecraft:cow", EntityProps::default(), seed);
            let beef: i32 = drops
                .iter()
                .filter(|(i, _)| *i == item("minecraft:beef"))
                .map(|(_, n)| n)
                .sum();
            // Not cooked: the cow was not on fire.
            assert!((1..=3).contains(&beef), "{drops:?}");
            assert!(drops
                .iter()
                .all(|(i, n)| *i != item("minecraft:leather") || *n <= 2));
        }
    }

    #[test]
    fn sheep_drop_wool_of_their_colour_unless_sheared() {
        let black = EntityProps {
            sheep_color: Some("black"),
            ..Default::default()
        };
        let drops = mob_drops("minecraft:sheep", black.clone(), 3);
        assert!(
            drops.contains(&(item("minecraft:black_wool"), 1)),
            "{drops:?}"
        );
        assert!(drops.iter().any(|(i, _)| *i == item("minecraft:mutton")));
        let sheared = EntityProps {
            sheared: true,
            ..black
        };
        let drops = mob_drops("minecraft:sheep", sheared, 3);
        assert!(!drops
            .iter()
            .any(|(i, _)| *i == item("minecraft:black_wool")));
    }

    fn tables() -> &'static LootTables {
        static TABLES: std::sync::OnceLock<LootTables> = std::sync::OnceLock::new();
        TABLES.get_or_init(LootTables::load)
    }

    fn chest(table: &str, seed: i64) -> Vec<Stack> {
        let mut slots = vec![Stack::EMPTY; 27];
        assert!(tables().fill(table, seed, &mut slots), "{table}");
        slots.into_iter().filter(|s| s.count > 0).collect()
    }

    #[test]
    fn structure_chests_fill_from_their_tables() {
        // Buried treasure always holds a heart of the sea.
        let heart = item("minecraft:heart_of_the_sea");
        for seed in 1..20 {
            let items = chest("minecraft:chests/buried_treasure", seed);
            assert!(items.iter().any(|s| s.item == heart), "{items:?}");
            for s in &items {
                let max = ItemInfo::get(s.item).unwrap().max_stack;
                assert!(s.count >= 1 && s.count <= i32::from(max), "{s:?}");
            }
        }
        // Village chests sit under chests/village/.
        assert!(!chest("minecraft:chests/village/village_weaponsmith", 5).is_empty());
        // The same seed gives the same chest.
        assert_eq!(
            chest("minecraft:chests/simple_dungeon", 9),
            chest("minecraft:chests/simple_dungeon", 9)
        );
        let mut slots = vec![Stack::EMPTY; 27];
        assert!(!tables().fill("minecraft:chests/nothing_here", 1, &mut slots));
    }

    #[test]
    fn chest_loot_comes_enchanted_and_worn() {
        let book = item("minecraft:enchanted_book");
        let (mut books, mut enchanted_gear, mut worn) = (0, 0, 0);
        for seed in 1..200 {
            for s in chest("minecraft:chests/end_city_treasure", seed)
                .into_iter()
                .chain(chest("minecraft:chests/simple_dungeon", seed))
                .chain(chest("minecraft:chests/bastion_other", seed))
            {
                if s.item == book {
                    assert!(!s.enchants.is_empty());
                    books += 1;
                } else if !s.enchants.is_empty() {
                    enchanted_gear += 1;
                }
                if s.damage > 0 {
                    worn += 1;
                    assert!(s.damage < ItemInfo::get(s.item).unwrap().max_damage);
                }
            }
        }
        assert!(
            books > 0 && enchanted_gear > 0 && worn > 0,
            "{books} {enchanted_gear} {worn}"
        );
    }
}
