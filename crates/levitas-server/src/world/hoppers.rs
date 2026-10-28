//! Hoppers, droppers and dispensers, following the Minecraft Wiki.
//!
//! A hopper moves one item every 8 ticks: first out into the container it points at,
//! then in from the container above or from items lying on top of it. Power locks it.
//! Droppers and dispensers fire one item from a random slot 4 ticks after they get
//! power: a dropper into the container in front or onto the ground, a dispenser using
//! the item where it can (arrows, buckets, bone meal, flint and steel, TNT) and dropping
//! it otherwise.

use levitas_registry::block_entity_type;

use super::containers::BlockEntity;
use super::*;
use crate::blocks::{self, offset, prop, with_bool, Dir};
use crate::recipes::fuel_ticks;

const HOPPER_COOLDOWN: u32 = 8;
const DISPENSE_DELAY: u64 = 4;
const LEVEL_EVENT_DISPENSE: i32 = 1000;
const LEVEL_EVENT_DISPENSE_FAIL: i32 = 1001;
const LEVEL_EVENT_SHOOT: i32 = 1002;
const LEVEL_EVENT_SMOKE: i32 = 2000;

impl World {
    /// The slots a container takes items into from the side `from` (pointing from the
    /// container to where the item comes from).
    fn insert_slots(entity: &BlockEntity, from: Dir, stack: &Stack) -> Vec<usize> {
        match entity {
            // Furnaces take what to cook from above and fuel from the sides.
            BlockEntity::Furnace(_) => match from {
                Dir::Up => vec![0],
                Dir::Down => Vec::new(),
                _ if fuel_ticks(stack.item) > 0 => vec![1],
                _ => Vec::new(),
            },
            BlockEntity::Chest(c) => (0..c.items.len()).collect(),
            BlockEntity::Campfire(_) => Vec::new(),
        }
    }

    /// The slots a container gives items out of towards `to`.
    fn extract_slots(entity: &BlockEntity, to: Dir) -> Vec<usize> {
        match entity {
            // A hopper under a furnace takes its output.
            BlockEntity::Furnace(_) => {
                if to == Dir::Down {
                    vec![2]
                } else {
                    vec![1]
                }
            }
            BlockEntity::Chest(c) => (0..c.items.len()).collect(),
            BlockEntity::Campfire(_) => Vec::new(),
        }
    }

    /// Puts as much of `stack` as fits into the container at `pos`, entering from the
    /// side `from`. Returns what is left.
    pub(super) fn insert_stack(&mut self, pos: BlockPos, from: Dir, mut stack: Stack) -> Stack {
        let Some(entity) = self.block_entities.get_mut(&pos) else {
            return stack;
        };
        let slots = Self::insert_slots(entity, from, &stack);
        let items = entity.stacks_mut();
        let mut changed = Vec::new();
        // Topping up stacks of the same kind first, then empty slots.
        for &i in &slots {
            let s = items[i];
            if !s.is_empty() && s.same_kind(&stack) && s.count < s.max_stack() {
                let moved = (s.max_stack() - s.count).min(stack.count);
                items[i] = s.with_count(s.count + moved);
                stack = stack.with_count(stack.count - moved);
                changed.push(i);
            }
            if stack.is_empty() {
                break;
            }
        }
        for &i in &slots {
            if stack.is_empty() {
                break;
            }
            if items[i].is_empty() {
                let moved = stack.count.min(stack.max_stack());
                items[i] = stack.with_count(moved);
                stack = stack.with_count(stack.count - moved);
                changed.push(i);
            }
        }
        if !changed.is_empty() {
            self.container_changed(pos, &changed);
        }
        stack
    }

    /// Takes one item out of a slot of the container at `pos`.
    fn take_one(&mut self, pos: BlockPos, slot: usize) -> Stack {
        let Some(entity) = self.block_entities.get_mut(&pos) else {
            return Stack::EMPTY;
        };
        let s = entity.stacks_mut()[slot];
        if s.is_empty() {
            return Stack::EMPTY;
        }
        entity.stacks_mut()[slot] = s.with_count(s.count - 1);
        self.container_changed(pos, &[slot]);
        s.with_count(1)
    }

    /// Saves, shows and tells comparators about changed container slots.
    fn container_changed(&mut self, pos: BlockPos, slots: &[usize]) {
        self.dirty.insert(pos.chunk());
        self.redstone_dirty.push(pos);
        self.notify_viewers(pos, slots, None);
    }

    /// Moves items one at a time from the container at `from` towards `dir` into the
    /// container there. Returns whether one moved.
    fn transfer(&mut self, from: BlockPos, dir: Dir, to: BlockPos) -> bool {
        let Some(source) = self.block_entities.get(&from) else {
            return false;
        };
        if !self.block_entities.contains_key(&to) {
            return false;
        }
        let slots = Self::extract_slots(source, dir);
        let stacks: Vec<Stack> = source.stacks().to_vec();
        for slot in slots {
            let s = stacks[slot];
            if s.is_empty() {
                continue;
            }
            let rest = self.insert_stack(to, dir.opposite(), s.with_count(1));
            if rest.is_empty() {
                self.take_one(from, slot);
                return true;
            }
        }
        false
    }

    pub(super) fn tick_hoppers(&mut self) {
        let hoppers: Vec<BlockPos> = self
            .block_entities
            .iter_mut()
            .filter_map(|(&pos, e)| match e {
                BlockEntity::Chest(c) if c.kind == block_entity_type::HOPPER => {
                    c.cooldown = c.cooldown.saturating_sub(1);
                    (c.cooldown == 0).then_some(pos)
                }
                _ => None,
            })
            .collect();
        for pos in hoppers {
            let Some(state) = self.loaded_block(pos) else {
                continue;
            };
            if prop(state, "enabled") == Some("false") {
                continue;
            }
            let facing = blocks::facing(state).unwrap_or(Dir::Down);
            let pushed = self.transfer(pos, facing, offset(pos, facing));
            let above = offset(pos, Dir::Up);
            let pulled = self.transfer(above, Dir::Down, pos) || self.hopper_pickup(pos);
            if pushed || pulled {
                if let Some(BlockEntity::Chest(c)) = self.block_entities.get_mut(&pos) {
                    c.cooldown = HOPPER_COOLDOWN;
                }
            }
        }
    }

    /// Power locks a hopper; it opens again when the power goes.
    pub(super) fn react_hopper(&mut self, pos: BlockPos, state: u16) {
        let enabled = self.power_at(pos) == 0;
        if (prop(state, "enabled") == Some("true")) != enabled {
            self.set_block(pos, with_bool(state, "enabled", enabled));
        }
    }

    /// A dropper or dispenser fires when power arrives (at it or at the block above).
    pub(super) fn react_dispenser(&mut self, pos: BlockPos, state: u16) {
        let powered = self.power_at(pos) > 0 || self.power_at(offset(pos, Dir::Up)) > 0;
        let triggered = prop(state, "triggered") == Some("true");
        if powered && !triggered {
            let on = with_bool(state, "triggered", true);
            self.set_block(pos, on);
            self.schedule(pos, on, DISPENSE_DELAY);
        } else if !powered && triggered {
            self.set_block(pos, with_bool(state, "triggered", false));
        }
    }

    /// Fires one item from a dropper or dispenser.
    pub(super) fn dispense(&mut self, pos: BlockPos, state: u16) {
        let Some(BlockEntity::Chest(c)) = self.block_entities.get(&pos) else {
            return;
        };
        let full: Vec<usize> = (0..c.items.len())
            .filter(|&i| !c.items[i].is_empty())
            .collect();
        let dropper = c.kind == block_entity_type::DROPPER;
        let facing = blocks::facing(state).unwrap_or(Dir::North);
        if full.is_empty() {
            self.level_event(LEVEL_EVENT_DISPENSE_FAIL, pos, 0);
            return;
        }
        let slot = full[self.rng.below(full.len() as u32) as usize];
        let stack = c.items[slot];
        let front = offset(pos, facing);

        if dropper {
            if self.block_entities.contains_key(&front) {
                let rest = self.insert_stack(front, facing.opposite(), stack.with_count(1));
                if rest.is_empty() {
                    self.take_one(pos, slot);
                    self.level_event(LEVEL_EVENT_DISPENSE, pos, 0);
                }
                return;
            }
            self.shoot_out(pos, facing, slot, stack);
            return;
        }

        let name = ItemInfo::get(stack.item).map_or("", |i| i.name);
        let front_state = self.loaded_block(front).unwrap_or(block::AIR);
        let replaceable =
            is_air(front_state) || block_has_tag(front_state, "minecraft:replaceable");
        let used: Option<Option<&str>> = match name {
            "minecraft:arrow" => {
                let from = [0, 1, 2].map(|i| {
                    f64::from([pos.x, pos.y, pos.z][i]) + 0.5 + 0.7 * f64::from(facing.step()[i])
                });
                let dir = facing.step().map(f64::from);
                self.shoot_arrow(
                    0,
                    false,
                    arrows::Shot {
                        from,
                        dir: [dir[0], dir[1] + 0.1, dir[2]],
                        speed: 1.1,
                        inaccuracy: 6.0,
                        critical: false,
                        base_damage: 2.0,
                        pickup: false,
                        flaming: false,
                    },
                );
                self.level_event(LEVEL_EVENT_SHOOT, pos, 0);
                Some(None)
            }
            "minecraft:water_bucket" | "minecraft:lava_bucket" if replaceable => {
                let fluid = if name == "minecraft:water_bucket" {
                    block::WATER
                } else {
                    block::LAVA
                };
                self.set_block_updating(front, fluid);
                Some(Some("minecraft:bucket"))
            }
            "minecraft:bucket" => {
                let filled = match blocks::name(front_state) {
                    "minecraft:water" if prop(front_state, "level") == Some("0") => {
                        Some("minecraft:water_bucket")
                    }
                    "minecraft:lava" if prop(front_state, "level") == Some("0") => {
                        Some("minecraft:lava_bucket")
                    }
                    _ => None,
                };
                match filled {
                    Some(filled) => {
                        self.set_block_updating(front, block::AIR);
                        Some(Some(filled))
                    }
                    None => None,
                }
            }
            "minecraft:bone_meal" => {
                if self.fertilize_from_dispenser(front, front_state) {
                    Some(None)
                } else {
                    self.level_event(LEVEL_EVENT_DISPENSE_FAIL, pos, 0);
                    return;
                }
            }
            "minecraft:tnt" if replaceable => {
                self.prime_tnt(front, tnt::FUSE);
                Some(None)
            }
            "minecraft:flint_and_steel" => {
                if blocks::name(front_state) == "minecraft:tnt" {
                    self.set_block_updating(front, block::AIR);
                    self.prime_tnt(front, tnt::FUSE);
                } else if let Some(fire) =
                    crate::blocks::fire::fire_for(front, &mut |q| self.block(q))
                {
                    self.light_fire(front, fire);
                } else {
                    self.level_event(LEVEL_EVENT_DISPENSE_FAIL, pos, 0);
                    return;
                }
                // The flint and steel wears instead of being used up.
                if let Some(BlockEntity::Chest(c)) = self.block_entities.get_mut(&pos) {
                    let s = &mut c.items[slot];
                    s.damage += 1;
                    if s.damage >= ItemInfo::get(s.item).map_or(0, |i| i.max_damage) {
                        *s = Stack::EMPTY;
                    }
                }
                self.container_changed(pos, &[slot]);
                self.level_event(LEVEL_EVENT_DISPENSE, pos, 0);
                return;
            }
            _ => None,
        };
        match used {
            Some(left) => {
                if let Some(BlockEntity::Chest(c)) = self.block_entities.get_mut(&pos) {
                    let s = c.items[slot];
                    c.items[slot] = match left.and_then(ItemInfo::id_of) {
                        // A bucket swaps for the bucket it becomes.
                        Some(other) if s.count == 1 => Stack::new(other, 1),
                        _ => s.with_count(s.count - 1),
                    };
                }
                self.container_changed(pos, &[slot]);
                self.level_event(LEVEL_EVENT_DISPENSE, pos, 0);
            }
            None => self.shoot_out(pos, facing, slot, stack),
        }
    }

    /// Drops one item out of the front of a dropper or dispenser.
    fn shoot_out(&mut self, pos: BlockPos, facing: Dir, slot: usize, stack: Stack) {
        self.take_one(pos, slot);
        let step = facing.step().map(f64::from);
        let at = [0, 1, 2].map(|i| f64::from([pos.x, pos.y, pos.z][i]) + 0.5 + 0.7 * step[i]);
        let speed = self.rng.f64() * 0.1 + 0.2;
        let mut vel = step.map(|v| v * speed);
        vel[1] += 0.2;
        for v in &mut vel {
            *v += (self.rng.f64() - 0.5) * 0.0172 * 6.0;
        }
        self.spawn_dropped(stack.with_count(1), at, vel);
        self.level_event(LEVEL_EVENT_DISPENSE, pos, 0);
        self.level_event(LEVEL_EVENT_SMOKE, pos, facing.face() as i32);
    }

    fn level_event(&mut self, event: i32, pos: BlockPos, data: i32) {
        self.broadcast(
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(event)
                .position(pos)
                .i32(data)
                .bool(false),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-hoppers-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut world, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        world.chunk(0, 0);
        world
    }

    fn place(w: &mut World, pos: BlockPos, state: u16) {
        w.set_block(pos, state);
        if let Some(e) = BlockEntity::for_block(state) {
            w.block_entities.insert(pos, e);
        }
    }

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    fn count(w: &mut World, pos: BlockPos) -> i32 {
        w.block_entities
            .get_mut(&pos)
            .unwrap()
            .stacks_mut()
            .iter()
            .map(|s| s.count.max(0))
            .sum()
    }

    fn settle(w: &mut World, ticks: u64) {
        for _ in 0..ticks {
            w.tick += 1;
            w.tick_scheduled();
            w.tick_hoppers();
            w.tick_redstone();
        }
    }

    #[test]
    fn hoppers_move_items_from_chest_to_chest_and_pick_up_drops() {
        let mut w = world();
        let y = -60;
        let top = BlockPos::new(1, y + 1, 1);
        let hopper = BlockPos::new(1, y, 1);
        let bottom = BlockPos::new(2, y, 1);
        place(&mut w, top, state("minecraft:chest"));
        place(
            &mut w,
            hopper,
            crate::blocks::with(state("minecraft:hopper"), "facing", "east"),
        );
        place(&mut w, bottom, state("minecraft:chest"));
        let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
        w.block_entities.get_mut(&top).unwrap().stacks_mut()[0] = Stack::new(dirt, 3);
        settle(&mut w, 40);
        assert_eq!(count(&mut w, top), 0);
        assert_eq!(count(&mut w, bottom), 3);

        // With nothing above but an item on top, the hopper sucks it in.
        w.set_block(top, block::AIR);
        w.block_entities.remove(&top);
        w.spawn_dropped(
            Stack::new(dirt, 2),
            [1.5, f64::from(y) + 1.0, 1.5],
            [0.0; 3],
        );
        settle(&mut w, 40);
        assert!(w.items.is_empty());
        assert_eq!(count(&mut w, bottom), 5);

        // Power locks it.
        w.set_block(BlockPos::new(1, y, 0), state("minecraft:redstone_block"));
        settle(&mut w, 2);
        assert_eq!(prop(w.block(hopper), "enabled"), Some("false"));
    }

    #[test]
    fn droppers_fill_chests_and_dispensers_pour_water() {
        let mut w = world();
        let y = -60;
        let dropper = BlockPos::new(1, y, 1);
        place(
            &mut w,
            dropper,
            crate::blocks::with(state("minecraft:dropper"), "facing", "east"),
        );
        let chest = BlockPos::new(2, y, 1);
        place(&mut w, chest, state("minecraft:chest"));
        let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
        w.block_entities.get_mut(&dropper).unwrap().stacks_mut()[4] = Stack::new(dirt, 2);
        let button = BlockPos::new(0, y, 1);
        let lever = crate::blocks::with(
            crate::blocks::with(state("minecraft:lever"), "face", "wall"),
            "facing",
            "west",
        );
        w.set_block(button, with_bool(lever, "powered", true));
        settle(&mut w, 6);
        assert_eq!(count(&mut w, chest), 1);
        assert_eq!(count(&mut w, dropper), 1);

        let dispenser = BlockPos::new(1, y, 5);
        place(
            &mut w,
            dispenser,
            crate::blocks::with(state("minecraft:dispenser"), "facing", "east"),
        );
        let bucket = ItemInfo::id_of("minecraft:water_bucket").unwrap();
        w.block_entities.get_mut(&dispenser).unwrap().stacks_mut()[0] = Stack::new(bucket, 1);
        w.set_block(BlockPos::new(0, y, 5), with_bool(lever, "powered", true));
        settle(&mut w, 6);
        assert_eq!(
            crate::blocks::name(w.block(BlockPos::new(2, y, 5))),
            "minecraft:water"
        );
        let left = w.block_entities.get_mut(&dispenser).unwrap().stacks_mut()[0];
        assert_eq!(ItemInfo::get(left.item).unwrap().name, "minecraft:bucket");
    }
}
