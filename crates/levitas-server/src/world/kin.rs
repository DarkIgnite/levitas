//! Dispatch to mobs whose behaviour lives in a module of its own: goats, frogs (and
//! tadpoles), llamas, dolphins and pandas.
//!
//! Each module provides the same hooks, called from the shared mob code:
//! - `spawned(w, mob, biome)`: a mob generated with a chunk, in `biome`;
//! - `tick(w, id, mob) -> bool`: its own turn; `true` keeps it from wandering this tick;
//! - `use_on(w, player, target, slot, held) -> bool`: a player uses `held` on it;
//! - `hurt(w, id, mob, attacker)`: it was hurt, by player `attacker` if any;
//! - `spawn_extras(id, mob)`: entity data showing its state to a new viewer;
//! - `write_nbt` / `read_nbt`: its own saved tags;
//! - `block_tick(w, pos, state) -> bool`: random ticks of its blocks (frogspawn);
//! - `may_love(w, mob) -> bool` (pandas) and `bred(w, id, mob) -> bool` (frogs).
//!
//! A module keeps state of its own in `Mob::kin`, downcast to its type.

use levitas_nbt::Compound;

use super::mobs::{Mob, MobKind};
use super::*;

pub(super) fn spawned(w: &mut World, mob: &mut Mob, biome: &str) {
    match mob.kind {
        MobKind::Goat => super::goats::spawned(w, mob, biome),
        MobKind::Frog | MobKind::Tadpole => super::frogs::spawned(w, mob, biome),
        MobKind::Llama | MobKind::TraderLlama => super::llamas::spawned(w, mob, biome),
        MobKind::Dolphin => super::dolphins::spawned(w, mob, biome),
        MobKind::Panda => super::pandas::spawned(w, mob, biome),
        _ => {}
    }
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    match mob.kind {
        MobKind::Goat => super::goats::tick(w, id, mob),
        MobKind::Frog | MobKind::Tadpole => super::frogs::tick(w, id, mob),
        MobKind::Llama | MobKind::TraderLlama => super::llamas::tick(w, id, mob),
        MobKind::Dolphin => super::dolphins::tick(w, id, mob),
        MobKind::Panda => super::pandas::tick(w, id, mob),
        _ => false,
    }
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    let Some(kind) = w.mobs.get(&target).map(|m| m.kind) else {
        return false;
    };
    match kind {
        MobKind::Goat => super::goats::use_on(w, player, target, slot, held),
        MobKind::Frog | MobKind::Tadpole => super::frogs::use_on(w, player, target, slot, held),
        MobKind::Llama | MobKind::TraderLlama => {
            super::llamas::use_on(w, player, target, slot, held)
        }
        MobKind::Dolphin => super::dolphins::use_on(w, player, target, slot, held),
        MobKind::Panda => super::pandas::use_on(w, player, target, slot, held),
        _ => false,
    }
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, attacker: Option<i32>) {
    match mob.kind {
        MobKind::Goat => super::goats::hurt(w, id, mob, attacker),
        MobKind::Frog | MobKind::Tadpole => super::frogs::hurt(w, id, mob, attacker),
        MobKind::Llama | MobKind::TraderLlama => super::llamas::hurt(w, id, mob, attacker),
        MobKind::Dolphin => super::dolphins::hurt(w, id, mob, attacker),
        MobKind::Panda => super::pandas::hurt(w, id, mob, attacker),
        _ => {}
    }
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    match mob.kind {
        MobKind::Goat => super::goats::spawn_extras(id, mob),
        MobKind::Frog | MobKind::Tadpole => super::frogs::spawn_extras(id, mob),
        MobKind::Llama | MobKind::TraderLlama => super::llamas::spawn_extras(id, mob),
        MobKind::Dolphin => super::dolphins::spawn_extras(id, mob),
        MobKind::Panda => super::pandas::spawn_extras(id, mob),
        _ => Vec::new(),
    }
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    match mob.kind {
        MobKind::Goat => super::goats::write_nbt(mob, nbt),
        MobKind::Frog | MobKind::Tadpole => super::frogs::write_nbt(mob, nbt),
        MobKind::Llama | MobKind::TraderLlama => super::llamas::write_nbt(mob, nbt),
        MobKind::Dolphin => super::dolphins::write_nbt(mob, nbt),
        MobKind::Panda => super::pandas::write_nbt(mob, nbt),
        _ => {}
    }
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    match mob.kind {
        MobKind::Goat => super::goats::read_nbt(mob, nbt),
        MobKind::Frog | MobKind::Tadpole => super::frogs::read_nbt(mob, nbt),
        MobKind::Llama | MobKind::TraderLlama => super::llamas::read_nbt(mob, nbt),
        MobKind::Dolphin => super::dolphins::read_nbt(mob, nbt),
        MobKind::Panda => super::pandas::read_nbt(mob, nbt),
        _ => {}
    }
}

/// Whether an adult fed its food may fall in love now (pandas need bamboo about).
pub(super) fn may_love(w: &World, mob: &Mob) -> bool {
    match mob.kind {
        MobKind::Panda => super::pandas::may_love(w, mob),
        _ => true,
    }
}

/// Two of them bred: returns whether the module made what comes of it itself (frogs
/// lay frogspawn), so that no baby appears.
pub(super) fn bred(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    match mob.kind {
        MobKind::Frog => super::frogs::bred(w, id, mob),
        _ => false,
    }
}

/// Random ticks of blocks these mobs own. Returns whether `state` was one of them.
pub(super) fn block_tick(w: &mut World, pos: BlockPos, state: u16) -> bool {
    super::frogs::block_tick(w, pos, state)
}

/// Whether random ticks should reach `state` for these modules.
pub fn ticks_randomly(state: u16) -> bool {
    crate::blocks::name(state) == "minecraft:frogspawn"
}
