//! Finding a way for a mob to walk somewhere: A* search over the blocks it can stand in.
//!
//! A mob can stand where its body fits (no collision boxes, no lava) on top of something
//! solid, or in water. From a spot it can walk to a neighbour on the same level, jump up
//! one block (if there is head room), or drop down up to three blocks. Searches are
//! capped so a mob that cannot get somewhere gives up quickly and heads for the closest
//! spot it found.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use levitas_registry::collision_boxes;

use super::mobs::Mob;
use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};

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

impl World {
    /// Whether a body `height` blocks tall fits at `pos` (its feet).
    fn body_fits(&self, pos: BlockPos, height: i32) -> bool {
        (0..height).all(|dy| {
            self.loaded_state(BlockPos::new(pos.x, pos.y + dy, pos.z))
                .is_some_and(|s| {
                    collision_boxes(s).is_empty()
                        && !fluid_at(s).is_some_and(|f| f.fluid == Fluid::Lava)
                })
        })
    }

    /// Whether a mob can stand with its feet at `pos`.
    fn can_stand(&self, pos: BlockPos, height: i32) -> bool {
        if !self.body_fits(pos, height) {
            return false;
        }
        let below = self.loaded_state(BlockPos::new(pos.x, pos.y - 1, pos.z));
        let feet = self.loaded_state(pos);
        below.is_some_and(|s| !collision_boxes(s).is_empty())
            || feet
                .and_then(fluid_at)
                .is_some_and(|f| f.fluid == Fluid::Water)
    }

    /// The spots reachable in one move from `pos`, with what each move costs.
    fn moves(&self, pos: BlockPos, height: i32) -> Vec<(BlockPos, u32)> {
        let mut out = Vec::with_capacity(4);
        for (dx, dz) in SIDES {
            let next = BlockPos::new(pos.x + dx, pos.y, pos.z + dz);
            if self.can_stand(next, height) {
                out.push((next, WALK));
                continue;
            }
            let up = BlockPos::new(next.x, next.y + 1, next.z);
            let head_room = self.body_fits(BlockPos::new(pos.x, pos.y + height, pos.z), 1);
            if head_room && self.can_stand(up, height) {
                out.push((up, JUMP));
                continue;
            }
            if !self.body_fits(next, height) {
                continue;
            }
            for drop in 1..=MAX_DROP {
                let down = BlockPos::new(next.x, next.y - drop, next.z);
                if !self.body_fits(down, height) {
                    break;
                }
                if self.can_stand(down, height) {
                    out.push((down, WALK + DROP_PER_BLOCK * drop as u32));
                    break;
                }
            }
        }
        out
    }

    /// The spots to walk through from `from` to `to` (both feet positions), ending at
    /// `to` or, if it cannot be reached, at the closest spot found. `None` if the mob
    /// cannot move at all.
    pub(super) fn find_path(
        &self,
        from: BlockPos,
        to: BlockPos,
        height: i32,
    ) -> Option<Vec<BlockPos>> {
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
            for (next, step) in self.moves(pos, height) {
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
        let height = mob.size().1.ceil() as i32;
        let feet = BlockPos::new(
            mob.pos[0].floor() as i32,
            (mob.pos[1] + 0.01).floor() as i32,
            mob.pos[2].floor() as i32,
        );
        mob.path_ticks = mob.path_ticks.saturating_sub(1);
        let goal_moved = mob.path.last().is_some_and(|end| {
            (end.x - goal.x).abs() + (end.y - goal.y).abs() + (end.z - goal.z).abs() > 2
        });
        if mob.path.is_empty() || mob.path_ticks == 0 || goal_moved {
            mob.path = self.find_path(feet, goal, height).unwrap_or_default();
            mob.path_ticks = REPATH_TICKS;
        }
        // Spots already reached are done.
        while let Some(&next) = mob.path.first() {
            let centre = [f64::from(next.x) + 0.5, f64::from(next.z) + 0.5];
            let close = (centre[0] - mob.pos[0]).hypot(centre[1] - mob.pos[2]) < 0.4;
            if close && (f64::from(next.y) - mob.pos[1]).abs() < 1.1 {
                mob.path.remove(0);
            } else {
                break;
            }
        }
        let aim = mob.path.first().copied().unwrap_or(goal);
        mob.target = Some(([f64::from(aim.x) + 0.5, f64::from(aim.z) + 0.5], 20));
    }
}
