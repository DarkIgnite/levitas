//! Camels and camel husks: sitting down, dashing, the second seat, and what makes a
//! camel husk different.
//!
//! Riding, saddles, steering, feeding and stats live in `horses.rs` (camels count as
//! tame there); this module only adds what is camel-specific. Behaviour follows the
//! Minecraft Wiki (Camel, Camel Husk, Riding):
//! - Now and then an idle camel sits down, for two to seven seconds, then stands up
//!   again. A sitting camel does not wander. It stands up when hurt (at once, with no
//!   animation), when it panics, when a player close by holds its food, or, if someone
//!   rides it, once its rider moves forward. It never stands up under a block it would
//!   not fit beneath. How long an idle camel stands before it sits again is not on the
//!   wiki: Levitas rolls 30 to 90 seconds, and gives it a second to get up.
//! - A saddled camel dashes when its rider jumps (the client sends "start jump with
//!   horse" and moves the camel itself, as it steers it). It can dash again 2.75
//!   seconds later; while in the air after a dash it shows as dashing.
//! - Up to two players ride a camel: once a camel has a rider, a second player sits
//!   behind them. When the rider gets off, the one behind takes the reins.
//! - Camels regain one health point with a chance of 1 in 900 each tick.
//! - Camel husks are undead camels: they do not burn in sunlight and cannot breed.
//!   A rabbit's foot heals them (cactus does nothing for them), and once a player has
//!   used one it is marked persistent. Husk jockeys (a husk and a parched riding a
//!   camel husk in deserts) are not spawned.
//! - Saved as vanilla: `LastPoseTick`, negative while sitting.

use levitas_nbt::Compound;

use super::mobs::{Mob, MobKind};
use super::*;

/// Entity data: health, the pose, whether it is dashing, and when its pose last changed.
const DATA_HEALTH: u8 = 9;
const DATA_POSE: u8 = 6;
const DATA_DASHING: u8 = 19;
const DATA_LAST_POSE_TICK: u8 = 20;
const SERIALIZER_LONG: i32 = 2;
const SERIALIZER_FLOAT: i32 = 3;
const SERIALIZER_BOOL: i32 = 8;
const SERIALIZER_POSE: i32 = 20;
const POSE_STANDING: i32 = 0;
const POSE_SITTING: i32 = 10;
/// Ticks between dashes: 2.75 seconds.
const DASH_COOLDOWN: u32 = 55;
/// Ticks into a dash before touching the ground counts as landing.
const DASH_LIFTOFF: u32 = 5;
/// The longest a dash shows, should its landing be missed.
const DASH_MAX_TICKS: u32 = 40;
/// Half an adult camel's width, a little inside its edges, for finding its footing.
const HALF_WIDTH: f64 = 0.8;
/// Ticks a camel sits: two to seven seconds.
const SIT_TICKS: (u32, u32) = (40, 140);
/// Ticks an idle camel stands before it sits down again (not on the wiki).
const STAND_TICKS: (u32, u32) = (600, 1800);
/// Ticks a camel takes to get up before it walks off.
const RISING_TICKS: u32 = 20;
/// How far back the pose change of a camel standing up at once is dated, so that the
/// client shows it standing with no animation.
const INSTANT_POSE_AGO: i64 = 200;
/// Players this close holding its food get a sitting camel up.
const TEMPT_RANGE: f64 = 10.0;
/// One in this many ticks, a hurt camel regains a health point.
const REGEN_ONE_IN: u32 = 900;

/// What the module keeps for a camel, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq)]
struct Camel {
    sitting: bool,
    /// The game tick its pose last changed.
    pose_tick: i64,
    /// Ticks until an idle camel sits down, or a sitting one stands up; rolled when
    /// first needed.
    pose_timer: Option<u32>,
    /// Ticks still getting up, during which it does not walk.
    rising: u32,
    dashing: bool,
    /// Ticks since the dash began, and until it may dash again.
    dash_ticks: u32,
    dash_cooldown: u32,
    /// The player in the back seat.
    back: Option<i32>,
}

impl Camel {
    fn new() -> Camel {
        Camel {
            sitting: false,
            pose_tick: 0,
            pose_timer: None,
            rising: 0,
            dashing: false,
            dash_ticks: 0,
            dash_cooldown: 0,
            back: None,
        }
    }

    /// The last pose change as the client and saves carry it: negative while sitting.
    fn pose_value(&self) -> i64 {
        if self.sitting {
            -self.pose_tick.max(1)
        } else {
            self.pose_tick
        }
    }
}

fn camel_of(mob: &Mob) -> Option<&Camel> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Camel>())
}

/// The camel's state, made fresh if it has none yet (a spawn egg, a summon).
fn camel_mut(mob: &mut Mob) -> &mut Camel {
    if !mob.kin.as_ref().is_some_and(|k| k.is::<Camel>()) {
        mob.kin = Some(Box::new(Camel::new()));
    }
    mob.kin
        .as_mut()
        .and_then(|k| k.downcast_mut::<Camel>())
        .expect("just made")
}

fn roll(rng: &mut Rng, (lo, hi): (u32, u32)) -> u32 {
    lo + rng.below(hi - lo + 1)
}

/// The item tag of what it eats: cactus for camels, rabbit's feet for husks.
fn food_tag(kind: MobKind) -> &'static str {
    if kind == MobKind::CamelHusk {
        "minecraft:camel_husk_food"
    } else {
        "minecraft:camel_food"
    }
}

fn camel_data(id: i32, c: &Camel) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_POSE)
        .varint(SERIALIZER_POSE)
        .varint(if c.sitting {
            POSE_SITTING
        } else {
            POSE_STANDING
        })
        .u8(DATA_DASHING)
        .varint(SERIALIZER_BOOL)
        .bool(c.dashing)
        .u8(DATA_LAST_POSE_TICK)
        .varint(SERIALIZER_LONG)
        .varlong(c.pose_value())
        .u8(0xFF)
}

/// Its health, which the rider's HUD shows.
fn health_data(id: i32, health: f32) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_HEALTH)
        .varint(SERIALIZER_FLOAT)
        .f32(health)
        .u8(0xFF)
}

/// Who rides a camel: the rider in front, then the one behind.
fn passengers(id: i32, front: Option<i32>, back: Option<i32>) -> Writer {
    let aboard: Vec<i32> = front.into_iter().chain(back).collect();
    let mut packet = Writer::packet(clientbound::SET_PASSENGERS)
        .varint(id)
        .varint(aboard.len() as i32);
    for e in aboard {
        packet = packet.varint(e);
    }
    packet
}

fn rider(mob: &Mob) -> Option<i32> {
    mob.horse.as_ref().and_then(|h| h.rider)
}

pub(super) fn spawned(w: &mut World, mob: &mut Mob, _biome: &str) {
    let mut c = Camel::new();
    c.pose_timer = Some(roll(&mut w.rng, STAND_TICKS));
    mob.kin = Some(Box::new(c));
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    camel_mut(mob);
    let Some(mut kin) = mob.kin.take() else {
        return false;
    };
    let resting = match kin.downcast_mut::<Camel>() {
        Some(c) => w.tick_camel(id, mob, c),
        None => false,
    };
    mob.kin = Some(kin);
    resting
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    let Some(mob) = w.mobs.get_mut(&target) else {
        return false;
    };
    let husk = mob.kind == MobKind::CamelHusk;
    if husk {
        // Once a player has used it, a camel husk stays.
        mob.persistent = true;
        if levitas_registry::item_has_tag(held, food_tag(MobKind::CamelHusk))
            && w.feed_husk(player, target, slot)
        {
            return true;
        }
    }
    if w.take_back_seat(player, target, held) {
        return true;
    }
    if husk && levitas_registry::item_has_tag(held, "minecraft:camel_food") {
        // Cactus is nothing to a husk: used as an empty hand would be.
        w.use_on_horse(player, target, slot, "minecraft:air");
        return true;
    }
    false
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, _attacker: Option<i32>) {
    // A hurt camel gets up at once to flee, if there is room.
    let c = camel_mut(mob);
    if !c.sitting {
        return;
    }
    let mut c = c.clone();
    if !w.mob_fits(mob.kind, mob.pos) {
        return;
    }
    c.sitting = false;
    c.rising = 0;
    c.pose_tick = w.tick as i64 - INSTANT_POSE_AGO;
    c.pose_timer = Some(roll(&mut w.rng, STAND_TICKS));
    w.send_to_viewers(mob, camel_data(id, &c));
    *camel_mut(mob) = c;
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let Some(c) = camel_of(mob) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if c.sitting || c.dashing || c.pose_tick != 0 {
        out.push(camel_data(id, c));
    }
    if c.back.is_some() {
        // After the single rider `horses.rs` shows.
        out.push(passengers(id, rider(mob), c.back));
    }
    out
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let value = camel_of(mob).map_or(0, Camel::pose_value);
    nbt.insert("LastPoseTick", value);
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let mut c = Camel::new();
    let value = nbt.int("LastPoseTick").unwrap_or(0);
    c.sitting = value < 0;
    c.pose_tick = value.abs();
    mob.kin = Some(Box::new(c));
}

pub(super) fn damage_taken(
    _w: &mut World,
    _id: i32,
    _mob: &mut Mob,
    damage: f32,
    _damage_type: &str,
    _attacker: Option<i32>,
) -> f32 {
    damage
}

/// Camel husks are undead and never breed.
pub(super) fn may_love(_w: &World, mob: &Mob) -> bool {
    mob.kind != MobKind::CamelHusk
}

pub(super) fn bred(_w: &mut World, _id: i32, _mob: &mut Mob) -> bool {
    false
}

pub(super) fn tick_world(_w: &mut World) {}

pub(super) fn block_placed(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

pub(super) fn block_broken(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

pub(super) fn block_used(
    _w: &mut World,
    _player: i32,
    _slot: usize,
    _pos: BlockPos,
    _state: u16,
) -> bool {
    false
}

pub(super) fn block_tick(_w: &mut World, _pos: BlockPos, _state: u16) -> bool {
    false
}

pub(super) fn ticks_randomly(_state: u16) -> bool {
    false
}

pub(super) fn scheduled_tick(_w: &mut World, _pos: BlockPos, _state: u16) -> bool {
    false
}

/// A player riding something pressed and let go of jump ("start jump with horse",
/// with a power of 0 to 100). On a saddled camel they steer, it dashes, unless it
/// dashed less than 2.75 seconds ago or sits. Returns whether a camel dashed.
///
/// Called from the world's event loop once `play.rs` forwards Player Command action 3
/// ("start jump with horse"); until then nothing calls it outside the tests.
pub(super) fn rider_jumped(w: &mut World, player: i32, _power: i32) -> bool {
    let Some(target) = w.players.get(&player).and_then(|p| p.riding) else {
        return false;
    };
    let Some(mut mob) = w.mobs.remove(&target) else {
        return false;
    };
    let dashed = w.dash(target, &mut mob, player);
    w.mobs.insert(target, mob);
    dashed
}

impl World {
    /// A camel's turn. Returns whether it rests (sitting, or getting up) this tick.
    fn tick_camel(&mut self, id: i32, mob: &mut Mob, c: &mut Camel) -> bool {
        self.camel_seats(id, mob, c);
        self.tick_dash(id, mob, c);
        if mob.health < mob.max_health() && self.rng.below(REGEN_ONE_IN) == 0 {
            mob.health = (mob.health + 1.0).min(mob.max_health());
            self.send_to_viewers(mob, health_data(id, mob.health));
        }
        if c.sitting {
            return self.tick_sitting(id, mob, c);
        }
        if c.rising > 0 {
            c.rising -= 1;
            mob.target = None;
            mob.path.clear();
            return true;
        }
        let idle = rider(mob).is_none()
            && c.back.is_none()
            && !c.dashing
            && mob.panic == 0
            && mob.love == 0
            && mob.vehicle.is_none()
            && mob.leader.is_none()
            && mob.on_ground_now()
            && !self.in_water(mob.pos);
        if !idle {
            return false;
        }
        let timer = c
            .pose_timer
            .get_or_insert_with(|| roll(&mut self.rng, STAND_TICKS));
        if *timer > 0 {
            *timer -= 1;
            return false;
        }
        // One following a player who holds its food keeps walking.
        if self.camel_tempted(mob.kind, mob.pos) {
            return false;
        }
        c.sitting = true;
        c.pose_tick = self.tick as i64;
        c.pose_timer = Some(roll(&mut self.rng, SIT_TICKS));
        mob.target = None;
        mob.path.clear();
        mob.vel = [0.0, mob.vel[1].min(0.0), 0.0];
        self.send_to_viewers(mob, camel_data(id, c));
        let sound = format!("minecraft:entity.{}.sit", mob.kind.sound());
        self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
        true
    }

    /// A sitting camel's turn: it gets up when its time is up, when it panics, when
    /// its food is held close by or when its rider moves forward, if there is room.
    fn tick_sitting(&mut self, id: i32, mob: &mut Mob, c: &mut Camel) -> bool {
        mob.target = None;
        mob.path.clear();
        let ridden = rider(mob);
        let timer = c
            .pose_timer
            .get_or_insert_with(|| roll(&mut self.rng, SIT_TICKS));
        *timer = timer.saturating_sub(1);
        let time_up = *timer == 0 && ridden.is_none() && c.back.is_none();
        let urged = ridden
            .and_then(|r| self.players.get(&r))
            .is_some_and(|p| p.forward);
        let wants_up = time_up || urged || mob.panic > 0 || self.camel_tempted(mob.kind, mob.pos);
        if !wants_up || !self.mob_fits(mob.kind, mob.pos) {
            return true;
        }
        c.sitting = false;
        c.pose_tick = self.tick as i64;
        c.rising = RISING_TICKS;
        c.pose_timer = Some(roll(&mut self.rng, STAND_TICKS));
        self.send_to_viewers(mob, camel_data(id, c));
        let sound = format!("minecraft:entity.{}.stand", mob.kind.sound());
        self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
        true
    }

    /// Whether a player close by holds what this camel eats, in either hand.
    fn camel_tempted(&self, kind: MobKind, at: [f64; 3]) -> bool {
        let tag = food_tag(kind);
        self.players.values().any(|p| {
            let d: f64 = (0..3).map(|i| (p.pos[i] - at[i]).powi(2)).sum();
            p.alive
                && d <= TEMPT_RANGE * TEMPT_RANGE
                && [p.hand_slot(0), p.hand_slot(1)].iter().any(|&slot| {
                    ItemInfo::get(p.inventory.slots[slot].item)
                        .is_some_and(|i| levitas_registry::item_has_tag(i.name, tag))
                })
        })
    }

    /// Keeps the two seats right: a back-seat player who left is let go, one whose
    /// rider left moves up to steer, and one still aboard is carried along.
    fn camel_seats(&mut self, id: i32, mob: &mut Mob, c: &mut Camel) {
        let Some(back) = c.back else {
            return;
        };
        let aboard = self
            .players
            .get(&back)
            .is_some_and(|p| p.alive && p.riding == Some(id));
        if !aboard {
            c.back = None;
            self.send_to_viewers(mob, passengers(id, rider(mob), None));
            return;
        }
        if rider(mob).is_none() {
            if let Some(h) = mob.horse.as_mut() {
                h.rider = Some(back);
            }
            c.back = None;
            self.send_to_viewers(mob, passengers(id, Some(back), None));
            return;
        }
        self.move_player(back, Some(mob.pos), None, true);
    }

    /// Dash timing: a dash ends when the camel is back on the ground (or after two
    /// seconds at most), and the rider hears when the next one is ready.
    fn tick_dash(&mut self, id: i32, mob: &mut Mob, c: &mut Camel) {
        if c.dashing {
            c.dash_ticks += 1;
            let landed = c.dash_ticks >= DASH_LIFTOFF && self.camel_grounded(mob);
            if landed || c.dash_ticks >= DASH_MAX_TICKS || rider(mob).is_none() {
                c.dashing = false;
                self.send_to_viewers(mob, camel_data(id, c));
            }
        }
        if c.dash_cooldown > 0 {
            c.dash_cooldown -= 1;
            if c.dash_cooldown == 0 && rider(mob).is_some() {
                let sound = format!("minecraft:entity.{}.dash_ready", mob.kind.sound());
                self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
            }
        }
    }

    /// Whether the camel stands on something anywhere under its width: its rider's
    /// client moves a steered camel, so the shared ground flag is not kept up for it.
    /// Blocks taller than a full block (fences, walls) are found one block down.
    fn camel_grounded(&self, mob: &Mob) -> bool {
        let (x, y, z) = (mob.pos[0], mob.pos[1], mob.pos[2]);
        let feet = (y - 0.05).floor() as i32;
        let offsets = [-HALF_WIDTH, 0.0, HALF_WIDTH];
        offsets.iter().any(|dx| {
            offsets.iter().any(|dz| {
                let (bx, bz) = ((x + dx).floor() as i32, (z + dz).floor() as i32);
                [feet, feet - 1].iter().any(|&by| {
                    self.loaded_block(BlockPos::new(bx, by, bz))
                        .is_some_and(|state| {
                            levitas_registry::collision_boxes(state).iter().any(|b| {
                                let top = f64::from(by) + f64::from(b[4]);
                                top >= y - 0.06 && top <= y + 0.06
                            })
                        })
                })
            })
        })
    }

    /// The rider of `mob` asked it to dash.
    fn dash(&mut self, id: i32, mob: &mut Mob, player: i32) -> bool {
        if !matches!(mob.kind, MobKind::Camel | MobKind::CamelHusk)
            || rider(mob) != Some(player)
            || !mob.horse.as_ref().is_some_and(|h| h.saddled)
            || mob.age < 0
        {
            return false;
        }
        let c = camel_mut(mob);
        if c.sitting || c.dashing || c.dash_cooldown > 0 {
            return false;
        }
        c.dashing = true;
        c.dash_ticks = 0;
        c.dash_cooldown = DASH_COOLDOWN;
        let packet = camel_data(id, c);
        self.send_to_viewers(mob, packet);
        let sound = format!("minecraft:entity.{}.dash", mob.kind.sound());
        self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
        true
    }

    /// A second player sits behind an adult camel's rider, if the seat is free.
    /// Feeding a camel its cactus and saddling it are left to `horses.rs`. Returns
    /// whether the click was taken as boarding.
    fn take_back_seat(&mut self, player: i32, target: i32, held: &str) -> bool {
        let Some(mob) = self.mobs.get(&target) else {
            return false;
        };
        let Some(front) = rider(mob) else {
            return false;
        };
        let saddled = mob.horse.as_ref().is_some_and(|h| h.saddled);
        let fed = mob.kind == MobKind::Camel
            && levitas_registry::item_has_tag(held, "minecraft:camel_food");
        let saddling = held == "minecraft:saddle" && !saddled;
        if mob.age < 0 || front == player || fed || saddling {
            return false;
        }
        if camel_of(mob).is_some_and(|c| c.back.is_some()) {
            return false;
        }
        let Some(p) = self.players.get(&player) else {
            return false;
        };
        if !p.alive || p.sneaking || p.riding.is_some() {
            return false;
        }
        let Some(mut mob) = self.mobs.remove(&target) else {
            return false;
        };
        camel_mut(&mut mob).back = Some(player);
        let at = mob.pos;
        let packet = passengers(target, Some(front), Some(player));
        self.players.get_mut(&player).expect("checked").riding = Some(target);
        self.mobs.insert(target, mob);
        self.broadcast(packet, None);
        self.move_player(player, Some(at), None, true);
        true
    }

    /// A rabbit's foot heals a hurt camel husk. Returns whether it was taken.
    fn feed_husk(&mut self, player: i32, target: i32, slot: usize) -> bool {
        let Some(mut mob) = self.mobs.remove(&target) else {
            return false;
        };
        let max = mob.max_health();
        if mob.health >= max {
            self.mobs.insert(target, mob);
            return false; // used as anything else is
        }
        mob.health = (mob.health + 2.0).min(max);
        self.send_to_viewers(&mob, health_data(target, mob.health));
        let sound = format!("minecraft:entity.{}.eat", mob.kind.sound());
        self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
        self.mobs.insert(target, mob);
        if self.players.get(&player).is_some_and(|p| p.survival()) {
            let p = self.players.get_mut(&player).expect("checked");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(player, update);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_players() -> (World, i32, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-camels-{}-{}",
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
        for cx in -2..=2 {
            for cz in -2..=2 {
                w.chunk(cx, cz);
            }
        }
        let mut ids = Vec::new();
        for (name, uuid) in [("Alice", 21), ("Bob", 22)] {
            let (out, frames) = tokio::sync::mpsc::channel(100_000);
            std::mem::forget(frames);
            let id = next_entity_id();
            let profile = Profile {
                name: name.to_owned(),
                uuid,
                properties: Vec::new(),
            };
            w.players
                .insert(id, Player::new(profile, out, [-20.5, -60.0, -20.5]));
            ids.push(id);
        }
        (w, ids[0], ids[1])
    }

    fn camel(w: &mut World, kind: MobKind) -> i32 {
        let mut mob = Mob::new(kind, [3.5, -60.0, 3.5], &mut w.rng);
        mob.age = 0;
        mob.set_on_ground(true);
        let id = next_entity_id();
        w.mobs.insert(id, mob);
        id
    }

    fn turn(w: &mut World, id: i32) -> bool {
        let mut mob = w.mobs.remove(&id).unwrap();
        let resting = tick(w, id, &mut mob);
        w.mobs.insert(id, mob);
        resting
    }

    fn state(w: &World, id: i32) -> Camel {
        camel_of(&w.mobs[&id]).cloned().expect("camel state")
    }

    fn set_state(w: &mut World, id: i32, f: impl FnOnce(&mut Camel)) {
        f(camel_mut(w.mobs.get_mut(&id).unwrap()));
    }

    /// Puts `item` in the player's main hand and returns that slot.
    fn hold(w: &mut World, id: i32, item: &str) -> usize {
        let p = w.players.get_mut(&id).unwrap();
        let slot = p.hand_slot(0);
        p.inventory.slots[slot] = Stack::new(ItemInfo::id_of(item).unwrap(), 1);
        slot
    }

    #[test]
    fn idle_camels_sit_a_while_and_stand_up_again() {
        let (mut w, _, _) = world_with_players();
        let id = camel(&mut w, MobKind::Camel);
        assert!(!turn(&mut w, id), "rolls its timer and walks on");
        set_state(&mut w, id, |c| c.pose_timer = Some(0));
        w.mobs.get_mut(&id).unwrap().target = Some(([9.0, 9.0], 40));
        assert!(turn(&mut w, id), "sits down");
        let c = state(&w, id);
        assert!(c.sitting && c.pose_tick == w.tick as i64);
        assert!(c.pose_value() < 0);
        let timer = c.pose_timer.unwrap();
        assert!((SIT_TICKS.0..=SIT_TICKS.1).contains(&timer));
        assert!(
            w.mobs[&id].target.is_none(),
            "a sitting camel does not wander"
        );
        for _ in 0..timer - 1 {
            assert!(turn(&mut w, id));
            assert!(state(&w, id).sitting);
        }
        assert!(turn(&mut w, id), "gets up");
        let c = state(&w, id);
        assert!(!c.sitting && c.rising == RISING_TICKS);
        for _ in 0..RISING_TICKS {
            assert!(turn(&mut w, id));
        }
        assert!(!turn(&mut w, id), "walks again");
        let t = state(&w, id).pose_timer.unwrap();
        assert!((STAND_TICKS.0 - 1..=STAND_TICKS.1).contains(&t));
    }

    #[test]
    fn a_tempted_camel_does_not_sit_down() {
        let (mut w, alice, _) = world_with_players();
        let id = camel(&mut w, MobKind::Camel);
        w.players.get_mut(&alice).unwrap().pos = [6.5, -60.0, 3.5];
        // Cactus in the off hand counts too.
        let p = w.players.get_mut(&alice).unwrap();
        p.inventory.slots[p.hand_slot(1)] =
            Stack::new(ItemInfo::id_of("minecraft:cactus").unwrap(), 1);
        set_state(&mut w, id, |c| c.pose_timer = Some(0));
        assert!(!turn(&mut w, id));
        assert!(!state(&w, id).sitting);
    }

    #[test]
    fn hurt_or_tempted_camels_get_up_unless_a_block_is_in_the_way() {
        let (mut w, alice, _) = world_with_players();
        let id = camel(&mut w, MobKind::Camel);
        set_state(&mut w, id, |c| {
            c.sitting = true;
            c.pose_timer = Some(1000);
        });
        let mut mob = w.mobs.remove(&id).unwrap();
        hurt(&mut w, id, &mut mob, None);
        w.mobs.insert(id, mob);
        let c = state(&w, id);
        assert!(!c.sitting && c.rising == 0, "up at once");
        assert!(c.pose_tick < w.tick as i64);

        // Under a low roof it stays down.
        set_state(&mut w, id, |c| c.sitting = true);
        let stone = BlockInfo::by_name("minecraft:stone").unwrap().default_state;
        w.set_block(BlockPos::new(3, -58, 3), stone);
        let mut mob = w.mobs.remove(&id).unwrap();
        hurt(&mut w, id, &mut mob, None);
        w.mobs.insert(id, mob);
        assert!(state(&w, id).sitting);
        w.set_block(BlockPos::new(3, -58, 3), 0);

        // Cactus held close by gets it up.
        w.players.get_mut(&alice).unwrap().pos = [6.5, -60.0, 3.5];
        hold(&mut w, alice, "minecraft:cactus");
        assert!(turn(&mut w, id));
        assert!(!state(&w, id).sitting);
    }

    #[test]
    fn a_saddled_camel_dashes_when_its_rider_jumps() {
        let (mut w, alice, _) = world_with_players();
        let id = camel(&mut w, MobKind::Camel);
        // Unsaddled: no dash.
        w.use_on_horse(alice, id, 0, "minecraft:air");
        assert_eq!(w.players[&alice].riding, Some(id));
        assert!(!rider_jumped(&mut w, alice, 100));
        w.get_off(alice);
        w.mobs.get_mut(&id).unwrap().horse.as_mut().unwrap().saddled = true;
        w.use_on_horse(alice, id, 0, "minecraft:air");
        assert!(rider_jumped(&mut w, alice, 100));
        let c = state(&w, id);
        assert!(c.dashing && c.dash_cooldown == DASH_COOLDOWN);
        assert!(!rider_jumped(&mut w, alice, 100), "not again so soon");
        // Up in the air it stays dashing, and lands.
        w.mobs.get_mut(&id).unwrap().pos[1] = -58.0;
        for _ in 0..DASH_LIFTOFF + 2 {
            turn(&mut w, id);
        }
        assert!(state(&w, id).dashing);
        w.mobs.get_mut(&id).unwrap().pos[1] = -60.0;
        turn(&mut w, id);
        assert!(!state(&w, id).dashing, "landed");
        assert!(!rider_jumped(&mut w, alice, 100), "still cooling down");
        for _ in 0..DASH_COOLDOWN {
            turn(&mut w, id);
        }
        assert_eq!(state(&w, id).dash_cooldown, 0);
        assert!(rider_jumped(&mut w, alice, 50));
    }

    #[test]
    fn a_dash_over_a_gap_lands_on_the_edges_and_never_sticks() {
        let (mut w, alice, _) = world_with_players();
        let id = camel(&mut w, MobKind::Camel);
        w.mobs.get_mut(&id).unwrap().horse.as_mut().unwrap().saddled = true;
        w.use_on_horse(alice, id, 0, "minecraft:air");
        // A one-block ditch under its middle.
        w.set_block(BlockPos::new(3, -61, 3), 0);
        assert!(rider_jumped(&mut w, alice, 100));
        for _ in 0..DASH_LIFTOFF {
            turn(&mut w, id);
        }
        assert!(!state(&w, id).dashing, "its edges stand on the ground");
        // High in the air, the dash still ends in time.
        for _ in 0..DASH_COOLDOWN {
            turn(&mut w, id);
        }
        w.mobs.get_mut(&id).unwrap().pos[1] = -50.0;
        assert!(rider_jumped(&mut w, alice, 100));
        for _ in 0..DASH_MAX_TICKS {
            turn(&mut w, id);
        }
        assert!(!state(&w, id).dashing);
    }

    #[test]
    fn a_second_player_rides_behind_and_takes_over() {
        let (mut w, alice, bob) = world_with_players();
        let id = camel(&mut w, MobKind::Camel);
        // No back seat without a rider.
        assert!(!use_on(&mut w, bob, id, 0, "minecraft:air"));
        w.use_on_horse(alice, id, 0, "minecraft:air");
        // Cactus feeds it, and a saddle saddles it, instead.
        assert!(!use_on(&mut w, bob, id, 0, "minecraft:cactus"));
        assert!(!use_on(&mut w, bob, id, 0, "minecraft:saddle"));
        // Unsaddled, it still carries two.
        assert!(use_on(&mut w, bob, id, 0, "minecraft:air"));
        assert_eq!(w.players[&bob].riding, Some(id));
        assert_eq!(state(&w, id).back, Some(bob));
        assert_eq!(spawn_extras(id, &w.mobs[&id]).len(), 1);
        // Carried along.
        w.mobs.get_mut(&id).unwrap().pos = [5.5, -60.0, 3.5];
        turn(&mut w, id);
        assert_eq!(w.players[&bob].pos, [5.5, -60.0, 3.5]);
        // The rider gets off: Bob steers now.
        w.get_off(alice);
        turn(&mut w, id);
        assert_eq!(w.mobs[&id].horse.as_ref().unwrap().rider, Some(bob));
        assert_eq!(state(&w, id).back, None);
        assert_eq!(w.players[&bob].riding, Some(id));
        // Bob gets off too.
        w.get_off(bob);
        turn(&mut w, id);
        assert!(w.mobs[&id].horse.as_ref().unwrap().rider.is_none());
    }

    #[test]
    fn a_back_seat_player_who_leaves_frees_the_seat() {
        let (mut w, alice, bob) = world_with_players();
        let id = camel(&mut w, MobKind::Camel);
        w.mobs.get_mut(&id).unwrap().horse.as_mut().unwrap().saddled = true;
        w.use_on_horse(alice, id, 0, "minecraft:air");
        assert!(use_on(&mut w, bob, id, 0, "minecraft:air"));
        w.get_off(bob);
        turn(&mut w, id);
        assert_eq!(state(&w, id).back, None);
        assert_eq!(w.mobs[&id].horse.as_ref().unwrap().rider, Some(alice));
    }

    #[test]
    fn sitting_is_saved_as_a_negative_pose_tick() {
        let (mut w, _, _) = world_with_players();
        let id = camel(&mut w, MobKind::Camel);
        set_state(&mut w, id, |c| {
            c.sitting = true;
            c.pose_tick = 1234;
        });
        let mut nbt = Compound::new();
        write_nbt(&w.mobs[&id], &mut nbt);
        assert_eq!(nbt.int("LastPoseTick"), Some(-1234));
        let mut back = Mob::new(MobKind::Camel, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        let c = camel_of(&back).unwrap();
        assert!(c.sitting && c.pose_tick == 1234);
        // Through the whole mob too.
        let full = w.mobs[&id].to_nbt();
        let back = Mob::read(&full, &mut w.rng).unwrap();
        assert!(camel_of(&back).unwrap().sitting);
        assert!(!spawn_extras(id, &back).is_empty());
    }

    #[test]
    fn camel_husks_eat_rabbit_feet_and_never_breed() {
        let (mut w, alice, bob) = world_with_players();
        let id = camel(&mut w, MobKind::CamelHusk);
        assert!(!may_love(&w, &w.mobs[&id]));
        let plain = Mob::new(MobKind::Camel, [0.0; 3], &mut w.rng);
        assert!(may_love(&w, &plain));
        // Cactus does not put it in love: it is ridden as with an empty hand.
        assert!(use_on(&mut w, alice, id, 0, "minecraft:cactus"));
        assert_eq!(w.mobs[&id].love, 0);
        assert_eq!(w.players[&alice].riding, Some(id));
        assert!(w.mobs[&id].persistent, "used by a player, it stays");
        // A second player with a rabbit's foot boards a husk at full health.
        let slot = hold(&mut w, bob, "minecraft:rabbit_foot");
        assert!(use_on(&mut w, bob, id, slot, "minecraft:rabbit_foot"));
        assert_eq!(state(&w, id).back, Some(bob));
        w.get_off(bob);
        w.get_off(alice);
        turn(&mut w, id);
        // A rabbit's foot heals a hurt husk, and is used up.
        let slot = hold(&mut w, alice, "minecraft:rabbit_foot");
        w.mobs.get_mut(&id).unwrap().health = 10.0;
        assert!(use_on(&mut w, alice, id, slot, "minecraft:rabbit_foot"));
        assert_eq!(w.mobs[&id].health, 12.0);
        assert!(w.players[&alice].inventory.slots[slot].is_empty());
        // At full health, with nobody aboard, it is left to the shared code.
        let full = w.mobs[&id].max_health();
        w.mobs.get_mut(&id).unwrap().health = full;
        assert!(!use_on(&mut w, alice, id, slot, "minecraft:rabbit_foot"));
        assert!(!MobKind::CamelHusk.burns_in_daylight());
    }
}
