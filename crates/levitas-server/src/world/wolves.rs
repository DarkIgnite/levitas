//! Wolves, following the Minecraft Wiki: wild ones are neutral until a player hits one,
//! when it and the wolves around it turn on that player. A bone tames a wild wolf one
//! time in three; the tamed wolf sits at first, wears a red collar (dye changes it),
//! has 40 health, follows its owner, teleports to them when left far behind and fights
//! whatever the owner fights or what attacks the owner. Meat mends it and, at full
//! health, puts it in love mode. Right-clicking sits it down or stands it up again.
//!
//! Cats are tamed the same way with raw cod or salmon: they sit, wear a collar,
//! follow and teleport, are mended and bred with fish, but never fight. Wild ones keep
//! away from players, and stray cats turn up around villages.

use super::mobs::{Mob, MobKind};
use super::survival::Damage;
use super::*;

const DATA_TAME_FLAGS: u8 = 18;
const DATA_OWNER: u8 = 19;
const DATA_COLLAR: u8 = 21;
const DATA_ANGER: u8 = 22;
/// The coat (an ID in `wolf_variant`).
const DATA_WOLF_VARIANT: u8 = 23;
const SERIALIZER_WOLF_VARIANT: i32 = 25;
const SERIALIZER_BYTE: i32 = 0;
const SERIALIZER_VARINT: i32 = 1;
const SERIALIZER_VARLONG: i32 = 2;
const SERIALIZER_OWNER: i32 = 13;
const SITTING: u8 = 0x01;
const TAMED: u8 = 0x04;
/// A new collar is red.
pub const DEFAULT_COLLAR: u8 = 14;
pub const TAMED_HEALTH: f32 = 40.0;
const ENTITY_EVENT_ATTACK: i8 = 4;
const ENTITY_EVENT_TAME_FAILED: i8 = 6;
const ENTITY_EVENT_TAME_DONE: i8 = 7;
const REACH: f64 = 2.0;
const ATTACK_TICKS: u32 = 20;
/// Ticks of anger at a player who hit a wild wolf: 20 to 39 seconds.
const ANGER_MIN: u32 = 400;
const ANGER_SPREAD: u32 = 400;
/// How far the pack joins in, how far a hunt goes on, and how far an owner can be before
/// the wolf comes to them or jumps to them.
const PACK_RANGE: f64 = 16.0;
const HUNT_RANGE: f64 = 20.0;
const FOLLOW_DISTANCE: f64 = 5.0;
const TELEPORT_DISTANCE: f64 = 12.0;

/// How much health a food mends, or `None` if wolves do not eat it.
pub fn meat_health(item: &str) -> Option<f32> {
    Some(match item {
        "minecraft:porkchop" | "minecraft:beef" | "minecraft:rabbit" => 3.0,
        "minecraft:chicken" | "minecraft:mutton" | "minecraft:cod" | "minecraft:salmon" => 2.0,
        "minecraft:cooked_porkchop" | "minecraft:cooked_beef" => 8.0,
        "minecraft:cooked_chicken" | "minecraft:cooked_mutton" => 6.0,
        "minecraft:cooked_rabbit" => 5.0,
        "minecraft:rotten_flesh" => 4.0,
        _ => return None,
    })
}

/// The collar colour a dye gives, or `None` for anything else.
fn dye_colour(item: &str) -> Option<u8> {
    let name = item.strip_prefix("minecraft:")?.strip_suffix("_dye")?;
    super::mobs::COLORS
        .iter()
        .position(|&c| c == name)
        .map(|i| i as u8)
}

/// What tames, mends and breeds an animal of `kind`.
fn tames_with(kind: MobKind, item: &str) -> bool {
    match kind {
        MobKind::Cat => matches!(item, "minecraft:cod" | "minecraft:salmon"),
        _ => item == "minecraft:bone",
    }
}

fn mends(kind: MobKind, item: &str) -> Option<f32> {
    match kind {
        MobKind::Cat => matches!(item, "minecraft:cod" | "minecraft:salmon").then_some(2.0),
        _ => meat_health(item),
    }
}

/// A cat's look: its variant (an ID in `cat_variant`) and collar.
fn cat_data(mob: &Mob, packet: Writer) -> Writer {
    const DATA_CAT_VARIANT: u8 = 20;
    const DATA_CAT_COLLAR: u8 = 23;
    const SERIALIZER_CAT_VARIANT: i32 = 21;
    packet
        .u8(DATA_CAT_VARIANT)
        .varint(SERIALIZER_CAT_VARIANT)
        .varint(i32::from(mob.variant))
        .u8(DATA_CAT_COLLAR)
        .varint(SERIALIZER_VARINT)
        .varint(i32::from(mob.collar))
        .u8(0xFF)
}

/// The entity data telling clients a wolf's (or cat's) owner, pose, collar and anger.
pub fn wolf_data(mob: &Mob, id: i32) -> Writer {
    let flags = if mob.sitting { SITTING } else { 0 } | if mob.owner.is_some() { TAMED } else { 0 };
    let packet = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_TAME_FLAGS)
        .varint(SERIALIZER_BYTE)
        .u8(flags)
        .u8(DATA_OWNER)
        .varint(SERIALIZER_OWNER);
    let packet = match mob.owner {
        Some(owner) => packet.bool(true).uuid(owner),
        None => packet.bool(false),
    };
    if mob.kind == MobKind::Cat {
        return cat_data(mob, packet);
    }
    packet
        .u8(DATA_COLLAR)
        .varint(SERIALIZER_VARINT)
        .varint(i32::from(mob.collar))
        .u8(DATA_ANGER)
        .varint(SERIALIZER_VARLONG)
        .varlong(if mob.anger > 0 {
            i64::from(mob.anger)
        } else {
            -1
        })
        .u8(DATA_WOLF_VARIANT)
        .varint(SERIALIZER_WOLF_VARIANT)
        .varint(i32::from(mob.variant))
        .u8(0xFF)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

impl World {
    /// A player right-clicks a wolf; an item that gets used up is taken here.
    pub(super) fn use_on_wolf(&mut self, id: i32, target: i32, slot: usize, held: &str) {
        let Some(mut mob) = self.mobs.remove(&target) else {
            return;
        };
        let uuid = self.players[&id].profile.uuid;
        let survival = self.players[&id].survival();
        let mut used = false;
        match mob.owner {
            None => {
                if tames_with(mob.kind, held) && mob.anger == 0 {
                    used = true;
                    if self.rng.below(3) == 0 {
                        mob.owner = Some(uuid);
                        mob.sitting = true;
                        mob.health = mob.max_health();
                        mob.hunting = None;
                        mob.target = None;
                        mob.path.clear();
                        self.send_to_viewers(&mob, entity_event(target, ENTITY_EVENT_TAME_DONE));
                        self.send_to_viewers(&mob, wolf_data(&mob, target));
                        self.advance(
                            id,
                            super::advancements::Trigger::TamedAnimal(mob.kind.entity_type()),
                        );
                    } else {
                        self.send_to_viewers(&mob, entity_event(target, ENTITY_EVENT_TAME_FAILED));
                    }
                }
            }
            Some(owner) if owner == uuid => {
                if let Some(colour) = dye_colour(held) {
                    if colour != mob.collar {
                        mob.collar = colour;
                        used = true;
                        self.send_to_viewers(&mob, wolf_data(&mob, target));
                    }
                } else if let Some(heal) = mends(mob.kind, held) {
                    if mob.health < mob.max_health() {
                        mob.health = (mob.health + heal).min(mob.max_health());
                        used = true;
                        let sound = format!("minecraft:entity.{}.ambient", mob.kind.sound());
                        self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
                    } else if mob.age >= 0 {
                        // Fed at full health it falls in love instead.
                        self.mobs.insert(target, mob);
                        self.feed_animal(id, target, slot);
                        return;
                    }
                } else {
                    mob.sitting = !mob.sitting;
                    mob.target = None;
                    mob.path.clear();
                    mob.prey = None;
                    mob.hunting = None;
                    self.send_to_viewers(&mob, wolf_data(&mob, target));
                }
            }
            Some(_) => {}
        }
        self.mobs.insert(target, mob);
        if used && survival {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
    }

    /// A wolf's turn: sits, follows or teleports to its owner, and fights.
    pub(super) fn tick_wolf(&mut self, id: i32, mob: &mut Mob) {
        mob.attack_cooldown = mob.attack_cooldown.saturating_sub(1);
        if mob.anger > 0 {
            mob.anger -= 1;
            if mob.anger == 0 {
                mob.hunting = None;
                self.send_to_viewers(mob, wolf_data(mob, id));
            }
        }
        if mob.sitting {
            mob.target = None;
            mob.path.clear();
            return;
        }
        let owner_pos = mob.owner.and_then(|uuid| {
            self.players
                .values()
                .find(|p| p.profile.uuid == uuid && p.alive)
                .map(|p| p.pos)
        });
        // The player it is angry at, or the mob it is fighting.
        let hunted = mob
            .hunting
            .and_then(|pid| self.players.get(&pid))
            .filter(|p| p.alive && p.survival() && distance(p.pos, mob.pos) <= HUNT_RANGE)
            .map(|p| p.pos);
        if hunted.is_none() {
            mob.hunting = None;
        }
        let prey_valid = mob
            .prey
            .and_then(|m| self.mobs.get(&m))
            .is_some_and(|m| m.alive() && distance(m.pos, mob.pos) <= HUNT_RANGE);
        if !prey_valid {
            mob.prey = None;
        }
        let target = match (hunted, mob.prey) {
            (Some(pos), _) => Some(pos),
            (None, Some(m)) => Some(self.mobs[&m].pos),
            (None, None) => None,
        };
        let Some(target) = target else {
            if let Some(owner) = owner_pos {
                let d = distance(owner, mob.pos);
                if d > TELEPORT_DISTANCE {
                    self.teleport_wolf(mob, owner);
                } else if d > FOLLOW_DISTANCE {
                    let goal = BlockPos::new(
                        owner[0].floor() as i32,
                        (owner[1] + 0.01).floor() as i32,
                        owner[2].floor() as i32,
                    );
                    self.follow_path(mob, goal);
                }
            }
            return;
        };
        mob.head_yaw = super::mobs::yaw_towards(mob.pos, target);
        let goal = BlockPos::new(
            target[0].floor() as i32,
            (target[1] + 0.01).floor() as i32,
            target[2].floor() as i32,
        );
        self.follow_path(mob, goal);
        let close = distance(target, mob.pos) <= REACH && (target[1] - mob.pos[1]).abs() < 2.0;
        if !close || mob.attack_cooldown > 0 {
            return;
        }
        mob.attack_cooldown = ATTACK_TICKS;
        let damage = mob.attack_damage();
        self.send_to_viewers(mob, entity_event(id, ENTITY_EVENT_ATTACK));
        match (hunted.and(mob.hunting), mob.prey) {
            (Some(pid), _) => {
                self.damage_from(pid, damage, Damage::Mob("Wolf"), Some(id));
            }
            (None, Some(prey)) => {
                self.hit_mob(prey, damage, None, [0.0, 0.0], "minecraft:mob_attack");
            }
            (None, None) => {}
        }
    }

    /// Puts a wolf on firm ground near its owner.
    fn teleport_wolf(&mut self, mob: &mut Mob, owner: [f64; 3]) {
        for _ in 0..10 {
            let dx = f64::from(self.rng.below(7)) - 3.0;
            let dz = f64::from(self.rng.below(7)) - 3.0;
            if dx.abs() < 2.0 && dz.abs() < 2.0 {
                continue;
            }
            for dy in [0.0, 1.0, -1.0] {
                let spot = [owner[0] + dx, owner[1] + dy, owner[2] + dz];
                if self.mob_fits(mob.kind, spot) {
                    mob.pos = spot;
                    mob.path.clear();
                    mob.target = None;
                    return;
                }
            }
        }
    }

    /// A player hurt a wolf: a wild one and the wild wolves around it turn on that player.
    pub(super) fn wolf_hurt_by(&mut self, target: i32, attacker: i32) {
        let Some(at) = self
            .mobs
            .get(&target)
            .filter(|m| m.owner.is_none())
            .map(|m| m.pos)
        else {
            return;
        };
        let ids: Vec<i32> = self
            .mobs
            .iter()
            .filter(|(_, m)| {
                m.kind == MobKind::Wolf
                    && m.owner.is_none()
                    && m.alive()
                    && distance(m.pos, at) <= PACK_RANGE
            })
            .map(|(&i, _)| i)
            .collect();
        for wolf in ids {
            let anger = ANGER_MIN + self.rng.below(ANGER_SPREAD);
            let mut mob = self.mobs.remove(&wolf).expect("listed");
            mob.anger = anger;
            mob.hunting = Some(attacker);
            self.send_to_viewers(&mob, wolf_data(&mob, wolf));
            self.mobs.insert(wolf, mob);
        }
    }

    /// The owner fought something, or was attacked: their standing tamed wolves join in.
    pub(super) fn rally_wolves(&mut self, owner: i32, enemy: i32) {
        let Some(uuid) = self.players.get(&owner).map(|p| p.profile.uuid) else {
            return;
        };
        let Some(foe) = self.mobs.get(&enemy) else {
            return;
        };
        // Wolves leave the owner's own pets alone, and keep away from creepers and
        // ghasts as vanilla's do.
        if !foe.alive()
            || foe.owner == Some(uuid)
            || matches!(foe.kind, MobKind::Creeper | MobKind::Ghast)
        {
            return;
        }
        for (&id, wolf) in &mut self.mobs {
            if id != enemy
                && wolf.kind == MobKind::Wolf
                && wolf.owner == Some(uuid)
                && !wolf.sitting
            {
                wolf.prey = Some(enemy);
            }
        }
    }
}

impl World {
    /// Stray cats turn up in villages, as vanilla's cat spawner does every minute: next
    /// to a villager near a player, while fewer than five cats live around.
    pub(super) fn tick_village_cats(&mut self) {
        const EVERY: u64 = 1200;
        const RANGE: f64 = 48.0;
        const MAX_CATS: usize = 5;
        if !self.tick.is_multiple_of(EVERY) || !crate::rules::bool("spawn_mobs") {
            return;
        }
        let players: Vec<[f64; 3]> = self
            .players
            .values()
            .filter(|p| p.alive)
            .map(|p| p.pos)
            .collect();
        for at in players {
            let villagers: Vec<[f64; 3]> = self
                .mobs
                .values()
                .filter(|m| {
                    m.alive() && m.kind == MobKind::Villager && distance(m.pos, at) <= RANGE
                })
                .map(|m| m.pos)
                .collect();
            if villagers.is_empty() {
                continue;
            }
            let cats = self
                .mobs
                .values()
                .filter(|m| m.kind == MobKind::Cat && distance(m.pos, at) <= RANGE)
                .count();
            if cats >= MAX_CATS {
                continue;
            }
            let spot = villagers[self.rng.below(villagers.len() as u32) as usize];
            if self.mob_fits(MobKind::Cat, spot) {
                let cat = Mob::new(MobKind::Cat, spot, &mut self.rng);
                self.mobs.insert(next_entity_id(), cat);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;
    use tokio::sync::mpsc;

    const OWNER: i32 = 7;
    const SLOT: usize = 36;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-wolf-{}-{}",
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
        for cx in 0..3 {
            w.chunk(cx, 0);
        }
        let (tx, _rx): (mpsc::Sender<Frame>, _) = mpsc::channel(16);
        let profile = Profile {
            uuid: 0xB0B,
            name: "Wolfy".to_owned(),
            properties: Vec::new(),
        };
        w.players
            .insert(OWNER, Player::new(profile, tx, [8.5, -60.0, 8.5]));
        w
    }

    fn give(w: &mut World, item: &str, count: i32) {
        let item = ItemInfo::id_of(item).expect("item");
        w.players.get_mut(&OWNER).unwrap().inventory.slots[SLOT] = Stack::new(item, count);
    }

    fn wolf(w: &mut World, pos: [f64; 3]) -> i32 {
        let id = next_entity_id();
        let mob = Mob::new(MobKind::Wolf, pos, &mut w.rng);
        w.mobs.insert(id, mob);
        id
    }

    fn tame(w: &mut World, id: i32) {
        give(w, "minecraft:bone", 64);
        for _ in 0..200 {
            w.use_on_wolf(OWNER, id, SLOT, "minecraft:bone");
            if w.mobs[&id].owner.is_some() {
                return;
            }
        }
        panic!("a bone never tamed the wolf");
    }

    #[test]
    fn cats_are_tamed_with_fish_keep_their_coat_and_turn_up_in_villages() {
        let mut w = world();
        let id = next_entity_id();
        w.mobs
            .insert(id, Mob::new(MobKind::Cat, [10.5, -60.0, 8.5], &mut w.rng));
        // A bone does nothing for a cat; fish does.
        give(&mut w, "minecraft:bone", 1);
        w.use_on_wolf(OWNER, id, SLOT, "minecraft:bone");
        assert!(w.mobs[&id].owner.is_none());
        give(&mut w, "minecraft:cod", 64);
        for _ in 0..200 {
            w.use_on_wolf(OWNER, id, SLOT, "minecraft:cod");
            if w.mobs[&id].owner.is_some() {
                break;
            }
        }
        let cat = &w.mobs[&id];
        assert_eq!(cat.owner, Some(0xB0B));
        assert!(cat.sitting);
        assert_eq!(cat.max_health(), 10.0, "tamed cats keep their health");
        // Its data carries its coat (index 20) and collar (23).
        let frame = wolf_data(cat, id).into_frame();
        let mut r = levitas_protocol::Reader::new(&frame);
        r.varint().unwrap();
        r.varint().unwrap();
        r.varint().unwrap();
        assert_eq!((r.u8().unwrap(), r.varint().unwrap()), (18, 0));
        r.u8().unwrap();
        assert_eq!((r.u8().unwrap(), r.varint().unwrap()), (19, 13));
        assert!(r.bool().unwrap());
        r.uuid().unwrap();
        assert_eq!((r.u8().unwrap(), r.varint().unwrap()), (20, 21));
        assert_eq!(r.varint().unwrap(), i32::from(cat.variant));
        assert_eq!((r.u8().unwrap(), r.varint().unwrap()), (23, 1));
        // The coat survives saving.
        w.mobs.get_mut(&id).unwrap().variant = 3;
        let back = Mob::read(&w.mobs[&id].to_nbt(), &mut w.rng).unwrap();
        assert_eq!((back.variant, back.owner), (3, Some(0xB0B)));
        // A village near a player gets a stray cat now and then.
        w.mobs.clear();
        w.mobs.insert(
            next_entity_id(),
            Mob::new(MobKind::Villager, [12.5, -60.0, 8.5], &mut w.rng),
        );
        w.tick = 1200;
        w.tick_village_cats();
        assert!(w.mobs.values().any(|m| m.kind == MobKind::Cat));
    }

    #[test]
    fn dyes_and_meat_are_told_apart() {
        assert_eq!(dye_colour("minecraft:red_dye"), Some(DEFAULT_COLLAR));
        assert_eq!(dye_colour("minecraft:white_dye"), Some(0));
        assert_eq!(dye_colour("minecraft:bone"), None);
        assert_eq!(meat_health("minecraft:cooked_beef"), Some(8.0));
        assert_eq!(meat_health("minecraft:wheat"), None);
    }

    #[test]
    fn bones_tame_a_wild_wolf_which_then_sits_with_a_red_collar() {
        let mut w = world();
        let id = wolf(&mut w, [10.5, -60.0, 8.5]);
        assert_eq!(w.mobs[&id].max_health(), 8.0);
        tame(&mut w, id);
        let wolf = &w.mobs[&id];
        assert_eq!(wolf.owner, Some(0xB0B));
        assert!(wolf.sitting);
        assert_eq!((wolf.health, wolf.max_health()), (40.0, 40.0));
        assert_eq!(wolf.collar, DEFAULT_COLLAR);
        // Bones were used up while trying.
        let left = w.players[&OWNER].inventory.slots[SLOT].count;
        assert!(left < 64);
    }

    #[test]
    fn the_owner_sits_dyes_and_feeds_but_a_stranger_cannot() {
        let mut w = world();
        let id = wolf(&mut w, [10.5, -60.0, 8.5]);
        tame(&mut w, id);
        // Standing up, then sitting down again.
        w.use_on_wolf(OWNER, id, SLOT, "");
        assert!(!w.mobs[&id].sitting);
        w.use_on_wolf(OWNER, id, SLOT, "");
        assert!(w.mobs[&id].sitting);
        // Dye changes the collar and is used up.
        give(&mut w, "minecraft:blue_dye", 2);
        w.use_on_wolf(OWNER, id, SLOT, "minecraft:blue_dye");
        assert_eq!(w.mobs[&id].collar, 11);
        assert_eq!(w.players[&OWNER].inventory.slots[SLOT].count, 1);
        // Meat mends a hurt wolf.
        w.mobs.get_mut(&id).unwrap().health = 10.0;
        give(&mut w, "minecraft:cooked_beef", 2);
        w.use_on_wolf(OWNER, id, SLOT, "minecraft:cooked_beef");
        assert_eq!(w.mobs[&id].health, 18.0);
        // Somebody else's wolf stays as it is.
        w.mobs.get_mut(&id).unwrap().owner = Some(0xDEAD);
        w.use_on_wolf(OWNER, id, SLOT, "");
        assert!(w.mobs[&id].sitting);
    }

    #[test]
    fn the_wolf_data_is_laid_out_as_26_3_reads_it() {
        let mut w = world();
        let id = wolf(&mut w, [10.5, -60.0, 8.5]);
        tame(&mut w, id);
        let frame = wolf_data(&w.mobs[&id], id).into_frame();
        let mut r = levitas_protocol::Reader::new(&frame);
        r.varint().unwrap(); // length
        r.varint().unwrap(); // packet ID
        assert_eq!(r.varint().unwrap(), id);
        assert_eq!((r.u8().unwrap(), r.varint().unwrap()), (18, 0));
        assert_eq!(r.u8().unwrap(), SITTING | TAMED);
        assert_eq!((r.u8().unwrap(), r.varint().unwrap()), (19, 13));
        assert!(r.bool().unwrap());
        assert_eq!(r.uuid().unwrap(), 0xB0B);
        assert_eq!((r.u8().unwrap(), r.varint().unwrap()), (21, 1));
        assert_eq!(r.varint().unwrap(), i32::from(DEFAULT_COLLAR));
        assert_eq!((r.u8().unwrap(), r.varint().unwrap()), (22, 2));
        assert_eq!(r.varlong().unwrap(), -1);
        // The coat: pale unless the biome grows another.
        assert_eq!((r.u8().unwrap(), r.varint().unwrap()), (23, 25));
        let pale = super::super::mobs::wolf_variant_for("minecraft:plains");
        assert_eq!(r.varint().unwrap(), i32::from(pale));
        assert_eq!(r.u8().unwrap(), 0xFF);
        let woods = super::super::mobs::wolf_variant_for("minecraft:forest");
        let rusty = super::super::mobs::wolf_variant_for("minecraft:sparse_jungle");
        let striped = super::super::mobs::wolf_variant_for("minecraft:eroded_badlands");
        assert!(woods != pale && rusty != pale && striped != pale && woods != rusty);
    }

    #[test]
    fn a_standing_wolf_follows_and_a_far_one_teleports_to_its_owner() {
        let mut w = world();
        let id = wolf(&mut w, [26.5, -60.0, 8.5]);
        tame(&mut w, id);
        let mut mob = w.mobs.remove(&id).unwrap();
        mob.sitting = false;
        w.tick_wolf(id, &mut mob);
        assert!(distance(mob.pos, [8.5, -60.0, 8.5]) <= 6.0);
        // Nearer, it walks instead.
        mob.pos = [16.5, -60.0, 8.5];
        w.tick_wolf(id, &mut mob);
        assert!(mob.target.is_some());
        // A sitting wolf stays put.
        mob.pos = [26.5, -60.0, 8.5];
        mob.sitting = true;
        w.tick_wolf(id, &mut mob);
        assert_eq!(mob.pos, [26.5, -60.0, 8.5]);
    }

    #[test]
    fn tamed_wolves_join_the_owners_fights() {
        let mut w = world();
        let id = wolf(&mut w, [10.5, -60.0, 8.5]);
        tame(&mut w, id);
        w.mobs.get_mut(&id).unwrap().sitting = false;
        let zombie = next_entity_id();
        let z = Mob::new(MobKind::Zombie, [12.5, -60.0, 8.5], &mut w.rng);
        w.mobs.insert(zombie, z);
        w.hit_mob(
            zombie,
            1.0,
            Some(OWNER),
            [0.0, 0.0],
            "minecraft:player_attack",
        );
        assert_eq!(w.mobs[&id].prey, Some(zombie));
        // A sitting wolf stays out of it.
        w.mobs.get_mut(&id).unwrap().prey = None;
        w.mobs.get_mut(&id).unwrap().sitting = true;
        w.hit_mob(zombie, 1.0, Some(OWNER), [0.0, 0.0], "minecraft:magic");
        assert_eq!(w.mobs[&id].prey, None);
    }

    #[test]
    fn hitting_a_wild_wolf_angers_the_pack_but_not_a_tamed_one() {
        let mut w = world();
        let hit = wolf(&mut w, [10.5, -60.0, 8.5]);
        let packmate = wolf(&mut w, [14.5, -60.0, 8.5]);
        let far = wolf(&mut w, [40.5, -60.0, 8.5]);
        w.hit_mob(hit, 1.0, Some(OWNER), [0.0, 0.0], "minecraft:player_attack");
        for angry in [hit, packmate] {
            assert!(w.mobs[&angry].anger >= ANGER_MIN);
            assert_eq!(w.mobs[&angry].hunting, Some(OWNER));
        }
        assert_eq!(w.mobs[&far].anger, 0);
        let pet = wolf(&mut w, [10.5, -60.0, 10.5]);
        tame(&mut w, pet);
        w.hit_mob(pet, 1.0, Some(OWNER), [0.0, 0.0], "minecraft:player_attack");
        assert_eq!(w.mobs[&pet].anger, 0);
    }

    #[test]
    fn a_wild_wolf_bites_the_player_who_angered_it() {
        let mut w = world();
        let id = wolf(&mut w, [9.5, -60.0, 8.5]);
        let mut mob = w.mobs.remove(&id).unwrap();
        mob.anger = 400;
        mob.hunting = Some(OWNER);
        let before = w.players[&OWNER].health;
        w.tick_wolf(id, &mut mob);
        assert!(w.players[&OWNER].health < before);
    }

    #[test]
    fn tamed_wolves_survive_saving_and_pups_inherit_the_owner() {
        let mut w = world();
        let id = wolf(&mut w, [10.5, -60.0, 8.5]);
        tame(&mut w, id);
        w.mobs.get_mut(&id).unwrap().collar = 3;
        let nbt = w.mobs[&id].to_nbt();
        let back = Mob::read(&nbt, &mut w.rng).expect("reads");
        assert_eq!(back.owner, Some(0xB0B));
        assert!(back.sitting);
        assert_eq!(back.collar, 3);
        assert_eq!(back.health, 40.0);
        // Two loving wolves, one tamed.
        let mate = wolf(&mut w, [10.5, -60.0, 9.5]);
        for i in [id, mate] {
            let m = w.mobs.get_mut(&i).unwrap();
            m.love = 100;
            m.sitting = false;
        }
        let mut first = w.mobs.remove(&id).unwrap();
        w.tick_breeding(id, &mut first);
        w.mobs.insert(id, first);
        let pup = w
            .mobs
            .values()
            .find(|m| m.kind == MobKind::Wolf && m.age < 0)
            .expect("a pup");
        assert_eq!(pup.owner, Some(0xB0B));
    }
}
