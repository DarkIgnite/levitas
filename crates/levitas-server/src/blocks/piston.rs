//! What pistons do to the blocks in their way, following the Minecraft Wiki: most
//! blocks are pushed, small things without a hitbox (torches, flowers, dust, ...) and
//! doors and beds break, and some never move: unbreakable blocks, obsidian, blocks
//! holding things, and extended pistons.

use levitas_registry::{block_has_tag, collision_boxes, BlockInfo};
use levitas_world::is_air;

use super::{name, prop};

/// The most blocks one piston pushes.
pub const PUSH_LIMIT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    /// Moved along.
    Push,
    /// Broken, dropping its items.
    Destroy,
    /// Stops the piston.
    Block,
    /// Nothing there (air or fluid): the push goes through.
    Empty,
}

pub fn is_piston(state: u16) -> bool {
    matches!(name(state), "minecraft:piston" | "minecraft:sticky_piston")
}

pub fn is_sticky(state: u16) -> bool {
    name(state) == "minecraft:sticky_piston"
}

/// Blocks that keep things inside: they never move.
fn holds_things(state: u16) -> bool {
    let n = name(state);
    [
        "chest",
        "barrel",
        "furnace",
        "smoker",
        "hopper",
        "dispenser",
        "dropper",
        "beacon",
        "spawner",
        "_sign",
        "banner",
        "jukebox",
        "lectern",
        "brewing_stand",
        "campfire",
        "bell",
        "conduit",
        "beehive",
        "bee_nest",
        "enchanting_table",
        "skull",
        "_head",
        "decorated_pot",
        "crafter",
        "trial_spawner",
        "vault",
    ]
    .iter()
    .any(|part| n.contains(part))
}

/// How a block reacts to being pushed or pulled.
pub fn reaction(state: u16) -> Reaction {
    if is_air(state) || matches!(name(state), "minecraft:water" | "minecraft:lava") {
        return Reaction::Empty;
    }
    let n = name(state);
    if BlockInfo::of(state).hardness < 0.0
        || matches!(
            n,
            "minecraft:obsidian"
                | "minecraft:crying_obsidian"
                | "minecraft:respawn_anchor"
                | "minecraft:reinforced_deepslate"
                | "minecraft:piston_head"
                | "minecraft:moving_piston"
        )
        || (is_piston(state) && prop(state, "extended") == Some("true"))
        || holds_things(state)
    {
        return Reaction::Block;
    }
    if collision_boxes(state).is_empty()
        || super::is_door(state)
        || block_has_tag(state, "minecraft:beds")
        || n.ends_with("shulker_box")
    {
        return Reaction::Destroy;
    }
    Reaction::Push
}

#[cfg(test)]
mod tests {
    use super::*;
    use levitas_registry::block;

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    #[test]
    fn what_pistons_push_break_and_stop_at() {
        assert_eq!(reaction(block::STONE), Reaction::Push);
        assert_eq!(reaction(state("minecraft:oak_slab")), Reaction::Push);
        assert_eq!(reaction(state("minecraft:torch")), Reaction::Destroy);
        assert_eq!(reaction(state("minecraft:poppy")), Reaction::Destroy);
        assert_eq!(reaction(block::BEDROCK), Reaction::Block);
        assert_eq!(reaction(state("minecraft:obsidian")), Reaction::Block);
        assert_eq!(reaction(block::CHEST), Reaction::Block);
        assert_eq!(reaction(block::AIR), Reaction::Empty);
        assert_eq!(reaction(block::WATER), Reaction::Empty);
    }
}
