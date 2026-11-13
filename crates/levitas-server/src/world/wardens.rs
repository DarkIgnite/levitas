//! Wardens, and the sculk shriekers that summon them.
//!
//! Behaviour follows the Minecraft Wiki (Warden, Sculk Shrieker, Sculk Sensor, Darkness):
//! - A player who moves without sneaking (or places or breaks a block) within 8 blocks
//!   of a sculk sensor sets off the sculk shriekers within 8 blocks of that sensor; a
//!   player standing on a shrieker sets it off too, sneaking or not. A shriek lasts 4.5
//!   seconds. Shriekers that can summon (`can_summon=true`, the naturally generated
//!   ones), when the difficulty is not Peaceful and `spawn_wardens` is on, also raise the
//!   warning level of the player, and of every player within 16 blocks of them, to one
//!   more than the highest among them (at most 4); such a player cannot set off another
//!   of them for 10 seconds, and their level drops by one after every 10 minutes
//!   without a warning. When the shriek ends, players in survival or adventure within
//!   40 blocks get Darkness (if the player who set it off is still within 40 blocks),
//!   the warning sound of the level plays, and at level 4 a warden emerges: up to 20
//!   tries at the highest spot of a random column in the 11x13x11 box around the
//!   shrieker that has a full top face below and room for the warden, with no liquid at
//!   its feet; never within 24 blocks of another warden. Breaking a shrieker in the
//!   middle of its shriek summons at once. A warden that finds no spot roars angrily.
//!   Shrieks are timed in memory: a shrieker saved mid-shriek (`shrieking=true` with no
//!   shriek running) counts as idle when set off, and random ticks reset it.
//! - Wardens are blind. They hear vibrations within 15 blocks, with 2 seconds between
//!   two: players moving without sneaking (not in creative or spectator), walking mobs
//!   (not other wardens), arrows landing and blocks placed or broken by players (not
//!   wool or carpet). Each adds anger towards what caused it: 35, or 10 for a projectile
//!   (35 for the second one in 5 seconds; none when its shooter is over 30 blocks
//!   away), and the warden goes to look. Touching a warden adds 35 (once a second),
//!   hitting it 100. Anger is kept per suspect between 0 and 150 and decays by 1 a
//!   second; it is dropped for players who die or turn creative or spectator.
//! - Idle, a warden heads for the closest entity it is aware of (in a 49x51x49 box,
//!   players first) and sniffs now and then (4.2 seconds, then 5 to 10 seconds before
//!   the next), which adds 35 anger to the nearest mob or player within 6 blocks across
//!   and 20 up or down. At 80 anger it roars for 4.2 seconds, adds 20 more and hunts;
//!   a player who hits it from within 5 blocks is hunted at once. Players it is angry at
//!   are hunted before mobs.
//! - Its blow deals 16, 30 or 45 by difficulty, every 1.8 seconds. When the target is
//!   out of reach, has been known for 10 seconds, nothing was attacked for 5 seconds
//!   and the target is within 14 blocks across (20 up or down), it charges a sonic boom
//!   for 1.7 seconds that strikes through blocks for 6, 10 or 15, ignoring armour and
//!   shields, then rests 1.3 seconds.
//! - Every 6 seconds it gives Darkness (13 seconds) to players in survival or
//!   adventure within 20 blocks.
//! - After 60 seconds with nothing heard or sniffed, an unnamed warden digs down and is
//!   gone (at once when it stands in a liquid). While emerging or digging it hears
//!   nothing and only `/kill` hurts it. Fire and lava never do, and nothing knocks it
//!   back.
//! - Shown with its poses (emerging, digging, sniffing, roaring) and its anger level;
//!   saved as vanilla: `anger.suspects` with `anger` and `uuid`.

use std::collections::HashMap;

use levitas_nbt::{Compound, Tag};

use super::mobs::{yaw_towards, Mob, MobKind};
use super::*;
use crate::blocks::fluid::fluid_at;
use crate::rules::Difficulty;
use crate::world::survival::Damage;

/// Module-wide state, in `World::kin_state` under this key.
const KEY: &str = "wardens";

const DATA_POSE: u8 = 6;
const DATA_ANGER: u8 = 16;
const SERIALIZER_VARINT: i32 = 1;
const SERIALIZER_POSE: i32 = 20;
const POSE_STANDING: i32 = 0;
const POSE_ROARING: i32 = 11;
const POSE_SNIFFING: i32 = 12;
const POSE_EMERGING: i32 = 13;
const POSE_DIGGING: i32 = 14;
/// Entity events: the attack swing, shaking tendrils, and the sonic boom animation.
const EVENT_ATTACK: i8 = 4;
const EVENT_TENDRILS: i8 = 61;
const EVENT_SONIC_BOOM: i8 = 62;
const GAME_MODE_ADVENTURE: u8 = 2;
const GAME_MODE_SPECTATOR: u8 = 3;

/// How long emerging and digging down take, in ticks.
const EMERGE_TICKS: u32 = 134;
const DIG_TICKS: u32 = 100;
/// Roaring and sniffing last 4.2 seconds; sniffs are 5 to 10 seconds apart.
const ROAR_TICKS: u32 = 84;
const SNIFF_TICKS: u32 = 84;
const SNIFF_COOLDOWN: (u32, u32) = (100, 200);
/// Anger: hunting starts at 80 and it never goes past 150.
const ANGRY: i32 = 80;
const MAX_ANGER: i32 = 150;
const VIBRATION_ANGER: i32 = 35;
const PROJECTILE_ANGER: i32 = 10;
const TOUCH_ANGER: i32 = 35;
const ATTACKED_ANGER: i32 = 100;
const ROAR_ANGER: i32 = 20;
const SNIFF_ANGER: i32 = 35;
/// Hearing: range, ticks between two vibrations, the span in which a second projectile
/// counts in full, and how far away a shooter may be.
const LISTEN_RANGE: f64 = 15.0;
const HEAR_COOLDOWN: u32 = 40;
const PROJECTILE_PAIR_TICKS: u64 = 100;
const PROJECTILE_MAX_DISTANCE: f64 = 30.0;
const TOUCH_COOLDOWN: u32 = 20;
/// Half the sides of the box a warden is aware of entities in (49x51x49).
const AWARE: (f64, f64) = (24.5, 25.5);
/// Sniffing reaches 6 blocks across and 20 up or down.
const SNIFF_REACH: (f64, f64) = (6.0, 20.0);
/// Ticks without hearing or sniffing anything before an unnamed warden digs down.
const CALM_TICKS: u32 = 1200;
/// A warden's Darkness: every 6 seconds, 20 blocks around, lasting 13 seconds.
const DARKNESS_EVERY: u64 = 120;
const DARKNESS_RANGE: f64 = 20.0;
const DARKNESS_TICKS: i32 = 260;
/// Melee: 1.8 seconds between blows.
const MELEE_COOLDOWN: u32 = 36;
const MELEE_DAMAGE: [f32; 3] = [16.0, 30.0, 45.0];
const MELEE_PUSH: f64 = 0.4;
/// Sonic boom: charging, the whole attack, the wait since the last attack and since the
/// target was found, range, damage by difficulty and the push it gives.
const BOOM_CHARGE: u32 = 34;
const BOOM_TOTAL: u32 = 60;
const BOOM_IDLE: u32 = 100;
const BOOM_CHASE: u32 = 200;
const BOOM_RANGE: (f64, f64) = (14.0, 20.0);
const BOOM_DAMAGE: [f32; 3] = [6.0, 10.0, 15.0];
const BOOM_PUSH: (f64, f64) = (1.5, 0.5);
/// A player hitting it from this close is hunted without a roar.
const NO_ROAR_RANGE: f64 = 5.0;
/// A player's width and height.
const PLAYER_SIZE: (f64, f64) = (0.6, 1.8);

/// Shriekers: how long a shriek lasts, a player's wait before setting off another, how
/// long before a warning level drops, the top level, who else is blamed, who gets
/// Darkness, and the ranges of sensors.
const SHRIEK_TICKS: u32 = 90;
const SHRIEK_COOLDOWN: u32 = 200;
const WARNING_DECAY: u32 = 12_000;
const MAX_WARNING: u8 = 4;
const BLAME_RANGE: f64 = 16.0;
const SHRIEK_DARKNESS_RANGE: f64 = 40.0;
const SENSOR_RANGE: i32 = 8;
const SHRIEKER_RANGE: i32 = 8;
/// Ticks between looks for sensors and shriekers around players.
const SCAN_EVERY: u64 = 10;
/// Summoning: tries, the box around the shrieker (half sides), and how close another
/// warden may be.
const SPAWN_TRIES: u32 = 20;
const SPAWN_BOX: (i32, i32) = (5, 6);
const OTHER_WARDEN_RANGE: f64 = 24.0;

/// Something a warden can be angry at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    Player(i32),
    Mob(i32),
}

#[derive(Debug, Clone, PartialEq)]
struct Suspect {
    uuid: u128,
    /// Found once it is about (suspects read from a save are matched by UUID).
    who: Option<Who>,
    anger: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Emerging(u32),
    Idle,
    Sniffing(u32),
    Roaring(u32),
    /// Charging a sonic boom at the target.
    Charging(u32),
    Digging(u32),
}

/// What the module keeps for a warden, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq)]
struct Warden {
    phase: Phase,
    suspects: Vec<Suspect>,
    /// What it hunts, after roaring, and for how many ticks.
    target: Option<Who>,
    chase_ticks: u32,
    /// Ticks since its last attack, and until it may attack again.
    since_attack: u32,
    attack_cooldown: u32,
    hear_cooldown: u32,
    touch_cooldown: u32,
    sniff_cooldown: u32,
    /// Ticks with nothing heard or sniffed.
    calm: u32,
    /// Where a vibration came from, to go and look.
    investigate: Option<[f64; 3]>,
    /// Projectiles heard lately: whose, and when.
    projectiles: Vec<(Who, u64)>,
    /// A player hit it from close by: hunt without roaring.
    skip_roar: bool,
    /// Hurt this tick: cancel the lift of the knockback.
    hurt: bool,
    shown_pose: i32,
    shown_anger: i32,
}

impl Warden {
    fn new(phase: Phase) -> Warden {
        Warden {
            phase,
            suspects: Vec::new(),
            target: None,
            chase_ticks: 0,
            since_attack: BOOM_IDLE,
            attack_cooldown: 0,
            hear_cooldown: 0,
            touch_cooldown: 0,
            sniff_cooldown: SNIFF_COOLDOWN.0,
            calm: 0,
            investigate: None,
            projectiles: Vec::new(),
            skip_roar: false,
            hurt: false,
            shown_pose: POSE_STANDING,
            shown_anger: 0,
        }
    }

    /// Its anger at its angriest suspect: the anger level the client shows.
    fn active_anger(&self) -> i32 {
        self.suspects.iter().map(|s| s.anger).max().unwrap_or(0)
    }

    fn anger_at(&self, who: Who) -> i32 {
        self.suspects
            .iter()
            .find(|s| s.who == Some(who))
            .map_or(0, |s| s.anger)
    }

    fn add_anger(&mut self, who: Who, uuid: u128, amount: i32) {
        match self
            .suspects
            .iter_mut()
            .find(|s| s.who == Some(who) || s.uuid == uuid)
        {
            Some(s) => {
                s.who = Some(who);
                s.anger = (s.anger + amount).clamp(0, MAX_ANGER);
            }
            None => self.suspects.push(Suspect {
                uuid,
                who: Some(who),
                anger: amount.clamp(0, MAX_ANGER),
            }),
        }
    }

    /// What to hunt once angry: the angriest player it is angry at, else the angriest.
    fn prey(&self) -> Option<Who> {
        if self.active_anger() < ANGRY {
            return None;
        }
        let angriest = |player: bool| {
            self.suspects
                .iter()
                .filter(|s| s.anger > 0)
                .filter(|s| !player || matches!(s.who, Some(Who::Player(_))))
                .filter(|s| s.who.is_some())
                .max_by_key(|s| s.anger)
                .and_then(|s| s.who)
        };
        let player = angriest(true).filter(|&who| self.anger_at(who) >= ANGRY);
        player.or_else(|| angriest(false))
    }

    fn pose(&self) -> i32 {
        match self.phase {
            Phase::Emerging(_) => POSE_EMERGING,
            Phase::Digging(_) => POSE_DIGGING,
            Phase::Sniffing(_) => POSE_SNIFFING,
            Phase::Roaring(_) => POSE_ROARING,
            Phase::Idle | Phase::Charging(_) => POSE_STANDING,
        }
    }

    /// Emerging or digging: deaf, and hurt by nothing but `/kill`.
    fn underground(&self) -> bool {
        matches!(self.phase, Phase::Emerging(_) | Phase::Digging(_))
    }
}

/// A player's warnings from shriekers.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Warning {
    level: u8,
    /// Ticks before they can set off a summoning shrieker again.
    cooldown: u32,
    /// Ticks since their level last rose or dropped.
    quiet: u32,
}

/// A shrieker shrieking.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shriek {
    pos: BlockPos,
    player: i32,
    ticks: u32,
    /// The warning level it raised the player to; 0 for one that cannot summon.
    level: u8,
}

/// A block a player placed or broke, heard by wardens around.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BlockVibration {
    at: [f64; 3],
    player: i32,
    tick: u64,
}

/// Where a player was, and when they last moved without sneaking.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Track {
    pos: [f64; 3],
    stepped: Option<u64>,
}

/// What the module keeps outside its wardens.
#[derive(Debug, Default)]
struct Wardens {
    /// Warning levels, by player UUID.
    warnings: HashMap<u128, Warning>,
    shrieks: Vec<Shriek>,
    players: HashMap<i32, Track>,
    vibrations: Vec<BlockVibration>,
    /// Wardens that dug down, removed after the mob tick.
    gone: Vec<i32>,
}

fn take_state(w: &mut World) -> Box<Wardens> {
    w.kin_state
        .remove(KEY)
        .and_then(|b| b.downcast::<Wardens>().ok())
        .unwrap_or_default()
}

fn put_state(w: &mut World, s: Box<Wardens>) {
    w.kin_state.insert(KEY, s);
}

fn state_ref(w: &World) -> Option<&Wardens> {
    w.kin_state
        .get(KEY)
        .and_then(|b| b.downcast_ref::<Wardens>())
}

fn warden_of(mob: &Mob) -> Option<&Warden> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Warden>())
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

fn centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

fn feet(pos: [f64; 3]) -> BlockPos {
    BlockPos::new(
        pos[0].floor() as i32,
        (pos[1] + 0.01).floor() as i32,
        pos[2].floor() as i32,
    )
}

/// A hit from `table` (Easy, Normal, Hard) on `who`: players take it by difficulty
/// (a warden is ticked outside the mob table, so `damage_from` does not scale it), mobs
/// always the Normal one.
fn by_difficulty(table: [f32; 3], who: Who) -> f32 {
    if matches!(who, Who::Mob(_)) {
        return table[1];
    }
    match crate::rules::difficulty() {
        Difficulty::Peaceful => 0.0,
        Difficulty::Easy => table[0],
        Difficulty::Normal => table[1],
        Difficulty::Hard => table[2],
    }
}

/// Players a warden notices and hunts: survival and adventure ones.
fn targetable(p: &Player) -> bool {
    p.alive && matches!(p.mode, GAME_MODE_SURVIVAL | GAME_MODE_ADVENTURE)
}

fn is_shrieker(state: u16) -> bool {
    crate::blocks::same_block(state, block::SCULK_SHRIEKER)
}

/// A shrieker whose block says it is shrieking.
fn shrieking(state: u16) -> bool {
    is_shrieker(state) && crate::blocks::prop(state, "shrieking") == Some("true")
}

fn is_sensor(state: u16) -> bool {
    crate::blocks::same_block(state, block::SCULK_SENSOR)
        || crate::blocks::same_block(state, block::CALIBRATED_SCULK_SENSOR)
}

/// Wool and carpet make no vibrations.
fn quiet_block(state: u16) -> bool {
    let name = crate::blocks::name(state);
    name.ends_with("_wool") || name.ends_with("_carpet")
}

fn pose_data(id: i32, pose: i32) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_POSE)
        .varint(SERIALIZER_POSE)
        .varint(pose)
        .u8(0xFF)
}

fn anger_data(id: i32, anger: i32) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_ANGER)
        .varint(SERIALIZER_VARINT)
        .varint(anger)
        .u8(0xFF)
}

/// One particle with no data of its own at `at`.
fn particle(kind: i32, at: [f64; 3]) -> Writer {
    Writer::packet(clientbound::LEVEL_PARTICLES)
        .varint(kind)
        .bool(false) // override limiter
        .bool(false) // always show
        .f64(at[0])
        .f64(at[1])
        .f64(at[2])
        .f32(0.0)
        .f32(0.0)
        .f32(0.0)
        .f32(0.0)
        .f32(0.0)
        .f32(0.0)
        .varint(1)
        .varint(0)
}

/// A shriek ring rising from a shrieker, starting after `delay` ticks.
fn shriek_particle(at: [f64; 3], delay: i32) -> Writer {
    Writer::packet(clientbound::LEVEL_PARTICLES)
        .varint(levitas_registry::particle::SHRIEK)
        .varint(delay)
        .bool(false)
        .bool(false)
        .f64(at[0])
        .f64(at[1])
        .f64(at[2])
        .f32(0.0)
        .f32(0.0)
        .f32(0.0)
        .f32(0.0)
        .f32(0.0)
        .f32(0.0)
        .varint(1)
        .varint(0)
}

pub(super) fn spawned(_w: &mut World, mob: &mut Mob, _biome: &str) {
    // Wardens never come with chunks; one that does stands ready.
    mob.kin = Some(Box::new(Warden::new(Phase::Idle)));
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if mob.kin.is_none() {
        // From a spawn egg or a command: there, without emerging.
        mob.kin = Some(Box::new(Warden::new(Phase::Idle)));
    }
    let Some(mut kin) = mob.kin.take() else {
        return false;
    };
    let busy = match kin.downcast_mut::<Warden>() {
        Some(wd) => w.tick_warden(id, mob, wd),
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

pub(super) fn hurt(w: &mut World, _id: i32, mob: &mut Mob, attacker: Option<i32>) {
    let Some(mut kin) = mob.kin.take() else {
        return;
    };
    if let Some(wd) = kin.downcast_mut::<Warden>() {
        wd.hurt = true;
        if let Some((pid, p)) = attacker.and_then(|a| w.players.get(&a).map(|p| (a, p))) {
            if targetable(p) {
                let who = Who::Player(pid);
                wd.add_anger(who, p.profile.uuid, ATTACKED_ANGER);
                wd.calm = 0;
                if distance(p.pos, mob.pos) <= NO_ROAR_RANGE {
                    wd.skip_roar = true;
                }
                if matches!(wd.phase, Phase::Sniffing(_)) {
                    wd.phase = Phase::Idle;
                }
            }
        }
    }
    mob.kin = Some(kin);
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let Some(wd) = warden_of(mob) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if wd.pose() != POSE_STANDING {
        out.push(pose_data(id, wd.pose()));
    }
    if wd.active_anger() > 0 {
        out.push(anger_data(id, wd.active_anger()));
    }
    out
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let Some(wd) = warden_of(mob) else {
        return;
    };
    let suspects: Vec<Tag> = wd
        .suspects
        .iter()
        .filter(|s| s.anger > 0)
        .map(|s| {
            let mut c = Compound::new();
            c.insert("anger", s.anger);
            c.insert("uuid", super::saved_entities::uuid_ints(s.uuid));
            Tag::Compound(c)
        })
        .collect();
    let mut anger = Compound::new();
    anger.insert("suspects", suspects);
    nbt.insert("anger", anger);
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let mut wd = Warden::new(Phase::Idle);
    let list = nbt
        .compound("anger")
        .and_then(|a| a.list("suspects"))
        .unwrap_or(&[]);
    for tag in list {
        let Tag::Compound(c) = tag else {
            continue;
        };
        let uuid = c.int_array("uuid").and_then(super::saved_entities::uuid_of);
        let anger = c.int("anger").unwrap_or(0).clamp(0, i64::from(MAX_ANGER)) as i32;
        if let (Some(uuid), true) = (uuid, anger > 0) {
            wd.suspects.push(Suspect {
                uuid,
                who: None,
                anger,
            });
        }
    }
    mob.kin = Some(Box::new(wd));
}

pub(super) fn damage_taken(
    _w: &mut World,
    _id: i32,
    mob: &mut Mob,
    damage: f32,
    damage_type: &str,
    _attacker: Option<i32>,
) -> f32 {
    if matches!(
        damage_type,
        "minecraft:in_fire"
            | "minecraft:on_fire"
            | "minecraft:lava"
            | "minecraft:hot_floor"
            | "minecraft:campfire"
    ) {
        return 0.0;
    }
    let killed = matches!(
        damage_type,
        "minecraft:generic_kill" | "minecraft:out_of_world"
    );
    if !killed && warden_of(mob).is_some_and(Warden::underground) {
        return 0.0;
    }
    damage
}

pub(super) fn tick_world(w: &mut World) {
    let mut s = take_state(w);
    w.tick_warnings(&mut s);
    w.tick_shrieks(&mut s);
    w.track_shrieker_players(&mut s);
    for id in std::mem::take(&mut s.gone) {
        if let Some(mob) = w.mobs.remove(&id) {
            w.send_to_viewers(&mob, items::remove_entity(id));
        }
    }
    let now = w.tick;
    s.vibrations.retain(|v| v.tick + 2 > now);
    put_state(w, s);
}

pub(super) fn block_placed(w: &mut World, player: i32, pos: BlockPos, state: u16) {
    if quiet_block(state) {
        return;
    }
    let mut s = take_state(w);
    w.sculk_block_vibration(&mut s, player, pos);
    put_state(w, s);
}

pub(super) fn block_broken(w: &mut World, player: i32, pos: BlockPos, state: u16) {
    let mut s = take_state(w);
    if is_shrieker(state) {
        // Broken mid-shriek: a warden it was summoning comes at once.
        if let Some(i) = s.shrieks.iter().position(|sh| sh.pos == pos) {
            let shriek = s.shrieks.remove(i);
            w.end_shriek(shriek, true);
        }
    }
    if !quiet_block(state) {
        w.sculk_block_vibration(&mut s, player, pos);
    }
    put_state(w, s);
}

/// Random ticks of a shrieker left shrieking (saved mid-shriek, or its chunk unloaded
/// before the shriek ended): with no shriek running there, it falls quiet.
pub(super) fn block_tick(w: &mut World, pos: BlockPos, state: u16) -> bool {
    if !shrieking(state) {
        return false;
    }
    let running = state_ref(w).is_some_and(|s| s.shrieks.iter().any(|sh| sh.pos == pos));
    if !running {
        w.set_block_updating(pos, crate::blocks::with_bool(state, "shrieking", false));
    }
    true
}

pub(super) fn ticks_randomly(state: u16) -> bool {
    shrieking(state)
}

impl World {
    /// A warden's turn. Returns whether it stands still this tick (moving itself).
    fn tick_warden(&mut self, id: i32, mob: &mut Mob, wd: &mut Warden) -> bool {
        // Nothing knocks it back.
        mob.knockback = [0.0; 2];
        if std::mem::take(&mut wd.hurt) && mob.vel[1] > 0.0 {
            mob.vel[1] = 0.0;
        }
        if !matches!(wd.phase, Phase::Digging(_)) {
            self.warden_darkness(id, mob);
        }
        let busy = self.warden_turn(id, mob, wd);
        self.show_warden(id, mob, wd);
        busy
    }

    fn warden_turn(&mut self, id: i32, mob: &mut Mob, wd: &mut Warden) -> bool {
        match wd.phase {
            Phase::Emerging(t) => {
                stand(mob);
                wd.phase = if t > 1 {
                    Phase::Emerging(t - 1)
                } else {
                    Phase::Idle
                };
                return true;
            }
            Phase::Digging(t) => {
                stand(mob);
                if t > 1 {
                    wd.phase = Phase::Digging(t - 1);
                } else if t == 1 {
                    wd.phase = Phase::Digging(0);
                    self.warden_gone(id);
                }
                return true;
            }
            _ => {}
        }
        wd.attack_cooldown = wd.attack_cooldown.saturating_sub(1);
        wd.since_attack = wd.since_attack.saturating_add(1);
        wd.hear_cooldown = wd.hear_cooldown.saturating_sub(1);
        wd.touch_cooldown = wd.touch_cooldown.saturating_sub(1);
        wd.sniff_cooldown = wd.sniff_cooldown.saturating_sub(1);
        self.keep_suspects_of_warden(wd);
        let heard = wd.hear_cooldown == 0 && self.warden_listen(id, mob, wd);
        let touched = wd.touch_cooldown == 0 && self.warden_feel(mob, wd);
        if heard || touched {
            wd.calm = 0;
        }

        match wd.phase {
            Phase::Roaring(t) => {
                stand(mob);
                if let Some(who) = wd.target {
                    if let Some((at, _)) = self.suspect_at(who) {
                        face(mob, at);
                    }
                }
                if t > 1 {
                    wd.phase = Phase::Roaring(t - 1);
                } else {
                    wd.phase = Phase::Idle;
                    if let Some(who) = wd.target {
                        let uuid = self.suspect_uuid(who).unwrap_or(0);
                        wd.add_anger(who, uuid, ROAR_ANGER);
                    }
                }
                return true;
            }
            Phase::Sniffing(t) => {
                stand(mob);
                if t > 1 {
                    wd.phase = Phase::Sniffing(t - 1);
                } else {
                    wd.phase = Phase::Idle;
                    wd.sniff_cooldown =
                        SNIFF_COOLDOWN.0 + self.rng.below(SNIFF_COOLDOWN.1 - SNIFF_COOLDOWN.0 + 1);
                    if let Some(who) = self.nearest_suspect(mob.pos, SNIFF_REACH, false) {
                        let uuid = self.suspect_uuid(who).unwrap_or(0);
                        wd.add_anger(who, uuid, SNIFF_ANGER);
                        wd.calm = 0;
                    }
                }
                return true;
            }
            Phase::Charging(t) => {
                stand(mob);
                let Some(who) = wd.target else {
                    wd.phase = Phase::Idle;
                    return true;
                };
                if let Some((at, _)) = self.suspect_at(who) {
                    face(mob, at);
                }
                if t > 1 {
                    wd.phase = Phase::Charging(t - 1);
                } else {
                    wd.phase = Phase::Idle;
                    self.sonic_boom(id, mob, who);
                    wd.attack_cooldown = BOOM_TOTAL - BOOM_CHARGE;
                    wd.since_attack = 0;
                }
                return true;
            }
            _ => {}
        }

        // Idle: hunt what it is angry at, else look into what it heard, else sniff out
        // whatever is about.
        if let Some(who) = wd.prey() {
            wd.calm = 0;
            if wd.target != Some(who) {
                wd.target = Some(who);
                wd.chase_ticks = 0;
                if !std::mem::take(&mut wd.skip_roar) {
                    wd.phase = Phase::Roaring(ROAR_TICKS);
                    stand(mob);
                    self.play_sound_at("minecraft:entity.warden.roar", mob.pos, 3.0, 1.0, None);
                    return true;
                }
            }
            wd.skip_roar = false;
            return self.warden_chase(id, mob, wd, who);
        }
        if wd.target.take().is_some() {
            mob.hunting = None;
        }
        wd.skip_roar = false;
        if let Some(at) = wd.investigate {
            if horizontal(at, mob.pos) < 2.0 && (at[1] - mob.pos[1]).abs() < 3.0 {
                wd.investigate = None;
            } else {
                self.follow_path(mob, feet(at));
            }
        } else if let Some(who) = self.nearest_suspect(mob.pos, AWARE, true) {
            if wd.sniff_cooldown == 0 {
                wd.phase = Phase::Sniffing(SNIFF_TICKS);
                stand(mob);
                self.play_sound_at("minecraft:entity.warden.sniff", mob.pos, 5.0, 1.0, None);
                return true;
            }
            if let Some((at, _)) = self.suspect_at(who) {
                self.follow_path(mob, feet(at));
            }
        }
        if heard || touched {
            return false;
        }
        wd.calm += 1;
        if wd.calm >= CALM_TICKS && mob.name.is_none() {
            let in_liquid = self
                .loaded_block(feet(mob.pos))
                .and_then(fluid_at)
                .is_some();
            stand(mob);
            if in_liquid {
                self.warden_gone(id);
                wd.phase = Phase::Digging(0);
            } else {
                wd.phase = Phase::Digging(DIG_TICKS);
                self.play_sound_at("minecraft:entity.warden.dig", mob.pos, 5.0, 1.0, None);
            }
            return true;
        }
        false
    }

    /// Hunting `who`: walking up, striking within reach, booming from afar.
    fn warden_chase(&mut self, id: i32, mob: &mut Mob, wd: &mut Warden, who: Who) -> bool {
        let Some((at, (_, height))) = self.suspect_at(who) else {
            wd.target = None;
            mob.hunting = None;
            return false;
        };
        wd.chase_ticks = wd.chase_ticks.saturating_add(1);
        mob.hunting = match who {
            Who::Player(pid) => Some(pid),
            Who::Mob(_) => None,
        };
        mob.head_yaw = yaw_towards(mob.pos, at);
        let (width, own_height) = mob.size();
        let reach = ((width * 2.0).powi(2) + 0.6).sqrt();
        let across = horizontal(at, mob.pos);
        let in_reach =
            across <= reach && at[1] < mob.pos[1] + own_height && mob.pos[1] < at[1] + height;
        if in_reach {
            if wd.attack_cooldown == 0 {
                self.warden_strike(id, mob, who, at);
                wd.attack_cooldown = MELEE_COOLDOWN;
                wd.since_attack = 0;
            }
        } else if wd.chase_ticks >= BOOM_CHASE
            && wd.since_attack >= BOOM_IDLE
            && wd.attack_cooldown == 0
            && in_boom_range(mob.pos, at)
        {
            wd.phase = Phase::Charging(BOOM_CHARGE);
            stand(mob);
            face(mob, at);
            self.send_to_viewers(mob, entity_event(id, EVENT_SONIC_BOOM));
            self.play_sound_at(
                "minecraft:entity.warden.sonic_charge",
                mob.pos,
                3.0,
                1.0,
                None,
            );
            return true;
        }
        self.follow_path(mob, feet(at));
        false
    }

    fn warden_strike(&mut self, id: i32, mob: &Mob, who: Who, at: [f64; 3]) {
        let damage = by_difficulty(MELEE_DAMAGE, who);
        self.send_to_viewers(mob, entity_event(id, EVENT_ATTACK));
        self.play_sound_at(
            "minecraft:entity.warden.attack_impact",
            mob.pos,
            1.0,
            1.0,
            None,
        );
        let yaw = f64::from(yaw_towards(mob.pos, at)).to_radians();
        let away = [-yaw.sin(), yaw.cos()];
        match who {
            Who::Player(pid) => {
                if self.damage_from(pid, damage, Damage::Mob("Warden"), Some(id)) {
                    self.push_player(pid, [away[0] * MELEE_PUSH, 0.36, away[1] * MELEE_PUSH]);
                }
            }
            Who::Mob(mid) => {
                self.hit_mob(mid, damage, None, away, "minecraft:mob_attack");
            }
        }
    }

    /// The sonic boom goes off: straight through blocks at its target, if in range.
    fn sonic_boom(&mut self, id: i32, mob: &Mob, who: Who) {
        let Some((at, (_, height))) = self.suspect_at(who) else {
            return;
        };
        if !in_boom_range(mob.pos, at) {
            return;
        }
        let chest = [mob.pos[0], mob.pos[1] + 1.6, mob.pos[2]];
        let body = [at[0], at[1] + height / 2.0, at[2]];
        let d = [body[0] - chest[0], body[1] - chest[1], body[2] - chest[2]];
        let length = distance(chest, body).max(1e-6);
        let steps = length.ceil() as i32;
        for i in 1..=steps {
            let t = f64::from(i) / f64::from(steps);
            let point = [
                chest[0] + d[0] * t,
                chest[1] + d[1] * t,
                chest[2] + d[2] * t,
            ];
            self.send_to_viewers(mob, particle(levitas_registry::particle::SONIC_BOOM, point));
        }
        self.play_sound_at(
            "minecraft:entity.warden.sonic_boom",
            mob.pos,
            3.0,
            1.0,
            None,
        );
        let damage = by_difficulty(BOOM_DAMAGE, who);
        let dir = [d[0] / length, d[1] / length, d[2] / length];
        match who {
            Who::Player(pid) => {
                // Magic: through armour and shields.
                if self.damage_from(pid, damage, Damage::Magic, Some(id)) {
                    self.push_player(
                        pid,
                        [
                            dir[0] * BOOM_PUSH.0,
                            dir[1] * BOOM_PUSH.1 + 0.1,
                            dir[2] * BOOM_PUSH.0,
                        ],
                    );
                }
            }
            Who::Mob(mid) => {
                self.hit_mob(mid, damage, None, [0.0, 0.0], "minecraft:sonic_boom");
                if let Some(m) = self.mobs.get_mut(&mid).filter(|m| m.alive()) {
                    m.push([dir[0] * BOOM_PUSH.0, BOOM_PUSH.1, dir[2] * BOOM_PUSH.0]);
                }
            }
        }
    }

    /// Every 6 seconds: Darkness for players around.
    fn warden_darkness(&mut self, id: i32, mob: &Mob) {
        if !(self.tick + u64::from(id.unsigned_abs())).is_multiple_of(DARKNESS_EVERY) {
            return;
        }
        let near: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| targetable(p) && distance(p.pos, mob.pos) <= DARKNESS_RANGE)
            .map(|(&pid, _)| pid)
            .collect();
        for pid in near {
            self.add_effect(pid, darkness());
        }
    }

    /// Tells viewers about a new pose or anger level.
    fn show_warden(&mut self, id: i32, mob: &Mob, wd: &mut Warden) {
        let pose = wd.pose();
        if pose != wd.shown_pose {
            wd.shown_pose = pose;
            self.send_to_viewers(mob, pose_data(id, pose));
        }
        let anger = wd.active_anger();
        let crossed =
            (anger >= ANGRY) != (wd.shown_anger >= ANGRY) || (anger == 0) != (wd.shown_anger == 0);
        if anger != wd.shown_anger
            && (crossed || (anger - wd.shown_anger).abs() >= 5 || self.tick.is_multiple_of(20))
        {
            wd.shown_anger = anger;
            self.send_to_viewers(mob, anger_data(id, anger));
        }
    }

    fn warden_gone(&mut self, id: i32) {
        let mut s = take_state(self);
        s.gone.push(id);
        put_state(self, s);
    }

    /// Matches suspects to who is about, drops those gone, and lets anger cool.
    fn keep_suspects_of_warden(&self, wd: &mut Warden) {
        let decay = self.tick.is_multiple_of(20);
        let resolve = self.tick.is_multiple_of(20);
        for s in &mut wd.suspects {
            if decay {
                s.anger -= 1;
            }
            if s.who.is_none() && resolve {
                s.who = self.suspect_by_uuid(s.uuid);
            }
            match s.who {
                Some(Who::Player(pid)) => match self.players.get(&pid) {
                    Some(p) if targetable(p) => {}
                    // Dead, gone, or creative or spectator: forgotten.
                    _ => s.anger = 0,
                },
                Some(Who::Mob(mid)) if !self.mobs.get(&mid).is_some_and(Mob::alive) => {
                    s.anger = 0;
                }
                _ => {}
            }
        }
        wd.suspects.retain(|s| s.anger > 0);
        if wd.target.is_some_and(|t| wd.anger_at(t) <= 0) {
            wd.target = None;
        }
        let now = self.tick;
        wd.projectiles
            .retain(|&(_, at)| at + PROJECTILE_PAIR_TICKS > now);
    }

    /// Whose UUID this is, among players and mobs about.
    fn suspect_by_uuid(&self, uuid: u128) -> Option<Who> {
        self.players
            .iter()
            .find(|(_, p)| p.profile.uuid == uuid)
            .map(|(&pid, _)| Who::Player(pid))
            .or_else(|| {
                self.mobs
                    .iter()
                    .find(|(_, m)| m.uuid == uuid)
                    .map(|(&mid, _)| Who::Mob(mid))
            })
    }

    fn suspect_uuid(&self, who: Who) -> Option<u128> {
        match who {
            Who::Player(pid) => self.players.get(&pid).map(|p| p.profile.uuid),
            Who::Mob(mid) => self.mobs.get(&mid).map(|m| m.uuid),
        }
    }

    /// Where something a warden may hunt stands, and its size, while it may be hunted.
    fn suspect_at(&self, who: Who) -> Option<([f64; 3], (f64, f64))> {
        match who {
            Who::Player(pid) => self
                .players
                .get(&pid)
                .filter(|p| targetable(p))
                .map(|p| (p.pos, PLAYER_SIZE)),
            Who::Mob(mid) => self
                .mobs
                .get(&mid)
                .filter(|m| m.alive() && m.kind != MobKind::Warden)
                .map(|m| (m.pos, m.size())),
        }
    }

    /// The nearest player (or, with none, mob) within `reach` (across, up or down) of
    /// `at`; with `players_first` false, the nearest of either.
    fn nearest_suspect(&self, at: [f64; 3], reach: (f64, f64), players_first: bool) -> Option<Who> {
        let within = |pos: [f64; 3]| {
            (pos[0] - at[0]).abs() <= reach.0
                && (pos[2] - at[2]).abs() <= reach.0
                && (pos[1] - at[1]).abs() <= reach.1
                && (players_first || horizontal(pos, at) <= reach.0)
        };
        let player = self
            .players
            .iter()
            .filter(|(_, p)| targetable(p) && within(p.pos))
            .map(|(&pid, p)| (Who::Player(pid), distance(p.pos, at)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if players_first && player.is_some() {
            return player.map(|(who, _)| who);
        }
        let mob = self
            .mobs
            .iter()
            .filter(|(_, m)| m.alive() && m.kind != MobKind::Warden && within(m.pos))
            .map(|(&mid, m)| (Who::Mob(mid), distance(m.pos, at)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match (player, mob) {
            (Some(p), Some(m)) => Some(if p.1 <= m.1 { p.0 } else { m.0 }),
            (p, m) => p.or(m).map(|(who, _)| who),
        }
    }

    /// Hears the nearest vibration within range, if any. Returns whether it did.
    fn warden_listen(&mut self, id: i32, mob: &Mob, wd: &mut Warden) -> bool {
        let here = [mob.pos[0], mob.pos[1] + 1.0, mob.pos[2]];
        // (where, who caused it, whether a projectile)
        let mut best: Option<([f64; 3], Who, bool, f64)> = None;
        let mut offer = |at: [f64; 3], who: Who, projectile: bool| {
            let d = distance(at, here);
            if d <= LISTEN_RANGE && best.is_none_or(|b| d < b.3) {
                best = Some((at, who, projectile, d));
            }
        };
        let state = state_ref(self);
        for (&pid, p) in &self.players {
            if !targetable(p) || p.sneaking {
                continue;
            }
            let moved = state
                .and_then(|s| s.players.get(&pid))
                .is_some_and(|t| distance(t.pos, p.pos) > 1e-3);
            if moved {
                offer(p.pos, Who::Player(pid), false);
            }
        }
        if let Some(s) = state {
            for v in &s.vibrations {
                if self.players.get(&v.player).is_some_and(targetable) {
                    offer(v.at, Who::Player(v.player), false);
                }
            }
        }
        for (&mid, m) in &self.mobs {
            if m.alive()
                && m.kind != MobKind::Warden
                && m.on_ground_now()
                && m.vel[0].hypot(m.vel[2]) > 0.01
            {
                offer(m.pos, Who::Mob(mid), false);
            }
        }
        for a in self.arrows.values() {
            if a.stuck == Some(0) {
                let who = if a.by_player {
                    Who::Player(a.shooter)
                } else {
                    Who::Mob(a.shooter)
                };
                if who != Who::Mob(id) {
                    offer(a.pos, who, true);
                }
            }
        }
        let Some((at, who, projectile, _)) = best else {
            return false;
        };
        wd.hear_cooldown = HEAR_COOLDOWN;
        wd.investigate = Some(at);
        let now = self.tick;
        let amount = if projectile {
            let pair = wd.projectiles.iter().any(|&(w, _)| w == who);
            wd.projectiles.push((who, now));
            let far = self
                .suspect_at(who)
                .is_none_or(|(pos, _)| distance(pos, mob.pos) > PROJECTILE_MAX_DISTANCE);
            if far {
                0
            } else if pair {
                VIBRATION_ANGER
            } else {
                PROJECTILE_ANGER
            }
        } else {
            VIBRATION_ANGER
        };
        if amount > 0 && self.suspect_at(who).is_some() {
            let uuid = self.suspect_uuid(who).unwrap_or(0);
            wd.add_anger(who, uuid, amount);
        }
        self.send_to_viewers(mob, entity_event(id, EVENT_TENDRILS));
        let sound = if wd.active_anger() >= ANGRY {
            "minecraft:entity.warden.listening_angry"
        } else {
            "minecraft:entity.warden.listening"
        };
        self.play_sound_at(sound, mob.pos, 2.0, 1.0, None);
        true
    }

    /// Players and mobs touching it anger it. Returns whether any did.
    fn warden_feel(&self, mob: &Mob, wd: &mut Warden) -> bool {
        let (width, height) = mob.size();
        let touching = |pos: [f64; 3], (w2, h2): (f64, f64)| {
            let reach = (width + w2) / 2.0 + 0.05;
            (pos[0] - mob.pos[0]).abs() < reach
                && (pos[2] - mob.pos[2]).abs() < reach
                && pos[1] < mob.pos[1] + height
                && mob.pos[1] < pos[1] + h2
        };
        let mut touched: Vec<(Who, u128)> = Vec::new();
        for (&pid, p) in &self.players {
            if targetable(p) && touching(p.pos, PLAYER_SIZE) {
                touched.push((Who::Player(pid), p.profile.uuid));
            }
        }
        for (&mid, m) in &self.mobs {
            if m.alive() && m.kind != MobKind::Warden && touching(m.pos, m.size()) {
                touched.push((Who::Mob(mid), m.uuid));
            }
        }
        if touched.is_empty() {
            return false;
        }
        for (who, uuid) in touched {
            wd.add_anger(who, uuid, TOUCH_ANGER);
        }
        wd.touch_cooldown = TOUCH_COOLDOWN;
        true
    }

    // --- Shriekers ---

    /// Warning levels drop after 10 quiet minutes; waits between shrieks run out.
    fn tick_warnings(&mut self, s: &mut Wardens) {
        for warning in s.warnings.values_mut() {
            warning.cooldown = warning.cooldown.saturating_sub(1);
            warning.quiet += 1;
            if warning.quiet >= WARNING_DECAY {
                warning.quiet = 0;
                warning.level = warning.level.saturating_sub(1);
            }
        }
        s.warnings.retain(|_, w| w.level > 0 || w.cooldown > 0);
    }

    fn tick_shrieks(&mut self, s: &mut Wardens) {
        let mut ended = Vec::new();
        s.shrieks.retain_mut(|sh| {
            sh.ticks = sh.ticks.saturating_sub(1);
            if sh.ticks == 0 {
                ended.push(*sh);
                false
            } else {
                true
            }
        });
        for shriek in ended {
            self.end_shriek(shriek, false);
        }
    }

    /// Notes where players are, and every few ticks lets those who moved without
    /// sneaking set off sensors, and those standing on shriekers set them off.
    fn track_shrieker_players(&mut self, s: &mut Wardens) {
        let now = self.tick;
        s.players.retain(|pid, _| self.players.contains_key(pid));
        for (&pid, p) in &self.players {
            let track = s.players.entry(pid).or_insert(Track {
                pos: p.pos,
                stepped: None,
            });
            let moved = distance(track.pos, p.pos) > 1e-3;
            if moved && p.alive && !p.sneaking && p.mode != GAME_MODE_SPECTATOR {
                track.stepped = Some(now);
            }
            track.pos = p.pos;
        }
        if !now.is_multiple_of(SCAN_EVERY) {
            return;
        }
        let around: Vec<(i32, [f64; 3], bool)> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive && p.mode != GAME_MODE_SPECTATOR)
            .map(|(&pid, p)| {
                let stepped = s
                    .players
                    .get(&pid)
                    .and_then(|t| t.stepped)
                    .is_some_and(|t| t + SCAN_EVERY > now);
                (pid, p.pos, stepped)
            })
            .collect();
        for (pid, pos, stepped) in around {
            let under = feet(pos);
            if self.loaded_block(under).is_some_and(is_shrieker) {
                self.set_off_shrieker(s, under, pid);
            }
            if stepped {
                self.sculk_vibration(s, pid, pos);
            }
        }
    }

    /// A player placed or broke a block: wardens and sensors around hear it.
    fn sculk_block_vibration(&mut self, s: &mut Wardens, player: i32, pos: BlockPos) {
        if !self
            .players
            .get(&player)
            .is_some_and(|p| p.alive && p.mode != GAME_MODE_SPECTATOR)
        {
            return;
        }
        let at = centre(pos);
        s.vibrations.push(BlockVibration {
            at,
            player,
            tick: self.tick,
        });
        self.sculk_vibration(s, player, at);
    }

    /// A vibration a player made at `at`: sensors within 8 blocks pass it on to the
    /// shriekers within 8 blocks of them.
    fn sculk_vibration(&mut self, s: &mut Wardens, player: i32, at: [f64; 3]) {
        let middle = feet(at);
        let sensors = self.sculk_blocks_near(middle, SENSOR_RANGE, at, is_sensor);
        if sensors.is_empty() {
            return;
        }
        let shriekers =
            self.sculk_blocks_near(middle, SENSOR_RANGE + SHRIEKER_RANGE, at, is_shrieker);
        let reach = f64::from(SHRIEKER_RANGE);
        for pos in shriekers {
            if sensors
                .iter()
                .any(|&sensor| distance(centre(sensor), centre(pos)) <= reach)
            {
                self.set_off_shrieker(s, pos, player);
            }
        }
    }

    /// The loaded blocks within `radius` of `at` (a sphere) that `wanted` picks.
    fn sculk_blocks_near(
        &self,
        middle: BlockPos,
        radius: i32,
        at: [f64; 3],
        wanted: fn(u16) -> bool,
    ) -> Vec<BlockPos> {
        let mut found = Vec::new();
        let r = f64::from(radius);
        for dy in -radius..=radius {
            let y = middle.y + dy;
            if !(MIN_Y..=MAX_Y).contains(&y) {
                continue;
            }
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let pos = BlockPos::new(middle.x + dx, y, middle.z + dz);
                    if self.loaded_block(pos).is_some_and(wanted) && distance(centre(pos), at) <= r
                    {
                        found.push(pos);
                    }
                }
            }
        }
        found
    }

    /// A shrieker is set off by `player`: it shrieks, and if it can summon, warns.
    fn set_off_shrieker(&mut self, s: &mut Wardens, pos: BlockPos, player: i32) {
        let Some(mut state) = self.loaded_block(pos) else {
            return;
        };
        if !is_shrieker(state) {
            return;
        }
        if shrieking(state) {
            if s.shrieks.iter().any(|sh| sh.pos == pos) {
                return;
            }
            // Left shrieking by a save or an unload mid-shriek: it is idle really.
            state = crate::blocks::with_bool(state, "shrieking", false);
            self.set_block_updating(pos, state);
        }
        let Some(p) = self.players.get(&player) else {
            return;
        };
        let summons = crate::blocks::prop(state, "can_summon") == Some("true")
            && crate::rules::difficulty() != Difficulty::Peaceful
            && crate::rules::bool("spawn_wardens");
        let mut level = 0;
        if summons {
            if s.warnings
                .get(&p.profile.uuid)
                .is_some_and(|w| w.cooldown > 0)
            {
                return;
            }
            // Everyone close by is blamed along with them.
            let from = p.pos;
            let blamed: Vec<u128> = self
                .players
                .values()
                .filter(|q| {
                    q.alive && q.mode != GAME_MODE_SPECTATOR && distance(q.pos, from) <= BLAME_RANGE
                })
                .map(|q| q.profile.uuid)
                .collect();
            let top = blamed
                .iter()
                .filter_map(|u| s.warnings.get(u))
                .map(|w| w.level)
                .max()
                .unwrap_or(0);
            level = (top + 1).min(MAX_WARNING);
            for uuid in blamed {
                s.warnings.insert(
                    uuid,
                    Warning {
                        level,
                        cooldown: SHRIEK_COOLDOWN,
                        quiet: 0,
                    },
                );
            }
        }
        self.set_block_updating(pos, crate::blocks::with_bool(state, "shrieking", true));
        let at = centre(pos);
        if crate::blocks::prop(state, "waterlogged") != Some("true") {
            self.play_sound_at("minecraft:block.sculk_shrieker.shriek", at, 2.0, 1.0, None);
        }
        let top = [at[0], f64::from(pos.y) + 0.5, at[2]];
        for ring in 0..10 {
            self.broadcast(shriek_particle(top, ring * 5), None);
        }
        s.shrieks.push(Shriek {
            pos,
            player,
            ticks: SHRIEK_TICKS,
            level,
        });
    }

    /// A shriek is over (or its shrieker broke): Darkness, the warning, and at the last
    /// warning a warden.
    fn end_shriek(&mut self, shriek: Shriek, broken: bool) {
        let at = centre(shriek.pos);
        if !broken {
            if let Some(state) = self.loaded_block(shriek.pos).filter(|&s| is_shrieker(s)) {
                self.set_block_updating(
                    shriek.pos,
                    crate::blocks::with_bool(state, "shrieking", false),
                );
            }
        }
        if shriek.level == 0 {
            return;
        }
        let near = self
            .players
            .get(&shriek.player)
            .is_some_and(|p| distance(p.pos, at) <= SHRIEK_DARKNESS_RANGE);
        if near && !broken {
            let dark: Vec<i32> = self
                .players
                .iter()
                .filter(|(_, p)| targetable(p) && distance(p.pos, at) <= SHRIEK_DARKNESS_RANGE)
                .map(|(&pid, _)| pid)
                .collect();
            for pid in dark {
                self.add_effect(pid, darkness());
            }
        }
        let warning = match shriek.level {
            1 => Some("minecraft:entity.warden.nearby_close"),
            2 => Some("minecraft:entity.warden.nearby_closer"),
            3 => Some("minecraft:entity.warden.nearby_closest"),
            _ => None,
        };
        if let Some(sound) = warning {
            self.play_sound_at(sound, at, 5.0, 1.0, None);
        }
        if shriek.level >= MAX_WARNING && !self.summon_warden(shriek.pos) {
            self.play_sound_at(
                "minecraft:entity.warden.listening_angry",
                at,
                5.0,
                1.0,
                None,
            );
        }
    }

    /// A warden emerges near a shrieker. Returns false when no spot was found (true
    /// when another warden is close by already, and none comes).
    fn summon_warden(&mut self, shrieker: BlockPos) -> bool {
        let at = centre(shrieker);
        if self.mobs.values().any(|m| {
            m.kind == MobKind::Warden && m.alive() && distance(m.pos, at) <= OTHER_WARDEN_RANGE
        }) {
            return true;
        }
        let height = MobKind::Warden.size().1.ceil() as i32;
        for _ in 0..SPAWN_TRIES {
            let x = shrieker.x + self.rng.below(2 * SPAWN_BOX.0 as u32 + 1) as i32 - SPAWN_BOX.0;
            let z = shrieker.z + self.rng.below(2 * SPAWN_BOX.0 as u32 + 1) as i32 - SPAWN_BOX.0;
            let spot = (-SPAWN_BOX.1..=SPAWN_BOX.1)
                .rev()
                .map(|dy| BlockPos::new(x, shrieker.y + dy, z))
                .find(|&pos| self.warden_fits(pos, height));
            let Some(pos) = spot else {
                continue;
            };
            let feet = [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y),
                f64::from(pos.z) + 0.5,
            ];
            let mut mob = Mob::new(MobKind::Warden, feet, &mut self.rng);
            mob.kin = Some(Box::new(Warden::new(Phase::Emerging(EMERGE_TICKS))));
            self.mobs.insert(next_entity_id(), mob);
            self.play_sound_at("minecraft:entity.warden.emerge", feet, 5.0, 1.0, None);
            return true;
        }
        false
    }

    /// Whether a warden can stand with its feet at `pos`: a full top face below, room
    /// for its body, no liquid at its feet, and no one in the way.
    fn warden_fits(&self, pos: BlockPos, height: i32) -> bool {
        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
        if !self
            .loaded_block(below)
            .is_some_and(|s| levitas_registry::face_sturdy(s, 1))
        {
            return false;
        }
        let Some(foot) = self.loaded_block(pos) else {
            return false;
        };
        if fluid_at(foot).is_some() {
            return false;
        }
        let room = (0..height).all(|dy| {
            self.loaded_block(BlockPos::new(pos.x, pos.y + dy, pos.z))
                .is_some_and(|s| levitas_registry::collision_boxes(s).is_empty())
        });
        if !room {
            return false;
        }
        let spot = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let (width, tall) = MobKind::Warden.size();
        let clear = |p: [f64; 3], (w2, h2): (f64, f64)| {
            let reach = (width + w2) / 2.0;
            !((p[0] - spot[0]).abs() < reach
                && (p[2] - spot[2]).abs() < reach
                && p[1] < spot[1] + tall
                && spot[1] < p[1] + h2)
        };
        self.players
            .values()
            .all(|p| !p.alive || clear(p.pos, PLAYER_SIZE))
            && self
                .mobs
                .values()
                .all(|m| !m.alive() || clear(m.pos, m.size()))
    }
}

/// The warden stands where it is this tick.
fn stand(mob: &mut Mob) {
    mob.target = None;
    mob.path.clear();
    mob.vel[0] = 0.0;
    mob.vel[2] = 0.0;
}

fn face(mob: &mut Mob, at: [f64; 3]) {
    mob.yaw = yaw_towards(mob.pos, at);
    mob.head_yaw = mob.yaw;
}

fn in_boom_range(from: [f64; 3], to: [f64; 3]) -> bool {
    let (across, up) = BOOM_RANGE;
    let h = horizontal(from, to) / across;
    let v = (to[1] - from[1]) / up;
    h * h + v * v <= 1.0
}

fn darkness() -> levitas_registry::Effect {
    levitas_registry::Effect {
        id: levitas_registry::mob_effect::DARKNESS,
        duration: DARKNESS_TICKS,
        amplifier: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-wardens-{}-{}",
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
        let alice = add_player(&mut w, "Alice", 11, [-28.5, -60.0, -28.5]);
        (w, alice)
    }

    fn add_player(w: &mut World, name: &str, uuid: u128, at: [f64; 3]) -> i32 {
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let id = next_entity_id();
        let profile = Profile {
            name: name.to_owned(),
            uuid,
            properties: Vec::new(),
        };
        w.players.insert(id, Player::new(profile, out, at));
        id
    }

    fn warden(w: &mut World, at: [f64; 3], phase: Phase) -> (i32, Mob) {
        let mut mob = Mob::new(MobKind::Warden, at, &mut w.rng);
        mob.kin = Some(Box::new(Warden::new(phase)));
        (next_entity_id(), mob)
    }

    fn state(mob: &Mob) -> &Warden {
        warden_of(mob).expect("warden state")
    }

    fn state_mut(mob: &mut Mob) -> &mut Warden {
        mob.kin
            .as_mut()
            .and_then(|k| k.downcast_mut::<Warden>())
            .expect("warden state")
    }

    /// One server tick for a warden held outside the mob table, then the module's.
    fn step(w: &mut World, id: i32, mob: &mut Mob) -> bool {
        let busy = tick(w, id, mob);
        tick_world(w);
        w.tick += 1;
        busy
    }

    fn jiggle(w: &mut World, player: i32, i: u32) {
        let p = w.players.get_mut(&player).unwrap();
        p.pos[0] += if i.is_multiple_of(2) { 0.1 } else { -0.1 };
    }

    fn summoning_shrieker() -> u16 {
        crate::blocks::with_bool(block::SCULK_SHRIEKER, "can_summon", true)
    }

    fn wardens_in(w: &World) -> usize {
        w.mobs
            .values()
            .filter(|m| m.kind == MobKind::Warden)
            .count()
    }

    #[test]
    fn wardens_shrug_off_fire_and_cannot_be_hurt_underground() {
        let (mut w, _) = world_with_player();
        assert_eq!(MobKind::Warden.max_health(), 500.0);
        let (id, mut mob) = warden(&mut w, [0.5, -60.0, 0.5], Phase::Idle);
        for fire in ["minecraft:lava", "minecraft:on_fire", "minecraft:in_fire"] {
            assert_eq!(damage_taken(&mut w, id, &mut mob, 4.0, fire, None), 0.0);
        }
        let hit = damage_taken(&mut w, id, &mut mob, 7.0, "minecraft:player_attack", None);
        assert_eq!(hit, 7.0);
        state_mut(&mut mob).phase = Phase::Emerging(10);
        let hit = damage_taken(&mut w, id, &mut mob, 7.0, "minecraft:player_attack", None);
        assert_eq!(hit, 0.0);
        let kill = damage_taken(&mut w, id, &mut mob, 1e9, "minecraft:generic_kill", None);
        assert_eq!(kill, 1e9);
        // Knockback is wiped on its turn.
        state_mut(&mut mob).phase = Phase::Idle;
        mob.knockback = [0.8, -0.4];
        tick(&mut w, id, &mut mob);
        assert_eq!(mob.knockback, [0.0, 0.0]);
    }

    #[test]
    fn anger_is_saved_and_read_back_as_vanilla() {
        let (mut w, alice) = world_with_player();
        let (_, mut mob) = warden(&mut w, [0.5, -60.0, 0.5], Phase::Idle);
        let uuid = 0x0123_4567_89ab_cdef_0011_2233_4455_6677;
        state_mut(&mut mob).add_anger(Who::Player(alice), uuid, 90);
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        let suspects = nbt
            .compound("anger")
            .and_then(|a| a.list("suspects"))
            .expect("suspects");
        assert_eq!(suspects.len(), 1);
        let mut back = Mob::new(MobKind::Warden, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        let wd = state(&back);
        assert_eq!(wd.suspects.len(), 1);
        assert_eq!(wd.suspects[0].uuid, uuid);
        assert_eq!(wd.suspects[0].anger, 90);
        assert_eq!(wd.suspects[0].who, None);
        // One angry shows its anger level to new viewers.
        assert_eq!(spawn_extras(1, &back).len(), 1);
        let mut empty = Mob::new(MobKind::Warden, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut empty, &Compound::new());
        assert!(state(&empty).suspects.is_empty());
        assert!(spawn_extras(1, &empty).is_empty());
    }

    #[test]
    fn walking_by_a_sensor_four_times_summons_a_warden() {
        let (mut w, alice) = world_with_player();
        w.players.get_mut(&alice).unwrap().pos = [0.5, -60.0, 0.5];
        w.set_block(BlockPos::new(3, -60, 0), block::SCULK_SENSOR);
        w.set_block(BlockPos::new(6, -60, 0), summoning_shrieker());
        let shrieking = |w: &World| {
            w.loaded_block(BlockPos::new(6, -60, 0))
                .and_then(|s| crate::blocks::prop(s, "shrieking"))
                == Some("true")
        };
        let mut shrieks = 0;
        let mut was = false;
        for i in 0..1200 {
            jiggle(&mut w, alice, i);
            tick_world(&mut w);
            w.tick += 1;
            let now = shrieking(&w);
            if now && !was {
                shrieks += 1;
            }
            was = now;
            if wardens_in(&w) > 0 {
                break;
            }
        }
        assert_eq!(shrieks, 4, "four shrieks, ten seconds apart");
        assert_eq!(wardens_in(&w), 1, "the fourth summons a warden");
        let summoned = w.mobs.values().find(|m| m.kind == MobKind::Warden).unwrap();
        assert!(matches!(state(summoned).phase, Phase::Emerging(_)));
        assert!(distance(summoned.pos, centre(BlockPos::new(6, -60, 0))) <= 9.0);
        let p = &w.players[&alice];
        assert!(p.effect(levitas_registry::mob_effect::DARKNESS).is_some());
        let s = state_ref(&w).unwrap();
        assert_eq!(s.warnings[&11].level, MAX_WARNING);
        // No second warden while one is close.
        assert!(w.summon_warden(BlockPos::new(6, -60, 0)));
        assert_eq!(wardens_in(&w), 1);
    }

    #[test]
    fn a_shrieker_saved_mid_shriek_is_not_stuck() {
        let (mut w, alice) = world_with_player();
        w.players.get_mut(&alice).unwrap().pos = [0.5, -60.0, 0.5];
        let pos = BlockPos::new(6, -60, 0);
        let stuck = crate::blocks::with_bool(summoning_shrieker(), "shrieking", true);
        w.set_block(BlockPos::new(3, -60, 0), block::SCULK_SENSOR);
        w.set_block(pos, stuck);
        assert!(ticks_randomly(stuck));
        assert!(!ticks_randomly(summoning_shrieker()));
        // Set off as if idle: it shrieks and warns.
        for i in 0..20 {
            jiggle(&mut w, alice, i);
            tick_world(&mut w);
            w.tick += 1;
        }
        let s = state_ref(&w).unwrap();
        assert_eq!(s.shrieks.len(), 1);
        assert_eq!(s.warnings[&11].level, 1);
        // A random tick leaves a running shriek alone, and quiets a stuck one.
        let state = w.block(pos);
        assert!(block_tick(&mut w, pos, state));
        assert!(shrieking(w.block(pos)));
        let other = BlockPos::new(-6, -60, 0);
        w.set_block(other, stuck);
        assert!(block_tick(&mut w, other, stuck));
        assert!(!shrieking(w.block(other)));
        let state = w.block(other);
        assert!(!block_tick(&mut w, other, state));
    }

    #[test]
    fn sneaking_sets_off_no_sensor_but_standing_on_a_shrieker_does() {
        let (mut w, alice) = world_with_player();
        w.players.get_mut(&alice).unwrap().pos = [0.5, -60.0, 0.5];
        w.players.get_mut(&alice).unwrap().sneaking = true;
        w.set_block(BlockPos::new(3, -60, 0), block::SCULK_SENSOR);
        w.set_block(BlockPos::new(6, -60, 0), summoning_shrieker());
        for i in 0..60 {
            jiggle(&mut w, alice, i);
            tick_world(&mut w);
            w.tick += 1;
        }
        assert!(state_ref(&w).unwrap().shrieks.is_empty());
        assert!(state_ref(&w).unwrap().warnings.is_empty());
        // Standing on it, sneaking or not.
        w.players.get_mut(&alice).unwrap().pos = [6.5, -59.5, 0.5];
        for _ in 0..20 {
            tick_world(&mut w);
            w.tick += 1;
        }
        let s = state_ref(&w).unwrap();
        assert_eq!(s.shrieks.len(), 1);
        assert_eq!(s.warnings[&11].level, 1);
    }

    #[test]
    fn warnings_cool_down_and_fade() {
        let (mut w, _) = world_with_player();
        let mut s = Wardens::default();
        s.warnings.insert(
            5,
            Warning {
                level: 2,
                cooldown: SHRIEK_COOLDOWN,
                quiet: 0,
            },
        );
        for _ in 0..WARNING_DECAY {
            w.tick_warnings(&mut s);
        }
        assert_eq!(s.warnings[&5].level, 1);
        assert_eq!(s.warnings[&5].cooldown, 0);
        for _ in 0..WARNING_DECAY {
            w.tick_warnings(&mut s);
        }
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn vibrations_anger_a_warden_until_it_roars_and_strikes() {
        let (mut w, alice) = world_with_player();
        w.players.get_mut(&alice).unwrap().pos = [2.3, -60.0, 0.5];
        let (id, mut mob) = warden(&mut w, [0.5, -60.0, 0.5], Phase::Idle);
        tick_world(&mut w); // note where players are
        let full = w.players[&alice].health;
        let mut roared = false;
        for i in 0..800 {
            jiggle(&mut w, alice, i);
            step(&mut w, id, &mut mob);
            roared |= matches!(state(&mob).phase, Phase::Roaring(_));
            if w.players[&alice].health < full {
                break;
            }
        }
        assert!(roared, "roared before the hunt");
        assert!(w.players[&alice].health < full, "struck");
        assert_eq!(state(&mob).target, Some(Who::Player(alice)));
        assert!(state(&mob).anger_at(Who::Player(alice)) >= ANGRY);
    }

    #[test]
    fn creative_and_sneaking_players_go_unheard() {
        let (mut w, alice) = world_with_player();
        w.players.get_mut(&alice).unwrap().pos = [8.5, -60.0, 0.5];
        w.players.get_mut(&alice).unwrap().sneaking = true;
        let (id, mut mob) = warden(&mut w, [0.5, -60.0, 0.5], Phase::Idle);
        state_mut(&mut mob).sniff_cooldown = 10_000;
        tick_world(&mut w);
        for i in 0..100 {
            jiggle(&mut w, alice, i);
            step(&mut w, id, &mut mob);
        }
        assert!(state(&mob).suspects.is_empty());
        let p = w.players.get_mut(&alice).unwrap();
        p.sneaking = false;
        p.mode = GAME_MODE_CREATIVE;
        for i in 0..100 {
            jiggle(&mut w, alice, i);
            step(&mut w, id, &mut mob);
        }
        assert!(state(&mob).suspects.is_empty());
        w.players.get_mut(&alice).unwrap().mode = GAME_MODE_SURVIVAL;
        for i in 0..3 {
            jiggle(&mut w, alice, i);
            step(&mut w, id, &mut mob);
        }
        assert_eq!(state(&mob).anger_at(Who::Player(alice)), VIBRATION_ANGER);
        assert!(state(&mob).investigate.is_some());
    }

    #[test]
    fn a_sonic_boom_goes_through_walls() {
        let (mut w, alice) = world_with_player();
        w.players.get_mut(&alice).unwrap().pos = [10.5, -60.0, 0.5];
        for y in -60..=-57 {
            for z in -3..=3 {
                w.set_block(BlockPos::new(5, y, z), block::STONE);
            }
        }
        let (id, mut mob) = warden(&mut w, [0.5, -60.0, 0.5], Phase::Idle);
        let wd = state_mut(&mut mob);
        wd.add_anger(Who::Player(alice), 11, MAX_ANGER);
        wd.target = Some(Who::Player(alice));
        wd.chase_ticks = BOOM_CHASE;
        tick_world(&mut w);
        let full = w.players[&alice].health;
        let mut charged = false;
        for _ in 0..80 {
            step(&mut w, id, &mut mob);
            charged |= matches!(state(&mob).phase, Phase::Charging(_));
            if w.players[&alice].health < full {
                break;
            }
        }
        assert!(charged);
        let hurt = full - w.players[&alice].health;
        let expected = by_difficulty(BOOM_DAMAGE, Who::Player(alice));
        assert!((hurt - expected).abs() < 1e-3, "{hurt} vs {expected}");
        assert_eq!(state(&mob).attack_cooldown, BOOM_TOTAL - BOOM_CHARGE);
        assert!(!in_boom_range([0.0; 3], [15.0, 0.0, 0.0]));
        assert!(in_boom_range([0.0; 3], [0.0, 19.0, 0.0]));
    }

    #[test]
    fn a_calm_warden_digs_down_and_is_gone() {
        let (mut w, _) = world_with_player();
        let (id, mut mob) = warden(&mut w, [0.5, -60.0, 0.5], Phase::Idle);
        state_mut(&mut mob).calm = CALM_TICKS - 1;
        assert!(step(&mut w, id, &mut mob));
        assert!(matches!(state(&mob).phase, Phase::Digging(_)));
        assert_eq!(spawn_extras(id, &mob).len(), 1, "shown digging");
        for _ in 0..DIG_TICKS - 1 {
            step(&mut w, id, &mut mob);
        }
        // Its last tick, from the mob table as the server runs it.
        tick(&mut w, id, &mut mob);
        w.mobs.insert(id, mob);
        tick_world(&mut w);
        assert!(!w.mobs.contains_key(&id), "dug down");
        // A named one stays.
        let (id, mut named) = warden(&mut w, [0.5, -60.0, 0.5], Phase::Idle);
        named.name = Some("Bob");
        state_mut(&mut named).calm = CALM_TICKS + 5;
        assert!(!step(&mut w, id, &mut named));
        assert_eq!(state(&named).phase, Phase::Idle);
    }

    #[test]
    fn wardens_spread_darkness_every_six_seconds() {
        let (mut w, alice) = world_with_player();
        w.players.get_mut(&alice).unwrap().pos = [15.5, -60.0, 0.5];
        let bob = add_player(&mut w, "Bob", 12, [0.5, -60.0, 25.5]);
        let (id, mut mob) = warden(&mut w, [0.5, -60.0, 0.5], Phase::Idle);
        state_mut(&mut mob).sniff_cooldown = 10_000;
        for _ in 0..DARKNESS_EVERY {
            step(&mut w, id, &mut mob);
        }
        let dark = |w: &World, p: i32| {
            w.players[&p]
                .effect(levitas_registry::mob_effect::DARKNESS)
                .is_some()
        };
        assert!(dark(&w, alice));
        assert!(!dark(&w, bob));
    }

    #[test]
    fn a_warden_emerges_before_doing_anything() {
        let (mut w, alice) = world_with_player();
        w.players.get_mut(&alice).unwrap().pos = [2.0, -60.0, 0.5];
        let (id, mut mob) = warden(&mut w, [0.5, -60.0, 0.5], Phase::Emerging(EMERGE_TICKS));
        assert_eq!(spawn_extras(id, &mob).len(), 1, "shown emerging");
        tick_world(&mut w);
        for i in 0..EMERGE_TICKS - 1 {
            jiggle(&mut w, alice, i);
            assert!(step(&mut w, id, &mut mob));
        }
        assert!(state(&mob).suspects.is_empty(), "deaf while emerging");
        step(&mut w, id, &mut mob);
        assert_eq!(state(&mob).phase, Phase::Idle);
    }
}
