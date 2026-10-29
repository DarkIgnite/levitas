//! Anvils and grindstones: what they make of the items put in them.
//!
//! Follows the Minecraft Wiki's anvil mechanics: an item is repaired by a quarter of its
//! durability per unit of its repair material, or by combining it with a second one of
//! the same kind (plus 12%); enchantments from the second item or a book are merged,
//! equal levels going up one; every enchantment added and every unit used costs levels,
//! on top of both items' prior work penalty, which doubles plus one each time. Survival
//! players cannot pay 40 levels or more. Renaming costs a level more, or just one level
//! and the prior work penalty when renaming alone, which is always allowed. A grindstone
//! strips all but curses, merging two of the same item (plus 5%), and gives back some
//! experience.

use levitas_registry::{ItemInfo, TagRegistry};

use crate::enchanting::{definition, Enchantments};
use crate::inventory::Stack;

/// Survival players cannot pay this much at an anvil.
pub const TOO_EXPENSIVE: i32 = 40;

/// What the anvil offers: the result, its level cost, and how many of the right-hand
/// stack it uses up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnvilResult {
    pub result: Stack,
    pub cost: i32,
    pub uses: i32,
}

fn name(stack: &Stack) -> &'static str {
    ItemInfo::get(stack.item).map_or("", |i| i.name)
}

fn is_book(stack: &Stack) -> bool {
    name(stack) == "minecraft:enchanted_book"
}

fn max_damage(stack: &Stack) -> i32 {
    ItemInfo::get(stack.item).map_or(0, |i| i.max_damage)
}

/// Whether `material` repairs `stack` (iron ingots for iron tools, ...).
fn repairs(stack: &Stack, material: &Stack) -> bool {
    let Some(spec) = ItemInfo::get(stack.item).and_then(|i| i.repair_with) else {
        return false;
    };
    match spec.strip_prefix('#') {
        Some(tag) => TagRegistry::get("minecraft:item").is_some_and(|t| {
            t.members(tag)
                .binary_search(&i64::from(material.item))
                .is_ok()
        }),
        None => spec == name(material),
    }
}

/// The anvil's result for `left` and `right`, if any.
#[cfg(test)]
pub fn anvil(left: Stack, right: Stack) -> Option<AnvilResult> {
    anvil_named(left, right, None)
}

/// The anvil's result for `left` and `right` with the name typed in its box: `Some("")`
/// takes a custom name off, `None` leaves names alone.
pub fn anvil_named(left: Stack, right: Stack, rename: Option<&str>) -> Option<AnvilResult> {
    if left.is_empty() {
        return None;
    }
    let new_name = match rename.map(crate::names::clean) {
        Some(n) if n.trim().is_empty() => left.name.map(|_| None),
        Some(n) if Some(n.as_str()) != left.name => Some(crate::names::intern(&n)),
        _ => None,
    };
    if right.is_empty() {
        // Renaming alone: the whole stack, never too expensive.
        let name = new_name?;
        let result = Stack { name, ..left };
        let cost = (left.repair_cost + 1).min(TOO_EXPENSIVE - 1);
        return Some(AnvilResult {
            result,
            cost,
            uses: 0,
        });
    }
    let mut found = combine(left, right)?;
    if let Some(name) = new_name {
        found.result.name = name;
        found.cost += 1;
    }
    Some(found)
}

/// Repairing or enchanting `left` with `right`.
fn combine(left: Stack, right: Stack) -> Option<AnvilResult> {
    let mut result = left.with_count(1);
    let mut cost = left.repair_cost + right.repair_cost;
    let mut uses = 1;
    let max = max_damage(&left);

    if max > 0 && repairs(&left, &right) {
        // Each unit of material mends a quarter of the item.
        let quarter = (max / 4).max(1);
        if left.damage == 0 {
            return None;
        }
        let units = ((left.damage + quarter - 1) / quarter).min(right.count);
        result.damage = (left.damage - units * quarter).max(0);
        cost += units;
        uses = units;
    } else {
        let same = right.item == left.item;
        if !same && !is_book(&right) {
            return None;
        }
        let mut changed = false;
        if same && max > 0 && left.damage > 0 {
            let remaining = (max - left.damage) + (max - right.damage) + max * 12 / 100;
            result.damage = (max - remaining).max(0);
            cost += 2;
            changed = true;
        }
        let mut enchants = left.enchants;
        let mut incompatible = false;
        let mut any_applied = false;
        for (e, level) in right.enchants.iter() {
            let Some(def) = definition(e) else {
                continue;
            };
            let current = enchants.level(e);
            let fits = is_book(&left) || def.supports(left.item);
            let compatible = enchants
                .iter()
                .all(|(other, _)| other == e || def.compatible(other));
            if !fits || !compatible {
                incompatible = true;
                cost += 1;
                continue;
            }
            let new = if current == level {
                (level + 1).min(def.max_level)
            } else {
                current.max(level)
            };
            enchants.set(e, new);
            let per_level = if is_book(&right) {
                (def.anvil_cost / 2).max(1)
            } else {
                def.anvil_cost
            };
            cost += (per_level * new) as i32;
            any_applied = true;
        }
        if incompatible && !any_applied && !changed {
            return None;
        }
        if !any_applied && !changed {
            return None;
        }
        result.enchants = enchants;
    }
    result.repair_cost = left.repair_cost.max(right.repair_cost) * 2 + 1;
    Some(AnvilResult { result, cost, uses })
}

/// What a grindstone makes of its two inputs, and the experience it gives back.
pub fn grindstone(top: Stack, bottom: Stack) -> Option<(Stack, i32)> {
    let (item, other) = match (top.is_empty(), bottom.is_empty()) {
        (true, true) => return None,
        (false, true) => (top, None),
        (true, false) => (bottom, None),
        (false, false) => (top, Some(bottom)),
    };
    let mut result = item.with_count(1);
    let mut xp_base = 0;
    if let Some(other) = other {
        if other.item != item.item {
            return None;
        }
        let max = max_damage(&item);
        if max == 0 && item.enchants.is_empty() && other.enchants.is_empty() {
            return None;
        }
        if max > 0 {
            let remaining = (max - item.damage) + (max - other.damage) + max * 5 / 100;
            result.damage = (max - remaining).max(0);
        }
        xp_base += stripped_xp(&other.enchants);
    }
    xp_base += stripped_xp(&item.enchants);
    let mut kept = Enchantments::NONE;
    for (e, level) in item.enchants.iter() {
        if definition(e).is_some_and(|d| d.curse) {
            kept.set(e, level);
        }
    }
    if other.is_none() && kept == item.enchants {
        return None; // nothing to take off
    }
    result.enchants = kept;
    result.repair_cost = 0;
    if is_book(&result) && kept.is_empty() {
        if let Some(book) = ItemInfo::id_of("minecraft:book") {
            result.item = book;
        }
    }
    Some((result, xp_base))
}

/// The experience the grindstone's stripped enchantments are worth, before the random
/// part: the sum of their minimum costs.
fn stripped_xp(enchants: &Enchantments) -> i32 {
    enchants
        .iter()
        .filter_map(|(e, level)| {
            definition(e)
                .filter(|d| !d.curse)
                .map(|d| d.min_cost(level))
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enchanting::enchantment_id;

    fn stack(name: &str) -> Stack {
        Stack::new(ItemInfo::id_of(name).unwrap(), 1)
    }

    fn enchanted(name: &str, enchantment: &str, level: u32) -> Stack {
        let mut s = stack(name);
        s.enchants.set(enchantment_id(enchantment).unwrap(), level);
        s
    }

    #[test]
    fn books_add_enchantments_and_equal_levels_go_up() {
        let sword = enchanted("minecraft:diamond_sword", "minecraft:sharpness", 3);
        let book = enchanted("minecraft:enchanted_book", "minecraft:sharpness", 3);
        let r = anvil(sword, book).unwrap();
        assert_eq!(r.result.enchants.level_of("minecraft:sharpness"), 4);
        assert_eq!(r.result.repair_cost, 1);
        assert!(r.cost > 0 && r.cost < TOO_EXPENSIVE);

        // Smite does not go with sharpness.
        let smite = enchanted("minecraft:enchanted_book", "minecraft:smite", 1);
        assert!(anvil(sword, smite).is_none());
    }

    #[test]
    fn materials_mend_a_quarter_each() {
        let mut pick = stack("minecraft:iron_pickaxe");
        pick.damage = 200; // of 250
        let ingots = Stack::new(ItemInfo::id_of("minecraft:iron_ingot").unwrap(), 5);
        let r = anvil(pick, ingots).unwrap();
        assert_eq!(r.uses, 4);
        assert_eq!(r.result.damage, 0);
        assert!(anvil(pick, stack("minecraft:gold_ingot")).is_none());
        // Naming a stack names all of it, for a level and the work penalty.
        let dirt = Stack::new(ItemInfo::id_of("minecraft:dirt").unwrap(), 12);
        let named = anvil_named(dirt, Stack::EMPTY, Some("Soil")).unwrap();
        assert_eq!((named.result.name, named.result.count), (Some("Soil"), 12));
        assert_eq!((named.cost, named.uses), (1, 0));
        // An unchanged name is no work, and an empty box takes the name off.
        assert!(anvil_named(named.result, Stack::EMPTY, Some("Soil")).is_none());
        let plain = anvil_named(named.result, Stack::EMPTY, Some("")).unwrap();
        assert_eq!(plain.result.name, None);
        assert!(anvil_named(dirt, Stack::EMPTY, Some("")).is_none());
    }

    #[test]
    fn grindstones_strip_all_but_curses() {
        let mut sword = enchanted("minecraft:iron_sword", "minecraft:sharpness", 2);
        sword
            .enchants
            .set(enchantment_id("minecraft:vanishing_curse").unwrap(), 1);
        let (clean, xp) = grindstone(sword, Stack::EMPTY).unwrap();
        assert_eq!(clean.enchants.level_of("minecraft:sharpness"), 0);
        assert_eq!(clean.enchants.level_of("minecraft:vanishing_curse"), 1);
        assert!(xp > 0);
        let book = enchanted("minecraft:enchanted_book", "minecraft:mending", 1);
        assert_eq!(
            name(&grindstone(book, Stack::EMPTY).unwrap().0),
            "minecraft:book"
        );
        assert!(grindstone(stack("minecraft:iron_sword"), Stack::EMPTY).is_none());
    }
}
