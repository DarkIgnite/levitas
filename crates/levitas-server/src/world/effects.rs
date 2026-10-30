//! Status effects on players: given by food, milk and `/effect`, counted down each tick,
//! shown in the client's effect list, saved with the player.
//!
//! What each effect does follows the Minecraft Wiki. The server applies the ones that
//! change its own rules: regeneration, poison, wither, hunger, saturation, instant health
//! and damage, absorption, resistance, fire resistance, water breathing, strength,
//! weakness, haste, mining fatigue, jump boost and slow falling (fall damage). Movement
//! and sight effects (speed, slowness, jump boost, levitation, night vision, blindness,
//! ...) are carried out by the client once it is told about them.

use levitas_nbt::{Compound, Tag};
use levitas_registry::{mob_effect, Effect};

use super::*;
use crate::world::survival::{Damage, MAX_HEALTH};

/// Shown in the inventory and the HUD.
const FLAG_SHOW_ICON: u8 = 0x04;
const FLAG_SHOW_PARTICLES: u8 = 0x02;
/// Entity data index of a player's extra (absorption) health, a float.
const DATA_ABSORPTION: u8 = 17;
const SERIALIZER_FLOAT: i32 = 3;
/// A duration of -1 lasts forever.
pub const INFINITE: i32 = -1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveEffect {
    pub id: i32,
    pub amplifier: u8,
    /// Ticks left, or `INFINITE`.
    pub duration: i32,
}

/// The registry name of an effect (`minecraft:speed`), from its ID.
pub fn effect_name(id: i32) -> Option<&'static str> {
    usize::try_from(id)
        .ok()
        .and_then(|i| mob_effect::NAMES.get(i))
        .copied()
}

/// An effect's ID from its name, with or without the `minecraft:` prefix.
pub fn effect_id(name: &str) -> Option<i32> {
    let full = if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    };
    mob_effect::NAMES
        .iter()
        .position(|n| *n == full)
        .map(|i| i as i32)
}

/// Whether an effect's periodic action happens this tick: regeneration heals every 50
/// ticks, poison every 25 and wither every 40, twice as often per level.
pub(super) fn pulses(duration: i32, interval: i32, amplifier: u8) -> bool {
    let every = interval >> amplifier.min(31);
    every <= 0 || duration % every == 0
}

impl Player {
    pub fn effect(&self, id: i32) -> Option<&ActiveEffect> {
        self.effects.iter().find(|e| e.id == id)
    }

    /// The level of an effect, counting from 1, or 0 when the player does not have it.
    pub fn effect_level(&self, id: i32) -> u32 {
        self.effect(id).map_or(0, |e| u32::from(e.amplifier) + 1)
    }

    /// How much faster the player digs with haste, or slower with mining fatigue.
    pub fn dig_speed_factor(&self) -> f32 {
        let haste = self.effect_level(mob_effect::HASTE);
        let fatigue = self.effect_level(mob_effect::MINING_FATIGUE);
        let mut factor = 1.0 + 0.2 * haste as f32;
        if fatigue > 0 {
            factor *= 0.3f32.powi(fatigue.min(4) as i32);
        }
        factor
    }

    /// Attack damage added by strength and taken away by weakness.
    pub fn attack_bonus(&self) -> f32 {
        3.0 * self.effect_level(mob_effect::STRENGTH) as f32
            - 4.0 * self.effect_level(mob_effect::WEAKNESS) as f32
    }
}

pub(super) fn update_packet(entity: i32, e: &ActiveEffect) -> Writer {
    Writer::packet(clientbound::UPDATE_MOB_EFFECT)
        .varint(entity)
        .varint(e.id)
        .varint(i32::from(e.amplifier))
        .varint(e.duration)
        .u8(FLAG_SHOW_ICON | FLAG_SHOW_PARTICLES)
}

pub(super) fn remove_packet(entity: i32, effect: i32) -> Writer {
    Writer::packet(clientbound::REMOVE_MOB_EFFECT)
        .varint(entity)
        .varint(effect)
}

impl World {
    /// Gives a player an effect. A stronger or longer one replaces what they have.
    pub(super) fn add_effect(&mut self, id: i32, effect: Effect) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if !p.alive {
            return;
        }
        let new = ActiveEffect {
            id: effect.id,
            amplifier: effect.amplifier,
            duration: effect.duration,
        };
        match effect.id {
            mob_effect::INSTANT_HEALTH => {
                self.heal(id, f32::from(4u8 << effect.amplifier.min(5)));
                return;
            }
            mob_effect::INSTANT_DAMAGE => {
                let amount = f32::from(6u8 << effect.amplifier.min(5));
                self.damage(id, amount, Damage::Magic);
                return;
            }
            mob_effect::SATURATION => {
                let level = i32::from(effect.amplifier) + 1;
                p.food = (p.food + level).min(20);
                p.saturation = (p.saturation + level as f32 * 2.0).min(p.food as f32);
                p.health_dirty = true;
                return;
            }
            _ => {}
        }
        let lasts_longer = |old: &ActiveEffect| {
            old.duration != INFINITE && (new.duration == INFINITE || new.duration > old.duration)
        };
        match p.effects.iter_mut().find(|e| e.id == new.id) {
            Some(old) if new.amplifier > old.amplifier => *old = new,
            Some(old) if new.amplifier == old.amplifier && lasts_longer(old) => *old = new,
            Some(_) => return,
            None => p.effects.push(new),
        }
        if new.id == mob_effect::ABSORPTION {
            let extra = 4.0 * (f32::from(new.amplifier) + 1.0);
            p.absorption = p.absorption.max(extra);
            self.send_absorption(id);
        }
        self.broadcast(update_packet(id, &new), None);
    }

    /// Takes an effect away.
    pub(super) fn remove_effect(&mut self, id: i32, effect: i32) -> bool {
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        let before = p.effects.len();
        p.effects.retain(|e| e.id != effect);
        if p.effects.len() == before {
            return false;
        }
        if effect == mob_effect::ABSORPTION {
            p.absorption = 0.0;
            self.send_absorption(id);
        }
        self.broadcast(remove_packet(id, effect), None);
        true
    }

    /// Takes every effect away (milk, death). Returns how many there were.
    pub(super) fn clear_effects(&mut self, id: i32) -> usize {
        let ids: Vec<i32> = self
            .players
            .get(&id)
            .map(|p| p.effects.iter().map(|e| e.id).collect())
            .unwrap_or_default();
        for &effect in &ids {
            self.remove_effect(id, effect);
        }
        ids.len()
    }

    pub(super) fn heal(&mut self, id: i32, amount: f32) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if p.alive && p.health < MAX_HEALTH {
            p.health = (p.health + amount).min(MAX_HEALTH);
            p.health_dirty = true;
        }
    }

    /// Gives a mob an effect. A stronger or longer one replaces what it has.
    pub(super) fn add_mob_effect(&mut self, id: i32, effect: Effect) {
        let (is_undead, is_spider) = match self.mobs.get(&id) {
            Some(m) if m.alive() => (m.kind.is_undead(), m.kind == super::mobs::MobKind::Spider),
            _ => return,
        };
        match effect.id {
            mob_effect::INSTANT_HEALTH => {
                if is_undead {
                    let amount = f32::from(6u8 << effect.amplifier.min(5));
                    self.hit_mob(id, amount, None, [0.0, 0.0], "minecraft:magic");
                } else {
                    self.heal_mob(id, f32::from(4u8 << effect.amplifier.min(5)));
                }
                return;
            }
            mob_effect::INSTANT_DAMAGE => {
                if is_undead {
                    self.heal_mob(id, f32::from(4u8 << effect.amplifier.min(5)));
                } else {
                    let amount = f32::from(6u8 << effect.amplifier.min(5));
                    self.hit_mob(id, amount, None, [0.0, 0.0], "minecraft:magic");
                }
                return;
            }
            mob_effect::REGENERATION if is_undead => return,
            mob_effect::POISON if is_undead || is_spider => return,
            _ => {}
        }
        let new = ActiveEffect {
            id: effect.id,
            amplifier: effect.amplifier,
            duration: effect.duration,
        };
        let lasts_longer = |old: &ActiveEffect| {
            old.duration != INFINITE && (new.duration == INFINITE || new.duration > old.duration)
        };
        let Some(mob) = self.mobs.get_mut(&id) else {
            return;
        };
        match mob.effects.iter_mut().find(|e| e.id == new.id) {
            Some(old) if new.amplifier > old.amplifier => *old = new,
            Some(old) if new.amplifier == old.amplifier && lasts_longer(old) => *old = new,
            Some(_) => return,
            None => mob.effects.push(new),
        }
        let viewers: Vec<i32> = mob.viewers.iter().copied().collect();
        let f = frame(update_packet(id, &new));
        for pid in viewers {
            if let Some(p) = self.players.get_mut(&pid) {
                p.deliver(&f);
            }
        }
    }

    /// Takes an effect away from a mob.
    #[allow(dead_code)]
    pub(super) fn remove_mob_effect(&mut self, id: i32, effect: i32) -> bool {
        let Some(mob) = self.mobs.get_mut(&id) else {
            return false;
        };
        let before = mob.effects.len();
        mob.effects.retain(|e| e.id != effect);
        if mob.effects.len() == before {
            return false;
        }
        let viewers: Vec<i32> = mob.viewers.iter().copied().collect();
        let f = frame(remove_packet(id, effect));
        for pid in viewers {
            if let Some(p) = self.players.get_mut(&pid) {
                p.deliver(&f);
            }
        }
        true
    }

    /// Clears every effect on a mob.
    #[allow(dead_code)]
    pub(super) fn clear_mob_effects(&mut self, id: i32) -> usize {
        let ids: Vec<i32> = self
            .mobs
            .get(&id)
            .map(|m| m.effects.iter().map(|e| e.id).collect())
            .unwrap_or_default();
        for &effect in &ids {
            self.remove_mob_effect(id, effect);
        }
        ids.len()
    }

    pub(super) fn heal_mob(&mut self, id: i32, amount: f32) {
        let Some(mob) = self.mobs.get_mut(&id) else {
            return;
        };
        if mob.alive() && mob.health < mob.kind.max_health() {
            mob.health = (mob.health + amount).min(mob.kind.max_health());
        }
    }

    pub(super) fn send_absorption(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let packet = Writer::packet(clientbound::SET_ENTITY_DATA)
            .varint(id)
            .u8(DATA_ABSORPTION)
            .varint(SERIALIZER_FLOAT)
            .f32(p.absorption)
            .u8(0xFF);
        self.broadcast(packet, None);
    }

    /// Shows a joining player the effects they have.
    pub(super) fn send_effects(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let packets: Vec<Writer> = p.effects.iter().map(|e| update_packet(id, e)).collect();
        let has_absorption = p.absorption > 0.0;
        for packet in packets {
            self.send(id, packet);
        }
        if has_absorption {
            self.send_absorption(id);
        }
    }

    /// Effects act and count down, once per tick.
    pub(super) fn tick_effects(&mut self) {
        let ids: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive && !p.effects.is_empty())
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let effects = self.players[&id].effects.clone();
            for e in effects {
                self.effect_acts(id, &e);
            }
            let Some(p) = self.players.get_mut(&id) else {
                continue;
            };
            let mut ended = Vec::new();
            for e in &mut p.effects {
                if e.duration == INFINITE {
                    continue;
                }
                e.duration -= 1;
                if e.duration <= 0 {
                    ended.push(e.id);
                }
            }
            for effect in ended {
                if effect == mob_effect::RAID_OMEN {
                    self.raid_omen_expired(id);
                }
                self.remove_effect(id, effect);
            }
        }
    }

    fn effect_acts(&mut self, id: i32, e: &ActiveEffect) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if !p.alive {
            return;
        }
        match e.id {
            mob_effect::REGENERATION if pulses(e.duration, 50, e.amplifier) => self.heal(id, 1.0),
            // Poison never kills.
            mob_effect::POISON if pulses(e.duration, 25, e.amplifier) && p.health > 1.0 => {
                self.damage(id, 1.0, Damage::Magic);
            }
            mob_effect::WITHER if pulses(e.duration, 40, e.amplifier) => {
                self.damage(id, 1.0, Damage::Wither);
            }
            mob_effect::HUNGER => {
                p.exhaustion += 0.005 * (f32::from(e.amplifier) + 1.0);
            }
            _ => {}
        }
    }

    /// The `/effect` command, on the player who runs it.
    pub(super) fn effect_command(&mut self, id: i32, args: &[&str]) -> String {
        const USAGE: &str =
            "Usage: /effect give <effect> [seconds] [amplifier] or /effect clear [effect]";
        match args {
            ["give", effect, rest @ ..] => {
                let Some(effect_id) = effect_id(effect) else {
                    return format!("Unknown effect: {effect}");
                };
                let duration = match rest.first() {
                    Some(&"infinite") => INFINITE,
                    Some(s) => match s.parse::<i32>() {
                        Ok(seconds) if seconds > 0 => seconds.saturating_mul(20),
                        _ => return USAGE.to_owned(),
                    },
                    None => 30 * 20,
                };
                let amplifier = rest.get(1).and_then(|a| a.parse::<u8>().ok()).unwrap_or(0);
                self.add_effect(
                    id,
                    Effect {
                        id: effect_id,
                        duration,
                        amplifier,
                    },
                );
                format!(
                    "Applied effect {}",
                    effect_name(effect_id).unwrap_or(effect)
                )
            }
            ["clear"] => format!("Removed {} effects", self.clear_effects(id)),
            ["clear", effect] => match effect_id(effect) {
                Some(effect_id) if self.remove_effect(id, effect_id) => {
                    format!(
                        "Removed effect {}",
                        effect_name(effect_id).unwrap_or(effect)
                    )
                }
                Some(_) => "You do not have that effect".to_owned(),
                None => format!("Unknown effect: {effect}"),
            },
            _ => USAGE.to_owned(),
        }
    }
}

/// Effects in the vanilla playerdata form (`active_effects`).
pub fn effects_to_nbt(effects: &[ActiveEffect]) -> Tag {
    Tag::List(
        effects
            .iter()
            .filter_map(|e| {
                Some(Tag::Compound(
                    Compound::new()
                        .with("id", effect_name(e.id)?)
                        .with("amplifier", e.amplifier as i8)
                        .with("duration", e.duration)
                        .with("show_particles", true)
                        .with("show_icon", true),
                ))
            })
            .collect(),
    )
}

pub fn effects_from_nbt(tag: Option<&Tag>) -> Vec<ActiveEffect> {
    let Some(Tag::List(list)) = tag else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| match t {
            Tag::Compound(c) => Some(ActiveEffect {
                id: effect_id(c.str("id")?)?,
                amplifier: c.int("amplifier").unwrap_or(0).clamp(0, 255) as u8,
                duration: c.int("duration").unwrap_or(0) as i32,
            }),
            _ => None,
        })
        .filter(|e| e.duration > 0 || e.duration == INFINITE)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_are_looked_up_by_name() {
        assert_eq!(effect_id("speed"), Some(mob_effect::SPEED));
        assert_eq!(effect_id("minecraft:poison"), Some(mob_effect::POISON));
        assert_eq!(
            effect_name(mob_effect::REGENERATION),
            Some("minecraft:regeneration")
        );
        assert_eq!(effect_id("nonsense"), None);
    }

    #[test]
    fn effects_pulse_faster_at_higher_levels() {
        assert!(pulses(100, 50, 0));
        assert!(!pulses(75, 50, 0));
        assert!(pulses(75, 50, 1)); // every 25 ticks at level II
    }

    #[test]
    fn effects_survive_saving() {
        let effects = vec![ActiveEffect {
            id: mob_effect::SPEED,
            amplifier: 1,
            duration: 400,
        }];
        assert_eq!(effects_from_nbt(Some(&effects_to_nbt(&effects))), effects);
    }

    #[test]
    fn mob_effects_can_be_removed_and_cleared() {
        let dir = std::env::temp_dir().join(format!("levitas-effects-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let id = next_entity_id();
        let mut cow = super::super::mobs::Mob::new(
            super::super::mobs::MobKind::Cow,
            [0.0, 0.0, 0.0],
            &mut w.rng,
        );
        cow.effects.push(ActiveEffect {
            id: mob_effect::SPEED,
            amplifier: 0,
            duration: 100,
        });
        cow.effects.push(ActiveEffect {
            id: mob_effect::STRENGTH,
            amplifier: 0,
            duration: 100,
        });
        w.mobs.insert(id, cow);

        assert!(w.remove_mob_effect(id, mob_effect::SPEED));
        assert_eq!(w.mobs[&id].effects.len(), 1);
        assert_eq!(w.clear_mob_effects(id), 1);
        assert!(w.mobs[&id].effects.is_empty());
    }
}
