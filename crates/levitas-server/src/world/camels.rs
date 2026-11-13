//! STUB: to be written. Hooks are described in `kin.rs`.

use levitas_nbt::Compound;

use super::mobs::Mob;
use super::*;

pub(super) fn spawned(_w: &mut World, _mob: &mut Mob, _biome: &str) {}

pub(super) fn tick(_w: &mut World, _id: i32, _mob: &mut Mob) -> bool {
    false
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

pub(super) fn hurt(_w: &mut World, _id: i32, _mob: &mut Mob, _attacker: Option<i32>) {}

pub(super) fn spawn_extras(_id: i32, _mob: &Mob) -> Vec<Writer> {
    Vec::new()
}

pub(super) fn write_nbt(_mob: &Mob, _nbt: &mut Compound) {}

pub(super) fn read_nbt(_mob: &mut Mob, _nbt: &Compound) {}

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

pub(super) fn bred(_w: &mut World, _id: i32, _mob: &mut Mob) -> bool {
    false
}

pub(super) fn tick_world(_w: &mut World) {}

pub(super) fn block_placed(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

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

pub(super) fn block_tick(_w: &mut World, _pos: BlockPos, _state: u16) -> bool {
    false
}

pub(super) fn ticks_randomly(_state: u16) -> bool {
    false
}

pub(super) fn scheduled_tick(_w: &mut World, _pos: BlockPos, _state: u16) -> bool {
    false
}
