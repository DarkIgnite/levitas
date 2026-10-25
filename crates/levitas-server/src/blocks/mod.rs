//! Block behaviour rules that do not depend on the rest of the world: directions, block
//! families, whether a block can stay where it is, and how it connects to its neighbours.
//! [`place`] computes the state of a block being placed.
//!
//! Functions read neighbours through a `get` closure so they can be tested without a world.

pub mod place;

use levitas_protocol::BlockPos;
use levitas_registry::{block, block_has_tag, collision_boxes, face_sturdy, BlockInfo};
use levitas_world::is_air;

/// A direction, numbered like the protocol's block faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    pub const ALL: [Dir; 6] = [
        Dir::Down,
        Dir::Up,
        Dir::North,
        Dir::South,
        Dir::West,
        Dir::East,
    ];
    pub const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::South, Dir::West, Dir::East];

    pub fn from_face(face: i32) -> Option<Dir> {
        usize::try_from(face)
            .ok()
            .and_then(|i| Dir::ALL.get(i).copied())
    }

    pub fn face(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        ["down", "up", "north", "south", "west", "east"][self as usize]
    }

    pub fn from_name(name: &str) -> Option<Dir> {
        Dir::ALL.into_iter().find(|d| d.name() == name)
    }

    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }

    /// Turned 90 degrees clockwise seen from above (horizontal directions only).
    pub fn clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            vertical => vertical,
        }
    }

    pub fn counter_clockwise(self) -> Dir {
        self.clockwise().opposite()
    }

    pub fn is_horizontal(self) -> bool {
        !matches!(self, Dir::Down | Dir::Up)
    }

    pub fn axis(self) -> &'static str {
        match self {
            Dir::Down | Dir::Up => "y",
            Dir::North | Dir::South => "z",
            Dir::West | Dir::East => "x",
        }
    }

    /// The horizontal direction a player with this yaw faces.
    pub fn from_yaw(yaw: f32) -> Dir {
        match ((yaw / 90.0 + 0.5).floor() as i32).rem_euclid(4) {
            0 => Dir::South,
            1 => Dir::West,
            2 => Dir::North,
            _ => Dir::East,
        }
    }

    /// The direction a player looks in most, vertical included.
    pub fn from_look(yaw: f32, pitch: f32) -> Dir {
        if pitch > 45.0 {
            Dir::Down
        } else if pitch < -45.0 {
            Dir::Up
        } else {
            Dir::from_yaw(yaw)
        }
    }
}

pub fn offset(pos: BlockPos, dir: Dir) -> BlockPos {
    pos.offset(dir as i32)
}

/// Value of a property of `state`.
pub fn prop(state: u16, name: &str) -> Option<&'static str> {
    BlockInfo::of(state).property(state, name)
}

/// `state` with a property changed; unchanged if the block lacks it.
pub fn with(state: u16, name: &str, value: &str) -> u16 {
    BlockInfo::of(state)
        .with(state, name, value)
        .unwrap_or(state)
}

pub fn with_bool(state: u16, name: &str, value: bool) -> u16 {
    with(state, name, if value { "true" } else { "false" })
}

pub fn name(state: u16) -> &'static str {
    BlockInfo::of(state).name
}

pub fn same_block(a: u16, b: u16) -> bool {
    BlockInfo::of(a).first_state == BlockInfo::of(b).first_state
}

fn facing(state: u16) -> Option<Dir> {
    prop(state, "facing").and_then(Dir::from_name)
}

/// Wall-mounted variants of standing blocks: wall torches, signs, banners, heads, fans.
pub fn is_wall_mounted(state: u16) -> bool {
    let n = name(state);
    n.contains("_wall_") || n.starts_with("minecraft:wall_")
}

pub fn is_stairs(state: u16) -> bool {
    block_has_tag(state, "minecraft:stairs")
}

pub fn is_door(state: u16) -> bool {
    block_has_tag(state, "minecraft:doors")
}

pub fn is_bed(state: u16) -> bool {
    block_has_tag(state, "minecraft:beds")
}

pub fn is_chest(state: u16) -> bool {
    matches!(name(state), "minecraft:chest" | "minecraft:trapped_chest")
}

fn is_pane(state: u16) -> bool {
    let n = name(state);
    n.ends_with("glass_pane") || n == "minecraft:iron_bars"
}

/// The other half of a two-part block (doors, tall plants, beds), if `state` is one.
pub fn other_part(state: u16, pos: BlockPos) -> Option<BlockPos> {
    match prop(state, "half") {
        Some("lower") => return Some(offset(pos, Dir::Up)),
        Some("upper") => return Some(offset(pos, Dir::Down)),
        _ => {}
    }
    if is_bed(state) {
        let facing = facing(state)?;
        return Some(match prop(state, "part")? {
            "foot" => offset(pos, facing),
            _ => offset(pos, facing.opposite()),
        });
    }
    None
}

/// Whether `state` is a full block on its `dir` side, for attaching things to it.
pub fn sturdy(state: u16, dir: Dir) -> bool {
    face_sturdy(state, dir.face())
}

/// Whether the top of `state` can hold something small in its centre (a torch on a
/// fence post, for example).
fn supports_center_top(state: u16) -> bool {
    let samples = [7.5 / 16.0, 8.5 / 16.0];
    samples.iter().all(|&x| {
        samples.iter().all(|&z| {
            collision_boxes(state)
                .iter()
                .any(|b| b[4] >= 1.0 && b[0] <= x && x <= b[3] && b[2] <= z && z <= b[5])
        })
    })
}

fn is_soil(state: u16) -> bool {
    block_has_tag(state, "minecraft:dirt") || name(state) == "minecraft:farmland"
}

/// Whether a block can stay at `pos` given its neighbours. Blocks that cannot are popped
/// off (dropping their loot) when a neighbour changes, and cannot be placed there.
pub fn can_survive(state: u16, pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> bool {
    let n = name(state);
    let below = |get: &mut dyn FnMut(BlockPos) -> u16| get(offset(pos, Dir::Down));

    // The two parts of doors, tall plants and beds keep each other.
    if let Some(other) = other_part(state, pos) {
        let partner = get(other);
        let matches = same_block(partner, state)
            && match prop(state, "half") {
                Some("lower") => prop(partner, "half") == Some("upper"),
                Some("upper") => prop(partner, "half") == Some("lower"),
                _ => prop(partner, "part") != prop(state, "part"),
            };
        if !matches {
            return false;
        }
        if prop(state, "half") == Some("upper") {
            return true; // the lower half checks the ground
        }
    }

    // Attached to the face of a neighbour.
    if let Some(face) = prop(state, "face") {
        let facing = facing(state).unwrap_or(Dir::North);
        return match face {
            "floor" => sturdy(below(get), Dir::Up),
            "ceiling" => sturdy(get(offset(pos, Dir::Up)), Dir::Down),
            _ => sturdy(get(offset(pos, facing.opposite())), facing),
        };
    }
    let wall_attached =
        is_wall_mounted(state) || matches!(n, "minecraft:ladder" | "minecraft:tripwire_hook");
    if wall_attached && !n.contains("hanging") {
        if let Some(facing) = facing(state) {
            return sturdy(get(offset(pos, facing.opposite())), facing);
        }
    }
    if n == "minecraft:cocoa" {
        let facing = facing(state).unwrap_or(Dir::North);
        return name(get(offset(pos, facing))) == "minecraft:jungle_log";
    }
    if n.ends_with("lantern") && prop(state, "hanging") == Some("true") {
        let above = get(offset(pos, Dir::Up));
        return sturdy(above, Dir::Down) || name(above) == "minecraft:chain";
    }

    // Sitting on the block below.
    let ground = below(get);
    if block_has_tag(state, "minecraft:crops")
        || matches!(
            n,
            "minecraft:attached_melon_stem"
                | "minecraft:attached_pumpkin_stem"
                | "minecraft:torchflower_crop"
                | "minecraft:pitcher_crop"
        )
    {
        return name(ground) == "minecraft:farmland";
    }
    if n == "minecraft:nether_wart" {
        return name(ground) == "minecraft:soul_sand";
    }
    if n == "minecraft:dead_bush" {
        return is_soil(ground)
            || block_has_tag(ground, "minecraft:sand")
            || block_has_tag(ground, "minecraft:terracotta");
    }
    if n == "minecraft:cactus" {
        return same_block(ground, state) || block_has_tag(ground, "minecraft:sand");
    }
    if n == "minecraft:sugar_cane" {
        return same_block(ground, state)
            || is_soil(ground)
            || block_has_tag(ground, "minecraft:sand");
    }
    let plant = block_has_tag(state, "minecraft:small_flowers")
        || block_has_tag(state, "minecraft:saplings")
        || matches!(
            n,
            "minecraft:short_grass"
                | "minecraft:fern"
                | "minecraft:tall_grass"
                | "minecraft:large_fern"
                | "minecraft:sunflower"
                | "minecraft:lilac"
                | "minecraft:rose_bush"
                | "minecraft:peony"
                | "minecraft:sweet_berry_bush"
                | "minecraft:pink_petals"
                | "minecraft:wildflowers"
                | "minecraft:bush"
                | "minecraft:firefly_bush"
                | "minecraft:short_dry_grass"
                | "minecraft:tall_dry_grass"
                | "minecraft:leaf_litter"
        );
    if plant {
        return is_soil(ground)
            || (n == "minecraft:wither_rose" && name(ground) == "minecraft:soul_sand");
    }
    if matches!(n, "minecraft:brown_mushroom" | "minecraft:red_mushroom") {
        return sturdy(ground, Dir::Up);
    }
    let needs_center = matches!(
        n,
        "minecraft:torch" | "minecraft:soul_torch" | "minecraft:redstone_torch"
    ) || block_has_tag(state, "minecraft:candles")
        || n.ends_with("lantern");
    if needs_center {
        return supports_center_top(ground);
    }
    let needs_floor = n == "minecraft:redstone_wire"
        || n == "minecraft:repeater"
        || n == "minecraft:comparator"
        || n == "minecraft:snow"
        || block_has_tag(state, "minecraft:rails")
        || block_has_tag(state, "minecraft:pressure_plates")
        || block_has_tag(state, "minecraft:standing_signs")
        || block_has_tag(state, "minecraft:wool_carpets")
        || matches!(n, "minecraft:moss_carpet" | "minecraft:pale_moss_carpet")
        || (block_has_tag(state, "minecraft:banners") && !is_wall_mounted(state))
        || (is_door(state) && prop(state, "half") == Some("lower"));
    if needs_floor {
        return sturdy(ground, Dir::Up);
    }
    if n == "minecraft:cake" || n.ends_with("_cake") {
        return !is_air(ground);
    }
    true
}

/// Recomputes properties that depend on neighbours: fence, pane and wall connections,
/// stair corners, double chests and the upper half of doors.
pub fn connect(state: u16, pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> u16 {
    if block_has_tag(state, "minecraft:fences") {
        let mut s = state;
        for dir in Dir::HORIZONTAL {
            let neighbor = get(offset(pos, dir));
            s = with_bool(s, dir.name(), fence_connects(state, neighbor, dir));
        }
        return s;
    }
    if is_pane(state) {
        let mut s = state;
        for dir in Dir::HORIZONTAL {
            let neighbor = get(offset(pos, dir));
            let connects = is_pane(neighbor)
                || block_has_tag(neighbor, "minecraft:walls")
                || (sturdy(neighbor, dir.opposite()) && !connection_exception(neighbor));
            s = with_bool(s, dir.name(), connects);
        }
        return s;
    }
    if block_has_tag(state, "minecraft:walls") {
        return wall_shape(state, pos, get);
    }
    if is_stairs(state) {
        return with(state, "shape", stairs_shape(state, pos, get));
    }
    if is_chest(state) {
        return chest_type(state, pos, get);
    }
    // The upper half of a door mirrors the lower half's open and powered state.
    if is_door(state) && prop(state, "half") == Some("upper") {
        let lower = get(offset(pos, Dir::Down));
        if same_block(lower, state) {
            let mut s = state;
            for p in ["open", "powered", "facing"] {
                if let Some(v) = prop(lower, p) {
                    s = with(s, p, v);
                }
            }
            return s;
        }
    }
    state
}

/// Blocks that never connect to fences, panes and walls despite being solid.
fn connection_exception(state: u16) -> bool {
    block_has_tag(state, "minecraft:leaves")
        || block_has_tag(state, "minecraft:shulker_boxes")
        || matches!(
            name(state),
            "minecraft:barrier"
                | "minecraft:carved_pumpkin"
                | "minecraft:jack_o_lantern"
                | "minecraft:melon"
                | "minecraft:pumpkin"
        )
}

fn fence_connects(fence: u16, neighbor: u16, dir: Dir) -> bool {
    if block_has_tag(neighbor, "minecraft:fences") {
        // Wooden fences join wooden fences; nether brick fences join each other.
        return block_has_tag(neighbor, "minecraft:wooden_fences")
            == block_has_tag(fence, "minecraft:wooden_fences");
    }
    if block_has_tag(neighbor, "minecraft:fence_gates") {
        return facing(neighbor).is_some_and(|f| f.axis() != dir.axis());
    }
    sturdy(neighbor, dir.opposite()) && !connection_exception(neighbor)
}

fn wall_shape(state: u16, pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> u16 {
    let above = get(offset(pos, Dir::Up));
    let mut s = state;
    let mut connected = [false; 4];
    for (i, dir) in Dir::HORIZONTAL.into_iter().enumerate() {
        let neighbor = get(offset(pos, dir));
        let connects = block_has_tag(neighbor, "minecraft:walls")
            || is_pane(neighbor)
            || (block_has_tag(neighbor, "minecraft:fence_gates")
                && facing(neighbor).is_some_and(|f| f.axis() != dir.axis()))
            || (sturdy(neighbor, dir.opposite()) && !connection_exception(neighbor));
        connected[i] = connects;
        // A side is tall when the block above covers it.
        let height = if !connects {
            "none"
        } else if sturdy(above, Dir::Down) || block_has_tag(above, "minecraft:walls") {
            "tall"
        } else {
            "low"
        };
        s = with(s, dir.name(), height);
    }
    // The post shows unless the wall runs straight through with nothing on top.
    let [north, south, west, east] = connected;
    let straight = (north && south && !west && !east) || (west && east && !north && !south);
    let post = !straight || (!is_air(above) && !block_has_tag(above, "minecraft:walls"));
    with_bool(s, "up", post)
}

fn stairs_shape(state: u16, pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> &'static str {
    let Some(facing) = facing(state) else {
        return "straight";
    };
    let half = prop(state, "half");
    let can_take = |side: Dir, get: &mut dyn FnMut(BlockPos) -> u16| {
        let n = get(offset(pos, side));
        !is_stairs(n) || self::facing(n) != Some(facing) || prop(n, "half") != half
    };
    // Stairs in front make an outer corner, stairs behind an inner corner.
    let ahead = get(offset(pos, facing));
    if is_stairs(ahead) && prop(ahead, "half") == half {
        if let Some(other) = self::facing(ahead) {
            if other.axis() != facing.axis() && can_take(other.opposite(), get) {
                return if other == facing.counter_clockwise() {
                    "outer_left"
                } else {
                    "outer_right"
                };
            }
        }
    }
    let behind = get(offset(pos, facing.opposite()));
    if is_stairs(behind) && prop(behind, "half") == half {
        if let Some(other) = self::facing(behind) {
            if other.axis() != facing.axis() && can_take(other, get) {
                return if other == facing.counter_clockwise() {
                    "inner_left"
                } else {
                    "inner_right"
                };
            }
        }
    }
    "straight"
}

/// The side a double chest half's partner is on.
fn chest_partner_side(state: u16) -> Option<Dir> {
    let facing = facing(state)?;
    match prop(state, "type")? {
        "left" => Some(facing.clockwise()),
        "right" => Some(facing.counter_clockwise()),
        _ => None,
    }
}

/// Keeps double chests paired: a half whose partner is gone becomes single, and a single
/// chest next to a half that points at it becomes that half's partner.
fn chest_type(state: u16, pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> u16 {
    let facing = facing(state).unwrap_or(Dir::North);
    if let Some(side) = chest_partner_side(state) {
        let partner = get(offset(pos, side));
        // The partner may still be single right after placement; it pairs on its own
        // update.
        let partner_side = chest_partner_side(partner);
        let paired = same_block(partner, state)
            && self::facing(partner) == Some(facing)
            && (partner_side.is_none() || partner_side == Some(side.opposite()));
        return if paired {
            state
        } else {
            with(state, "type", "single")
        };
    }
    for side in [facing.clockwise(), facing.counter_clockwise()] {
        let partner = get(offset(pos, side));
        if same_block(partner, state)
            && self::facing(partner) == Some(facing)
            && chest_partner_side(partner) == Some(side.opposite())
        {
            return with(
                state,
                "type",
                if side == facing.clockwise() {
                    "left"
                } else {
                    "right"
                },
            );
        }
    }
    state
}

/// For a double chest half, its partner's position.
pub fn chest_partner(state: u16, pos: BlockPos) -> Option<BlockPos> {
    chest_partner_side(state).map(|side| offset(pos, side))
}

/// What a block looks like after a neighbour changed: popped (air) if it can no longer
/// stay, otherwise with its connections refreshed.
pub fn updated(state: u16, pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> u16 {
    if is_air(state) {
        return state;
    }
    if !can_survive(state, pos, get) {
        return block::AIR;
    }
    connect(state, pos, get)
}

/// Whether `state` is something a player opens or toggles by using it with any item.
pub fn toggles(state: u16) -> bool {
    let n = name(state);
    (is_door(state) && n != "minecraft:iron_door")
        || (block_has_tag(state, "minecraft:trapdoors") && n != "minecraft:iron_trapdoor")
        || block_has_tag(state, "minecraft:fence_gates")
        || n == "minecraft:lever"
        || block_has_tag(state, "minecraft:buttons")
}

#[cfg(test)]
mod tests;
