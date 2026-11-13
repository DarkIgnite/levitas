//! Happy ghasts, ghastlings and dried ghast blocks.
//!
//! Behaviour follows the Minecraft Wiki (Happy Ghast, Dried Ghast, Harness, Golden
//! Dandelion):
//! - A dried ghast block (`hydration` 0 to 3) that is waterlogged hydrates one stage
//!   at a time; out of water it dries back one stage at a time, at the same rate. A
//!   stage takes a 5000 tick delay, which only starts when the block gets a random
//!   tick (a scheduled tick, so about 5.3 minutes a stage on average). A fully
//!   hydrated block still in water becomes a ghastling (a baby happy ghast) facing the
//!   way the block faced, and leaves its water behind.
//! - Ghastlings grow up in 20 minutes; each snowball fed to one takes a tenth off the
//!   time left. A golden dandelion resets and locks its growth, a second one unlocks
//!   it (`AgeLocked`). Now and then a ghastling follows a player, or an adult mob of
//!   `#minecraft:followable_friendly_mobs`, within 16 blocks, a little faster than it
//!   wanders, stopping 5 blocks away. Ghastlings do not drown. Happy ghasts never
//!   breed and do not panic when hurt; they take no fall damage. A ghastling is 0.95
//!   blocks wide and tall, an adult 4.
//! - Happy ghasts and ghastlings follow players holding a snowball, or a harness when
//!   they wear none (`#minecraft:happy_ghast_tempt_items`). They roam within 64 blocks
//!   of their home (32 when harnessed, or a ghastling); the home moves to where they
//!   are when a harness goes on or comes off, a rider gets off, or a temptation ends.
//! - Any harness (`#minecraft:harnesses`) used on an adult is worn in the body slot
//!   and saved in `equipment.body` with all its components; shears take it off again
//!   when nobody rides it. A harnessed adult carries up to four players: the first one
//!   steers, flying where they look at about 3.6 blocks a second. Getting off puts a
//!   player on top of it.
//! - While a player stands on top of an adult (or less than 2 blocks above it) and is
//!   not riding it, it stays still, facing the nearest cardinal direction: the
//!   `still_timeout` is set to 10 and counts down once nobody is there.
//! - It heals 1 health every 30 seconds, or every second between y 187 and 196 in the
//!   Overworld (cloud level), or out in rain or snow under open sky while not touching
//!   water. It drops its harness when it dies.

use levitas_nbt::{Compound, Tag};

use super::breeding::BABY_AGE;
use super::mobs::{yaw_towards, Mob, MobKind};
use super::weather::Precipitation;
use super::*;
use crate::blocks::{prop, with};

const DRIED_GHAST: &str = "minecraft:dried_ghast";
const SNOWBALL: &str = "minecraft:snowball";
const SHEARS: &str = "minecraft:shears";
const GOLDEN_DANDELION: &str = "minecraft:golden_dandelion";
const HARNESSES: &str = "minecraft:harnesses";
const TEMPT_ITEMS: &str = "minecraft:happy_ghast_tempt_items";
const FOLLOWABLE: &str = "minecraft:followable_friendly_mobs";

/// Entity data: Ageable Mob's age lock, then the happy ghast's own two booleans (Ageable
/// Mob takes 16 and 17, Animal adds nothing).
const DATA_AGE_LOCKED: u8 = 17;
#[allow(dead_code)] // Leads are not handled yet; the default (false) is what clients assume.
const DATA_LEASH_HOLDER: u8 = 18;
const DATA_STAYS_STILL: u8 = 19;
const SERIALIZER_BOOL: i32 = 8;
/// Equipment slot of the body (where a harness is worn).
const SLOT_BODY: u8 = 6;

/// Ticks for one stage of hydrating or drying, once a random tick started it.
const HYDRATION_DELAY: u64 = 5000;
const MAX_HYDRATION: u8 = 3;

/// A ghastling's hitbox width and height, in blocks (an adult's is 4).
const GHASTLING_SIZE: f64 = 0.95;

/// How many players fit on a happy ghast.
const MAX_RIDERS: usize = 4;
/// A rider's speed, in blocks per tick (about 3.6 blocks a second).
const RIDE_SPEED: f64 = 0.18;
/// How long a client's own move of the happy ghast keeps the server from steering it.
const CLIENT_STEER_TICKS: u64 = 10;
/// The farthest the pilot's client may move it in one packet.
const MAX_MOVE: f64 = 10.0;
/// Wandering and following speeds in blocks per tick (the wiki gives a 0.05 speed;
/// ghastlings follow at 1.1 times that).
const WANDER_SPEED: f64 = 0.05;
const FOLLOW_SPEED: f64 = 0.055;
const TEMPT_SPEED: f64 = 0.08;
/// How far a temptation or something to follow is noticed, and how close they come.
const TEMPT_RANGE: f64 = 16.0;
const FOLLOW_RANGE: f64 = 16.0;
const FOLLOW_STOP: f64 = 5.0;
/// A ghastling starts following something about once in this many ticks, for this long.
const FOLLOW_ONE_IN: u32 = 200;
const FOLLOW_TICKS: (u32, u32) = (200, 400);
/// How far from home it roams: unharnessed adults, and harnessed ones and ghastlings.
const HOME_RADIUS: i32 = 64;
const HOME_RADIUS_CLOSE: i32 = 32;
/// How far one wandering hop goes, sideways and up or down.
const WANDER_REACH: f64 = 8.0;
const WANDER_REACH_UP: f64 = 4.0;
/// A new wandering goal about once in this many ticks.
const WANDER_ONE_IN: u32 = 60;
/// Ticks between heals, normally and in rain, snow or at cloud level.
const HEAL_SLOW: u32 = 600;
const HEAL_FAST: u32 = 20;
const CLOUD_LEVEL: (f64, f64) = (187.0, 196.0);
/// What `still_timeout` is set to while a player is on top.
const STILL_TICKS: i32 = 10;
/// How far above its top a player still makes it stay still.
const STILL_ABOVE: f64 = 2.0;
/// Players' eye height, for hovering at their face.
const PLAYER_EYES: f64 = 1.62;

/// Something a ghastling follows.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Followed {
    Player(i32),
    Mob(i32),
}

/// The players on a happy ghast's back, in seat order; the first one steers. A fixed
/// array, so that the per-tick riding code can copy it without allocating.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Riders {
    ids: [i32; MAX_RIDERS],
    len: usize,
}

impl Riders {
    fn as_slice(&self) -> &[i32] {
        &self.ids[..self.len]
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn is_full(&self) -> bool {
        self.len >= MAX_RIDERS
    }

    fn first(&self) -> Option<i32> {
        self.as_slice().first().copied()
    }

    fn contains(&self, id: i32) -> bool {
        self.as_slice().contains(&id)
    }

    fn push(&mut self, id: i32) {
        if !self.is_full() {
            self.ids[self.len] = id;
            self.len += 1;
        }
    }

    /// Keeps the riders for whom `keep` holds, in order.
    fn retain(&mut self, mut keep: impl FnMut(i32) -> bool) {
        let mut kept = Riders::default();
        for &r in self.as_slice() {
            if keep(r) {
                kept.push(r);
            }
        }
        *self = kept;
    }
}

/// What the module keeps for a happy ghast or ghastling, in `Mob::kin`.
#[derive(Debug, Clone)]
struct Ghast {
    /// The harness it wears, with all its components (empty: none).
    harness: Stack,
    /// Players on its back; the first one steers.
    riders: Riders,
    /// Where it roams around.
    home: [i32; 3],
    /// Ticks it still stays still (above 0 while a player stands on it), and whether
    /// viewers were told it stays still.
    still_timeout: i32,
    shown_still: bool,
    /// A ghastling's growth is stopped by a golden dandelion.
    age_locked: bool,
    /// Followed by a temptation last tick (its end moves home).
    tempted: bool,
    /// What a ghastling follows, and for how many more ticks.
    follow: Option<(Followed, u32)>,
    /// Ticks since it last healed.
    heal_ticks: u32,
    /// The tick of the pilot's client's last move of it (0: never).
    client_moved: u64,
    /// Equipment it was saved with besides the body slot, kept as it was.
    other_equipment: Compound,
}

impl Ghast {
    fn new(at: [f64; 3]) -> Ghast {
        Ghast {
            harness: Stack::EMPTY,
            riders: Riders::default(),
            home: block_of(at),
            still_timeout: 0,
            shown_still: false,
            age_locked: false,
            tempted: false,
            follow: None,
            heal_ticks: 0,
            client_moved: 0,
            other_equipment: Compound::new(),
        }
    }

    fn harnessed(&self) -> bool {
        !self.harness.is_empty()
    }

    fn set_home(&mut self, at: [f64; 3]) {
        self.home = block_of(at);
    }
}

fn block_of(at: [f64; 3]) -> [i32; 3] {
    at.map(|v| v.floor() as i32)
}

/// The module's state of a happy ghast, made on first use (spawn eggs, commands).
fn ghast(mob: &mut Mob) -> &mut Ghast {
    if !mob.kin.as_ref().is_some_and(|k| k.is::<Ghast>()) {
        mob.kin = Some(Box::new(Ghast::new(mob.pos)));
    }
    mob.kin
        .as_mut()
        .and_then(|k| k.downcast_mut::<Ghast>())
        .expect("happy ghast state")
}

fn ghast_ref(mob: &Mob) -> Option<&Ghast> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Ghast>())
}

/// Hitbox width and height: a ghastling is 0.95 blocks, not half an adult (the shared
/// `Mob::size` halves babies).
fn size(mob: &Mob) -> (f64, f64) {
    if mob.age < 0 {
        (GHASTLING_SIZE, GHASTLING_SIZE)
    } else {
        mob.kind.size()
    }
}

fn bool_data(id: i32, index: u8, value: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(index)
        .varint(SERIALIZER_BOOL)
        .bool(value)
        .u8(0xFF)
}

/// The harness it wears, or none (an empty stack).
fn harness_packet(id: i32, harness: Stack) -> Writer {
    let packet = Writer::packet(clientbound::SET_EQUIPMENT)
        .varint(id)
        .u8(SLOT_BODY);
    crate::inventory::write_stack(packet, harness)
}

/// Who rides it.
fn passengers(id: i32, riders: &[i32]) -> Writer {
    let mut packet = Writer::packet(clientbound::SET_PASSENGERS)
        .varint(id)
        .varint(riders.len() as i32);
    for &r in riders {
        packet = packet.varint(r);
    }
    packet
}

/// Where rider `seat` sits: the pilot at the front above its face, the others on the
/// remaining sides going clockwise.
fn seat(mob: &Mob, seat: usize) -> [f64; 3] {
    let (width, height) = size(mob);
    let yaw = f64::from(mob.yaw).to_radians() + std::f64::consts::FRAC_PI_2 * seat as f64;
    let r = width / 4.0;
    [
        mob.pos[0] - yaw.sin() * r,
        mob.pos[1] + height,
        mob.pos[2] + yaw.cos() * r,
    ]
}

/// The way a player looks, as a unit vector.
fn look(yaw: f32, pitch: f32) -> [f64; 3] {
    let (yaw, pitch) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ]
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// Where a mob's feet go to hover with its middle at a player's eyes.
fn hover_at(mob: &Mob, player: [f64; 3]) -> [f64; 3] {
    [
        player[0],
        player[1] + PLAYER_EYES - size(mob).1 / 2.0,
        player[2],
    ]
}

/// Where items it drops appear: its middle.
fn middle(mob: &Mob) -> [f64; 3] {
    [mob.pos[0], mob.pos[1] + size(mob).1 / 2.0, mob.pos[2]]
}

fn is_dried_ghast(state: u16) -> bool {
    crate::blocks::name(state) == DRIED_GHAST
}

fn hydration(state: u16) -> u8 {
    prop(state, "hydration")
        .and_then(|h| h.parse().ok())
        .unwrap_or(0)
}

fn waterlogged(state: u16) -> bool {
    prop(state, "waterlogged") == Some("true")
}

/// The yaw a dried ghast's face looks along.
fn facing_yaw(state: u16) -> f32 {
    match prop(state, "facing") {
        Some("south") => 0.0,
        Some("west") => 90.0,
        Some("east") => -90.0,
        _ => 180.0,
    }
}

pub(super) fn spawned(_w: &mut World, mob: &mut Mob, _biome: &str) {
    ghast(mob);
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if !mob.alive() {
        return false;
    }
    // It does not panic, and it moves itself.
    mob.panic = 0;
    mob.target = None;
    mob.path.clear();
    if ghast(mob).age_locked && mob.age < 0 {
        mob.age -= 1; // undoes the growing up of `tick_breeding`
    }
    sync_riders(w, id, mob);
    heal(w, mob);
    stay_still(w, id, mob);
    if !ghast(mob).riders.is_empty() {
        ridden(w, mob);
    } else if ghast(mob).still_timeout > 0 {
        mob.yaw = (mob.yaw / 90.0).round() * 90.0;
        mob.head_yaw = mob.yaw;
        mob.fly_goal = None;
    } else {
        roam(w, id, mob);
    }
    mob.set_on_ground(false);
    true
}

/// Whether rider `r` is no longer on happy ghast `id` (sneaking clears their seat).
fn got_off(w: &World, id: i32, r: i32) -> bool {
    w.players
        .get(&r)
        .is_none_or(|p| !p.alive || p.riding != Some(id))
}

/// Lets go of riders who got off, putting them on top.
fn sync_riders(w: &mut World, id: i32, mob: &mut Mob) {
    let before = ghast(mob).riders;
    if !before.as_slice().iter().any(|&r| got_off(w, id, r)) {
        return;
    }
    let at = mob.pos;
    let top = [mob.pos[0], mob.pos[1] + size(mob).1, mob.pos[2]];
    let st = ghast(mob);
    st.riders.retain(|r| !got_off(w, id, r));
    st.set_home(at);
    let left = st.riders;
    w.send_to_viewers(mob, passengers(id, left.as_slice()));
    for &r in before.as_slice() {
        if !left.contains(r)
            && w.players
                .get(&r)
                .is_some_and(|p| p.alive && p.riding.is_none())
        {
            w.teleport(r, top);
        }
    }
    if left.is_empty() {
        w.play_sound_at(
            "minecraft:entity.happy_ghast.harness_goggles_up",
            at,
            1.0,
            1.0,
            None,
        );
    }
}

/// Heals 1 now and then: much faster in rain or snow, or at cloud level.
fn heal(w: &mut World, mob: &mut Mob) {
    let max = mob.max_health();
    if mob.health >= max {
        ghast(mob).heal_ticks = 0;
        return;
    }
    let fast = heals_fast(w, mob);
    let st = ghast(mob);
    st.heal_ticks += 1;
    if st.heal_ticks >= if fast { HEAL_FAST } else { HEAL_SLOW } {
        st.heal_ticks = 0;
        mob.health = (mob.health + 1.0).min(max);
    }
}

fn heals_fast(w: &World, mob: &Mob) -> bool {
    let y = mob.pos[1];
    if w.dimension == Dimension::Overworld && (CLOUD_LEVEL.0..=CLOUD_LEVEL.1).contains(&y) {
        return true;
    }
    if !w.weather.is_raining() || w.in_water(mob.pos) {
        return false;
    }
    let above = BlockPos::new(
        mob.pos[0].floor() as i32,
        (mob.pos[1] + size(mob).1).floor() as i32,
        mob.pos[2].floor() as i32,
    );
    w.light_at(above).is_some_and(|(sky, _)| sky == 15)
        && w.precipitation_kind(above) != Precipitation::None
}

/// An adult with a player standing on it (who is not riding it) stays still.
fn stay_still(w: &mut World, id: i32, mob: &mut Mob) {
    let on_top = mob.age >= 0 && {
        let (width, height) = size(mob);
        let reach = width / 2.0 + 0.3;
        let top = mob.pos[1] + height;
        w.players.values().any(|p| {
            p.alive
                && p.riding.is_none()
                && (p.pos[0] - mob.pos[0]).abs() <= reach
                && (p.pos[2] - mob.pos[2]).abs() <= reach
                && p.pos[1] >= top - 0.5
                && p.pos[1] - top < STILL_ABOVE
        })
    };
    let st = ghast(mob);
    if on_top {
        st.still_timeout = STILL_TICKS;
    } else {
        st.still_timeout = (st.still_timeout - 1).max(0);
    }
    let still = st.still_timeout > 0;
    if still != st.shown_still {
        st.shown_still = still;
        w.send_to_viewers(mob, bool_data(id, DATA_STAYS_STILL, still));
    }
}

/// Ridden: the pilot steers (their client, or the server from their look and forward
/// key), and the riders go along.
fn ridden(w: &mut World, mob: &mut Mob) {
    let st = ghast(mob);
    let Some(pilot) = st.riders.first() else {
        return;
    };
    let client =
        st.client_moved != 0 && w.tick.saturating_sub(st.client_moved) <= CLIENT_STEER_TICKS;
    if !client {
        if let Some(p) = w.players.get(&pilot) {
            let (yaw, pitch, forward) = (p.yaw, p.pitch, p.forward);
            mob.yaw = yaw;
            mob.head_yaw = yaw;
            if forward {
                let by = look(yaw, pitch).map(|v| v * RIDE_SPEED);
                move_by(w, mob, by);
            }
        }
    }
    carry_riders(w, mob);
}

fn carry_riders(w: &mut World, mob: &mut Mob) {
    let riders = ghast(mob).riders;
    for (i, &r) in riders.as_slice().iter().enumerate() {
        let at = seat(mob, i);
        w.move_player(r, Some(at), None, true);
    }
}

/// Moves it by `by`, stopping at blocks. Returns whether something was in the way.
fn move_by(w: &World, mob: &mut Mob, by: [f64; 3]) -> bool {
    let (width, height) = size(mob);
    let half = width / 2.0;
    let mut pos = mob.pos;
    let mut blocked = false;
    for axis in 0..3 {
        let push = match axis {
            0 => mob.knockback[0],
            2 => mob.knockback[1],
            _ => 0.0,
        };
        let want = by[axis] + push;
        let delta = w.clip_move(pos, half, height, axis, want);
        if (delta - want).abs() > 1e-9 {
            blocked = true;
        }
        pos[axis] += delta;
    }
    for k in &mut mob.knockback {
        *k *= 0.91;
    }
    mob.pos = pos;
    blocked
}

/// Flies towards `goal` at `speed`, stopping `stop` short of it. Returns whether
/// something was in the way.
fn fly_towards(w: &World, mob: &mut Mob, goal: [f64; 3], speed: f64, stop: f64) -> bool {
    let d = [0, 1, 2].map(|i| goal[i] - mob.pos[i]);
    let length = distance(goal, mob.pos);
    if length <= stop.max(0.5) {
        return move_by(w, mob, [0.0; 3]);
    }
    let step = speed.min(length - stop);
    if d[0].hypot(d[2]) > 0.1 {
        mob.yaw = yaw_towards(mob.pos, goal);
    }
    mob.head_yaw = mob.yaw;
    move_by(w, mob, d.map(|v| v / length * step))
}

/// The nearest player within reach holding something it wants, if any.
fn tempting_player(w: &World, mob: &Mob, harnessed: bool) -> Option<[f64; 3]> {
    w.players
        .values()
        .filter(|p| p.alive)
        .filter(|p| {
            [p.hand_slot(0), p.hand_slot(1)].iter().any(|&slot| {
                ItemInfo::get(p.inventory.slots[slot].item).is_some_and(|i| {
                    levitas_registry::item_has_tag(i.name, TEMPT_ITEMS)
                        && !(harnessed && levitas_registry::item_has_tag(i.name, HARNESSES))
                })
            })
        })
        .map(|p| p.pos)
        .filter(|&pos| distance(pos, mob.pos) <= TEMPT_RANGE)
        .min_by(|a, b| distance(*a, mob.pos).total_cmp(&distance(*b, mob.pos)))
}

/// Following a temptation, a ghastling following a friend, or wandering about home.
fn roam(w: &mut World, id: i32, mob: &mut Mob) {
    let harnessed = ghast(mob).harnessed();
    if let Some(player) = tempting_player(w, mob, harnessed) {
        ghast(mob).tempted = true;
        let goal = hover_at(mob, player);
        fly_towards(w, mob, goal, TEMPT_SPEED, FOLLOW_STOP);
        return;
    }
    if std::mem::take(&mut ghast(mob).tempted) {
        let at = mob.pos;
        ghast(mob).set_home(at);
    }
    if mob.age < 0 && follow_friend(w, id, mob) {
        return;
    }
    wander(w, mob);
}

/// A ghastling following a player or a friendly mob now and then. Returns whether it
/// follows something this tick.
fn follow_friend(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if ghast(mob).follow.is_none() && w.rng.below(FOLLOW_ONE_IN) == 0 {
        let ticks = FOLLOW_TICKS.0 + w.rng.below(FOLLOW_TICKS.1 - FOLLOW_TICKS.0 + 1);
        let friend = pick_friend(w, id, mob);
        ghast(mob).follow = friend.map(|f| (f, ticks));
    }
    let Some((followed, ticks)) = ghast(mob).follow else {
        return false;
    };
    let at = match followed {
        Followed::Player(p) => w
            .players
            .get(&p)
            .filter(|p| p.alive)
            .map(|p| hover_at(mob, p.pos)),
        Followed::Mob(m) => w.mobs.get(&m).filter(|m| m.alive()).map(|m| m.pos),
    };
    let goal = at.filter(|&g| distance(g, mob.pos) <= FOLLOW_RANGE * 1.5);
    match goal {
        Some(goal) if ticks > 0 => {
            ghast(mob).follow = Some((followed, ticks - 1));
            fly_towards(w, mob, goal, FOLLOW_SPEED, FOLLOW_STOP);
            true
        }
        _ => {
            let here = mob.pos;
            let st = ghast(mob);
            st.follow = None;
            st.set_home(here);
            false
        }
    }
}

/// The nearest player within range, or else the nearest adult friendly mob.
fn pick_friend(w: &World, id: i32, mob: &Mob) -> Option<Followed> {
    let near = |pos: [f64; 3]| distance(pos, mob.pos);
    let player = w
        .players
        .iter()
        .filter(|(_, p)| p.alive && near(p.pos) <= FOLLOW_RANGE)
        .min_by(|a, b| near(a.1.pos).total_cmp(&near(b.1.pos)))
        .map(|(&pid, _)| Followed::Player(pid));
    if player.is_some() {
        return player;
    }
    w.mobs
        .iter()
        .filter(|&(&mid, m)| {
            mid != id
                && m.alive()
                && m.age >= 0
                && near(m.pos) <= FOLLOW_RANGE
                && w.mob_tagged(mid, FOLLOWABLE)
        })
        .min_by(|a, b| near(a.1.pos).total_cmp(&near(b.1.pos)))
        .map(|(&mid, _)| Followed::Mob(mid))
}

/// How far from home it may roam.
fn home_radius(st: &Ghast, baby: bool) -> i32 {
    if st.harnessed() || baby {
        HOME_RADIUS_CLOSE
    } else {
        HOME_RADIUS
    }
}

/// Drifts towards a goal now and then, staying near home.
fn wander(w: &mut World, mob: &mut Mob) {
    let reached = mob.fly_goal.is_some_and(|g| distance(g, mob.pos) < 1.0);
    if reached {
        mob.fly_goal = None;
    }
    if mob.fly_goal.is_none() && w.rng.below(WANDER_ONE_IN) == 0 {
        let baby = mob.age < 0;
        let st = ghast(mob);
        let radius = home_radius(st, baby);
        let home = st.home.map(|v| f64::from(v) + 0.5);
        let r = |w: &mut World, reach: f64| (w.rng.f64() * 2.0 - 1.0) * reach;
        let mut goal = [
            mob.pos[0] + r(w, WANDER_REACH),
            mob.pos[1] + r(w, WANDER_REACH_UP),
            mob.pos[2] + r(w, WANDER_REACH),
        ];
        // The home restriction is a distance in all three directions, so that it does
        // not drift up to the sky or down to the ground either.
        if distance(goal, home) > f64::from(radius) {
            // Too far out: head back home instead.
            let d = [
                home[0] - mob.pos[0],
                home[1] - mob.pos[1],
                home[2] - mob.pos[2],
            ];
            let length = distance(home, mob.pos).max(1e-6);
            goal = [0, 1, 2].map(|i| mob.pos[i] + d[i] / length * WANDER_REACH);
        }
        mob.fly_goal = Some(goal);
    }
    match mob.fly_goal {
        Some(goal) => {
            if fly_towards(w, mob, goal, WANDER_SPEED, 0.0) {
                mob.fly_goal = None; // something in the way: pick another goal
            }
        }
        None => {
            move_by(w, mob, [0.0; 3]);
        }
    }
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    let Some(mut mob) = w.mobs.remove(&target) else {
        return false;
    };
    let took = use_on_ghast(w, player, target, &mut mob, slot, held);
    w.mobs.insert(target, mob);
    took
}

fn use_on_ghast(
    w: &mut World,
    player: i32,
    id: i32,
    mob: &mut Mob,
    slot: usize,
    held: &str,
) -> bool {
    if !mob.alive() {
        return false;
    }
    let survival = w.players.get(&player).is_some_and(|p| p.survival());
    let baby = mob.age < 0;
    if baby && held == SNOWBALL {
        mob.age += -mob.age / 10;
        w.play_sound_at(
            "minecraft:entity.ghastling.ambient",
            mob.pos,
            1.0,
            1.0,
            None,
        );
        if survival {
            use_up(w, player, slot);
        }
        return true;
    }
    if baby && held == GOLDEN_DANDELION {
        let st = ghast(mob);
        st.age_locked = !st.age_locked;
        let locked = st.age_locked;
        if locked {
            mob.age = BABY_AGE;
        }
        w.send_to_viewers(mob, bool_data(id, DATA_AGE_LOCKED, locked));
        if survival {
            use_up(w, player, slot);
        }
        return true;
    }
    if baby {
        return false;
    }
    let (harness, riders) = {
        let st = ghast(mob);
        (st.harness, st.riders.len)
    };
    if harness.is_empty() && levitas_registry::item_has_tag(held, HARNESSES) {
        // The very stack held, with its name and other components.
        let worn = w
            .players
            .get(&player)
            .and_then(|p| p.inventory.slots.get(slot).copied())
            .filter(|s| s.is_named(held))
            .or_else(|| ItemInfo::id_of(held).map(|item| Stack::new(item, 1)))
            .map_or(Stack::EMPTY, |s| s.with_count(1));
        if worn.is_empty() {
            return false;
        }
        let at = mob.pos;
        let st = ghast(mob);
        st.harness = worn;
        st.set_home(at);
        w.send_to_viewers(mob, harness_packet(id, worn));
        w.play_sound_at("minecraft:entity.happy_ghast.equip", at, 1.0, 1.0, None);
        if survival {
            use_up(w, player, slot);
        }
        return true;
    }
    if harness.is_empty() {
        return false;
    }
    if held == SHEARS {
        if riders > 0 {
            return false;
        }
        let at = mob.pos;
        let st = ghast(mob);
        st.harness = Stack::EMPTY;
        st.set_home(at);
        w.send_to_viewers(mob, harness_packet(id, Stack::EMPTY));
        w.play_sound_at("minecraft:entity.happy_ghast.unequip", at, 1.0, 1.0, None);
        w.scatter_item(middle(mob), harness);
        if survival {
            w.damage_item(player, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
        }
        return true;
    }
    mount(w, player, id, mob);
    true
}

/// Takes one of the item in `slot`.
fn use_up(w: &mut World, player: i32, slot: usize) {
    let Some(p) = w.players.get_mut(&player) else {
        return;
    };
    p.inventory.take(slot, 1);
    let update = p.inventory.set_slot_packet(slot);
    w.send(player, update);
}

/// Puts a player on a harnessed happy ghast, if there is a seat.
fn mount(w: &mut World, player: i32, id: i32, mob: &mut Mob) {
    let Some(p) = w.players.get(&player) else {
        return;
    };
    if !p.alive || p.sneaking || p.riding.is_some() {
        return;
    }
    let st = ghast(mob);
    if st.riders.is_full() || st.riders.contains(player) {
        return;
    }
    st.riders.push(player);
    let riders = st.riders;
    if riders.len == 1 {
        mob.fly_goal = None;
        w.play_sound_at(
            "minecraft:entity.happy_ghast.harness_goggles_down",
            mob.pos,
            1.0,
            1.0,
            None,
        );
    }
    w.players.get_mut(&player).expect("checked").riding = Some(id);
    w.broadcast(passengers(id, riders.as_slice()), None);
    let at = seat(mob, riders.len - 1);
    w.move_player(player, Some(at), None, true);
}

/// Where the pilot's client moved the happy ghast they steer (a Move Vehicle packet).
/// Returns whether `target` is a happy ghast (the move was handled here).
pub(super) fn ghast_moved(
    w: &mut World,
    player: i32,
    target: i32,
    pos: [f64; 3],
    yaw: f32,
) -> bool {
    let Some(mut mob) = w.mobs.remove(&target) else {
        return false;
    };
    if mob.kind != MobKind::HappyGhast {
        w.mobs.insert(target, mob);
        return false;
    }
    if ghast(&mut mob).riders.first() == Some(player) {
        let d = distance(pos, mob.pos);
        if d > MAX_MOVE || pos.iter().any(|v| !v.is_finite()) {
            let back = Writer::packet(clientbound::MOVE_VEHICLE)
                .f64(mob.pos[0])
                .f64(mob.pos[1])
                .f64(mob.pos[2])
                .f32(mob.yaw)
                .f32(0.0);
            w.send(player, back);
        } else {
            mob.pos = pos;
            mob.yaw = yaw;
            mob.head_yaw = yaw;
            ghast(&mut mob).client_moved = w.tick.max(1);
            carry_riders(w, &mut mob);
        }
    }
    w.mobs.insert(target, mob);
    true
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, _attacker: Option<i32>) {
    if mob.health > 0.0 {
        return;
    }
    // Dying: riders fall off, and the harness drops.
    let st = ghast(mob);
    let riders = std::mem::take(&mut st.riders);
    let harness = std::mem::replace(&mut st.harness, Stack::EMPTY);
    for &r in riders.as_slice() {
        if let Some(p) = w.players.get_mut(&r) {
            if p.riding == Some(id) {
                p.riding = None;
            }
        }
    }
    if !riders.is_empty() {
        w.send_to_viewers(mob, passengers(id, &[]));
    }
    if !harness.is_empty() {
        w.scatter_item(middle(mob), harness);
    }
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let Some(st) = ghast_ref(mob) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if st.harnessed() {
        out.push(harness_packet(id, st.harness));
    }
    if st.age_locked {
        out.push(bool_data(id, DATA_AGE_LOCKED, true));
    }
    if st.still_timeout > 0 {
        out.push(bool_data(id, DATA_STAYS_STILL, true));
    }
    if !st.riders.is_empty() {
        out.push(passengers(id, st.riders.as_slice()));
    }
    out
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let fresh;
    let st = match ghast_ref(mob) {
        Some(st) => st,
        None => {
            fresh = Ghast::new(mob.pos);
            &fresh
        }
    };
    nbt.insert("still_timeout", st.still_timeout);
    nbt.insert("AgeLocked", st.age_locked);
    nbt.insert("home_pos", st.home.to_vec());
    nbt.insert("home_radius", home_radius(st, mob.age < 0));
    // Always written, so that a harness taken off is not brought back from what the
    // mob was loaded with.
    let mut equipment = st.other_equipment.clone();
    if st.harnessed() {
        equipment.insert("body", st.harness.to_nbt());
    }
    nbt.insert("equipment", equipment);
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let st = ghast(mob);
    st.still_timeout = nbt.int("still_timeout").unwrap_or(0).clamp(0, 1000) as i32;
    st.age_locked = nbt.int("AgeLocked").unwrap_or(0) != 0;
    if let Some(&[x, y, z]) = nbt.int_array("home_pos") {
        st.home = [x, y, z];
    }
    let mut equipment = nbt
        .get("equipment")
        .and_then(Tag::as_compound)
        .cloned()
        .unwrap_or_default();
    let body = equipment.remove("body");
    st.harness = body
        .as_ref()
        .and_then(Tag::as_compound)
        .map(Stack::from_nbt)
        .filter(|s| {
            ItemInfo::get(s.item).is_some_and(|i| levitas_registry::item_has_tag(i.name, HARNESSES))
        })
        .map_or(Stack::EMPTY, |s| s.with_count(1));
    if !st.harnessed() {
        // Something else worn on the body is kept as it was.
        if let Some(body) = body {
            equipment.insert("body", body);
        }
    }
    st.other_equipment = equipment;
}

pub(super) fn damage_taken(
    _w: &mut World,
    _id: i32,
    mob: &mut Mob,
    damage: f32,
    damage_type: &str,
    _attacker: Option<i32>,
) -> f32 {
    match damage_type {
        // `#minecraft:fall_damage_immune`; ghastlings do not drown.
        "minecraft:fall" => 0.0,
        "minecraft:drown" if mob.age < 0 => 0.0,
        _ => damage,
    }
}

/// Happy ghasts never breed.
pub(super) fn may_love(_w: &World, _mob: &Mob) -> bool {
    false
}

pub(super) fn bred(_w: &mut World, _id: i32, _mob: &mut Mob) -> bool {
    false
}

pub(super) fn tick_world(_w: &mut World) {}

pub(super) fn block_placed(w: &mut World, _player: i32, pos: BlockPos, state: u16) {
    if is_dried_ghast(state) && waterlogged(state) {
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        w.play_sound_at(
            "minecraft:block.dried_ghast.place_in_water",
            at,
            1.0,
            1.0,
            None,
        );
    }
}

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

/// A random tick of a dried ghast starts the delay to its next stage, if it has one
/// to go to (wetter in water, drier out of it).
pub(super) fn block_tick(w: &mut World, pos: BlockPos, state: u16) -> bool {
    if !is_dried_ghast(state) {
        return false;
    }
    let changes = waterlogged(state) || hydration(state) > 0;
    if changes && !w.is_scheduled(pos, state) {
        w.schedule(pos, state, HYDRATION_DELAY);
    }
    true
}

pub(super) fn ticks_randomly(state: u16) -> bool {
    is_dried_ghast(state)
}

/// The delay of a dried ghast ran out: one stage wetter in water (a ghastling after the
/// last), one stage drier out of it.
pub(super) fn scheduled_tick(w: &mut World, pos: BlockPos, state: u16) -> bool {
    if !is_dried_ghast(state) {
        return false;
    }
    let level = hydration(state);
    let at = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ];
    if waterlogged(state) {
        if level >= MAX_HYDRATION {
            become_ghastling(w, pos, state);
            return true;
        }
        let wetter = with(state, "hydration", &(level + 1).to_string());
        w.set_block_updating(pos, wetter);
    } else if level > 0 {
        let drier = with(state, "hydration", &(level - 1).to_string());
        w.set_block_updating(pos, drier);
    } else {
        return true;
    }
    // The wiki's "state change" sound; its 26.3 sound event is `transition`.
    w.play_sound_at("minecraft:block.dried_ghast.transition", at, 1.0, 1.0, None);
    true
}

/// A fully hydrated dried ghast turns into a ghastling, leaving its water.
fn become_ghastling(w: &mut World, pos: BlockPos, state: u16) {
    w.set_block_updating(pos, block::WATER);
    let at = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ];
    let mut baby = Mob::new(MobKind::HappyGhast, at, &mut w.rng);
    baby.age = BABY_AGE;
    baby.yaw = facing_yaw(state);
    baby.head_yaw = baby.yaw;
    baby.kin = Some(Box::new(Ghast::new(at)));
    w.mobs.insert(next_entity_id(), baby);
    w.play_sound_at("minecraft:entity.ghastling.spawn", at, 1.0, 1.0, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-happy-ghasts-{}-{}",
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

    fn hold(w: &mut World, id: i32, item: &str) {
        w.players.get_mut(&id).unwrap().inventory.slots[36] =
            Stack::new(ItemInfo::id_of(item).unwrap(), 1);
    }

    fn held_count(w: &World, id: i32) -> i32 {
        w.players[&id].inventory.slots[36].count
    }

    fn adult(w: &mut World, at: [f64; 3]) -> i32 {
        let mut mob = Mob::new(MobKind::HappyGhast, at, &mut w.rng);
        mob.age = 0;
        ghast(&mut mob);
        let id = next_entity_id();
        w.mobs.insert(id, mob);
        id
    }

    fn run(w: &mut World, id: i32, ticks: u32) {
        for _ in 0..ticks {
            let mut m = w.mobs.remove(&id).unwrap();
            tick(w, id, &mut m);
            w.mobs.insert(id, m);
        }
    }

    fn state_of(w: &World, id: i32) -> &Ghast {
        ghast_ref(&w.mobs[&id]).expect("happy ghast state")
    }

    fn worn(w: &World, id: i32) -> Option<&'static str> {
        let st = state_of(w, id);
        st.harnessed()
            .then(|| ItemInfo::get(st.harness.item).map(|i| i.name))
            .flatten()
    }

    fn dried_ghast(hydration: u8, water: bool) -> u16 {
        let base = BlockInfo::by_name(DRIED_GHAST).unwrap().default_state;
        let state = with(base, "hydration", &hydration.to_string());
        crate::blocks::with_bool(state, "waterlogged", water)
    }

    #[test]
    fn a_dried_ghast_in_water_hydrates_into_a_ghastling() {
        let (mut w, _) = world_with_player();
        let pos = BlockPos::new(4, -60, 4);
        w.set_block(pos, with(dried_ghast(0, true), "facing", "south"));
        assert!(ticks_randomly(w.block(pos)));
        let before = w.mobs.len();
        for stage in 1..=3 {
            let state = w.block(pos);
            assert!(block_tick(&mut w, pos, state));
            assert!(w.is_scheduled(pos, state), "a random tick starts the delay");
            assert!(scheduled_tick(&mut w, pos, state));
            assert_eq!(hydration(w.block(pos)), stage);
        }
        let state = w.block(pos);
        assert!(scheduled_tick(&mut w, pos, state));
        assert_eq!(w.block(pos), block::WATER, "its water stays behind");
        assert_eq!(w.mobs.len(), before + 1);
        let baby = w
            .mobs
            .values()
            .find(|m| m.kind == MobKind::HappyGhast)
            .expect("a ghastling");
        assert_eq!(baby.age, BABY_AGE);
        assert_eq!(baby.yaw, 0.0, "faces the way the block faced");
        assert_eq!(size(baby), (GHASTLING_SIZE, GHASTLING_SIZE));
    }

    #[test]
    fn the_hydration_sound_exists() {
        assert!(levitas_registry::sound_id("minecraft:block.dried_ghast.transition").is_some());
    }

    #[test]
    fn a_dried_ghast_out_of_water_dries_back() {
        let (mut w, _) = world_with_player();
        let pos = BlockPos::new(4, -60, 4);
        w.set_block(pos, dried_ghast(2, false));
        let state = w.block(pos);
        assert!(block_tick(&mut w, pos, state));
        assert!(w.is_scheduled(pos, state));
        scheduled_tick(&mut w, pos, state);
        assert_eq!(hydration(w.block(pos)), 1);
        // Fully dry, nothing more to do.
        let dry = dried_ghast(0, false);
        let other = BlockPos::new(6, -60, 6);
        w.set_block(other, dry);
        block_tick(&mut w, other, dry);
        assert!(!w.is_scheduled(other, dry));
    }

    #[test]
    fn snowballs_speed_up_a_ghastling_and_golden_dandelions_lock_it() {
        let (mut w, alice) = world_with_player();
        let id = adult(&mut w, [0.5, -58.0, 0.5]);
        w.mobs.get_mut(&id).unwrap().age = BABY_AGE;
        hold(&mut w, alice, SNOWBALL);
        assert!(use_on(&mut w, alice, id, 36, SNOWBALL));
        assert_eq!(w.mobs[&id].age, BABY_AGE - BABY_AGE / 10);
        assert_eq!(held_count(&w, alice), 0, "the snowball is eaten");

        hold(&mut w, alice, GOLDEN_DANDELION);
        assert!(use_on(&mut w, alice, id, 36, GOLDEN_DANDELION));
        assert!(state_of(&w, id).age_locked);
        assert_eq!(w.mobs[&id].age, BABY_AGE, "growth starts over");
        // `tick_breeding` grows it a tick; a locked ghastling stays as it was.
        let mut m = w.mobs.remove(&id).unwrap();
        w.tick_breeding(id, &mut m);
        tick(&mut w, id, &mut m);
        assert_eq!(m.age, BABY_AGE);
        let nbt = m.to_nbt();
        assert_eq!(nbt.int("AgeLocked"), Some(1));
        let back = Mob::read(&nbt, &mut w.rng).unwrap();
        assert!(ghast_ref(&back).unwrap().age_locked);
        // Adults do not eat snowballs.
        m.age = 0;
        w.mobs.insert(id, m);
        hold(&mut w, alice, SNOWBALL);
        assert!(!use_on(&mut w, alice, id, 36, SNOWBALL));
    }

    #[test]
    fn a_harness_is_worn_ridden_by_four_and_sheared_off() {
        let (mut w, alice) = world_with_player();
        let id = adult(&mut w, [0.5, -58.0, 0.5]);
        // No harness: nobody gets on.
        assert!(!use_on(&mut w, alice, id, 36, "minecraft:air"));
        hold(&mut w, alice, "minecraft:lime_harness");
        w.players.get_mut(&alice).unwrap().inventory.slots[36].damage = 0;
        w.players.get_mut(&alice).unwrap().inventory.slots[36].repair_cost = 3;
        assert!(use_on(&mut w, alice, id, 36, "minecraft:lime_harness"));
        assert_eq!(worn(&w, id), Some("minecraft:lime_harness"));
        assert_eq!(state_of(&w, id).harness.repair_cost, 3, "components stay");
        assert_eq!(held_count(&w, alice), 0);
        let riders: Vec<i32> = (0..5)
            .map(|i| add_player(&mut w, &format!("P{i}"), 100 + i, [1.5, -60.0, 1.5]))
            .collect();
        for &r in &riders {
            use_on(&mut w, r, id, 36, "minecraft:air");
        }
        assert_eq!(state_of(&w, id).riders.as_slice(), &riders[..4]);
        assert_eq!(w.players[&riders[0]].riding, Some(id));
        assert!(w.players[&riders[4]].riding.is_none(), "only four fit");
        // Shears do nothing while it is ridden.
        hold(&mut w, alice, SHEARS);
        assert!(!use_on(&mut w, alice, id, 36, SHEARS));

        // Saved with its harness in the body slot, components and all.
        let nbt = w.mobs[&id].to_nbt();
        let body = nbt
            .get("equipment")
            .and_then(Tag::as_compound)
            .and_then(|e| e.get("body"))
            .and_then(Tag::as_compound)
            .and_then(|b| b.str("id"))
            .map(str::to_owned);
        assert_eq!(body.as_deref(), Some("minecraft:lime_harness"));
        let back = Mob::read(&nbt, &mut w.rng).unwrap();
        let back_harness = ghast_ref(&back).unwrap().harness;
        assert_eq!(back_harness, state_of(&w, id).harness);

        // Everyone gets off (sneaking clears their seat) and lands on top.
        for &r in &riders[..4] {
            w.players.get_mut(&r).unwrap().riding = None;
        }
        run(&mut w, id, 1);
        assert!(state_of(&w, id).riders.is_empty());
        let top = w.mobs[&id].pos[1] + size(&w.mobs[&id]).1;
        assert!((w.players[&riders[0]].pos[1] - top).abs() < 1e-9);
        let items = w.items.len();
        assert!(use_on(&mut w, alice, id, 36, SHEARS));
        assert!(!state_of(&w, id).harnessed());
        assert_eq!(w.items.len(), items + 1, "the harness drops");
        // Taken off, it stays off after saving and loading.
        let back = Mob::read(&w.mobs[&id].to_nbt(), &mut w.rng).unwrap();
        assert!(!ghast_ref(&back).unwrap().harnessed());
    }

    #[test]
    fn the_pilot_steers_where_they_look() {
        let (mut w, alice) = world_with_player();
        let id = adult(&mut w, [0.5, -50.0, 0.5]);
        hold(&mut w, alice, "minecraft:white_harness");
        use_on(&mut w, alice, id, 36, "minecraft:white_harness");
        let bob = add_player(&mut w, "Bob", 12, [1.5, -60.0, 1.5]);
        let carol = add_player(&mut w, "Carol", 13, [1.5, -60.0, 1.5]);
        use_on(&mut w, bob, id, 36, "minecraft:air");
        use_on(&mut w, carol, id, 36, "minecraft:air");
        {
            let p = w.players.get_mut(&bob).unwrap();
            p.yaw = 0.0;
            p.pitch = 0.0;
            p.forward = true;
        }
        let start = w.mobs[&id].pos;
        run(&mut w, id, 10);
        let pos = w.mobs[&id].pos;
        assert!(
            (pos[2] - start[2] - 10.0 * RIDE_SPEED).abs() < 1e-6,
            "{pos:?}"
        );
        assert!((pos[0] - start[0]).abs() < 1e-6 && (pos[1] - start[1]).abs() < 1e-6);
        // Riders go along.
        assert!(distance(w.players[&carol].pos, pos) < 5.0);
        // The pilot's client moves it; another rider's cannot.
        let to = [pos[0] + 3.0, pos[1], pos[2]];
        assert!(ghast_moved(&mut w, bob, id, to, 0.0));
        assert_eq!(w.mobs[&id].pos, to);
        assert!(ghast_moved(&mut w, carol, id, [0.0, 0.0, 0.0], 0.0));
        assert_eq!(w.mobs[&id].pos, to);
        // Too far at once is refused.
        ghast_moved(&mut w, bob, id, [to[0] + 50.0, to[1], to[2]], 0.0);
        assert_eq!(w.mobs[&id].pos, to);
    }

    #[test]
    fn it_stays_still_while_a_player_stands_on_it() {
        let (mut w, alice) = world_with_player();
        let id = adult(&mut w, [0.5, -50.0, 0.5]);
        w.mobs.get_mut(&id).unwrap().yaw = 100.0;
        w.players.get_mut(&alice).unwrap().pos = [1.0, -46.0, 0.0];
        let start = w.mobs[&id].pos;
        run(&mut w, id, 200);
        assert_eq!(w.mobs[&id].pos, start);
        assert_eq!(state_of(&w, id).still_timeout, STILL_TICKS);
        assert_eq!(w.mobs[&id].yaw, 90.0, "faces a cardinal direction");
        w.players.get_mut(&alice).unwrap().pos = [-28.5, -60.0, -28.5];
        run(&mut w, id, STILL_TICKS as u32);
        assert_eq!(state_of(&w, id).still_timeout, 0);
        assert!(!state_of(&w, id).shown_still);
    }

    #[test]
    fn happy_ghasts_follow_snowballs_and_stay_near_home() {
        let (mut w, alice) = world_with_player();
        let id = adult(&mut w, [0.5, -50.0, 0.5]);
        w.players.get_mut(&alice).unwrap().pos = [12.5, -60.0, 0.5];
        hold(&mut w, alice, SNOWBALL);
        run(&mut w, id, 100);
        let pos = w.mobs[&id].pos;
        assert!(pos[0] > 5.0, "came closer: {pos:?}");
        // Holding nothing ends it, and home moves to where it is.
        w.players.get_mut(&alice).unwrap().inventory.slots[36] = Stack::EMPTY;
        run(&mut w, id, 1);
        assert_eq!(state_of(&w, id).home, block_of(w.mobs[&id].pos));
        // Wandering keeps it within its home radius, up and down too.
        run(&mut w, id, 3000);
        let home = state_of(&w, id).home.map(f64::from);
        let pos = w.mobs[&id].pos;
        assert!(distance(pos, home) < f64::from(HOME_RADIUS) + 10.0);
    }

    #[test]
    fn wandering_does_not_drift_far_above_home() {
        let (mut w, _) = world_with_player();
        // Right over home but beyond its radius (80 blocks up): it heads back in.
        let id = adult(&mut w, [0.5, 40.0, 0.5]);
        {
            let m = w.mobs.get_mut(&id).unwrap();
            ghast(m).home = [0, -40, 0];
        }
        run(&mut w, id, 4000);
        let pos = w.mobs[&id].pos;
        assert!(
            pos[1] < -40.0 + f64::from(HOME_RADIUS) + 10.0,
            "came back: {pos:?}"
        );
    }

    #[test]
    fn it_heals_slowly_takes_no_fall_damage_and_drops_its_harness() {
        let (mut w, alice) = world_with_player();
        let id = adult(&mut w, [0.5, -50.0, 0.5]);
        w.mobs.get_mut(&id).unwrap().health = 10.0;
        run(&mut w, id, HEAL_SLOW);
        assert_eq!(w.mobs[&id].health, 11.0);
        let mut m = w.mobs.remove(&id).unwrap();
        assert_eq!(
            damage_taken(&mut w, id, &mut m, 5.0, "minecraft:fall", None),
            0.0
        );
        w.mobs.insert(id, m);
        hold(&mut w, alice, "minecraft:black_harness");
        use_on(&mut w, alice, id, 36, "minecraft:black_harness");
        let items = w.items.len();
        let mut m = w.mobs.remove(&id).unwrap();
        m.health = 0.0;
        hurt(&mut w, id, &mut m, Some(alice));
        assert_eq!(w.items.len(), items + 1);
        assert!(!ghast_ref(&m).unwrap().harnessed());
        assert!(!may_love(&w, &m));
    }
}
