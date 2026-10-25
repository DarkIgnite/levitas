//! Block loot tables: what a broken block drops. Tables are the vanilla JSON from the data
//! generator, evaluated with the context Levitas tracks so far: the block state and the
//! tool item. Enchantments are not tracked yet, so silk touch and fortune never apply.

use std::collections::HashMap;

use levitas_registry::{BlockInfo, ItemInfo, TagRegistry, BLOCK_LOOT_JSON};
use serde_json::Value;

use crate::rng::Rng;

pub struct LootTables {
    blocks: HashMap<String, Value>,
    item_ids: HashMap<&'static str, i32>,
}

/// What a loot table is rolled with.
pub struct LootContext<'a> {
    pub state: u16,
    /// Item ID of the tool used, 0 for an empty hand.
    pub tool: i32,
    pub rng: &'a mut Rng,
}

impl LootTables {
    pub fn load() -> Self {
        let blocks: serde_json::Map<String, Value> =
            serde_json::from_str(BLOCK_LOOT_JSON).expect("block loot tables are valid JSON");
        let item_ids = levitas_registry::ITEMS
            .iter()
            .enumerate()
            .map(|(id, item)| (item.name, id as i32))
            .collect();
        Self {
            blocks: blocks.into_iter().collect(),
            item_ids,
        }
    }

    /// Items dropped by breaking a block, as `(item ID, count)` pairs.
    pub fn block_drops(&self, ctx: &mut LootContext) -> Vec<(i32, i32)> {
        let name = BlockInfo::of(ctx.state).name;
        let Some(table) = self.blocks.get(name) else {
            return Vec::new();
        };
        let mut drops = Vec::new();
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
                        count = apply(function, count, ctx.rng);
                    }
                }
                if count > 0 {
                    drops.push((item, count));
                }
            }
        }
        drops
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
        "minecraft:match_tool" => match_tool(&c["predicate"], ctx.tool),
        "minecraft:random_chance" => ctx.rng.f64() < number(&c["chance"], ctx.rng),
        "minecraft:random_chance_with_enchanted_bonus" => {
            ctx.rng.f64() < number(&c["unenchanted_chance"], ctx.rng)
        }
        // No fortune yet: use the chance for enchantment level 0.
        "minecraft:table_bonus" => {
            let chance = c["chances"][0].as_f64().unwrap_or(0.0);
            ctx.rng.f64() < chance
        }
        "minecraft:block_state_property" => block_state_matches(&c["properties"], ctx.state),
        "minecraft:inverted" => !condition(&c["term"], ctx),
        "minecraft:any_of" => array(&c["terms"]).iter().any(|t| condition(t, ctx)),
        "minecraft:all_of" => array(&c["terms"]).iter().all(|t| condition(t, ctx)),
        _ => false,
    }
}

/// Matches an item predicate. Predicates on components (enchantments and so on) never
/// match, since item components are not tracked yet.
fn match_tool(predicate: &Value, tool: i32) -> bool {
    if predicate.get("predicates").is_some() {
        return false;
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

fn apply(function: &Value, count: i32, rng: &mut Rng) -> i32 {
    match function["function"].as_str().unwrap_or_default() {
        "minecraft:set_count" => {
            let n = number(&function["count"], rng).floor() as i32;
            if function["add"].as_bool() == Some(true) {
                count + n
            } else {
                n
            }
        }
        // Fortune level 0: only the binomial formula adds anything.
        "minecraft:apply_bonus" if function["formula"] == "minecraft:binomial_with_bonus_count" => {
            let extra = function["parameters"]["extra"].as_i64().unwrap_or(0);
            let p = function["parameters"]["probability"]
                .as_f64()
                .unwrap_or(0.0);
            count + (0..extra).filter(|_| rng.f64() < p).count() as i32
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
        let tables = LootTables::load();
        let tool = ItemInfo::id_of(tool).unwrap_or(0);
        let mut rng = Rng::new(1);
        tables.block_drops(&mut LootContext {
            state,
            tool,
            rng: &mut rng,
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

    #[test]
    fn grass_block_drops_dirt() {
        assert_eq!(
            drops(block::GRASS_BLOCK, "minecraft:air"),
            [(item("minecraft:dirt"), 1)]
        );
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
}
