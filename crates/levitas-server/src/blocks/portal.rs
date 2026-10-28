//! Nether portal frames, following the Minecraft Wiki: a rectangle of obsidian (corners
//! optional) around an opening 2 to 21 blocks wide and 3 to 21 high, standing along x
//! or z. Fire in the opening lights it.

use levitas_protocol::BlockPos;
use levitas_registry::BlockInfo;
use levitas_world::is_air;

use super::{name, offset, prop, with, Dir};

pub const MIN_WIDTH: i32 = 2;
pub const MIN_HEIGHT: i32 = 3;
pub const MAX_SIZE: i32 = 21;

/// An opening in a frame: its lowest corner (towards negative x or z), the direction
/// it extends in (east or south), and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opening {
    pub corner: BlockPos,
    pub along: Dir,
    pub width: i32,
    pub height: i32,
}

fn obsidian(state: u16) -> bool {
    name(state) == "minecraft:obsidian"
}

/// Whether a portal can open over this block: nothing there but air or fire.
fn open(state: u16) -> bool {
    is_air(state) || matches!(name(state), "minecraft:fire" | "minecraft:nether_portal")
}

fn step(pos: BlockPos, dir: Dir, n: i32) -> BlockPos {
    let s = dir.step();
    BlockPos::new(pos.x + s[0] * n, pos.y + s[1] * n, pos.z + s[2] * n)
}

/// The frame opening that `pos` is in, if it is a complete one.
pub fn find_opening(pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> Option<Opening> {
    [Dir::East, Dir::South]
        .into_iter()
        .find_map(|along| opening_along(pos, along, get))
}

fn opening_along(
    pos: BlockPos,
    along: Dir,
    get: &mut impl FnMut(BlockPos) -> u16,
) -> Option<Opening> {
    if !open(get(pos)) {
        return None;
    }
    // Down to the frame's bottom.
    let mut bottom = pos;
    for _ in 0..MAX_SIZE {
        if !open(get(offset(bottom, Dir::Down))) {
            break;
        }
        bottom = offset(bottom, Dir::Down);
    }
    if !obsidian(get(offset(bottom, Dir::Down))) {
        return None;
    }
    // Back to the frame's side.
    let back = along.opposite();
    let mut corner = bottom;
    for _ in 0..MAX_SIZE {
        let next = offset(corner, back);
        if !open(get(next)) || !obsidian(get(offset(next, Dir::Down))) {
            break;
        }
        corner = next;
    }
    if !obsidian(get(offset(corner, back))) {
        return None;
    }
    // Across the bottom to the other side.
    let mut width = 0;
    while width < MAX_SIZE {
        let at = step(corner, along, width);
        if !open(get(at)) || !obsidian(get(offset(at, Dir::Down))) {
            break;
        }
        width += 1;
    }
    if width < MIN_WIDTH || !obsidian(get(step(corner, along, width))) {
        return None;
    }
    // Up row by row, with obsidian at both ends, to an obsidian top.
    let mut height = 0;
    while height < MAX_SIZE {
        let row = step(corner, Dir::Up, height);
        if !(0..width).all(|i| open(get(step(row, along, i)))) {
            break;
        }
        if !obsidian(get(offset(row, back))) || !obsidian(get(step(row, along, width))) {
            return None;
        }
        height += 1;
    }
    let top = step(corner, Dir::Up, height);
    if height < MIN_HEIGHT || !(0..width).all(|i| obsidian(get(step(top, along, i)))) {
        return None;
    }
    Some(Opening {
        corner,
        along,
        width,
        height,
    })
}

impl Opening {
    /// Every block of the opening.
    pub fn blocks(&self) -> impl Iterator<Item = BlockPos> + '_ {
        (0..self.height).flat_map(move |h| {
            (0..self.width).map(move |i| step(step(self.corner, Dir::Up, h), self.along, i))
        })
    }

    /// The portal block filling the opening.
    pub fn portal_state(&self) -> u16 {
        let portal = BlockInfo::by_name("minecraft:nether_portal")
            .expect("nether portals exist")
            .default_state;
        with(
            portal,
            "axis",
            if self.along == Dir::East { "x" } else { "z" },
        )
    }
}

/// Whether a portal block at `pos` still has portal or obsidian above, below and on
/// both sides along its axis.
pub fn portal_survives(state: u16, pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> bool {
    let sides = if prop(state, "axis") == Some("z") {
        [Dir::North, Dir::South]
    } else {
        [Dir::West, Dir::East]
    };
    [Dir::Up, Dir::Down, sides[0], sides[1]]
        .into_iter()
        .all(|d| {
            let next = get(offset(pos, d));
            obsidian(next) || name(next) == "minecraft:nether_portal"
        })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use levitas_registry::block;

    /// A 4 x 5 obsidian frame along x at the origin, with or without its corners.
    fn frame(corners: bool) -> HashMap<BlockPos, u16> {
        let obsidian = BlockInfo::by_name("minecraft:obsidian")
            .unwrap()
            .default_state;
        let mut blocks = HashMap::new();
        for x in 0..4 {
            for y in 0..5 {
                let edge_x = x == 0 || x == 3;
                let edge_y = y == 0 || y == 4;
                if (edge_x || edge_y) && (corners || !(edge_x && edge_y)) {
                    blocks.insert(BlockPos::new(x, y, 0), obsidian);
                }
            }
        }
        blocks
    }

    #[test]
    fn frames_are_found_with_or_without_corners() {
        for corners in [true, false] {
            let blocks = frame(corners);
            let mut get = |p: BlockPos| blocks.get(&p).copied().unwrap_or(block::AIR);
            let opening = find_opening(BlockPos::new(2, 2, 0), &mut get).unwrap();
            assert_eq!(opening.corner, BlockPos::new(1, 1, 0));
            assert_eq!(
                (opening.along, opening.width, opening.height),
                (Dir::East, 2, 3)
            );
            assert_eq!(opening.blocks().count(), 6);
        }
    }

    #[test]
    fn broken_frames_are_not() {
        let mut blocks = frame(true);
        blocks.remove(&BlockPos::new(3, 2, 0));
        let mut get = |p: BlockPos| blocks.get(&p).copied().unwrap_or(block::AIR);
        assert!(find_opening(BlockPos::new(1, 1, 0), &mut get).is_none());
    }
}
