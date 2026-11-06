//! Attributes that enchantments change, sent to each player for themselves. Since
//! 1.21 enchantments such as efficiency (mining efficiency), aqua affinity (submerged
//! mining speed), depth strider (water movement efficiency), respiration (oxygen
//! bonus), swift sneak (sneaking speed), blast and fire protection and sweeping edge
//! act through attribute modifiers that the server works out from the equipment and
//! sends; the client then mines, swims and sneaks by them. The effects are read from
//! the enchantment definitions the data generator wrote.

use std::sync::OnceLock;

use levitas_registry::{attribute, ENCHANTMENTS_JSON};
use serde_json::Value;

use super::*;

/// A modifier as the attributes packet carries it: its ID, amount and operation.
pub(super) type Modifier = (String, f64, u8);
/// Each attribute with the modifiers on it.
pub(super) type Modifiers = Vec<(i32, Vec<Modifier>)>;

/// One `minecraft:attributes` effect of an enchantment.
struct AttributeEffect {
    enchantment: i32,
    slots: Vec<String>,
    attribute: i32,
    id: String,
    amount: Value,
    operation: u8,
}

fn effects() -> &'static [AttributeEffect] {
    static EFFECTS: OnceLock<Vec<AttributeEffect>> = OnceLock::new();
    EFFECTS.get_or_init(|| {
        let all: serde_json::Map<String, Value> =
            serde_json::from_str(ENCHANTMENTS_JSON).expect("enchantments are valid JSON");
        let mut out = Vec::new();
        for (name, d) in all {
            let Some(enchantment) = crate::enchanting::enchantment_id(&name) else {
                continue;
            };
            let slots: Vec<String> = d["slots"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect();
            for e in d["effects"]["minecraft:attributes"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let Some(attribute) = e["attribute"]
                    .as_str()
                    .and_then(|a| attribute::NAMES.iter().position(|&n| n == a))
                else {
                    continue;
                };
                let operation = match e["operation"].as_str() {
                    Some("add_multiplied_base") => 1,
                    Some("add_multiplied_total") => 2,
                    _ => 0,
                };
                out.push(AttributeEffect {
                    enchantment,
                    slots: slots.clone(),
                    attribute: attribute as i32,
                    id: e["id"]
                        .as_str()
                        .unwrap_or("minecraft:enchantment")
                        .to_owned(),
                    amount: e["amount"].clone(),
                    operation,
                });
            }
        }
        out
    })
}

/// A level-based value (`minecraft:linear`, `levels_squared`, `fraction`, a number).
fn level_value(v: &Value, level: u32) -> f64 {
    let level = f64::from(level);
    if let Some(n) = v.as_f64() {
        return n;
    }
    match v["type"].as_str() {
        Some("minecraft:linear") => {
            v["base"].as_f64().unwrap_or(0.0)
                + v["per_level_above_first"].as_f64().unwrap_or(0.0) * (level - 1.0)
        }
        Some("minecraft:levels_squared") => level * level + v["added"].as_f64().unwrap_or(0.0),
        Some("minecraft:fraction") => {
            let d = level_value(&v["denominator"], level as u32);
            if d == 0.0 {
                0.0
            } else {
                level_value(&v["numerator"], level as u32) / d
            }
        }
        Some("minecraft:clamped") => level_value(&v["value"], level as u32).clamp(
            v["min"].as_f64().unwrap_or(f64::MIN),
            v["max"].as_f64().unwrap_or(f64::MAX),
        ),
        _ => 0.0,
    }
}

/// The value an attribute has with no modifiers, for a player.
fn base_value(id: i32) -> f64 {
    match id {
        attribute::SUBMERGED_MINING_SPEED => 0.2,
        attribute::SNEAKING_SPEED => 0.3,
        attribute::BURNING_TIME => 1.0,
        attribute::MAX_HEALTH => 20.0,
        _ => 0.0,
    }
}

/// Whether an enchantment slot group (`mainhand`, `armor`, `any`, ...) covers `slot`.
fn covers(group: &str, slot: &str) -> bool {
    group == slot
        || group == "any"
        || (group == "armor" && matches!(slot, "head" | "chest" | "legs" | "feet"))
        || (group == "hand" && matches!(slot, "mainhand" | "offhand"))
}

/// The modifiers a player's equipment puts on each attribute enchantments touch, as
/// `(attribute, [(modifier id, amount, operation)])`, every such attribute listed (so a
/// removed enchantment clears its modifier).
fn modifiers(p: &Player) -> Modifiers {
    let equipment = [
        ("mainhand", p.inventory.held(p.held)),
        ("offhand", p.inventory.slots[OFFHAND]),
        ("head", p.inventory.slots[5]),
        ("chest", p.inventory.slots[6]),
        ("legs", p.inventory.slots[7]),
        ("feet", p.inventory.slots[8]),
    ];
    let mut out: Modifiers = Vec::new();
    for e in effects() {
        if !out.iter().any(|(a, _)| *a == e.attribute) {
            out.push((e.attribute, Vec::new()));
        }
        for (slot, stack) in equipment {
            if stack.is_empty() || !e.slots.iter().any(|g| covers(g, slot)) {
                continue;
            }
            let level = stack
                .enchants
                .iter()
                .find(|&(id, _)| id == e.enchantment)
                .map_or(0, |(_, l)| l);
            if level == 0 {
                continue;
            }
            let amount = level_value(&e.amount, level);
            let list = &mut out
                .iter_mut()
                .find(|(a, _)| *a == e.attribute)
                .expect("listed")
                .1;
            // Vanilla names each modifier after the enchantment and its slot.
            list.push((format!("{}/{slot}", e.id), amount, e.operation));
        }
    }
    // Health Boost: 4 more health a level, under vanilla's modifier name.
    let boost = p.effect_level(levitas_registry::mob_effect::HEALTH_BOOST);
    let mut health = Vec::new();
    if boost > 0 {
        health.push((
            "minecraft:effect.health_boost".to_owned(),
            4.0 * f64::from(boost),
            0,
        ));
    }
    out.push((attribute::MAX_HEALTH, health));
    out
}

fn attributes_packet(id: i32, attrs: &[(i32, Vec<Modifier>)]) -> Writer {
    let mut packet = Writer::packet(clientbound::UPDATE_ATTRIBUTES)
        .varint(id)
        .varint(attrs.len() as i32);
    for (attribute, mods) in attrs {
        packet = packet
            .varint(*attribute)
            .f64(base_value(*attribute))
            .varint(mods.len() as i32);
        for (name, amount, operation) in mods {
            packet = packet.string(name).f64(*amount).u8(*operation);
        }
    }
    packet
}

impl World {
    /// Sends each player the attributes their enchanted equipment gives, whenever that
    /// changes (checked every few ticks).
    pub(super) fn tick_attributes(&mut self) {
        if !self.tick.is_multiple_of(4) {
            return;
        }
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for id in ids {
            let p = &self.players[&id];
            let now = modifiers(p);
            if p.sent_attributes.as_ref() == Some(&now) {
                continue;
            }
            let packet = attributes_packet(id, &now);
            self.players.get_mut(&id).expect("listed").sent_attributes = Some(now);
            self.send(id, packet);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn enchanted_gear_gives_the_attributes_vanilla_does() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let profile = Profile {
            uuid: 3,
            name: "Geared".to_owned(),
            properties: Vec::new(),
        };
        let mut p = Player::new(profile, tx, [0.0; 3]);
        let pick = ItemInfo::id_of("minecraft:diamond_pickaxe").unwrap();
        let helmet = ItemInfo::id_of("minecraft:iron_helmet").unwrap();
        let mut tool = Stack::new(pick, 1);
        tool.enchants.set(
            crate::enchanting::enchantment_id("minecraft:efficiency").unwrap(),
            3,
        );
        let mut hat = Stack::new(helmet, 1);
        hat.enchants.set(
            crate::enchanting::enchantment_id("minecraft:aqua_affinity").unwrap(),
            1,
        );
        let slot = p.hand_slot(0);
        p.inventory.slots[slot] = tool;
        p.inventory.slots[5] = hat;
        let mods = modifiers(&p);
        let of = |a: i32| &mods.iter().find(|(x, _)| *x == a).unwrap().1;
        // Efficiency III: 3 squared plus 1.
        assert_eq!(of(attribute::MINING_EFFICIENCY)[0].1, 10.0);
        // Aqua affinity multiplies the total by 1 + 4: 0.2 becomes 1.
        assert_eq!(of(attribute::SUBMERGED_MINING_SPEED)[0].1, 4.0);
        assert_eq!(of(attribute::SUBMERGED_MINING_SPEED)[0].2, 2);
        // Gear without the enchantment leaves the attribute bare (and listed).
        assert!(of(attribute::WATER_MOVEMENT_EFFICIENCY).is_empty());
    }
}
