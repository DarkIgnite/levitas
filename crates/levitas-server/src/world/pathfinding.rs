//! Finding a way for a mob to walk somewhere: A* search over the blocks it can stand in.
//!
//! A mob can stand where its body fits (no collision boxes) on top of something solid, or
//! in water. From a spot it can walk to a neighbour on the same level, jump up one block
//! (if there is head room), or drop down up to three blocks. Searches are capped so a mob
//! that cannot get somewhere gives up quickly and heads for the closest spot it found.
//!
//! Spots cost more the more they hurt, following the pathfinding penalties of the
//! Minecraft Wiki's "Mob AI" page (in blocks walked, -1 for never):
//! - lava, powder snow, cactus and sweet berry bushes, and the top of fences, walls and
//!   closed fence gates: never;
//! - fire, magma blocks, lit campfires and lava cauldrons underfoot or in the body
//!   ("damage - fire"): 16; next to one ("danger - fire"): 8;
//! - water: 8, and 8 next to water ("water border"); next to cactus or a berry bush: 8;
//! - closed doors are walls, except wooden and copper doors for a mob that can open
//!   them (villagers) or break them (some zombies); open doors cost nothing.
//!
//! Mobs override some of these as that page lists: animals never path through fire,
//! villagers and piglins never through fire and avoid being near it twice as much, water
//! costs nothing to fish, chickens, axolotls, turtles and drowned, foxes do not mind
//! berry bushes, goats and wolves keep off powder snow, striders walk on lava, and so on.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::OnceLock;

use levitas_registry::{collision_boxes, BLOCK_STATE_COUNT};

use super::mobs::{Mob, MobKind};
use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::blocks::{self, prop};

/// Most spots looked at per search.
const MAX_NODES: usize = 600;
/// Farthest a mob drops down in one go.
const MAX_DROP: i32 = 3;
/// Costs, in tenths of a block walked.
const WALK: u32 = 10;
const JUMP: u32 = 20;
const DROP_PER_BLOCK: u32 = 5;
/// Ticks before a path is worked out again.
const REPATH_TICKS: u32 = 20;

const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const AROUND: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// What a block state means to a walking mob, as bits.
const SOLID: u16 = 1;
const DOOR: u16 = 1 << 1;
const DOOR_OPEN: u16 = 1 << 2;
/// A door mobs can open: wooden and copper ones (`mob_interactable_doors`).
const MOB_DOOR: u16 = 1 << 3;
/// Fences, walls and closed fence gates: too tall to stand on.
const TALL: u16 = 1 << 4;
/// Fire, magma blocks, lit campfires and lava cauldrons.
const FIRE: u16 = 1 << 5;
/// Cactus and sweet berry bushes.
const PRICKLY: u16 = 1 << 6;
const POWDER_SNOW: u16 = 1 << 7;
const WATER: u16 = 1 << 8;
const LAVA: u16 = 1 << 9;

/// The bits of a block state, worked out once for every state.
pub(super) fn flags(state: u16) -> u16 {
    static TABLE: OnceLock<Vec<u16>> = OnceLock::new();
    let table = TABLE.get_or_init(|| (0..BLOCK_STATE_COUNT).map(|s| classify(s as u16)).collect());
    table.get(usize::from(state)).copied().unwrap_or(SOLID)
}

fn classify(state: u16) -> u16 {
    let name = blocks::name(state);
    let tag = |t: &str| block_has_tag(state, t);
    let open = prop(state, "open") == Some("true");
    let mut f = 0;
    if !collision_boxes(state).is_empty() {
        f |= SOLID;
    }
    if tag("minecraft:doors") {
        f |= DOOR;
        if open {
            f |= DOOR_OPEN;
        }
        if tag("minecraft:mob_interactable_doors") {
            f |= MOB_DOOR;
        }
    }
    if tag("minecraft:fences") || tag("minecraft:walls") || (tag("minecraft:fence_gates") && !open)
    {
        f |= TALL;
    }
    let lit_campfire = tag("minecraft:campfires") && prop(state, "lit") == Some("true");
    if tag("minecraft:fire")
        || lit_campfire
        || name == "minecraft:magma_block"
        || name == "minecraft:lava_cauldron"
    {
        f |= FIRE;
    }
    if name == "minecraft:cactus" || name == "minecraft:sweet_berry_bush" {
        f |= PRICKLY;
    }
    if name == "minecraft:powder_snow" {
        f |= POWDER_SNOW;
    }
    match fluid_at(state).map(|f| f.fluid) {
        Some(Fluid::Water) => f |= WATER,
        Some(Fluid::Lava) => f |= LAVA,
        _ => {}
    }
    f
}

/// Whether a state is a closed door that mobs can open or break.
pub(super) fn closed_mob_door(state: u16) -> bool {
    let f = flags(state);
    f & MOB_DOOR != 0 && f & DOOR_OPEN == 0
}

/// How much a mob minds each kind of spot, in blocks walked; negative for never.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PathRules {
    pub water: i8,
    pub water_border: i8,
    pub lava: i8,
    pub damage_fire: i8,
    pub danger_fire: i8,
    pub damage_other: i8,
    pub danger_other: i8,
    pub powder_snow: i8,
    pub danger_powder_snow: i8,
    pub open_door: i8,
    /// Walks through closed wooden and copper doors (opening or breaking them).
    pub closed_doors: bool,
}

impl PathRules {
    /// The wiki's default penalties.
    pub const DEFAULT: PathRules = PathRules {
        water: 8,
        water_border: 8,
        lava: -1,
        damage_fire: 16,
        danger_fire: 8,
        damage_other: -1,
        danger_other: 8,
        powder_snow: -1,
        danger_powder_snow: 0,
        open_door: 0,
        closed_doors: false,
    };

    /// The penalties for a kind of mob.
    pub(super) fn of_kind(kind: MobKind) -> PathRules {
        use MobKind::*;
        let mut r = PathRules::DEFAULT;
        let animal = !kind.is_monster()
            && !matches!(
                kind,
                Villager | WanderingTrader | IronGolem | SnowGolem | CopperGolem
            );
        if animal {
            r.damage_fire = -1;
        }
        if kind.swims()
            || matches!(
                kind,
                Axolotl | Turtle | Chicken | Drowned | Guardian | ElderGuardian
            )
        {
            r.water = 0;
            r.water_border = 0;
        }
        match kind {
            Frog => {
                r.water = 4;
                r.water_border = 4;
            }
            Sniffer => {
                r.water = -1;
                r.danger_powder_snow = -1;
            }
            Enderman => r.water = -1,
            Villager | WanderingTrader | Piglin | PiglinBrute => {
                r.damage_fire = -1;
                r.danger_fire = 16;
            }
            Parrot => {
                r.damage_fire = -1;
                r.danger_fire = -1;
            }
            Fox => {
                r.damage_other = 0;
                r.danger_other = 0;
            }
            Goat | Wolf => {
                r.powder_snow = -1;
                r.danger_powder_snow = -1;
            }
            Strider => {
                r.water = -1;
                r.lava = 0;
                r.damage_fire = 0;
                r.danger_fire = 0;
            }
            Blaze => {
                r.water = -1;
                r.lava = 8;
                r.damage_fire = 8;
                r.danger_fire = 8;
            }
            Breeze => r.damage_fire = -1,
            Warden => {
                r.damage_fire = 0;
                r.danger_fire = 0;
                r.damage_other = 8;
                r.powder_snow = 8;
                r.lava = 8;
            }
            WitherSkeleton | ZombifiedPiglin => r.lava = 8,
            Turtle => r.open_door = -1,
            _ => {}
        }
        if kind == Villager {
            r.closed_doors = true;
        }
        r
    }

    /// The penalties for one mob: a zombie that can break doors walks through them.
    pub(super) fn of(mob: &Mob) -> PathRules {
        let mut r = PathRules::of_kind(mob.kind);
        if mob.ai.break_doors == Some(true) {
            r.closed_doors = true;
        }
        r
    }
}

/// The penalty `m` as an addition to a spot's cost, or `None` for never.
fn penalty(m: i8) -> Option<u32> {
    (m >= 0).then(|| u32::from(m.unsigned_abs()))
}

/// What a block in a mob's body costs, or `None` if the body cannot be there.
fn body_penalty(f: u16, r: &PathRules) -> Option<u32> {
    if f & DOOR != 0 {
        if f & DOOR_OPEN != 0 {
            return penalty(r.open_door);
        }
        return (f & MOB_DOOR != 0 && r.closed_doors).then_some(0);
    }
    if f & POWDER_SNOW != 0 {
        return penalty(r.powder_snow);
    }
    if f & SOLID != 0 {
        return None;
    }
    if f & LAVA != 0 {
        return penalty(r.lava);
    }
    if f & FIRE != 0 {
        return penalty(r.damage_fire);
    }
    if f & PRICKLY != 0 {
        return penalty(r.damage_other);
    }
    if f & WATER != 0 {
        return penalty(r.water);
    }
    Some(0)
}

/// One search: the world, who is walking, and what each spot was found to cost.
struct Search<'a> {
    world: &'a World,
    height: i32,
    rules: PathRules,
    spots: HashMap<BlockPos, Option<u32>>,
}

impl Search<'_> {
    fn flags_at(&self, pos: BlockPos) -> Option<u16> {
        self.world.loaded_state(pos).map(flags)
    }

    /// Whether the body can be in the block at `pos` at all.
    fn clear(&self, pos: BlockPos) -> bool {
        self.flags_at(pos)
            .is_some_and(|f| body_penalty(f, &self.rules).is_some())
    }

    fn column_clear(&self, pos: BlockPos) -> bool {
        (0..self.height).all(|dy| self.clear(BlockPos::new(pos.x, pos.y + dy, pos.z)))
    }

    /// The extra cost of standing with the feet at `pos` (in tenths of a block), or
    /// `None` if the mob cannot stand there.
    fn spot(&mut self, pos: BlockPos) -> Option<u32> {
        if let Some(&known) = self.spots.get(&pos) {
            return known;
        }
        let cost = self.work_out(pos);
        self.spots.insert(pos, cost);
        cost
    }

    fn work_out(&self, pos: BlockPos) -> Option<u32> {
        let r = &self.rules;
        let mut worst = 0;
        let mut feet = 0;
        for dy in 0..self.height {
            let f = self.flags_at(BlockPos::new(pos.x, pos.y + dy, pos.z))?;
            if dy == 0 {
                feet = f;
            }
            worst = worst.max(body_penalty(f, r)?);
        }
        let below = self.flags_at(BlockPos::new(pos.x, pos.y - 1, pos.z))?;
        if below & TALL != 0 {
            return None;
        }
        if below & SOLID == 0 && feet & WATER == 0 {
            return None; // nothing to stand on
        }
        if below & FIRE != 0 {
            worst = worst.max(penalty(r.damage_fire)?);
        }
        // What is next to the spot, at the feet and just below them.
        let mut near = 0;
        for dy in [0, -1] {
            for (dx, dz) in AROUND {
                if let Some(f) = self.flags_at(BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)) {
                    near |= f;
                }
            }
        }
        if near & (FIRE | LAVA) != 0 {
            worst = worst.max(penalty(r.danger_fire)?);
        }
        if near & PRICKLY != 0 {
            worst = worst.max(penalty(r.danger_other)?);
        }
        if near & POWDER_SNOW != 0 {
            worst = worst.max(penalty(r.danger_powder_snow)?);
        }
        if near & WATER != 0 && feet & WATER == 0 {
            worst = worst.max(penalty(r.water_border)?);
        }
        Some(worst * WALK)
    }

    /// The spots reachable in one move from `pos`, with what each move costs.
    fn moves(&mut self, pos: BlockPos, out: &mut Vec<(BlockPos, u32)>) {
        out.clear();
        let height = self.height;
        let head_room = self.clear(BlockPos::new(pos.x, pos.y + height, pos.z));
        for (dx, dz) in SIDES {
            let next = BlockPos::new(pos.x + dx, pos.y, pos.z + dz);
            if let Some(extra) = self.spot(next) {
                out.push((next, WALK + extra));
                continue;
            }
            let up = BlockPos::new(next.x, next.y + 1, next.z);
            if head_room {
                if let Some(extra) = self.spot(up) {
                    out.push((up, JUMP + extra));
                    continue;
                }
            }
            if !self.column_clear(next) {
                continue;
            }
            for drop in 1..=MAX_DROP {
                let down = BlockPos::new(next.x, next.y - drop, next.z);
                if !self.column_clear(down) {
                    break;
                }
                if let Some(extra) = self.spot(down) {
                    out.push((down, WALK + DROP_PER_BLOCK * drop as u32 + extra));
                    break;
                }
            }
        }
    }
}

impl World {
    /// What standing with the feet at `pos` costs a mob following `rules` (0 for a
    /// harmless spot, in tenths of a block), or `None` if it cannot stand there.
    pub(super) fn spot_cost(&self, pos: BlockPos, height: i32, rules: PathRules) -> Option<u32> {
        Search {
            world: self,
            height,
            rules,
            spots: HashMap::new(),
        }
        .work_out(pos)
    }

    /// The spots to walk through from `from` to `to` (both feet positions), ending at
    /// `to` or, if it cannot be reached, at the closest spot found. `None` if the mob
    /// cannot move at all. Uses the default penalties.
    #[cfg(test)]
    pub(super) fn find_path(
        &self,
        from: BlockPos,
        to: BlockPos,
        height: i32,
    ) -> Option<Vec<BlockPos>> {
        self.find_path_for(from, to, height, PathRules::DEFAULT)
    }

    /// Like `find_path`, with the penalties of a particular mob.
    pub(super) fn find_path_for(
        &self,
        from: BlockPos,
        to: BlockPos,
        height: i32,
        rules: PathRules,
    ) -> Option<Vec<BlockPos>> {
        let mut search = Search {
            world: self,
            height,
            rules,
            spots: HashMap::new(),
        };
        let estimate = |p: BlockPos| {
            ((p.x - to.x).unsigned_abs()
                + (p.y - to.y).unsigned_abs()
                + (p.z - to.z).unsigned_abs())
                * WALK
        };
        let mut open = BinaryHeap::new();
        let mut cost: HashMap<BlockPos, u32> = HashMap::new();
        let mut came_from: HashMap<BlockPos, BlockPos> = HashMap::new();
        open.push(Reverse((estimate(from), 0u32, (from.x, from.y, from.z))));
        cost.insert(from, 0);
        let mut best = (estimate(from), from);
        let mut seen = 0;
        let mut moves = Vec::with_capacity(4);
        while let Some(Reverse((_, spent, (x, y, z)))) = open.pop() {
            let pos = BlockPos::new(x, y, z);
            if spent > cost.get(&pos).copied().unwrap_or(u32::MAX) {
                continue; // a cheaper way here was found meanwhile
            }
            if pos == to {
                best = (0, pos);
                break;
            }
            seen += 1;
            if seen > MAX_NODES {
                break;
            }
            search.moves(pos, &mut moves);
            for &(next, step) in &moves {
                let total = spent + step;
                if total < cost.get(&next).copied().unwrap_or(u32::MAX) {
                    cost.insert(next, total);
                    came_from.insert(next, pos);
                    let left = estimate(next);
                    if left < best.0 {
                        best = (left, next);
                    }
                    open.push(Reverse((total + left, total, (next.x, next.y, next.z))));
                }
            }
        }
        let mut path = vec![best.1];
        while let Some(&previous) = came_from.get(path.last().expect("not empty")) {
            if previous == from {
                break;
            }
            path.push(previous);
        }
        if path == [from] {
            return None;
        }
        path.reverse();
        Some(path)
    }

    /// Steers a mob along a path to `goal`, working the path out again now and then.
    /// Without a path it heads straight for the goal.
    pub(super) fn follow_path(&mut self, mob: &mut Mob, goal: BlockPos) {
        if mob.leash_pull {
            return; // its stretched lead steers it (`leads.rs`)
        }
        let goal_moved = mob.path.last().is_some_and(|end| {
            (end.x - goal.x).abs() + (end.y - goal.y).abs() + (end.z - goal.z).abs() > 2
        });
        self.steer_along(mob, goal, mob.path.is_empty() || goal_moved);
    }

    /// Steers a mob along its path to `goal`, working the path out again when `search`
    /// or once the path is old.
    pub(super) fn steer_along(&mut self, mob: &mut Mob, goal: BlockPos, search: bool) {
        let height = mob.size().1.ceil() as i32;
        let feet = mob_feet(mob);
        mob.path_ticks = mob.path_ticks.saturating_sub(1);
        if search || mob.path_ticks == 0 {
            mob.path = self
                .find_path_for(feet, goal, height, PathRules::of(mob))
                .unwrap_or_default();
            mob.path_ticks = REPATH_TICKS;
        }
        advance_path(mob.pos, &mut mob.path);
        let aim = mob.path.first().copied().unwrap_or(goal);
        mob.target = Some(([f64::from(aim.x) + 0.5, f64::from(aim.z) + 0.5], 20));
    }
}

/// The block a mob's feet are in.
pub(super) fn mob_feet(mob: &Mob) -> BlockPos {
    BlockPos::new(
        mob.pos[0].floor() as i32,
        (mob.pos[1] + 0.01).floor() as i32,
        mob.pos[2].floor() as i32,
    )
}

/// Drops the spots of `path` a mob at `pos` has already reached.
pub(super) fn advance_path(pos: [f64; 3], path: &mut Vec<BlockPos>) {
    let reached = path
        .iter()
        .take_while(|next| {
            let centre = [f64::from(next.x) + 0.5, f64::from(next.z) + 0.5];
            let close = (centre[0] - pos[0]).hypot(centre[1] - pos[2]) < 0.4;
            close && (f64::from(next.y) - pos[1]).abs() < 1.1
        })
        .count();
    path.drain(..reached);
}
