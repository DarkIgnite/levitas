//! Copper golems, following the Minecraft Wiki (Copper Golem, Copper Chest, Copper
//! Golem Statue, Oxidation):
//! - A carved pumpkin or jack o'lantern placed last next to a block of copper (on top,
//!   or beside or under it) makes a golem where the pumpkin was, of the block's
//!   oxidation stage; the block becomes a copper chest of that stage, facing the way
//!   the pumpkin did, which pairs with a copper chest beside it. Waxed blocks work too,
//!   but neither golem nor chest is waxed.
//! - Item sorting: a golem with empty hands walks to the nearest copper chest within a
//!   65x17x65 area, stops for three seconds and takes up to 16 of the first item in
//!   it. Holding something, it goes to the nearest chest or trapped chest instead and
//!   puts the items in if the chest is empty or already holds that item (by type,
//!   components ignored): into the frontmost slot that takes them, and what is left
//!   only into empty slots after it. A large chest counts as one. Chests that let it
//!   down are remembered; after ten (or when none is left to try) it wanders for seven
//!   seconds and forgets them, as it does whenever it succeeds. It reaches chests up to
//!   one block above and two below its feet, and two golems never work the same chest
//!   at once. A player using an empty hand on a golem holding something takes it,
//!   dropped on the ground; a golem that dies drops it too.
//! - Oxidation: every 504,000 to 552,000 ticks of game time an unwaxed golem goes one
//!   stage further (unaffected, exposed, weathered, oxidized). An axe takes off the
//!   wax, or else a stage; so does a lightning bolt hitting an unwaxed golem. Honeycomb
//!   waxes it. A fully oxidized unwaxed golem standing wholly in air turns into an
//!   oxidized copper golem statue with a random pose, 0.58% of ticks, dropping what it
//!   holds.
//! - Statues: using one with anything but an axe or a honeycomb (unless sneaking with
//!   an item) cycles its pose; an axe scrapes off the wax or a stage, and reanimates an
//!   unaffected unwaxed statue into a golem; honeycomb waxes it. Unwaxed statues
//!   oxidize on random ticks like other copper blocks.
//! - Copper chests: placing one gives it chest storage (saved as `minecraft:chest`, as
//!   vanilla does); two side by side with the same facing pair into a large chest of
//!   the less oxidized stage, unwaxed unless both were waxed (not when sneaking). Using
//!   one opens it, its lid opening and closing with the copper chest sounds. An axe
//!   while sneaking scrapes off the wax or a stage, honeycomb waxes it, and unwaxed
//!   ones oxidize on random ticks; both halves of a large chest change together.
//! - Copper golems take no fall damage and do not drown. They never breed.
//! - Entity data: weathering state (16) and copper golem state (17), which shows the
//!   taking and dropping animations. Saved as vanilla: `weather_state`,
//!   `next_weather_age` (-1 not yet rolled, -2 waxed) and the held item in
//!   `equipment.mainhand`.

use std::collections::{HashMap, HashSet};

use levitas_nbt::{Compound, Tag};
use levitas_registry::{item_has_tag, menu, STATE_BLOCK};

use super::containers::View;
use super::mobs::{yaw_towards, Mob, MobKind};
use super::*;
use crate::blocks::{self, offset, Dir};

/// Where the module keeps chests being worked and golems turned to stone, in
/// `World::kin_state`.
const STATE_KEY: &str = "copper_golems";
const DATA_WEATHER: u8 = 16;
const DATA_GOLEM_STATE: u8 = 17;
const SERIALIZER_COPPER_GOLEM_STATE: i32 = 37;
const SERIALIZER_WEATHERING_COPPER_STATE: i32 = 38;
/// Copper golem states as the client knows them.
const STATE_IDLE: u8 = 0;
const STATE_GETTING_ITEM: u8 = 1;
const STATE_GETTING_NO_ITEM: u8 = 2;
const STATE_DROPPING_ITEM: u8 = 3;
const STATE_DROPPING_NO_ITEM: u8 = 4;
/// Oxidation stages, in the order of the weathering copper state.
const STAGES: [&str; 4] = ["unaffected", "exposed", "weathered", "oxidized"];
const OXIDIZED: u8 = 3;
/// `next_weather_age` values: not rolled yet, and waxed.
const UNROLLED: i64 = -1;
const WAXED: i64 = -2;
/// Ticks of game time between oxidation stages: 7 hours to 7 hours 40 minutes.
const WEATHER_TICKS: (u32, u32) = (504_000, 552_000);
/// Chance a tick that an oxidized unwaxed golem turns into a statue.
const STATUE_CHANCE: f64 = 0.0058;
const POSES: [&str; 4] = ["standing", "sitting", "running", "star"];
/// How far a golem looks for chests: 65x17x65 blocks around it.
const SEARCH_RADIUS: i32 = 32;
const SEARCH_HEIGHT: i32 = 8;
/// Ticks between looks for a chest when it has none in mind.
const SEARCH_EVERY: u32 = 20;
/// Ticks between rebuilds of the index of chests, from all block entities.
const INDEX_EVERY: u64 = 200;
/// Ticks spent examining a chest.
const EXAMINE_TICKS: u32 = 60;
/// Chests a golem remembers having failed at; at this many it gives up for a while.
const MEMORY: usize = 10;
/// Ticks of wandering once it gave up.
const WANDER_TICKS: u32 = 140;
/// Ticks of walking towards a chest before it counts as out of reach.
const TRAVEL_TICKS: u32 = 400;
/// Most items taken from a copper chest at once.
const TAKE_AT_MOST: i32 = 16;
const CHEST_SLOTS: usize = 27;
/// Block event that sets how many have a chest open (it animates the lid).
const BLOCK_EVENT_CHEST_VIEWERS: u8 = 1;
const SIZE: (f64, f64) = (0.49, 0.98);

/// What the module keeps for a golem, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq)]
struct CopperGolem {
    /// Oxidation stage, 0 (unaffected) to 3 (oxidized).
    weather: u8,
    /// The game time of the next stage; `UNROLLED` or `WAXED`.
    next_age: i64,
    held: Stack,
    /// The state shown to clients.
    state: u8,
    /// The chest it is walking to, and for how long it has been.
    goal: Option<BlockPos>,
    travel: u32,
    /// The chest being examined (both halves of a large one), and ticks left of it.
    examining: Option<(Vec<BlockPos>, u32)>,
    /// Chests it failed at since it last succeeded.
    visited: Vec<BlockPos>,
    wander: u32,
    search_in: u32,
    /// Turned to stone: gone at the end of the tick.
    statue: bool,
}

impl CopperGolem {
    fn new(weather: u8) -> CopperGolem {
        CopperGolem {
            weather: weather.min(OXIDIZED),
            next_age: UNROLLED,
            held: Stack::EMPTY,
            state: STATE_IDLE,
            goal: None,
            travel: 0,
            examining: None,
            visited: Vec::new(),
            wander: 0,
            search_in: 0,
            statue: false,
        }
    }

    fn waxed(&self) -> bool {
        self.next_age == WAXED
    }

    /// Remembers a chest that let it down; after ten of them it wanders off.
    fn failed_at(&mut self, pos: BlockPos) {
        if !self.visited.contains(&pos) {
            self.visited.push(pos);
        }
        if self.visited.len() >= MEMORY {
            self.wander = WANDER_TICKS;
        }
    }
}

/// How many have a chest's lid open, as last told to clients.
#[derive(Debug, Clone, Copy)]
struct Lid {
    players: usize,
    sent: u8,
}

/// What the module keeps for all golems, in `World::kin_state`.
#[derive(Default)]
struct Shared {
    /// Chests being examined: by which golem, and until when.
    busy: HashMap<BlockPos, (i32, u64)>,
    /// Golems that turned into statues this tick, to be taken out of the world.
    statues: Vec<i32>,
    /// Chests whose lid this module animates.
    lids: HashMap<BlockPos, Lid>,
    /// Chests and copper chests by chunk, and when to rebuild that index.
    chests: HashMap<(i32, i32), Vec<BlockPos>>,
    index_at: u64,
    /// Golems holding something or examining a chest, to be looked after if they die.
    carriers: HashSet<i32>,
}

fn shared(w: &mut World) -> &mut Shared {
    let entry = w
        .kin_state
        .entry(STATE_KEY)
        .or_insert_with(|| Box::new(Shared::default()));
    if !entry.is::<Shared>() {
        *entry = Box::new(Shared::default());
    }
    entry
        .downcast_mut::<Shared>()
        .expect("just made sure it is one")
}

fn shared_ref(w: &World) -> Option<&Shared> {
    w.kin_state
        .get(STATE_KEY)
        .and_then(|s| s.downcast_ref::<Shared>())
}

fn golem_of(mob: &Mob) -> Option<&CopperGolem> {
    mob.kin
        .as_ref()
        .and_then(|k| k.downcast_ref::<CopperGolem>())
}

fn golem_mut(mob: &mut Mob) -> &mut CopperGolem {
    if !mob.kin.as_ref().is_some_and(|k| k.is::<CopperGolem>()) {
        mob.kin = Some(Box::new(CopperGolem::new(0)));
    }
    mob.kin
        .as_mut()
        .and_then(|k| k.downcast_mut::<CopperGolem>())
        .expect("just made sure it is one")
}

fn golem_data(id: i32, g: &CopperGolem) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_WEATHER)
        .varint(SERIALIZER_WEATHERING_COPPER_STATE)
        .varint(i32::from(g.weather))
        .u8(DATA_GOLEM_STATE)
        .varint(SERIALIZER_COPPER_GOLEM_STATE)
        .varint(i32::from(g.state))
        .u8(0xFF)
}

/// What the golem holds, in its main hand.
fn held_packet(id: i32, held: Stack) -> Writer {
    let packet = Writer::packet(clientbound::SET_EQUIPMENT).varint(id).u8(0);
    crate::inventory::write_stack(packet, held)
}

/// The sound family for hurt, death and step sounds: weathered and oxidized golems
/// have their own.
pub(super) fn sound_name(mob: &Mob) -> &'static str {
    match golem_of(mob).map_or(0, |g| g.weather) {
        2 => "copper_golem_weathered",
        3 => "copper_golem_oxidized",
        _ => "copper_golem",
    }
}

fn roll_next_age(w: &mut World) -> i64 {
    let (lo, hi) = WEATHER_TICKS;
    w.tick as i64 + i64::from(lo + w.rng.below(hi - lo + 1))
}

/// The prefix of a copper block's name for an oxidation stage.
fn stage_prefix(stage: u8) -> &'static str {
    ["", "exposed_", "weathered_", "oxidized_"][usize::from(stage.min(OXIDIZED))]
}

/// A block's name split into whether it is waxed, its oxidation stage and the rest
/// (`waxed_weathered_copper_chest` is waxed, 2, `copper_chest`).
fn split_copper(name: &str) -> (bool, u8, &str) {
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    let (waxed, bare) = match bare.strip_prefix("waxed_") {
        Some(rest) => (true, rest),
        None => (false, bare),
    };
    for stage in (1..=OXIDIZED).rev() {
        if let Some(rest) = bare.strip_prefix(stage_prefix(stage)) {
            return (waxed, stage, rest);
        }
    }
    (waxed, 0, bare)
}

/// The oxidation stage of a block of copper (any stage, waxed or not).
fn copper_block_stage(state: u16) -> Option<u8> {
    if !block_has_tag(state, "minecraft:copper") {
        return None;
    }
    Some(split_copper(blocks::name(state)).1)
}

fn is_copper_chest(state: u16) -> bool {
    block_has_tag(state, "minecraft:copper_chests")
}

/// Chests a golem puts things in.
fn is_wooden_chest(state: u16) -> bool {
    matches!(
        blocks::name(state),
        "minecraft:chest" | "minecraft:trapped_chest"
    )
}

/// A copper chest's waxing and stage.
fn copper_chest_of(state: u16) -> Option<(bool, u8)> {
    if !is_copper_chest(state) {
        return None;
    }
    let (waxed, stage, _) = split_copper(blocks::name(state));
    Some((waxed, stage))
}

/// A statue's waxing and stage.
fn statue_of(state: u16) -> Option<(bool, u8)> {
    if !block_has_tag(state, "minecraft:copper_golem_statues") {
        return None;
    }
    let (waxed, stage, _) = split_copper(blocks::name(state));
    Some((waxed, stage))
}

/// `from` (a copper block of some kind) as the variant called `base` with this
/// waxing and stage, keeping its properties.
fn copper_variant(base: &str, waxed: bool, stage: u8, from: u16) -> Option<u16> {
    let name = format!(
        "minecraft:{}{}{base}",
        if waxed { "waxed_" } else { "" },
        stage_prefix(stage)
    );
    let info = BlockInfo::by_name(&name)?;
    let props = BlockInfo::of(from).properties_of(from);
    Some(info.state_with(props))
}

fn statue_state(waxed: bool, stage: u8, from: u16) -> Option<u16> {
    copper_variant("copper_golem_statue", waxed, stage, from)
}

/// The oxidation stage of an unwaxed block that oxidizes, for random ticks.
fn weathering_stage(state: u16) -> Option<u8> {
    let name = blocks::name(state);
    if !name.contains("copper") || name.contains("raw_") || name.contains("_ore") {
        return None;
    }
    let (waxed, stage, base) = split_copper(name);
    if waxed {
        return None;
    }
    if stage > 0 {
        return Some(stage);
    }
    // An unaffected block counts only if it has an exposed form.
    let exposed = match base {
        "copper_block" => "minecraft:exposed_copper".to_owned(),
        _ => format!("minecraft:exposed_{base}"),
    };
    BlockInfo::by_name(&exposed).map(|_| 0)
}

/// The sound of a chest's lid.
fn lid_sound(state: u16, open: bool) -> String {
    let action = if open { "open" } else { "close" };
    match copper_chest_of(state) {
        Some((_, 2)) => format!("minecraft:block.copper_chest_weathered.{action}"),
        Some((_, 3)) => format!("minecraft:block.copper_chest_oxidized.{action}"),
        Some(_) => format!("minecraft:block.copper_chest.{action}"),
        None => format!("minecraft:block.chest.{action}"),
    }
}

fn yaw_of(facing: Dir) -> f32 {
    match facing {
        Dir::West => 90.0,
        Dir::North => 180.0,
        Dir::East => 270.0,
        _ => 0.0,
    }
}

fn centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

fn feet_of(pos: [f64; 3]) -> BlockPos {
    BlockPos::new(
        pos[0].floor() as i32,
        (pos[1] + 0.01).floor() as i32,
        pos[2].floor() as i32,
    )
}

/// Whether a golem at `feet` can work the chest at `chest`: next to it (or on it), up
/// to one block higher and two lower.
fn within_reach(feet: BlockPos, chest: BlockPos) -> bool {
    let dy = chest.y - feet.y;
    (chest.x - feet.x).abs() <= 1 && (chest.z - feet.z).abs() <= 1 && (-2..=1).contains(&dy)
}

/// The block of copper a pumpkin at `head` was placed next to, if any: below first.
fn copper_next_to(w: &World, head: BlockPos) -> Option<(BlockPos, u8)> {
    Dir::ALL.into_iter().find_map(|d| {
        let q = offset(head, d);
        let stage = copper_block_stage(w.loaded_block(q)?)?;
        Some((q, stage))
    })
}

fn chest_storage() -> Option<BlockEntity> {
    BlockEntity::for_block(block::CHEST)
}

/// A copper chest Levitas made no storage for gets some.
fn ensure_storage(w: &mut World, pos: BlockPos) {
    if let std::collections::hash_map::Entry::Vacant(e) = w.block_entities.entry(pos) {
        if let Some(entity) = chest_storage() {
            e.insert(entity);
        }
    }
}

/// Whether players looking into `view` see the chest at `pos`.
fn views_chest(view: View, pos: BlockPos) -> bool {
    match view {
        View::Chest(q, _) => q == pos,
        View::DoubleChest(a, b) => a == pos || b == pos,
        _ => false,
    }
}

pub(super) fn spawned(_w: &mut World, mob: &mut Mob, _biome: &str) {
    golem_mut(mob);
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    golem_mut(mob);
    let Some(mut kin) = mob.kin.take() else {
        return false;
    };
    let busy = match kin.downcast_mut::<CopperGolem>() {
        Some(g) => {
            let busy = w.tick_copper_golem(id, mob, g);
            if !g.statue && (!g.held.is_empty() || g.examining.is_some()) {
                shared(w).carriers.insert(id);
            }
            busy
        }
        None => false,
    };
    mob.kin = Some(kin);
    busy
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    let Some(mut mob) = w.mobs.remove(&target) else {
        return false;
    };
    let used = w.use_on_copper_golem(player, target, &mut mob, slot, held);
    w.mobs.insert(target, mob);
    used
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, _attacker: Option<i32>) {
    let g = golem_mut(mob);
    g.goal = None;
    // A hit cuts short what it was doing at a chest. What it holds drops when it dies,
    // from `tick_world`, whatever killed it.
    if let Some((halves, _)) = g.examining.take() {
        g.state = STATE_IDLE;
        let data = golem_data(id, g);
        w.send_to_viewers(mob, data);
        w.stop_examining(id, &halves);
    }
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let Some(g) = golem_of(mob) else {
        return Vec::new();
    };
    let mut packets = vec![golem_data(id, g)];
    if !g.held.is_empty() {
        packets.push(held_packet(id, g.held));
    }
    packets
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let default = CopperGolem::new(0);
    let g = golem_of(mob).unwrap_or(&default);
    nbt.insert("weather_state", STAGES[usize::from(g.weather)]);
    nbt.insert("next_weather_age", g.next_age);
    let mut equipment = nbt
        .get("equipment")
        .and_then(Tag::as_compound)
        .cloned()
        .unwrap_or_default();
    equipment.remove("mainhand");
    if !g.held.is_empty() {
        equipment.insert("mainhand", g.held.to_nbt());
    }
    // Written even when empty, so that an item put away does not come back from the
    // copy kept of what was loaded.
    nbt.insert("equipment", equipment);
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let weather = nbt
        .str("weather_state")
        .and_then(|s| STAGES.iter().position(|&n| n == s))
        .unwrap_or(0) as u8;
    let mut g = CopperGolem::new(weather);
    g.next_age = nbt.int("next_weather_age").unwrap_or(UNROLLED);
    if g.next_age < WAXED {
        g.next_age = UNROLLED;
    }
    if let Some(hand) = nbt
        .compound("equipment")
        .and_then(|e| e.compound("mainhand"))
    {
        g.held = Stack::from_nbt(hand);
    }
    mob.kin = Some(Box::new(g));
}

pub(super) fn damage_taken(
    w: &mut World,
    id: i32,
    mob: &mut Mob,
    damage: f32,
    damage_type: &str,
    _attacker: Option<i32>,
) -> f32 {
    match damage_type {
        // Immune to falls, and never out of breath.
        "minecraft:fall" | "minecraft:drown" => 0.0,
        "minecraft:lightning_bolt" => {
            let g = golem_mut(mob);
            if !g.waxed() && g.weather > 0 {
                g.weather -= 1;
                g.next_age = UNROLLED;
                let data = golem_data(id, g);
                w.send_to_viewers(mob, data);
            }
            damage
        }
        _ => damage,
    }
}

pub(super) fn may_love(_w: &World, _mob: &Mob) -> bool {
    false
}

pub(super) fn bred(_w: &mut World, _id: i32, _mob: &mut Mob) -> bool {
    false
}

pub(super) fn tick_world(w: &mut World) {
    let Some(s) = w
        .kin_state
        .get_mut(STATE_KEY)
        .and_then(|s| s.downcast_mut::<Shared>())
    else {
        return;
    };
    let now = w.tick;
    s.busy.retain(|_, &mut (_, until)| until > now);
    let gone = std::mem::take(&mut s.statues);
    let carriers: Vec<i32> = s.carriers.iter().copied().collect();
    let lids: Vec<BlockPos> = s.lids.keys().copied().collect();
    for id in gone {
        w.mobs.remove(&id);
    }
    for id in carriers {
        w.look_after_carrier(id);
    }
    for pos in lids {
        w.sync_lid(pos);
    }
}

pub(super) fn block_placed(w: &mut World, player: i32, pos: BlockPos, state: u16) {
    if is_copper_chest(state) || is_wooden_chest(state) {
        let s = shared(w);
        let list = s.chests.entry(pos.chunk()).or_default();
        if !list.contains(&pos) {
            list.push(pos);
        }
    }
    if is_copper_chest(state) {
        ensure_storage(w, pos);
        let sneaking = w.players.get(&player).is_some_and(|p| p.sneaking);
        if !sneaking {
            w.pair_copper_chest(pos, state);
        }
        return;
    }
    if !matches!(
        blocks::name(state),
        "minecraft:carved_pumpkin" | "minecraft:jack_o_lantern"
    ) {
        return;
    }
    // Another golem may have been built of it already.
    if w.loaded_block(pos) != Some(state) {
        return;
    }
    if let Some((body, stage)) = copper_next_to(w, pos) {
        w.build_copper_golem(pos, state, body, stage);
    }
}

pub(super) fn block_broken(w: &mut World, _player: i32, pos: BlockPos, state: u16) {
    // The other half of a large copper chest is left single.
    if is_copper_chest(state) {
        if let Some(q) = w.chest_partner_of(pos, state) {
            if let Some(other) = w.loaded_block(q) {
                w.set_block_updating(q, blocks::with(other, "type", "single"));
            }
        }
    }
}

pub(super) fn block_used(
    w: &mut World,
    player: i32,
    slot: usize,
    pos: BlockPos,
    state: u16,
) -> bool {
    if is_copper_chest(state) {
        return w.use_copper_chest(player, slot, pos, state);
    }
    if statue_of(state).is_some() {
        return w.use_statue(player, slot, pos, state);
    }
    false
}

/// Random ticks of unwaxed statues and copper chests: they oxidize like other copper
/// blocks.
pub(super) fn block_tick(w: &mut World, pos: BlockPos, state: u16) -> bool {
    if let Some((waxed, stage)) = statue_of(state) {
        if !waxed && stage < OXIDIZED && oxidizes_now(w, pos, stage) {
            if let Some(next) = statue_state(false, stage + 1, state) {
                w.set_block_updating(pos, next);
            }
        }
        return true;
    }
    if let Some((waxed, stage)) = copper_chest_of(state) {
        if !waxed && stage < OXIDIZED && oxidizes_now(w, pos, stage) {
            w.set_copper_chest(pos, state, false, stage + 1);
        }
        return true;
    }
    false
}

pub(super) fn ticks_randomly(state: u16) -> bool {
    statue_of(state)
        .or_else(|| copper_chest_of(state))
        .is_some_and(|(waxed, stage)| !waxed && stage < OXIDIZED)
}

pub(super) fn scheduled_tick(_w: &mut World, _pos: BlockPos, _state: u16) -> bool {
    false
}

/// Whether a random tick moves the unwaxed copper block at `pos`, at `stage`, one stage
/// further: copper within four blocks (taxicab) that is less oxidized holds it back,
/// more oxidized copper makes it likelier.
fn oxidizes_now(w: &mut World, pos: BlockPos, stage: u8) -> bool {
    if w.rng.f64() >= 64.0 / 1125.0 {
        return false; // no pre-oxidation this time
    }
    let (mut nearby, mut ahead) = (0u32, 0u32);
    for dx in -4i32..=4 {
        for dy in -4i32..=4 {
            for dz in -4i32..=4 {
                let d = dx.abs() + dy.abs() + dz.abs();
                if d == 0 || d > 4 {
                    continue;
                }
                let q = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                let Some(other) = w.loaded_block(q).and_then(weathering_stage) else {
                    continue;
                };
                if other < stage {
                    return false;
                }
                nearby += 1;
                if other > stage {
                    ahead += 1;
                }
            }
        }
    }
    let c = f64::from(ahead + 1) / f64::from(nearby + 1);
    let m = if stage == 0 { 0.75 } else { 1.0 };
    w.rng.f64() < m * c * c
}

impl World {
    /// A pumpkin at `head` next to the copper block at `body`: the block becomes a
    /// copper chest and a golem stands where the pumpkin was.
    fn build_copper_golem(&mut self, head: BlockPos, pumpkin: u16, body: BlockPos, stage: u8) {
        let facing = blocks::facing(pumpkin).unwrap_or(Dir::South);
        let chest_name = format!("minecraft:{}copper_chest", stage_prefix(stage));
        let Some(chest) = BlockInfo::by_name(&chest_name) else {
            return;
        };
        let chest = chest.state_with([
            ("facing", facing.name()),
            ("type", "single"),
            ("waterlogged", "false"),
        ]);
        self.set_block_updating(head, block::AIR);
        self.remove_block_entity(body);
        self.set_block_updating(body, chest);
        if let Some(entity) = chest_storage() {
            self.block_entities.insert(body, entity);
        }
        let s = shared(self);
        let list = s.chests.entry(body.chunk()).or_default();
        if !list.contains(&body) {
            list.push(body);
        }
        self.pair_copper_chest(body, chest);
        let at = [
            f64::from(head.x) + 0.5,
            f64::from(head.y),
            f64::from(head.z) + 0.5,
        ];
        let mut mob = Mob::new(MobKind::CopperGolem, at, &mut self.rng);
        mob.yaw = yaw_of(facing);
        mob.head_yaw = mob.yaw;
        mob.kin = Some(Box::new(CopperGolem::new(stage)));
        self.mobs.insert(next_entity_id(), mob);
        self.play_sound_at("minecraft:entity.copper_golem.spawn", at, 1.0, 1.0, None);
        let builders: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| (0..3).map(|i| (p.pos[i] - at[i]).powi(2)).sum::<f64>() <= 25.0)
            .map(|(&pid, _)| pid)
            .collect();
        for pid in builders {
            self.advance(
                pid,
                super::advancements::Trigger::SummonedEntity(entity_type::COPPER_GOLEM),
            );
        }
    }

    /// A single copper chest at `pos` pairs with a single copper chest beside it facing
    /// the same way: both take the less oxidized stage, and stay waxed only if both were.
    fn pair_copper_chest(&mut self, pos: BlockPos, state: u16) {
        if blocks::prop(state, "type") != Some("single") {
            return;
        }
        let Some(facing) = blocks::facing(state) else {
            return;
        };
        let Some((waxed, stage)) = copper_chest_of(state) else {
            return;
        };
        // A "left" half has its partner clockwise of its facing.
        for (side, mine, theirs) in [
            (facing.clockwise(), "left", "right"),
            (facing.counter_clockwise(), "right", "left"),
        ] {
            let q = offset(pos, side);
            let Some(other) = self.loaded_block(q) else {
                continue;
            };
            let Some((other_waxed, other_stage)) = copper_chest_of(other) else {
                continue;
            };
            if blocks::facing(other) != Some(facing)
                || blocks::prop(other, "type") != Some("single")
            {
                continue;
            }
            let (waxed, stage) = (waxed && other_waxed, stage.min(other_stage));
            let me = copper_variant("copper_chest", waxed, stage, state);
            let them = copper_variant("copper_chest", waxed, stage, other);
            if let (Some(me), Some(them)) = (me, them) {
                ensure_storage(self, q);
                self.set_block_updating(pos, blocks::with(me, "type", mine));
                self.set_block_updating(q, blocks::with(them, "type", theirs));
            }
            return;
        }
    }

    /// The other half of the large chest at `pos`, if it is one and both halves agree.
    fn chest_partner_of(&self, pos: BlockPos, state: u16) -> Option<BlockPos> {
        let q = blocks::chest_partner(state, pos)?;
        let other = self.loaded_block(q)?;
        let same_family = if is_copper_chest(state) {
            is_copper_chest(other)
        } else {
            blocks::same_block(other, state)
        };
        let agrees = same_family
            && blocks::facing(other) == blocks::facing(state)
            && blocks::chest_partner(other, q) == Some(pos);
        agrees.then_some(q)
    }

    /// The chest at `pos` as its window shows it: the right half of a large chest first.
    fn chest_halves(&self, pos: BlockPos) -> Vec<BlockPos> {
        let Some(state) = self.loaded_block(pos) else {
            return vec![pos];
        };
        match self.chest_partner_of(pos, state) {
            Some(q) if blocks::prop(state, "type") == Some("right") => vec![pos, q],
            Some(q) => vec![q, pos],
            None => vec![pos],
        }
    }

    /// Sets the copper chest at `pos` (and its other half) to this waxing and stage.
    fn set_copper_chest(&mut self, pos: BlockPos, state: u16, waxed: bool, stage: u8) {
        let partner = self.chest_partner_of(pos, state);
        for q in std::iter::once(pos).chain(partner) {
            let Some(s) = self.loaded_block(q) else {
                continue;
            };
            if let Some(next) = copper_variant("copper_chest", waxed, stage, s) {
                self.set_block_updating(q, next);
            }
        }
    }

    /// A golem's turn. Returns whether it is busy with a chest (and walks itself).
    fn tick_copper_golem(&mut self, id: i32, mob: &mut Mob, g: &mut CopperGolem) -> bool {
        if g.statue {
            return true;
        }
        self.weather_copper_golem(id, mob, g);
        if g.weather == OXIDIZED
            && !g.waxed()
            && self.rng.f64() < STATUE_CHANCE
            && self.wholly_in_air(mob.pos)
        {
            self.turn_to_statue(id, mob, g);
            return true;
        }
        if let Some((halves, left)) = g.examining.as_mut() {
            mob.target = None;
            mob.path.clear();
            if *left <= 1 {
                let halves = std::mem::take(halves);
                g.examining = None;
                g.state = STATE_IDLE;
                self.send_to_viewers(mob, golem_data(id, g));
                self.stop_examining(id, &halves);
            } else {
                *left -= 1;
            }
            return true;
        }
        if mob.panic > 0 || mob.vehicle.is_some() {
            g.goal = None;
            return false;
        }
        if g.wander > 0 {
            g.wander -= 1;
            if g.wander == 0 {
                g.visited.clear();
            }
            return false;
        }
        let Some(goal) = g.goal else {
            if g.search_in > 0 {
                g.search_in -= 1;
                return false;
            }
            g.search_in = SEARCH_EVERY;
            g.goal = self.nearest_chest(id, mob.pos, g);
            g.travel = 0;
            // Every chest about was tried: wander, forget them and try again, so that
            // a chest refilled or emptied since is seen.
            if g.goal.is_none() && !g.visited.is_empty() {
                g.wander = WANDER_TICKS;
            }
            return false;
        };
        let wanted = g.held.is_empty();
        if !self.chest_usable(id, goal, wanted) {
            g.goal = None;
            g.failed_at(goal);
            return false;
        }
        if within_reach(feet_of(mob.pos), goal) {
            g.goal = None;
            self.examine(id, mob, g, goal);
            return true;
        }
        g.travel += 1;
        if g.travel > TRAVEL_TICKS {
            g.goal = None;
            g.failed_at(goal);
            return false;
        }
        let stand = self.place_beside(goal, mob.pos);
        // `follow_path` searches again when it has no path left or its old one is due.
        let searches = mob.path.is_empty() || mob.path_ticks <= 1;
        self.follow_path(mob, stand);
        // A path just found that stops short of the chest (the best partial path there
        // is), or the end of one reached still out of reach: no way there, and no point
        // in searching again every tick.
        let ends_short = searches && mob.path.last().is_some_and(|&end| !within_reach(end, goal));
        if ends_short || (mob.path.is_empty() && !within_reach(feet_of(mob.pos), goal)) {
            mob.target = None;
            g.goal = None;
            g.failed_at(goal);
            return false;
        }
        true
    }

    /// Where a golem stands to work the chest at `chest`: the open spot above or beside
    /// it nearest to `from`, or the chest itself if there is none.
    fn place_beside(&self, chest: BlockPos, from: [f64; 3]) -> BlockPos {
        let distance = |p: BlockPos| {
            let c = centre(p);
            (0..3).map(|i| (c[i] - from[i]).powi(2)).sum::<f64>()
        };
        [Dir::Up, Dir::North, Dir::South, Dir::East, Dir::West]
            .into_iter()
            .map(|d| offset(chest, d))
            .filter(|&q| self.loaded_block(q).is_some_and(is_air))
            .min_by(|&a, &b| distance(a).total_cmp(&distance(b)))
            .unwrap_or(chest)
    }

    /// Oxidation over game time.
    fn weather_copper_golem(&mut self, id: i32, mob: &Mob, g: &mut CopperGolem) {
        match g.next_age {
            UNROLLED => g.next_age = roll_next_age(self),
            WAXED => {}
            due if g.weather < OXIDIZED && self.tick as i64 >= due => {
                g.weather += 1;
                g.next_age = roll_next_age(self);
                self.send_to_viewers(mob, golem_data(id, g));
            }
            _ => {}
        }
    }

    /// Whether a golem's whole body at `pos` is in air, inside the world.
    fn wholly_in_air(&self, pos: [f64; 3]) -> bool {
        let half = SIZE.0 / 2.0;
        let (lo, hi) = (feet_of(pos).y, (pos[1] + SIZE.1).floor() as i32);
        if lo < MIN_Y || hi > MAX_Y {
            return false;
        }
        let xs = [
            (pos[0] - half).floor() as i32,
            (pos[0] + half).floor() as i32,
        ];
        let zs = [
            (pos[2] - half).floor() as i32,
            (pos[2] + half).floor() as i32,
        ];
        (xs[0]..=xs[1]).all(|x| {
            (zs[0]..=zs[1]).all(|z| {
                (lo..=hi).all(|y| {
                    self.loaded_block(BlockPos::new(x, y, z))
                        .is_some_and(is_air)
                })
            })
        })
    }

    fn turn_to_statue(&mut self, id: i32, mob: &mut Mob, g: &mut CopperGolem) {
        let feet = feet_of(mob.pos);
        let Some(info) = BlockInfo::by_name("minecraft:oxidized_copper_golem_statue") else {
            return;
        };
        let pose = POSES[self.rng.below(POSES.len() as u32) as usize];
        let statue = info.state_with([
            ("facing", Dir::from_yaw(mob.yaw).name()),
            ("copper_golem_pose", pose),
            ("waterlogged", "false"),
        ]);
        self.set_block_updating(feet, statue);
        self.play_sound_at(
            "minecraft:entity.copper_golem_become_statue",
            mob.pos,
            1.0,
            1.0,
            None,
        );
        let held = std::mem::replace(&mut g.held, Stack::EMPTY);
        if !held.is_empty() {
            self.scatter_item(mob.pos, held);
        }
        g.statue = true;
        g.goal = None;
        self.send_to_viewers(mob, super::items::remove_entity(id));
        let s = shared(self);
        s.statues.push(id);
        s.carriers.remove(&id);
    }

    /// A golem that held something or worked a chest: once it dies, what it held drops
    /// and the chest closes.
    fn look_after_carrier(&mut self, id: i32) {
        let Some(mob) = self.mobs.get_mut(&id) else {
            shared(self).carriers.remove(&id);
            return;
        };
        let pos = mob.pos;
        let alive = mob.alive();
        let Some(g) = mob
            .kin
            .as_mut()
            .and_then(|k| k.downcast_mut::<CopperGolem>())
        else {
            shared(self).carriers.remove(&id);
            return;
        };
        if alive {
            if g.held.is_empty() && g.examining.is_none() {
                shared(self).carriers.remove(&id);
            }
            return;
        }
        let held = std::mem::replace(&mut g.held, Stack::EMPTY);
        let open = g.examining.take().map(|(halves, _)| halves);
        g.goal = None;
        shared(self).carriers.remove(&id);
        if let Some(halves) = open {
            self.stop_examining(id, &halves);
        }
        if !held.is_empty() {
            self.scatter_item(pos, held);
        }
    }

    /// Whether the chest at `pos` is one the golem can work now: a copper chest when
    /// it wants an item, a wooden one when it has one, with storage, and no other golem
    /// at it (or at its other half).
    fn chest_usable(&self, id: i32, pos: BlockPos, copper: bool) -> bool {
        let Some(state) = self.loaded_block(pos) else {
            return false;
        };
        let kind_ok = if copper {
            is_copper_chest(state)
        } else {
            is_wooden_chest(state)
        };
        if !kind_ok || !matches!(self.block_entities.get(&pos), Some(BlockEntity::Chest(_))) {
            return false;
        }
        let busy = shared_ref(self).map(|s| &s.busy);
        let taken = |q: BlockPos| {
            busy.and_then(|b| b.get(&q))
                .is_some_and(|&(by, until)| by != id && until > self.tick)
        };
        !taken(pos) && !self.chest_partner_of(pos, state).is_some_and(taken)
    }

    /// Rebuilds the index of chests now and then from all block entities, to find
    /// chests in chunks loaded since; placed chests go in as they are placed.
    fn refresh_chest_index(&mut self) {
        if shared_ref(self).is_some_and(|s| s.index_at > self.tick) {
            return;
        }
        let mut chests: HashMap<(i32, i32), Vec<BlockPos>> = HashMap::new();
        for (&pos, entity) in &self.block_entities {
            if !matches!(entity, BlockEntity::Chest(_)) {
                continue;
            }
            let Some(state) = self.loaded_block(pos) else {
                continue;
            };
            if is_copper_chest(state) || is_wooden_chest(state) {
                chests.entry(pos.chunk()).or_default().push(pos);
            }
        }
        let next = self.tick + INDEX_EVERY;
        let s = shared(self);
        s.chests = chests;
        s.index_at = next;
    }

    /// The nearest chest it has not tried yet, within its search area.
    fn nearest_chest(&mut self, id: i32, at: [f64; 3], g: &CopperGolem) -> Option<BlockPos> {
        self.refresh_chest_index();
        let feet = feet_of(at);
        let copper = g.held.is_empty();
        let distance = |p: BlockPos| {
            let c = centre(p);
            (0..3).map(|i| (c[i] - at[i]).powi(2)).sum::<f64>()
        };
        let index = &shared_ref(self)?.chests;
        let chunks = |c: i32| ((c - SEARCH_RADIUS) >> 4)..=((c + SEARCH_RADIUS) >> 4);
        chunks(feet.x)
            .flat_map(|cx| chunks(feet.z).map(move |cz| (cx, cz)))
            .filter_map(|c| index.get(&c))
            .flatten()
            .copied()
            .filter(|p| {
                (p.x - feet.x).abs() <= SEARCH_RADIUS
                    && (p.z - feet.z).abs() <= SEARCH_RADIUS
                    && (p.y - feet.y).abs() <= SEARCH_HEIGHT
                    && !g.visited.contains(p)
            })
            .filter(|&p| self.chest_usable(id, p, copper))
            .min_by(|&a, &b| distance(a).total_cmp(&distance(b)))
    }

    /// Opens the chest at `pos` (both halves of a large one) and takes from it or puts
    /// into it.
    fn examine(&mut self, id: i32, mob: &mut Mob, g: &mut CopperGolem, pos: BlockPos) {
        mob.target = None;
        mob.path.clear();
        mob.head_yaw = yaw_towards(mob.pos, centre(pos));
        let halves = self.chest_halves(pos);
        let until = self.tick + u64::from(EXAMINE_TICKS);
        for &q in &halves {
            shared(self).busy.insert(q, (id, until));
            self.sync_lid(q);
        }
        let (state, sound) = if g.held.is_empty() {
            match self.take_from_chest(&halves) {
                Some(stack) => {
                    g.held = stack;
                    g.visited.clear();
                    self.send_to_viewers(mob, held_packet(id, stack));
                    (STATE_GETTING_ITEM, "no_item_get")
                }
                None => {
                    halves.iter().for_each(|&q| g.failed_at(q));
                    (STATE_GETTING_NO_ITEM, "no_item_no_get")
                }
            }
        } else {
            let left = self.put_in_chest(&halves, g.held);
            if left.count < g.held.count {
                g.held = left;
                g.visited.clear();
                self.send_to_viewers(mob, held_packet(id, left));
                (STATE_DROPPING_ITEM, "item_drop")
            } else {
                halves.iter().for_each(|&q| g.failed_at(q));
                (STATE_DROPPING_NO_ITEM, "item_no_drop")
            }
        };
        g.state = state;
        g.examining = Some((halves, EXAMINE_TICKS));
        self.send_to_viewers(mob, golem_data(id, g));
        let sound = format!("minecraft:entity.copper_golem.{sound}");
        self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
    }

    /// Done with a chest: its lid closes and other golems may use it.
    fn stop_examining(&mut self, id: i32, halves: &[BlockPos]) {
        for &pos in halves {
            let s = shared(self);
            if s.busy.get(&pos).is_some_and(|&(by, _)| by == id) {
                s.busy.remove(&pos);
            }
            self.sync_lid(pos);
        }
    }

    /// Tells clients how many have the chest at `pos` open (players and a golem), so
    /// the lid shows it, with the lid's sound when it opens or shuts.
    fn sync_lid(&mut self, pos: BlockPos) {
        let Some(state) = self
            .loaded_block(pos)
            .filter(|&s| is_copper_chest(s) || is_wooden_chest(s))
        else {
            shared(self).lids.remove(&pos);
            return;
        };
        let players = self
            .players
            .values()
            .filter(|p| p.open.as_ref().is_some_and(|o| views_chest(o.view, pos)))
            .count();
        let now = self.tick;
        let golems = shared_ref(self)
            .and_then(|s| s.busy.get(&pos))
            .is_some_and(|&(_, until)| until > now);
        let total = (players + usize::from(golems)).min(255) as u8;
        let wooden = is_wooden_chest(state);
        let s = shared(self);
        let lid = s.lids.entry(pos).or_insert(Lid {
            players,
            // The shared container code tells clients about players at wooden chests.
            sent: if wooden { players.min(255) as u8 } else { 0 },
        });
        let by_players = wooden && lid.players != players;
        if by_players {
            lid.sent = players.min(255) as u8;
        }
        lid.players = players;
        let sent = lid.sent;
        lid.sent = total;
        if total == 0 {
            s.lids.remove(&pos);
        }
        if total == sent {
            return;
        }
        self.broadcast(
            Writer::packet(clientbound::BLOCK_EVENT)
                .position(pos)
                .u8(BLOCK_EVENT_CHEST_VIEWERS)
                .u8(total)
                .varint(i32::from(STATE_BLOCK[usize::from(state)])),
            None,
        );
        // One sound for a large chest, and none for what the shared code sounds.
        let opened_or_shut = (sent == 0) != (total == 0);
        if opened_or_shut && !by_players && blocks::prop(state, "type") != Some("left") {
            let pitch = self.rng.f64() as f32 * 0.1 + 0.9;
            self.play_sound_at(&lid_sound(state, total > 0), centre(pos), 0.5, pitch, None);
        }
    }

    /// All slots of a chest's halves, in window order.
    fn chest_slots(&self, halves: &[BlockPos]) -> Vec<(BlockPos, usize, Stack)> {
        halves
            .iter()
            .filter_map(|&q| self.block_entities.get(&q).map(|e| (q, e)))
            .flat_map(|(q, e)| e.stacks().iter().enumerate().map(move |(i, &s)| (q, i, s)))
            .collect()
    }

    /// Takes up to 16 of the first item in a chest.
    fn take_from_chest(&mut self, halves: &[BlockPos]) -> Option<Stack> {
        let (pos, slot, stack) = self
            .chest_slots(halves)
            .into_iter()
            .find(|(_, _, s)| !s.is_empty())?;
        let n = stack.count.min(TAKE_AT_MOST);
        self.block_entities.get_mut(&pos)?.items_mut()[slot] = stack.with_count(stack.count - n);
        self.chest_changed(pos, &[slot]);
        Some(stack.with_count(n))
    }

    /// Puts `held` in a chest if it is empty or holds that item: into the frontmost slot
    /// that takes it, the rest only into empty slots after it. Returns what is left.
    fn put_in_chest(&mut self, halves: &[BlockPos], held: Stack) -> Stack {
        let slots = self.chest_slots(halves);
        let matches = slots.iter().all(|(_, _, s)| s.is_empty())
            || slots
                .iter()
                .any(|(_, _, s)| !s.is_empty() && s.item == held.item);
        if !matches {
            return held;
        }
        let max = held.max_stack();
        let takes = |s: &Stack| s.is_empty() || (s.same_kind(&held) && s.count < max);
        let Some(first) = slots.iter().position(|(_, _, s)| takes(s)) else {
            return held;
        };
        let mut left = held.count;
        let mut changed: Vec<(BlockPos, usize)> = Vec::new();
        for (n, &(pos, slot, s)) in slots.iter().enumerate().skip(first) {
            if left == 0 {
                break;
            }
            if n != first && !s.is_empty() {
                continue;
            }
            let have = if s.is_empty() { 0 } else { s.count };
            let add = left.min(max - have);
            if add <= 0 {
                continue;
            }
            if let Some(entity) = self.block_entities.get_mut(&pos) {
                entity.items_mut()[slot] = held.with_count(have + add);
                left -= add;
                changed.push((pos, slot));
            }
        }
        for &q in halves {
            let in_half: Vec<usize> = changed
                .iter()
                .filter(|(p, _)| *p == q)
                .map(|&(_, s)| s)
                .collect();
            self.chest_changed(q, &in_half);
        }
        held.with_count(left)
    }

    fn chest_changed(&mut self, pos: BlockPos, slots: &[usize]) {
        if slots.is_empty() {
            return;
        }
        self.dirty.insert(pos.chunk());
        self.redstone_dirty.push(pos);
        self.notify_viewers(pos, slots, None);
    }

    /// A player uses `held` on a golem: an empty hand takes what it carries, an axe
    /// scrapes, honeycomb waxes.
    fn use_on_copper_golem(
        &mut self,
        player: i32,
        id: i32,
        mob: &mut Mob,
        slot: usize,
        held: &str,
    ) -> bool {
        let survival = self.players.get(&player).is_some_and(|p| p.survival());
        let g = golem_mut(mob);
        if held.is_empty() {
            if g.held.is_empty() {
                return false;
            }
            let stack = std::mem::replace(&mut g.held, Stack::EMPTY);
            self.send_to_viewers(mob, held_packet(id, Stack::EMPTY));
            self.scatter_item([mob.pos[0], mob.pos[1] + 0.5, mob.pos[2]], stack);
            return true;
        }
        if item_has_tag(held, "minecraft:axes") {
            let sound = if g.waxed() {
                g.next_age = UNROLLED;
                "minecraft:item.axe.wax_off"
            } else if g.weather > 0 {
                g.weather -= 1;
                g.next_age = UNROLLED;
                "minecraft:item.axe.scrape"
            } else {
                return false;
            };
            let data = golem_data(id, g);
            self.send_to_viewers(mob, data);
            self.play_sound_at(sound, mob.pos, 1.0, 1.0, None);
            if survival {
                self.damage_item(player, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
            }
            return true;
        }
        if held == "minecraft:honeycomb" && !g.waxed() {
            g.next_age = WAXED;
            self.play_sound_at("minecraft:item.honeycomb.wax_on", mob.pos, 1.0, 1.0, None);
            self.use_up_one(player, slot);
            return true;
        }
        false
    }

    /// Takes one of the held item from a player in survival.
    fn use_up_one(&mut self, player: i32, slot: usize) {
        let Some(p) = self.players.get_mut(&player) else {
            return;
        };
        if p.survival() {
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(player, update);
        }
    }

    /// A player uses a copper chest: honeycomb waxes it, an axe while sneaking scrapes
    /// it, and otherwise it opens as a chest does.
    fn use_copper_chest(&mut self, player: i32, slot: usize, pos: BlockPos, state: u16) -> bool {
        let Some((waxed, stage)) = copper_chest_of(state) else {
            return false;
        };
        let Some(p) = self.players.get(&player) else {
            return false;
        };
        let stack = p.inventory.slots[slot];
        let (sneaking, survival) = (p.sneaking, p.survival());
        let held = ItemInfo::get(stack.item)
            .filter(|_| !stack.is_empty())
            .map_or("", |i| i.name);
        let at = centre(pos);
        if held == "minecraft:honeycomb" && !waxed {
            self.set_copper_chest(pos, state, true, stage);
            self.play_sound_at("minecraft:item.honeycomb.wax_on", at, 1.0, 1.0, None);
            self.use_up_one(player, slot);
            return true;
        }
        if sneaking && item_has_tag(held, "minecraft:axes") && (waxed || stage > 0) {
            let (sound, stage) = if waxed {
                ("minecraft:item.axe.wax_off", stage)
            } else {
                ("minecraft:item.axe.scrape", stage - 1)
            };
            self.set_copper_chest(pos, state, false, stage);
            self.play_sound_at(sound, at, 1.0, 1.0, None);
            if survival {
                self.damage_item(player, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
            }
            return true;
        }
        if sneaking && !held.is_empty() {
            return false;
        }
        self.open_copper_chest(player, pos)
    }

    /// Opens a copper chest's window, single or large; its lid opens.
    fn open_copper_chest(&mut self, player: i32, pos: BlockPos) -> bool {
        let halves = self.chest_halves(pos);
        for &q in &halves {
            ensure_storage(self, q);
        }
        // Java Edition calls them plain chests.
        let (view, window_menu, title) = match halves[..] {
            [right, left] => (
                View::DoubleChest(right, left),
                menu::GENERIC_9X6,
                "Large Chest",
            ),
            _ => (View::Chest(pos, CHEST_SLOTS), menu::GENERIC_9X3, "Chest"),
        };
        self.open_view(player, view, window_menu, title);
        for &q in &halves {
            self.sync_lid(q);
        }
        true
    }

    /// A player uses a copper golem statue: an axe scrapes it (bringing an unaffected
    /// one to life), honeycomb waxes it, anything else turns it to its next pose.
    fn use_statue(&mut self, player: i32, slot: usize, pos: BlockPos, state: u16) -> bool {
        let Some((waxed, stage)) = statue_of(state) else {
            return false;
        };
        let Some(p) = self.players.get(&player) else {
            return false;
        };
        let stack = p.inventory.slots[slot];
        let (sneaking, survival) = (p.sneaking, p.survival());
        let held = ItemInfo::get(stack.item)
            .filter(|_| !stack.is_empty())
            .map_or("", |i| i.name);
        let at = centre(pos);
        if item_has_tag(held, "minecraft:axes") {
            if waxed {
                if let Some(s) = statue_state(false, stage, state) {
                    self.set_block_updating(pos, s);
                }
                self.play_sound_at("minecraft:item.axe.wax_off", at, 1.0, 1.0, None);
            } else if stage > 0 {
                if let Some(s) = statue_state(false, stage - 1, state) {
                    self.set_block_updating(pos, s);
                }
                self.play_sound_at("minecraft:item.axe.scrape", at, 1.0, 1.0, None);
            } else {
                self.reanimate_statue(pos, state);
            }
            if survival {
                self.damage_item(player, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
            }
            return true;
        }
        if held == "minecraft:honeycomb" && !waxed {
            if let Some(s) = statue_state(true, stage, state) {
                self.set_block_updating(pos, s);
            }
            self.play_sound_at("minecraft:item.honeycomb.wax_on", at, 1.0, 1.0, None);
            self.use_up_one(player, slot);
            return true;
        }
        if sneaking && !held.is_empty() {
            return false;
        }
        let pose = blocks::prop(state, "copper_golem_pose").unwrap_or(POSES[0]);
        let next = POSES[(POSES.iter().position(|&p| p == pose).unwrap_or(0) + 1) % POSES.len()];
        self.set_block_updating(pos, blocks::with(state, "copper_golem_pose", next));
        true
    }

    /// An unaffected unwaxed statue scraped with an axe comes to life.
    fn reanimate_statue(&mut self, pos: BlockPos, state: u16) {
        self.remove_block_entity(pos);
        self.set_block_updating(pos, block::AIR);
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let mut mob = Mob::new(MobKind::CopperGolem, at, &mut self.rng);
        mob.yaw = yaw_of(blocks::facing(state).unwrap_or(Dir::South));
        mob.head_yaw = mob.yaw;
        mob.kin = Some(Box::new(CopperGolem::new(0)));
        self.mobs.insert(next_entity_id(), mob);
        self.play_sound_at("minecraft:item.axe.scrape", at, 1.0, 1.0, None);
        self.play_sound_at("minecraft:entity.copper_golem.spawn", at, 1.0, 1.0, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-copper-golems-{}-{}",
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
            .insert(alice, Player::new(profile, out, [-12.5, -60.0, -12.5]));
        (w, alice)
    }

    fn state_of(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    fn golem(w: &mut World, at: [f64; 3], weather: u8) -> (i32, Mob) {
        let mut mob = Mob::new(MobKind::CopperGolem, at, &mut w.rng);
        mob.kin = Some(Box::new(CopperGolem::new(weather)));
        (next_entity_id(), mob)
    }

    fn g(mob: &Mob) -> &CopperGolem {
        golem_of(mob).expect("copper golem state")
    }

    fn stack(name: &str, count: i32) -> Stack {
        Stack::new(ItemInfo::id_of(name).unwrap(), count)
    }

    fn hold(w: &mut World, id: i32, item: Stack) {
        w.players.get_mut(&id).unwrap().inventory.slots[36] = item;
    }

    fn chest(w: &mut World, pos: BlockPos, name: &str, items: &[(usize, Stack)]) {
        w.set_block(pos, state_of(name));
        let mut entity = chest_storage().unwrap();
        for &(slot, s) in items {
            entity.items_mut()[slot] = s;
        }
        w.block_entities.insert(pos, entity);
    }

    fn items(w: &World, pos: BlockPos) -> Vec<Stack> {
        w.block_entities[&pos].stacks().to_vec()
    }

    fn ticks(w: &mut World, id: i32, mob: &mut Mob, n: u32, done: impl Fn(&Mob) -> bool) {
        for _ in 0..n {
            tick(w, id, mob);
            if done(mob) {
                break;
            }
        }
    }

    #[test]
    fn a_pumpkin_on_copper_makes_a_golem_and_a_copper_chest() {
        let (mut w, _) = world_with_player();
        let body = BlockPos::new(5, -60, 5);
        let head = offset(body, Dir::Up);
        w.set_block(body, state_of("minecraft:waxed_weathered_copper"));
        let pumpkin = blocks::with(state_of("minecraft:carved_pumpkin"), "facing", "east");
        w.set_block(head, pumpkin);
        block_placed(&mut w, 0, head, pumpkin);
        let mob = w
            .mobs
            .values()
            .find(|m| m.kind == MobKind::CopperGolem)
            .expect("a golem");
        assert_eq!(g(mob).weather, 2);
        assert!(!g(mob).waxed(), "never waxed");
        assert_eq!(mob.pos, [5.5, -59.0, 5.5]);
        assert!(is_air(w.block(head)));
        let chest = w.block(body);
        assert_eq!(blocks::name(chest), "minecraft:weathered_copper_chest");
        assert_eq!(blocks::prop(chest, "facing"), Some("east"));
        assert!(matches!(
            w.block_entities.get(&body),
            Some(BlockEntity::Chest(_))
        ));
        // A pumpkin beside an unaffected block works too; one on stone does not.
        let side = BlockPos::new(9, -60, 5);
        w.set_block(side, state_of("minecraft:copper_block"));
        let head = offset(side, Dir::West);
        w.set_block(head, pumpkin);
        block_placed(&mut w, 0, head, pumpkin);
        assert_eq!(
            blocks::name(w.block(side)),
            "minecraft:copper_chest",
            "built sideways"
        );
        let golems = w
            .mobs
            .values()
            .filter(|m| m.kind == MobKind::CopperGolem)
            .count();
        assert_eq!(golems, 2);
        let stone = BlockPos::new(12, -60, 5);
        w.set_block(stone, block::STONE);
        w.set_block(offset(stone, Dir::Up), pumpkin);
        block_placed(&mut w, 0, offset(stone, Dir::Up), pumpkin);
        assert_eq!(w.block(offset(stone, Dir::Up)), pumpkin);
    }

    #[test]
    fn copper_chests_pair_into_the_less_oxidized_stage_and_part_when_broken() {
        let (mut w, alice) = world_with_player();
        let a = BlockPos::new(-11, -60, -12);
        let b = offset(a, Dir::North.clockwise());
        let first = blocks::with(
            state_of("minecraft:waxed_oxidized_copper_chest"),
            "facing",
            "north",
        );
        let second = blocks::with(
            state_of("minecraft:exposed_copper_chest"),
            "facing",
            "north",
        );
        w.set_block(a, first);
        block_placed(&mut w, alice, a, first);
        assert_eq!(blocks::prop(w.block(a), "type"), Some("single"));
        w.set_block(b, second);
        block_placed(&mut w, alice, b, second);
        let (sa, sb) = (w.block(a), w.block(b));
        assert_eq!(blocks::name(sa), "minecraft:exposed_copper_chest");
        assert_eq!(blocks::name(sb), "minecraft:exposed_copper_chest");
        assert_eq!(w.chest_partner_of(a, sa), Some(b));
        assert_eq!(w.chest_partner_of(b, sb), Some(a));
        // Opening either half shows both, and the lid opens.
        hold(&mut w, alice, Stack::EMPTY);
        assert!(block_used(&mut w, alice, 36, a, sa));
        let view = w.players[&alice].open.as_ref().map(|o| o.view);
        assert!(matches!(view, Some(View::DoubleChest(..))), "{view:?}");
        assert_eq!(shared(&mut w).lids[&a].sent, 1);
        // Honeycomb waxes both halves; a sneaking axe scrapes the wax off both.
        w.players.get_mut(&alice).unwrap().open = None;
        hold(&mut w, alice, stack("minecraft:honeycomb", 1));
        assert!(block_used(&mut w, alice, 36, a, sa));
        assert_eq!(copper_chest_of(w.block(b)), Some((true, 1)));
        hold(&mut w, alice, stack("minecraft:iron_axe", 1));
        w.players.get_mut(&alice).unwrap().sneaking = true;
        let s = w.block(a);
        assert!(block_used(&mut w, alice, 36, a, s));
        assert_eq!(copper_chest_of(w.block(b)), Some((false, 1)));
        let s = w.block(a);
        assert!(block_used(&mut w, alice, 36, a, s));
        assert_eq!(copper_chest_of(w.block(a)), Some((false, 0)));
        assert_eq!(copper_chest_of(w.block(b)), Some((false, 0)));
        let s = w.block(b);
        block_broken(&mut w, alice, b, s);
        assert_eq!(blocks::prop(w.block(a), "type"), Some("single"));
        // Sneaking keeps a placed chest single.
        let c = offset(a, Dir::North.counter_clockwise());
        let third = blocks::with(state_of("minecraft:copper_chest"), "facing", "north");
        w.set_block(c, third);
        block_placed(&mut w, alice, c, third);
        assert_eq!(blocks::prop(w.block(c), "type"), Some("single"));
    }

    #[test]
    fn copper_chests_open_and_close_their_lid() {
        let (mut w, alice) = world_with_player();
        let pos = BlockPos::new(-11, -60, -12);
        let state = state_of("minecraft:exposed_copper_chest");
        w.set_block(pos, state);
        block_placed(&mut w, alice, pos, state);
        assert!(matches!(
            w.block_entities.get(&pos),
            Some(BlockEntity::Chest(_))
        ));
        assert!(block_used(&mut w, alice, 36, pos, state));
        assert!(w.players[&alice].open.is_some(), "the chest window opened");
        assert_eq!(shared(&mut w).lids[&pos].sent, 1);
        w.players.get_mut(&alice).unwrap().open = None;
        tick_world(&mut w);
        assert!(!shared(&mut w).lids.contains_key(&pos), "closed");
    }

    #[test]
    fn copper_chests_oxidize_on_random_ticks_unless_waxed() {
        let (mut w, _) = world_with_player();
        let pos = BlockPos::new(0, -60, 0);
        let state = state_of("minecraft:copper_chest");
        w.set_block(pos, state);
        assert!(ticks_randomly(state));
        assert!(!ticks_randomly(state_of("minecraft:waxed_copper_chest")));
        for _ in 0..2_000 {
            let s = w.block(pos);
            assert!(block_tick(&mut w, pos, s));
            if copper_chest_of(w.block(pos)) != Some((false, 0)) {
                break;
            }
        }
        assert_eq!(copper_chest_of(w.block(pos)), Some((false, 1)));
    }

    #[test]
    fn golems_oxidize_over_time_and_wax_stops_it() {
        let (mut w, _) = world_with_player();
        let (id, mut mob) = golem(&mut w, [0.5, -60.0, 0.5], 0);
        tick(&mut w, id, &mut mob);
        let due = g(&mob).next_age;
        let now = w.tick as i64;
        assert!((now + 504_000..=now + 552_000).contains(&due), "{due}");
        w.tick = due as u64;
        tick(&mut w, id, &mut mob);
        assert_eq!(g(&mob).weather, 1);
        assert!(g(&mob).next_age > due);
        golem_mut(&mut mob).next_age = WAXED;
        w.tick += 1_000_000;
        tick(&mut w, id, &mut mob);
        assert_eq!(g(&mob).weather, 1, "waxed golems stay as they are");
    }

    #[test]
    fn oxidized_golems_turn_to_statues_unless_waxed() {
        let (mut w, _) = world_with_player();
        let (id, mut mob) = golem(&mut w, [0.5, -60.0, 0.5], OXIDIZED);
        golem_mut(&mut mob).held = stack("minecraft:cobblestone", 5);
        ticks(&mut w, id, &mut mob, 5_000, |m| g(m).statue);
        assert!(g(&mob).statue, "turned to stone");
        let statue = w.block(BlockPos::new(0, -60, 0));
        assert_eq!(statue_of(statue), Some((false, OXIDIZED)));
        assert!(w.items.values().any(|i| i.stack().count == 5), "dropped");
        w.mobs.insert(id, mob);
        tick_world(&mut w);
        assert!(!w.mobs.contains_key(&id), "the golem is gone");
        let (id, mut waxed) = golem(&mut w, [2.5, -60.0, 0.5], OXIDIZED);
        golem_mut(&mut waxed).next_age = WAXED;
        for _ in 0..5_000 {
            tick(&mut w, id, &mut waxed);
        }
        assert!(!g(&waxed).statue);
    }

    #[test]
    fn golems_sort_items_from_copper_chests_into_matching_chests() {
        let (mut w, _) = world_with_player();
        let copper = BlockPos::new(3, -60, 0);
        let odd = BlockPos::new(1, -60, 0);
        let good = BlockPos::new(1, -60, 1);
        let cobble = stack("minecraft:cobblestone", 20);
        chest(&mut w, copper, "minecraft:copper_chest", &[(3, cobble)]);
        chest(
            &mut w,
            odd,
            "minecraft:chest",
            &[(0, stack("minecraft:dirt", 1))],
        );
        chest(
            &mut w,
            good,
            "minecraft:trapped_chest",
            &[
                (0, stack("minecraft:cobblestone", 64)),
                (1, stack("minecraft:cobblestone", 60)),
                (4, stack("minecraft:cobblestone", 1)),
            ],
        );
        let (id, mut mob) = golem(&mut w, [2.5, -60.0, 0.5], 0);
        ticks(&mut w, id, &mut mob, 200, |m| !g(m).held.is_empty());
        assert_eq!(g(&mob).held.count, 16);
        assert_eq!(g(&mob).state, STATE_GETTING_ITEM);
        assert_eq!(items(&w, copper)[3].count, 4);
        // The dirt chest is tried first and refused; the other takes it all.
        ticks(&mut w, id, &mut mob, 600, |m| g(m).held.is_empty());
        assert!(g(&mob).held.is_empty(), "put away");
        assert_eq!(items(&w, odd)[0].count, 1);
        assert_eq!(items(&w, odd)[1].count, 0);
        let good = items(&w, good);
        assert_eq!(good[1].count, 64, "the frontmost slot that takes them");
        assert_eq!(good[2].count, 12, "then empty slots");
        assert_eq!(good[4].count, 1, "later stacks are left alone");
        assert!(g(&mob).visited.is_empty(), "forgotten after a success");
    }

    #[test]
    fn a_large_chest_counts_as_one() {
        let (mut w, _) = world_with_player();
        let left = BlockPos::new(1, -60, 0);
        let right = BlockPos::new(1, -60, 1);
        let facing_west = |name: &str, kind: &str| {
            blocks::with(blocks::with(state_of(name), "facing", "west"), "type", kind)
        };
        // Facing west, a left half has its partner to the north... or south: ask.
        let (l, r) = if offset(left, Dir::West.clockwise()) == right {
            ("left", "right")
        } else {
            ("right", "left")
        };
        w.set_block(left, facing_west("minecraft:chest", l));
        w.set_block(right, facing_west("minecraft:chest", r));
        for (pos, items) in [
            (left, vec![]),
            (right, vec![stack("minecraft:cobblestone", 1)]),
        ] {
            let mut entity = chest_storage().unwrap();
            for (i, s) in items.into_iter().enumerate() {
                entity.items_mut()[i] = s;
            }
            w.block_entities.insert(pos, entity);
        }
        let halves = w.chest_halves(left);
        assert_eq!(halves.len(), 2);
        let (id, mut mob) = golem(&mut w, [2.5, -60.0, 0.5], 0);
        golem_mut(&mut mob).held = stack("minecraft:cobblestone", 3);
        golem_mut(&mut mob).goal = Some(left);
        tick(&mut w, id, &mut mob);
        assert!(g(&mob).held.is_empty(), "the other half holds cobblestone");
        let total: i32 = [left, right]
            .iter()
            .flat_map(|&p| items(&w, p))
            .filter(|s| !s.is_empty())
            .map(|s| s.count)
            .sum();
        assert_eq!(total, 4);
        let s = shared(&mut w);
        assert!(s.busy.contains_key(&left) && s.busy.contains_key(&right));
    }

    #[test]
    fn a_golem_that_tried_every_chest_tries_again_later() {
        let (mut w, _) = world_with_player();
        let copper = BlockPos::new(3, -60, 0);
        chest(&mut w, copper, "minecraft:copper_chest", &[]);
        let (id, mut mob) = golem(&mut w, [2.5, -60.0, 0.5], 0);
        ticks(&mut w, id, &mut mob, 200, |m| !g(m).visited.is_empty());
        assert_eq!(g(&mob).visited, vec![copper]);
        // Refilled: it comes back once it has wandered.
        w.block_entities.get_mut(&copper).unwrap().items_mut()[0] = stack("minecraft:stone", 2);
        ticks(&mut w, id, &mut mob, 600, |m| !g(m).held.is_empty());
        assert_eq!(g(&mob).held.count, 2);
    }

    #[test]
    fn an_unreachable_chest_is_given_up_at_once() {
        let (mut w, _) = world_with_player();
        // Walled in on all sides and above, in a pit below the golem's reach.
        let pos = BlockPos::new(8, -63, 8);
        chest(&mut w, pos, "minecraft:copper_chest", &[]);
        let (id, mut mob) = golem(&mut w, [0.5, -60.0, 0.5], 0);
        golem_mut(&mut mob).goal = Some(pos);
        tick(&mut w, id, &mut mob);
        assert_eq!(g(&mob).goal, None);
        assert_eq!(g(&mob).visited, vec![pos]);
    }

    #[test]
    fn two_golems_never_share_a_chest_and_ten_failures_send_one_wandering() {
        let (mut w, _) = world_with_player();
        let copper = BlockPos::new(3, -60, 0);
        chest(
            &mut w,
            copper,
            "minecraft:copper_chest",
            &[(0, stack("minecraft:stone", 64))],
        );
        let (a, mut first) = golem(&mut w, [2.5, -60.0, 0.5], 0);
        let (b, mut second) = golem(&mut w, [2.5, -60.0, 1.5], 0);
        tick(&mut w, a, &mut first);
        tick(&mut w, a, &mut first);
        assert!(g(&first).examining.is_some());
        tick(&mut w, b, &mut second);
        assert_eq!(g(&second).goal, None, "the chest is taken");
        let mut lonely = CopperGolem::new(0);
        for x in 0..10 {
            lonely.failed_at(BlockPos::new(x, 0, 0));
        }
        assert_eq!(lonely.wander, WANDER_TICKS);
    }

    #[test]
    fn a_dead_golem_drops_what_it_holds_and_lets_go_of_the_chest() {
        let (mut w, _) = world_with_player();
        let copper = BlockPos::new(3, -60, 0);
        chest(
            &mut w,
            copper,
            "minecraft:copper_chest",
            &[(0, stack("minecraft:stone", 9))],
        );
        let (id, mut mob) = golem(&mut w, [2.5, -60.0, 0.5], 0);
        ticks(&mut w, id, &mut mob, 50, |m| g(m).examining.is_some());
        assert_eq!(g(&mob).held.count, 9);
        w.kill_mob(id, &mut mob);
        w.mobs.insert(id, mob);
        tick_world(&mut w);
        assert!(w.items.values().any(|i| i.stack().count == 9), "dropped");
        assert!(g(&w.mobs[&id]).held.is_empty());
        assert!(!shared(&mut w).busy.contains_key(&copper));
        assert!(!shared(&mut w).lids.contains_key(&copper), "lid shut");
    }

    #[test]
    fn players_take_items_scrape_and_wax_golems() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = golem(&mut w, [-11.5, -60.0, -12.5], 2);
        golem_mut(&mut mob).held = stack("minecraft:dirt", 3);
        w.mobs.insert(id, mob);
        assert!(use_on(&mut w, alice, id, 36, ""));
        assert!(g(&w.mobs[&id]).held.is_empty());
        assert!(w.items.values().any(|i| i.stack().count == 3));
        assert!(!use_on(&mut w, alice, id, 36, ""), "nothing left to take");
        hold(&mut w, alice, stack("minecraft:honeycomb", 2));
        assert!(use_on(&mut w, alice, id, 36, "minecraft:honeycomb"));
        assert!(g(&w.mobs[&id]).waxed());
        assert_eq!(w.players[&alice].inventory.slots[36].count, 1);
        hold(&mut w, alice, stack("minecraft:iron_axe", 1));
        assert!(use_on(&mut w, alice, id, 36, "minecraft:iron_axe"));
        assert!(!g(&w.mobs[&id]).waxed(), "wax first");
        assert_eq!(g(&w.mobs[&id]).weather, 2);
        assert!(use_on(&mut w, alice, id, 36, "minecraft:iron_axe"));
        assert_eq!(g(&w.mobs[&id]).weather, 1);
        assert!(w.players[&alice].inventory.slots[36].damage > 0);
        // Lightning scrapes a stage too.
        let mut mob = w.mobs.remove(&id).unwrap();
        let dealt = damage_taken(&mut w, id, &mut mob, 5.0, "minecraft:lightning_bolt", None);
        assert_eq!(dealt, 5.0);
        assert_eq!(g(&mob).weather, 0);
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 5.0, "minecraft:fall", None),
            0.0
        );
    }

    #[test]
    fn statues_change_pose_wax_scrape_and_come_back_to_life() {
        let (mut w, alice) = world_with_player();
        let pos = BlockPos::new(-11, -60, -12);
        let oxidized = state_of("minecraft:oxidized_copper_golem_statue");
        w.set_block(pos, oxidized);
        hold(&mut w, alice, Stack::EMPTY);
        assert!(block_used(&mut w, alice, 36, pos, oxidized));
        assert_eq!(
            blocks::prop(w.block(pos), "copper_golem_pose"),
            Some("sitting")
        );
        hold(&mut w, alice, stack("minecraft:honeycomb", 1));
        let s = w.block(pos);
        assert!(block_used(&mut w, alice, 36, pos, s));
        assert_eq!(statue_of(w.block(pos)), Some((true, OXIDIZED)));
        assert!(!ticks_randomly(w.block(pos)), "waxed statues stay");
        hold(&mut w, alice, stack("minecraft:stone_axe", 1));
        for expected in [(false, 3), (false, 2), (false, 1), (false, 0)] {
            let s = w.block(pos);
            assert!(block_used(&mut w, alice, 36, pos, s));
            assert_eq!(statue_of(w.block(pos)), Some(expected));
        }
        assert_eq!(
            blocks::prop(w.block(pos), "copper_golem_pose"),
            Some("sitting")
        );
        let s = w.block(pos);
        assert!(block_used(&mut w, alice, 36, pos, s));
        assert!(is_air(w.block(pos)));
        let mob = w
            .mobs
            .values()
            .find(|m| m.kind == MobKind::CopperGolem)
            .expect("brought to life");
        assert_eq!(g(mob).weather, 0);
    }

    #[test]
    fn lone_statues_oxidize_on_random_ticks() {
        let (mut w, _) = world_with_player();
        let pos = BlockPos::new(0, -60, 0);
        let statue = state_of("minecraft:copper_golem_statue");
        w.set_block(pos, statue);
        assert!(ticks_randomly(statue));
        for _ in 0..2_000 {
            let s = w.block(pos);
            assert!(block_tick(&mut w, pos, s));
            if statue_of(w.block(pos)) != Some((false, 0)) {
                break;
            }
        }
        assert_eq!(statue_of(w.block(pos)), Some((false, 1)));
        // Less oxidized copper close by holds it back.
        w.set_block(BlockPos::new(2, -60, 0), state_of("minecraft:copper_block"));
        for _ in 0..2_000 {
            let s = w.block(pos);
            block_tick(&mut w, pos, s);
        }
        assert_eq!(statue_of(w.block(pos)), Some((false, 1)));
        assert_eq!(weathering_stage(state_of("minecraft:cut_copper")), Some(0));
        assert_eq!(
            weathering_stage(state_of("minecraft:waxed_cut_copper")),
            None
        );
        assert_eq!(weathering_stage(state_of("minecraft:copper_ore")), None);
    }

    #[test]
    fn golems_are_saved_and_read_back_as_vanilla() {
        let (mut w, _) = world_with_player();
        let (_, mut mob) = golem(&mut w, [0.5, -60.0, 0.5], 1);
        golem_mut(&mut mob).next_age = WAXED;
        golem_mut(&mut mob).held = stack("minecraft:cobblestone", 7);
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        assert_eq!(nbt.str("weather_state"), Some("exposed"));
        assert_eq!(nbt.int("next_weather_age"), Some(WAXED));
        let hand = nbt
            .compound("equipment")
            .unwrap()
            .compound("mainhand")
            .unwrap();
        assert_eq!(hand.str("id"), Some("minecraft:cobblestone"));
        let mut back = Mob::new(MobKind::CopperGolem, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        assert_eq!(g(&back).weather, 1);
        assert!(g(&back).waxed());
        assert_eq!(g(&back).held.count, 7);
        assert_eq!(spawn_extras(1, &back).len(), 2);
        let mut bare = Mob::new(MobKind::CopperGolem, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut bare, &Compound::new());
        assert_eq!(*g(&bare), CopperGolem::new(0));
        assert_eq!(sound_name(&bare), "copper_golem");
        assert!(!may_love(&w, &bare));
    }
}
