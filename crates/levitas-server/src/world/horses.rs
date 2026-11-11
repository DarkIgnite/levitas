//! Horses and their kin: horses, donkeys, mules, skeleton and zombie horses, llamas and
//! camels. Taming by riding, temper, feeding, saddles, riding and steering, breeding.
//!
//! Behaviour follows the Minecraft Wiki: an untamed horse throws off a rider unless a
//! roll under its temper (0 to 100) tames it; each failed try and some foods raise the
//! temper. A tamed adult takes a saddle (shears take it off again) and its rider steers
//! it: the rider's client moves it, as with a boat, using the speed and jump strength
//! the horse was born with (horses 0.1125 to 0.3375 speed and 0.4 to 1.0 jump, donkeys
//! and mules 0.175 and 0.5, health 15 to 30). Tamed horses and donkeys breed on golden
//! carrots and golden apples; a horse and a donkey make a mule, which cannot breed.

use levitas_nbt::{Compound, Tag};

use super::mobs::{Mob, MobKind};
use super::*;

/// Entity data: the horse flags byte (0x02 tame, 0x08 bred, 0x20 rearing), a horse's
/// variant (colour + markings * 256), a llama's strength and variant.
const DATA_HORSE_FLAGS: u8 = 17;
const DATA_HORSE_VARIANT: u8 = 18;
const DATA_LLAMA_STRENGTH: u8 = 19;
const DATA_LLAMA_VARIANT: u8 = 20;
const FLAG_TAME: u8 = 0x02;
const FLAG_REARING: u8 = 0x20;
const SERIALIZER_BYTE: i32 = 0;
const SERIALIZER_INT: i32 = 1;
/// Equipment slot of a saddle.
const SLOT_SADDLE: u8 = 7;
const ENTITY_EVENT_TAMED: i8 = 7;
const ENTITY_EVENT_SMOKE: i8 = 6;
const MAX_TEMPER: u32 = 100;
/// The farthest the rider's client may move its horse in one packet.
const MAX_MOVE: f64 = 10.0;

/// What a horse of some kind is born with, and how it is doing.
#[derive(Debug, Clone)]
pub struct Horse {
    pub saddled: bool,
    pub temper: u32,
    /// The player on its back.
    pub rider: Option<i32>,
    /// Movement speed and jump strength attributes, and full health.
    pub speed: f64,
    pub jump: f64,
    pub max_health: f32,
    /// A horse's markings (0 to 4; its colour is `Mob::variant`), a llama's strength.
    pub markings: u8,
    /// Ticks an untamed horse has been ridden this time.
    pub ridden_for: u32,
}

/// Whether `kind` is a horse of some kind.
pub fn is_horse(kind: MobKind) -> bool {
    matches!(
        kind,
        MobKind::Horse
            | MobKind::Donkey
            | MobKind::Mule
            | MobKind::SkeletonHorse
            | MobKind::ZombieHorse
            | MobKind::Llama
            | MobKind::Camel
            | MobKind::CamelHusk
    )
}

/// Whether it has to be tamed before it lets a rider stay on (camels and skeleton
/// horses never need it).
fn needs_taming(kind: MobKind) -> bool {
    matches!(
        kind,
        MobKind::Horse | MobKind::Donkey | MobKind::Mule | MobKind::ZombieHorse | MobKind::Llama
    )
}

/// Whether it can wear a saddle (llamas wear carpets instead).
fn takes_saddle(kind: MobKind) -> bool {
    is_horse(kind) && kind != MobKind::Llama
}

/// A new horse's stats, rolled as vanilla rolls them.
pub fn roll(kind: MobKind, rng: &mut Rng) -> Horse {
    let health = 15.0 + rng.below(8) as f32 + rng.below(9) as f32;
    let (speed, jump, max_health) = match kind {
        MobKind::Horse => (
            (0.45 + rng.f64() * 0.3 + rng.f64() * 0.3 + rng.f64() * 0.3) * 0.25,
            0.4 + rng.f64() * 0.2 + rng.f64() * 0.2 + rng.f64() * 0.2,
            health,
        ),
        MobKind::Donkey | MobKind::Mule | MobKind::Llama => (0.175, 0.5, health),
        MobKind::SkeletonHorse => (0.2, 0.4 + rng.f64() * 0.6, 15.0),
        MobKind::ZombieHorse => (0.2, 0.4 + rng.f64() * 0.6, 25.0),
        _ => (0.09, 0.42, 32.0), // camels
    };
    Horse {
        saddled: false,
        temper: 0,
        rider: None,
        speed,
        jump,
        max_health,
        markings: match kind {
            MobKind::Horse => rng.below(5) as u8,
            // A llama's strength: 1 to 5, mostly low.
            MobKind::Llama => {
                let top = rng.below(5) + 1;
                1 + rng.below(top) as u8
            }
            _ => 0,
        },
        ridden_for: 0,
    }
}

/// A new horse's colour (`Mob::variant`): seven coats for horses, four for llamas.
pub fn roll_variant(kind: MobKind, rng: &mut Rng) -> u8 {
    match kind {
        MobKind::Horse => rng.below(7) as u8,
        MobKind::Llama => rng.below(4) as u8,
        _ => 0,
    }
}

/// How much a food heals a horse and raises its temper, and whether it can make a
/// tamed adult breed.
fn food_value(kind: MobKind, item: &str) -> Option<(f32, u32, bool)> {
    if kind == MobKind::Llama {
        return match item {
            "minecraft:wheat" => Some((2.0, 0, false)),
            "minecraft:hay_block" => Some((10.0, 0, true)),
            _ => None,
        };
    }
    if matches!(kind, MobKind::Camel | MobKind::CamelHusk) {
        return (levitas_registry::item_has_tag(item, "minecraft:camel_food"))
            .then_some((2.0, 0, true));
    }
    if matches!(kind, MobKind::SkeletonHorse | MobKind::Mule)
        && item.starts_with("minecraft:golden")
    {
        return Some((4.0, 5, false));
    }
    match item {
        "minecraft:sugar" => Some((1.0, 3, false)),
        "minecraft:wheat" => Some((2.0, 3, false)),
        "minecraft:apple" => Some((3.0, 3, false)),
        "minecraft:hay_block" => Some((20.0, 0, false)),
        "minecraft:golden_carrot" => Some((4.0, 5, true)),
        "minecraft:golden_apple" | "minecraft:enchanted_golden_apple" => Some((10.0, 10, true)),
        _ => None,
    }
}

/// The entity data of a horse: its flags and variant.
pub fn horse_data(id: i32, mob: &Mob) -> Writer {
    let mut flags = 0;
    if mob.owner.is_some() || !needs_taming(mob.kind) {
        flags |= FLAG_TAME;
    }
    if mob.panic > 0 && mob.kind == MobKind::Horse {
        flags |= FLAG_REARING;
    }
    let mut packet = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_HORSE_FLAGS)
        .varint(SERIALIZER_BYTE)
        .u8(flags);
    let markings = mob.horse.as_ref().map_or(0, |h| h.markings);
    match mob.kind {
        MobKind::Horse => {
            packet = packet
                .u8(DATA_HORSE_VARIANT)
                .varint(SERIALIZER_INT)
                .varint(i32::from(mob.variant) | i32::from(markings) << 8);
        }
        MobKind::Llama => {
            packet = packet
                .u8(DATA_LLAMA_STRENGTH)
                .varint(SERIALIZER_INT)
                .varint(i32::from(markings))
                .u8(DATA_LLAMA_VARIANT)
                .varint(SERIALIZER_INT)
                .varint(i32::from(mob.variant));
        }
        _ => {}
    }
    packet.u8(0xFF)
}

/// The saddle a horse wears, or none.
pub fn saddle_packet(id: i32, saddled: bool) -> Writer {
    let packet = Writer::packet(clientbound::SET_EQUIPMENT)
        .varint(id)
        .u8(SLOT_SADDLE);
    let saddle = ItemInfo::id_of("minecraft:saddle")
        .filter(|_| saddled)
        .map_or(Stack::EMPTY, |item| Stack::new(item, 1));
    crate::inventory::write_stack(packet, saddle)
}

/// The speed and jump strength the rider's client moves the horse with, and its health.
pub fn attributes_packet(id: i32, horse: &Horse) -> Writer {
    use levitas_registry::attribute;
    let mut packet = Writer::packet(clientbound::UPDATE_ATTRIBUTES)
        .varint(id)
        .varint(3);
    for (attr, value) in [
        (attribute::MOVEMENT_SPEED, horse.speed),
        (attribute::JUMP_STRENGTH, horse.jump),
        (attribute::MAX_HEALTH, f64::from(horse.max_health)),
    ] {
        packet = packet.varint(attr).f64(value).varint(0);
    }
    packet
}

/// Who rides a horse.
pub fn passengers(id: i32, rider: Option<i32>) -> Writer {
    let packet = Writer::packet(clientbound::SET_PASSENGERS).varint(id);
    match rider {
        Some(r) => packet.varint(1).varint(r),
        None => packet.varint(0),
    }
}

/// The packets showing a horse as it is, after it spawned for someone.
pub fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let Some(horse) = &mob.horse else {
        return Vec::new();
    };
    let mut out = vec![horse_data(id, mob), attributes_packet(id, horse)];
    if horse.saddled {
        out.push(saddle_packet(id, true));
    }
    if horse.rider.is_some() {
        out.push(passengers(id, horse.rider));
    }
    out
}

/// Saves a horse's own tags.
pub fn write_nbt(mob: &Mob, kept: Option<&Compound>, nbt: &mut Compound) {
    let Some(horse) = &mob.horse else {
        return;
    };
    nbt.insert("Tame", i8::from(mob.owner.is_some()));
    nbt.insert("Temper", horse.temper as i32);
    match mob.kind {
        MobKind::Horse => nbt.insert(
            "Variant",
            i32::from(mob.variant) | i32::from(horse.markings) << 8,
        ),
        MobKind::Llama => {
            nbt.insert("Variant", i32::from(mob.variant));
            nbt.insert("Strength", i32::from(horse.markings));
        }
        _ => {}
    }
    if let Some(owner) = mob.owner {
        let owner = [
            (owner >> 96) as i32,
            (owner >> 64) as i32,
            (owner >> 32) as i32,
            owner as i32,
        ];
        nbt.insert("Owner", owner.to_vec());
    }
    // The saddle sits among whatever else it wears (horse armour, a llama's carpet).
    let mut equipment = kept
        .and_then(|k| k.get("equipment"))
        .and_then(Tag::as_compound)
        .cloned()
        .unwrap_or_default();
    equipment.remove("saddle");
    if horse.saddled {
        equipment.insert(
            "saddle",
            Compound::new()
                .with("id", "minecraft:saddle")
                .with("count", 1i32),
        );
    }
    if !equipment.is_empty() {
        nbt.insert("equipment", equipment);
    }
    let attribute =
        |id: &str, base: f64| Tag::Compound(Compound::new().with("id", id).with("base", base));
    nbt.insert(
        "attributes",
        vec![
            attribute("minecraft:movement_speed", horse.speed),
            attribute("minecraft:jump_strength", horse.jump),
            attribute("minecraft:max_health", f64::from(horse.max_health)),
        ],
    );
}

/// Reads a horse's own tags back.
pub fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let Some(horse) = mob.horse.as_mut() else {
        return;
    };
    horse.temper = nbt
        .int("Temper")
        .unwrap_or(0)
        .clamp(0, i64::from(MAX_TEMPER)) as u32;
    if let Some(variant) = nbt.int("Variant") {
        mob.variant = (variant & 0xFF) as u8;
        if mob.kind == MobKind::Horse {
            horse.markings = ((variant >> 8) & 0xFF).min(4) as u8;
        }
    }
    if mob.kind == MobKind::Llama {
        horse.markings = nbt.int("Strength").unwrap_or(1).clamp(1, 5) as u8;
        mob.variant = mob.variant.min(3);
    }
    let tame = nbt.int("Tame").unwrap_or(0) != 0;
    if let Some(&[a, b, c, d]) = nbt.int_array("Owner") {
        mob.owner = Some(
            (u128::from(a as u32) << 96)
                | (u128::from(b as u32) << 64)
                | (u128::from(c as u32) << 32)
                | u128::from(d as u32),
        );
    } else if tame {
        // Tamed by someone, unknown who.
        mob.owner = Some(0);
    }
    horse.saddled = nbt
        .get("equipment")
        .and_then(Tag::as_compound)
        .is_some_and(|e| e.get("saddle").is_some())
        // Before 1.21.5 the saddle had its own tag.
        || nbt.get("SaddleItem").is_some();
    for attr in nbt.list("attributes").into_iter().flatten() {
        let Some(a) = attr.as_compound() else {
            continue;
        };
        let base = a.get("base").and_then(Tag::as_f64);
        match (a.str("id"), base) {
            (Some("minecraft:movement_speed"), Some(v)) => horse.speed = v,
            (Some("minecraft:jump_strength"), Some(v)) => horse.jump = v,
            (Some("minecraft:max_health"), Some(v)) => horse.max_health = v as f32,
            _ => {}
        }
    }
}

impl World {
    /// Sends a packet about mob `target` to those who see it.
    fn show_mob(&mut self, target: i32, packet: Writer) {
        if let Some(m) = self.mobs.remove(&target) {
            self.send_to_viewers(&m, packet);
            self.mobs.insert(target, m);
        }
    }

    /// A player uses a horse: feeding it, saddling or unsaddling it, or getting on.
    /// Returns whether the horse took it.
    pub(super) fn use_on_horse(&mut self, id: i32, target: i32, slot: usize, held: &str) -> bool {
        let Some(mob) = self.mobs.get(&target) else {
            return false;
        };
        let kind = mob.kind;
        let adult = mob.age >= 0;
        let tame = mob.owner.is_some() || !needs_taming(kind);
        let Some(horse) = mob.horse.as_ref() else {
            return false;
        };
        let (saddled, ridden) = (horse.saddled, horse.rider.is_some());
        let survival = self.players.get(&id).is_some_and(|p| p.survival());
        if let Some((heal, temper, breeds)) = food_value(kind, held) {
            let mob = self.mobs.get_mut(&target).expect("checked");
            let max = mob.max_health();
            let hurt = mob.health < max;
            let wants_love = breeds && tame && adult && mob.love == 0 && mob.age == 0;
            let horse = mob.horse.as_mut().expect("checked");
            let calmer = !tame && horse.temper < MAX_TEMPER && temper > 0;
            if !hurt && !wants_love && !calmer && adult {
                return false;
            }
            mob.health = (mob.health + heal).min(max);
            if calmer {
                horse.temper = (horse.temper + temper).min(MAX_TEMPER);
            }
            if wants_love && kind != MobKind::Mule {
                mob.love = 600;
                mob.helped_by = Some(id);
                self.show_mob(target, entity_event(target, 18));
            }
            if !adult {
                let mob = self.mobs.get_mut(&target).expect("checked");
                mob.age += -mob.age / 10;
            }
            let sound = format!("minecraft:entity.{}.eat", kind.sound());
            let pos = self.mobs[&target].pos;
            self.play_sound_at(&sound, pos, 1.0, 1.0, None);
            if survival {
                let p = self.players.get_mut(&id).expect("player exists");
                p.inventory.take(slot, 1);
                let update = p.inventory.set_slot_packet(slot);
                self.send(id, update);
            }
            return true;
        }
        if held == "minecraft:saddle" && takes_saddle(kind) && tame && adult && !saddled {
            let mob = self.mobs.get_mut(&target).expect("checked");
            mob.horse.as_mut().expect("checked").saddled = true;
            let pos = mob.pos;
            self.show_mob(target, saddle_packet(target, true));
            self.play_sound_at("minecraft:entity.horse.saddle", pos, 1.0, 1.0, None);
            if survival {
                let p = self.players.get_mut(&id).expect("player exists");
                p.inventory.take(slot, 1);
                let update = p.inventory.set_slot_packet(slot);
                self.send(id, update);
            }
            return true;
        }
        if held == "minecraft:shears" && saddled && !ridden {
            let mob = self.mobs.get_mut(&target).expect("checked");
            mob.horse.as_mut().expect("checked").saddled = false;
            let pos = mob.pos;
            self.show_mob(target, saddle_packet(target, false));
            self.play_sound_at("minecraft:item.saddle.unequip", pos, 1.0, 1.0, None);
            if let Some(item) = ItemInfo::id_of("minecraft:saddle") {
                self.scatter_item([pos[0], pos[1] + 1.0, pos[2]], Stack::new(item, 1));
            }
            if survival {
                self.damage_item(id, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
            }
            return true;
        }
        if !adult || ridden {
            return adult;
        }
        self.mount_horse(id, target);
        true
    }

    /// Puts a player on a horse's back.
    fn mount_horse(&mut self, id: i32, target: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        if !p.alive || p.sneaking || p.riding.is_some() {
            return;
        }
        let Some(mob) = self.mobs.get_mut(&target) else {
            return;
        };
        let Some(horse) = mob.horse.as_mut() else {
            return;
        };
        horse.rider = Some(id);
        horse.ridden_for = 0;
        mob.target = None;
        mob.path.clear();
        let at = mob.pos;
        self.players.get_mut(&id).expect("checked").riding = Some(target);
        self.broadcast(passengers(target, Some(id)), None);
        self.move_player(id, Some(at), None, true);
    }

    /// A rider gets off their horse, beside it. Returns whether they were on one.
    pub(super) fn dismount_horse(&mut self, id: i32, target: i32) -> bool {
        let Some(mob) = self.mobs.get_mut(&target) else {
            return false;
        };
        let Some(horse) = mob.horse.as_mut() else {
            return false;
        };
        if horse.rider == Some(id) {
            horse.rider = None;
        }
        let yaw = f64::from(mob.yaw).to_radians();
        let at = [
            mob.pos[0] + yaw.cos(),
            mob.pos[1] + 0.5,
            mob.pos[2] + yaw.sin(),
        ];
        self.broadcast(passengers(target, None), None);
        if self.players.get(&id).is_some_and(|p| p.alive) {
            self.teleport(id, at);
        }
        true
    }

    /// Whether the rider's client steers this horse: it wears a saddle and is tamed.
    pub(super) fn horse_steered(mob: &Mob) -> bool {
        mob.horse
            .as_ref()
            .is_some_and(|h| h.rider.is_some() && h.saddled)
            && (mob.owner.is_some() || !needs_taming(mob.kind))
    }

    /// Where the rider's client moved their horse.
    pub(super) fn horse_moved(&mut self, id: i32, target: i32, pos: [f64; 3], yaw: f32) {
        let Some(mob) = self.mobs.get_mut(&target) else {
            return;
        };
        if !Self::horse_steered(mob) || mob.horse.as_ref().and_then(|h| h.rider) != Some(id) {
            return;
        }
        let d: f64 = (0..3).map(|i| (pos[i] - mob.pos[i]).powi(2)).sum();
        if d > MAX_MOVE * MAX_MOVE || pos.iter().any(|v| !v.is_finite()) {
            let back = Writer::packet(clientbound::MOVE_VEHICLE)
                .f64(mob.pos[0])
                .f64(mob.pos[1])
                .f64(mob.pos[2])
                .f32(mob.yaw)
                .f32(0.0);
            self.send(id, back);
            return;
        }
        mob.pos = pos;
        mob.yaw = yaw;
        mob.head_yaw = yaw;
        self.move_player(id, Some(pos), None, true);
    }

    /// A horse's turn: an untamed one bucks its rider off or gives in, a rider who left
    /// is let go, and a horse nobody steers carries its rider along.
    pub(super) fn tick_horse(&mut self, id: i32, mob: &mut Mob) {
        let kind = mob.kind;
        let Some(rider) = mob.horse.as_ref().and_then(|h| h.rider) else {
            return;
        };
        if self
            .players
            .get(&rider)
            .is_none_or(|p| p.riding != Some(id) || !p.alive)
        {
            if let Some(h) = mob.horse.as_mut() {
                h.rider = None;
            }
            self.send_to_viewers(mob, passengers(id, None));
            return;
        }
        if needs_taming(kind) && mob.owner.is_none() {
            let horse = mob.horse.as_mut().expect("checked");
            horse.ridden_for += 1;
            if horse.ridden_for < 20 + self.rng.below(40) {
                return;
            }
            let temper = horse.temper;
            if self.rng.below(MAX_TEMPER) < temper {
                mob.owner = self.players.get(&rider).map(|p| p.profile.uuid);
                self.send_to_viewers(mob, entity_event(id, ENTITY_EVENT_TAMED));
                self.send_to_viewers(mob, horse_data(id, mob));
                self.advance(
                    rider,
                    super::advancements::Trigger::TamedAnimal(kind.entity_type()),
                );
            } else {
                horse.temper = (temper + 5).min(MAX_TEMPER);
                horse.rider = None;
                if let Some(p) = self.players.get_mut(&rider) {
                    p.riding = None;
                }
                self.send_to_viewers(mob, entity_event(id, ENTITY_EVENT_SMOKE));
                mob.panic = 20;
                self.send_to_viewers(mob, horse_data(id, mob));
                let sound = format!("minecraft:entity.{}.angry", kind.sound());
                self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
                self.send_to_viewers(mob, passengers(id, None));
                let yaw = f64::from(mob.yaw).to_radians();
                let at = [
                    mob.pos[0] + yaw.cos(),
                    mob.pos[1] + 0.5,
                    mob.pos[2] + yaw.sin(),
                ];
                self.teleport(rider, at);
            }
            return;
        }
        if !Self::horse_steered(mob) {
            // Wandering with its rider aboard.
            self.move_player(rider, Some(mob.pos), None, true);
        }
    }

    /// The kind of foal two horses make: a horse and a donkey make a mule.
    pub(super) fn foal_kind(a: MobKind, b: MobKind) -> Option<MobKind> {
        match (a, b) {
            (MobKind::Horse, MobKind::Donkey) | (MobKind::Donkey, MobKind::Horse) => {
                Some(MobKind::Mule)
            }
            (MobKind::Mule, _) | (_, MobKind::Mule) => None,
            (x, y) if x == y => Some(x),
            _ => None,
        }
    }

    /// A foal's stats: between its parents', with some of its own.
    pub(super) fn foal_stats(&mut self, foal: &mut Mob, a: &Horse, b: &Horse) {
        let fresh = roll(foal.kind, &mut self.rng);
        if let Some(h) = foal.horse.as_mut() {
            h.speed = (a.speed + b.speed + fresh.speed) / 3.0;
            h.jump = (a.jump + b.jump + fresh.jump) / 3.0;
            h.max_health = (a.max_health + b.max_health + fresh.max_health) / 3.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_rider() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-horse-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        for cx in -1..=1 {
            for cz in -1..=1 {
                w.chunk(cx, cz);
            }
        }
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 7,
            properties: Vec::new(),
        };
        w.players
            .insert(alice, Player::new(profile, out, [2.5, -60.0, 2.5]));
        (w, alice)
    }

    #[test]
    fn horses_are_tamed_saddled_ridden_and_saved() {
        let (mut w, alice) = world_with_rider();
        let horse = next_entity_id();
        let mut mob = Mob::new(MobKind::Horse, [3.5, -60.0, 2.5], &mut w.rng);
        mob.age = 0;
        w.mobs.insert(horse, mob);
        // An untamed horse will not take a saddle.
        assert!(w.use_on_horse(alice, horse, 0, "minecraft:empty"));
        assert_eq!(w.players[&alice].riding, Some(horse));
        assert_eq!(w.mobs[&horse].horse.as_ref().unwrap().rider, Some(alice));
        // With its temper at the top, the next try tames it.
        w.mobs
            .get_mut(&horse)
            .unwrap()
            .horse
            .as_mut()
            .unwrap()
            .temper = MAX_TEMPER;
        for _ in 0..80 {
            let mut m = w.mobs.remove(&horse).unwrap();
            w.tick_horse(horse, &mut m);
            w.mobs.insert(horse, m);
        }
        assert_eq!(w.mobs[&horse].owner, Some(7), "tamed by Alice");
        w.get_off(alice);
        assert!(w.players[&alice].riding.is_none());
        assert!(w.mobs[&horse].horse.as_ref().unwrap().rider.is_none());

        // Saddled, it is steered by its rider's client.
        assert!(w.use_on_horse(alice, horse, 0, "minecraft:saddle"));
        assert!(w.mobs[&horse].horse.as_ref().unwrap().saddled);
        w.use_on_horse(alice, horse, 0, "minecraft:empty");
        assert!(World::horse_steered(&w.mobs[&horse]));
        w.horse_moved(alice, horse, [8.5, -60.0, 2.5], 90.0);
        assert_eq!(w.mobs[&horse].pos, [8.5, -60.0, 2.5]);
        assert_eq!(w.players[&alice].pos, [8.5, -60.0, 2.5]);
        // Too far in one go is refused.
        w.horse_moved(alice, horse, [80.5, -60.0, 2.5], 90.0);
        assert_eq!(w.mobs[&horse].pos, [8.5, -60.0, 2.5]);
        w.get_off(alice);

        // Saved and loaded as vanilla stores it.
        let nbt = w.mobs[&horse].to_nbt();
        assert_eq!(nbt.int("Tame"), Some(1));
        assert!(nbt
            .get("equipment")
            .and_then(Tag::as_compound)
            .is_some_and(|e| e.get("saddle").is_some()));
        let back = Mob::read(&nbt, &mut w.rng).unwrap();
        let (h, b) = (
            w.mobs[&horse].horse.as_ref().unwrap(),
            back.horse.as_ref().unwrap(),
        );
        assert!(b.saddled && back.owner == Some(7));
        assert_eq!(
            (b.speed, b.jump, b.max_health),
            (h.speed, h.jump, h.max_health)
        );
        assert_eq!(back.variant, w.mobs[&horse].variant);

        // Shears take the saddle off again.
        assert!(w.use_on_horse(alice, horse, 0, "minecraft:shears"));
        assert!(!w.mobs[&horse].horse.as_ref().unwrap().saddled);
    }

    #[test]
    fn an_untamed_horse_bucks_its_rider_and_grows_calmer() {
        let (mut w, alice) = world_with_rider();
        let horse = next_entity_id();
        w.mobs.insert(
            horse,
            Mob::new(MobKind::Horse, [3.5, -60.0, 2.5], &mut w.rng),
        );
        w.mobs.get_mut(&horse).unwrap().age = 0;
        w.use_on_horse(alice, horse, 0, "minecraft:empty");
        for _ in 0..80 {
            let mut m = w.mobs.remove(&horse).unwrap();
            w.tick_horse(horse, &mut m);
            w.mobs.insert(horse, m);
        }
        let h = w.mobs[&horse].horse.as_ref().unwrap();
        assert!(
            h.rider.is_none() && w.players[&alice].riding.is_none(),
            "thrown off"
        );
        assert_eq!(h.temper, 5);
        assert!(w.mobs[&horse].owner.is_none());
    }

    #[test]
    fn horse_stats_fall_in_vanilla_ranges() {
        let mut rng = Rng::new(7);
        for _ in 0..200 {
            let h = roll(MobKind::Horse, &mut rng);
            assert!((0.1125..=0.3375).contains(&h.speed), "{}", h.speed);
            assert!((0.4..=1.0).contains(&h.jump));
            assert!((15.0..=30.0).contains(&h.max_health));
            assert!(h.markings <= 4);
        }
        let d = roll(MobKind::Donkey, &mut rng);
        assert_eq!((d.speed, d.jump), (0.175, 0.5));
        assert!((1..=5).contains(&roll(MobKind::Llama, &mut rng).markings));
    }

    #[test]
    fn horses_and_donkeys_make_mules_which_do_not_breed() {
        assert_eq!(
            World::foal_kind(MobKind::Horse, MobKind::Donkey),
            Some(MobKind::Mule)
        );
        assert_eq!(
            World::foal_kind(MobKind::Horse, MobKind::Horse),
            Some(MobKind::Horse)
        );
        assert_eq!(World::foal_kind(MobKind::Mule, MobKind::Mule), None);
        assert_eq!(World::foal_kind(MobKind::Horse, MobKind::Llama), None);
    }

    #[test]
    fn foods_heal_calm_and_some_make_tame_horses_breed() {
        assert_eq!(
            food_value(MobKind::Horse, "minecraft:sugar"),
            Some((1.0, 3, false))
        );
        assert_eq!(
            food_value(MobKind::Horse, "minecraft:golden_carrot"),
            Some((4.0, 5, true))
        );
        assert_eq!(
            food_value(MobKind::Llama, "minecraft:hay_block"),
            Some((10.0, 0, true))
        );
        assert!(food_value(MobKind::Horse, "minecraft:dirt").is_none());
    }
}
