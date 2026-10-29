//! Water and lava flowing, mixing, and buckets.
//!
//! A fluid block that may need to move gets a fluid tick, scheduled whenever it or a
//! block next to it changes. On its tick a flowing block recomputes its level from its
//! neighbours (and dries up when nothing feeds it), then the fluid spreads: down first,
//! otherwise sideways towards the nearest way down. See [`crate::blocks::fluid`] for
//! the rules about levels and which blocks fluid may enter.

use std::collections::{BTreeMap, HashSet};

use levitas_nbt::{Compound, Tag};

use super::*;
use crate::blocks::fluid::{can_hold_fluid, can_waterlog, fluid_at, Fluid, FluidState};
use crate::blocks::{self, offset, prop, with_bool, Dir};
use levitas_registry::collision_boxes;

const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::South, Dir::West, Dir::East];
/// Level event for lava hissing as water hardens it.
const LEVEL_EVENT_LAVA_FIZZ: i32 = 1501;
/// Fluid ticks run per game tick at most, as a guard against runaway flows.
const MAX_FLUID_TICKS: usize = 65_536;

/// Pending fluid ticks, by the game tick they are due.
#[derive(Default)]
pub struct FluidTicks {
    due: BTreeMap<u64, Vec<(BlockPos, Fluid)>>,
    pending: HashSet<(BlockPos, Fluid)>,
    /// Lava next to a change, checked for meeting water at the next tick.
    lava_checks: Vec<BlockPos>,
}

impl World {
    /// The block at `pos` if its chunk is loaded; never loads or generates chunks.
    pub(super) fn loaded_block(&self, pos: BlockPos) -> Option<u16> {
        if !(MIN_Y..=MAX_Y).contains(&pos.y) {
            return None;
        }
        let chunk = self.chunks.get(&pos.chunk())?;
        Some(chunk.get((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize))
    }

    /// Lets fluids at and around `pos` react to a change there.
    pub(super) fn fluids_changed_around(&mut self, pos: BlockPos) {
        for at in std::iter::once(pos).chain(Dir::ALL.map(|d| offset(pos, d))) {
            let Some(fluid) = self.loaded_block(at).and_then(fluid_at) else {
                continue;
            };
            if fluid.fluid == Fluid::Lava {
                self.fluid_ticks.lava_checks.push(at);
            }
            self.schedule_fluid(at, fluid.fluid);
        }
    }

    fn schedule_fluid(&mut self, pos: BlockPos, fluid: Fluid) {
        self.schedule_fluid_in(pos, fluid, self.fluid_delay(fluid));
    }

    fn schedule_fluid_in(&mut self, pos: BlockPos, fluid: Fluid, delay: u64) {
        if self.fluid_ticks.pending.insert((pos, fluid)) {
            let due = self.tick + delay;
            self.fluid_ticks
                .due
                .entry(due)
                .or_default()
                .push((pos, fluid));
        }
    }

    pub(super) fn tick_fluids(&mut self) {
        for pos in std::mem::take(&mut self.fluid_ticks.lava_checks) {
            self.lava_meets_water(pos);
        }
        let mut budget = MAX_FLUID_TICKS;
        while let Some(entry) = self.fluid_ticks.due.first_entry() {
            if *entry.key() > self.tick || budget == 0 {
                break;
            }
            for (pos, fluid) in entry.remove() {
                self.fluid_ticks.pending.remove(&(pos, fluid));
                budget = budget.saturating_sub(1);
                if self.chunks.contains_key(&pos.chunk()) {
                    self.flow(pos, fluid);
                }
            }
        }
    }

    /// One flow step of the fluid at `pos`.
    fn flow(&mut self, pos: BlockPos, fluid: Fluid) {
        let Some(mut state) = self
            .loaded_block(pos)
            .and_then(fluid_at)
            .filter(|f| f.fluid == fluid)
        else {
            return;
        };
        if !state.source {
            match self.fed_level(pos, fluid) {
                None => {
                    self.set_block_updating(pos, block::AIR);
                    return;
                }
                Some(fed) if fed != state => {
                    self.set_block_updating(pos, fed.block());
                    state = fed;
                }
                Some(_) => {}
            }
        }
        self.spread(pos, state);
    }

    /// What a flowing (non-source) block at `pos` becomes, judging by what feeds it:
    /// fluid above makes it fall, the strongest neighbour sideways gives it a level one
    /// step weaker, and water between two sources becomes a source itself.
    fn fed_level(&self, pos: BlockPos, fluid: Fluid) -> Option<FluidState> {
        let mut strongest = 0;
        let mut sources = 0;
        for dir in HORIZONTAL {
            let Some(next) = self.loaded_block(offset(pos, dir)).and_then(fluid_at) else {
                continue;
            };
            if next.fluid == fluid {
                sources += usize::from(next.source);
                strongest = strongest.max(next.amount);
            }
        }
        if fluid == Fluid::Water && sources >= 2 {
            let below = self
                .loaded_block(offset(pos, Dir::Down))
                .unwrap_or(block::AIR);
            let below_is_source = fluid_at(below).is_some_and(|f| f.source && f.fluid == fluid);
            if below_is_source || !collision_boxes(below).is_empty() {
                return Some(FluidState::source(fluid));
            }
        }
        let above = self.loaded_block(offset(pos, Dir::Up)).and_then(fluid_at);
        if above.is_some_and(|f| f.fluid == fluid) {
            return Some(FluidState::falling(fluid));
        }
        let amount = strongest.saturating_sub(self.drop_off(fluid));
        (amount > 0).then(|| FluidState::flowing(fluid, amount))
    }

    fn spread(&mut self, pos: BlockPos, state: FluidState) {
        let below = offset(pos, Dir::Down);
        let Some(below_state) = self.loaded_block(below) else {
            return;
        };
        let falls = can_hold_fluid(below_state)
            || (state.fluid == Fluid::Lava
                && fluid_at(below_state).is_some_and(|f| f.fluid == Fluid::Water));
        if falls {
            if state.fluid == Fluid::Lava && fluid_at(below_state).is_some() {
                // Lava pouring onto water turns it to stone.
                self.set_block_updating(below, block::STONE);
                self.fizz(below);
            } else {
                self.flow_into(below, FluidState::falling(state.fluid));
            }
            if self.source_neighbors(pos, state.fluid) >= 3 {
                self.spread_sideways(pos, state);
            }
        } else if state.source || !self.is_hole(below, state.fluid) {
            self.spread_sideways(pos, state);
        }
    }

    fn source_neighbors(&self, pos: BlockPos, fluid: Fluid) -> usize {
        HORIZONTAL
            .iter()
            .filter(|&&d| {
                self.loaded_block(offset(pos, d))
                    .and_then(fluid_at)
                    .is_some_and(|f| f.source && f.fluid == fluid)
            })
            .count()
    }

    /// Whether fluid could go down at `pos`: it holds the same fluid already, or fluid
    /// could flow into it.
    fn is_hole(&self, pos: BlockPos, fluid: Fluid) -> bool {
        self.loaded_block(pos).is_some_and(|state| {
            fluid_at(state).is_some_and(|f| f.fluid == fluid) || can_hold_fluid(state)
        })
    }

    fn spread_sideways(&mut self, pos: BlockPos, state: FluidState) {
        let amount = if state.falling {
            7
        } else {
            state.amount.saturating_sub(self.drop_off(state.fluid))
        };
        if amount == 0 {
            return;
        }
        for dir in self.flow_directions(pos, state.fluid) {
            self.flow_into(offset(pos, dir), FluidState::flowing(state.fluid, amount));
        }
    }

    /// The sideways directions fluid flows from `pos`: those leading soonest to a way
    /// down, or every open direction if there is none within reach.
    fn flow_directions(&self, pos: BlockPos, fluid: Fluid) -> Vec<Dir> {
        let mut best = i32::MAX;
        let mut dirs = Vec::new();
        for dir in HORIZONTAL {
            let next = offset(pos, dir);
            if !self.loaded_block(next).is_some_and(can_hold_fluid) {
                continue;
            }
            let distance = if self.is_hole(offset(next, Dir::Down), fluid) {
                0
            } else {
                self.slope_distance(next, 1, dir.opposite(), fluid)
            };
            if distance < best {
                best = distance;
                dirs.clear();
            }
            if distance == best {
                dirs.push(dir);
            }
        }
        dirs
    }

    fn slope_distance(&self, pos: BlockPos, depth: i32, from: Dir, fluid: Fluid) -> i32 {
        let mut best = 1000;
        for dir in HORIZONTAL.into_iter().filter(|&d| d != from) {
            let next = offset(pos, dir);
            if !self.loaded_block(next).is_some_and(can_hold_fluid) {
                continue;
            }
            if self.is_hole(offset(next, Dir::Down), fluid) {
                return depth;
            }
            if depth < self.fluid_slope_distance(fluid) {
                best = best.min(self.slope_distance(next, depth + 1, dir.opposite(), fluid));
            }
        }
        best
    }

    /// Fluid flows into `pos`, washing away what was there.
    fn flow_into(&mut self, pos: BlockPos, state: FluidState) {
        let Some(old) = self.loaded_block(pos) else {
            return;
        };
        if !can_hold_fluid(old) {
            return;
        }
        if !is_air(old) {
            self.pop_block(pos, old);
        }
        self.set_block_updating(pos, state.block());
    }

    /// Lava touching water from above or the side hardens: a source into obsidian,
    /// flowing lava into cobblestone. On soul soil next to blue ice it becomes basalt.
    fn lava_meets_water(&mut self, pos: BlockPos) {
        let Some(lava) = self
            .loaded_block(pos)
            .and_then(fluid_at)
            .filter(|f| f.fluid == Fluid::Lava)
        else {
            return;
        };
        let on_soul_soil = self.loaded_block(offset(pos, Dir::Down)) == Some(block::SOUL_SOIL);
        for dir in [Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East] {
            let Some(next) = self.loaded_block(offset(pos, dir)) else {
                continue;
            };
            let hardened = if fluid_at(next).is_some_and(|f| f.fluid == Fluid::Water) {
                if lava.source {
                    block::OBSIDIAN
                } else {
                    block::COBBLESTONE
                }
            } else if on_soul_soil && next == block::BLUE_ICE {
                BlockInfo::of(block::BASALT).default_state
            } else {
                continue;
            };
            self.set_block_updating(pos, hardened);
            self.fizz(pos);
            return;
        }
    }

    fn fizz(&mut self, pos: BlockPos) {
        self.broadcast(
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(LEVEL_EVENT_LAVA_FIZZ)
                .position(pos)
                .i32(0)
                .bool(false),
            None,
        );
    }

    /// Using a bucket: an empty one scoops up the fluid source the player looks at, a
    /// full one pours its fluid out where the player looks.
    /// A glass bottle used on water fills with it. Returns whether a bottle was used.
    pub(super) fn fill_bottle(&mut self, id: i32, hand: i32) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let slot = p.hand_slot(hand);
        let stack = p.inventory.slots[slot];
        if ItemInfo::get(stack.item).is_none_or(|i| i.name != "minecraft:glass_bottle") {
            return false;
        }
        let (eye, look, reach) = p.eye_ray();
        let survival = p.survival();
        let Some((pos, _)) = self.ray_cast(eye, look, reach, true) else {
            return true;
        };
        let water = fluid_at(self.block(pos)).is_some_and(|f| f.fluid == Fluid::Water);
        if !water {
            return true;
        }
        self.play_sound("minecraft:item.bottle.fill", pos, None);
        let bottle = crate::potions::stack("minecraft:potion", "water");
        let p = self.players.get_mut(&id).expect("player exists");
        if !survival {
            let (changed, _) = p.inventory.add(bottle);
            let updates: Vec<Writer> = changed
                .into_iter()
                .map(|s| p.inventory.set_slot_packet(s))
                .collect();
            for update in updates {
                self.send(id, update);
            }
            return true;
        }
        if stack.count == 1 {
            p.inventory.slots[slot] = bottle;
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
            return true;
        }
        p.inventory.take(slot, 1);
        let mut updates = vec![p.inventory.set_slot_packet(slot)];
        let (changed, over) = p.inventory.add(bottle);
        updates.extend(changed.into_iter().map(|s| p.inventory.set_slot_packet(s)));
        let at = p.pos;
        for update in updates {
            self.send(id, update);
        }
        if over > 0 {
            self.scatter_item(at, bottle);
        }
        true
    }

    pub(super) fn use_bucket(&mut self, id: i32, hand: i32) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let slot = p.hand_slot(hand);
        let stack = p.inventory.slots[slot];
        let item_name = ItemInfo::get(stack.item).map_or("", |i| i.name);
        let carried = match item_name {
            "minecraft:bucket" => None,
            "minecraft:water_bucket" => Some(Fluid::Water),
            "minecraft:lava_bucket" => Some(Fluid::Lava),
            _ => return false,
        };
        let (eye, look, reach) = p.eye_ray();
        match carried {
            None => {
                let Some((pos, _)) = self.ray_cast(eye, look, reach, true) else {
                    return true;
                };
                let state = self.block(pos);
                let Some(fluid) = fluid_at(state).filter(|f| f.source) else {
                    return true;
                };
                let (taken, filled, sound) = match fluid.fluid {
                    Fluid::Water if can_waterlog(state) || prop(state, "waterlogged").is_some() => {
                        (
                            with_bool(state, "waterlogged", false),
                            "minecraft:water_bucket",
                            "fill",
                        )
                    }
                    Fluid::Water if Fluid::from_name(blocks::name(state)).is_some() => {
                        (block::AIR, "minecraft:water_bucket", "fill")
                    }
                    Fluid::Lava => (block::AIR, "minecraft:lava_bucket", "fill_lava"),
                    // Plants that are always underwater keep their water.
                    Fluid::Water => return true,
                };
                self.set_block_updating(pos, taken);
                self.play_sound(&format!("minecraft:item.bucket.{sound}"), pos, None);
                self.exchange_held(id, slot, filled);
            }
            Some(fluid) => {
                let Some((hit, face)) = self.ray_cast(eye, look, reach, false) else {
                    return true;
                };
                // In the Nether water boils away as it is poured.
                if fluid == Fluid::Water && self.dimension.ultrawarm() {
                    let pos = offset(hit, face);
                    self.play_sound("minecraft:block.fire.extinguish", pos, None);
                    self.exchange_held(id, slot, "minecraft:bucket");
                    return true;
                }
                let clicked = self.block(hit);
                let (pos, target) = if fluid == Fluid::Water && can_waterlog(clicked) {
                    (hit, with_bool(clicked, "waterlogged", true))
                } else {
                    let pos = if can_hold_fluid(clicked) {
                        hit
                    } else {
                        offset(hit, face)
                    };
                    let there = self.block(pos);
                    if !(can_hold_fluid(there) || fluid_at(there).is_some_and(|f| !f.source)) {
                        return true;
                    }
                    if !is_air(there) && fluid_at(there).is_none() {
                        self.pop_block(pos, there);
                    }
                    (pos, FluidState::source(fluid).block())
                };
                self.set_block_updating(pos, target);
                let sound = match fluid {
                    Fluid::Water => "empty",
                    Fluid::Lava => "empty_lava",
                };
                self.play_sound(&format!("minecraft:item.bucket.{sound}"), pos, None);
                self.exchange_held(id, slot, "minecraft:bucket");
            }
        }
        true
    }

    /// Ticks between flow steps: lava in the Nether runs three times as often.
    fn fluid_delay(&self, fluid: Fluid) -> u64 {
        match fluid {
            Fluid::Lava if self.dimension.ultrawarm() => 10,
            _ => fluid.delay(),
        }
    }

    /// How much weaker fluid gets per block: Nether lava runs as far as water.
    pub(super) fn drop_off(&self, fluid: Fluid) -> u8 {
        match fluid {
            Fluid::Lava if self.dimension.ultrawarm() => 1,
            _ => fluid.drop_off(),
        }
    }

    fn fluid_slope_distance(&self, fluid: Fluid) -> i32 {
        match fluid {
            Fluid::Lava if self.dimension.ultrawarm() => 4,
            _ => fluid.slope_distance(),
        }
    }

    /// Swaps one of the items in `slot` for one `result` (a bucket filled or emptied).
    /// Creative players keep what they had.
    pub(super) fn exchange_held(&mut self, id: i32, slot: usize, result: &str) {
        let Some(item) = ItemInfo::id_of(result) else {
            return;
        };
        let p = self.players.get_mut(&id).expect("player exists");
        if !p.survival() {
            return;
        }
        let mut updates = vec![slot];
        let mut overflow = None;
        if p.inventory.slots[slot].count <= 1 {
            p.inventory.slots[slot] = Stack::new(item, 1);
        } else {
            p.inventory.take(slot, 1);
            let (changed, left) = p.inventory.add(Stack::new(item, 1));
            updates.extend(changed);
            overflow = (left > 0).then(|| Stack::new(item, left));
        }
        let packets: Vec<Writer> = updates
            .into_iter()
            .map(|s| p.inventory.set_slot_packet(s))
            .collect();
        for packet in packets {
            self.send(id, packet);
        }
        if let Some(stack) = overflow {
            self.throw_from_player(id, stack);
        }
    }

    /// The first block along a ray, and the face the ray entered it through. With
    /// `sources`, fluid sources are hit and other fluid passed through; without, all
    /// fluid is passed through. Any other block that is not air stops the ray.
    fn ray_cast(
        &mut self,
        from: [f64; 3],
        dir: [f64; 3],
        reach: f64,
        sources: bool,
    ) -> Option<(BlockPos, Dir)> {
        let mut cell = from.map(|v| v.floor() as i32);
        let step = dir.map(|d| if d > 0.0 { 1 } else { -1 });
        let delta = dir.map(|d| {
            if d == 0.0 {
                f64::INFINITY
            } else {
                1.0 / d.abs()
            }
        });
        let mut next = [0, 1, 2].map(|i| {
            let edge = if dir[i] > 0.0 {
                f64::from(cell[i] + 1)
            } else {
                f64::from(cell[i])
            };
            if dir[i] == 0.0 {
                f64::INFINITY
            } else {
                (edge - from[i]).abs() * delta[i]
            }
        });
        let faces = [
            [Dir::West, Dir::East],
            [Dir::Down, Dir::Up],
            [Dir::North, Dir::South],
        ];
        let mut face = Dir::Up;
        let mut travelled = 0.0;
        while travelled <= reach {
            let pos = BlockPos::new(cell[0], cell[1], cell[2]);
            if (MIN_Y..=MAX_Y).contains(&pos.y) {
                let state = self.block(pos);
                let fluid = fluid_at(state);
                let pure_fluid = Fluid::from_name(blocks::name(state)).is_some();
                if sources && fluid.is_some_and(|f| f.source) {
                    return Some((pos, face));
                }
                if !is_air(state) && !pure_fluid {
                    return Some((pos, face));
                }
            }
            let axis = (0..3)
                .min_by(|&a, &b| next[a].total_cmp(&next[b]))
                .expect("three axes");
            travelled = next[axis];
            next[axis] += delta[axis];
            cell[axis] += step[axis];
            // The ray enters the new cell through the face pointing back at it.
            face = faces[axis][usize::from(step[axis] < 0)];
        }
        None
    }

    /// Fluid ticks in a chunk, in the vanilla `fluid_ticks` form.
    pub(super) fn fluid_ticks_in(&self, cx: i32, cz: i32) -> Vec<Compound> {
        self.fluid_ticks
            .due
            .iter()
            .flat_map(|(&due, ticks)| ticks.iter().map(move |t| (due, t)))
            .filter(|(_, (pos, _))| pos.chunk() == (cx, cz))
            .map(|(due, (pos, fluid))| {
                Compound::new()
                    .with("i", fluid.name())
                    .with("x", pos.x)
                    .with("y", pos.y)
                    .with("z", pos.z)
                    .with("t", due.saturating_sub(self.tick) as i32)
                    .with("p", 0)
            })
            .collect()
    }

    /// Restores fluid ticks saved with a chunk.
    pub(super) fn load_fluid_ticks(&mut self, ticks: &[Tag]) {
        for tick in ticks.iter().filter_map(Tag::as_compound) {
            let (Some(fluid), Some(x), Some(y), Some(z)) = (
                tick.str("i").and_then(Fluid::from_name),
                tick.int("x"),
                tick.int("y"),
                tick.int("z"),
            ) else {
                continue;
            };
            let pos = BlockPos::new(x as i32, y as i32, z as i32);
            let delay = tick.int("t").unwrap_or(0).max(0) as u64;
            self.schedule_fluid_in(pos, fluid, delay);
        }
    }

    /// Forgets fluid ticks of an unloaded chunk (they were saved with it).
    pub(super) fn drop_fluid_ticks(&mut self, cx: i32, cz: i32) {
        let ticks = &mut self.fluid_ticks;
        for list in ticks.due.values_mut() {
            list.retain(|(pos, fluid)| {
                let keep = pos.chunk() != (cx, cz);
                if !keep {
                    ticks.pending.remove(&(*pos, *fluid));
                }
                keep
            });
        }
        ticks.due.retain(|_, list| !list.is_empty());
    }
}
