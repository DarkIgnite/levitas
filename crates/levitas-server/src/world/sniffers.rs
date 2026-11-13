//! Sniffers: sniffing, digging up ancient seeds, and their eggs.
//!
//! Behaviour follows the Minecraft Wiki (Sniffer, Sniffer Egg):
//! - Now and then a sniffer smells the air (scenting). An adult that has not dug for a
//!   while sniffs long, picks a block of `minecraft:sniffer_diggable_block` nearby and
//!   searches its way there; with its head (2.25 blocks in front of its body) over the
//!   block it sploots and digs, and drops what the gameplay loot table
//!   `minecraft:sniffer_digging` gives (torchflower seeds or a pitcher pod, even odds).
//!   Then it rises, is happy for a moment, and does not search again for eight minutes.
//!   It remembers its last 20 dug blocks and never digs those again.
//! - It digs only on the ground, out of water, not in a boat or minecart, not
//!   panicking, in love or tempted by torchflower seeds; anything of that stops a dig
//!   under way. Snifflets never dig, but they scent.
//! - Two sniffers bred lay a sniffer egg, dropped as an item, not a snifflet.
//! - A placed sniffer egg cracks twice (block property `hatch` 0, 1, 2) and then hatches
//!   into a snifflet: 10 minutes in all on a block of `minecraft:sniffer_egg_hatch_boost`
//!   (moss), 20 minutes on anything else. It runs on scheduled block ticks, so
//!   `random_tick_speed` does not change it. Snifflets take 48000 ticks to grow up.
//!
//! The wiki gives no timings for the steps of a dig; the ones here are close to what the
//! animations show.
//!
//! Entity data (26.3, after Ageable Mob's baby 16 and age locked 17; Animal adds none):
//! sniffer state 18 (IDLING 0, FEELING_HAPPY 1, SCENTING 2, SNIFFING 3, SEARCHING 4,
//! DIGGING 5, RISING 6) and the game tick it drops its seed at 19.
//! Saved as vanilla: the dug blocks as the brain memory
//! `minecraft:sniffer_explored_positions` (a list of `{dimension, pos}`) and the wait
//! before the next search as `minecraft:sniff_cooldown` (`{value: {}, ttl}`). An egg keeps
//! its `hatch` in the block state and its timer in the chunk's scheduled block ticks.

use std::collections::VecDeque;

use levitas_nbt::{Compound, Tag};

use super::mobs::{yaw_towards, Mob, MobKind};
use super::*;

const DATA_STATE: u8 = 18;
const DATA_DROP_SEED_AT: u8 = 19;
const SERIALIZER_VARINT: i32 = 1;
const SERIALIZER_SNIFFER_STATE: i32 = 35;

const DIGGABLE: &str = "minecraft:sniffer_diggable_block";
const HATCH_BOOST: &str = "minecraft:sniffer_egg_hatch_boost";
const EGG: &str = "minecraft:sniffer_egg";
const LOOT: &str = "minecraft:sniffer_digging";

const MEMORY_EXPLORED: &str = "minecraft:sniffer_explored_positions";
const MEMORY_COOLDOWN: &str = "minecraft:sniff_cooldown";
const OVERWORLD: &str = "minecraft:overworld";

/// Ticks between searches after a dig: eight minutes.
const SEARCH_COOLDOWN: u32 = 9600;
/// The first wait of a sniffer that has just come into the world (or grown up).
const FIRST_WAIT: (u32, u32) = (600, 2400);
/// Ticks before trying again when there was nothing to dig near by.
const RETRY: u32 = 400;
/// How many dug blocks it remembers.
const EXPLORED: usize = 20;
/// Chance per tick of scenting while idle, and how long it lasts.
const SCENT_CHANCE: u32 = 500;
const SCENT_TICKS: u32 = 40;
/// The long sniff before a search.
const SNIFF_TICKS: u32 = 40;
/// The longest a search goes on before it gives up.
const SEARCH_TICKS: u32 = 400;
/// The searching sound plays this often while it walks.
const SEARCH_SOUND_EVERY: u32 = 40;
/// Splooting and digging, the seed coming up this many ticks in, rising, being happy.
const DIG_TICKS: u32 = 120;
const DROP_AFTER: u32 = 80;
const RISE_TICKS: u32 = 30;
const HAPPY_TICKS: u32 = 40;
/// How far around it looks for a block to dig.
const SEARCH_RANGE: i32 = 8;
/// Its head is this far in front of the middle of its body.
const HEAD_OFFSET: f64 = 2.25;
/// It digs once the block is this far from its body (its head then over it).
const DIG_REACH: (f64, f64) = (1.6, 2.9);
/// Ticks between the cracks of an egg, on moss and elsewhere (10 or 20 minutes in all).
const HATCH_STAGE_BOOSTED: u64 = 4000;
const HATCH_STAGE: u64 = 8000;
/// A snifflet grows up in two in-game days.
const SNIFFLET_AGE: i32 = -48_000;

/// The sniffer states of the entity data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idling = 0,
    FeelingHappy = 1,
    Scenting = 2,
    Sniffing = 3,
    Searching = 4,
    Digging = 5,
    Rising = 6,
}

/// What the module keeps for a sniffer, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq)]
struct Sniffer {
    phase: Phase,
    /// Ticks spent in the phase so far.
    ticks: u32,
    /// Ticks until it next searches, rolled on its first turn.
    cooldown: Option<u32>,
    /// The block it is searching for, then digging.
    goal: Option<BlockPos>,
    /// The blocks it dug last, newest at the back.
    explored: VecDeque<BlockPos>,
}

impl Sniffer {
    fn new() -> Sniffer {
        Sniffer {
            phase: Phase::Idling,
            ticks: 0,
            cooldown: None,
            goal: None,
            explored: VecDeque::new(),
        }
    }

    fn remember(&mut self, pos: BlockPos) {
        if self.explored.len() >= EXPLORED {
            self.explored.pop_front();
        }
        self.explored.push_back(pos);
    }

    /// Whether the seed of the dig under way has come up already.
    fn dug(&self) -> bool {
        matches!(self.phase, Phase::Rising | Phase::FeelingHappy)
            || (self.phase == Phase::Digging && self.ticks >= DROP_AFTER)
    }

    /// The wait before the next search when a dig stops now.
    fn wait_after_stop(&self) -> u32 {
        if self.dug() {
            SEARCH_COOLDOWN
        } else {
            RETRY
        }
    }
}

fn sniffer_of(mob: &Mob) -> Option<&Sniffer> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Sniffer>())
}

fn state_data(id: i32, phase: Phase) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_STATE)
        .varint(SERIALIZER_SNIFFER_STATE)
        .varint(phase as i32)
        .u8(0xFF)
}

fn drop_seed_data(id: i32, at: u64) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_DROP_SEED_AT)
        .varint(SERIALIZER_VARINT)
        .varint(at.min(i32::MAX as u64) as i32)
        .u8(0xFF)
}

fn is_egg(state: u16) -> bool {
    crate::blocks::name(state) == EGG
}

fn centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 1.0,
        f64::from(pos.z) + 0.5,
    ]
}

/// The unit vector in x and z from `from` towards `to`, or along `yaw` when they meet.
fn heading(from: [f64; 3], to: [f64; 3], yaw: f32) -> [f64; 2] {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    let length = dx.hypot(dz);
    if length < 1e-6 {
        let r = f64::from(yaw).to_radians();
        [-r.sin(), r.cos()]
    } else {
        [dx / length, dz / length]
    }
}

pub(super) fn spawned(w: &mut World, mob: &mut Mob, _biome: &str) {
    let mut s = Sniffer::new();
    s.cooldown = Some(first_wait(&mut w.rng));
    mob.kin = Some(Box::new(s));
}

fn first_wait(rng: &mut Rng) -> u32 {
    FIRST_WAIT.0 + rng.below(FIRST_WAIT.1 - FIRST_WAIT.0 + 1)
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    let mut s = match mob.kin.take().and_then(|k| k.downcast::<Sniffer>().ok()) {
        Some(s) => s,
        None => Box::new(Sniffer::new()),
    };
    let busy = w.tick_sniffer(id, mob, &mut s);
    mob.kin = Some(s);
    busy
}

pub(super) fn use_on(
    _w: &mut World,
    _player: i32,
    _target: i32,
    _slot: usize,
    _held: &str,
) -> bool {
    // Torchflower seeds go through the shared breeding code.
    false
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, _attacker: Option<i32>) {
    // A hurt sniffer gives up whatever it was doing; one that has dug its seed up
    // keeps its eight minutes.
    let Some(mut kin) = mob.kin.take() else {
        return;
    };
    if let Some(s) = kin.downcast_mut::<Sniffer>() {
        if s.phase != Phase::Idling {
            let wait = s.wait_after_stop();
            w.sniffer_stop(id, mob, s, wait);
        }
    }
    mob.kin = Some(kin);
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    match sniffer_of(mob) {
        Some(s) if s.phase != Phase::Idling => vec![state_data(id, s.phase)],
        _ => Vec::new(),
    }
}

/// The dug blocks and the search wait, as brain memories.
pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let Some(s) = sniffer_of(mob) else {
        return;
    };
    let mut memories = nbt
        .compound("Brain")
        .and_then(|b| b.compound("memories"))
        .cloned()
        .unwrap_or_default();
    if !s.explored.is_empty() {
        let places: Vec<Tag> = s
            .explored
            .iter()
            .map(|p| {
                Tag::Compound(
                    Compound::new()
                        .with("dimension", OVERWORLD)
                        .with("pos", vec![p.x, p.y, p.z]),
                )
            })
            .collect();
        memories.insert(MEMORY_EXPLORED, Compound::new().with("value", places));
    }
    // A dig under way ends in the full wait once it is saved (it does not go on after
    // a load); one stopped early in the short one.
    let wait = match s.phase {
        Phase::Idling => s.cooldown.unwrap_or(0),
        _ => s.wait_after_stop(),
    };
    if wait > 0 {
        memories.insert(
            MEMORY_COOLDOWN,
            Compound::new()
                .with("value", Compound::new())
                .with("ttl", i64::from(wait)),
        );
    }
    if !memories.is_empty() {
        let mut brain = nbt.compound("Brain").cloned().unwrap_or_default();
        brain.insert("memories", memories);
        nbt.insert("Brain", brain);
    }
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let memories = nbt.compound("Brain").and_then(|b| b.compound("memories"));
    let mut s = Sniffer::new();
    let places = memories
        .and_then(|m| m.compound(MEMORY_EXPLORED))
        .and_then(|e| e.list("value"))
        .unwrap_or_default();
    for place in places.iter().filter_map(Tag::as_compound) {
        if place.str("dimension").is_some_and(|d| d != OVERWORLD) {
            continue;
        }
        if let Some(&[x, y, z]) = place.int_array("pos") {
            s.remember(BlockPos::new(x, y, z));
        }
    }
    s.cooldown = memories
        .and_then(|m| m.compound(MEMORY_COOLDOWN))
        .map(|c| c.int("ttl").unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32);
    mob.kin = Some(Box::new(s));
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

pub(super) fn may_love(_w: &World, _mob: &Mob) -> bool {
    true
}

/// Two sniffers bred: one of them lays a sniffer egg, dropped as an item.
pub(super) fn bred(w: &mut World, _id: i32, mob: &mut Mob) -> bool {
    let Some(item) = ItemInfo::id_of(EGG) else {
        return false;
    };
    let pitch = 0.3 + (w.rng.f64() as f32 - 0.5) * 0.24;
    w.play_sound_at(
        "minecraft:block.sniffer_egg.plop",
        mob.pos,
        1.0,
        pitch,
        None,
    );
    w.scatter_item(mob.pos, Stack::new(item, 1));
    true
}

pub(super) fn tick_world(_w: &mut World) {}

/// A sniffer egg placed by a player starts its hatching timer.
pub(super) fn block_placed(w: &mut World, _player: i32, pos: BlockPos, state: u16) {
    if is_egg(state) {
        schedule_crack(w, pos, state);
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

/// A random tick of a sniffer egg: one without a hatching timer (set by a command,
/// say) gets one. Eggs hatch on their timer alone, whatever `random_tick_speed` is.
pub(super) fn block_tick(w: &mut World, pos: BlockPos, state: u16) -> bool {
    if !is_egg(state) {
        return false;
    }
    if !w.is_scheduled(pos, state) {
        schedule_crack(w, pos, state);
    }
    true
}

pub(super) fn ticks_randomly(state: u16) -> bool {
    is_egg(state)
}

/// The scheduled tick of a sniffer egg: it cracks, or hatches once cracked twice.
pub(super) fn scheduled_tick(w: &mut World, pos: BlockPos, state: u16) -> bool {
    if !is_egg(state) {
        return false;
    }
    let hatch: u8 = crate::blocks::prop(state, "hatch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let at = [pos.x, pos.y, pos.z].map(|v| f64::from(v) + 0.5);
    let pitch = 0.72 + w.rng.f64() as f32 * 0.16;
    if hatch < 2 {
        let cracked = crate::blocks::with(state, "hatch", &(hatch + 1).to_string());
        w.set_block_updating(pos, cracked);
        w.play_sound_at("minecraft:block.sniffer_egg.crack", at, 0.7, pitch, None);
        schedule_crack(w, pos, cracked);
        return true;
    }
    w.set_block_updating(pos, block::AIR);
    w.play_sound_at("minecraft:block.sniffer_egg.hatch", at, 0.7, pitch, None);
    let mut snifflet = Mob::new(
        MobKind::Sniffer,
        [at[0], f64::from(pos.y), at[2]],
        &mut w.rng,
    );
    snifflet.age = SNIFFLET_AGE;
    w.mobs.insert(next_entity_id(), snifflet);
    true
}

/// Ticks until the next crack of an egg at `pos`: half as long on moss.
fn crack_delay(w: &mut World, pos: BlockPos) -> u64 {
    let below = w.block(BlockPos::new(pos.x, pos.y - 1, pos.z));
    if block_has_tag(below, HATCH_BOOST) {
        HATCH_STAGE_BOOSTED
    } else {
        HATCH_STAGE
    }
}

fn schedule_crack(w: &mut World, pos: BlockPos, state: u16) {
    let delay = crack_delay(w, pos);
    w.schedule(pos, state, delay);
}

impl World {
    /// A sniffer's turn. Returns whether it keeps still (scenting, sniffing, digging).
    fn tick_sniffer(&mut self, id: i32, mob: &mut Mob, s: &mut Sniffer) -> bool {
        let cooldown = s.cooldown.get_or_insert_with(|| first_wait(&mut self.rng));
        let baby = mob.age < 0;
        if s.phase == Phase::Idling {
            *cooldown = cooldown.saturating_sub(1);
            if *cooldown == 0 && !baby && self.may_dig(mob) {
                self.sniffer_phase(id, mob, s, Phase::Sniffing);
                self.play_sound_at("minecraft:entity.sniffer.sniffing", mob.pos, 1.0, 1.0, None);
                mob.target = None;
                mob.path.clear();
                return true;
            }
            if mob.panic == 0
                && mob.love == 0
                && mob.on_ground_now()
                && !self.in_water(mob.pos)
                && self.rng.below(SCENT_CHANCE) == 0
            {
                self.sniffer_phase(id, mob, s, Phase::Scenting);
                let pitch = if baby { 1.3 } else { 1.0 };
                self.play_sound_at(
                    "minecraft:entity.sniffer.scenting",
                    mob.pos,
                    1.0,
                    pitch,
                    None,
                );
                mob.target = None;
                mob.path.clear();
                return true;
            }
            return false;
        }
        s.ticks += 1;
        // Off the ground (falling, or the ground taken from under it) it stops keeping
        // still, so it falls or swims as any mob does.
        if s.phase != Phase::Searching && !mob.on_ground_now() {
            let wait = s.wait_after_stop();
            self.sniffer_stop(id, mob, s, wait);
            return false;
        }
        match s.phase {
            Phase::Idling => false,
            Phase::Scenting => {
                if mob.panic > 0 || s.ticks >= SCENT_TICKS {
                    self.sniffer_phase(id, mob, s, Phase::Idling);
                    return false;
                }
                hold_still(mob);
                true
            }
            Phase::Sniffing => {
                if !self.may_dig(mob) {
                    self.sniffer_stop(id, mob, s, RETRY);
                    return false;
                }
                hold_still(mob);
                if s.ticks < SNIFF_TICKS {
                    return true;
                }
                match self.dig_spot(mob, s) {
                    Some(goal) => {
                        s.goal = Some(goal);
                        self.sniffer_phase(id, mob, s, Phase::Searching);
                    }
                    None => self.sniffer_stop(id, mob, s, RETRY),
                }
                true
            }
            Phase::Searching => self.sniffer_search(id, mob, s),
            Phase::Digging => {
                if !self.may_dig(mob) {
                    let wait = s.wait_after_stop();
                    self.sniffer_stop(id, mob, s, wait);
                    return false;
                }
                hold_still(mob);
                if s.ticks == DROP_AFTER {
                    if let Some(goal) = s.goal {
                        self.dig_up(mob, goal);
                        s.remember(goal);
                    }
                }
                if s.ticks >= DIG_TICKS {
                    self.sniffer_phase(id, mob, s, Phase::Rising);
                    self.play_sound_at(
                        "minecraft:entity.sniffer.digging_stop",
                        mob.pos,
                        1.0,
                        1.0,
                        None,
                    );
                }
                true
            }
            Phase::Rising => {
                hold_still(mob);
                if s.ticks >= RISE_TICKS {
                    self.sniffer_phase(id, mob, s, Phase::FeelingHappy);
                    self.play_sound_at("minecraft:entity.sniffer.happy", mob.pos, 1.0, 1.0, None);
                }
                true
            }
            Phase::FeelingHappy => {
                if s.ticks >= HAPPY_TICKS || mob.panic > 0 {
                    self.sniffer_stop(id, mob, s, SEARCH_COOLDOWN);
                    return false;
                }
                hold_still(mob);
                true
            }
        }
    }

    /// Walking to the block it smelled; it sploots once its head is over it.
    fn sniffer_search(&mut self, id: i32, mob: &mut Mob, s: &mut Sniffer) -> bool {
        let Some(goal) = s.goal else {
            self.sniffer_stop(id, mob, s, RETRY);
            return false;
        };
        if !self.may_dig(mob) || s.ticks > SEARCH_TICKS || !self.diggable(goal, s) {
            self.sniffer_stop(id, mob, s, RETRY);
            return false;
        }
        if s.ticks.is_multiple_of(SEARCH_SOUND_EVERY) {
            let volume = if self.rng.below(2) == 0 { 1.0 } else { 0.8 };
            self.play_sound_at(
                "minecraft:entity.sniffer.searching",
                mob.pos,
                volume,
                1.0,
                None,
            );
        }
        let spot = centre(goal);
        let dir = heading(mob.pos, spot, mob.yaw);
        let far = (spot[0] - mob.pos[0]).hypot(spot[2] - mob.pos[2]);
        let level = (mob.pos[1] - 0.01).floor() as i32;
        if level == goal.y && (DIG_REACH.0..=DIG_REACH.1).contains(&far) {
            mob.yaw = yaw_towards(mob.pos, spot);
            mob.head_yaw = mob.yaw;
            let head = self.head_block(mob, dir);
            if self.diggable(head, s) {
                s.goal = Some(head);
                hold_still(mob);
                self.sniffer_phase(id, mob, s, Phase::Digging);
                let drop_at = self.tick + u64::from(DROP_AFTER);
                self.send_to_viewers(mob, drop_seed_data(id, drop_at));
                self.play_sound_at("minecraft:entity.sniffer.digging", mob.pos, 1.0, 1.0, None);
                return true;
            }
        }
        // Walk to where its head comes over the block, standing at the block's height.
        let to = BlockPos::new(
            (spot[0] - dir[0] * HEAD_OFFSET).floor() as i32,
            goal.y + 1,
            (spot[2] - dir[1] * HEAD_OFFSET).floor() as i32,
        );
        self.follow_path(mob, to);
        if mob.path.is_empty() {
            // No way there, or there and still not over it: it gives up rather than
            // working the path out again every tick.
            self.sniffer_stop(id, mob, s, RETRY);
        }
        false
    }

    /// Moves a sniffer on to `phase`, telling viewers.
    fn sniffer_phase(&mut self, id: i32, mob: &Mob, s: &mut Sniffer, phase: Phase) {
        s.phase = phase;
        s.ticks = 0;
        self.send_to_viewers(mob, state_data(id, phase));
    }

    /// Back to idling, not searching again for `wait` ticks.
    fn sniffer_stop(&mut self, id: i32, mob: &mut Mob, s: &mut Sniffer, wait: u32) {
        s.goal = None;
        s.cooldown = Some(wait);
        if s.phase != Phase::Idling {
            self.sniffer_phase(id, mob, s, Phase::Idling);
        }
        mob.target = None;
        mob.path.clear();
    }

    /// Whether a sniffer may search and dig now.
    fn may_dig(&self, mob: &Mob) -> bool {
        let below = BlockPos::new(
            mob.pos[0].floor() as i32,
            (mob.pos[1] - 0.01).floor() as i32,
            mob.pos[2].floor() as i32,
        );
        mob.age >= 0
            && mob.alive()
            && mob.panic == 0
            && mob.love == 0
            && mob.vehicle.is_none()
            && self.loaded_block(below).is_some_and(|b| !is_air(b))
            && !self.in_water(mob.pos)
            && self.tempting_player(mob).is_none()
    }

    /// Whether the block at `pos` can be dug: diggable, open above, not dug lately.
    fn diggable(&self, pos: BlockPos, s: &Sniffer) -> bool {
        let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
        self.loaded_block(pos)
            .is_some_and(|b| block_has_tag(b, DIGGABLE))
            && self.loaded_block(above).is_some_and(is_air)
            && !s.explored.contains(&pos)
    }

    /// The block under a sniffer's head, facing along `dir`.
    fn head_block(&self, mob: &Mob, dir: [f64; 2]) -> BlockPos {
        BlockPos::new(
            (mob.pos[0] + dir[0] * HEAD_OFFSET).floor() as i32,
            (mob.pos[1] - 0.01).floor() as i32,
            (mob.pos[2] + dir[1] * HEAD_OFFSET).floor() as i32,
        )
    }

    /// A diggable block near by, on top of the ground, picked at random among all of
    /// them (without collecting them).
    fn dig_spot(&mut self, mob: &Mob, s: &Sniffer) -> Option<BlockPos> {
        let feet = mob.pos[1].floor() as i32;
        let (mx, mz) = (mob.pos[0].floor() as i32, mob.pos[2].floor() as i32);
        let mut rng = Rng::new(self.rng.next_u64());
        let mut seen = 0u32;
        let mut pick = None;
        for x in mx - SEARCH_RANGE..=mx + SEARCH_RANGE {
            for z in mz - SEARCH_RANGE..=mz + SEARCH_RANGE {
                // The top of the ground there, within a few blocks up or down.
                let top = (feet - 4..=feet + 2).rev().find(|&y| {
                    self.loaded_block(BlockPos::new(x, y, z))
                        .is_some_and(|b| !is_air(b))
                });
                let Some(y) = top else {
                    continue;
                };
                let pos = BlockPos::new(x, y, z);
                if self.diggable(pos, s) {
                    seen += 1;
                    if rng.below(seen) == 0 {
                        pick = Some(pos);
                    }
                }
            }
        }
        pick
    }

    /// The seed comes up: an item from the digging loot table on top of the block.
    fn dig_up(&mut self, mob: &Mob, pos: BlockPos) {
        let mut ctx = crate::loot::LootContext {
            state: 0,
            tool: 0,
            rng: &mut self.rng,
            enchants: crate::enchanting::Enchantments::NONE,
            entity: Default::default(),
        };
        let items = self.loot.gameplay(LOOT, &mut ctx);
        let at = centre(pos);
        for stack in items {
            self.scatter_item(at, stack);
        }
        let pitch = [0.6, 0.7, 0.8][self.rng.below(3) as usize];
        self.play_sound_at(
            "minecraft:entity.sniffer.drop_seed",
            mob.pos,
            0.8,
            pitch,
            None,
        );
    }
}

/// Keeps a sniffer where it is this tick.
fn hold_still(mob: &mut Mob) {
    mob.target = None;
    mob.path.clear();
    mob.vel[0] = 0.0;
    mob.vel[2] = 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-sniffers-{}-{}",
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
        w.players
            .insert(alice, Player::new(profile, out, [-28.5, -60.0, -28.5]));
        (w, alice)
    }

    /// Stone under everything near the origin but the grass block at (2, -61, 0).
    fn one_grass_block(w: &mut World) -> BlockPos {
        for x in -12..=12 {
            for z in -12..=12 {
                w.set_block(BlockPos::new(x, -61, z), block::STONE);
            }
        }
        let grass = BlockPos::new(2, -61, 0);
        w.set_block(
            grass,
            BlockInfo::by_name("minecraft:grass_block")
                .unwrap()
                .default_state,
        );
        grass
    }

    fn sniffer(w: &mut World, at: [f64; 3]) -> Mob {
        let mut mob = Mob::new(MobKind::Sniffer, at, &mut w.rng);
        mob.set_on_ground(true);
        let mut s = Sniffer::new();
        s.cooldown = Some(1);
        mob.kin = Some(Box::new(s));
        mob
    }

    fn state(mob: &Mob) -> &Sniffer {
        sniffer_of(mob).expect("sniffer state")
    }

    fn state_mut(mob: &mut Mob) -> &mut Sniffer {
        mob.kin
            .as_mut()
            .and_then(|k| k.downcast_mut::<Sniffer>())
            .expect("sniffer state")
    }

    fn egg_timers(w: &World) -> Vec<i64> {
        w.scheduled_ticks_in(0, 0)
            .iter()
            .filter(|t| t.str("i") == Some(EGG))
            .filter_map(|t| t.int("t"))
            .collect()
    }

    fn items_named(w: &World, name: &str) -> i32 {
        let id = ItemInfo::id_of(name).unwrap();
        w.items
            .values()
            .filter(|i| i.stack().item == id)
            .map(|i| i.stack().count)
            .sum()
    }

    /// Ticks the sniffer until it is idle again after a dig, or `max` ticks pass.
    fn run(w: &mut World, id: i32, mob: &mut Mob, max: u32) -> Vec<Phase> {
        let mut seen = vec![state(mob).phase];
        for _ in 0..max {
            w.tick += 1;
            tick(w, id, mob);
            let phase = state(mob).phase;
            if seen.last() != Some(&phase) {
                seen.push(phase);
            }
            if phase == Phase::Idling && seen.len() > 1 {
                break;
            }
        }
        seen
    }

    #[test]
    fn a_sniffer_digs_up_a_seed_and_not_the_same_block_again() {
        let (mut w, _) = world_with_player();
        let grass = one_grass_block(&mut w);
        let id = next_entity_id();
        let mut mob = sniffer(&mut w, [0.5, -60.0, 0.5]);
        mob.yaw = -90.0;
        let mut phases = Vec::new();
        // Scenting may come first; then the long sniff and the dig.
        for _ in 0..4 {
            phases.extend(run(&mut w, id, &mut mob, 1000));
            if state(&mob).explored.contains(&grass) {
                break;
            }
            state_mut(&mut mob).cooldown = Some(1);
        }
        for p in [
            Phase::Sniffing,
            Phase::Searching,
            Phase::Digging,
            Phase::Rising,
            Phase::FeelingHappy,
        ] {
            assert!(phases.contains(&p), "{p:?} in {phases:?}");
        }
        let s = state(&mob);
        assert_eq!(s.phase, Phase::Idling);
        assert_eq!(s.cooldown, Some(SEARCH_COOLDOWN));
        assert_eq!(s.explored.iter().collect::<Vec<_>>(), vec![&grass]);
        let seeds = items_named(&w, "minecraft:torchflower_seeds");
        let pods = items_named(&w, "minecraft:pitcher_pod");
        assert_eq!(seeds + pods, 1, "one seed dug up");
        // With the only grass dug, the next search finds nothing.
        state_mut(&mut mob).cooldown = Some(1);
        let mut dug_again = false;
        for _ in 0..200 {
            w.tick += 1;
            tick(&mut w, id, &mut mob);
            dug_again |= matches!(state(&mob).phase, Phase::Searching | Phase::Digging);
        }
        assert!(!dug_again);
        assert_eq!(state(&mob).cooldown.map(|c| c > 0), Some(true));
    }

    #[test]
    fn the_seed_table_gives_seeds_or_pods_at_even_odds() {
        let (mut w, _) = world_with_player();
        let mob = sniffer(&mut w, [0.5, -60.0, 0.5]);
        for _ in 0..400 {
            w.dig_up(&mob, BlockPos::new(0, -61, 0));
        }
        let seeds = items_named(&w, "minecraft:torchflower_seeds");
        let pods = items_named(&w, "minecraft:pitcher_pod");
        assert_eq!(seeds + pods, 400);
        assert!((140..=260).contains(&seeds), "{seeds}");
    }

    #[test]
    fn snifflets_and_disturbed_sniffers_do_not_dig() {
        let (mut w, _) = world_with_player();
        one_grass_block(&mut w);
        let id = next_entity_id();
        let mut baby = sniffer(&mut w, [0.5, -60.0, 0.5]);
        baby.age = SNIFFLET_AGE;
        for _ in 0..300 {
            w.tick += 1;
            tick(&mut w, id, &mut baby);
            assert!(matches!(
                state(&baby).phase,
                Phase::Idling | Phase::Scenting
            ));
        }
        // A sniffer that panics mid-dig gives it up.
        let mut mob = sniffer(&mut w, [0.5, -60.0, 0.5]);
        for _ in 0..200 {
            w.tick += 1;
            tick(&mut w, id, &mut mob);
            if state(&mob).phase == Phase::Digging {
                break;
            }
        }
        if state(&mob).phase == Phase::Digging {
            mob.panic = 40;
            assert!(!tick(&mut w, id, &mut mob));
            assert_eq!(state(&mob).phase, Phase::Idling);
            assert!(state(&mob).explored.is_empty());
            assert_eq!(state(&mob).cooldown, Some(RETRY));
        }
        // In love: no search starts.
        let mut lover = sniffer(&mut w, [0.5, -60.0, 0.5]);
        lover.love = 600;
        assert!(!w.may_dig(&lover));
    }

    #[test]
    fn hurting_a_sniffer_after_its_dig_keeps_the_long_wait() {
        let (mut w, _) = world_with_player();
        let id = next_entity_id();
        for (phase, ticks, wait) in [
            (Phase::Searching, 5, RETRY),
            (Phase::Digging, DROP_AFTER - 1, RETRY),
            (Phase::Digging, DROP_AFTER, SEARCH_COOLDOWN),
            (Phase::Rising, 3, SEARCH_COOLDOWN),
            (Phase::FeelingHappy, 3, SEARCH_COOLDOWN),
        ] {
            let mut mob = sniffer(&mut w, [0.5, -60.0, 0.5]);
            let s = state_mut(&mut mob);
            s.phase = phase;
            s.ticks = ticks;
            hurt(&mut w, id, &mut mob, None);
            assert_eq!(state(&mob).phase, Phase::Idling);
            assert_eq!(state(&mob).cooldown, Some(wait), "{phase:?} at {ticks}");
        }
    }

    #[test]
    fn a_sniffer_off_the_ground_does_not_hang_in_the_air() {
        let (mut w, _) = world_with_player();
        let id = next_entity_id();
        let mut mob = sniffer(&mut w, [0.5, -50.0, 0.5]);
        mob.set_on_ground(false);
        state_mut(&mut mob).cooldown = Some(100_000);
        for _ in 0..3000 {
            w.tick += 1;
            assert!(!tick(&mut w, id, &mut mob), "falling, not scenting");
        }
        // The ground taken away mid-dig: it stops and falls.
        mob.set_on_ground(true);
        let s = state_mut(&mut mob);
        s.phase = Phase::Digging;
        s.ticks = 10;
        mob.set_on_ground(false);
        assert!(!tick(&mut w, id, &mut mob));
        assert_eq!(state(&mob).phase, Phase::Idling);
        assert_eq!(state(&mob).cooldown, Some(RETRY));
    }

    #[test]
    fn a_sniffer_digs_a_block_a_step_down() {
        let (mut w, _) = world_with_player();
        // The sniffer stands on stone at -61; the grass lies one lower, in a dip.
        for x in -12..=12 {
            for z in -12..=12 {
                w.set_block(BlockPos::new(x, -61, z), block::STONE);
                w.set_block(BlockPos::new(x, -62, z), block::STONE);
            }
        }
        let grass = BlockPos::new(6, -62, 0);
        for x in 3..=9 {
            for z in -2..=2 {
                w.set_block(BlockPos::new(x, -61, z), block::AIR);
            }
        }
        w.set_block(
            grass,
            BlockInfo::by_name("minecraft:grass_block")
                .unwrap()
                .default_state,
        );
        let id = next_entity_id();
        let mut mob = sniffer(&mut w, [0.5, -60.0, 0.5]);
        let s = state_mut(&mut mob);
        s.phase = Phase::Searching;
        s.goal = Some(grass);
        // Walk it by hand along its path, as step() would, down into the dip.
        let mut dug = false;
        for _ in 0..SEARCH_TICKS {
            w.tick += 1;
            tick(&mut w, id, &mut mob);
            match state(&mob).phase {
                Phase::Searching => {}
                Phase::Digging => {
                    dug = true;
                    break;
                }
                _ => break,
            }
            if let Some(&next) = mob.path.first() {
                mob.pos = [
                    f64::from(next.x) + 0.5,
                    f64::from(next.y),
                    f64::from(next.z) + 0.5,
                ];
            }
        }
        assert!(dug, "dug the lower block, at {:?}", mob.pos);
        assert_eq!(state(&mob).goal, Some(grass));
    }

    #[test]
    fn dig_memories_are_saved_as_vanilla_and_read_back() {
        let (mut w, _) = world_with_player();
        let mut mob = sniffer(&mut w, [0.5, -60.0, 0.5]);
        let s = state_mut(&mut mob);
        s.remember(BlockPos::new(1, -61, 2));
        s.remember(BlockPos::new(-3, -61, 4));
        s.cooldown = Some(5000);
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        let memories = nbt
            .compound("Brain")
            .and_then(|b| b.compound("memories"))
            .expect("memories");
        let places = memories
            .compound(MEMORY_EXPLORED)
            .and_then(|e| e.list("value"))
            .expect("explored");
        assert_eq!(places.len(), 2);
        let first = places[0].as_compound().unwrap();
        assert_eq!(first.str("dimension"), Some(OVERWORLD));
        assert_eq!(first.int_array("pos"), Some(&[1, -61, 2][..]));
        let cooldown = memories.compound(MEMORY_COOLDOWN).expect("cooldown");
        assert_eq!(cooldown.int("ttl"), Some(5000));
        assert!(cooldown.compound("value").is_some());

        let mut loaded = Mob::new(MobKind::Sniffer, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut loaded, &nbt);
        let back = state(&loaded);
        assert_eq!(back.cooldown, Some(5000));
        assert_eq!(
            back.explored.iter().copied().collect::<Vec<_>>(),
            vec![BlockPos::new(1, -61, 2), BlockPos::new(-3, -61, 4)]
        );

        // Saved mid-dig after the seed came up: the full wait.
        let s = state_mut(&mut mob);
        s.phase = Phase::Rising;
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        let mut loaded = Mob::new(MobKind::Sniffer, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut loaded, &nbt);
        assert_eq!(state(&loaded).cooldown, Some(SEARCH_COOLDOWN));

        // Nothing saved: the first wait is rolled on its first turn.
        let mut fresh = Mob::new(MobKind::Sniffer, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut fresh, &Compound::new());
        assert_eq!(state(&fresh).cooldown, None);
        assert!(state(&fresh).explored.is_empty());
    }

    #[test]
    fn bred_sniffers_lay_an_egg_item() {
        let (mut w, _) = world_with_player();
        let mut mob = sniffer(&mut w, [0.5, -60.0, 0.5]);
        assert!(bred(&mut w, 1, &mut mob), "no snifflet");
        assert_eq!(items_named(&w, EGG), 1);
        assert!(!w.mobs.values().any(|m| m.kind == MobKind::Sniffer));
    }

    #[test]
    fn eggs_crack_twice_and_hatch_into_a_snifflet() {
        let (mut w, alice) = world_with_player();
        let egg = BlockInfo::by_name(EGG).unwrap().default_state;
        let pos = BlockPos::new(3, -60, 3);
        w.set_block(pos, egg);
        block_placed(&mut w, alice, pos, egg);
        assert_eq!(
            egg_timers(&w),
            vec![HATCH_STAGE as i64],
            "20 minutes on grass"
        );
        assert!(scheduled_tick(&mut w, pos, egg));
        let once = w.block(pos);
        assert_eq!(crate::blocks::prop(once, "hatch"), Some("1"));
        assert!(scheduled_tick(&mut w, pos, once));
        let twice = w.block(pos);
        assert_eq!(crate::blocks::prop(twice, "hatch"), Some("2"));
        assert!(w.mobs.is_empty());
        assert!(scheduled_tick(&mut w, pos, twice));
        assert!(is_air(w.block(pos)), "hatched");
        let snifflets: Vec<_> = w
            .mobs
            .values()
            .filter(|m| m.kind == MobKind::Sniffer)
            .collect();
        assert_eq!(snifflets.len(), 1);
        assert_eq!(snifflets[0].age, SNIFFLET_AGE);
        // Other blocks are not ours.
        assert!(!scheduled_tick(&mut w, pos, block::STONE));
    }

    #[test]
    fn eggs_on_moss_hatch_twice_as_fast() {
        let (mut w, alice) = world_with_player();
        let moss = BlockInfo::by_name("minecraft:moss_block")
            .unwrap()
            .default_state;
        let pale = BlockInfo::by_name("minecraft:pale_moss_block")
            .unwrap()
            .default_state;
        let egg = BlockInfo::by_name(EGG).unwrap().default_state;
        w.set_block(BlockPos::new(3, -61, 3), moss);
        w.set_block(BlockPos::new(3, -60, 3), egg);
        block_placed(&mut w, alice, BlockPos::new(3, -60, 3), egg);
        w.set_block(BlockPos::new(5, -61, 5), pale);
        w.set_block(BlockPos::new(5, -60, 5), egg);
        block_placed(&mut w, alice, BlockPos::new(5, -60, 5), egg);
        let mut timers = egg_timers(&w);
        timers.sort_unstable();
        assert_eq!(timers, vec![HATCH_STAGE_BOOSTED as i64, HATCH_STAGE as i64]);
        assert_eq!(HATCH_STAGE_BOOSTED * 3, 12_000, "10 minutes in all");
        assert_eq!(HATCH_STAGE * 3, 24_000, "20 minutes in all");
    }

    #[test]
    fn eggs_without_a_timer_get_one() {
        let (mut w, _) = world_with_player();
        let egg = BlockInfo::by_name(EGG).unwrap().default_state;
        let pos = BlockPos::new(3, -60, 3);
        w.set_block(pos, egg);
        assert!(ticks_randomly(egg));
        assert!(!ticks_randomly(block::STONE));
        assert!(block_tick(&mut w, pos, egg));
        assert!(block_tick(&mut w, pos, egg));
        assert_eq!(egg_timers(&w).len(), 1);
    }

    #[test]
    fn state_shows_to_new_viewers_and_explored_blocks_are_capped() {
        let (mut w, _) = world_with_player();
        let mut mob = sniffer(&mut w, [0.5, -60.0, 0.5]);
        assert!(spawn_extras(1, &mob).is_empty());
        let s = state_mut(&mut mob);
        s.phase = Phase::Digging;
        for x in 0..30 {
            s.remember(BlockPos::new(x, -61, 0));
        }
        assert_eq!(s.explored.len(), EXPLORED);
        assert_eq!(s.explored.front(), Some(&BlockPos::new(10, -61, 0)));
        assert_eq!(spawn_extras(1, &mob).len(), 1);
        assert_eq!(Phase::Rising as i32, 6);
    }
}
