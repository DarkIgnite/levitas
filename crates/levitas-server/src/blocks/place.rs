//! The state a block gets when a player places it: which way it faces, which half of a
//! block a slab or stair takes, wall or floor variants, both halves of doors and beds,
//! double slabs and double chests.

use levitas_protocol::BlockPos;
use levitas_registry::{block_has_tag, BlockInfo};

use super::*;

/// Where and how a player clicked to place a block.
#[derive(Debug, Clone, Copy)]
pub struct PlaceContext {
    pub clicked: BlockPos,
    pub face: Dir,
    /// Where on the clicked block's face, each coordinate in 0..=1.
    pub cursor: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub sneaking: bool,
}

impl PlaceContext {
    fn player_dir(&self) -> Dir {
        Dir::from_yaw(self.yaw)
    }

    /// Height of the click within the block at `target` (0..1 for side clicks).
    fn hit_y(&self, target: BlockPos) -> f32 {
        (self.clicked.y - target.y) as f32 + self.cursor[1]
    }

    /// Where the click was, relative to the corner of `target`.
    fn hit(&self, target: BlockPos) -> [f32; 3] {
        [
            (self.clicked.x - target.x) as f32 + self.cursor[0],
            (self.clicked.y - target.y) as f32 + self.cursor[1],
            (self.clicked.z - target.z) as f32 + self.cursor[2],
        ]
    }
}

fn replaceable(state: u16) -> bool {
    block_has_tag(state, "minecraft:replaceable")
}

/// Whether placing `slab` with this click merges into the slab `existing` at the clicked
/// position, making a double slab.
fn merges_into_clicked(slab: u16, existing: u16, ctx: &PlaceContext) -> bool {
    if !same_block(slab, existing) || !block_has_tag(slab, "minecraft:slabs") {
        return false;
    }
    let upper = ctx.cursor[1] > 0.5;
    match prop(existing, "type") {
        Some("bottom") => ctx.face == Dir::Up || (upper && ctx.face.is_horizontal()),
        Some("top") => ctx.face == Dir::Down || (!upper && ctx.face.is_horizontal()),
        _ => false,
    }
}

/// The wall-mounted variant of a standing block, if it has one (torch → wall_torch,
/// oak_sign → oak_wall_sign, white_banner → white_wall_banner, ...).
pub fn wall_variant(block_state: u16) -> Option<u16> {
    let name = name(block_state).strip_prefix("minecraft:")?;
    let parts: Vec<&str> = name.split('_').collect();
    for split in (1..=parts.len()).rev() {
        let (head, tail) = parts.split_at(split - 1);
        let candidate = if head.is_empty() {
            format!("minecraft:wall_{}", tail.join("_"))
        } else {
            format!("minecraft:{}_wall_{}", head.join("_"), tail.join("_"))
        };
        if let Some(info) = BlockInfo::by_name(&candidate) {
            return Some(info.default_state);
        }
    }
    None
}

/// The blocks to set for placing `block_state` (an item's default block), or `None` if it
/// cannot be placed there.
pub fn place(
    block_state: u16,
    ctx: &PlaceContext,
    get: &mut impl FnMut(BlockPos) -> u16,
) -> Option<Vec<(BlockPos, u16)>> {
    let clicked_state = get(ctx.clicked);
    let replacing =
        merges_into_clicked(block_state, clicked_state, ctx) || replaceable(clicked_state);
    let target = if replacing {
        ctx.clicked
    } else {
        offset(ctx.clicked, ctx.face)
    };
    let existing = get(target);
    let merging_slab = block_has_tag(block_state, "minecraft:slabs")
        && same_block(existing, block_state)
        && prop(existing, "type") != Some("double");
    if !replaceable(existing) && !merging_slab {
        return None;
    }
    if !(levitas_world::MIN_Y..=levitas_world::MAX_Y).contains(&target.y) {
        return None;
    }

    let mut state = choose_variant(block_state, ctx)?;
    state = orient(state, ctx, target, existing, get)?;
    if prop(existing, "level") == Some("0")
        && name(existing) == "minecraft:water"
        && prop(state, "type") != Some("double")
    {
        state = with_bool(state, "waterlogged", true);
    }
    if fluid::fluid_at(existing).is_some_and(|f| f.fluid == fluid::Fluid::Water) {
        if let Some(concrete) = concrete_from_powder(state) {
            state = concrete;
        }
    }
    state = concrete_hardens(state, target, get);

    // Two-part blocks claim their second position too.
    let mut blocks = vec![(target, state)];
    if prop(state, "half") == Some("lower") {
        let above = offset(target, Dir::Up);
        if above.y > levitas_world::MAX_Y || !replaceable(get(above)) {
            return None;
        }
        blocks.push((above, with(state, "half", "upper")));
    } else if is_bed(state) {
        let head = offset(target, facing(state)?);
        if !replaceable(get(head)) {
            return None;
        }
        blocks.push((head, with(state, "part", "head")));
    }

    // Check survival and connections with the new blocks in place.
    let placed = blocks.clone();
    let mut view = |p: BlockPos| {
        placed
            .iter()
            .find(|(q, _)| *q == p)
            .map_or_else(|| get(p), |&(_, s)| s)
    };
    if !can_survive(state, target, &mut view) {
        return None;
    }
    for entry in &mut blocks {
        entry.1 = connect(entry.1, entry.0, &mut view);
    }
    Some(blocks)
}

/// Picks the standing or wall-mounted form of blocks that have both.
fn choose_variant(block_state: u16, ctx: &PlaceContext) -> Option<u16> {
    let n = name(block_state);
    if n.ends_with("_hanging_sign") {
        return match ctx.face {
            Dir::Down => Some(block_state),
            Dir::Up => None,
            _ => wall_variant(block_state),
        };
    }
    match wall_variant(block_state) {
        Some(wall) if ctx.face.is_horizontal() => Some(wall),
        Some(_) if ctx.face == Dir::Down => Some(block_state),
        _ => Some(block_state),
    }
}

fn facing(state: u16) -> Option<Dir> {
    prop(state, "facing").and_then(Dir::from_name)
}

/// Sets direction-like properties from the click and the player's look.
fn orient(
    mut state: u16,
    ctx: &PlaceContext,
    target: BlockPos,
    existing: u16,
    get: &mut impl FnMut(BlockPos) -> u16,
) -> Option<u16> {
    let n = name(state);
    let player = ctx.player_dir();
    let face = ctx.face;
    let hit_y = ctx.hit_y(target);
    let top_half = face == Dir::Down || (face != Dir::Up && hit_y > 0.5);

    if prop(state, "facing").is_some() {
        let horizontal_only = BlockInfo::of(state)
            .properties
            .iter()
            .find(|(p, _)| *p == "facing")
            .is_some_and(|(_, values)| !values.contains(&"up"));
        let attached_to_face = is_wall_mounted(state)
            || block_has_tag(state, "minecraft:shulker_boxes")
            || n.contains("amethyst_bud")
            || n == "minecraft:amethyst_cluster"
            || matches!(n, "minecraft:ladder" | "minecraft:tripwire_hook");
        let dir = if prop(state, "face").is_some() {
            // Buttons, levers, grindstones: on the floor or ceiling they face the way the
            // player looks, on a wall away from it.
            if face.is_horizontal() {
                face
            } else {
                player
            }
        } else if n.ends_with("_rod") {
            // End rods and lightning rods point away from what they are on, or continue
            // a rod they are placed against.
            let clicked = get(ctx.clicked);
            if same_block(clicked, state) && facing(clicked) == Some(face) {
                face.opposite()
            } else {
                face
            }
        } else if attached_to_face {
            if horizontal_only && !face.is_horizontal() {
                return None;
            }
            face
        } else if n == "minecraft:cocoa" {
            if !face.is_horizontal() {
                return None;
            }
            face.opposite()
        } else if n == "minecraft:hopper" {
            match face.opposite() {
                Dir::Up => Dir::Down,
                d => d,
            }
        } else if n == "minecraft:observer" {
            Dir::from_look(ctx.yaw, ctx.pitch)
        } else if !horizontal_only {
            // Pistons, dispensers, droppers, barrels, command blocks: the front faces the
            // player.
            Dir::from_look(ctx.yaw, ctx.pitch).opposite()
        } else if block_has_tag(state, "minecraft:trapdoors") {
            if face.is_horizontal() {
                face
            } else {
                player.opposite()
            }
        } else if is_stairs(state)
            || is_door(state)
            || is_bed(state)
            || block_has_tag(state, "minecraft:fence_gates")
            || matches!(
                n,
                "minecraft:campfire" | "minecraft:soul_campfire" | "minecraft:decorated_pot"
            )
        {
            player
        } else if block_has_tag(state, "minecraft:anvil") {
            player.clockwise()
        } else {
            player.opposite()
        };
        state = with(state, "facing", dir.name());
    }

    if prop(state, "face").is_some() {
        let attach = match face {
            Dir::Up => "floor",
            Dir::Down => "ceiling",
            _ => "wall",
        };
        state = with(state, "face", attach);
    }
    if prop(state, "axis").is_some() {
        state = with(state, "axis", face.axis());
    }
    if block_has_tag(state, "minecraft:trapdoors") {
        let top = if face.is_horizontal() {
            hit_y > 0.5
        } else {
            face == Dir::Down
        };
        state = with(state, "half", if top { "top" } else { "bottom" });
    } else if is_stairs(state) {
        state = with(state, "half", if top_half { "top" } else { "bottom" });
    }
    if block_has_tag(state, "minecraft:slabs") {
        let kind = if same_block(existing, state) {
            "double"
        } else if top_half {
            "top"
        } else {
            "bottom"
        };
        state = with(state, "type", kind);
        if kind == "double" {
            state = with_bool(state, "waterlogged", false);
        }
    }
    if prop(state, "rotation").is_some() {
        // Signs, banners and heads face the player.
        let segment = (((ctx.yaw + 180.0) * 16.0 / 360.0 + 0.5).floor() as i32).rem_euclid(16);
        state = with(state, "rotation", &segment.to_string());
    }
    if prop(state, "hanging").is_some() {
        state = with_bool(state, "hanging", face == Dir::Down);
    }
    if block_has_tag(state, "minecraft:rails") {
        let shape = if player.axis() == "x" {
            "east_west"
        } else {
            "north_south"
        };
        state = with(state, "shape", shape);
    }
    if is_door(state) {
        state = with(state, "hinge", door_hinge(state, ctx, target, get));
    }
    if is_chest(state) {
        state = chest_placement(state, ctx, target, get);
    }
    Some(state)
}

/// Doors hinge away from a door next to them (making a double door) and towards solid
/// blocks; otherwise on the side of the block the player clicked closer to.
fn door_hinge(
    state: u16,
    ctx: &PlaceContext,
    target: BlockPos,
    get: &mut impl FnMut(BlockPos) -> u16,
) -> &'static str {
    let facing = facing(state).unwrap_or(Dir::North);
    let left_pos = offset(target, facing.counter_clockwise());
    let right_pos = offset(target, facing.clockwise());
    let (left, right) = (get(left_pos), get(right_pos));
    let full = |p: BlockPos, get: &mut dyn FnMut(BlockPos) -> u16| {
        i32::from(levitas_registry::is_full_cube(get(p)))
    };
    let score = full(right_pos, get) + full(offset(right_pos, Dir::Up), get)
        - full(left_pos, get)
        - full(offset(left_pos, Dir::Up), get);
    let door_here = |s: u16| same_block(s, state) && prop(s, "half") == Some("lower");
    let (left_door, right_door) = (door_here(left), door_here(right));
    if (left_door && !right_door) || score > 0 {
        return "right";
    }
    if (right_door && !left_door) || score < 0 {
        return "left";
    }
    let [hx, _, hz] = ctx.hit(target);
    let (dx, dz) = match facing {
        Dir::North => (0, -1),
        Dir::South => (0, 1),
        Dir::West => (-1, 0),
        _ => (1, 0),
    };
    let left_side = (dx >= 0 || hz >= 0.5)
        && (dx <= 0 || hz <= 0.5)
        && (dz >= 0 || hx <= 0.5)
        && (dz <= 0 || hx >= 0.5);
    if left_side {
        "left"
    } else {
        "right"
    }
}

/// A chest placed next to a single chest facing the same way joins it into a double
/// chest, unless the player sneaks.
fn chest_placement(
    state: u16,
    ctx: &PlaceContext,
    target: BlockPos,
    get: &mut impl FnMut(BlockPos) -> u16,
) -> u16 {
    if ctx.sneaking {
        return with(state, "type", "single");
    }
    let facing = facing(state).unwrap_or(Dir::North);
    let single_facing = |s: u16| {
        same_block(s, state) && prop(s, "type") == Some("single") && self::facing(s) == Some(facing)
    };
    if single_facing(get(offset(target, facing.clockwise()))) {
        with(state, "type", "left")
    } else if single_facing(get(offset(target, facing.counter_clockwise()))) {
        with(state, "type", "right")
    } else {
        with(state, "type", "single")
    }
}
