//! Breezes and their wind charges.
//!
//! Behaviour follows the Minecraft Wiki (Breeze, Wind Charge):
//! - A breeze goes for a survival player it can see within 16 blocks, or else an iron
//!   golem, and turns on any survival player who hurts it. It forgets a target that
//!   goes beyond its follow range or stays out of sight for five seconds.
//! - It moves by leaping: it compresses like a spring, then jumps up to 15 blocks
//!   across and 5 up. Near its target it hops away, far from it or out of sight it
//!   leaps closer, and in between it hops to the side, keeping its distance. Without a
//!   target it hops about now and then. In water or lava it floats up quickly, and
//!   from there it can jump only about one block up.
//! - It is immune to fall damage, and deflects every projectile but wind charges: the
//!   hit does nothing.
//! - Standing on the ground (or floating) with its target in sight within 16 blocks it
//!   inhales and shoots a wind charge (`breeze_wind_charge`), at most once every 32
//!   ticks. The charge flies straight and bursts on the first block or entity it
//!   touches: an entity hit directly takes 1 damage, and the burst throws everything
//!   within 6 blocks away from it, with 40% less knockback than an explosion of its
//!   size, without breaking blocks. The burst flips unpowered non-iron doors,
//!   trapdoors and fence gates, presses buttons, flips levers and puts out candles
//!   near it (only with `mobGriefing`), and rings bells.
//! - Sounds are `minecraft:entity.breeze.*`. Its pose shows inhaling, shooting and
//!   long jumping to clients.
//!
//! Its movement is its own (`tick` returns `true`, so the shared walking never runs):
//! gravity, collisions and knockback are worked out here, so falls never add up.

use std::collections::HashMap;
use std::f64::consts::PI;

use levitas_nbt::Compound;
use levitas_registry::sound_id;

use super::mobs::{yaw_towards, Mob, MobKind};
use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::blocks::{self, offset, prop, with_bool, Dir};
use crate::world::survival::Damage;

/// Where the wind charges in flight live in `World::kin_state`.
const STATE_KEY: &str = "breezes";
const DATA_POSE: u8 = 6;
const SERIALIZER_POSE: i32 = 20;
const POSE_STANDING: i32 = 0;
const POSE_LONG_JUMPING: i32 = 6;
const POSE_SHOOTING: i32 = 16;
const POSE_INHALING: i32 = 17;
const GAME_MODE_SPECTATOR: u8 = 3;
const PLAYER_SIZE: (f64, f64) = (0.6, 1.8);
const PLAYER_EYES: f64 = 1.62;
/// How far it notices targets and shoots at them.
const RANGE: f64 = 16.0;
/// Ticks between looks for a new target while it has none.
const SCAN_EVERY: u32 = 20;
/// Ticks out of sight before a target is forgotten.
const FORGET_AFTER: u32 = 100;
/// Ticks between attempts to shoot, the inhale before a shot and the shooting pose.
const SHOOT_COOLDOWN: u32 = 32;
const INHALE_TICKS: u32 = 10;
const SHOOT_TICKS: u32 = 8;
/// Ticks spent compressed before a jump.
const CROUCH_TICKS: u32 = 10;
/// Ticks between hops while hunting, and while idle.
const HUNT_HOP: (u32, u32) = (20, 50);
const IDLE_HOP: (u32, u32) = (100, 300);
/// Closer than this it hops away from its target, farther it leaps towards it.
const KEEP_MIN: f64 = 5.0;
const KEEP_MAX: f64 = 12.0;
/// The longest jump across and up, how far down it looks for somewhere to land, and
/// how far above the higher end a jump peaks.
const JUMP_FAR: f64 = 15.0;
const JUMP_UP: f64 = 5.0;
const JUMP_DOWN: i32 = 8;
const JUMP_CLEARANCE: f64 = 1.5;
/// How high a jump out of water or lava peaks.
const LIQUID_APEX: f64 = 1.0;
/// Ticks a jump may last at most.
const MAX_FLIGHT: u32 = 80;
/// Ticks after a jump off before floating in a liquid can end it.
const TAKE_OFF: u32 = 3;
const GRAVITY: f64 = 0.08;
const DRAG: f64 = 0.98;
/// Floating up through water and lava.
const FLOAT_UP: f64 = 0.06;
const FLOAT_MAX: f64 = 0.3;
/// Wind charges: speed in blocks per tick, aim spread, ticks before one fades, and
/// half its size.
const CHARGE_SPEED: f64 = 0.7;
const CHARGE_SPREAD: f64 = 0.03;
const CHARGE_LIFE: u32 = 100;
const CHARGE_HALF: f64 = 0.16;
const DIRECT_DAMAGE: f32 = 1.0;
/// The burst: its explosion power, how far it throws entities (twice the power), how
/// much of an explosion's knockback it has, and how far it reaches blocks.
const BURST_POWER: f32 = 3.0;
const BURST_REACH: f64 = 6.0;
const BURST_KNOCKBACK: f64 = 0.6;
const BLOCK_REACH: f64 = 3.0;
const BURST_VIEW: f64 = 64.0;
/// Ticks a button pressed by wind stays down.
const WOODEN_BUTTON_TICKS: u64 = 30;
const STONE_BUTTON_TICKS: u64 = 20;
/// Damage types of the projectiles a breeze deflects.
const DEFLECTED: &[&str] = &[
    "minecraft:arrow",
    "minecraft:trident",
    "minecraft:thrown",
    "minecraft:fireball",
    "minecraft:unattributed_fireball",
    "minecraft:mob_projectile",
    "minecraft:spit",
    "minecraft:wither_skull",
];

/// What a breeze goes after.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Foe {
    Player(i32),
    Mob(i32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Act {
    Idle,
    /// Breathing in before a shot.
    Inhaling(u32),
    /// Just shot.
    Shooting(u32),
    /// Compressed, about to jump off with `launch`.
    Crouching {
        ticks: u32,
        launch: [f64; 3],
    },
    /// In the air on a long jump, for this many ticks.
    Jumping(u32),
}

/// What the module keeps for a breeze, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq)]
struct Breeze {
    foe: Option<Foe>,
    /// Ticks the foe has been out of sight.
    unseen: u32,
    /// Ticks until it looks for a new foe.
    scan: u32,
    shoot_cooldown: u32,
    hop_cooldown: u32,
    act: Act,
    on_ground: bool,
    /// Whether its feet are in water or lava.
    in_liquid: bool,
    /// The pose viewers were last shown.
    pose: i32,
    /// Ticks until the next idle sound.
    idle_sound: u32,
}

impl Breeze {
    fn new() -> Breeze {
        Breeze {
            foe: None,
            unseen: 0,
            scan: 0,
            shoot_cooldown: 0,
            hop_cooldown: HUNT_HOP.0,
            act: Act::Idle,
            on_ground: false,
            in_liquid: false,
            pose: POSE_STANDING,
            idle_sound: 80,
        }
    }

    fn jumping(&self) -> bool {
        matches!(self.act, Act::Jumping(_))
    }
}

/// A wind charge in flight.
struct Charge {
    pos: [f64; 3],
    vel: [f64; 3],
    shooter: i32,
    age: u32,
}

/// The wind charges in flight, by entity ID.
#[derive(Default)]
struct Charges(HashMap<i32, Charge>);

fn breeze_of(mob: &Mob) -> Option<&Breeze> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Breeze>())
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn roll(rng: &mut Rng, (lo, hi): (u32, u32)) -> u32 {
    lo + rng.below(hi - lo + 1)
}

fn eyes(mob: &Mob) -> [f64; 3] {
    [mob.pos[0], mob.pos[1] + mob.size().1 * 0.85, mob.pos[2]]
}

fn player_body(p: &Player) -> [f64; 3] {
    [p.pos[0], p.pos[1] + PLAYER_SIZE.1 / 2.0, p.pos[2]]
}

fn mob_body(m: &Mob) -> [f64; 3] {
    [m.pos[0], m.pos[1] + m.size().1 / 2.0, m.pos[2]]
}

fn pose_data(id: i32, pose: i32) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_POSE)
        .varint(SERIALIZER_POSE)
        .varint(pose)
        .u8(0xFF)
}

fn charge_spawn(id: i32, c: &Charge) -> Writer {
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::BREEZE_WIND_CHARGE)
        .f64(c.pos[0])
        .f64(c.pos[1])
        .f64(c.pos[2])
        .lp_vec3(c.vel)
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(c.shooter)
}

fn charge_sync(id: i32, c: &Charge) -> Writer {
    Writer::packet(clientbound::ENTITY_POSITION_SYNC)
        .varint(id)
        .varint(0) // a linear path
        .f64(c.pos[0])
        .f64(c.pos[1])
        .f64(c.pos[2])
        .f32(0.0)
        .f32(0.0)
        .bool(false)
}

/// The vertical speed a jump needs to peak `apex` blocks up, and the ticks it then
/// takes to come down to `dy`, under the same gravity `breeze_move` uses.
fn launch_speed(apex: f64, dy: f64) -> Option<(f64, u32)> {
    let peak = |vy: f64| {
        let (mut y, mut v, mut top) = (0.0f64, vy, 0.0f64);
        while v > 0.0 {
            v = (v - GRAVITY) * DRAG;
            y += v;
            top = top.max(y);
        }
        top
    };
    let mut vy = 0.2;
    while peak(vy) < apex {
        vy += 0.02;
        if vy > 2.0 {
            return None;
        }
    }
    let (mut y, mut v) = (0.0, vy);
    for t in 1..=MAX_FLIGHT {
        v = (v - GRAVITY) * DRAG;
        y += v;
        if v < 0.0 && y <= dy {
            return Some((vy, t));
        }
    }
    None
}

/// The velocity that carries a breeze `dx`, `dy`, `dz` away in one jump, if it can.
/// Out of a liquid the jump peaks only about a block up.
fn launch(dx: f64, dy: f64, dz: f64, from_liquid: bool) -> Option<[f64; 3]> {
    if dy > JUMP_UP || dx.hypot(dz) > JUMP_FAR {
        return None;
    }
    let apex = if from_liquid {
        if dy >= LIQUID_APEX {
            return None;
        }
        LIQUID_APEX
    } else {
        dy.max(0.0) + JUMP_CLEARANCE
    };
    let (vy, ticks) = launch_speed(apex, dy)?;
    let t = f64::from(ticks);
    Some([dx / t, vy, dz / t])
}

/// How a block reacts to a wind burst.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gust {
    Bell,
    Button,
    Lever,
    Open,
    Candle,
}

fn gust_reaction(state: u16) -> Option<Gust> {
    let name = blocks::name(state);
    if name == "minecraft:bell" {
        Some(Gust::Bell)
    } else if block_has_tag(state, "minecraft:buttons") {
        (prop(state, "powered") == Some("false")).then_some(Gust::Button)
    } else if name == "minecraft:lever" {
        Some(Gust::Lever)
    } else if (blocks::is_door(state) && name != "minecraft:iron_door")
        || (block_has_tag(state, "minecraft:trapdoors") && name != "minecraft:iron_trapdoor")
        || block_has_tag(state, "minecraft:fence_gates")
    {
        // One held by redstone keeps the state its power gives it.
        (prop(state, "powered") != Some("true")).then_some(Gust::Open)
    } else if (block_has_tag(state, "minecraft:candles")
        || block_has_tag(state, "minecraft:candle_cakes"))
        && prop(state, "lit") == Some("true")
    {
        Some(Gust::Candle)
    } else {
        None
    }
}

/// The wood (or metal) family a door, trapdoor, gate or button sounds like.
fn sound_family(name: &str) -> &'static str {
    if name.contains("bamboo") {
        "bamboo_wood"
    } else if name.contains("cherry") {
        "cherry_wood"
    } else if name.contains("crimson") || name.contains("warped") {
        "nether_wood"
    } else if name.contains("copper") {
        "copper"
    } else {
        "wooden"
    }
}

fn open_sound(state: u16, opening: bool) -> String {
    let family = sound_family(blocks::name(state));
    let what = if blocks::is_door(state) {
        format!("{family}_door")
    } else if block_has_tag(state, "minecraft:trapdoors") {
        format!("{family}_trapdoor")
    } else if family == "wooden" {
        "fence_gate".to_owned()
    } else {
        format!("{family}_fence_gate")
    };
    let how = if opening { "open" } else { "close" };
    format!("minecraft:block.{what}.{how}")
}

fn button_sound(name: &str) -> String {
    let family = match sound_family(name) {
        "wooden" if name.contains("stone") || name.contains("blackstone") => "stone",
        family => family,
    };
    format!("minecraft:block.{family}_button.click_on")
}

fn charges(w: &mut World) -> &mut Charges {
    let entry = w
        .kin_state
        .entry(STATE_KEY)
        .or_insert_with(|| Box::new(Charges::default()));
    if !entry.is::<Charges>() {
        *entry = Box::new(Charges::default());
    }
    entry
        .downcast_mut::<Charges>()
        .expect("just made sure it is one")
}

pub(super) fn spawned(_w: &mut World, mob: &mut Mob, _biome: &str) {
    mob.kin = Some(Box::new(Breeze::new()));
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if mob.vehicle.is_some() {
        return false;
    }
    let mut kin = mob
        .kin
        .take()
        .filter(|k| k.is::<Breeze>())
        .unwrap_or_else(|| Box::new(Breeze::new()));
    if let Some(b) = kin.downcast_mut::<Breeze>() {
        w.tick_breeze(id, mob, b);
    }
    mob.kin = Some(kin);
    true
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

pub(super) fn hurt(w: &mut World, _id: i32, mob: &mut Mob, attacker: Option<i32>) {
    // It turns on a survival player who hurts it.
    let Some(pid) = attacker else {
        return;
    };
    if !w.players.get(&pid).is_some_and(|p| p.alive && p.survival()) {
        return;
    }
    if breeze_of(mob).is_none() {
        mob.kin = Some(Box::new(Breeze::new()));
    }
    if let Some(b) = mob.kin.as_mut().and_then(|k| k.downcast_mut::<Breeze>()) {
        b.foe = Some(Foe::Player(pid));
        b.unseen = 0;
    }
    mob.hunting = Some(pid);
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    match breeze_of(mob) {
        Some(b) if b.pose != POSE_STANDING => vec![pose_data(id, b.pose)],
        _ => Vec::new(),
    }
}

pub(super) fn write_nbt(_mob: &Mob, _nbt: &mut Compound) {}

pub(super) fn read_nbt(_mob: &mut Mob, _nbt: &Compound) {}

pub(super) fn damage_taken(
    w: &mut World,
    _id: i32,
    mob: &mut Mob,
    damage: f32,
    damage_type: &str,
    _attacker: Option<i32>,
) -> f32 {
    if damage_type == "minecraft:fall" {
        return 0.0;
    }
    if DEFLECTED.contains(&damage_type) {
        w.play_sound_at("minecraft:entity.breeze.deflect", mob.pos, 1.0, 1.0, None);
        return 0.0;
    }
    damage
}

pub(super) fn tick_world(w: &mut World) {
    let flying = match w
        .kin_state
        .get_mut(STATE_KEY)
        .and_then(|s| s.downcast_mut::<Charges>())
    {
        Some(c) if !c.0.is_empty() => std::mem::take(&mut c.0),
        _ => return,
    };
    let kept = w.tick_wind_charges(flying);
    charges(w).0.extend(kept);
}

pub(super) fn block_placed(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

pub(super) fn block_broken(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

pub(super) fn block_tick(_w: &mut World, _pos: BlockPos, _state: u16) -> bool {
    false
}

pub(super) fn ticks_randomly(_state: u16) -> bool {
    false
}

impl World {
    /// A breeze's turn: choosing a target, shooting, hopping and moving.
    fn tick_breeze(&mut self, id: i32, mob: &mut Mob, b: &mut Breeze) {
        mob.target = None;
        mob.path.clear();
        b.shoot_cooldown = b.shoot_cooldown.saturating_sub(1);
        b.hop_cooldown = b.hop_cooldown.saturating_sub(1);
        let eye = eyes(mob);
        let seen = self.breeze_foe(mob, b, eye);
        if let Some((body, _)) = seen {
            mob.yaw = yaw_towards(mob.pos, body);
            mob.head_yaw = mob.yaw;
        }
        mob.hunting = match b.foe {
            Some(Foe::Player(pid)) => Some(pid),
            _ => None,
        };
        match b.act {
            Act::Idle if b.on_ground || b.in_liquid => {
                let in_reach =
                    seen.is_some_and(|(body, visible)| visible && distance(eye, body) <= RANGE);
                if in_reach && b.shoot_cooldown == 0 {
                    b.shoot_cooldown = SHOOT_COOLDOWN;
                    b.act = Act::Inhaling(INHALE_TICKS);
                    self.breeze_pose(id, mob, b, POSE_INHALING);
                    let pitch = 1.0 + self.rng.below(2) as f32 * 0.1;
                    self.play_sound_at("minecraft:entity.breeze.inhale", mob.pos, 1.0, pitch, None);
                } else if b.hop_cooldown == 0 {
                    match self.breeze_hop(mob, seen, b.in_liquid) {
                        Some(launch) => {
                            b.act = Act::Crouching {
                                ticks: CROUCH_TICKS,
                                launch,
                            };
                            self.breeze_pose(id, mob, b, POSE_LONG_JUMPING);
                            self.play_sound_at(
                                "minecraft:entity.breeze.charge",
                                mob.pos,
                                1.0,
                                1.0,
                                None,
                            );
                        }
                        None => b.hop_cooldown = HUNT_HOP.0,
                    }
                }
            }
            Act::Idle => {}
            Act::Jumping(t) => b.act = Act::Jumping(t + 1),
            Act::Inhaling(ticks) if ticks > 1 => b.act = Act::Inhaling(ticks - 1),
            Act::Inhaling(_) => {
                if let Some((body, true)) = seen {
                    self.breeze_shoot(id, mob, body);
                }
                b.act = Act::Shooting(SHOOT_TICKS);
                self.breeze_pose(id, mob, b, POSE_SHOOTING);
            }
            Act::Shooting(ticks) if ticks > 1 => b.act = Act::Shooting(ticks - 1),
            Act::Shooting(_) => {
                b.act = Act::Idle;
                self.breeze_pose(id, mob, b, POSE_STANDING);
            }
            Act::Crouching { ticks, launch } if ticks > 1 => {
                b.act = Act::Crouching {
                    ticks: ticks - 1,
                    launch,
                };
            }
            Act::Crouching { launch, .. } => {
                mob.vel = launch;
                b.on_ground = false;
                b.act = Act::Jumping(0);
                self.play_sound_at("minecraft:entity.breeze.jump", mob.pos, 1.0, 1.0, None);
            }
        }
        self.breeze_move(mob, b);
        // A jump ends on landing, on falling into water or lava, or when it has gone on
        // far too long.
        if let Act::Jumping(t) = b.act {
            if b.on_ground || (b.in_liquid && t >= TAKE_OFF) || t >= MAX_FLIGHT {
                b.act = Act::Idle;
                mob.vel[0] = 0.0;
                mob.vel[2] = 0.0;
                b.hop_cooldown = if b.foe.is_some() {
                    roll(&mut self.rng, HUNT_HOP)
                } else {
                    roll(&mut self.rng, IDLE_HOP)
                };
                self.breeze_pose(id, mob, b, POSE_STANDING);
                if b.on_ground {
                    self.play_sound_at("minecraft:entity.breeze.land", mob.pos, 1.0, 1.0, None);
                }
            }
        }
        b.idle_sound = b.idle_sound.saturating_sub(1);
        if b.idle_sound == 0 {
            b.idle_sound = 80 + self.rng.below(240);
            let sound = if self.rng.below(4) == 0 {
                "minecraft:entity.breeze.whirl"
            } else if b.on_ground {
                "minecraft:entity.breeze.idle_ground"
            } else {
                "minecraft:entity.breeze.idle_air"
            };
            let pitch = 0.8 + self.rng.f64() as f32 * 0.4;
            self.play_sound_at(sound, mob.pos, 1.0, pitch, None);
        }
        if mob.pos[1] < f64::from(MIN_Y - 64) {
            mob.health = 0.0;
        }
    }

    fn breeze_pose(&mut self, id: i32, mob: &Mob, b: &mut Breeze, pose: i32) {
        if b.pose != pose {
            b.pose = pose;
            self.send_to_viewers(mob, pose_data(id, pose));
        }
    }

    /// Where a foe's body is, if it is still something to go after.
    fn breeze_foe_body(&self, foe: Foe) -> Option<[f64; 3]> {
        match foe {
            Foe::Player(pid) => self
                .players
                .get(&pid)
                .filter(|p| p.alive && p.survival())
                .map(player_body),
            Foe::Mob(mid) => self.mobs.get(&mid).filter(|m| m.alive()).map(mob_body),
        }
    }

    /// Keeps the breeze's foe or finds a new one (once a second): the nearest survival
    /// player in sight within range, or else the nearest iron golem. Returns where the
    /// foe's body is and whether the breeze sees it.
    fn breeze_foe(&mut self, mob: &Mob, b: &mut Breeze, eye: [f64; 3]) -> Option<([f64; 3], bool)> {
        if let Some(foe) = b.foe {
            let forget = mob.kind.follow_range().max(RANGE);
            if let Some(body) = self
                .breeze_foe_body(foe)
                .filter(|&body| distance(body, mob.pos) <= forget)
            {
                let visible = self.line_of_sight(eye, body);
                b.unseen = if visible { 0 } else { b.unseen + 1 };
                if b.unseen < FORGET_AFTER {
                    return Some((body, visible));
                }
            }
            b.foe = None;
            b.unseen = 0;
        }
        if b.scan > 0 {
            b.scan -= 1;
            return None;
        }
        b.scan = SCAN_EVERY;
        let pick = |best: Option<(Foe, [f64; 3], f64)>, (foe, body): (Foe, [f64; 3])| {
            let d = distance(eye, body);
            match best {
                Some((_, _, bd)) if bd <= d => best,
                _ if d > RANGE || !self.line_of_sight(eye, body) => best,
                _ => Some((foe, body, d)),
            }
        };
        let found = self
            .players
            .iter()
            .filter(|(_, p)| p.alive && p.survival())
            .map(|(&pid, p)| (Foe::Player(pid), player_body(p)))
            .fold(None, pick)
            .or_else(|| {
                self.mobs
                    .iter()
                    .filter(|(_, m)| m.kind == MobKind::IronGolem && m.alive())
                    .map(|(&mid, m)| (Foe::Mob(mid), mob_body(m)))
                    .fold(None, pick)
            });
        let (foe, body, _) = found?;
        b.foe = Some(foe);
        b.unseen = 0;
        Some((body, true))
    }

    /// Picks where to hop to and returns the launch velocity: away from a foe that is
    /// close, towards one that is far or out of sight, to the side otherwise, and
    /// anywhere near without a foe.
    fn breeze_hop(
        &mut self,
        mob: &Mob,
        seen: Option<([f64; 3], bool)>,
        in_liquid: bool,
    ) -> Option<[f64; 3]> {
        let jitter = |rng: &mut Rng, by: f64| (rng.f64() * 2.0 - 1.0) * by;
        for _ in 0..8 {
            let (angle, length) = match seen {
                Some((body, visible)) => {
                    let (dx, dz) = (body[0] - mob.pos[0], body[2] - mob.pos[2]);
                    let toward = dz.atan2(dx);
                    let d = dx.hypot(dz);
                    if d < KEEP_MIN {
                        (
                            toward + PI + jitter(&mut self.rng, 1.0),
                            4.0 + self.rng.f64() * 4.0,
                        )
                    } else if d > KEEP_MAX || !visible {
                        (
                            toward + jitter(&mut self.rng, 0.5),
                            (d - 8.0).clamp(3.0, JUMP_FAR),
                        )
                    } else {
                        let side = if self.rng.below(2) == 0 { PI } else { -PI } / 2.0;
                        (
                            toward + side + jitter(&mut self.rng, 0.6),
                            3.0 + self.rng.f64() * 4.0,
                        )
                    }
                }
                None => (self.rng.f64() * 2.0 * PI, 2.0 + self.rng.f64() * 4.0),
            };
            let x = (mob.pos[0] + angle.cos() * length).floor() as i32;
            let z = (mob.pos[2] + angle.sin() * length).floor() as i32;
            let Some(y) = self.breeze_landing(mob.kind, x, mob.pos[1].floor() as i32, z) else {
                continue;
            };
            let spot = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
            if let Some(v) = launch(
                spot[0] - mob.pos[0],
                spot[1] - mob.pos[1],
                spot[2] - mob.pos[2],
                in_liquid,
            ) {
                return Some(v);
            }
        }
        None
    }

    /// The height nearest `base` in column `x`, `z` where a breeze can stand.
    fn breeze_landing(&self, kind: MobKind, x: i32, base: i32, z: i32) -> Option<i32> {
        let up = JUMP_UP as i32;
        let offsets = std::iter::once(0).chain((1..=up.max(JUMP_DOWN)).flat_map(|k| [k, -k]));
        for dy in offsets {
            if dy > up || dy < -JUMP_DOWN {
                continue;
            }
            let y = base + dy;
            let below = self.loaded_block(BlockPos::new(x, y - 1, z));
            let floor = below.is_some_and(|s| {
                !levitas_registry::collision_boxes(s).is_empty() && fluid_at(s).is_none()
            });
            if floor && self.mob_fits(kind, [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5])
            {
                return Some(y);
            }
        }
        None
    }

    /// Shoots a wind charge at `body`.
    fn breeze_shoot(&mut self, id: i32, mob: &Mob, body: [f64; 3]) {
        let from = [mob.pos[0], mob.pos[1] + mob.size().1 * 0.6, mob.pos[2]];
        let d = [0, 1, 2].map(|i| body[i] - from[i]);
        let length = distance(body, from).max(1e-6);
        let mut dir = d.map(|v| v / length + (self.rng.f64() * 2.0 - 1.0) * CHARGE_SPREAD);
        let norm = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2])
            .sqrt()
            .max(1e-6);
        dir = dir.map(|v| v / norm);
        let charge = Charge {
            pos: from,
            vel: dir.map(|v| v * CHARGE_SPEED),
            shooter: id,
            age: 0,
        };
        let cid = next_entity_id();
        self.broadcast(charge_spawn(cid, &charge), None);
        charges(self).0.insert(cid, charge);
        let pitch = 1.0 + self.rng.below(3) as f32 * 0.1;
        self.play_sound_at("minecraft:entity.breeze.shoot", mob.pos, 1.5, pitch, None);
    }

    /// Moves a breeze one tick: gravity (or floating up in a liquid), knockback and
    /// collisions. It never builds up a fall.
    fn breeze_move(&mut self, mob: &mut Mob, b: &mut Breeze) {
        let feet = BlockPos::new(
            mob.pos[0].floor() as i32,
            mob.pos[1].floor() as i32,
            mob.pos[2].floor() as i32,
        );
        b.in_liquid = self
            .loaded_block(feet)
            .and_then(fluid_at)
            .is_some_and(|f| matches!(f.fluid, Fluid::Water | Fluid::Lava));
        // Just jumping off, the liquid does not hold it back.
        let taking_off = matches!(b.act, Act::Jumping(t) if t < TAKE_OFF);
        if b.in_liquid && !taking_off {
            mob.vel[1] = (mob.vel[1] * 0.8 + FLOAT_UP).min(FLOAT_MAX);
            mob.vel[0] *= 0.8;
            mob.vel[2] *= 0.8;
        } else {
            mob.vel[1] = (mob.vel[1] - GRAVITY) * DRAG;
        }
        let (width, height) = mob.size();
        let half = width / 2.0;
        let mut pos = mob.pos;
        let dy = self.clip_move(pos, half, height, 1, mob.vel[1]);
        pos[1] += dy;
        let stopped = (dy - mob.vel[1]).abs() > 1e-9;
        b.on_ground = stopped && mob.vel[1] < 0.0;
        mob.set_on_ground(b.on_ground);
        if stopped {
            mob.vel[1] = 0.0;
        }
        for (axis, push) in [(0, 0), (2, 1)] {
            let wanted = mob.vel[axis] + mob.knockback[push];
            let d = self.clip_move(pos, half, height, axis, wanted);
            pos[axis] += d;
            if (d - wanted).abs() > 1e-9 {
                mob.vel[axis] = 0.0;
                mob.knockback[push] = 0.0;
            }
        }
        let friction = if b.on_ground { 0.546 } else { 0.91 };
        for k in &mut mob.knockback {
            *k *= friction;
            if k.abs() < 0.003 {
                *k = 0.0;
            }
        }
        if b.on_ground && !b.jumping() {
            mob.vel[0] *= 0.5;
            mob.vel[2] *= 0.5;
        }
        mob.pos = pos;
    }

    /// Moves the wind charges in flight; returns those still flying.
    fn tick_wind_charges(&mut self, flying: HashMap<i32, Charge>) -> HashMap<i32, Charge> {
        let mut kept = HashMap::new();
        for (cid, mut c) in flying {
            c.age += 1;
            if c.pos[1] > f64::from(MAX_Y + 30) {
                // Far above the world it bursts in the open.
                self.wind_burst(cid, &c, c.pos, None);
                continue;
            }
            if c.age > CHARGE_LIFE {
                self.broadcast(items::remove_entity(cid), None);
                continue;
            }
            let speed = distance(c.vel, [0.0; 3]);
            let steps = (speed / 0.25).ceil().max(1.0) as usize;
            let mut at = c.pos;
            let mut hit = None;
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let p = [0, 1, 2].map(|k| c.pos[k] + c.vel[k] * t);
                if self.point_in_block(p) {
                    hit = Some(None);
                    break;
                }
                if let Some(foe) = self.charge_touches(p, c.shooter) {
                    at = p;
                    hit = Some(Some(foe));
                    break;
                }
                at = p;
            }
            match hit {
                Some(target) => self.wind_burst(cid, &c, at, target),
                None => {
                    c.pos = at;
                    self.broadcast(charge_sync(cid, &c), None);
                    kept.insert(cid, c);
                }
            }
        }
        kept
    }

    /// The player or mob (not the shooter) a wind charge at `p` touches.
    fn charge_touches(&self, p: [f64; 3], shooter: i32) -> Option<Foe> {
        let inside = |pos: [f64; 3], (width, height): (f64, f64)| {
            let reach = width / 2.0 + CHARGE_HALF;
            (p[0] - pos[0]).abs() < reach
                && (p[2] - pos[2]).abs() < reach
                && p[1] > pos[1] - CHARGE_HALF
                && p[1] < pos[1] + height + CHARGE_HALF
        };
        let player = self
            .players
            .iter()
            .find(|(_, pl)| {
                pl.alive && pl.mode != GAME_MODE_SPECTATOR && inside(pl.pos, PLAYER_SIZE)
            })
            .map(|(&pid, _)| Foe::Player(pid));
        player.or_else(|| {
            self.mobs
                .iter()
                .find(|(&mid, m)| mid != shooter && m.alive() && inside(m.pos, m.size()))
                .map(|(&mid, _)| Foe::Mob(mid))
        })
    }

    /// A wind charge bursts at `at`, having hit `target` directly if anything.
    fn wind_burst(&mut self, cid: i32, c: &Charge, at: [f64; 3], target: Option<Foe>) {
        self.broadcast(items::remove_entity(cid), None);
        let horizontal = c.vel[0].hypot(c.vel[2]).max(1e-9);
        let away = [c.vel[0] / horizontal, c.vel[2] / horizontal];
        match target {
            Some(Foe::Player(pid)) => {
                self.damage_from(pid, DIRECT_DAMAGE, Damage::Mob("Breeze"), Some(c.shooter));
            }
            Some(Foe::Mob(mid)) => {
                self.hit_mob(mid, DIRECT_DAMAGE, None, away, "minecraft:wind_charge");
            }
            None => {}
        }
        // Everything near is thrown away from the burst, by how close its feet are,
        // in the direction of its eyes.
        let push_from = |feet: [f64; 3], eye: [f64; 3]| -> Option<[f64; 3]> {
            let length = distance(feet, at);
            if length > BURST_REACH {
                return None;
            }
            let strength = (1.0 - length / BURST_REACH) * BURST_KNOCKBACK;
            let d = [0, 1, 2].map(|i| eye[i] - at[i]);
            let towards = distance(eye, at);
            Some(if towards < 1e-6 {
                [0.0, strength, 0.0]
            } else {
                d.map(|v| v / towards * strength)
            })
        };
        let mut pushed: Vec<(i32, [f64; 3])> = Vec::new();
        for (&pid, p) in &self.players {
            if p.alive && p.mode != GAME_MODE_SPECTATOR {
                let eye = [p.pos[0], p.pos[1] + PLAYER_EYES, p.pos[2]];
                if let Some(v) = push_from(p.pos, eye) {
                    pushed.push((pid, v));
                }
            }
        }
        for (&mid, m) in &mut self.mobs {
            if mid == c.shooter
                || !m.alive()
                || matches!(
                    m.kind,
                    MobKind::IronGolem | MobKind::Warden | MobKind::Shulker
                )
            {
                continue;
            }
            if let Some(v) = push_from(m.pos, eyes(m)) {
                m.push(v);
            }
        }
        for t in self.tnt.values_mut() {
            let pos = t.pos();
            if let Some(v) = push_from(pos, pos) {
                t.push(v);
            }
        }
        // Players near see and hear it; those caught in it are thrown.
        let particle = levitas_registry::particle::GUST_EMITTER_LARGE;
        let sound = sound_id("minecraft:entity.breeze.wind_burst").map_or(0, |s| s + 1);
        let viewers: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| distance(p.pos, at) < BURST_VIEW)
            .map(|(&pid, _)| pid)
            .collect();
        for pid in viewers {
            let mut packet = Writer::packet(clientbound::EXPLODE)
                .f64(at[0])
                .f64(at[1])
                .f64(at[2])
                .f32(BURST_POWER)
                .i32(0);
            packet = match pushed.iter().find(|(p, _)| *p == pid) {
                Some((_, v)) => packet.bool(true).f64(v[0]).f64(v[1]).f64(v[2]),
                None => packet.bool(false),
            };
            self.send(
                pid,
                packet.varint(particle).varint(sound).varint(0).bool(true),
            );
        }
        self.wind_blocks(at);
    }

    /// The burst works the blocks near it: doors, trapdoors and gates swing, buttons
    /// are pressed, levers flipped and candles put out (with `mobGriefing`), and bells
    /// ring.
    fn wind_blocks(&mut self, at: [f64; 3]) {
        let griefing = crate::rules::bool("mob_griefing");
        let r = BLOCK_REACH.ceil() as i32;
        let centre = at.map(|v| v.floor() as i32);
        let mut doors: Vec<BlockPos> = Vec::new();
        for dx in -r..=r {
            for dy in -r..=r {
                for dz in -r..=r {
                    let pos = BlockPos::new(centre[0] + dx, centre[1] + dy, centre[2] + dz);
                    let middle = [
                        f64::from(pos.x) + 0.5,
                        f64::from(pos.y) + 0.5,
                        f64::from(pos.z) + 0.5,
                    ];
                    let length = distance(middle, at);
                    if length > BLOCK_REACH {
                        continue;
                    }
                    let Some(state) = self.loaded_block(pos) else {
                        continue;
                    };
                    let Some(gust) = gust_reaction(state) else {
                        continue;
                    };
                    if gust != Gust::Bell && !griefing {
                        continue;
                    }
                    // The wind must reach it: nothing solid in between.
                    if length > 1.0 {
                        let near =
                            [0, 1, 2].map(|i| middle[i] - (middle[i] - at[i]) / length * 0.6);
                        if !self.line_of_sight(at, near) {
                            continue;
                        }
                    }
                    self.wind_works(pos, state, gust, at, &mut doors);
                }
            }
        }
    }

    fn wind_works(
        &mut self,
        pos: BlockPos,
        state: u16,
        gust: Gust,
        at: [f64; 3],
        doors: &mut Vec<BlockPos>,
    ) {
        let middle = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        match gust {
            Gust::Bell => {
                self.ring_bell(pos, Dir::from_yaw(yaw_towards(middle, at)));
            }
            Gust::Button => {
                let delay = if block_has_tag(state, "minecraft:wooden_buttons") {
                    WOODEN_BUTTON_TICKS
                } else {
                    STONE_BUTTON_TICKS
                };
                self.set_block_updating(pos, with_bool(state, "powered", true));
                self.schedule(pos, state, delay);
                self.play_sound(&button_sound(blocks::name(state)), pos, None);
            }
            Gust::Lever => {
                let on = prop(state, "powered") != Some("true");
                self.set_block_updating(pos, with_bool(state, "powered", on));
                let pitch = if on { 0.6 } else { 0.5 };
                self.play_sound_at("minecraft:block.lever.click", middle, 1.0, pitch, None);
            }
            Gust::Candle => {
                self.set_block_updating(pos, with_bool(state, "lit", false));
                self.play_sound("minecraft:block.candle.extinguish", pos, None);
            }
            Gust::Open => {
                // A door keeps its open state on the lower half; the upper one follows.
                let pos = if blocks::is_door(state) && prop(state, "half") == Some("upper") {
                    offset(pos, Dir::Down)
                } else {
                    pos
                };
                if doors.contains(&pos) {
                    return;
                }
                doors.push(pos);
                let state = self.block(pos);
                if gust_reaction(state) != Some(Gust::Open) {
                    return;
                }
                let opening = prop(state, "open") != Some("true");
                self.set_block_updating(pos, with_bool(state, "open", opening));
                self.play_sound(&open_sound(state, opening), pos, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::with;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-breezes-{}-{}",
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
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 21,
            properties: Vec::new(),
        };
        // Far away, so that only what a test puts close is noticed.
        w.players
            .insert(alice, Player::new(profile, out, [-40.5, -60.0, -40.5]));
        (w, alice)
    }

    fn breeze(w: &mut World, at: [f64; 3]) -> (i32, Mob) {
        let mut mob = Mob::new(MobKind::Breeze, at, &mut w.rng);
        spawned(w, &mut mob, "minecraft:plains");
        (next_entity_id(), mob)
    }

    fn state_of(mob: &Mob) -> &Breeze {
        breeze_of(mob).expect("breeze state")
    }

    fn block_state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    fn flying(w: &World) -> usize {
        w.kin_state
            .get(STATE_KEY)
            .and_then(|s| s.downcast_ref::<Charges>())
            .map_or(0, |c| c.0.len())
    }

    #[test]
    fn breezes_have_thirty_health() {
        let (mut w, _) = world_with_player();
        let (_, mob) = breeze(&mut w, [0.5, -60.0, 0.5]);
        assert_eq!(mob.health, 30.0);
    }

    #[test]
    fn a_breeze_shoots_wind_charges_at_a_survival_player() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = breeze(&mut w, [-30.5, -60.0, -40.5]);
        let full = w.players[&alice].health;
        let mut shot = false;
        for _ in 0..200 {
            assert!(tick(&mut w, id, &mut mob));
            shot |= flying(&w) > 0;
            tick_world(&mut w);
            if w.players[&alice].health < full {
                break;
            }
        }
        assert!(shot, "shot a wind charge");
        assert_eq!(state_of(&mob).foe, Some(Foe::Player(alice)));
        assert!(w.players[&alice].health < full, "the charge hit her");
        assert_eq!(mob.health, mob.max_health(), "never hurt by its own jumps");
    }

    #[test]
    fn creative_and_distant_players_are_left_alone() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = breeze(&mut w, [-20.5, -60.0, -40.5]);
        for _ in 0..40 {
            tick(&mut w, id, &mut mob);
        }
        assert_eq!(state_of(&mob).foe, None, "20 blocks is out of range");
        w.players.get_mut(&alice).unwrap().mode = GAME_MODE_CREATIVE;
        let (id, mut mob) = breeze(&mut w, [-32.5, -60.0, -40.5]);
        for _ in 0..40 {
            tick(&mut w, id, &mut mob);
        }
        assert_eq!(state_of(&mob).foe, None);
        assert_eq!(flying(&w), 0);
    }

    #[test]
    fn being_hurt_by_a_player_makes_them_the_target() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = breeze(&mut w, [0.5, -60.0, 0.5]);
        hurt(&mut w, id, &mut mob, Some(alice));
        assert_eq!(state_of(&mob).foe, Some(Foe::Player(alice)));
        assert_eq!(mob.hunting, Some(alice));
        // Alice is about 58 blocks off, beyond its follow range: it lets her go.
        tick(&mut w, id, &mut mob);
        assert_eq!(state_of(&mob).foe, None);
    }

    #[test]
    fn projectiles_are_deflected_and_falls_do_nothing() {
        let (mut w, _) = world_with_player();
        let (id, mut mob) = breeze(&mut w, [0.5, -60.0, 0.5]);
        for kind in ["minecraft:arrow", "minecraft:trident", "minecraft:thrown"] {
            assert_eq!(damage_taken(&mut w, id, &mut mob, 6.0, kind, None), 0.0);
        }
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 4.0, "minecraft:fall", None),
            0.0
        );
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 5.0, "minecraft:player_attack", None),
            5.0
        );
        w.mobs.insert(id, mob);
        assert!(!w.hit_mob(id, 6.0, None, [1.0, 0.0], "minecraft:arrow"));
        assert_eq!(w.mobs[&id].health, 30.0);
        assert!(w.hit_mob(id, 6.0, None, [1.0, 0.0], "minecraft:mob_attack"));
        assert_eq!(w.mobs[&id].health, 24.0);
    }

    #[test]
    fn a_breeze_falls_without_harm_and_lands() {
        let (mut w, _) = world_with_player();
        let (id, mut mob) = breeze(&mut w, [0.5, -40.0, 0.5]);
        for _ in 0..100 {
            tick(&mut w, id, &mut mob);
        }
        assert!(state_of(&mob).on_ground);
        assert!((mob.pos[1] + 60.0).abs() < 1e-6, "{}", mob.pos[1]);
        assert_eq!(mob.health, 30.0);
    }

    #[test]
    fn jumps_land_where_they_were_aimed() {
        for (dx, dy, dz, liquid) in [
            (10.0, 0.0, 3.0, false),
            (-4.0, 4.0, 0.0, false),
            (2.0, -6.0, -12.0, false),
            (3.0, 0.5, 0.0, true),
        ] {
            let v = launch(dx, dy, dz, liquid).expect("a jump this size is possible");
            let (mut p, mut vy, mut top) = ([0.0f64; 3], v[1], 0.0f64);
            for _ in 0..MAX_FLIGHT {
                vy = (vy - GRAVITY) * DRAG;
                p = [p[0] + v[0], p[1] + vy, p[2] + v[2]];
                top = top.max(p[1]);
                if vy < 0.0 && p[1] <= dy {
                    break;
                }
            }
            assert!((p[0] - dx).abs() < 1e-6 && (p[2] - dz).abs() < 1e-6);
            assert!((p[1] - dy).abs() < 1.0, "{} vs {dy}", p[1]);
            if liquid {
                assert!(
                    top < LIQUID_APEX + 0.2,
                    "out of a liquid it peaks low: {top}"
                );
            }
        }
        assert!(launch(16.0, 0.0, 0.0, false).is_none(), "too far");
        assert!(launch(1.0, 6.0, 0.0, false).is_none(), "too high");
        assert!(
            launch(1.0, 2.0, 0.0, true).is_none(),
            "too high from a liquid"
        );
    }

    #[test]
    fn a_breeze_leaps_towards_a_far_target() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = breeze(&mut w, [-40.5, -60.0, -26.5]);
        hurt(&mut w, id, &mut mob, Some(alice));
        let start = mob.pos;
        let mut jumped = false;
        for _ in 0..200 {
            tick(&mut w, id, &mut mob);
            jumped |= state_of(&mob).jumping();
        }
        assert!(jumped, "it jumped");
        let before = (start[2] - w.players[&alice].pos[2]).abs();
        let after = (mob.pos[2] - w.players[&alice].pos[2]).abs();
        assert!(after < before - 1.0, "{before} -> {after}");
        assert_eq!(mob.health, 30.0);
    }

    #[test]
    fn a_breeze_in_water_keeps_acting() {
        let (mut w, alice) = world_with_player();
        let water = block_state("minecraft:water");
        for x in -3..=3 {
            for z in -3..=3 {
                for y in -61..=-58 {
                    w.set_block(BlockPos::new(x, y, z), water);
                }
            }
        }
        let (id, mut mob) = breeze(&mut w, [0.5, -60.5, 0.5]);
        // Stuck mid-jump in the water, as after falling in.
        if let Some(b) = mob.kin.as_mut().and_then(|k| k.downcast_mut::<Breeze>()) {
            b.act = Act::Jumping(10);
        }
        hurt(&mut w, id, &mut mob, Some(alice));
        w.players.get_mut(&alice).unwrap().pos = [10.5, -60.0, 0.5];
        let mut shot = false;
        for _ in 0..100 {
            tick(&mut w, id, &mut mob);
            shot |= flying(&w) > 0;
        }
        assert!(shot, "it still shoots from the water");
    }

    #[test]
    fn a_burst_throws_mobs_and_works_blocks() {
        let (mut w, _) = world_with_player();
        let y = -60;
        let lever = with(block_state("minecraft:lever"), "face", "floor");
        w.set_block(BlockPos::new(1, y, 0), lever);
        let door = BlockPos::new(-1, y, 0);
        let lower = with(block_state("minecraft:oak_door"), "half", "lower");
        w.set_block(door, lower);
        w.set_block(offset(door, Dir::Up), with(lower, "half", "upper"));
        let iron = BlockPos::new(0, y, 1);
        let iron_lower = with(block_state("minecraft:iron_door"), "half", "lower");
        w.set_block(iron, iron_lower);
        w.set_block(offset(iron, Dir::Up), with(iron_lower, "half", "upper"));
        let held = BlockPos::new(0, y, -1);
        let powered = with_bool(
            with(block_state("minecraft:spruce_door"), "half", "lower"),
            "powered",
            true,
        );
        w.set_block(held, powered);
        w.set_block(offset(held, Dir::Up), with(powered, "half", "upper"));
        let cow = next_entity_id();
        w.mobs
            .insert(cow, Mob::new(MobKind::Cow, [3.5, -60.0, 0.5], &mut w.rng));
        let charge = Charge {
            pos: [0.5, -59.5, 0.5],
            vel: [0.7, 0.0, 0.0],
            shooter: -1,
            age: 0,
        };
        w.wind_burst(next_entity_id(), &charge, [0.5, -59.5, 0.5], None);
        assert!(
            w.mobs[&cow].knockback[0] > 0.05,
            "thrown away from the burst"
        );
        assert_eq!(w.mobs[&cow].health, w.mobs[&cow].max_health(), "no damage");
        assert_eq!(
            prop(w.block(BlockPos::new(1, y, 0)), "powered"),
            Some("true")
        );
        assert_eq!(prop(w.block(door), "open"), Some("true"));
        assert_eq!(
            prop(w.block(iron), "open"),
            Some("false"),
            "iron stays shut"
        );
        assert_eq!(
            prop(w.block(held), "open"),
            Some("false"),
            "a powered door keeps its state"
        );
        assert!(
            !is_air(w.block(BlockPos::new(0, y - 1, 0))),
            "nothing broken"
        );
    }

    #[test]
    fn door_and_button_sounds_exist() {
        for name in [
            "minecraft:cherry_door",
            "minecraft:bamboo_trapdoor",
            "minecraft:crimson_fence_gate",
            "minecraft:oak_fence_gate",
            "minecraft:copper_door",
        ] {
            let state = block_state(name);
            assert!(sound_id(&open_sound(state, true)).is_some(), "{name}");
        }
        for name in [
            "minecraft:warped_button",
            "minecraft:stone_button",
            "minecraft:oak_button",
        ] {
            assert!(sound_id(&button_sound(name)).is_some(), "{name}");
        }
    }

    #[test]
    fn a_charge_hits_a_mob_for_one_damage() {
        let (mut w, _) = world_with_player();
        let cow = next_entity_id();
        w.mobs
            .insert(cow, Mob::new(MobKind::Cow, [6.5, -60.0, 0.5], &mut w.rng));
        let full = w.mobs[&cow].health;
        charges(&mut w).0.insert(
            next_entity_id(),
            Charge {
                pos: [0.5, -59.0, 0.5],
                vel: [CHARGE_SPEED, 0.0, 0.0],
                shooter: -1,
                age: 0,
            },
        );
        for _ in 0..20 {
            tick_world(&mut w);
        }
        assert_eq!(flying(&w), 0, "it burst");
        assert_eq!(w.mobs[&cow].health, full - DIRECT_DAMAGE);
    }
}
