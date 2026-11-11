//! Foxes, parrots, turtles, axolotls, and mobs caught in buckets.
//!
//! Behaviour follows the Minecraft Wiki:
//! - Foxes come red, or white in snowy biomes; they sleep by day, keep away from
//!   players who are not trusted, and at night hunt chickens, rabbits and fish.
//! - Parrots come in five colours; seeds tame one in ten tries; a tamed parrot sits or
//!   follows its owner, flying, and teleports to them when far; a cookie kills it.
//! - Turtles remember the beach they hatched on. A turtle that bred (on seagrass)
//!   carries an egg home and lays one to four on sand; eggs crack on random ticks,
//!   nearly always at night, and hatch after the third into baby turtles, which drop a
//!   scute when they grow up.
//! - Axolotls come in four colours (blue only by breeding, one in 1200); in water they
//!   hunt fish, squid, drowned and guardians; out of water they dry out after five
//!   minutes (rain keeps them wet). They breed on tropical fish buckets, which give back
//!   a water bucket.
//! - A water bucket scoops up cod, salmon, pufferfish, tropical fish, axolotls and
//!   tadpoles; emptying the filled bucket pours the water and lets the mob out.

use super::mobs::{Mob, MobKind};
use super::*;
use crate::blocks::{prop, with};

const DATA_FOX_TYPE: u8 = 17;
const DATA_FOX_FLAGS: u8 = 18;
const FOX_SLEEPING: u8 = 0x20;
const DATA_PARROT_VARIANT: u8 = 19;
const DATA_TURTLE_HAS_EGG: u8 = 17;
const DATA_AXOLOTL_VARIANT: u8 = 17;
const SERIALIZER_BYTE: i32 = 0;
const SERIALIZER_INT: i32 = 1;
const SERIALIZER_BOOL: i32 = 8;
/// Ticks out of water before an axolotl starts drying out.
const AXOLOTL_MOIST: u32 = 6000;
const HUNT_RANGE: f64 = 8.0;

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// The bucket a mob is caught in, if it can be.
pub fn bucket_of(kind: MobKind) -> Option<&'static str> {
    Some(match kind {
        MobKind::Cod => "minecraft:cod_bucket",
        MobKind::Salmon => "minecraft:salmon_bucket",
        MobKind::Pufferfish => "minecraft:pufferfish_bucket",
        MobKind::TropicalFish => "minecraft:tropical_fish_bucket",
        MobKind::Axolotl => "minecraft:axolotl_bucket",
        MobKind::Tadpole => "minecraft:tadpole_bucket",
        _ => return None,
    })
}

/// The mob a filled bucket holds.
pub fn mob_in_bucket(item: &str) -> Option<MobKind> {
    [
        MobKind::Cod,
        MobKind::Salmon,
        MobKind::Pufferfish,
        MobKind::TropicalFish,
        MobKind::Axolotl,
        MobKind::Tadpole,
    ]
    .into_iter()
    .find(|&k| bucket_of(k) == Some(item))
}

/// Whether it swims when in water although it walks on land.
pub fn amphibious(kind: MobKind) -> bool {
    matches!(kind, MobKind::Axolotl | MobKind::Turtle | MobKind::Frog)
}

/// A new mob's colour or coat, for the kinds this module looks after.
pub fn roll_variant(kind: MobKind, rng: &mut Rng) -> Option<u8> {
    Some(match kind {
        MobKind::Parrot => rng.below(5) as u8,
        MobKind::Axolotl => rng.below(4) as u8,
        // Tropical fish: one of the 22 common patterns and colours, packed as vanilla.
        MobKind::TropicalFish => rng.below(255) as u8,
        _ => return None,
    })
}

/// The entity data showing such a mob as it is, after it spawned for someone.
pub fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let packet = || Writer::packet(clientbound::SET_ENTITY_DATA).varint(id);
    match mob.kind {
        MobKind::Fox => vec![fox_data(id, mob)],
        MobKind::Parrot => vec![packet()
            .u8(DATA_PARROT_VARIANT)
            .varint(SERIALIZER_INT)
            .varint(i32::from(mob.variant))
            .u8(0xFF)],
        MobKind::Axolotl => vec![packet()
            .u8(DATA_AXOLOTL_VARIANT)
            .varint(SERIALIZER_INT)
            .varint(i32::from(mob.variant))
            .u8(0xFF)],
        MobKind::Turtle if mob.egg => vec![turtle_data(id, true)],
        _ => Vec::new(),
    }
}

fn fox_data(id: i32, mob: &Mob) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_FOX_TYPE)
        .varint(SERIALIZER_INT)
        .varint(i32::from(mob.variant))
        .u8(DATA_FOX_FLAGS)
        .varint(SERIALIZER_BYTE)
        .u8(if mob.sitting { FOX_SLEEPING } else { 0 })
        .u8(0xFF)
}

fn turtle_data(id: i32, egg: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_TURTLE_HAS_EGG)
        .varint(SERIALIZER_BOOL)
        .bool(egg)
        .u8(0xFF)
}

/// Saves these mobs' own tags.
pub fn write_nbt(mob: &Mob, nbt: &mut levitas_nbt::Compound) {
    match mob.kind {
        MobKind::Fox => {
            nbt.insert("Type", if mob.variant == 1 { "snow" } else { "red" });
            nbt.insert("Sleeping", mob.sitting);
        }
        MobKind::Parrot | MobKind::Axolotl | MobKind::TropicalFish => {
            nbt.insert("Variant", i32::from(mob.variant));
        }
        MobKind::Turtle => nbt.insert("HasEgg", mob.egg),
        _ => {}
    }
    if let Some(h) = mob.home {
        nbt.insert("home_pos", vec![h.x, h.y, h.z]);
    }
    if mob.kind == MobKind::Parrot {
        nbt.insert("Sitting", mob.sitting);
    }
}

/// Reads these mobs' own tags back.
pub fn read_nbt(mob: &mut Mob, nbt: &levitas_nbt::Compound) {
    match mob.kind {
        MobKind::Fox => {
            mob.variant = u8::from(nbt.str("Type") == Some("snow"));
            mob.sitting = nbt.int("Sleeping").unwrap_or(0) != 0;
        }
        MobKind::Parrot | MobKind::Axolotl | MobKind::TropicalFish => {
            if let Some(v) = nbt.int("Variant") {
                mob.variant = (v & 0xFF) as u8;
            }
            if mob.kind == MobKind::Parrot {
                mob.sitting = nbt.int("Sitting").unwrap_or(0) != 0;
                if let Some(&[a, b, c, d]) = nbt.int_array("Owner") {
                    mob.owner = Some(
                        (u128::from(a as u32) << 96)
                            | (u128::from(b as u32) << 64)
                            | (u128::from(c as u32) << 32)
                            | u128::from(d as u32),
                    );
                }
            }
        }
        MobKind::Turtle => mob.egg = nbt.int("HasEgg").unwrap_or(0) != 0,
        _ => {}
    }
    if let Some(&[x, y, z]) = nbt.int_array("home_pos") {
        mob.home = Some(BlockPos::new(x, y, z));
    }
}

impl World {
    /// A player uses a water bucket on a mob that fits in one. Returns whether it did.
    pub(super) fn scoop_mob(&mut self, id: i32, target: i32, slot: usize) -> bool {
        let Some(mob) = self.mobs.get(&target) else {
            return false;
        };
        let Some(bucket) = bucket_of(mob.kind) else {
            return false;
        };
        let mob = self.mobs.remove(&target).expect("checked");
        self.send_to_viewers(&mob, items::remove_entity(target));
        self.play_sound_at("minecraft:item.bucket.fill_fish", mob.pos, 1.0, 1.0, None);
        self.exchange_held(id, slot, bucket);
        true
    }

    /// The mob a filled bucket held comes out where its water was poured.
    pub(super) fn release_from_bucket(&mut self, item: &str, pos: BlockPos) {
        let Some(kind) = mob_in_bucket(item) else {
            return;
        };
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.1,
            f64::from(pos.z) + 0.5,
        ];
        let mut mob = Mob::new(kind, at, &mut self.rng);
        mob.persistent = true;
        if kind == MobKind::Tadpole {
            mob.age = -24_000;
        }
        self.play_sound_at("minecraft:item.bucket.empty_fish", at, 1.0, 1.0, None);
        self.mobs.insert(next_entity_id(), mob);
    }

    /// The turn of a fox, parrot, turtle or axolotl. Returns whether it is busy (and
    /// should not wander).
    pub(super) fn tick_critter(&mut self, id: i32, mob: &mut Mob) -> bool {
        match mob.kind {
            MobKind::Fox => self.tick_fox(id, mob),
            MobKind::Parrot => self.tick_parrot(mob),
            MobKind::Turtle => self.tick_turtle(id, mob),
            MobKind::Axolotl => self.tick_axolotl(id, mob),
            _ => false,
        }
    }

    fn tick_fox(&mut self, id: i32, mob: &mut Mob) -> bool {
        let day = self.dimension.has_sky() && self.day_time.rem_euclid(24_000) < 12_000;
        let player_near = self
            .players
            .values()
            .any(|p| p.alive && p.survival() && distance(p.pos, mob.pos) < 6.0);
        let sleeps = day && !player_near && mob.panic == 0 && mob.hurt_ticks == 0;
        if sleeps != mob.sitting {
            mob.sitting = sleeps;
            self.send_to_viewers(mob, fox_data(id, mob));
        }
        if sleeps {
            mob.target = None;
            mob.path.clear();
            return true;
        }
        // At night: after chickens, rabbits and fish close by.
        if day {
            return false;
        }
        mob.attack_cooldown = mob.attack_cooldown.saturating_sub(1);
        let prey = self
            .mobs
            .iter()
            .filter(|(_, m)| {
                m.alive()
                    && matches!(
                        m.kind,
                        MobKind::Chicken | MobKind::Rabbit | MobKind::Cod | MobKind::Salmon
                    )
                    && distance(m.pos, mob.pos) <= HUNT_RANGE
            })
            .min_by(|a, b| distance(a.1.pos, mob.pos).total_cmp(&distance(b.1.pos, mob.pos)))
            .map(|(&mid, m)| (mid, m.pos));
        let Some((mid, at)) = prey else {
            return false;
        };
        mob.target = Some(([at[0], at[2]], 20));
        if distance(at, mob.pos) <= 1.5 && mob.attack_cooldown == 0 {
            mob.attack_cooldown = 20;
            self.hit_mob(mid, 2.0, None, [0.0, 0.0], "minecraft:mob_attack");
        }
        true
    }

    fn tick_parrot(&mut self, mob: &mut Mob) -> bool {
        let Some(owner) = mob.owner else {
            return false;
        };
        if mob.sitting {
            mob.fly_goal = Some(mob.pos);
            return true;
        }
        let Some(at) = self
            .players
            .values()
            .find(|p| p.profile.uuid == owner && p.alive)
            .map(|p| p.pos)
        else {
            return false;
        };
        let d = distance(at, mob.pos);
        if d > 12.0 {
            mob.pos = [at[0] + 0.5, at[1] + 1.0, at[2] + 0.5];
        } else if d > 3.0 {
            mob.fly_goal = Some([at[0], at[1] + 1.5, at[2]]);
            return true;
        }
        false
    }

    /// A player uses a parrot: seeds may tame it, its owner makes it sit or stand, a
    /// cookie kills it. Returns whether the parrot took it.
    pub(super) fn use_on_parrot(&mut self, id: i32, target: i32, slot: usize, held: &str) -> bool {
        let Some(mob) = self.mobs.get(&target) else {
            return false;
        };
        let uuid = self.players.get(&id).map(|p| p.profile.uuid);
        let survival = self.players.get(&id).is_some_and(|p| p.survival());
        let take = |w: &mut World| {
            if survival {
                let p = w.players.get_mut(&id).expect("player exists");
                p.inventory.take(slot, 1);
                let update = p.inventory.set_slot_packet(slot);
                w.send(id, update);
            }
        };
        if held == "minecraft:cookie" {
            take(self);
            let mut mob = self.mobs.remove(&target).expect("checked");
            self.hurt_mob(target, &mut mob, 1000.0, "minecraft:magic");
            self.mobs.insert(target, mob);
            return true;
        }
        if mob.owner.is_none() && levitas_registry::item_has_tag(held, "minecraft:parrot_food") {
            take(self);
            let tamed = self.rng.below(10) == 0;
            let mut mob = self.mobs.remove(&target).expect("checked");
            if tamed {
                mob.owner = uuid;
                mob.sitting = true;
                self.send_to_viewers(&mob, entity_event(target, 7));
                self.send_to_viewers(&mob, super::wolves::wolf_data(&mob, target));
                self.advance(
                    id,
                    super::advancements::Trigger::TamedAnimal(mob.kind.entity_type()),
                );
            } else {
                self.send_to_viewers(&mob, entity_event(target, 6));
            }
            self.mobs.insert(target, mob);
            return true;
        }
        if mob.owner.is_some() && mob.owner == uuid {
            let mut mob = self.mobs.remove(&target).expect("checked");
            mob.sitting = !mob.sitting;
            self.send_to_viewers(&mob, super::wolves::wolf_data(&mob, target));
            self.mobs.insert(target, mob);
            return true;
        }
        false
    }

    fn tick_turtle(&mut self, id: i32, mob: &mut Mob) -> bool {
        let here = BlockPos::new(
            mob.pos[0].floor() as i32,
            mob.pos[1].floor() as i32,
            mob.pos[2].floor() as i32,
        );
        let home = *mob.home.get_or_insert(here);
        if !mob.egg {
            return false;
        }
        let near_home = (here.x - home.x).abs() <= 2 && (here.z - home.z).abs() <= 2;
        let below = self.block(BlockPos::new(here.x, here.y - 1, here.z));
        if near_home && block_has_tag(below, "minecraft:sand") && self.block(here) == block::AIR {
            let eggs = 1 + self.rng.below(4);
            if let Some(egg) = BlockInfo::by_name("minecraft:turtle_egg") {
                let state = with(egg.default_state, "eggs", &eggs.to_string());
                self.set_block_updating(here, state);
            }
            self.play_sound_at("minecraft:entity.turtle.lay_egg", mob.pos, 1.0, 1.0, None);
            mob.egg = false;
            self.send_to_viewers(mob, turtle_data(id, false));
            return false;
        }
        mob.target = Some(([f64::from(home.x) + 0.5, f64::from(home.z) + 0.5], 40));
        true
    }

    /// A turtle that bred carries an egg instead of a baby appearing.
    pub(super) fn turtle_bred(&mut self, id: i32, mob: &mut Mob) {
        mob.egg = true;
        self.send_to_viewers(mob, turtle_data(id, true));
    }

    /// A turtle egg's random tick: it cracks (nearly always at night, otherwise one
    /// time in 500), and after the third crack the babies hatch.
    pub(super) fn turtle_egg_tick(&mut self, pos: BlockPos, state: u16) {
        let below = self.block(BlockPos::new(pos.x, pos.y - 1, pos.z));
        if !block_has_tag(below, "minecraft:sand") {
            return;
        }
        let time = self.day_time.rem_euclid(24_000);
        let night = (21_062..=21_905).contains(&time);
        if !night && self.rng.below(500) != 0 {
            return;
        }
        let hatch: u8 = prop(state, "hatch")
            .and_then(|h| h.parse().ok())
            .unwrap_or(0);
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        if hatch < 2 {
            self.set_block(pos, with(state, "hatch", &(hatch + 1).to_string()));
            self.play_sound_at("minecraft:entity.turtle.egg_crack", at, 0.7, 1.0, None);
            return;
        }
        let eggs: u8 = prop(state, "eggs")
            .and_then(|e| e.parse().ok())
            .unwrap_or(1);
        self.set_block_updating(pos, block::AIR);
        self.play_sound_at("minecraft:entity.turtle.egg_hatch", at, 0.7, 1.0, None);
        for _ in 0..eggs {
            let mut baby = Mob::new(MobKind::Turtle, at, &mut self.rng);
            baby.age = -24_000;
            baby.home = Some(pos);
            self.mobs.insert(next_entity_id(), baby);
        }
    }

    /// A baby turtle that grows up sheds a scute.
    pub(super) fn turtle_grew_up(&mut self, mob: &Mob) {
        if mob.kind == MobKind::Turtle {
            if let Some(scute) = ItemInfo::id_of("minecraft:turtle_scute") {
                self.scatter_item(mob.pos, Stack::new(scute, 1));
            }
        }
    }

    fn tick_axolotl(&mut self, id: i32, mob: &mut Mob) -> bool {
        let wet = self.in_water(mob.pos)
            || (self.weather.is_raining()
                && self
                    .light_at(BlockPos::new(
                        mob.pos[0].floor() as i32,
                        mob.pos[1].floor() as i32,
                        mob.pos[2].floor() as i32,
                    ))
                    .is_some_and(|(sky, _)| sky == 15));
        if wet {
            mob.converting = None;
        } else {
            let dry = mob.converting.get_or_insert(0);
            *dry += 1;
            if *dry > AXOLOTL_MOIST && self.tick.is_multiple_of(20) {
                self.hurt_mob(id, mob, 1.0, "minecraft:dry_out");
            }
        }
        if !self.in_water(mob.pos) {
            return false;
        }
        mob.attack_cooldown = mob.attack_cooldown.saturating_sub(1);
        let prey = self
            .mobs
            .iter()
            .filter(|(_, m)| {
                m.alive()
                    && matches!(
                        m.kind,
                        MobKind::Cod
                            | MobKind::Salmon
                            | MobKind::TropicalFish
                            | MobKind::Pufferfish
                            | MobKind::Squid
                            | MobKind::GlowSquid
                            | MobKind::Drowned
                            | MobKind::Guardian
                            | MobKind::ElderGuardian
                            | MobKind::Tadpole
                    )
                    && distance(m.pos, mob.pos) <= HUNT_RANGE
            })
            .min_by(|a, b| distance(a.1.pos, mob.pos).total_cmp(&distance(b.1.pos, mob.pos)))
            .map(|(&mid, m)| (mid, m.pos));
        let Some((mid, at)) = prey else {
            return false;
        };
        mob.fly_goal = Some(at);
        if distance(at, mob.pos) <= 1.5 && mob.attack_cooldown == 0 {
            mob.attack_cooldown = 20;
            self.hit_mob(mid, 2.0, None, [0.0, 0.0], "minecraft:mob_attack");
        }
        true
    }

    /// Feeding an axolotl its tropical fish bucket leaves a water bucket.
    pub(super) fn bucket_food_returns(&mut self, id: i32, slot: usize, held: &str) {
        if held.ends_with("_bucket") {
            self.exchange_held(id, slot, "minecraft:water_bucket");
        }
    }

    /// The colour of a baby axolotl: a parent's, or blue one time in 1200.
    pub(super) fn axolotl_baby_variant(&mut self, a: u8, b: u8) -> u8 {
        if self.rng.below(1200) == 0 {
            4
        } else if self.rng.below(2) == 0 {
            a
        } else {
            b
        }
    }

    /// Whether a fox's coat for `biome` is the snowy one.
    pub(super) fn fox_variant_for(biome: &str) -> u8 {
        u8::from(biome.contains("snowy") || biome.contains("frozen") || biome.contains("grove"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-critters-{}-{}",
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
            uuid: 11,
            properties: Vec::new(),
        };
        w.players
            .insert(alice, Player::new(profile, out, [20.5, -60.0, 20.5]));
        (w, alice)
    }

    fn hold(w: &mut World, id: i32, item: &str) {
        w.players.get_mut(&id).unwrap().inventory.slots[36] =
            Stack::new(ItemInfo::id_of(item).unwrap(), 1);
    }

    #[test]
    fn fish_go_into_buckets_and_out_again() {
        let (mut w, alice) = world_with_player();
        hold(&mut w, alice, "minecraft:water_bucket");
        let cod = next_entity_id();
        w.mobs
            .insert(cod, Mob::new(MobKind::Cod, [3.5, -60.0, 3.5], &mut w.rng));
        assert!(w.scoop_mob(alice, cod, 36));
        assert!(!w.mobs.contains_key(&cod));
        let held = w.players[&alice].inventory.slots[36].item;
        assert_eq!(ItemInfo::get(held).unwrap().name, "minecraft:cod_bucket");
        w.release_from_bucket("minecraft:cod_bucket", BlockPos::new(5, -60, 5));
        assert!(w
            .mobs
            .values()
            .any(|m| m.kind == MobKind::Cod && m.persistent));
        assert_eq!(
            mob_in_bucket("minecraft:axolotl_bucket"),
            Some(MobKind::Axolotl)
        );
        assert!(bucket_of(MobKind::Squid).is_none());
    }

    #[test]
    fn axolotls_dry_out_on_land() {
        let (mut w, _) = world_with_player();
        let mut axolotl = Mob::new(MobKind::Axolotl, [3.5, -60.0, 3.5], &mut w.rng);
        assert!(axolotl.variant < 4, "blue only from breeding");
        let full = axolotl.health;
        for _ in 0..AXOLOTL_MOIST + 100 {
            w.tick += 1;
            w.tick_axolotl(1, &mut axolotl);
        }
        assert!(axolotl.health < full, "dried out");
    }

    #[test]
    fn foxes_sleep_by_day_and_wake_at_night() {
        let (mut w, _) = world_with_player();
        let mut fox = Mob::new(MobKind::Fox, [3.5, -60.0, 3.5], &mut w.rng);
        w.day_time = 6000;
        assert!(w.tick_fox(1, &mut fox));
        assert!(fox.sitting, "asleep");
        w.day_time = 18_000;
        w.tick_fox(1, &mut fox);
        assert!(!fox.sitting, "awake at night");
        assert_eq!(World::fox_variant_for("minecraft:snowy_taiga"), 1);
        assert_eq!(World::fox_variant_for("minecraft:taiga"), 0);
    }

    #[test]
    fn parrots_are_tamed_with_seeds_and_killed_by_cookies() {
        let (mut w, alice) = world_with_player();
        let parrot = next_entity_id();
        w.mobs.insert(
            parrot,
            Mob::new(MobKind::Parrot, [3.5, -59.0, 3.5], &mut w.rng),
        );
        for _ in 0..200 {
            hold(&mut w, alice, "minecraft:wheat_seeds");
            w.use_on_parrot(alice, parrot, 36, "minecraft:wheat_seeds");
            if w.mobs[&parrot].owner.is_some() {
                break;
            }
        }
        assert_eq!(w.mobs[&parrot].owner, Some(11));
        assert!(w.mobs[&parrot].sitting);
        hold(&mut w, alice, "minecraft:cookie");
        w.use_on_parrot(alice, parrot, 36, "minecraft:cookie");
        assert!(!w.mobs[&parrot].alive());
    }

    #[test]
    fn turtles_lay_eggs_at_home_that_hatch_into_babies() {
        let (mut w, _) = world_with_player();
        let home = BlockPos::new(3, -60, 3);
        w.set_block(BlockPos::new(3, -61, 3), block::SAND);
        let mut turtle = Mob::new(MobKind::Turtle, [3.5, -60.0, 3.5], &mut w.rng);
        turtle.home = Some(home);
        w.turtle_bred(1, &mut turtle);
        w.tick_turtle(1, &mut turtle);
        assert!(!turtle.egg, "laid");
        let egg = w.block(home);
        assert_eq!(crate::blocks::name(egg), "minecraft:turtle_egg");
        w.day_time = 21_500;
        for _ in 0..3 {
            let state = w.block(home);
            w.turtle_egg_tick(home, state);
        }
        assert!(is_air(w.block(home)), "hatched");
        let babies = w
            .mobs
            .values()
            .filter(|m| m.kind == MobKind::Turtle && m.age < 0 && m.home == Some(home))
            .count();
        assert!((1..=4).contains(&babies));
    }
}
