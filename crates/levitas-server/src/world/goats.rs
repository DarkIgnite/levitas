//! Goats: screaming goats, horns, ramming and milking.
//!
//! Behaviour follows the Minecraft Wiki (Goat, Goat Horn):
//! - One goat in fifty is a screaming goat: it looks the same but makes its own sounds
//!   (`minecraft:entity.goat.screaming.*`) and rams far more often. A baby takes after
//!   a parent picked at random: a screaming parent always passes it on, a normal one
//!   gives the usual one in fifty.
//! - Goats are born with both horns; an adult that spawns has a one in ten chance of
//!   missing one of them, left or right alike. Every 30 seconds to 5 minutes (5 to 15
//!   seconds for a screaming goat) a goat picks something standing still that it can
//!   see 4 to 16 blocks away: a player not in creative or spectator (never on
//!   Peaceful) or any mob but goats and ghasts. Players are watched for a tick to see
//!   that they stand still. It locks on to where that was, lowers its head and stomps,
//!   backing off a little, then charges. A charge that connects deals 2 (3 on Hard;
//!   half for a baby) and knocks the target about 9 blocks away (half as far for a
//!   baby); one that misses runs a couple of blocks past and stops, unless a block
//!   stops it first. Wheat held close by calls off a ram being prepared.
//! - An adult that charges into a block of `minecraft:snaps_goat_horn` loses one of its
//!   horns, which drops as a goat horn: one of the four regular instruments, or one of
//!   the four screaming ones for a screaming goat; both horns of a goat play the same.
//! - A bucket used on an adult gives a milk bucket.
//! - Saved as vanilla: `IsScreamingGoat`, `HasLeftHorn`, `HasRightHorn`.

use levitas_nbt::Compound;

use super::mobs::{yaw_towards, Mob, MobKind};
use super::*;
use crate::rules::Difficulty;
use crate::world::survival::Damage;

const DATA_SCREAMING: u8 = 18;
const DATA_LEFT_HORN: u8 = 19;
const DATA_RIGHT_HORN: u8 = 20;
const SERIALIZER_BOOL: i32 = 8;
/// Entity events: lowering the head to ram, and raising it again.
const EVENT_LOWER_HEAD: i8 = 58;
const EVENT_RAISE_HEAD: i8 = 59;
/// One goat in this many is a screaming goat.
const SCREAMING_ONE_IN: u32 = 50;
/// One adult in this many spawns missing a horn.
const ONE_HORN_ONE_IN: u32 = 10;
/// Game mode 3; players in it (and in creative) are never rammed.
const GAME_MODE_SPECTATOR: u8 = 3;
/// Ticks between rams: 30 seconds to 5 minutes, or 5 to 15 seconds when screaming.
const COOLDOWN: (u32, u32) = (600, 6000);
const SCREAMING_COOLDOWN: (u32, u32) = (100, 300);
/// Ticks before looking again when nothing was there to ram.
const RETRY: u32 = 20;
/// How close and how far a target may be.
const RAM_MIN: f64 = 4.0;
const RAM_MAX: f64 = 16.0;
/// Only targets slower than this (blocks per tick) count as standing still.
const STILL: f64 = 0.03;
/// Ticks of stomping with the head down before the charge.
const PREPARE_TICKS: u32 = 20;
/// Backing off while stomping, in blocks per tick.
const BACK_OFF: f64 = 0.03;
/// Charging speed in blocks per tick, how far past the target a goat that missed runs,
/// and the longest a charge lasts.
const RAM_SPEED: f64 = 0.4;
const OVERSHOOT: f64 = 2.0;
const MAX_CHARGE_TICKS: u32 = 60;
/// Sideways and upward push of a hit: about 9 blocks of flight for an adult.
const RAM_PUSH: f64 = 0.95;
const RAM_LIFT: f64 = 0.4;
/// Players within this many blocks holding wheat call off a ram being prepared.
const TEMPT_RANGE: f64 = 10.0;
const SNAPS_HORN: &str = "minecraft:snaps_goat_horn";
const PLAYER_SIZE: (f64, f64) = (0.6, 1.8);

/// What the module keeps for a goat, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq)]
struct Goat {
    screaming: bool,
    left_horn: bool,
    right_horn: bool,
    /// Ticks until it next looks for something to ram, rolled on its first turn.
    cooldown: Option<u32>,
    ram: Ram,
}

impl Goat {
    fn new(screaming: bool) -> Goat {
        Goat {
            screaming,
            left_horn: true,
            right_horn: true,
            cooldown: None,
            ram: Ram::Idle,
        }
    }

    fn sound(&self, what: &str) -> String {
        if self.screaming {
            format!("minecraft:entity.goat.screaming.{what}")
        } else {
            format!("minecraft:entity.goat.{what}")
        }
    }
}

/// What a goat rams.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Victim {
    Player(i32),
    Mob(i32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ram {
    Idle,
    /// Watching a victim for a tick to see that it stands still at `at`.
    Eyeing {
        victim: Victim,
        at: [f64; 3],
    },
    /// Head down and stomping, aimed at where the victim was.
    Preparing {
        victim: Victim,
        at: [f64; 3],
        ticks: u32,
    },
    /// Running at it along `dir` (a unit vector in x and z), `left` blocks still to go.
    Charging {
        victim: Victim,
        dir: [f64; 2],
        left: f64,
        ticks: u32,
    },
}

fn roll_cooldown(rng: &mut Rng, screaming: bool) -> u32 {
    let (lo, hi) = if screaming {
        SCREAMING_COOLDOWN
    } else {
        COOLDOWN
    };
    lo + rng.below(hi - lo + 1)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn goat_of(mob: &Mob) -> Option<&Goat> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Goat>())
}

/// A goat that comes into the world other than by breeding: one in fifty screams, and
/// an adult may be missing a horn.
fn fresh_goat(rng: &mut Rng, adult: bool) -> Goat {
    let mut g = Goat::new(rng.below(SCREAMING_ONE_IN) == 0);
    if adult && rng.below(ONE_HORN_ONE_IN) == 0 {
        if rng.below(2) == 0 {
            g.left_horn = false;
        } else {
            g.right_horn = false;
        }
    }
    g
}

/// The goat bred from `a` and `b`: it takes after one of them, picked at random. A
/// screaming parent passes screaming on; a normal one gives the usual one in fifty.
fn offspring(rng: &mut Rng, a: &Mob, b: &Mob) -> Goat {
    let parent = if rng.below(2) == 0 { a } else { b };
    let screaming =
        goat_of(parent).is_some_and(|g| g.screaming) || rng.below(SCREAMING_ONE_IN) == 0;
    Goat::new(screaming)
}

/// Gives a goat bred from `a` and `b` its state, before it is spawned and shown.
/// Breeding calls it when the baby is a goat.
pub(super) fn bred_baby(rng: &mut Rng, a: &Mob, b: &Mob, baby: &mut Mob) {
    baby.kin = Some(Box::new(offspring(rng, a, b)));
}

/// The sound name a goat uses for its ambient, hurt and death sounds in place of
/// `MobKind::sound()`: screaming goats have their own.
pub(super) fn sound_name(mob: &Mob) -> Option<&'static str> {
    goat_of(mob)
        .filter(|g| g.screaming)
        .map(|_| "goat.screaming")
}

fn goat_data(id: i32, g: &Goat) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_SCREAMING)
        .varint(SERIALIZER_BOOL)
        .bool(g.screaming)
        .u8(DATA_LEFT_HORN)
        .varint(SERIALIZER_BOOL)
        .bool(g.left_horn)
        .u8(DATA_RIGHT_HORN)
        .varint(SERIALIZER_BOOL)
        .bool(g.right_horn)
        .u8(0xFF)
}

/// Damage of a ram that connects, by age and difficulty.
fn ram_damage(baby: bool, difficulty: Difficulty) -> f32 {
    let hard = difficulty == Difficulty::Hard;
    match (baby, hard) {
        (false, false) => 2.0,
        (false, true) => 3.0,
        (true, false) => 1.0,
        (true, true) => 1.5,
    }
}

/// The instrument registry IDs a goat's horns may play.
fn horn_instruments(screaming: bool) -> &'static [i64] {
    let tag = if screaming {
        "minecraft:screaming_goat_horns"
    } else {
        "minecraft:regular_goat_horns"
    };
    levitas_registry::TagRegistry::get("minecraft:instrument").map_or(&[], |t| t.members(tag))
}

/// The goat horn a goat sheds: the same instrument for both of its horns.
fn horn_stack(mob: &Mob, screaming: bool) -> Option<Stack> {
    let item = ItemInfo::id_of("minecraft:goat_horn")?;
    let mut stack = Stack::new(item, 1);
    let options = horn_instruments(screaming);
    if !options.is_empty() {
        let pick = (mob.uuid % options.len() as u128) as usize;
        stack.instrument = Some(options[pick] as u8);
    }
    Some(stack)
}

pub(super) fn spawned(w: &mut World, mob: &mut Mob, _biome: &str) {
    mob.kin = Some(Box::new(fresh_goat(&mut w.rng, mob.age >= 0)));
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if mob.kin.is_none() {
        // A goat from a spawn egg (or bred without `bred_baby`): rolled on its first turn.
        let g = fresh_goat(&mut w.rng, mob.age >= 0);
        if g.screaming || !g.left_horn || !g.right_horn {
            w.send_to_viewers(mob, goat_data(id, &g));
        }
        mob.kin = Some(Box::new(g));
    }
    let Some(mut kin) = mob.kin.take() else {
        return false;
    };
    let busy = match kin.downcast_mut::<Goat>() {
        Some(g) => w.tick_goat(id, mob, g),
        None => false,
    };
    mob.kin = Some(kin);
    busy
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    if held != "minecraft:bucket" {
        return false;
    }
    let Some(mob) = w.mobs.get(&target) else {
        return false;
    };
    if mob.age < 0 {
        return false;
    }
    let screaming = goat_of(mob).is_some_and(|g| g.screaming);
    let pos = mob.pos;
    let sound = if screaming {
        "minecraft:entity.goat.screaming.milk"
    } else {
        "minecraft:entity.goat.milk"
    };
    w.play_sound_at(sound, pos, 1.0, 1.0, None);
    w.exchange_held(player, slot, "minecraft:milk_bucket");
    true
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, _attacker: Option<i32>) {
    // A goat that is hurt forgets its ram (it does not hit back).
    let Some(mut kin) = mob.kin.take() else {
        return;
    };
    if let Some(g) = kin.downcast_mut::<Goat>() {
        if g.ram != Ram::Idle {
            w.stop_ram(id, mob, g);
        }
    }
    mob.kin = Some(kin);
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    match goat_of(mob) {
        Some(g) if g.screaming || !g.left_horn || !g.right_horn => vec![goat_data(id, g)],
        _ => Vec::new(),
    }
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let default = Goat::new(false);
    let g = goat_of(mob).unwrap_or(&default);
    nbt.insert("IsScreamingGoat", g.screaming);
    nbt.insert("HasLeftHorn", g.left_horn);
    nbt.insert("HasRightHorn", g.right_horn);
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let flag = |name: &str, default: bool| nbt.int(name).map_or(default, |v| v != 0);
    let mut g = Goat::new(flag("IsScreamingGoat", false));
    g.left_horn = flag("HasLeftHorn", true);
    g.right_horn = flag("HasRightHorn", true);
    mob.kin = Some(Box::new(g));
}

impl World {
    /// A goat's turn. Returns whether it is busy ramming (and moves itself).
    fn tick_goat(&mut self, id: i32, mob: &mut Mob, g: &mut Goat) -> bool {
        let screaming = g.screaming;
        let cooldown = g
            .cooldown
            .get_or_insert_with(|| roll_cooldown(&mut self.rng, screaming));
        let distracted = mob.panic > 0 || mob.love > 0 || mob.vehicle.is_some();
        match g.ram {
            Ram::Idle => {
                if distracted {
                    return false;
                }
                if *cooldown > 0 {
                    *cooldown -= 1;
                    return false;
                }
                let Some((victim, at)) = self.pick_victim(mob) else {
                    *cooldown = RETRY;
                    return false;
                };
                g.ram = Ram::Eyeing { victim, at };
                mob.target = None;
                mob.path.clear();
                true
            }
            Ram::Eyeing { victim, at } => {
                let still = self.victim_at(victim).is_some_and(|(now, _)| {
                    (now[0] - at[0]).hypot(now[2] - at[2]) < STILL
                        && (RAM_MIN..=RAM_MAX).contains(&distance(now, mob.pos))
                });
                if distracted || !still {
                    // It moved: look again soon, without lowering the head.
                    g.ram = Ram::Idle;
                    *cooldown = RETRY;
                    return false;
                }
                g.ram = Ram::Preparing {
                    victim,
                    at,
                    ticks: PREPARE_TICKS,
                };
                mob.target = None;
                mob.path.clear();
                self.send_to_viewers(mob, entity_event(id, EVENT_LOWER_HEAD));
                let pitch = if mob.age < 0 { 1.5 } else { 1.0 };
                self.play_sound_at(&g.sound("prepare_ram"), mob.pos, 1.0, pitch, None);
                true
            }
            Ram::Preparing { victim, at, ticks } => {
                if distracted || self.victim_at(victim).is_none() || self.tempted(mob.pos) {
                    self.stop_ram(id, mob, g);
                    return false;
                }
                let (dx, dz) = (at[0] - mob.pos[0], at[2] - mob.pos[2]);
                let length = dx.hypot(dz).max(1e-6);
                let dir = [dx / length, dz / length];
                mob.yaw = yaw_towards(mob.pos, at);
                mob.head_yaw = mob.yaw;
                mob.target = None;
                mob.path.clear();
                self.move_goat(mob, [-dir[0] * BACK_OFF, -dir[1] * BACK_OFF]);
                g.ram = if ticks > 1 {
                    Ram::Preparing {
                        victim,
                        at,
                        ticks: ticks - 1,
                    }
                } else {
                    let length = (at[0] - mob.pos[0]).hypot(at[2] - mob.pos[2]);
                    Ram::Charging {
                        victim,
                        dir,
                        left: length + OVERSHOOT,
                        ticks: MAX_CHARGE_TICKS,
                    }
                };
                true
            }
            Ram::Charging {
                victim,
                dir,
                left,
                ticks,
            } => {
                if mob.panic > 0 || mob.vehicle.is_some() {
                    self.stop_ram(id, mob, g);
                    return false;
                }
                mob.yaw = yaw_towards(
                    mob.pos,
                    [mob.pos[0] + dir[0], mob.pos[1], mob.pos[2] + dir[1]],
                );
                mob.head_yaw = mob.yaw;
                mob.target = None;
                mob.path.clear();
                let step = RAM_SPEED.min(left.max(0.0));
                let (moved, blocked) = self.move_goat(mob, [dir[0] * step, dir[1] * step]);
                if self.ram_connects(mob, victim) {
                    self.ram_hit(id, mob, g, victim, dir);
                    self.stop_ram(id, mob, g);
                } else if blocked {
                    self.ram_bump(id, mob, g, dir);
                    self.stop_ram(id, mob, g);
                } else if left - moved <= 1e-3 || ticks <= 1 {
                    self.stop_ram(id, mob, g);
                } else {
                    g.ram = Ram::Charging {
                        victim,
                        dir,
                        left: left - moved,
                        ticks: ticks - 1,
                    };
                }
                true
            }
        }
    }

    /// The ram is over: head up, and a new wait before the next.
    fn stop_ram(&mut self, id: i32, mob: &Mob, g: &mut Goat) {
        g.ram = Ram::Idle;
        g.cooldown = Some(roll_cooldown(&mut self.rng, g.screaming));
        self.send_to_viewers(mob, entity_event(id, EVENT_RAISE_HEAD));
    }

    /// Something to ram: standing still, in sight, 4 to 16 blocks away, picked at
    /// random among all that qualify, without collecting them.
    fn pick_victim(&mut self, mob: &Mob) -> Option<(Victim, [f64; 3])> {
        let eyes = [mob.pos[0], mob.pos[1] + mob.size().1 * 0.85, mob.pos[2]];
        let in_range = |at: [f64; 3]| (RAM_MIN..=RAM_MAX).contains(&distance(at, mob.pos));
        let mut rng = Rng::new(self.rng.next_u64());
        let mut seen = 0u32;
        let mut pick = None;
        let mut offer = |victim: Victim, at: [f64; 3]| {
            seen += 1;
            if rng.below(seen) == 0 {
                pick = Some((victim, at));
            }
        };
        if crate::rules::difficulty() != Difficulty::Peaceful {
            for (&pid, p) in &self.players {
                let body = [p.pos[0], p.pos[1] + PLAYER_SIZE.1 / 2.0, p.pos[2]];
                if p.alive
                    && !matches!(p.mode, GAME_MODE_CREATIVE | GAME_MODE_SPECTATOR)
                    && in_range(p.pos)
                    && self.line_of_sight(eyes, body)
                {
                    offer(Victim::Player(pid), p.pos);
                }
            }
        }
        for (&mid, m) in &self.mobs {
            if m.alive()
                && !matches!(m.kind, MobKind::Goat | MobKind::Ghast)
                && m.vel[0].hypot(m.vel[2]) < STILL
                && in_range(m.pos)
                && self.line_of_sight(eyes, [m.pos[0], m.pos[1] + m.size().1 / 2.0, m.pos[2]])
            {
                offer(Victim::Mob(mid), m.pos);
            }
        }
        pick
    }

    /// Where a victim is now, and its width and height, if it is still about.
    fn victim_at(&self, victim: Victim) -> Option<([f64; 3], (f64, f64))> {
        match victim {
            Victim::Player(pid) => self
                .players
                .get(&pid)
                .filter(|p| p.alive)
                .map(|p| (p.pos, PLAYER_SIZE)),
            Victim::Mob(mid) => self
                .mobs
                .get(&mid)
                .filter(|m| m.alive())
                .map(|m| (m.pos, m.size())),
        }
    }

    /// Whether a player close by holds wheat, which calls a goat off its ram.
    fn tempted(&self, at: [f64; 3]) -> bool {
        self.players.values().any(|p| {
            p.alive
                && distance(p.pos, at) <= TEMPT_RANGE
                && ItemInfo::get(p.inventory.held(p.held).item)
                    .is_some_and(|i| levitas_registry::item_has_tag(i.name, "minecraft:goat_food"))
        })
    }

    /// Whether a charging goat has reached its victim.
    fn ram_connects(&self, mob: &Mob, victim: Victim) -> bool {
        let Some((at, (width, height))) = self.victim_at(victim) else {
            return false;
        };
        let (gw, gh) = mob.size();
        let reach = (gw + width) / 2.0 + 0.2;
        (at[0] - mob.pos[0]).abs() < reach
            && (at[2] - mob.pos[2]).abs() < reach
            && at[1] < mob.pos[1] + gh
            && mob.pos[1] < at[1] + height
    }

    /// The charge connects: damage and a big push along the charge.
    fn ram_hit(&mut self, id: i32, mob: &Mob, g: &Goat, victim: Victim, dir: [f64; 2]) {
        let baby = mob.age < 0;
        let difficulty = crate::rules::difficulty();
        let damage = ram_damage(baby, difficulty);
        let force = if baby { RAM_PUSH / 2.0 } else { RAM_PUSH };
        let push = [dir[0] * force, RAM_LIFT, dir[1] * force];
        match victim {
            Victim::Player(pid) => {
                if difficulty != Difficulty::Peaceful
                    && self.damage_from(pid, damage, Damage::Mob("Goat"), Some(id))
                {
                    self.push_player(pid, push);
                }
            }
            Victim::Mob(mid) => {
                self.hit_mob(mid, damage, None, [0.0, 0.0], "minecraft:mob_attack");
                if let Some(m) = self.mobs.get_mut(&mid).filter(|m| m.alive()) {
                    m.push(push);
                }
            }
        }
        let pitch = if baby { 1.5 } else { 1.0 };
        self.play_sound_at(&g.sound("ram_impact"), mob.pos, 1.0, pitch, None);
    }

    /// The charge ran into a block: one that snaps goat horns costs an adult a horn.
    fn ram_bump(&mut self, id: i32, mob: &Mob, g: &mut Goat, dir: [f64; 2]) {
        if mob.age < 0 || !(g.left_horn || g.right_horn) {
            return;
        }
        let (width, _) = mob.size();
        let ahead = width / 2.0 + 0.5;
        let (x, z) = (
            (mob.pos[0] + dir[0] * ahead).floor() as i32,
            (mob.pos[2] + dir[1] * ahead).floor() as i32,
        );
        let feet = mob.pos[1].floor() as i32;
        let snaps = (0..2).any(|dy| {
            self.loaded_block(BlockPos::new(x, feet + dy, z))
                .is_some_and(|state| block_has_tag(state, SNAPS_HORN))
        });
        if !snaps {
            return;
        }
        let left = if g.left_horn && g.right_horn {
            self.rng.below(2) == 0
        } else {
            g.left_horn
        };
        if left {
            g.left_horn = false;
        } else {
            g.right_horn = false;
        }
        if let Some(horn) = horn_stack(mob, g.screaming) {
            self.scatter_item(mob.pos, horn);
        }
        self.play_sound_at("minecraft:entity.goat.horn_break", mob.pos, 1.0, 1.0, None);
        self.send_to_viewers(mob, goat_data(id, g));
    }

    /// Moves a ramming goat by `by` in x and z, blocks allowing, with gravity. Returns
    /// how far it got and whether a block stopped it.
    fn move_goat(&mut self, mob: &mut Mob, by: [f64; 2]) -> (f64, bool) {
        let (width, height) = mob.size();
        let half = width / 2.0;
        let mut pos = mob.pos;
        let dy = self.clip_move(pos, half, height, 1, mob.vel[1]);
        pos[1] += dy;
        if (dy - mob.vel[1]).abs() > 1e-9 {
            mob.vel[1] = 0.0;
        }
        mob.vel[1] = (mob.vel[1] - 0.08) * 0.98;
        let mut blocked = false;
        let mut moved = [0.0; 2];
        for (i, axis) in [0, 2].into_iter().enumerate() {
            let d = self.clip_move(pos, half, height, axis, by[i]);
            pos[axis] += d;
            moved[i] = d;
            if (d - by[i]).abs() > 1e-9 {
                blocked = true;
            }
        }
        mob.vel[0] = moved[0];
        mob.vel[2] = moved[1];
        mob.pos = pos;
        (moved[0].hypot(moved[1]), blocked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-goats-{}-{}",
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
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 11,
            properties: Vec::new(),
        };
        // Far away, so that only what a test puts close is rammed.
        w.players
            .insert(alice, Player::new(profile, out, [-28.5, -60.0, -28.5]));
        (w, alice)
    }

    fn goat(w: &mut World, at: [f64; 3]) -> (i32, Mob) {
        let mut mob = Mob::new(MobKind::Goat, at, &mut w.rng);
        let mut g = Goat::new(false);
        g.cooldown = Some(0);
        mob.kin = Some(Box::new(g));
        (next_entity_id(), mob)
    }

    fn state(mob: &Mob) -> &Goat {
        goat_of(mob).expect("goat state")
    }

    fn hold(w: &mut World, id: i32, item: &str) {
        w.players.get_mut(&id).unwrap().inventory.slots[36] =
            Stack::new(ItemInfo::id_of(item).unwrap(), 1);
    }

    #[test]
    fn one_goat_in_fifty_screams_and_one_in_ten_lacks_a_horn() {
        let (mut w, _) = world_with_player();
        let (mut screaming, mut left, mut right) = (0, 0, 0);
        for _ in 0..10_000 {
            let mut mob = Mob::new(MobKind::Goat, [0.5, -60.0, 0.5], &mut w.rng);
            spawned(&mut w, &mut mob, "minecraft:snowy_slopes");
            let g = state(&mob);
            assert!(g.left_horn || g.right_horn, "never both lost");
            screaming += usize::from(g.screaming);
            left += usize::from(!g.left_horn);
            right += usize::from(!g.right_horn);
        }
        assert!((120..=280).contains(&screaming), "{screaming}");
        assert!((380..=620).contains(&left), "{left}");
        assert!((380..=620).contains(&right), "{right}");
        // Babies always have both horns.
        for _ in 0..200 {
            let mut mob = Mob::new(MobKind::Goat, [0.5, -60.0, 0.5], &mut w.rng);
            mob.age = -24_000;
            spawned(&mut w, &mut mob, "minecraft:snowy_slopes");
            assert!(state(&mob).left_horn && state(&mob).right_horn);
        }
        // Goats from elsewhere roll on their first turn.
        let mut mob = Mob::new(MobKind::Goat, [0.5, -60.0, 0.5], &mut w.rng);
        tick(&mut w, 1, &mut mob);
        assert!(goat_of(&mob).is_some());
    }

    #[test]
    fn goats_are_saved_and_read_back_as_vanilla() {
        let (mut w, _) = world_with_player();
        let mut mob = Mob::new(MobKind::Goat, [0.5, -60.0, 0.5], &mut w.rng);
        let mut g = Goat::new(true);
        g.left_horn = false;
        mob.kin = Some(Box::new(g));
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        assert_eq!(nbt.int("IsScreamingGoat"), Some(1));
        assert_eq!(nbt.int("HasLeftHorn"), Some(0));
        assert_eq!(nbt.int("HasRightHorn"), Some(1));
        let mut back = Mob::new(MobKind::Goat, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        let g = state(&back);
        assert!(g.screaming && !g.left_horn && g.right_horn);
        // A goat saved without these tags has both horns and does not scream.
        let mut bare = Mob::new(MobKind::Goat, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut bare, &Compound::new());
        assert_eq!(*state(&bare), Goat::new(false));
        assert_eq!(spawn_extras(1, &back).len(), 1);
        assert!(spawn_extras(1, &bare).is_empty());
    }

    #[test]
    fn adults_are_milked_with_a_bucket() {
        let (mut w, alice) = world_with_player();
        let (id, mob) = goat(&mut w, [-26.5, -60.0, -28.5]);
        w.mobs.insert(id, mob);
        hold(&mut w, alice, "minecraft:bucket");
        assert!(use_on(&mut w, alice, id, 36, "minecraft:bucket"));
        let held = w.players[&alice].inventory.slots[36].item;
        assert_eq!(ItemInfo::get(held).unwrap().name, "minecraft:milk_bucket");
        let (baby, mut mob) = goat(&mut w, [-26.5, -60.0, -26.5]);
        mob.age = -24_000;
        w.mobs.insert(baby, mob);
        hold(&mut w, alice, "minecraft:bucket");
        assert!(!use_on(&mut w, alice, baby, 36, "minecraft:bucket"));
        assert!(!use_on(&mut w, alice, id, 36, "minecraft:wheat"));
    }

    #[test]
    fn a_goat_rams_a_mob_standing_still() {
        let (mut w, _) = world_with_player();
        let (id, mut mob) = goat(&mut w, [0.5, -60.0, 0.5]);
        let cow = next_entity_id();
        w.mobs
            .insert(cow, Mob::new(MobKind::Cow, [8.5, -60.0, 0.5], &mut w.rng));
        let full = w.mobs[&cow].health;
        let mut charged = false;
        for _ in 0..120 {
            assert!(tick(&mut w, id, &mut mob) || charged);
            if matches!(state(&mob).ram, Ram::Charging { .. }) {
                charged = true;
            }
            if charged && state(&mob).ram == Ram::Idle {
                break;
            }
        }
        assert!(charged, "charged at the cow");
        assert_eq!(state(&mob).ram, Ram::Idle);
        assert!(w.mobs[&cow].health < full, "the ram connected");
        assert!(
            w.mobs[&cow].knockback[0] > 0.3,
            "knocked away from the goat"
        );
        assert!(state(&mob).cooldown.unwrap() >= COOLDOWN.0);
        assert!(state(&mob).left_horn && state(&mob).right_horn);
    }

    #[test]
    fn a_goat_rams_survival_players_but_not_too_close_ones() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = goat(&mut w, [-28.5, -60.0, -18.5]);
        let full = w.players[&alice].health;
        for _ in 0..120 {
            tick(&mut w, id, &mut mob);
            if w.players[&alice].health < full {
                break;
            }
        }
        assert!(w.players[&alice].health < full, "rammed");
        // Nothing within four blocks is a target.
        let (id, mut close) = goat(&mut w, [-26.5, -60.0, -28.5]);
        assert!(!tick(&mut w, id, &mut close));
        assert_eq!(state(&close).ram, Ram::Idle);
    }

    #[test]
    fn babies_take_after_a_parent() {
        let mut rng = Rng::new(7);
        let mut parent = |screaming: bool| {
            let mut mob = Mob::new(MobKind::Goat, [0.5, -60.0, 0.5], &mut rng);
            mob.kin = Some(Box::new(Goat::new(screaming)));
            mob
        };
        let (loud, quiet) = (parent(true), parent(false));
        let mut rng = Rng::new(9);
        let count = |rng: &mut Rng, a: &Mob, b: &Mob| {
            (0..10_000)
                .filter(|_| offspring(rng, a, b).screaming)
                .count()
        };
        assert_eq!(count(&mut rng, &loud, &loud), 10_000);
        let mixed = count(&mut rng, &loud, &quiet);
        assert!((4_800..=5_400).contains(&mixed), "{mixed}");
        let normal = count(&mut rng, &quiet, &quiet);
        assert!((120..=280).contains(&normal), "{normal}");
        let mut baby = Mob::new(MobKind::Goat, [0.5, -60.0, 0.5], &mut rng);
        bred_baby(&mut rng, &loud, &loud, &mut baby);
        let g = state(&baby);
        assert!(g.screaming && g.left_horn && g.right_horn);
        assert_eq!(sound_name(&baby), Some("goat.screaming"));
        assert_eq!(sound_name(&quiet), None);
    }

    #[test]
    fn players_on_the_move_and_creative_ones_are_left_alone() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = goat(&mut w, [-28.5, -60.0, -18.5]);
        assert!(tick(&mut w, id, &mut mob));
        assert!(matches!(state(&mob).ram, Ram::Eyeing { .. }));
        w.players.get_mut(&alice).unwrap().pos[0] += 0.2;
        assert!(!tick(&mut w, id, &mut mob));
        assert_eq!(state(&mob).ram, Ram::Idle);
        assert_eq!(state(&mob).cooldown, Some(RETRY));
        // Creative players are never picked; adventure ones are.
        w.players.get_mut(&alice).unwrap().mode = GAME_MODE_CREATIVE;
        let (id, mut mob) = goat(&mut w, [-28.5, -60.0, -18.5]);
        assert!(!tick(&mut w, id, &mut mob));
        w.players.get_mut(&alice).unwrap().mode = 2;
        let (id, mut mob) = goat(&mut w, [-28.5, -60.0, -18.5]);
        assert!(tick(&mut w, id, &mut mob));
        assert!(tick(&mut w, id, &mut mob));
        assert!(matches!(state(&mob).ram, Ram::Preparing { .. }));
    }

    #[test]
    fn wheat_calls_off_a_ram() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = goat(&mut w, [-28.5, -60.0, -18.5]);
        assert!(tick(&mut w, id, &mut mob));
        assert!(tick(&mut w, id, &mut mob));
        assert!(matches!(state(&mob).ram, Ram::Preparing { .. }));
        hold(&mut w, alice, "minecraft:wheat");
        assert!(!tick(&mut w, id, &mut mob));
        assert_eq!(state(&mob).ram, Ram::Idle);
    }

    #[test]
    fn charging_into_stone_snaps_off_a_horn() {
        let (mut w, _) = world_with_player();
        for y in -60..=-58 {
            for z in -2..=2 {
                w.set_block(BlockPos::new(4, y, z), block::STONE);
            }
        }
        let (id, mut mob) = goat(&mut w, [0.5, -60.0, 0.5]);
        if let Some(g) = mob.kin.as_mut().and_then(|k| k.downcast_mut::<Goat>()) {
            g.ram = Ram::Charging {
                victim: Victim::Player(-1),
                dir: [1.0, 0.0],
                left: 10.0,
                ticks: MAX_CHARGE_TICKS,
            };
        }
        for _ in 0..30 {
            tick(&mut w, id, &mut mob);
            if state(&mob).ram == Ram::Idle {
                break;
            }
        }
        let g = state(&mob);
        assert_eq!(g.ram, Ram::Idle, "stopped at the wall");
        assert!(g.left_horn != g.right_horn, "one horn lost");
        let horn = w
            .items
            .values()
            .find(|i| ItemInfo::get(i.stack().item).unwrap().name == "minecraft:goat_horn")
            .expect("a goat horn dropped");
        let instrument = i64::from(horn.stack().instrument.expect("plays something"));
        assert!(horn_instruments(false).contains(&instrument));
        assert!(!horn_instruments(true).contains(&instrument));
        assert_eq!(horn_instruments(false).len(), 4);
    }

    #[test]
    fn screaming_goats_ram_more_often_and_hit_by_difficulty() {
        let mut rng = Rng::new(1);
        for _ in 0..100 {
            let c = roll_cooldown(&mut rng, true);
            assert!((100..=300).contains(&c));
            let c = roll_cooldown(&mut rng, false);
            assert!((600..=6000).contains(&c));
        }
        assert_eq!(ram_damage(false, Difficulty::Normal), 2.0);
        assert_eq!(ram_damage(false, Difficulty::Hard), 3.0);
        assert_eq!(ram_damage(true, Difficulty::Easy), 1.0);
        assert_eq!(
            Goat::new(true).sound("prepare_ram"),
            "minecraft:entity.goat.screaming.prepare_ram"
        );
    }
}
