//! Dispatch to mobs whose behaviour lives in a module of its own: goats, frogs (and
//! tadpoles), llamas, dolphins, pandas, breezes, wardens, withers and creakings.
//!
//! Each module provides the same hooks, called from the shared mob code:
//! - `spawned(w, mob, biome)`: a mob generated with a chunk, in `biome`;
//! - `tick(w, id, mob) -> bool`: its own turn; `true` keeps it from wandering this tick;
//! - `use_on(w, player, target, slot, held) -> bool`: a player uses `held` on it;
//! - `hurt(w, id, mob, attacker)`: it was hurt, by player `attacker` if any;
//! - `spawn_extras(id, mob)`: entity data showing its state to a new viewer;
//! - `write_nbt` / `read_nbt`: its own saved tags;
//! - `block_tick(w, pos, state) -> bool`: random ticks of its blocks (frogspawn);
//! - `may_love(w, mob) -> bool` (pandas) and `bred(w, id, mob) -> bool` (frogs);
//! - for breezes, wardens, withers and creakings also `damage_taken`, `tick_world`,
//!   `block_placed`, `block_broken` and `ticks_randomly`, and `own_ai` skips the shared
//!   hunting for them.
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
        MobKind::Breeze => super::breezes::spawned(w, mob, biome),
        MobKind::Warden => super::wardens::spawned(w, mob, biome),
        MobKind::Wither => super::withers::spawned(w, mob, biome),
        MobKind::Creaking => super::creakings::spawned(w, mob, biome),
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
        MobKind::Breeze => super::breezes::tick(w, id, mob),
        MobKind::Warden => super::wardens::tick(w, id, mob),
        MobKind::Wither => super::withers::tick(w, id, mob),
        MobKind::Creaking => super::creakings::tick(w, id, mob),
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
        MobKind::Breeze => super::breezes::use_on(w, player, target, slot, held),
        MobKind::Warden => super::wardens::use_on(w, player, target, slot, held),
        MobKind::Wither => super::withers::use_on(w, player, target, slot, held),
        MobKind::Creaking => super::creakings::use_on(w, player, target, slot, held),
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
        MobKind::Breeze => super::breezes::hurt(w, id, mob, attacker),
        MobKind::Warden => super::wardens::hurt(w, id, mob, attacker),
        MobKind::Wither => super::withers::hurt(w, id, mob, attacker),
        MobKind::Creaking => super::creakings::hurt(w, id, mob, attacker),
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
        MobKind::Breeze => super::breezes::spawn_extras(id, mob),
        MobKind::Warden => super::wardens::spawn_extras(id, mob),
        MobKind::Wither => super::withers::spawn_extras(id, mob),
        MobKind::Creaking => super::creakings::spawn_extras(id, mob),
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
        MobKind::Breeze => super::breezes::write_nbt(mob, nbt),
        MobKind::Warden => super::wardens::write_nbt(mob, nbt),
        MobKind::Wither => super::withers::write_nbt(mob, nbt),
        MobKind::Creaking => super::creakings::write_nbt(mob, nbt),
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
        MobKind::Breeze => super::breezes::read_nbt(mob, nbt),
        MobKind::Warden => super::wardens::read_nbt(mob, nbt),
        MobKind::Wither => super::withers::read_nbt(mob, nbt),
        MobKind::Creaking => super::creakings::read_nbt(mob, nbt),
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

/// Whether a mob in love may go on to find a partner now (pandas need bamboo about).
pub(super) fn may_mate(w: &World, mob: &mut Mob) -> bool {
    match mob.kind {
        MobKind::Panda => super::pandas::may_mate(w, mob),
        _ => true,
    }
}

/// A baby just made from `a` and `b`, before it is shown: it takes after them.
pub(super) fn baby_made(rng: &mut Rng, a: &Mob, b: &Mob, baby: &mut Mob) {
    if baby.kind == MobKind::Goat {
        super::goats::bred_baby(rng, a, b, baby);
    }
}

/// The sound family to use in place of `MobKind::sound()` (screaming goats).
pub(super) fn sound_name(mob: &Mob) -> &'static str {
    match mob.kind {
        MobKind::Goat => super::goats::sound_name(mob).unwrap_or(mob.kind.sound()),
        _ => mob.kind.sound(),
    }
}

/// Full health for mobs whose module decides it (weak pandas have less).
pub(super) fn max_health(mob: &Mob) -> Option<f32> {
    match mob.kind {
        MobKind::Panda => super::pandas::max_health(mob),
        _ => None,
    }
}

/// A scheduled block tick for these modules' blocks (frogspawn hatching).
pub(super) fn scheduled_tick(w: &mut World, pos: BlockPos) {
    super::frogs::hatch(w, pos);
}

/// Random ticks of blocks these mobs own. Returns whether `state` was one of them.
pub(super) fn block_tick(w: &mut World, pos: BlockPos, state: u16) -> bool {
    super::frogs::block_tick(w, pos, state)
        || super::breezes::block_tick(w, pos, state)
        || super::wardens::block_tick(w, pos, state)
        || super::withers::block_tick(w, pos, state)
        || super::creakings::block_tick(w, pos, state)
}

/// Whether random ticks should reach `state` for these modules.
pub fn ticks_randomly(state: u16) -> bool {
    crate::blocks::name(state) == "minecraft:frogspawn"
        || super::breezes::ticks_randomly(state)
        || super::wardens::ticks_randomly(state)
        || super::withers::ticks_randomly(state)
        || super::creakings::ticks_randomly(state)
}

/// Whether the module runs this monster's whole AI itself: the shared hunting is
/// skipped (its `tick` decides where it goes and what it attacks).
pub fn own_ai(kind: MobKind) -> bool {
    matches!(
        kind,
        MobKind::Breeze | MobKind::Warden | MobKind::Wither | MobKind::Creaking
    )
}

/// The damage a mob actually takes from `damage` of `damage_type` (before armour and
/// effects): 0 cancels the hit (a creaking whose heart lives, a breeze deflecting a
/// projectile, a wither's armour against arrows).
pub(super) fn damage_taken(
    w: &mut World,
    id: i32,
    mob: &mut Mob,
    damage: f32,
    damage_type: &str,
    attacker: Option<i32>,
) -> f32 {
    match mob.kind {
        MobKind::Breeze => super::breezes::damage_taken(w, id, mob, damage, damage_type, attacker),
        MobKind::Warden => super::wardens::damage_taken(w, id, mob, damage, damage_type, attacker),
        MobKind::Wither => super::withers::damage_taken(w, id, mob, damage, damage_type, attacker),
        MobKind::Creaking => {
            super::creakings::damage_taken(w, id, mob, damage, damage_type, attacker)
        }
        _ => damage,
    }
}

/// Once a tick, for what a module keeps outside its mobs (projectiles in flight,
/// shriekers and warning levels, creaking hearts). Module-wide state lives in
/// `World::kin_state`, under the module's name.
pub(super) fn tick_world(w: &mut World) {
    super::breezes::tick_world(w);
    super::wardens::tick_world(w);
    super::withers::tick_world(w);
    super::creakings::tick_world(w);
}

/// A player placed a block (building a wither).
pub(super) fn block_placed(w: &mut World, player: i32, pos: BlockPos, state: u16) {
    super::withers::block_placed(w, player, pos, state);
    super::breezes::block_placed(w, player, pos, state);
    super::wardens::block_placed(w, player, pos, state);
    super::creakings::block_placed(w, player, pos, state);
}

/// A player broke a block (a creaking heart, sculk).
pub(super) fn block_broken(w: &mut World, player: i32, pos: BlockPos, state: u16) {
    super::creakings::block_broken(w, player, pos, state);
    super::wardens::block_broken(w, player, pos, state);
    super::withers::block_broken(w, player, pos, state);
    super::breezes::block_broken(w, player, pos, state);
}
