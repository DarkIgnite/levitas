//! Which blocks burn, and where fire can stay.
//!
//! Flammability follows the Minecraft Wiki's fire page: each flammable block has odds of
//! catching fire from a nearby flame (encouragement) and of burning away (flammability).

use levitas_protocol::BlockPos;
use levitas_registry::{block, block_has_tag, BlockInfo};
use levitas_world::is_air;

use super::{name, offset, sturdy, with_bool, Dir};

/// How readily a block catches fire from flames nearby, and how quickly it burns away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flammability {
    pub ignite: u32,
    pub burn: u32,
}

const fn odds(ignite: u32, burn: u32) -> Option<Flammability> {
    Some(Flammability { ignite, burn })
}

/// How `state` burns, or `None` for blocks fire cannot touch.
pub fn flammability(state: u16) -> Option<Flammability> {
    let n = name(state);
    // Nether wood never burns.
    if n.contains("crimson") || n.contains("warped") {
        return None;
    }
    let tag = |t: &str| block_has_tag(state, t);
    if tag("minecraft:logs_that_burn") || tag("minecraft:bamboo_blocks") {
        return odds(5, 5);
    }
    if tag("minecraft:planks")
        || tag("minecraft:wooden_slabs")
        || tag("minecraft:wooden_stairs")
        || tag("minecraft:wooden_fences")
        || tag("minecraft:fence_gates")
    {
        return odds(5, 20);
    }
    if tag("minecraft:leaves") || tag("minecraft:wool") {
        return odds(30, 60);
    }
    if tag("minecraft:wool_carpets") {
        return odds(60, 20);
    }
    if tag("minecraft:flowers") || tag("minecraft:small_flowers") {
        return odds(60, 100);
    }
    match n {
        "minecraft:short_grass"
        | "minecraft:tall_grass"
        | "minecraft:fern"
        | "minecraft:large_fern"
        | "minecraft:dead_bush"
        | "minecraft:bush"
        | "minecraft:red_shrub"
        | "minecraft:short_dry_grass"
        | "minecraft:tall_dry_grass"
        | "minecraft:firefly_bush"
        | "minecraft:leaf_litter"
        | "minecraft:sweet_berry_bush"
        | "minecraft:spore_blossom" => odds(60, 100),
        "minecraft:bamboo" | "minecraft:scaffolding" => odds(60, 60),
        "minecraft:hay_block" => odds(60, 20),
        "minecraft:bookshelf" | "minecraft:chiseled_bookshelf" | "minecraft:lectern" => {
            odds(30, 20)
        }
        "minecraft:dried_kelp_block"
        | "minecraft:azalea"
        | "minecraft:flowering_azalea"
        | "minecraft:hanging_roots" => odds(30, 60),
        "minecraft:tnt" => odds(15, 100),
        "minecraft:vine" | "minecraft:glow_lichen" => odds(15, 100),
        "minecraft:cave_vines" | "minecraft:cave_vines_plant" => odds(15, 60),
        "minecraft:big_dripleaf" | "minecraft:small_dripleaf" => odds(15, 100),
        "minecraft:target" => odds(15, 20),
        "minecraft:coal_block" => odds(5, 5),
        "minecraft:composter" | "minecraft:beehive" | "minecraft:bee_nest" => odds(5, 20),
        "minecraft:mangrove_roots" => odds(5, 20),
        "minecraft:moss_block"
        | "minecraft:moss_carpet"
        | "minecraft:pale_moss_block"
        | "minecraft:pale_moss_carpet"
        | "minecraft:pale_hanging_moss" => odds(5, 100),
        _ => None,
    }
}

pub fn is_fire(state: u16) -> bool {
    matches!(name(state), "minecraft:fire" | "minecraft:soul_fire")
}

/// Whether fire burns forever on top of `state` (netherrack, magma).
pub fn burns_forever_on(state: u16) -> bool {
    block_has_tag(state, "minecraft:infiniburn_overworld")
}

/// Whether any neighbour of `pos` can burn.
pub fn near_flammable(pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> bool {
    Dir::ALL
        .iter()
        .any(|&d| flammability(get(offset(pos, d))).is_some())
}

/// Whether fire at `pos` has something to hold it: a solid floor or something to burn.
pub fn fire_survives(state: u16, pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> bool {
    let below = get(offset(pos, Dir::Down));
    if name(state) == "minecraft:soul_fire" {
        return block_has_tag(below, "minecraft:soul_fire_base_blocks");
    }
    sturdy(below, Dir::Up) || near_flammable(pos, get)
}

/// The fire that would burn at `pos` (soul fire over soul sand and soil), clinging to
/// the flammable blocks around it when it has no floor; `None` if it cannot burn there.
pub fn fire_for(pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> Option<u16> {
    if !is_air(get(pos)) {
        return None;
    }
    let below = get(offset(pos, Dir::Down));
    if block_has_tag(below, "minecraft:soul_fire_base_blocks") {
        return BlockInfo::by_name("minecraft:soul_fire").map(|b| b.default_state);
    }
    let mut state = BlockInfo::of(block::FIRE).default_state;
    if !fire_survives(state, pos, get) {
        return None;
    }
    if !sturdy(below, Dir::Up) && flammability(below).is_none() {
        for d in [Dir::North, Dir::East, Dir::South, Dir::West, Dir::Up] {
            let burns = flammability(get(offset(pos, d))).is_some();
            state = with_bool(state, d.name(), burns);
        }
    }
    Some(state)
}
