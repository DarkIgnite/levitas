//! The wither: building it, its fight, its skulls and its boss bar.
//!
//! Behaviour follows the Minecraft Wiki (Wither):
//! - Four soul sand or soul soil blocks in a T with three wither skeleton skulls on the
//!   arms and the top make a wither, the last block placed being a skull. The T may
//!   stand along either axis, lie on its side or hang upside down; the two corners
//!   beside its foot must be air. Never on Peaceful. The blocks go and the wither
//!   appears at the foot of the T.
//! - For 220 ticks it is invulnerable, neither moves nor attacks, and its health
//!   fills up; then it lets off an explosion of power 7 and wakes, with a sound heard
//!   everywhere.
//! - It attacks players (not in creative or spectator) and every mob but the undead
//!   and ghasts within 40 blocks. The middle head steers: the wither flies 5 blocks
//!   above its target (level with it once armoured) and within 3 blocks of it, and the
//!   middle head shoots a black skull every 2 seconds while it can see the target, one
//!   in a thousand of them blue. Each side head picks a target of its own in sight and
//!   shoots it every 2 to 3 seconds; one that has not fired for a while (sooner when
//!   the wither keeps being hurt) shoots a blue skull in a random direction on Normal
//!   and Hard. Skulls deal 8 (scaled by difficulty for players), explode with power 1
//!   and give Wither II for 10 seconds on Normal and 40 on Hard. Blue ones fly slower
//!   and break even obsidian where they land.
//! - Below half health its wither armour makes it immune to arrows, tridents and wind
//!   charges.
//! - When hurt, 20 ticks later it breaks the blocks in a 3x4x3 box around it, but not
//!   those of `minecraft:wither_immune`, unbreakable ones or liquids; a waterlogged
//!   block leaves its water.
//! - It regenerates 1 health a second, and 5 for each kill; a mob it kills leaves a
//!   wither rose. It is immune to status effects, never despawns, and drops a nether
//!   star. Players who see it get a purple boss bar.
//! - Saved as vanilla: `Invul`.

use std::collections::{HashMap, HashSet};

use levitas_nbt::Compound;

use super::advancements::Trigger;
use super::mobs::{yaw_towards, Mob, MobKind};
use super::survival::Damage;
use super::*;
use crate::blocks::{
    self,
    fluid::{fluid_at, Fluid},
};
use crate::rules::Difficulty;

/// The module's key in `World::kin_state`.
const KEY: &str = "withers";

/// Entity data: the heads' targets (entity IDs, 0 for none) and the ticks of
/// invulnerability left. Living entities keep their health at index 9.
const DATA_HEAD_TARGETS: [u8; 3] = [16, 17, 18];
const DATA_INVUL: u8 = 19;
const DATA_HEALTH: u8 = 9;
/// A wither skull's entity data: whether it is a blue ("dangerous") one.
const DATA_SKULL_DANGEROUS: u8 = 8;
const SERIALIZER_VARINT: i32 = 1;
const SERIALIZER_FLOAT: i32 = 3;
const SERIALIZER_BOOL: i32 = 8;

const BOSS_ADD: i32 = 0;
const BOSS_REMOVE: i32 = 1;
const BOSS_HEALTH: i32 = 2;
const BOSS_PURPLE: i32 = 5;
/// The wither's bar darkens the sky.
const BOSS_DARKEN_SKY: u8 = 0x01;

const LEVEL_EVENT_BREAK_BLOCK: i32 = 1022;
const LEVEL_EVENT_SPAWNED: i32 = 1023;
const LEVEL_EVENT_SHOOT: i32 = 1024;
const LEVEL_EVENT_BLOCK_DESTROYED: i32 = 2001;

const GAME_MODE_SPECTATOR: u8 = 3;
const PLAYER_HEIGHT: f64 = 1.8;

/// Ticks of the spawning phase, and the blast that ends it.
const SPAWN_TICKS: i32 = 220;
const SPAWN_POWER: f32 = 7.0;
/// How far it notices targets.
const RANGE: f64 = 40.0;
/// Ticks between the middle head's skulls, and the chance of a blue one (in 1000).
const MAIN_COOLDOWN: u32 = 40;
const MAIN_BLUE_IN_1000: u32 = 1;
/// A side head's turns: every 10 to 19 ticks, or 40 to 59 after a shot.
const SIDE_TURN: (u32, u32) = (10, 10);
const SIDE_RELOAD: (u32, u32) = (40, 20);
/// A side head that has not shot for more than this many turns fires a blue skull;
/// each hit on the wither brings that closer.
const BLUE_AFTER_TURNS: u32 = 15;
const BLUE_PER_HURT: u32 = 3;
/// Ticks between being hurt and breaking the blocks around it.
const BREAK_DELAY: u32 = 20;
const REGEN_TICKS: u32 = 20;
const REGEN: f32 = 1.0;
const KILL_HEAL: f32 = 5.0;
/// How high above its target it flies, how close it keeps, how fast it goes.
const HOVER: f64 = 5.0;
const KEEP_NEAR: f64 = 3.0;
const SPEED: f64 = 0.3;
const CLIMB: f64 = 0.25;
const IDLE_SPEED: f64 = 0.08;

/// Wither skulls: speed added each tick along the flight and what is kept of it
/// (blue ones keep less, so they are slower), damage, blast and lifetime.
const SKULL_ACCELERATION: f64 = 0.1;
const SKULL_INERTIA: f64 = 0.95;
const BLUE_INERTIA: f64 = 0.73;
const SKULL_DAMAGE: f32 = 8.0;
const SKULL_POWER: f32 = 1.0;
const SKULL_LIFETIME: u32 = 600;
const SKULL_REACH: f64 = 0.15;
const SAMPLE: f64 = 0.25;
/// Wither II from a skull, by difficulty.
const WITHER_NORMAL_TICKS: i32 = 200;
const WITHER_HARD_TICKS: i32 = 800;
const WITHER_AMPLIFIER: u8 = 1;
/// Damage types the wither armour stops.
const ARMOUR_STOPS: [&str; 3] = [
    "minecraft:arrow",
    "minecraft:trident",
    "minecraft:wind_charge",
];

/// What the module keeps for a wither, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq)]
struct Wither {
    /// Ticks of the spawning phase left.
    invul: i32,
    /// What each head is after: middle, left, right (entity IDs, 0 for none).
    heads: [i32; 3],
    /// The side heads' turns and how many turns since each last shot.
    side_cooldown: [u32; 2],
    blue_count: [u32; 2],
    main_cooldown: u32,
    /// Ticks until it breaks the blocks around it, after being hurt.
    break_blocks: u32,
    regen: u32,
    /// Where an idle wither drifts to.
    wander: Option<[f64; 2]>,
    /// What viewers were last told.
    sent_heads: [i32; 3],
    sent_invul: i32,
    sent_health: f32,
}

impl Wither {
    fn new(invul: i32) -> Wither {
        Wither {
            invul,
            heads: [0; 3],
            side_cooldown: [0; 2],
            blue_count: [0; 2],
            main_cooldown: 0,
            break_blocks: 0,
            regen: 0,
            wander: None,
            sent_heads: [0; 3],
            sent_invul: invul,
            sent_health: f32::NAN,
        }
    }
}

/// A wither skull in flight.
struct Skull {
    pos: [f64; 3],
    vel: [f64; 3],
    blue: bool,
    shooter: i32,
    age: u32,
    sent_pos: [f64; 3],
}

/// The boss bar of one wither, and who sees it.
struct Bar {
    uuid: u128,
    shown: HashSet<i32>,
    progress: f32,
    pos: [f64; 3],
}

/// Module-wide state, in `World::kin_state`.
#[derive(Default)]
struct State {
    skulls: HashMap<i32, Skull>,
    bars: HashMap<i32, Bar>,
}

enum Hit {
    Block,
    Player(i32),
    Mob(i32),
}

fn take_state(w: &mut World) -> Box<State> {
    w.kin_state
        .remove(KEY)
        .and_then(|s| s.downcast::<State>().ok())
        .unwrap_or_default()
}

fn put_state(w: &mut World, state: Box<State>) {
    w.kin_state.insert(KEY, state);
}

fn wither_of(mob: &Mob) -> Option<&Wither> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Wither>())
}

fn wither_mut(mob: &mut Mob) -> Option<&mut Wither> {
    mob.kin.as_mut().and_then(|k| k.downcast_mut::<Wither>())
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn normalized(v: [f64; 3]) -> [f64; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    v.map(|x| x / length)
}

fn cell(at: [f64; 3]) -> BlockPos {
    BlockPos::new(
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    )
}

fn armoured(mob: &Mob) -> bool {
    mob.health <= mob.max_health() / 2.0
}

/// The health viewers see: during the spawning phase it grows from a third to full as
/// the bar fills. The wither itself is kept at full health meanwhile, so that damage
/// reaching it by a path that skips `damage_taken` cannot kill it before it wakes (each
/// tick puts its health back).
fn shown_health(mob: &Mob, wi: &Wither) -> f32 {
    if wi.invul <= 0 {
        return mob.health;
    }
    let max = mob.max_health();
    let done = 1.0 - wi.invul as f32 / SPAWN_TICKS as f32;
    max / 3.0 + max * 2.0 / 3.0 * done
}

/// Where a head's skulls come from: the middle one 3 blocks up, the side ones 2.2 up
/// and 1.3 to either side.
fn head_pos(mob: &Mob, head: usize) -> [f64; 3] {
    if head == 0 {
        return [mob.pos[0], mob.pos[1] + 3.0, mob.pos[2]];
    }
    let side = if head == 1 { -1.0 } else { 1.0 };
    let yaw = f64::from(mob.yaw).to_radians();
    [
        mob.pos[0] + yaw.cos() * 1.3 * side,
        mob.pos[1] + 2.2,
        mob.pos[2] + yaw.sin() * 1.3 * side,
    ]
}

fn wither_data(id: i32, wi: &Wither, health: Option<f32>) -> Writer {
    let mut packet = Writer::packet(clientbound::SET_ENTITY_DATA).varint(id);
    for (i, &index) in DATA_HEAD_TARGETS.iter().enumerate() {
        packet = packet
            .u8(index)
            .varint(SERIALIZER_VARINT)
            .varint(wi.heads[i]);
    }
    packet = packet
        .u8(DATA_INVUL)
        .varint(SERIALIZER_VARINT)
        .varint(wi.invul);
    if let Some(health) = health {
        packet = packet.u8(DATA_HEALTH).varint(SERIALIZER_FLOAT).f32(health);
    }
    packet.u8(0xFF)
}

fn skull_spawn(id: i32, s: &Skull) -> Writer {
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::WITHER_SKULL)
        .f64(s.pos[0])
        .f64(s.pos[1])
        .f64(s.pos[2])
        .lp_vec3(s.vel)
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(s.shooter)
}

fn skull_blue_data(id: i32) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_SKULL_DANGEROUS)
        .varint(SERIALIZER_BOOL)
        .bool(true)
        .u8(0xFF)
}

fn bar_add(bar: &Bar, name: &str) -> Writer {
    Writer::packet(clientbound::BOSS_EVENT)
        .uuid(bar.uuid)
        .varint(BOSS_ADD)
        .text(name)
        .f32(bar.progress)
        .varint(BOSS_PURPLE)
        .varint(0) // one bar
        .u8(BOSS_DARKEN_SKY)
}

fn bar_remove(uuid: u128) -> Writer {
    Writer::packet(clientbound::BOSS_EVENT)
        .uuid(uuid)
        .varint(BOSS_REMOVE)
}

pub(super) fn spawned(_w: &mut World, mob: &mut Mob, _biome: &str) {
    mob.persistent = true;
    mob.kin = Some(Box::new(Wither::new(0)));
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    // A wither from a spawn egg or a command wakes at once.
    let mut kin = match mob.kin.take() {
        Some(k) if k.is::<Wither>() => k,
        _ => Box::new(Wither::new(0)),
    };
    let mut state = take_state(w);
    if let Some(wi) = kin.downcast_mut::<Wither>() {
        tick_wither(w, &mut state, id, mob, wi);
    }
    put_state(w, state);
    mob.kin = Some(kin);
    true // it moves itself
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

pub(super) fn hurt(_w: &mut World, _id: i32, mob: &mut Mob, _attacker: Option<i32>) {
    if let Some(wi) = wither_mut(mob) {
        if wi.break_blocks == 0 {
            wi.break_blocks = BREAK_DELAY;
        }
    }
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    match wither_of(mob) {
        Some(wi) => vec![wither_data(id, wi, Some(shown_health(mob, wi)))],
        None => Vec::new(),
    }
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    nbt.insert("Invul", wither_of(mob).map_or(0, |wi| wi.invul));
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let invul = nbt
        .int("Invul")
        .unwrap_or(0)
        .clamp(0, i64::from(SPAWN_TICKS)) as i32;
    mob.persistent = true;
    mob.kin = Some(Box::new(Wither::new(invul)));
}

pub(super) fn damage_taken(
    _w: &mut World,
    _id: i32,
    mob: &mut Mob,
    damage: f32,
    damage_type: &str,
    _attacker: Option<i32>,
) -> f32 {
    let armour = armoured(mob);
    let Some(wi) = wither_mut(mob) else {
        return damage;
    };
    // Every hit, even one it shrugs off, brings the side heads' blue skulls closer.
    for c in &mut wi.blue_count {
        *c += BLUE_PER_HURT;
    }
    if wi.invul > 0 || (armour && ARMOUR_STOPS.contains(&damage_type)) {
        return 0.0;
    }
    damage
}

pub(super) fn tick_world(w: &mut World) {
    if !w.kin_state.contains_key(KEY) {
        return;
    }
    let mut state = take_state(w);
    tick_skulls(w, &mut state);
    // Bars of withers that died or went away; a dead one leaves its nether star.
    let ended: Vec<i32> = state
        .bars
        .keys()
        .copied()
        .filter(|id| w.mobs.get(id).is_none_or(|m| !m.alive()))
        .collect();
    for id in ended {
        let Some(bar) = state.bars.remove(&id) else {
            continue;
        };
        for pid in bar.shown {
            if w.players.contains_key(&pid) {
                w.send(pid, bar_remove(bar.uuid));
            }
        }
        let died = w
            .mobs
            .get(&id)
            .is_some_and(|m| m.kind == MobKind::Wither && !m.alive());
        if died && crate::rules::bool("mob_drops") {
            if let Some(star) = ItemInfo::id_of("minecraft:nether_star") {
                w.scatter_item(bar.pos, Stack::new(star, 1));
            }
        }
    }
    put_state(w, state);
}

pub(super) fn block_placed(w: &mut World, player: i32, pos: BlockPos, state: u16) {
    if !is_wither_skull(state) || crate::rules::difficulty() == Difficulty::Peaceful {
        return;
    }
    let Some(found) = find_structure(w, pos) else {
        return;
    };
    let (base, cells) = found;
    for &q in &cells {
        let old = w.block(q);
        w.level_event(LEVEL_EVENT_BLOCK_DESTROYED, q, i32::from(old));
        w.remove_block_entity(q);
        w.set_block(q, block::AIR);
    }
    for &q in &cells {
        w.update_neighbors(q);
    }
    let at = [
        f64::from(base.x) + 0.5,
        f64::from(base.y) + 0.55,
        f64::from(base.z) + 0.5,
    ];
    let mut mob = Mob::new(MobKind::Wither, at, &mut w.rng);
    mob.persistent = true;
    mob.health = mob.max_health();
    mob.kin = Some(Box::new(Wither::new(SPAWN_TICKS)));
    w.mobs.insert(next_entity_id(), mob);
    // Players close by summoned it.
    let near: Vec<i32> = w
        .players
        .iter()
        .filter(|(&pid, p)| pid == player || (0..3).all(|i| (p.pos[i] - at[i]).abs() <= 50.0))
        .map(|(&pid, _)| pid)
        .collect();
    for pid in near {
        w.advance(pid, Trigger::SummonedEntity(entity_type::WITHER));
    }
}

pub(super) fn block_broken(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

pub(super) fn block_tick(_w: &mut World, _pos: BlockPos, _state: u16) -> bool {
    false
}

pub(super) fn ticks_randomly(_state: u16) -> bool {
    false
}

fn is_wither_skull(state: u16) -> bool {
    matches!(
        blocks::name(state),
        "minecraft:wither_skeleton_skull" | "minecraft:wither_skeleton_wall_skull"
    )
}

fn is_soul_block(state: u16) -> bool {
    matches!(
        blocks::name(state),
        "minecraft:soul_sand" | "minecraft:soul_soil"
    )
}

/// The wither structure a skull just placed at `pos` completes, if any: the foot of
/// the T and the seven blocks it is made of.
fn find_structure(w: &World, pos: BlockPos) -> Option<(BlockPos, Vec<BlockPos>)> {
    const UPS: [[i32; 3]; 6] = [
        [0, 1, 0],
        [0, -1, 0],
        [1, 0, 0],
        [-1, 0, 0],
        [0, 0, 1],
        [0, 0, -1],
    ];
    const AXES: [[i32; 3]; 3] = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
    let at = |p: BlockPos, v: [i32; 3], n: i32| {
        BlockPos::new(p.x + v[0] * n, p.y + v[1] * n, p.z + v[2] * n)
    };
    let get = |p: BlockPos| w.loaded_block(p);
    for up in UPS {
        for arm in AXES {
            if (0..3).any(|i| arm[i] != 0 && up[i] != 0) {
                continue; // the arms go across
            }
            for k in -1..=1 {
                let top = at(pos, arm, -k);
                let middle = at(top, up, -1);
                let base = at(top, up, -2);
                let skulls = [at(top, arm, -1), top, at(top, arm, 1)];
                let soul = [at(middle, arm, -1), middle, at(middle, arm, 1), base];
                let corners = [at(base, arm, -1), at(base, arm, 1)];
                let built = skulls.iter().all(|&q| get(q).is_some_and(is_wither_skull))
                    && soul.iter().all(|&q| get(q).is_some_and(is_soul_block))
                    && corners.iter().all(|&q| get(q).is_some_and(is_air));
                if built {
                    let mut cells = skulls.to_vec();
                    cells.extend(soul);
                    return Some((base, cells));
                }
            }
        }
    }
    None
}

fn tick_wither(w: &mut World, state: &mut State, id: i32, mob: &mut Mob, wi: &mut Wither) {
    mob.persistent = true;
    // Immune to every status effect (instant ones are dealt with as they come). Viewers
    // were told of any it was given, so they are told it lost them.
    for e in std::mem::take(&mut mob.effects) {
        w.send_to_viewers(mob, super::effects::remove_packet(id, e.id));
    }
    mob.knockback = [0.0; 2];
    mob.target = None;
    mob.path.clear();
    let max = mob.max_health();
    if wi.invul > 0 {
        wi.invul -= 1;
        // Its health shown grows (see `shown_health`); its own stays full.
        mob.health = max;
        mob.vel = [0.0; 3];
        if wi.invul == 0 {
            let centre = [mob.pos[0], mob.pos[1] + mob.size().1 / 2.0, mob.pos[2]];
            explode_sparing_withers(w, centre, SPAWN_POWER);
            w.broadcast(
                Writer::packet(clientbound::LEVEL_EVENT)
                    .i32(LEVEL_EVENT_SPAWNED)
                    .position(cell(mob.pos))
                    .i32(0)
                    .bool(true),
                None,
            );
        }
        sync(w, state, id, mob, wi);
        return;
    }
    wi.regen += 1;
    if wi.regen >= REGEN_TICKS {
        wi.regen = 0;
        mob.health = (mob.health + REGEN).min(max);
    }
    if wi.break_blocks > 0 {
        wi.break_blocks -= 1;
        if wi.break_blocks == 0 && crate::rules::bool("mob_griefing") {
            break_around(w, mob);
        }
    }
    let eyes = head_pos(mob, 0);
    // The middle head keeps its target while it is about, or looks for the nearest.
    if target_body(w, wi.heads[0], mob.pos).is_none() {
        wi.heads[0] = 0;
        if wi.regen.is_multiple_of(10) {
            wi.heads[0] = pick_target(w, mob, eyes, true);
        }
    }
    mob.hunting = Some(wi.heads[0]).filter(|t| w.players.contains_key(t));
    let main = target_body(w, wi.heads[0], mob.pos);
    wi.main_cooldown = wi.main_cooldown.saturating_sub(1);
    if let Some((feet, height)) = main {
        let aim = [feet[0], feet[1] + height * 0.425, feet[2]];
        if wi.main_cooldown == 0 && w.line_of_sight(eyes, aim) {
            wi.main_cooldown = MAIN_COOLDOWN;
            let blue = w.rng.below(1000) < MAIN_BLUE_IN_1000;
            shoot(w, state, id, mob, 0, aim, blue);
        }
    }
    side_heads(w, state, id, mob, wi);
    fly(w, mob, wi, main.map(|(feet, _)| feet));
    sync(w, state, id, mob, wi);
}

/// The side heads' turns, as the wiki describes them.
fn side_heads(w: &mut World, state: &mut State, id: i32, mob: &Mob, wi: &mut Wither) {
    let fierce = matches!(
        crate::rules::difficulty(),
        Difficulty::Normal | Difficulty::Hard
    );
    for i in 0..2 {
        let head = i + 1;
        if wi.side_cooldown[i] > 0 {
            wi.side_cooldown[i] -= 1;
            if wi.side_cooldown[i] > 0 {
                continue;
            }
        }
        wi.side_cooldown[i] = SIDE_TURN.0 + w.rng.below(SIDE_TURN.1);
        wi.blue_count[i] += 1;
        if wi.blue_count[i] > BLUE_AFTER_TURNS && fierce {
            let from = head_pos(mob, head);
            let r = |rng: &mut Rng| rng.f64() * 2.0 - 1.0;
            let aim = [
                from[0] + r(&mut w.rng) * 10.0,
                from[1] - w.rng.f64() * 10.0,
                from[2] + r(&mut w.rng) * 10.0,
            ];
            shoot(w, state, id, mob, head, aim, true);
            wi.blue_count[i] = 0;
        }
        match target_body(w, wi.heads[head], mob.pos) {
            Some((feet, height)) => {
                let from = head_pos(mob, head);
                let aim = [feet[0], feet[1] + height * 0.425, feet[2]];
                if w.line_of_sight(from, aim) {
                    shoot(w, state, id, mob, head, aim, false);
                    wi.side_cooldown[i] = SIDE_RELOAD.0 + w.rng.below(SIDE_RELOAD.1);
                    wi.blue_count[i] = 0;
                } else {
                    wi.heads[head] = 0;
                }
            }
            None => {
                let from = head_pos(mob, head);
                wi.heads[head] = pick_target(w, mob, from, false);
            }
        }
    }
}

/// Whether the wither goes after `target`, and if so its feet and height.
fn target_body(w: &World, target: i32, from: [f64; 3]) -> Option<([f64; 3], f64)> {
    if target == 0 {
        return None;
    }
    let body = if let Some(p) = w.players.get(&target) {
        (p.alive && !matches!(p.mode, GAME_MODE_CREATIVE | GAME_MODE_SPECTATOR))
            .then_some((p.pos, PLAYER_HEIGHT))
    } else {
        w.mobs
            .get(&target)
            .filter(|m| hostile_to(m))
            .map(|m| (m.pos, m.size().1))
    }?;
    (distance(body.0, from) <= RANGE).then_some(body)
}

/// Mobs the wither attacks: all living ones but the undead and ghasts.
fn hostile_to(m: &Mob) -> bool {
    m.alive() && !m.kind.is_undead() && m.kind != MobKind::Ghast
}

/// A target in range and in sight of `eyes`: the nearest, or one at random.
fn pick_target(w: &mut World, mob: &Mob, eyes: [f64; 3], nearest: bool) -> i32 {
    // Those in range first (cheap), then sight tested only until one is seen: nearest
    // first, or in a random order.
    let mut candidates: Vec<(f64, i32, [f64; 3])> = Vec::new();
    let mut offer = |id: i32, at: [f64; 3], height: f64| {
        let d = distance(at, mob.pos);
        if d <= RANGE {
            candidates.push((d, id, [at[0], at[1] + height * 0.5, at[2]]));
        }
    };
    for (&pid, p) in &w.players {
        if p.alive && !matches!(p.mode, GAME_MODE_CREATIVE | GAME_MODE_SPECTATOR) {
            offer(pid, p.pos, PLAYER_HEIGHT);
        }
    }
    for (&mid, m) in &w.mobs {
        if hostile_to(m) {
            offer(mid, m.pos, m.size().1);
        }
    }
    if nearest {
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    } else {
        for i in (1..candidates.len()).rev() {
            let j = w.rng.below(i as u32 + 1) as usize;
            candidates.swap(i, j);
        }
    }
    candidates
        .into_iter()
        .find(|&(_, _, aim)| w.line_of_sight(eyes, aim))
        .map_or(0, |(_, id, _)| id)
}

/// Fires a skull from a head towards `aim`.
fn shoot(
    w: &mut World,
    state: &mut State,
    shooter: i32,
    mob: &Mob,
    head: usize,
    aim: [f64; 3],
    blue: bool,
) {
    let from = head_pos(mob, head);
    let dir = normalized([aim[0] - from[0], aim[1] - from[1], aim[2] - from[2]]);
    let skull = Skull {
        pos: from,
        vel: dir.map(|v| v * SKULL_ACCELERATION),
        blue,
        shooter,
        age: 0,
        sent_pos: from,
    };
    let sid = next_entity_id();
    w.broadcast(skull_spawn(sid, &skull), None);
    if blue {
        w.broadcast(skull_blue_data(sid), None);
    }
    state.skulls.insert(sid, skull);
    w.level_event(LEVEL_EVENT_SHOOT, cell(from), 0);
}

/// Flies after the target (above it, close by), or drifts about without climbing.
fn fly(w: &mut World, mob: &mut Mob, wi: &mut Wither, target: Option<[f64; 3]>) {
    let mut want = [0.0; 3];
    if let Some(t) = target {
        wi.wander = None;
        let hover = if armoured(mob) { t[1] } else { t[1] + HOVER };
        want[1] = ((hover - mob.pos[1]) * 0.1).clamp(-CLIMB, CLIMB);
        let (dx, dz) = (t[0] - mob.pos[0], t[2] - mob.pos[2]);
        let flat = dx.hypot(dz);
        if flat > KEEP_NEAR {
            want[0] = dx / flat * SPEED;
            want[2] = dz / flat * SPEED;
        }
        mob.yaw = yaw_towards(mob.pos, t);
        mob.head_yaw = mob.yaw;
    } else {
        if wi.wander.is_none() && w.rng.below(80) == 0 {
            let r = |rng: &mut Rng| (rng.f64() * 2.0 - 1.0) * 10.0;
            wi.wander = Some([mob.pos[0] + r(&mut w.rng), mob.pos[2] + r(&mut w.rng)]);
        }
        if let Some(g) = wi.wander {
            let (dx, dz) = (g[0] - mob.pos[0], g[1] - mob.pos[2]);
            let flat = dx.hypot(dz);
            if flat < 1.0 {
                wi.wander = None;
            } else {
                want[0] = dx / flat * IDLE_SPEED;
                want[2] = dz / flat * IDLE_SPEED;
                mob.yaw = yaw_towards(mob.pos, [g[0], mob.pos[1], g[1]]);
                mob.head_yaw = mob.yaw;
            }
        }
    }
    let (width, height) = mob.size();
    let half = width / 2.0;
    let mut pos = mob.pos;
    for axis in 0..3 {
        mob.vel[axis] = mob.vel[axis] * 0.6 + want[axis] * 0.4;
        let delta = w.clip_move(pos, half, height, axis, mob.vel[axis]);
        if (delta - mob.vel[axis]).abs() > 1e-9 {
            mob.vel[axis] = 0.0;
            if axis != 1 {
                wi.wander = None;
            }
        }
        pos[axis] += delta;
    }
    mob.pos = pos;
}

/// Tells viewers what changed, and keeps the boss bar in step with who sees it.
fn sync(w: &mut World, state: &mut State, id: i32, mob: &Mob, wi: &mut Wither) {
    let shown = shown_health(mob, wi);
    let half = mob.max_health() / 2.0;
    let health_changed = (shown - wi.sent_health).abs() >= 1.0
        || wi.sent_health.is_nan()
        || (shown <= half) != (wi.sent_health <= half);
    if wi.heads != wi.sent_heads || wi.invul != wi.sent_invul || health_changed {
        wi.sent_heads = wi.heads;
        wi.sent_invul = wi.invul;
        let health = health_changed.then_some(shown);
        if health_changed {
            wi.sent_health = shown;
        }
        w.send_to_viewers(mob, wither_data(id, wi, health));
    }
    let progress = (shown / mob.max_health()).clamp(0.0, 1.0);
    let bar = state.bars.entry(id).or_insert_with(|| Bar {
        uuid: mob.uuid ^ 0x5749_5448_4552_0000_0000_0000_0000_0000,
        shown: HashSet::new(),
        progress,
        pos: mob.pos,
    });
    bar.pos = mob.pos;
    let gone: Vec<i32> = bar
        .shown
        .iter()
        .copied()
        .filter(|p| !mob.viewers.contains(p))
        .collect();
    let new: Vec<i32> = mob
        .viewers
        .iter()
        .copied()
        .filter(|p| !bar.shown.contains(p))
        .collect();
    let moved = (progress - bar.progress).abs() > 1e-4;
    bar.progress = progress;
    let uuid = bar.uuid;
    for pid in &gone {
        bar.shown.remove(pid);
    }
    bar.shown.extend(new.iter().copied());
    let name = mob.name.unwrap_or("Wither");
    let shown: Vec<i32> = if moved {
        bar.shown.iter().copied().collect()
    } else {
        Vec::new()
    };
    for pid in gone {
        w.send(pid, bar_remove(uuid));
    }
    for pid in new {
        w.send(pid, bar_add(bar, name));
    }
    for pid in shown {
        w.send(
            pid,
            Writer::packet(clientbound::BOSS_EVENT)
                .uuid(uuid)
                .varint(BOSS_HEALTH)
                .f32(progress),
        );
    }
}

/// Breaks the blocks of a 3x4x3 box from the block of its tail up.
fn break_around(w: &mut World, mob: &Mob) {
    let feet = cell(mob.pos);
    let mut broken = Vec::new();
    for dy in 0..4 {
        for dx in -1..=1 {
            for dz in -1..=1 {
                let pos = BlockPos::new(feet.x + dx, feet.y + dy, feet.z + dz);
                if wither_breaks(w, pos, false) {
                    broken.push(pos);
                }
            }
        }
    }
    if broken.is_empty() {
        return;
    }
    for pos in broken {
        w.update_neighbors(pos);
    }
    w.level_event(LEVEL_EVENT_BREAK_BLOCK, feet, 0);
}

/// Breaks one block the way the wither does, dropping what it would. Fluids only go
/// when `fluids` is set (blue skulls). Returns whether something broke.
fn wither_breaks(w: &mut World, pos: BlockPos, fluids: bool) -> bool {
    if !(MIN_Y..=MAX_Y).contains(&pos.y) {
        return false;
    }
    let Some(state) = w.loaded_block(pos) else {
        return false;
    };
    if is_air(state)
        || block_has_tag(state, "minecraft:wither_immune")
        || BlockInfo::of(state).hardness < 0.0
    {
        return false;
    }
    // Liquids stay, but for a blue skull's. A waterlogged block (or an underwater
    // plant) breaks and leaves its water behind.
    let liquid = is_liquid(state);
    if liquid && !fluids {
        return false;
    }
    let after = if !liquid && fluid_at(state).is_some() {
        block::WATER
    } else {
        block::AIR
    };
    if !liquid {
        let drops = w.loot.block_drops(&mut LootContext {
            state,
            tool: 0,
            rng: &mut w.rng,
            enchants: Default::default(),
            entity: Default::default(),
        });
        for (item, count) in drops {
            w.drop_at_block(pos, Stack::new(item, count));
        }
    }
    w.level_event(LEVEL_EVENT_BLOCK_DESTROYED, pos, i32::from(state));
    w.remove_block_entity(pos);
    w.set_block(pos, after);
    true
}

/// A block that is only liquid: water, lava, or a bubble column.
fn is_liquid(state: u16) -> bool {
    let name = blocks::name(state);
    Fluid::from_name(name).is_some() || name == "minecraft:bubble_column"
}

/// An explosion that leaves withers unhurt (they are not hurt by their own kind).
fn explode_sparing_withers(w: &mut World, at: [f64; 3], power: f32) {
    let reach = f64::from(power) * 2.0 + 4.0;
    let near: Vec<i32> = w
        .mobs
        .iter()
        .filter(|(_, m)| m.kind == MobKind::Wither && distance(m.pos, at) <= reach)
        .map(|(&id, _)| id)
        .collect();
    let spared: Vec<(i32, Mob)> = near
        .into_iter()
        .filter_map(|id| w.mobs.remove(&id).map(|m| (id, m)))
        .collect();
    w.explode(at, power, "Wither");
    for (id, m) in spared {
        w.mobs.insert(id, m);
    }
}

fn tick_skulls(w: &mut World, state: &mut State) {
    let ids: Vec<i32> = state.skulls.keys().copied().collect();
    for id in ids {
        let Some(mut s) = state.skulls.remove(&id) else {
            continue;
        };
        s.age += 1;
        if s.age > SKULL_LIFETIME || !w.chunks.contains_key(&cell(s.pos).chunk()) {
            w.broadcast(items::remove_entity(id), None);
            continue;
        }
        if let Some(hit) = skull_flight(w, &mut s) {
            w.broadcast(items::remove_entity(id), None);
            skull_hits(w, &s, hit);
            continue;
        }
        // Speeding up along the flight, as clients also do.
        let dir = normalized(s.vel);
        let inertia = if s.blue { BLUE_INERTIA } else { SKULL_INERTIA };
        s.vel = [0, 1, 2].map(|i| (s.vel[i] + dir[i] * SKULL_ACCELERATION) * inertia);
        if s.age.is_multiple_of(10) {
            s.sent_pos = s.pos;
            w.broadcast(
                Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                    .varint(id)
                    .varint(0)
                    .f64(s.pos[0])
                    .f64(s.pos[1])
                    .f64(s.pos[2])
                    .f32(0.0)
                    .f32(0.0)
                    .bool(false),
                None,
            );
        }
        state.skulls.insert(id, s);
    }
}

/// Moves a skull along its velocity, stopping at the first block or entity. Withers
/// are flown through.
fn skull_flight(w: &World, s: &mut Skull) -> Option<Hit> {
    let speed = (s.vel[0] * s.vel[0] + s.vel[1] * s.vel[1] + s.vel[2] * s.vel[2]).sqrt();
    let steps = (speed / SAMPLE).ceil().max(1.0) as usize;
    let step = s.vel.map(|v| v / steps as f64);
    // What the skull could reach this tick, gathered once: players before mobs, each
    // with its feet, width and height.
    let near = |pos: [f64; 3], size: f64| distance(pos, s.pos) <= speed + 1.0 + size;
    let mut bodies: Vec<(Hit, [f64; 3], f64, f64)> = w
        .players
        .iter()
        .filter(|(_, p)| {
            p.alive && p.mode != GAME_MODE_SPECTATOR && near(p.pos, 0.6 + PLAYER_HEIGHT)
        })
        .map(|(&pid, p)| (Hit::Player(pid), p.pos, 0.6, PLAYER_HEIGHT))
        .collect();
    bodies.extend(
        w.mobs
            .iter()
            .filter(|(&mid, m)| {
                let (width, height) = m.size();
                mid != s.shooter
                    && m.kind != MobKind::Wither
                    && m.alive()
                    && near(m.pos, width + height)
            })
            .map(|(&mid, m)| {
                let (width, height) = m.size();
                (Hit::Mob(mid), m.pos, width, height)
            }),
    );
    for _ in 0..steps {
        let next = [0, 1, 2].map(|i| s.pos[i] + step[i]);
        let inside = |pos: [f64; 3], width: f64, height: f64| {
            let half = width / 2.0 + SKULL_REACH;
            (next[0] - pos[0]).abs() <= half
                && (next[2] - pos[2]).abs() <= half
                && next[1] >= pos[1] - SKULL_REACH
                && next[1] <= pos[1] + height + SKULL_REACH
        };
        if let Some(i) = bodies
            .iter()
            .position(|&(_, pos, width, height)| inside(pos, width, height))
        {
            s.pos = next;
            return Some(bodies.swap_remove(i).0);
        }
        if w.point_in_block(next) {
            return Some(Hit::Block);
        }
        s.pos = next;
    }
    None
}

fn wither_effect() -> Option<levitas_registry::Effect> {
    let duration = match crate::rules::difficulty() {
        Difficulty::Normal => WITHER_NORMAL_TICKS,
        Difficulty::Hard => WITHER_HARD_TICKS,
        _ => return None,
    };
    Some(levitas_registry::Effect {
        id: levitas_registry::mob_effect::WITHER,
        duration,
        amplifier: WITHER_AMPLIFIER,
    })
}

fn skull_hits(w: &mut World, s: &Skull, hit: Hit) {
    let shooter = w.mobs.contains_key(&s.shooter).then_some(s.shooter);
    match hit {
        Hit::Player(pid) => {
            // A projectile: projectile protection and shields count, and a blocked skull
            // does not push the wither back. (Vanilla's own `wither_skull` damage type
            // and death message need a `Damage` variant of their own.)
            if w.damage_from(pid, SKULL_DAMAGE, Damage::Arrow("Wither"), shooter) {
                if let Some(effect) = wither_effect() {
                    w.add_effect(pid, effect);
                }
                if w.players.get(&pid).is_some_and(|p| !p.alive) {
                    killed(w, s.shooter, None);
                }
            }
        }
        Hit::Mob(mid) => {
            let away = normalized([s.vel[0], 0.0, s.vel[2]]);
            if w.hit_mob(
                mid,
                SKULL_DAMAGE,
                None,
                [away[0], away[2]],
                "minecraft:wither_skull",
            ) {
                match w.mobs.get(&mid).map(|m| (m.alive(), m.pos)) {
                    Some((true, _)) => {
                        if let Some(effect) = wither_effect() {
                            w.add_mob_effect(mid, effect);
                        }
                    }
                    Some((false, at)) => killed(w, s.shooter, Some(at)),
                    None => {}
                }
            }
        }
        Hit::Block => {}
    }
    explode_sparing_withers(w, s.pos, SKULL_POWER);
    if s.blue && crate::rules::bool("mob_griefing") {
        // Blue skulls break nearly anything where they land.
        let at = cell(s.pos);
        let mut broken = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx * dx + dy * dy + dz * dz > 1 {
                        continue;
                    }
                    let pos = BlockPos::new(at.x + dx, at.y + dy, at.z + dz);
                    if wither_breaks(w, pos, true) {
                        broken.push(pos);
                    }
                }
            }
        }
        for pos in broken {
            w.update_neighbors(pos);
        }
    }
}

/// A skull from `shooter` killed something: the wither heals, and a mob leaves a
/// wither rose where it fell.
fn killed(w: &mut World, shooter: i32, mob_at: Option<[f64; 3]>) {
    if let Some(m) = w
        .mobs
        .get_mut(&shooter)
        .filter(|m| m.kind == MobKind::Wither && m.alive())
    {
        m.health = (m.health + KILL_HEAL).min(m.max_health());
    }
    let Some(at) = mob_at else {
        return;
    };
    let feet = cell(at);
    let below = BlockPos::new(feet.x, feet.y - 1, feet.z);
    let plant = crate::rules::bool("mob_griefing")
        && w.loaded_block(feet).is_some_and(is_air)
        && w.loaded_block(below)
            .is_some_and(|g| block_has_tag(g, "minecraft:supports_wither_rose"));
    if plant {
        w.set_block_updating(feet, block::WITHER_ROSE);
    } else if let Some(rose) = ItemInfo::id_of("minecraft:wither_rose") {
        w.scatter_item(at, Stack::new(rose, 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-withers-{}-{}",
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
        for cx in -3..=3 {
            for cz in -3..=3 {
                w.chunk(cx, cz);
            }
        }
        w
    }

    fn add_player(w: &mut World, at: [f64; 3]) -> i32 {
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 21,
            properties: Vec::new(),
        };
        w.players.insert(id, Player::new(profile, out, at));
        id
    }

    fn awake(w: &mut World, at: [f64; 3]) -> (i32, Mob) {
        let mut mob = Mob::new(MobKind::Wither, at, &mut w.rng);
        mob.kin = Some(Box::new(Wither::new(0)));
        (next_entity_id(), mob)
    }

    fn state(mob: &Mob) -> &Wither {
        wither_of(mob).expect("wither state")
    }

    fn skulls(w: &World) -> usize {
        w.kin_state
            .get(KEY)
            .and_then(|s| s.downcast_ref::<State>())
            .map_or(0, |s| s.skulls.len())
    }

    fn build(w: &mut World, base: BlockPos, arm: [i32; 3], skull_last: bool) -> BlockPos {
        let at = |p: BlockPos, n: i32, up: i32| {
            BlockPos::new(p.x + arm[0] * n, p.y + up, p.z + arm[2] * n)
        };
        w.set_block(base, block::SOUL_SAND);
        for n in -1..=1 {
            w.set_block(at(base, n, 1), block::SOUL_SOIL);
        }
        for n in [-1, 1] {
            w.set_block(at(base, n, 0), block::AIR);
            w.set_block(at(base, n, 2), block::WITHER_SKELETON_SKULL);
        }
        let last = at(base, 0, 2);
        if skull_last {
            w.set_block(last, block::WITHER_SKELETON_SKULL);
        }
        last
    }

    fn withers(w: &World) -> Vec<i32> {
        w.mobs
            .iter()
            .filter(|(_, m)| m.kind == MobKind::Wither)
            .map(|(&id, _)| id)
            .collect()
    }

    #[test]
    fn soul_sand_and_three_skulls_make_a_wither_either_way() {
        let mut w = world();
        let player = add_player(&mut w, [0.5, -60.0, 8.5]);
        for (base, arm) in [
            (BlockPos::new(2, -59, 2), [1, 0, 0]),
            (BlockPos::new(20, -59, 20), [0, 0, 1]),
        ] {
            let last = build(&mut w, base, arm, true);
            let before = withers(&w).len();
            let skull = w.block(last);
            block_placed(&mut w, player, last, skull);
            assert_eq!(withers(&w).len(), before + 1, "built along {arm:?}");
            assert!(is_air(w.block(base)) && is_air(w.block(last)));
            assert!(is_air(w.block(BlockPos::new(base.x, base.y + 1, base.z))));
        }
        let id = withers(&w)[0];
        let mob = &w.mobs[&id];
        assert_eq!(state(mob).invul, SPAWN_TICKS);
        assert!(mob.persistent, "never despawns");
        assert!(shown_health(mob, state(mob)) < mob.max_health());
        // A block in a lower corner spoils it; so does a soul block placed last.
        let base = BlockPos::new(-20, -59, -20);
        let last = build(&mut w, base, [1, 0, 0], true);
        w.set_block(BlockPos::new(base.x + 1, base.y, base.z), block::STONE);
        let before = withers(&w).len();
        block_placed(&mut w, player, last, block::WITHER_SKELETON_SKULL);
        block_placed(&mut w, player, base, block::SOUL_SAND);
        assert_eq!(withers(&w).len(), before);
    }

    #[test]
    fn the_spawning_phase_is_invulnerable_and_ends_in_a_blast() {
        let mut w = world();
        let id = next_entity_id();
        let mut mob = Mob::new(MobKind::Wither, [0.5, -59.0, 0.5], &mut w.rng);
        mob.kin = Some(Box::new(Wither::new(SPAWN_TICKS)));
        let start = mob.pos;
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 20.0, "minecraft:player_attack", None),
            0.0
        );
        let mut last = shown_health(&mob, state(&mob));
        assert!(last < mob.max_health() / 2.0);
        for _ in 0..SPAWN_TICKS - 1 {
            // A blow that skips `damage_taken` cannot kill it, and is undone.
            mob.health -= 150.0;
            tick(&mut w, id, &mut mob);
            assert_eq!(mob.health, mob.max_health());
            let shown = shown_health(&mob, state(&mob));
            assert!(shown >= last, "the bar fills");
            last = shown;
        }
        assert_eq!(mob.pos, start, "it does not move");
        assert!(!is_air(w.block(BlockPos::new(0, -61, 0))));
        tick(&mut w, id, &mut mob);
        assert_eq!(state(&mob).invul, 0);
        assert_eq!(mob.health, mob.max_health());
        assert!(is_air(w.block(BlockPos::new(0, -61, 0))), "the blast");
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 20.0, "minecraft:player_attack", None),
            20.0
        );
    }

    #[test]
    fn wither_armour_below_half_health_stops_arrows() {
        let mut w = world();
        let (id, mut mob) = awake(&mut w, [0.5, -55.0, 0.5]);
        let arrow =
            |w: &mut World, mob: &mut Mob| damage_taken(w, id, mob, 6.0, "minecraft:arrow", None);
        assert_eq!(arrow(&mut w, &mut mob), 6.0);
        mob.health = mob.max_health() / 2.0 - 1.0;
        assert_eq!(arrow(&mut w, &mut mob), 0.0);
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 6.0, "minecraft:trident", None),
            0.0
        );
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 6.0, "minecraft:player_attack", None),
            6.0
        );
    }

    #[test]
    fn it_regenerates_and_shrugs_off_effects() {
        let mut w = world();
        let (id, mut mob) = awake(&mut w, [0.5, -50.0, 0.5]);
        mob.health = 100.0;
        mob.effects.push(super::super::effects::ActiveEffect {
            id: levitas_registry::mob_effect::POISON,
            amplifier: 0,
            duration: 100,
        });
        for _ in 0..REGEN_TICKS * 3 {
            tick(&mut w, id, &mut mob);
        }
        assert_eq!(mob.health, 103.0);
        assert!(mob.effects.is_empty());
    }

    #[test]
    fn skulls_hit_players_and_wither_them() {
        let mut w = world();
        let alice = add_player(&mut w, [10.5, -60.0, 0.5]);
        let (id, mut mob) = awake(&mut w, [0.5, -60.0, 0.5]);
        for _ in 0..20 {
            tick(&mut w, id, &mut mob);
            if skulls(&w) > 0 {
                break;
            }
        }
        assert_eq!(state(&mob).heads[0], alice, "the middle head targets her");
        assert!(skulls(&w) > 0, "a skull is on its way");
        w.mobs.insert(id, mob);
        for _ in 0..100 {
            tick_world(&mut w);
            if skulls(&w) == 0 {
                break;
            }
        }
        assert_eq!(skulls(&w), 0);
        assert!(w.players[&alice].health < 20.0);
        // No Wither on Easy.
        let withered = w.players[&alice]
            .effect(levitas_registry::mob_effect::WITHER)
            .is_some();
        assert_eq!(withered, wither_effect().is_some());
    }

    #[test]
    fn it_flies_above_its_target_and_attacks_mobs_but_not_the_undead() {
        let mut w = world();
        let zombie = next_entity_id();
        w.mobs.insert(
            zombie,
            Mob::new(MobKind::Zombie, [6.5, -60.0, 0.5], &mut w.rng),
        );
        let cow = next_entity_id();
        w.mobs
            .insert(cow, Mob::new(MobKind::Cow, [12.5, -60.0, 0.5], &mut w.rng));
        let (id, mut mob) = awake(&mut w, [0.5, -60.0, 0.5]);
        for _ in 0..200 {
            tick(&mut w, id, &mut mob);
        }
        let wi = state(&mob);
        assert!(!wi.heads.contains(&zombie));
        assert_eq!(wi.heads[0], cow);
        let cow_pos = w.mobs[&cow].pos;
        assert!((mob.pos[0] - cow_pos[0]).hypot(mob.pos[2] - cow_pos[2]) < KEEP_NEAR + 0.5);
        assert!(mob.pos[1] > cow_pos[1] + 3.0, "hovers above it");
    }

    #[test]
    fn being_hurt_breaks_blocks_around_it_but_not_bedrock() {
        let mut w = world();
        let (id, mut mob) = awake(&mut w, [0.5, -59.0, 0.5]);
        let stone = BlockPos::new(1, -58, 0);
        let obsidian = BlockPos::new(-1, -57, 1);
        let bedrock = BlockPos::new(0, -56, -1);
        w.set_block(stone, block::STONE);
        w.set_block(obsidian, block::OBSIDIAN);
        w.set_block(bedrock, block::BEDROCK);
        hurt(&mut w, id, &mut mob, None);
        assert_eq!(state(&mob).break_blocks, BREAK_DELAY);
        // The last tick of the wait (it would drift about in the meantime).
        if let Some(wi) = wither_mut(&mut mob) {
            wi.break_blocks = 1;
        }
        tick(&mut w, id, &mut mob);
        assert!(is_air(w.block(stone)));
        assert!(is_air(w.block(obsidian)));
        assert_eq!(w.block(bedrock), block::BEDROCK);
    }

    #[test]
    fn withers_are_saved_and_show_a_boss_bar() {
        let mut w = world();
        let alice = add_player(&mut w, [0.5, -60.0, 0.5]);
        let (id, mut mob) = awake(&mut w, [0.5, -50.0, 0.5]);
        if let Some(wi) = wither_mut(&mut mob) {
            wi.invul = 100;
        }
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        assert_eq!(nbt.int("Invul"), Some(100));
        let mut back = Mob::new(MobKind::Wither, [0.5, -50.0, 0.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        assert_eq!(state(&back).invul, 100);
        assert!(back.persistent);
        assert_eq!(spawn_extras(id, &back).len(), 1);

        mob.viewers.insert(alice);
        tick(&mut w, id, &mut mob);
        let shown = |w: &World| {
            w.kin_state
                .get(KEY)
                .and_then(|s| s.downcast_ref::<State>())
                .is_some_and(|s| s.bars.get(&id).is_some_and(|b| b.shown.contains(&alice)))
        };
        assert!(shown(&w));
        // Dead, it drops a nether star and its bar goes.
        w.kill_mob(id, &mut mob);
        w.mobs.insert(id, mob);
        tick_world(&mut w);
        assert!(!shown(&w));
        assert!(w
            .items
            .values()
            .any(|i| ItemInfo::get(i.stack().item).unwrap().name == "minecraft:nether_star"));
    }

    #[test]
    fn waterlogged_blocks_break_and_leave_their_water() {
        let mut w = world();
        let (id, mut mob) = awake(&mut w, [0.5, -59.0, 0.5]);
        let slab = BlockPos::new(1, -58, 0);
        let water = BlockPos::new(-1, -58, 0);
        let wet = blocks::with_bool(block::OAK_SLAB, "waterlogged", true);
        w.set_block(slab, wet);
        w.set_block(water, block::WATER);
        if let Some(wi) = wither_mut(&mut mob) {
            wi.break_blocks = 1;
        }
        tick(&mut w, id, &mut mob);
        assert_eq!(
            w.block(slab),
            block::WATER,
            "the slab goes, its water stays"
        );
        assert_eq!(w.block(water), block::WATER, "liquids stay");
    }
}
