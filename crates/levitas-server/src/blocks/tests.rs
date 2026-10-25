use std::collections::HashMap;

use levitas_protocol::BlockPos;
use levitas_registry::{block, BlockInfo};

use super::place::{place, wall_variant, PlaceContext};
use super::*;

/// A tiny world: stone below y = 0, air above, plus whatever a test sets.
struct World(HashMap<BlockPos, u16>);

impl World {
    fn new() -> Self {
        World(HashMap::new())
    }

    fn get(&self, pos: BlockPos) -> u16 {
        self.0
            .get(&pos)
            .copied()
            .unwrap_or(if pos.y < 0 { block::STONE } else { block::AIR })
    }

    fn set(&mut self, pos: BlockPos, state: u16) {
        self.0.insert(pos, state);
    }

    /// Places `block` like a player would and stores the result.
    fn place(&mut self, block: u16, ctx: PlaceContext) -> Option<Vec<(BlockPos, u16)>> {
        let snapshot = World(self.0.clone());
        let placed = place(block, &ctx, &mut |p| snapshot.get(p))?;
        for &(pos, state) in &placed {
            self.set(pos, state);
        }
        Some(placed)
    }

    /// Re-evaluates the block at `pos` as if a neighbour changed.
    fn update(&mut self, pos: BlockPos) -> u16 {
        let snapshot = World(self.0.clone());
        let state = updated(self.get(pos), pos, &mut |p| snapshot.get(p));
        self.set(pos, state);
        state
    }
}

fn state(name: &str) -> u16 {
    BlockInfo::by_name(name).unwrap().default_state
}

fn pos(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// Clicking the top of the block below `(x, 0, z)`, looking with this yaw.
fn on_ground(x: i32, z: i32, yaw: f32) -> PlaceContext {
    PlaceContext {
        clicked: pos(x, -1, z),
        face: Dir::Up,
        cursor: [0.5, 1.0, 0.5],
        yaw,
        pitch: 30.0,
        sneaking: false,
    }
}

/// Clicking the `face` side of the block at `clicked`, `cursor_y` high.
fn on_side(clicked: BlockPos, face: Dir, cursor_y: f32, yaw: f32) -> PlaceContext {
    PlaceContext {
        clicked,
        face,
        cursor: [0.5, cursor_y, 0.5],
        yaw,
        pitch: 0.0,
        sneaking: false,
    }
}

const NORTH_YAW: f32 = 180.0;
const EAST_YAW: f32 = 270.0;

#[test]
fn directions() {
    assert_eq!(Dir::from_yaw(0.0), Dir::South);
    assert_eq!(Dir::from_yaw(90.0), Dir::West);
    assert_eq!(Dir::from_yaw(-90.0), Dir::East);
    assert_eq!(Dir::from_yaw(181.0), Dir::North);
    assert_eq!(Dir::North.clockwise(), Dir::East);
    assert_eq!(Dir::from_look(0.0, 80.0), Dir::Down);
}

#[test]
fn furnaces_face_the_player_and_stairs_face_away() {
    let mut w = World::new();
    let placed = w
        .place(state("minecraft:furnace"), on_ground(0, 0, NORTH_YAW))
        .unwrap();
    assert_eq!(prop(placed[0].1, "facing"), Some("south"));
    let placed = w
        .place(state("minecraft:oak_stairs"), on_ground(2, 0, NORTH_YAW))
        .unwrap();
    assert_eq!(prop(placed[0].1, "facing"), Some("north"));
    assert_eq!(prop(placed[0].1, "half"), Some("bottom"));
}

#[test]
fn upside_down_stairs_and_top_slabs_from_high_side_clicks() {
    let mut w = World::new();
    w.set(pos(0, 0, 0), block::STONE);
    let ctx = on_side(pos(0, 0, 0), Dir::East, 0.8, NORTH_YAW);
    let stairs = w.place(state("minecraft:oak_stairs"), ctx).unwrap()[0].1;
    assert_eq!(prop(stairs, "half"), Some("top"));
    let ctx = on_side(pos(0, 0, 0), Dir::West, 0.8, NORTH_YAW);
    let slab = w.place(state("minecraft:oak_slab"), ctx).unwrap()[0].1;
    assert_eq!(prop(slab, "type"), Some("top"));
}

#[test]
fn slabs_merge_into_double_slabs() {
    let mut w = World::new();
    w.place(state("minecraft:oak_slab"), on_ground(0, 0, 0.0))
        .unwrap();
    // Clicking the top of the bottom slab fills it up.
    let ctx = PlaceContext {
        clicked: pos(0, 0, 0),
        face: Dir::Up,
        cursor: [0.5, 0.5, 0.5],
        yaw: 0.0,
        pitch: 60.0,
        sneaking: false,
    };
    let placed = w.place(state("minecraft:oak_slab"), ctx).unwrap();
    assert_eq!(placed[0].0, pos(0, 0, 0));
    assert_eq!(prop(placed[0].1, "type"), Some("double"));
    // A different slab does not merge; it goes on top.
    let placed = w.place(state("minecraft:stone_slab"), ctx).unwrap();
    assert_eq!(placed[0].0, pos(0, 1, 0));
}

#[test]
fn logs_follow_the_clicked_face() {
    let mut w = World::new();
    w.set(pos(0, 0, 0), block::STONE);
    let log = state("minecraft:oak_log");
    assert_eq!(
        prop(
            w.place(log, on_side(pos(0, 0, 0), Dir::East, 0.5, 0.0))
                .unwrap()[0]
                .1,
            "axis"
        ),
        Some("x")
    );
    assert_eq!(
        prop(
            w.place(log, on_side(pos(0, 0, 0), Dir::North, 0.5, 0.0))
                .unwrap()[0]
                .1,
            "axis"
        ),
        Some("z")
    );
    assert_eq!(
        prop(w.place(log, on_ground(5, 5, 0.0)).unwrap()[0].1, "axis"),
        Some("y")
    );
}

#[test]
fn torches_stand_or_hang_on_walls() {
    let mut w = World::new();
    w.set(pos(0, 0, 0), block::STONE);
    let torch = state("minecraft:torch");
    let standing = w.place(torch, on_ground(3, 3, 0.0)).unwrap()[0].1;
    assert_eq!(name(standing), "minecraft:torch");
    let wall = w
        .place(torch, on_side(pos(0, 0, 0), Dir::South, 0.5, 0.0))
        .unwrap()[0]
        .1;
    assert_eq!(name(wall), "minecraft:wall_torch");
    assert_eq!(prop(wall, "facing"), Some("south"));
    // Not under a block, and not in mid-air.
    assert!(w
        .place(torch, on_side(pos(0, 0, 0), Dir::Down, 0.0, 0.0))
        .is_none());
    assert_eq!(
        wall_variant(state("minecraft:oak_sign")).map(name),
        Some("minecraft:oak_wall_sign")
    );
    assert_eq!(
        wall_variant(state("minecraft:white_banner")).map(name),
        Some("minecraft:white_wall_banner")
    );
    assert_eq!(wall_variant(block::STONE), None);
}

#[test]
fn torches_fit_on_fence_posts() {
    let mut w = World::new();
    w.place(state("minecraft:oak_fence"), on_ground(0, 0, 0.0))
        .unwrap();
    let ctx = PlaceContext {
        clicked: pos(0, 0, 0),
        face: Dir::Up,
        cursor: [0.5, 1.0, 0.5],
        yaw: 0.0,
        pitch: 60.0,
        sneaking: false,
    };
    assert!(w.place(state("minecraft:torch"), ctx).is_some());
}

#[test]
fn doors_take_two_blocks_and_hinge_next_to_each_other() {
    let mut w = World::new();
    let door = state("minecraft:oak_door");
    let placed = w.place(door, on_ground(0, 0, NORTH_YAW)).unwrap();
    assert_eq!(placed.len(), 2);
    assert_eq!(
        placed[1],
        (pos(0, 1, 0), with(placed[0].1, "half", "upper"))
    );
    assert_eq!(prop(placed[0].1, "facing"), Some("north"));
    let first_hinge = prop(placed[0].1, "hinge").unwrap();
    // A door on the other side of the first one hinges the opposite way: a double door.
    let neighbor_x = if first_hinge == "left" { 1 } else { -1 };
    let second = w.place(door, on_ground(neighbor_x, 0, NORTH_YAW)).unwrap()[0].1;
    assert_ne!(prop(second, "hinge"), Some(first_hinge));
    // No room under a ceiling.
    w.set(pos(5, 1, 5), block::STONE);
    assert!(w.place(door, on_ground(5, 5, NORTH_YAW)).is_none());
}

#[test]
fn beds_take_two_blocks() {
    let mut w = World::new();
    let placed = w
        .place(state("minecraft:red_bed"), on_ground(0, 0, EAST_YAW))
        .unwrap();
    assert_eq!(placed.len(), 2);
    assert_eq!(prop(placed[0].1, "part"), Some("foot"));
    assert_eq!(placed[1].0, pos(1, 0, 0));
    assert_eq!(prop(placed[1].1, "part"), Some("head"));
}

#[test]
fn fences_connect_to_fences_and_solid_blocks() {
    let mut w = World::new();
    let fence = state("minecraft:oak_fence");
    w.place(fence, on_ground(0, 0, 0.0)).unwrap();
    w.set(pos(-1, 0, 0), block::STONE);
    w.set(pos(0, 0, -1), state("minecraft:oak_leaves"));
    let second = w.place(fence, on_ground(1, 0, 0.0)).unwrap()[0].1;
    assert_eq!(prop(second, "west"), Some("true"));
    let first = w.update(pos(0, 0, 0));
    assert_eq!(prop(first, "east"), Some("true"), "joins the new fence");
    assert_eq!(prop(first, "west"), Some("true"), "joins stone");
    assert_eq!(prop(first, "north"), Some("false"), "not leaves");
}

#[test]
fn stairs_form_corners() {
    let mut w = World::new();
    let stairs = state("minecraft:oak_stairs");
    // Stairs facing north with stairs facing east in front of them: an outer corner.
    w.place(stairs, on_ground(0, -1, EAST_YAW)).unwrap();
    let corner = w.place(stairs, on_ground(0, 0, NORTH_YAW)).unwrap()[0].1;
    assert_eq!(prop(corner, "shape"), Some("outer_right"));
}

#[test]
fn chests_pair_up_unless_sneaking() {
    let mut w = World::new();
    let chest = state("minecraft:chest");
    let first = w.place(chest, on_ground(0, 0, NORTH_YAW)).unwrap()[0].1;
    assert_eq!(prop(first, "type"), Some("single"));
    let second = w.place(chest, on_ground(1, 0, NORTH_YAW)).unwrap()[0].1;
    assert_ne!(prop(second, "type"), Some("single"));
    let first = w.update(pos(0, 0, 0));
    assert_ne!(prop(first, "type"), Some("single"));
    assert_eq!(chest_partner(first, pos(0, 0, 0)), Some(pos(1, 0, 0)));
    assert_eq!(chest_partner(second, pos(1, 0, 0)), Some(pos(0, 0, 0)));
    let mut sneak = on_ground(-1, 0, NORTH_YAW);
    sneak.sneaking = true;
    assert_eq!(
        prop(w.place(chest, sneak).unwrap()[0].1, "type"),
        Some("single")
    );
    // Breaking one half leaves the other single.
    w.set(pos(1, 0, 0), block::AIR);
    assert_eq!(prop(w.update(pos(0, 0, 0)), "type"), Some("single"));
}

#[test]
fn unsupported_blocks_pop_off() {
    let mut w = World::new();
    w.set(pos(0, 0, 0), block::STONE);
    w.place(
        state("minecraft:torch"),
        on_side(pos(0, 0, 0), Dir::East, 0.5, 0.0),
    )
    .unwrap();
    w.place(state("minecraft:oak_door"), on_ground(5, 0, 0.0))
        .unwrap();
    w.place(state("minecraft:poppy"), on_ground(8, 0, 0.0));
    w.set(pos(0, 0, 0), block::AIR);
    assert_eq!(w.update(pos(1, 0, 0)), block::AIR, "wall torch falls off");
    w.set(pos(5, 0, 0), block::AIR);
    assert_eq!(
        w.update(pos(5, 1, 0)),
        block::AIR,
        "upper door half goes with the lower"
    );
    // Flowers need soil: stone below is not enough.
    assert!(w
        .place(state("minecraft:poppy"), on_ground(9, 0, 0.0))
        .is_none());
}

#[test]
fn buttons_and_pistons() {
    let mut w = World::new();
    w.set(pos(0, 0, 0), block::STONE);
    let button = w
        .place(
            state("minecraft:stone_button"),
            on_side(pos(0, 0, 0), Dir::West, 0.5, 0.0),
        )
        .unwrap()[0]
        .1;
    assert_eq!(
        (prop(button, "face"), prop(button, "facing")),
        (Some("wall"), Some("west"))
    );
    let floor = w
        .place(state("minecraft:stone_button"), on_ground(4, 4, NORTH_YAW))
        .unwrap()[0]
        .1;
    assert_eq!(
        (prop(floor, "face"), prop(floor, "facing")),
        (Some("floor"), Some("north"))
    );
    let mut look_down = on_ground(6, 6, 0.0);
    look_down.pitch = 80.0;
    let piston = w.place(state("minecraft:piston"), look_down).unwrap()[0].1;
    assert_eq!(prop(piston, "facing"), Some("up"));
}
