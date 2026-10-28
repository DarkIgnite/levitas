//! Loot tables: what a broken block or a killed mob drops. Tables are the vanilla JSON
//! from the data generator, evaluated with the context Levitas tracks so far: the block
//! state, the tool item and its enchantments (silk touch, fortune, looting) and a few
//! mob properties.

use std::collections::HashMap;

use levitas_registry::{BlockInfo, ItemInfo, TagRegistry, BLOCK_LOOT_JSON, ENTITY_LOOT_JSON};
use serde_json::Value;

use crate::enchanting::Enchantments;
use crate::rng::Rng;

pub struct LootTables {
    blocks: HashMap<String, Value>,
    /// Entity tables, keyed `minecraft:<entity>` (and `minecraft:sheep/<color>`).
    entities: HashMap<String, Value>,
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
        let item_ids = levitas_registry::ITEMS
            .iter()
            .enumerate()
            .map(|(id, item)| (item.name, id as i32))
            .collect();
        Self {
            blocks: blocks.into_iter().collect(),
            entities: entities.into_iter().collect(),
            item_ids,
        }
    }

    /// Items dropped by breaking a block, as `(item ID, count)` pairs.
    pub fn block_drops(&self, ctx: &mut LootContext) -> Vec<(i32, i32)> {
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
        drops
    }

    /// Items dropped by a mob of `entity` type (e.g. `minecraft:cow`) when it dies.
    pub fn entity_drops(&self, entity: &str, ctx: &mut LootContext) -> Vec<(i32, i32)> {
        let mut drops = Vec::new();
        if let Some(table) = self.entities.get(entity) {
            self.roll(table, ctx, &mut drops);
        }
        drops
    }

    /// The table a `minecraft:loot_table` entry refers to.
    fn table(&self, name: &str) -> Option<&Value> {
        let path = name.strip_prefix("minecraft:").unwrap_or(name);
        if let Some(entity) = path.strip_prefix("entities/") {
            return self.entities.get(&format!("minecraft:{entity}"));
        }
        let block = path.strip_prefix("blocks/")?;
        self.blocks.get(&format!("minecraft:{block}"))
    }

    fn roll(&self, table: &Value, ctx: &mut LootContext, drops: &mut Vec<(i32, i32)>) {
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
                let mut count = 1;
                for function in array(&entry["functions"])
                    .iter()
                    .chain(array(&pool["functions"]))
                    .chain(array(&table["functions"]))
                {
                    if conditions(function, ctx) {
                        count = apply(function, count, ctx);
                    }
                }
                if count > 0 {
                    drops.push((item, count));
                }
            }
        }
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
}
