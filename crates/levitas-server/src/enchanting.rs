//! Enchantments: what an item carries, the definitions from the data generator, and how
//! an enchanting table picks them.
//!
//! The table follows the Minecraft Wiki's enchanting mechanics: bookshelves set a base
//! level, the three slots cost a third, two thirds and all of it, and each offer rolls
//! a modified level from the item's enchantability, picks weighted enchantments whose
//! cost range contains it, and keeps adding compatible ones with falling odds.

use std::sync::OnceLock;

use levitas_protocol::{Reader, Writer};
use levitas_registry::{ItemInfo, SyncedRegistry, TagRegistry, ENCHANTMENTS_JSON};
use serde_json::Value;

use crate::rng::Rng;

/// The most enchantments one item keeps.
const MAX: usize = 8;

/// Enchantments on an item: `(registry ID, level)` pairs, level 0 marking free room.
/// A fixed array so stacks stay `Copy`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Enchantments([(u8, u8); MAX]);

impl Enchantments {
    pub const NONE: Enchantments = Enchantments([(0, 0); MAX]);

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&(_, level)| level == 0)
    }

    /// Each enchantment's ID and level.
    pub fn iter(&self) -> impl Iterator<Item = (i32, u32)> + '_ {
        self.0
            .iter()
            .filter(|&&(_, level)| level > 0)
            .map(|&(id, level)| (i32::from(id), u32::from(level)))
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    /// The level of an enchantment, 0 if absent.
    pub fn level(&self, id: i32) -> u32 {
        self.iter().find(|&(e, _)| e == id).map_or(0, |(_, l)| l)
    }

    /// The level of an enchantment by name (`minecraft:sharpness`), 0 if absent.
    pub fn level_of(&self, name: &str) -> u32 {
        enchantment_id(name).map_or(0, |id| self.level(id))
    }

    /// Sets an enchantment's level, replacing any it had. Ignored when full.
    pub fn set(&mut self, id: i32, level: u32) {
        let (Ok(id), Ok(level)) = (u8::try_from(id), u8::try_from(level.min(255))) else {
            return;
        };
        if let Some(slot) = self.0.iter_mut().find(|(e, l)| *e == id && *l > 0) {
            slot.1 = level;
        } else if let Some(slot) = self.0.iter_mut().find(|(_, l)| *l == 0) {
            *slot = (id, level);
        }
    }

    /// The component's network form: a count, then ID and level pairs.
    pub fn write(&self, packet: Writer) -> Writer {
        let mut packet = packet.varint(self.len() as i32);
        for (id, level) in self.iter() {
            packet = packet.varint(id).varint(level as i32);
        }
        packet
    }

    pub fn read(r: &mut Reader<'_>) -> std::io::Result<Self> {
        let mut e = Enchantments::NONE;
        for _ in 0..r.varint()?.clamp(0, 64) {
            let id = r.varint()?;
            let level = r.varint()?;
            e.set(id, level.max(0) as u32);
        }
        Ok(e)
    }

    /// The saved form: a compound of names to levels.
    pub fn to_nbt(self) -> levitas_nbt::Compound {
        let mut c = levitas_nbt::Compound::new();
        for (id, level) in self.iter() {
            if let Some(name) = enchantment_name(id) {
                c.insert(name, level as i32);
            }
        }
        c
    }

    /// Reads the saved form, also accepting the older `{levels: {...}}` wrapping.
    pub fn from_nbt(c: &levitas_nbt::Compound) -> Self {
        let c = c.compound("levels").unwrap_or(c);
        let mut e = Enchantments::NONE;
        for (name, tag) in c.iter() {
            if let (Some(id), Some(level)) = (enchantment_id(name), tag.as_i64()) {
                e.set(id, level.max(0) as u32);
            }
        }
        e
    }
}

fn registry() -> Option<&'static SyncedRegistry> {
    SyncedRegistry::get("minecraft:enchantment")
}

pub fn enchantment_id(name: &str) -> Option<i32> {
    registry()?.id_of(name)
}

pub fn enchantment_name(id: i32) -> Option<&'static str> {
    registry()?.entries.get(usize::try_from(id).ok()?).copied()
}

/// An enchantment's definition, as far as the table needs it.
pub struct Definition {
    pub id: i32,
    pub max_level: u32,
    pub weight: u32,
    min_cost: (i32, i32),
    max_cost: (i32, i32),
    /// The anvil's level cost per enchantment level.
    pub anvil_cost: u32,
    /// Curses survive the grindstone.
    pub curse: bool,
    supported: Value,
    primary: Option<Value>,
    exclusive: Option<Value>,
    in_table: bool,
}

impl Definition {
    pub fn min_cost(&self, level: u32) -> i32 {
        self.min_cost.0 + self.min_cost.1 * (level as i32 - 1)
    }

    fn max_cost(&self, level: u32) -> i32 {
        self.max_cost.0 + self.max_cost.1 * (level as i32 - 1)
    }

    /// Whether the enchantment can go on `item` at all (anvils, books).
    pub fn supports(&self, item: i32) -> bool {
        item_matches(&self.supported, item)
    }

    /// Whether an enchanting table offers it for `item`.
    fn offered_for(&self, item: i32) -> bool {
        is_book(item) || item_matches(self.primary.as_ref().unwrap_or(&self.supported), item)
    }

    /// Whether this and `other` may be on one item.
    pub fn compatible(&self, other: i32) -> bool {
        if other == self.id {
            return false;
        }
        let Some(set) = &self.exclusive else {
            return true;
        };
        match set {
            Value::String(s) => match s.strip_prefix('#') {
                Some(tag) => !TagRegistry::get("minecraft:enchantment")
                    .is_some_and(|t| t.members(tag).contains(&i64::from(other))),
                None => enchantment_id(s) != Some(other),
            },
            Value::Array(list) => !list
                .iter()
                .filter_map(Value::as_str)
                .any(|n| enchantment_id(n) == Some(other)),
            _ => true,
        }
    }
}

fn is_book(item: i32) -> bool {
    ItemInfo::get(item).is_some_and(|i| i.name == "minecraft:book")
}

fn item_matches(spec: &Value, item: i32) -> bool {
    let name = ItemInfo::get(item).map_or("", |i| i.name);
    let one = |s: &str| match s.strip_prefix('#') {
        Some(tag) => TagRegistry::get("minecraft:item")
            .is_some_and(|t| t.members(tag).binary_search(&i64::from(item)).is_ok()),
        None => s == name,
    };
    match spec {
        Value::String(s) => one(s),
        Value::Array(list) => list.iter().filter_map(Value::as_str).any(one),
        _ => false,
    }
}

/// Every enchantment's definition.
pub fn definitions() -> &'static [Definition] {
    static DEFS: OnceLock<Vec<Definition>> = OnceLock::new();
    DEFS.get_or_init(|| {
        let all: serde_json::Map<String, Value> =
            serde_json::from_str(ENCHANTMENTS_JSON).expect("enchantments are valid JSON");
        let in_table = TagRegistry::get("minecraft:enchantment")
            .map_or(&[][..], |t| t.members("minecraft:in_enchanting_table"));
        let curses = TagRegistry::get("minecraft:enchantment")
            .map_or(&[][..], |t| t.members("minecraft:curse"));
        let cost = |v: &Value| {
            (
                v["base"].as_i64().unwrap_or(0) as i32,
                v["per_level_above_first"].as_i64().unwrap_or(0) as i32,
            )
        };
        let mut defs: Vec<Definition> = all
            .into_iter()
            .filter_map(|(name, d)| {
                let id = enchantment_id(&name)?;
                Some(Definition {
                    id,
                    max_level: d["max_level"].as_u64().unwrap_or(1) as u32,
                    weight: d["weight"].as_u64().unwrap_or(1) as u32,
                    min_cost: cost(&d["min_cost"]),
                    max_cost: cost(&d["max_cost"]),
                    anvil_cost: d["anvil_cost"].as_u64().unwrap_or(1) as u32,
                    curse: curses.contains(&i64::from(id)),
                    supported: d["supported_items"].clone(),
                    primary: d.get("primary_items").cloned(),
                    exclusive: d.get("exclusive_set").cloned(),
                    in_table: in_table.contains(&i64::from(id)),
                })
            })
            .collect();
        defs.sort_by_key(|d| d.id);
        defs
    })
}

pub fn definition(id: i32) -> Option<&'static Definition> {
    definitions().iter().find(|d| d.id == id)
}

/// How many bookshelves count towards a table: at most 15.
pub const MAX_BOOKSHELVES: u32 = 15;

/// The level each of the three table slots costs with `bookshelves` around.
pub fn slot_costs(rng: &mut Rng, bookshelves: u32, item: i32) -> [u32; 3] {
    if ItemInfo::get(item).is_none_or(|i| i.enchantability == 0) {
        return [0; 3];
    }
    let b = bookshelves.min(MAX_BOOKSHELVES);
    let mut costs = [0; 3];
    for (slot, cost) in costs.iter_mut().enumerate() {
        let base = 1 + rng.below(8) + b / 2 + rng.below(b + 1);
        *cost = match slot {
            0 => (base / 3).max(1),
            1 => base * 2 / 3 + 1,
            _ => base.max(b * 2),
        };
    }
    costs
}

/// The enchantments an enchanting table puts on `item` for `level`.
pub fn select(rng: &mut Rng, item: i32, level: u32) -> Vec<(i32, u32)> {
    let Some(info) = ItemInfo::get(item).filter(|i| i.enchantability > 0) else {
        return Vec::new();
    };
    let e = u32::from(info.enchantability);
    let mut power = level + 1 + rng.below(e / 4 + 1) + rng.below(e / 4 + 1);
    let bonus = 1.0 + (rng.f64() as f32 + rng.f64() as f32 - 1.0) * 0.15;
    power = ((power as f32 * bonus).round() as u32).max(1);

    let mut pool = candidates(item, power);
    let mut chosen = Vec::new();
    let Some(first) = weighted(rng, &pool) else {
        return chosen;
    };
    chosen.push(first);
    while rng.below(50) <= power {
        pool.retain(|&(id, _)| {
            chosen
                .iter()
                .all(|&(c, _)| definition(c).is_some_and(|d| d.compatible(id)))
        });
        let Some(next) = weighted(rng, &pool) else {
            break;
        };
        chosen.push(next);
        power /= 2;
    }
    chosen
}

/// Each enchantment the table can offer for `item` at `power`, at the highest level
/// whose cost range contains it.
fn candidates(item: i32, power: u32) -> Vec<(i32, u32)> {
    let power = power as i32;
    definitions()
        .iter()
        .filter(|d| d.in_table && d.offered_for(item))
        .filter_map(|d| {
            (1..=d.max_level)
                .rev()
                .find(|&l| d.min_cost(l) <= power && power <= d.max_cost(l))
                .map(|l| (d.id, l))
        })
        .collect()
}

fn weighted(rng: &mut Rng, pool: &[(i32, u32)]) -> Option<(i32, u32)> {
    let weight = |id: i32| definition(id).map_or(0, |d| d.weight);
    let total: u32 = pool.iter().map(|&(id, _)| weight(id)).sum();
    if total == 0 {
        return None;
    }
    let mut pick = rng.below(total);
    pool.iter().copied().find(|&(id, _)| {
        let w = weight(id);
        if pick < w {
            true
        } else {
            pick -= w;
            false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> i32 {
        ItemInfo::id_of(name).unwrap()
    }

    #[test]
    fn enchantments_keep_their_levels() {
        let sharpness = enchantment_id("minecraft:sharpness").unwrap();
        let mut e = Enchantments::NONE;
        e.set(sharpness, 3);
        assert_eq!(e.level_of("minecraft:sharpness"), 3);
        e.set(sharpness, 5);
        assert_eq!(e.len(), 1);
        assert_eq!(Enchantments::from_nbt(&e.to_nbt()), e);
    }

    #[test]
    fn tables_offer_fitting_enchantments() {
        let sword = item("minecraft:diamond_sword");
        let mut rng = Rng::new(7);
        for _ in 0..200 {
            let picked = select(&mut rng, sword, 30);
            assert!(!picked.is_empty());
            for &(id, _) in &picked {
                let d = definition(id).unwrap();
                assert!(d.supports(sword), "{:?}", enchantment_name(id));
            }
            // Sharpness, smite and bane of arthropods never come together.
            let damage = ["sharpness", "smite", "bane_of_arthropods"]
                .iter()
                .filter(|n| {
                    picked
                        .iter()
                        .any(|&(id, _)| enchantment_name(id) == Some(&format!("minecraft:{n}")))
                })
                .count();
            assert!(damage <= 1);
        }
        assert!(select(&mut rng, item("minecraft:dirt"), 30).is_empty());
        assert!(!select(&mut rng, item("minecraft:book"), 30).is_empty());
    }

    #[test]
    fn bookshelves_raise_costs() {
        let sword = item("minecraft:iron_sword");
        let mut rng = Rng::new(1);
        let full = slot_costs(&mut rng, 15, sword);
        assert_eq!(full[2], 30);
        let none = slot_costs(&mut rng, 0, sword);
        assert!(none[2] <= 8);
        assert_eq!(slot_costs(&mut rng, 15, item("minecraft:dirt")), [0; 3]);
    }
}
