//! Plants as block states: crops and their ages, how far leaves are from a log, and the
//! shapes of the trees saplings grow into.
//!
//! Tree shapes follow the Minecraft Wiki's descriptions: oaks, birches, jungle, acacia
//! and cherry saplings grow a straight trunk with a rounded crown two blocks wide at
//! the bottom and one at the top; spruces grow a cone of alternating wide and narrow
//! rings; dark oaks need four saplings in a square and grow a thick trunk.

use levitas_protocol::BlockPos;
use levitas_registry::{block, block_has_tag, BlockInfo};
use levitas_world::is_air;

use super::{name, prop, with, Dir};

/// Ages of the crops that grow on farmland, from planted to ripe.
pub fn crop_max_age(state: u16) -> Option<u8> {
    match name(state) {
        "minecraft:wheat" | "minecraft:carrots" | "minecraft:potatoes" => Some(7),
        "minecraft:beetroots" => Some(3),
        _ => None,
    }
}

pub fn age(state: u16) -> u8 {
    prop(state, "age").and_then(|a| a.parse().ok()).unwrap_or(0)
}

pub fn with_age(state: u16, age: u8) -> u16 {
    with(state, "age", &age.to_string())
}

pub fn is_log(state: u16) -> bool {
    block_has_tag(state, "minecraft:logs")
}

pub fn is_leaves(state: u16) -> bool {
    block_has_tag(state, "minecraft:leaves")
}

/// What a leaves block's `distance` should be given its neighbours: one more than the
/// closest neighbouring log (0) or leaves, up to 7, which means no log within reach.
pub fn leaf_distance(neighbours: impl IntoIterator<Item = u16>) -> u8 {
    neighbours
        .into_iter()
        .filter_map(|s| {
            if is_log(s) {
                Some(0)
            } else if is_leaves(s) {
                prop(s, "distance").and_then(|d| d.parse::<u8>().ok())
            } else {
                None
            }
        })
        .min()
        .map_or(7, |d| (d + 1).min(7))
}

/// Whether leaves will rot away: not placed by a player and too far from any log.
pub fn leaves_decay(state: u16) -> bool {
    is_leaves(state)
        && prop(state, "persistent") == Some("false")
        && prop(state, "distance") == Some("7")
}

/// Kinds of trees saplings grow into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tree {
    /// A straight trunk with a round crown: oak, birch, jungle, acacia, cherry.
    Round(&'static str),
    Spruce,
    /// Needs a 2x2 square of saplings.
    DarkOak,
    /// One trunk under an especially large canopy of yellow, orange or red leaves, with
    /// shelf mushrooms here and there on the trunk.
    Poplar,
}

pub fn tree_of(sapling: u16) -> Option<Tree> {
    match name(sapling) {
        "minecraft:oak_sapling" => Some(Tree::Round("oak")),
        "minecraft:birch_sapling" => Some(Tree::Round("birch")),
        "minecraft:jungle_sapling" => Some(Tree::Round("jungle")),
        "minecraft:acacia_sapling" => Some(Tree::Round("acacia")),
        "minecraft:cherry_sapling" => Some(Tree::Round("cherry")),
        "minecraft:spruce_sapling" => Some(Tree::Spruce),
        "minecraft:dark_oak_sapling" => Some(Tree::DarkOak),
        "minecraft:poplar_sapling" => Some(Tree::Poplar),
        _ => None,
    }
}

fn wood(kind: &str, part: &str) -> u16 {
    BlockInfo::by_name(&format!("minecraft:{kind}_{part}")).map_or(block::AIR, |b| b.default_state)
}

/// Leaves that are not persistent, so they decay once their tree is cut.
fn leaves(kind: &str) -> u16 {
    let state = wood(kind, "leaves");
    with(with(state, "persistent", "false"), "distance", "1")
}

/// The blocks of a tree grown from a sapling at `base` (trunks first, then leaves).
/// `roll(n)` is a random number below `n`. For dark oaks `base` is the north-west
/// sapling of the square.
pub fn grow(tree: Tree, base: BlockPos, roll: &mut impl FnMut(u32) -> u32) -> Vec<(BlockPos, u16)> {
    let mut blocks = Vec::new();
    let at = |dx: i32, dy: i32, dz: i32| BlockPos::new(base.x + dx, base.y + dy, base.z + dz);
    match tree {
        Tree::Round(kind) => {
            let height = match kind {
                "birch" => 5 + roll(3) as i32,
                "jungle" => 4 + roll(7) as i32,
                _ => 4 + roll(3) as i32,
            };
            let log = wood(kind, "log");
            for dy in 0..height {
                blocks.push((at(0, dy, 0), log));
            }
            let leaf = leaves(kind);
            // Two wide layers below the top of the trunk, two narrow ones at the top.
            for layer in 0..4 {
                let dy = height - 3 + layer;
                let radius: i32 = if layer < 2 { 2 } else { 1 };
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        let corner = dx.abs() == radius && dz.abs() == radius;
                        // The top's corners are always bare, the others half the time.
                        if corner && (layer == 3 || roll(2) == 0) {
                            continue;
                        }
                        blocks.push((at(dx, dy, dz), leaf));
                    }
                }
            }
        }
        Tree::Spruce => {
            let height = 6 + roll(4) as i32;
            let log = wood("spruce", "log");
            for dy in 0..height - 1 {
                blocks.push((at(0, dy, 0), log));
            }
            let leaf = leaves("spruce");
            blocks.push((at(0, height, 0), leaf));
            blocks.push((at(0, height - 1, 0), leaf));
            // Rings from the top down, alternating narrow and wide, over a bare stem.
            let mut wide = false;
            for dy in (2..height - 1).rev() {
                let radius: i32 = if wide { 2 } else { 1 };
                wide = !wide;
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        let corner = dx.abs() == radius && dz.abs() == radius;
                        if corner || (dx == 0 && dz == 0) {
                            continue;
                        }
                        blocks.push((at(dx, dy, dz), leaf));
                    }
                }
            }
        }
        Tree::Poplar => {
            let height = 7 + roll(3) as i32;
            let log = wood("poplar", "log");
            for dy in 0..height {
                blocks.push((at(0, dy, 0), log));
            }
            // The canopy is one colour, picked when the tree grows.
            let colour = ["yellow_poplar", "orange_poplar", "red_poplar"][roll(3) as usize];
            let leaf = leaves(colour);
            // A tall rounded crown: narrow at its foot and top, three blocks out in the
            // middle, from four blocks under the top of the trunk to two above it.
            for dy in height - 4..=height + 1 {
                let radius: i32 = match dy - height {
                    -4 | 1 => 1,
                    -3 | 0 => 2,
                    _ => 3,
                };
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        if dx * dx + dz * dz > radius * radius + 1 {
                            continue;
                        }
                        // Ragged edges: the outermost ring is patchy.
                        if dx * dx + dz * dz >= radius * radius && roll(3) == 0 {
                            continue;
                        }
                        blocks.push((at(dx, dy, dz), leaf));
                    }
                }
            }
            // Now and then a shelf mushroom on the bare trunk, facing away from it.
            if let Some(mushroom) = BlockInfo::by_name("minecraft:shelf_mushroom") {
                for dy in 1..3 {
                    for dir in Dir::HORIZONTAL {
                        if roll(6) != 0 {
                            continue;
                        }
                        let [dx, _, dz] = dir.step();
                        let state = with(mushroom.default_state, "facing", dir.name());
                        let age = if roll(2) == 0 { "0" } else { "1" };
                        blocks.push((at(dx, dy, dz), with(state, "age", age)));
                    }
                }
            }
        }
        Tree::DarkOak => {
            let height = 6 + roll(3) as i32;
            let log = wood("dark_oak", "log");
            for dy in 0..height {
                for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    blocks.push((at(dx, dy, dz), log));
                }
            }
            let leaf = leaves("dark_oak");
            for layer in 0..3 {
                let dy = height - 2 + layer;
                let radius: i32 = if layer == 2 { 2 } else { 3 };
                for dx in -radius..=radius + 1 {
                    for dz in -radius..=radius + 1 {
                        let edge_x = dx == -radius || dx == radius + 1;
                        let edge_z = dz == -radius || dz == radius + 1;
                        if edge_x && edge_z {
                            continue;
                        }
                        blocks.push((at(dx, dy, dz), leaf));
                    }
                }
            }
        }
    }
    blocks
}

/// Whether a tree may grow through `state`: air, plants, leaves, or the sapling itself.
pub fn tree_can_replace(state: u16) -> bool {
    is_air(state)
        || is_leaves(state)
        || block_has_tag(state, "minecraft:saplings")
        || block_has_tag(state, "minecraft:replaceable")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    #[test]
    fn leaves_count_their_distance_from_logs() {
        let log = state("minecraft:oak_log");
        let leaf = leaves("oak");
        let far = with(leaf, "distance", "4");
        assert_eq!(leaf_distance([block::AIR, log]), 1);
        assert_eq!(leaf_distance([far, block::STONE]), 5);
        assert_eq!(leaf_distance([block::AIR, block::STONE]), 7);
        assert!(leaves_decay(with(leaf, "distance", "7")));
        // Leaves placed by a player stay.
        assert!(!leaves_decay(with(
            with(leaf, "distance", "7"),
            "persistent",
            "true"
        )));
    }

    #[test]
    fn oak_saplings_grow_a_trunk_and_a_crown() {
        let mut seed = 1u32;
        let mut roll = |n: u32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) % n
        };
        let base = BlockPos::new(0, 64, 0);
        let blocks = grow(Tree::Round("oak"), base, &mut roll);
        let logs = blocks.iter().filter(|(_, s)| is_log(*s)).count();
        assert!((4..=6).contains(&logs));
        let crown_top = blocks.iter().map(|(p, _)| p.y).max().unwrap();
        assert_eq!(crown_top, 64 + logs as i32);
        assert!(blocks.iter().any(|(p, s)| is_leaves(*s) && p.x == 2));
    }

    #[test]
    fn poplars_grow_tall_with_one_colour_of_leaves() {
        let mut seed = 7u32;
        let mut roll = |n: u32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) % n
        };
        assert_eq!(
            tree_of(state("minecraft:poplar_sapling")),
            Some(Tree::Poplar)
        );
        let base = BlockPos::new(0, 64, 0);
        let blocks = grow(Tree::Poplar, base, &mut roll);
        let logs = blocks.iter().filter(|(_, s)| is_log(*s)).count();
        assert!((7..=9).contains(&logs));
        let colours: std::collections::HashSet<&str> = blocks
            .iter()
            .filter(|(_, s)| is_leaves(*s))
            .map(|(_, s)| name(*s))
            .collect();
        assert_eq!(colours.len(), 1);
        assert!(colours.iter().all(|c| c.ends_with("_poplar_leaves")));
        // The canopy reaches three blocks out from the trunk.
        assert!(blocks.iter().any(|(p, s)| is_leaves(*s) && p.x.abs() == 3));
    }

    #[test]
    fn crops_know_when_they_are_ripe() {
        assert_eq!(crop_max_age(state("minecraft:wheat")), Some(7));
        assert_eq!(crop_max_age(state("minecraft:beetroots")), Some(3));
        assert_eq!(crop_max_age(block::STONE), None);
        let wheat = with_age(state("minecraft:wheat"), 5);
        assert_eq!(age(wheat), 5);
    }
}
