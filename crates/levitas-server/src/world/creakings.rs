//! Creakings and creaking hearts.
//!
//! Behaviour follows the Minecraft Wiki (Creaking, Creaking Heart, Resin Clump):
//! - A creaking heart between two pale oak logs or wood blocks (stripped or not) lying
//!   on the heart's own axis is dormant by day and awake at night (12600 to 23400 ticks
//!   of the day) in the Overworld; without them it is uprooted. Natural and crafted
//!   hearts behave alike: `natural` only decides the 20 to 24 experience a broken heart
//!   gives in survival.
//! - An awake heart with no creaking of its own looks for a spot in the 33x17x33 box
//!   around it: a block with a sturdy top that is not leaves below, two blocks without
//!   collision and three without liquid above, and no mob or player in the way. The
//!   creaking it makes is bound to it: the heart's position is the creaking's
//!   `home_pos`, saved as vanilla does (with the other mobs' homes, in `critters.rs`).
//!   Should a heart ever have two (one came back with its chunk), the extra one vanishes.
//! - At dawn a heart makes its creaking vanish (unless it was named) and turns dormant.
//!   A bound creaking more than 34 blocks from its heart vanishes, and so does one that
//!   shares a block with a player for more than five seconds; the heart then makes a new
//!   one. While a heart has its creaking, it stays awake (or dormant) without its logs.
//! - A creaking watched by a player in survival or adventure (looking within 60 degrees
//!   of it, both ways, with nothing solid between) cannot move or attack; a carved pumpkin
//!   on the player's head lets it move. A player who sees it within 12 blocks alerts
//!   it: it then chases the nearest such player while unwatched, quicker than a
//!   vindicator, and hits for 2.5, 3 or 4 (Easy, Normal, Hard). With no player in its
//!   follow range it calms down and wanders again. A bound creaking never goes more than
//!   32 blocks from its heart, not even after a player.
//! - A bound creaking takes no damage but from the void and `/kill`: a player's hit makes
//!   it sway and the heart sound, and at night grows two or three resin clumps on pale
//!   oak logs joined to the heart within 2 blocks (taxicab), nearest first, at most once
//!   every five seconds. Breaking the heart makes its creaking twitch and then die,
//!   counted as killed by the player who broke it; a heart gone any other way does the
//!   same without a killer. Creakings from spawn eggs or commands are bound to no heart:
//!   they take damage normally (they have 1 health) and never vanish.
//! - Entity data: can move (16), is active (17), is tearing down (18), home position (19).

use std::collections::{HashMap, HashSet, VecDeque};

use levitas_nbt::Compound;
use levitas_registry::collision_boxes;

use super::mobs::{yaw_towards, Mob, MobKind};
use super::*;
use crate::blocks::fluid::fluid_at;
use crate::blocks::{offset, sturdy, Dir};
use crate::rules::Difficulty;
use crate::world::survival::Damage;

/// Where the module keeps its state in `World::kin_state`.
const KEY: &str = "creakings";
const HEART: &str = "minecraft:creaking_heart";
const RESIN_CLUMP: &str = "minecraft:resin_clump";
/// The blocks that hold a heart awake, when they lie on its axis.
const PALE_OAK: [&str; 4] = [
    "minecraft:pale_oak_log",
    "minecraft:pale_oak_wood",
    "minecraft:stripped_pale_oak_log",
    "minecraft:stripped_pale_oak_wood",
];
const DATA_CAN_MOVE: u8 = 16;
const DATA_ACTIVE: u8 = 17;
const DATA_TEARING_DOWN: u8 = 18;
const DATA_HOME: u8 = 19;
const SERIALIZER_BOOL: i32 = 8;
const SERIALIZER_OPTIONAL_POS: i32 = 11;
/// The attack animation, as for other big melee mobs.
const EVENT_ATTACK: i8 = 4;
const DAY_TICKS: i64 = 24_000;
/// The time of day hearts are awake.
const NIGHT: (i64, i64) = (12_600, 23_400);
/// Ticks between looks at each heart.
const HEART_PERIOD: u64 = 20;
/// Half the size of the box a heart makes its creaking in, across and up.
const SPAWN_REACH: (i32, i32) = (16, 8);
/// Columns tried per look for a spot to make a creaking.
const SPAWN_ATTEMPTS: u32 = 10;
/// How far from its heart a bound creaking goes, and how far it may be pushed before
/// it vanishes.
const HOME_RADIUS: f64 = 32.0;
const VANISH_DISTANCE: f64 = 34.0;
/// How close a player who sees an idle creaking must be to alert it.
const ALERT_RANGE: f64 = 12.0;
/// The cosine of the 60 degrees a player's look may be off a creaking.
const VIEW_COS: f64 = 0.5;
/// Ticks a bound creaking may share a block with a player before it vanishes.
const SAME_BLOCK_TICKS: u32 = 100;
/// Ticks a creaking twitches after its heart is broken.
const TEARDOWN_TICKS: u32 = 45;
const ATTACK_TICKS: u32 = 20;
/// Chasing speed in blocks per tick (an attacking vindicator walks 0.17).
const CHASE_SPEED: f64 = 0.2;
const JUMP: f64 = 0.42;
const GRAVITY: f64 = 0.08;
const AIR_DRAG: f64 = 0.98;
const KNOCKBACK: f64 = 0.4;
/// Ticks between two growths of resin around a heart, and how far from it they grow.
const RESIN_COOLDOWN: u64 = 100;
const RESIN_REACH: i32 = 2;
const HEART_XP: (i32, i32) = (20, 24);
const GAME_MODE_ADVENTURE: u8 = 2;
/// A player's hitbox, for spots a heart may not make a creaking in.
const PLAYER_SIZE: (f64, f64) = (0.6, 1.8);

/// What the module keeps for all creakings, in `World::kin_state`.
#[derive(Debug, Default)]
struct Hearts {
    /// Hearts found so far (placed, random-ticked, or a creaking's home).
    known: HashSet<BlockPos>,
    /// Creakings to take out of the world at the end of this tick.
    vanishing: Vec<i32>,
    /// The tick from which each heart may grow resin again.
    resin_ready: HashMap<BlockPos, u64>,
}

/// What the module keeps for a creaking, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq)]
struct Creaking {
    /// Alerted by a player who saw it.
    active: bool,
    /// False while a player watches it.
    can_move: bool,
    tearing_down: Option<TearDown>,
    /// The player it chases while active.
    prey: Option<i32>,
    /// Ticks it has shared a block with a player.
    same_block: u32,
    /// Its body and head yaw when it froze, kept while it stays frozen.
    frozen_angles: Option<(f32, f32)>,
}

impl Creaking {
    fn new() -> Creaking {
        Creaking {
            active: false,
            can_move: true,
            tearing_down: None,
            prey: None,
            same_block: 0,
            frozen_angles: None,
        }
    }
}

/// The twitching after a heart is broken: ticks left, and the player who broke it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TearDown {
    ticks: u32,
    by: Option<i32>,
}

fn is_heart(state: u16) -> bool {
    crate::blocks::name(state) == HEART
}

fn is_pale_oak(state: u16) -> bool {
    PALE_OAK.contains(&crate::blocks::name(state))
}

/// The two sides of a block along `axis`.
fn axis_sides(axis: &str) -> [Dir; 2] {
    match axis {
        "x" => [Dir::West, Dir::East],
        "z" => [Dir::North, Dir::South],
        _ => [Dir::Down, Dir::Up],
    }
}

fn centre(pos: BlockPos) -> [f64; 3] {
    [pos.x, pos.y, pos.z].map(|v| f64::from(v) + 0.5)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn feet(pos: [f64; 3]) -> BlockPos {
    BlockPos::new(
        pos[0].floor() as i32,
        (pos[1] + 0.01).floor() as i32,
        pos[2].floor() as i32,
    )
}

/// Whether two upright boxes, given by their feet's centre and (width, height), overlap.
fn boxes_overlap(a: [f64; 3], a_size: (f64, f64), b: [f64; 3], b_size: (f64, f64)) -> bool {
    let reach = (a_size.0 + b_size.0) / 2.0;
    (a[0] - b[0]).abs() < reach
        && (a[2] - b[2]).abs() < reach
        && a[1] < b[1] + b_size.1
        && b[1] < a[1] + a_size.1
}

/// Players a creaking minds: alive, in survival or adventure.
fn minds(p: &Player) -> bool {
    p.alive && matches!(p.mode, GAME_MODE_SURVIVAL | GAME_MODE_ADVENTURE)
}

/// Damage of a creaking's blow by difficulty.
fn attack_damage(difficulty: Difficulty) -> f32 {
    match difficulty {
        Difficulty::Peaceful => 0.0,
        Difficulty::Easy => 2.5,
        Difficulty::Normal => 3.0,
        Difficulty::Hard => 4.0,
    }
}

fn creaking_of(mob: &Mob) -> Option<&Creaking> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Creaking>())
}

/// The creaking's state, taken out of the mob (made fresh if it has none yet).
fn take_state(mob: &mut Mob) -> Box<dyn std::any::Any + Send> {
    mob.kin
        .take()
        .filter(|k| k.is::<Creaking>())
        .unwrap_or_else(|| Box::new(Creaking::new()))
}

fn creaking_data(id: i32, c: &Creaking, home: Option<BlockPos>) -> Writer {
    let packet = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_CAN_MOVE)
        .varint(SERIALIZER_BOOL)
        .bool(c.can_move)
        .u8(DATA_ACTIVE)
        .varint(SERIALIZER_BOOL)
        .bool(c.active)
        .u8(DATA_TEARING_DOWN)
        .varint(SERIALIZER_BOOL)
        .bool(c.tearing_down.is_some())
        .u8(DATA_HOME)
        .varint(SERIALIZER_OPTIONAL_POS);
    match home {
        Some(pos) => packet.bool(true).position(pos),
        None => packet.bool(false),
    }
    .u8(0xFF)
}

/// The module-wide state, made when first needed.
fn hearts_mut(w: &mut World) -> &mut Hearts {
    let entry = w
        .kin_state
        .entry(KEY)
        .or_insert_with(|| Box::new(Hearts::default()));
    if !entry.is::<Hearts>() {
        *entry = Box::new(Hearts::default());
    }
    entry
        .downcast_mut::<Hearts>()
        .expect("creaking hearts state")
}

/// Whether hearts are awake now where `w` is.
fn night(w: &World) -> bool {
    w.dimension == Dimension::Overworld
        && (NIGHT.0..=NIGHT.1).contains(&w.day_time.rem_euclid(DAY_TICKS))
}

/// Starts the twitching before death; returns whether it had not started yet.
fn start_tear_down(c: &mut Creaking, by: Option<i32>) -> bool {
    if c.tearing_down.is_some() {
        return false;
    }
    c.tearing_down = Some(TearDown {
        ticks: TEARDOWN_TICKS,
        by,
    });
    c.prey = None;
    true
}

pub(super) fn spawned(_w: &mut World, mob: &mut Mob, _biome: &str) {
    // Creakings are never generated with chunks; any that is starts unbound and idle.
    mob.kin = Some(Box::new(Creaking::new()));
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    let mut kin = take_state(mob);
    let busy = match kin.downcast_mut::<Creaking>() {
        Some(c) => w.tick_creaking(id, mob, c),
        None => false,
    };
    mob.kin = Some(kin);
    busy
}

pub(super) fn use_on(
    _w: &mut World,
    _player: i32,
    _target: i32,
    _slot: usize,
    _held: &str,
) -> bool {
    false
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, attacker: Option<i32>) {
    // A creaking hit by a player turns on them.
    if let Some(pid) = attacker {
        w.creaking_alerted(id, mob, pid);
    }
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let fresh = Creaking::new();
    let c = creaking_of(mob).unwrap_or(&fresh);
    vec![creaking_data(id, c, mob.home)]
}

/// The heart a creaking is bound to is its `home_pos`, which `critters.rs` saves for
/// every mob; nothing else of a creaking is saved.
pub(super) fn write_nbt(_mob: &Mob, _nbt: &mut Compound) {}

pub(super) fn read_nbt(mob: &mut Mob, _nbt: &Compound) {
    mob.kin = Some(Box::new(Creaking::new()));
}

pub(super) fn damage_taken(
    w: &mut World,
    id: i32,
    mob: &mut Mob,
    damage: f32,
    damage_type: &str,
    attacker: Option<i32>,
) -> f32 {
    let Some(heart) = mob.home else {
        return damage;
    };
    if matches!(
        damage_type,
        "minecraft:out_of_world" | "minecraft:generic_kill"
    ) {
        return damage;
    }
    // Bound to a living heart: nothing hurts it, and a player's blow makes it sway.
    if let Some(pid) = attacker {
        let pitch = 0.8 + w.rng.f64() as f32 * 0.4;
        w.play_sound_at("minecraft:entity.creaking.sway", mob.pos, 1.0, pitch, None);
        w.play_sound_at(
            "minecraft:block.creaking_heart.hurt",
            centre(heart),
            1.0,
            1.0,
            None,
        );
        if night(w) {
            w.grow_resin(heart);
        }
        w.creaking_alerted(id, mob, pid);
    }
    0.0
}

pub(super) fn tick_world(w: &mut World) {
    let Some(state) = w.kin_state.remove(KEY) else {
        return; // no heart or creaking seen yet
    };
    let Ok(mut hearts) = state.downcast::<Hearts>() else {
        return;
    };
    for id in std::mem::take(&mut hearts.vanishing) {
        w.creaking_vanish(id);
    }
    if w.tick.is_multiple_of(HEART_PERIOD) && !hearts.known.is_empty() {
        w.tick_creaking_hearts(&mut hearts);
    }
    w.kin_state.insert(KEY, hearts);
}

pub(super) fn block_placed(w: &mut World, _player: i32, pos: BlockPos, state: u16) {
    if is_heart(state) {
        hearts_mut(w).known.insert(pos);
    } else if is_pale_oak(state) {
        // A log put next to a heart may wake it.
        for d in Dir::ALL {
            let next = offset(pos, d);
            if w.loaded_block(next).is_some_and(is_heart) {
                hearts_mut(w).known.insert(next);
            }
        }
    }
}

pub(super) fn block_broken(w: &mut World, player: i32, pos: BlockPos, state: u16) {
    if !is_heart(state) {
        return;
    }
    let bound: Vec<i32> = w
        .mobs
        .iter()
        .filter(|(_, m)| m.kind == MobKind::Creaking && m.home == Some(pos) && m.alive())
        .map(|(&id, _)| id)
        .collect();
    for id in bound {
        if let Some(mut mob) = w.mobs.remove(&id) {
            let mut kin = take_state(&mut mob);
            if let Some(c) = kin.downcast_mut::<Creaking>() {
                w.creaking_tear_down(id, &mob, c, Some(player));
            }
            mob.kin = Some(kin);
            w.mobs.insert(id, mob);
        }
    }
    let hearts = hearts_mut(w);
    hearts.known.remove(&pos);
    hearts.resin_ready.remove(&pos);
    let survival = w.players.get(&player).is_some_and(Player::survival);
    if survival && crate::blocks::prop(state, "natural") == Some("true") {
        let xp = HEART_XP.0 + w.rng.below((HEART_XP.1 - HEART_XP.0 + 1) as u32) as i32;
        w.spawn_xp(centre(pos), xp);
    }
}

pub(super) fn block_tick(w: &mut World, pos: BlockPos, state: u16) -> bool {
    if !is_heart(state) {
        return false;
    }
    hearts_mut(w).known.insert(pos);
    // An awake heart wrapped in wood on all sides now and then sounds.
    let wrapped = Dir::ALL.iter().all(|&d| {
        w.loaded_block(offset(pos, d))
            .is_some_and(crate::blocks::plants::is_log)
    });
    if wrapped && crate::blocks::prop(state, "creaking_heart_state") == Some("awake") {
        w.play_sound_at(
            "minecraft:block.creaking_heart.idle",
            centre(pos),
            1.0,
            1.0,
            None,
        );
    }
    true
}

pub(super) fn ticks_randomly(state: u16) -> bool {
    is_heart(state)
}

impl World {
    /// A creaking's turn. Returns whether it moved itself (or stood frozen).
    fn tick_creaking(&mut self, id: i32, mob: &mut Mob, c: &mut Creaking) -> bool {
        mob.attack_cooldown = mob.attack_cooldown.saturating_sub(1);
        if let Some(t) = c.tearing_down.as_mut() {
            mob.target = None;
            mob.path.clear();
            if t.ticks > 0 {
                t.ticks -= 1;
                self.creaking_move(mob, [0.0, 0.0]);
            } else {
                let by = t.by;
                self.creaking_dies(id, mob, by);
            }
            return true;
        }
        if let Some(heart) = mob.home {
            hearts_mut(self).known.insert(heart);
            if self.loaded_block(heart).is_some_and(|s| !is_heart(s)) {
                // Its heart is gone (blown up, or broken where no one saw).
                self.creaking_tear_down(id, mob, c, None);
                return true;
            }
            if distance(mob.pos, centre(heart)) > VANISH_DISTANCE {
                hearts_mut(self).vanishing.push(id);
                return true;
            }
            // This one is out of `mobs` during its turn: another bound to the same heart
            // (and not already leaving) means this one is the extra.
            if self.tick.is_multiple_of(HEART_PERIOD) && self.other_bound_creaking(heart) {
                hearts_mut(self).vanishing.push(id);
                return true;
            }
            let block = feet(mob.pos);
            if self
                .players
                .values()
                .any(|p| p.alive && feet(p.pos) == block)
            {
                c.same_block += 1;
                if c.same_block > SAME_BLOCK_TICKS {
                    hearts_mut(self).vanishing.push(id);
                    return true;
                }
            } else {
                c.same_block = 0;
            }
        }

        let (frozen, alerter) = self.creaking_watchers(mob);
        let range = mob.kind.follow_range();
        if !c.active {
            if let Some(pid) = alerter {
                c.active = true;
                c.prey = Some(pid);
                self.creaking_sound(mob, "activate");
                self.send_to_viewers(mob, creaking_data(id, c, mob.home));
            }
        }
        if c.active {
            let valid = |w: &World, pid: i32| {
                w.players
                    .get(&pid)
                    .is_some_and(|p| minds(p) && distance(p.pos, mob.pos) <= range)
            };
            if !c.prey.is_some_and(|pid| valid(self, pid)) {
                c.prey = self
                    .players
                    .iter()
                    .filter(|(_, p)| minds(p))
                    .map(|(&pid, p)| (pid, distance(p.pos, mob.pos)))
                    .filter(|&(_, d)| d <= range)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(pid, _)| pid);
            }
            if c.prey.is_none() {
                c.active = false;
                self.creaking_sound(mob, "deactivate");
                self.send_to_viewers(mob, creaking_data(id, c, mob.home));
            }
        }

        if frozen {
            if c.can_move {
                c.can_move = false;
                c.frozen_angles = Some((mob.yaw, mob.head_yaw));
                self.creaking_sound(mob, "freeze");
                self.send_to_viewers(mob, creaking_data(id, c, mob.home));
            }
            if let Some((yaw, head_yaw)) = c.frozen_angles {
                mob.yaw = yaw;
                mob.head_yaw = head_yaw;
            }
            mob.target = None;
            mob.path.clear();
            mob.knockback = [0.0; 2];
            self.creaking_move(mob, [0.0, 0.0]);
            return true;
        }
        if !c.can_move {
            c.can_move = true;
            c.frozen_angles = None;
            self.creaking_sound(mob, "unfreeze");
            self.send_to_viewers(mob, creaking_data(id, c, mob.home));
        }
        if let Some(pid) = c.prey.filter(|_| c.active) {
            return self.creaking_chase(id, mob, pid);
        }
        // Idle: it wanders, but not away from its heart.
        if let (Some(heart), Some((goal, ticks))) = (mob.home, mob.target) {
            let home = centre(heart);
            if (goal[0] - home[0]).hypot(goal[1] - home[2]) > HOME_RADIUS - 2.0 {
                mob.target = Some(([home[0], home[2]], ticks));
            }
        }
        false
    }

    /// Whether a player who minds creakings watches this one (and does not wear a
    /// carved pumpkin), and the nearest player within 12 blocks who sees it at all.
    fn creaking_watchers(&self, mob: &Mob) -> (bool, Option<i32>) {
        let range = mob.kind.follow_range();
        let pumpkin = ItemInfo::id_of("minecraft:carved_pumpkin");
        let mut frozen = false;
        let mut alerter: Option<(i32, f64)> = None;
        for (&pid, p) in &self.players {
            if !minds(p) {
                continue;
            }
            let d = distance(p.pos, mob.pos);
            if d > range || !self.player_sees_creaking(p, mob) {
                continue;
            }
            if Some(p.inventory.slots[5].item) != pumpkin {
                frozen = true;
            }
            let closer = match alerter {
                Some((_, best)) => d < best,
                None => true,
            };
            if d <= ALERT_RANGE && closer {
                alerter = Some((pid, d));
            }
        }
        (frozen, alerter.map(|(pid, _)| pid))
    }

    /// Whether `p` looks within 60 degrees of some part of the creaking's body, with
    /// nothing solid between.
    fn player_sees_creaking(&self, p: &Player, mob: &Mob) -> bool {
        let (eye, look, _) = p.eye_ray();
        let height = mob.size().1;
        [0.1, 0.5, 0.9].iter().any(|f| {
            let point = [mob.pos[0], mob.pos[1] + height * f, mob.pos[2]];
            let to = [0, 1, 2].map(|i| point[i] - eye[i]);
            let length = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
            if length < 1e-6 {
                return true;
            }
            let cos: f64 = (0..3).map(|i| look[i] * to[i] / length).sum();
            cos >= VIEW_COS && self.line_of_sight(eye, point)
        })
    }

    /// A player hit the creaking or came to its notice: it turns on them.
    fn creaking_alerted(&mut self, id: i32, mob: &mut Mob, pid: i32) {
        if !self.players.get(&pid).is_some_and(minds) {
            return;
        }
        let mut kin = take_state(mob);
        if let Some(c) = kin.downcast_mut::<Creaking>() {
            if c.tearing_down.is_none() {
                c.prey = Some(pid);
                if !c.active {
                    c.active = true;
                    self.creaking_sound(mob, "activate");
                    self.send_to_viewers(mob, creaking_data(id, c, mob.home));
                }
            }
        }
        mob.kin = Some(kin);
    }

    /// Runs at its prey, never more than 32 blocks from its heart, and hits it when
    /// close. Returns whether it moved itself (in water the shared code swims it).
    fn creaking_chase(&mut self, id: i32, mob: &mut Mob, pid: i32) -> bool {
        let Some(target) = self.players.get(&pid).map(|p| p.pos) else {
            return false;
        };
        let mut goal = target;
        if let Some(heart) = mob.home {
            let home = centre(heart);
            let d = distance(target, home);
            if d > HOME_RADIUS {
                let k = HOME_RADIUS / d;
                goal = [0, 1, 2].map(|i| home[i] + (target[i] - home[i]) * k);
            }
        }
        mob.head_yaw = yaw_towards(mob.pos, target);
        self.follow_path(mob, feet(goal));
        let in_liquid = self
            .loaded_block(feet(mob.pos))
            .and_then(fluid_at)
            .is_some();
        if !in_liquid {
            let by = match mob.target {
                Some((aim, _)) => {
                    let (dx, dz) = (aim[0] - mob.pos[0], aim[1] - mob.pos[2]);
                    let length = dx.hypot(dz);
                    if length > 1e-6 {
                        mob.yaw = yaw_towards(mob.pos, [aim[0], mob.pos[1], aim[1]]);
                        let step = CHASE_SPEED.min(length);
                        [dx / length * step, dz / length * step]
                    } else {
                        [0.0, 0.0]
                    }
                }
                None => [0.0, 0.0],
            };
            self.creaking_move(mob, by);
        }

        let (width, height) = mob.size();
        let reach = ((width * 2.0).powi(2) + 0.6).sqrt();
        let flat = (target[0] - mob.pos[0]).hypot(target[2] - mob.pos[2]);
        let dy = target[1] - mob.pos[1];
        let difficulty = crate::rules::difficulty();
        if flat <= reach
            && dy > -1.5
            && dy < height
            && mob.attack_cooldown == 0
            && difficulty != Difficulty::Peaceful
        {
            mob.attack_cooldown = ATTACK_TICKS;
            self.send_to_viewers(mob, entity_event(id, EVENT_ATTACK));
            self.creaking_sound(mob, "attack");
            let damage = attack_damage(difficulty);
            if self.damage_from(pid, damage, Damage::Mob(mob.kind.display()), Some(id)) {
                let yaw = f64::from(yaw_towards(mob.pos, target)).to_radians();
                self.push_player(pid, [-yaw.sin() * KNOCKBACK, 0.36, yaw.cos() * KNOCKBACK]);
            }
        }
        !in_liquid
    }

    /// Moves a creaking by `by` in x and z (plus what knockback it has), blocks
    /// allowing, with gravity; it jumps when a block stops it on the ground.
    fn creaking_move(&mut self, mob: &mut Mob, by: [f64; 2]) {
        let (width, height) = mob.size();
        let half = width / 2.0;
        let mut pos = mob.pos;
        let dy = self.clip_move(pos, half, height, 1, mob.vel[1]);
        pos[1] += dy;
        let stopped = (dy - mob.vel[1]).abs() > 1e-9;
        let grounded = stopped && mob.vel[1] < 0.0;
        if stopped {
            mob.vel[1] = 0.0;
        }
        let mut blocked = false;
        let mut moved = [0.0; 2];
        for (i, axis) in [0, 2].into_iter().enumerate() {
            let wanted = by[i] + mob.knockback[i];
            let d = self.clip_move(pos, half, height, axis, wanted);
            pos[axis] += d;
            moved[i] = d;
            if (d - wanted).abs() > 1e-9 {
                blocked = true;
                mob.knockback[i] = 0.0;
            }
        }
        for k in &mut mob.knockback {
            *k *= 0.6;
            if k.abs() < 0.003 {
                *k = 0.0;
            }
        }
        mob.vel[0] = moved[0];
        mob.vel[2] = moved[1];
        let walking = by[0] != 0.0 || by[1] != 0.0;
        mob.vel[1] = if grounded && blocked && walking {
            JUMP
        } else {
            (mob.vel[1] - GRAVITY) * AIR_DRAG
        };
        mob.pos = pos;
    }

    /// The creaking starts twitching before it dies (its heart is gone).
    fn creaking_tear_down(&mut self, id: i32, mob: &Mob, c: &mut Creaking, by: Option<i32>) {
        if start_tear_down(c, by) {
            self.creaking_sound(mob, "twitch");
            self.send_to_viewers(mob, creaking_data(id, c, mob.home));
        }
    }

    /// The twitching is over: it dies, killed by whoever broke its heart.
    fn creaking_dies(&mut self, id: i32, mob: &mut Mob, by: Option<i32>) {
        self.kill_mob(id, mob);
        if let Some(pid) = by.filter(|p| self.players.contains_key(p)) {
            let kind = mob.kind.entity_type();
            self.stat(pid, levitas_registry::stat_type::KILLED, kind, 1);
            self.stat(
                pid,
                levitas_registry::stat_type::CUSTOM,
                levitas_registry::custom_stat::MOB_KILLS,
                1,
            );
            self.advance(pid, super::advancements::Trigger::KilledEntity(kind));
        }
    }

    fn creaking_sound(&mut self, mob: &Mob, what: &str) {
        let pitch = 0.8 + self.rng.f64() as f32 * 0.4;
        let name = format!("minecraft:entity.creaking.{what}");
        self.play_sound_at(&name, mob.pos, 1.0, pitch, None);
    }

    /// Takes a creaking out of the world without a death.
    fn creaking_vanish(&mut self, id: i32) {
        if let Some(mob) = self.mobs.remove(&id) {
            self.send_to_viewers(&mob, super::items::remove_entity(id));
        }
    }

    /// Each known heart in a loaded chunk: wakes, sleeps or uproots, sends its creaking
    /// away at dawn and makes a new one at night.
    fn tick_creaking_hearts(&mut self, hearts: &mut Hearts) {
        let night = night(self);
        let peaceful = crate::rules::difficulty() == Difficulty::Peaceful;
        let positions: Vec<BlockPos> = hearts.known.iter().copied().collect();
        for pos in positions {
            let Some(state) = self.loaded_block(pos) else {
                continue; // not loaded: looked at again once it is
            };
            if !is_heart(state) {
                hearts.known.remove(&pos);
                hearts.resin_ready.remove(&pos);
                continue;
            }
            let mut bound = self.heart_creaking(pos);
            if let Some(mid) = bound {
                let named = self.mobs.get(&mid).is_some_and(|m| m.name.is_some());
                if !night && !named {
                    self.creaking_vanish(mid);
                    bound = None;
                }
            }
            let aligned = self.heart_aligned(pos, state);
            let wanted = match (bound.is_some() || aligned, night) {
                (false, _) => "uprooted",
                (true, true) => "awake",
                (true, false) => "dormant",
            };
            let new = crate::blocks::with(state, "creaking_heart_state", wanted);
            if new != state {
                self.set_block(pos, new);
            }
            if night && aligned && bound.is_none() && !peaceful {
                self.heart_spawn(pos);
            }
        }
    }

    /// Whether the blocks on both sides of the heart along its axis are pale oak logs
    /// or wood lying the same way.
    fn heart_aligned(&self, pos: BlockPos, state: u16) -> bool {
        let axis = crate::blocks::prop(state, "axis").unwrap_or("y");
        axis_sides(axis).iter().all(|&d| {
            self.loaded_block(offset(pos, d))
                .is_some_and(|s| is_pale_oak(s) && crate::blocks::prop(s, "axis") == Some(axis))
        })
    }

    /// The living creaking bound to the heart at `pos`, if any.
    fn heart_creaking(&self, pos: BlockPos) -> Option<i32> {
        self.mobs
            .iter()
            .find(|(_, m)| m.kind == MobKind::Creaking && m.home == Some(pos) && m.alive())
            .map(|(&id, _)| id)
    }

    /// Whether a living creaking in `mobs`, not already on its way out, is bound to the
    /// heart at `pos`.
    fn other_bound_creaking(&mut self, pos: BlockPos) -> bool {
        let leaving = std::mem::take(&mut hearts_mut(self).vanishing);
        let found = self.mobs.iter().any(|(id, m)| {
            m.kind == MobKind::Creaking && m.home == Some(pos) && m.alive() && !leaving.contains(id)
        });
        hearts_mut(self).vanishing = leaving;
        found
    }

    /// Looks for a spot around the heart and makes a creaking bound to it there.
    fn heart_spawn(&mut self, heart: BlockPos) -> Option<i32> {
        let (across, up) = SPAWN_REACH;
        for _ in 0..SPAWN_ATTEMPTS {
            let x = heart.x + self.rng.below(across as u32 * 2 + 1) as i32 - across;
            let z = heart.z + self.rng.below(across as u32 * 2 + 1) as i32 - across;
            let top = heart.y + self.rng.below(up as u32 * 2 + 1) as i32 - up;
            for y in (heart.y - up..=top).rev() {
                let at = BlockPos::new(x, y, z);
                if self.creaking_fits(at) {
                    return Some(self.spawn_bound_creaking(heart, at));
                }
            }
        }
        None
    }

    /// Whether a heart may make a creaking standing at `at`.
    fn creaking_fits(&self, at: BlockPos) -> bool {
        let Some(below) = self.loaded_block(offset(at, Dir::Down)) else {
            return false;
        };
        if !sturdy(below, Dir::Up) || block_has_tag(below, "minecraft:leaves") {
            return false;
        }
        for dy in 0..3 {
            let Some(state) = self.loaded_block(BlockPos::new(at.x, at.y + dy, at.z)) else {
                return false;
            };
            // Blocks two and more above the ground may be solid; liquid is never allowed.
            if fluid_at(state).is_some() || (dy < 2 && !collision_boxes(state).is_empty()) {
                return false;
            }
        }
        let spot = [
            f64::from(at.x) + 0.5,
            f64::from(at.y),
            f64::from(at.z) + 0.5,
        ];
        let size = MobKind::Creaking.size();
        let mob_in_way = self
            .mobs
            .values()
            .any(|m| m.alive() && boxes_overlap(m.pos, m.size(), spot, size));
        let player_in_way = self
            .players
            .values()
            .any(|p| p.alive && boxes_overlap(p.pos, PLAYER_SIZE, spot, size));
        !mob_in_way && !player_in_way
    }

    fn spawn_bound_creaking(&mut self, heart: BlockPos, at: BlockPos) -> i32 {
        let pos = [
            f64::from(at.x) + 0.5,
            f64::from(at.y),
            f64::from(at.z) + 0.5,
        ];
        let mut mob = Mob::new(MobKind::Creaking, pos, &mut self.rng);
        mob.yaw = self.rng.f64() as f32 * 360.0;
        mob.head_yaw = mob.yaw;
        mob.home = Some(heart);
        mob.kin = Some(Box::new(Creaking::new()));
        self.creaking_sound(&mob, "spawn");
        self.play_sound_at(
            "minecraft:block.creaking_heart.spawn",
            centre(heart),
            1.0,
            1.0,
            None,
        );
        let id = next_entity_id();
        self.mobs.insert(id, mob);
        id
    }

    /// Two or three resin clumps on pale oak joined to the heart within 2 blocks,
    /// nearest first, at most once every five seconds.
    fn grow_resin(&mut self, heart: BlockPos) {
        let now = self.tick;
        let hearts = hearts_mut(self);
        if hearts.resin_ready.get(&heart).is_some_and(|&t| now < t) {
            return;
        }
        hearts.resin_ready.insert(heart, now + RESIN_COOLDOWN);
        let Some(resin) = BlockInfo::by_name(RESIN_CLUMP) else {
            return;
        };
        let taxicab =
            |p: BlockPos| (p.x - heart.x).abs() + (p.y - heart.y).abs() + (p.z - heart.z).abs();
        let mut logs = Vec::new();
        let mut seen = HashSet::from([heart]);
        let mut queue = VecDeque::from([heart]);
        while let Some(at) = queue.pop_front() {
            for d in Dir::ALL {
                let next = offset(at, d);
                if taxicab(next) <= RESIN_REACH
                    && seen.insert(next)
                    && self.loaded_block(next).is_some_and(is_pale_oak)
                {
                    logs.push(next);
                    queue.push_back(next);
                }
            }
        }
        logs.sort_by_key(|&p| taxicab(p));
        let mut left = 2 + self.rng.below(2);
        for log in logs {
            if left == 0 {
                break;
            }
            let start = self.rng.below(6) as usize;
            for k in 0..6 {
                let d = Dir::ALL[(start + k) % 6];
                let spot = offset(log, d);
                if self.loaded_block(spot).is_some_and(is_air) {
                    let state = resin.state_with([(d.opposite().name(), "true")]);
                    self.set_block_updating(spot, state);
                    left -= 1;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    const HEART_AT: BlockPos = BlockPos { x: 0, y: -59, z: 0 };

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-creakings-{}-{}",
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
        // Far away (and in creative), so that only what a test puts close counts.
        let mut p = Player::new(profile, out, [-28.5, -60.0, -28.5]);
        p.mode = GAME_MODE_CREATIVE;
        w.players.insert(alice, p);
        (w, alice)
    }

    fn state_of(name: &str, props: &[(&str, &str)]) -> u16 {
        BlockInfo::by_name(name)
            .unwrap()
            .state_with(props.iter().copied())
    }

    /// A heart standing upright between two pale oak logs, known to the module.
    fn heart(w: &mut World, alice: i32) -> u16 {
        let log = state_of("minecraft:pale_oak_log", &[("axis", "y")]);
        w.set_block(BlockPos::new(0, -60, 0), log);
        w.set_block(BlockPos::new(0, -58, 0), log);
        let heart = state_of(HEART, &[("axis", "y"), ("natural", "true")]);
        w.set_block(HEART_AT, heart);
        block_placed(w, alice, HEART_AT, heart);
        heart
    }

    fn heart_state(w: &World) -> &'static str {
        crate::blocks::prop(w.loaded_block(HEART_AT).unwrap(), "creaking_heart_state").unwrap()
    }

    fn look_at_hearts(w: &mut World) {
        w.tick = (w.tick / HEART_PERIOD + 1) * HEART_PERIOD;
        tick_world(w);
    }

    fn bound(w: &World) -> Vec<i32> {
        w.mobs
            .iter()
            .filter(|(_, m)| m.kind == MobKind::Creaking && m.home == Some(HEART_AT))
            .map(|(&id, _)| id)
            .collect()
    }

    fn turn(w: &mut World, id: i32) -> bool {
        let mut mob = w.mobs.remove(&id).unwrap();
        let busy = tick(w, id, &mut mob);
        w.mobs.insert(id, mob);
        busy
    }

    fn state(w: &World, id: i32) -> Creaking {
        creaking_of(&w.mobs[&id]).cloned().expect("creaking state")
    }

    fn creaking(w: &mut World, at: [f64; 3], home: Option<BlockPos>) -> i32 {
        let mut mob = Mob::new(MobKind::Creaking, at, &mut w.rng);
        mob.home = home;
        mob.kin = Some(Box::new(Creaking::new()));
        let id = next_entity_id();
        w.mobs.insert(id, mob);
        id
    }

    /// Puts Alice in survival at `at`, looking along `yaw`.
    fn stand(w: &mut World, alice: i32, at: [f64; 3], yaw: f32) {
        let p = w.players.get_mut(&alice).unwrap();
        p.mode = GAME_MODE_SURVIVAL;
        p.pos = at;
        p.yaw = yaw;
        p.pitch = 0.0;
    }

    #[test]
    fn a_heart_wakes_at_night_makes_a_creaking_and_sends_it_away_at_dawn() {
        let (mut w, alice) = world_with_player();
        heart(&mut w, alice);
        w.day_time = 6000;
        look_at_hearts(&mut w);
        assert_eq!(heart_state(&w), "dormant");
        assert!(bound(&w).is_empty());
        w.day_time = 18_000;
        look_at_hearts(&mut w);
        assert_eq!(heart_state(&w), "awake");
        let made = bound(&w);
        assert_eq!(made.len(), 1, "one creaking bound to the heart");
        let pos = w.mobs[&made[0]].pos;
        assert!((pos[0] - 0.5).abs() <= 16.0 && (pos[2] - 0.5).abs() <= 16.0);
        assert!((pos[1] + 59.0).abs() <= 8.0);
        look_at_hearts(&mut w);
        assert_eq!(bound(&w), made, "no second one");
        // Its home is saved as vanilla does.
        let nbt = w.mobs[&made[0]].to_nbt();
        assert_eq!(nbt.int_array("home_pos"), Some(&[0, -59, 0][..]));
        let extras = spawn_extras(made[0], &w.mobs[&made[0]]);
        assert_eq!(extras.len(), 1);
        // At dawn it goes, and the heart sleeps.
        w.day_time = 24_000 + 23_500;
        look_at_hearts(&mut w);
        assert!(bound(&w).is_empty());
        assert_eq!(heart_state(&w), "dormant");
    }

    #[test]
    fn a_heart_without_its_logs_is_uprooted_and_makes_nothing() {
        let (mut w, alice) = world_with_player();
        heart(&mut w, alice);
        w.set_block(BlockPos::new(0, -58, 0), block::AIR);
        w.day_time = 18_000;
        look_at_hearts(&mut w);
        assert_eq!(heart_state(&w), "uprooted");
        assert!(bound(&w).is_empty());
        // Logs lying the wrong way do not count either.
        let log = state_of("minecraft:stripped_pale_oak_wood", &[("axis", "x")]);
        w.set_block(BlockPos::new(0, -58, 0), log);
        look_at_hearts(&mut w);
        assert_eq!(heart_state(&w), "uprooted");
        // Right way round, stripped wood does.
        let log = state_of("minecraft:stripped_pale_oak_wood", &[("axis", "y")]);
        w.set_block(BlockPos::new(0, -58, 0), log);
        block_placed(&mut w, alice, BlockPos::new(0, -58, 0), log);
        look_at_hearts(&mut w);
        assert_eq!(heart_state(&w), "awake");
        assert_eq!(bound(&w).len(), 1);
        // Once it has its creaking, the heart stays awake without its logs.
        w.set_block(BlockPos::new(0, -58, 0), block::AIR);
        look_at_hearts(&mut w);
        assert_eq!(heart_state(&w), "awake");
    }

    #[test]
    fn a_bound_creaking_cannot_be_hurt_but_an_unbound_one_can() {
        let (mut w, alice) = world_with_player();
        heart(&mut w, alice);
        w.day_time = 18_000;
        let tied = creaking(&mut w, [4.5, -60.0, 4.5], Some(HEART_AT));
        let free = creaking(&mut w, [-4.5, -60.0, -4.5], None);
        assert!(!w.hit_mob(
            tied,
            10.0,
            Some(alice),
            [0.0, 0.0],
            "minecraft:player_attack"
        ));
        assert!(w.mobs[&tied].alive());
        // The hit grew resin on the heart's logs, two or three clumps.
        let clumps = (-1..=1)
            .flat_map(|x| (-62..=-56).flat_map(move |y| (-1..=1).map(move |z| (x, y, z))))
            .filter(|&(x, y, z)| {
                w.loaded_block(BlockPos::new(x, y, z))
                    .is_some_and(|s| crate::blocks::name(s) == RESIN_CLUMP)
            })
            .count();
        assert!((2..=3).contains(&clumps), "{clumps}");
        // A creative player who hits it is not taken as prey.
        assert!(!state(&w, tied).active, "creative players are not prey");
        // A blow from no player (a mob, poison) does nothing either.
        assert!(!w.hit_mob(tied, 10.0, None, [0.0, 0.0], "minecraft:mob_attack"));
        assert!(w.mobs[&tied].alive());
        assert!(w.hit_mob(
            free,
            1.0,
            Some(alice),
            [0.0, 0.0],
            "minecraft:player_attack"
        ));
        assert!(!w.mobs[&free].alive());
        // The void still takes a bound one.
        assert!(w.hit_mob(tied, 10.0, None, [0.0, 0.0], "minecraft:out_of_world"));
    }

    #[test]
    fn breaking_the_heart_kills_its_creaking() {
        let (mut w, alice) = world_with_player();
        let heart = heart(&mut w, alice);
        w.day_time = 18_000;
        look_at_hearts(&mut w);
        let id = bound(&w)[0];
        block_broken(&mut w, alice, HEART_AT, heart);
        w.set_block(HEART_AT, block::AIR);
        let tear = state(&w, id).tearing_down.expect("twitching");
        assert_eq!(tear.by, Some(alice));
        for _ in 0..=TEARDOWN_TICKS {
            assert!(w.mobs[&id].alive());
            turn(&mut w, id);
        }
        assert!(!w.mobs[&id].alive(), "died");
        // A creaking whose heart went some other way twitches too.
        let other = creaking(&mut w, [3.5, -60.0, 3.5], Some(HEART_AT));
        turn(&mut w, other);
        assert_eq!(state(&w, other).tearing_down.map(|t| t.by), Some(None));
    }

    #[test]
    fn a_heart_keeps_only_one_creaking() {
        let (mut w, alice) = world_with_player();
        heart(&mut w, alice);
        w.day_time = 18_000;
        let first = creaking(&mut w, [4.5, -60.0, 4.5], Some(HEART_AT));
        let second = creaking(&mut w, [-4.5, -60.0, -4.5], Some(HEART_AT));
        w.tick = HEART_PERIOD * 10;
        turn(&mut w, first);
        turn(&mut w, second);
        tick_world(&mut w);
        assert_eq!(bound(&w).len(), 1, "one of the two stays");
        // The one left is not sent away later.
        let left = bound(&w)[0];
        w.tick = HEART_PERIOD * 11;
        turn(&mut w, left);
        tick_world(&mut w);
        assert_eq!(bound(&w), vec![left]);
    }

    #[test]
    fn no_creaking_is_made_inside_a_player() {
        let (mut w, alice) = world_with_player();
        let spot = BlockPos::new(5, -60, 5);
        assert!(w.creaking_fits(spot));
        stand(&mut w, alice, [5.8, -60.0, 5.3], 0.0);
        assert!(!w.creaking_fits(spot));
        // A mob in the way counts too.
        stand(&mut w, alice, [-20.5, -60.0, -20.5], 0.0);
        creaking(&mut w, [5.5, -60.0, 5.5], None);
        assert!(!w.creaking_fits(spot));
    }

    #[test]
    fn a_creaking_freezes_while_watched_and_chases_when_not() {
        let (mut w, alice) = world_with_player();
        let id = creaking(&mut w, [0.5, -60.0, 6.5], None);
        // Alice looks straight at it (yaw 0 faces south, +z).
        stand(&mut w, alice, [0.5, -60.0, 0.5], 0.0);
        assert!(turn(&mut w, id));
        let c = state(&w, id);
        assert!(!c.can_move && c.active, "frozen and alerted");
        let before = w.mobs[&id].pos;
        for _ in 0..20 {
            turn(&mut w, id);
        }
        assert_eq!(w.mobs[&id].pos[2], before[2], "did not move while watched");
        // She turns her back: it comes for her and hits her.
        w.players.get_mut(&alice).unwrap().yaw = 180.0;
        let full = w.players[&alice].health;
        for _ in 0..80 {
            turn(&mut w, id);
            if w.players[&alice].health < full {
                break;
            }
        }
        assert!(state(&w, id).can_move);
        assert!(w.mobs[&id].pos[2] < before[2] - 3.0, "closed in");
        assert!(w.players[&alice].health < full, "hit her");
        // A wall between them means she cannot watch it.
        for y in -60..=-57 {
            for x in -2..=2 {
                w.set_block(BlockPos::new(x, y, 3), block::STONE);
            }
        }
        let behind = creaking(&mut w, [0.5, -60.0, 6.5], None);
        w.players.get_mut(&alice).unwrap().yaw = 0.0;
        turn(&mut w, behind);
        assert!(state(&w, behind).can_move);
        assert!(!state(&w, behind).active);
    }

    #[test]
    fn a_carved_pumpkin_lets_it_move_and_creative_players_do_not_count() {
        let (mut w, alice) = world_with_player();
        let id = creaking(&mut w, [0.5, -60.0, 6.5], None);
        stand(&mut w, alice, [0.5, -60.0, 0.5], 0.0);
        w.players.get_mut(&alice).unwrap().inventory.slots[5] =
            Stack::new(ItemInfo::id_of("minecraft:carved_pumpkin").unwrap(), 1);
        turn(&mut w, id);
        let c = state(&w, id);
        assert!(c.can_move && c.active, "alerted, yet free to move");
        w.players.get_mut(&alice).unwrap().mode = GAME_MODE_CREATIVE;
        let other = creaking(&mut w, [0.5, -60.0, 8.5], None);
        w.players.get_mut(&alice).unwrap().inventory.slots[5] = Stack::EMPTY;
        turn(&mut w, other);
        let c = state(&w, other);
        assert!(c.can_move && !c.active);
        // The first calms down once no one is about.
        turn(&mut w, id);
        assert!(!state(&w, id).active);
    }

    #[test]
    fn a_creaking_far_from_its_heart_vanishes() {
        let (mut w, alice) = world_with_player();
        heart(&mut w, alice);
        w.day_time = 18_000;
        // Between looks at the heart, so that it makes no creaking of its own here.
        w.tick = 1;
        let id = creaking(&mut w, [36.5, -60.0, 0.5], Some(HEART_AT));
        assert!(turn(&mut w, id));
        tick_world(&mut w);
        assert!(!w.mobs.contains_key(&id));
        // One near enough stays, and wanders no further than its home.
        let near = creaking(&mut w, [20.5, -60.0, 0.5], Some(HEART_AT));
        w.mobs.get_mut(&near).unwrap().target = Some(([60.0, 0.5], 100));
        assert!(!turn(&mut w, near));
        tick_world(&mut w);
        assert!(w.mobs.contains_key(&near));
        assert_eq!(w.mobs[&near].target.map(|t| t.0), Some([0.5, 0.5]));
    }

    #[test]
    fn damage_by_difficulty_and_the_nights_of_hearts() {
        assert_eq!(attack_damage(Difficulty::Easy), 2.5);
        assert_eq!(attack_damage(Difficulty::Normal), 3.0);
        assert_eq!(attack_damage(Difficulty::Hard), 4.0);
        let (mut w, _) = world_with_player();
        for (time, awake) in [
            (12_599, false),
            (12_600, true),
            (23_400, true),
            (23_401, false),
        ] {
            w.day_time = time;
            assert_eq!(night(&w), awake, "{time}");
        }
        assert!(ticks_randomly(state_of(HEART, &[])));
        assert!(!ticks_randomly(block::STONE));
    }
}
