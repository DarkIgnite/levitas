//! Redstone in the world: power reaching blocks, dust carrying it, and the blocks that
//! answer it (lamps, doors, trapdoors, fence gates, TNT, torches and repeaters).
//!
//! Every block change marks its surroundings; at the end of the tick the marked dust
//! networks are worked out again and the marked components react. Dust takes the
//! strongest power fed into it and loses one per block; torches and repeaters switch a
//! few ticks later, so clocks run instead of settling in one tick.

use std::collections::{HashSet, VecDeque};

use super::*;
use crate::blocks::redstone::{
    attached_to, diode_output, is_comparator, is_conductor, is_diode, is_observer, is_repeater,
    is_wire, reacts, repeater_delay, strong, weak, wire_links, wire_power, wire_shape, MAX_POWER,
};
use crate::blocks::{self, offset, prop, with, with_bool, Dir};

/// The most dust one network update looks at, and the most redstone work in one tick,
/// as guards against runaway contraptions.
const MAX_NETWORK: usize = 4096;
const MAX_WORK: usize = 20_000;
/// A lamp stays lit this long after its power goes.
const LAMP_OFF_DELAY: u64 = 4;
const TORCH_DELAY: u64 = 2;
/// How long pressure plates stay down after something leaves them.
const PLATE_TICKS: u64 = 20;
/// Observers pulse for two ticks, two ticks after what they watch changes.
const OBSERVER_TICKS: u64 = 2;
const COMPARATOR_DELAY: u64 = 2;

impl World {
    fn state_at(&self, pos: BlockPos) -> u16 {
        self.loaded_block(pos).unwrap_or(block::AIR)
    }

    /// The power the block `state` at `pos` gives towards `to`, strong or weak. Only
    /// comparators need the world: their output level is kept apart from their state.
    fn emitted(&self, pos: BlockPos, state: u16, to: Dir, strong_only: bool) -> u8 {
        if is_comparator(state) {
            return if diode_output(state) == Some(to) {
                self.comparator_output(pos, state)
            } else {
                0
            };
        }
        if strong_only {
            strong(state, to)
        } else {
            weak(state, to)
        }
    }

    fn comparator_output(&self, pos: BlockPos, state: u16) -> u8 {
        self.comparators.get(&pos).copied().unwrap_or_else(|| {
            if prop(state, "powered") == Some("true") {
                MAX_POWER
            } else {
                0
            }
        })
    }

    /// Strong power going into the block at `pos` from its neighbours. Dust counts only
    /// when `with_dust`.
    fn strong_into(&self, pos: BlockPos, with_dust: bool) -> u8 {
        Dir::ALL
            .iter()
            .map(|&d| {
                let at = offset(pos, d);
                let from = self.state_at(at);
                if !with_dust && is_wire(from) {
                    0
                } else {
                    self.emitted(at, from, d.opposite(), true)
                }
            })
            .max()
            .unwrap_or(0)
    }

    /// The power the block at `from` gives its neighbour in direction `to`: a solid
    /// block passes on the strong power it gets, anything else gives its own.
    pub(super) fn power_from(&self, from: BlockPos, to: Dir, with_dust: bool) -> u8 {
        let state = self.state_at(from);
        if is_conductor(state) {
            self.strong_into(from, with_dust)
        } else if !with_dust && is_wire(state) {
            0
        } else {
            self.emitted(from, state, to, false)
        }
    }

    /// The strongest power reaching the block at `pos` from any side.
    pub(super) fn power_at(&self, pos: BlockPos) -> u8 {
        self.power_at_excluding(pos, true)
    }

    fn power_at_excluding(&self, pos: BlockPos, with_dust: bool) -> u8 {
        Dir::ALL
            .iter()
            .map(|&d| self.power_from(offset(pos, d), d.opposite(), with_dust))
            .max()
            .unwrap_or(0)
    }

    /// Notes a changed block so the redstone around it is looked at again.
    pub(super) fn redstone_changed(&mut self, pos: BlockPos) {
        self.redstone_dirty.push(pos);
    }

    /// Works out every marked redstone change. Runs at the end of each tick.
    pub(super) fn tick_redstone(&mut self) {
        let mut work = 0;
        while !self.redstone_dirty.is_empty() && work < MAX_WORK {
            let changed: HashSet<BlockPos> = std::mem::take(&mut self.redstone_dirty)
                .into_iter()
                .collect();
            // Power passes through a block, so look two steps out.
            let mut near = HashSet::new();
            for &pos in &changed {
                near.insert(pos);
                for d in Dir::ALL {
                    let one = offset(pos, d);
                    near.insert(one);
                    for e in Dir::ALL {
                        near.insert(offset(one, e));
                    }
                }
            }
            // Observers watching a changed block start their pulse.
            for &pos in &changed {
                for d in Dir::ALL {
                    let watcher = offset(pos, d);
                    let Some(state) = self.loaded_block(watcher) else {
                        continue;
                    };
                    if is_observer(state)
                        && blocks::facing(state) == Some(d.opposite())
                        && prop(state, "powered") != Some("true")
                    {
                        self.schedule_once(watcher, state, OBSERVER_TICKS);
                    }
                }
            }
            let mut wires_done = HashSet::new();
            for pos in near {
                work += 1;
                let Some(state) = self.loaded_block(pos) else {
                    continue;
                };
                if is_wire(state) {
                    if !wires_done.contains(&pos) {
                        work += self.update_dust(pos, &mut wires_done);
                    }
                } else if reacts(state) {
                    self.react(pos, state);
                }
            }
        }
        if work >= MAX_WORK {
            self.redstone_dirty.clear();
        }
    }

    /// Works out the power and shape of the dust network containing `start`. Returns
    /// how much dust it looked at.
    fn update_dust(&mut self, start: BlockPos, done: &mut HashSet<BlockPos>) -> usize {
        // The whole connected network.
        let mut network = vec![start];
        let mut seen: HashSet<BlockPos> = HashSet::from([start]);
        let mut queue = VecDeque::from([start]);
        while let Some(pos) = queue.pop_front() {
            if network.len() >= MAX_NETWORK {
                break;
            }
            for next in wire_links(pos, &mut |q| self.state_at(q)) {
                if seen.insert(next) {
                    network.push(next);
                    queue.push_back(next);
                }
            }
        }
        done.extend(network.iter().copied());

        // Power fed in from outside the dust, then spread along it, one less per block.
        let mut power: HashMap<BlockPos, u8> = network
            .iter()
            .map(|&pos| (pos, self.power_at_excluding(pos, false)))
            .collect();
        let mut by_level: Vec<Vec<BlockPos>> = vec![Vec::new(); usize::from(MAX_POWER) + 1];
        for (&pos, &p) in &power {
            by_level[usize::from(p)].push(pos);
        }
        for level in (1..=usize::from(MAX_POWER)).rev() {
            let current = std::mem::take(&mut by_level[level]);
            for pos in current {
                if usize::from(power[&pos]) != level {
                    continue;
                }
                for next in wire_links(pos, &mut |q| self.state_at(q)) {
                    let Some(p) = power.get_mut(&next) else {
                        continue;
                    };
                    let carried = level as u8 - 1;
                    if *p < carried {
                        *p = carried;
                        by_level[usize::from(carried)].push(next);
                    }
                }
            }
        }

        for &pos in &network {
            let state = self.state_at(pos);
            let shaped = wire_shape(state, pos, &mut |q| self.state_at(q));
            let new = with(shaped, "power", &power[&pos].to_string());
            if new != state {
                self.set_block(pos, new);
            }
        }
        network.len()
    }

    /// A component next to a change answers the power it now gets.
    fn react(&mut self, pos: BlockPos, state: u16) {
        let n = blocks::name(state);
        if crate::blocks::piston::is_piston(state) {
            self.react_piston(pos, state);
        } else if n == "minecraft:hopper" {
            self.react_hopper(pos, state);
        } else if n == "minecraft:dropper" || n == "minecraft:dispenser" {
            self.react_dispenser(pos, state);
        } else if blocks::is_door(state) {
            if prop(state, "half") == Some("upper") {
                return; // the lower half decides for both
            }
            let upper = offset(pos, Dir::Up);
            let powered = self.power_at(pos) > 0 || self.power_at(upper) > 0;
            if (prop(state, "powered") == Some("true")) != powered {
                let new = with_bool(with_bool(state, "powered", powered), "open", powered);
                self.set_block(pos, new);
                let top = self.state_at(upper);
                if blocks::is_door(top) {
                    let top = with_bool(with_bool(top, "powered", powered), "open", powered);
                    self.set_block(upper, top);
                }
                self.play_open_sound(pos, state, powered);
            }
        } else if block_has_tag(state, "minecraft:trapdoors")
            || block_has_tag(state, "minecraft:fence_gates")
        {
            let powered = self.power_at(pos) > 0;
            if (prop(state, "powered") == Some("true")) != powered {
                let new = with_bool(with_bool(state, "powered", powered), "open", powered);
                self.set_block(pos, new);
                self.play_open_sound(pos, state, powered);
            }
        } else if n == "minecraft:redstone_lamp" {
            let powered = self.power_at(pos) > 0;
            let lit = prop(state, "lit") == Some("true");
            if powered && !lit {
                self.set_block(pos, with_bool(state, "lit", true));
            } else if !powered && lit {
                self.schedule_once(pos, state, LAMP_OFF_DELAY);
            }
        } else if n == "minecraft:tnt" {
            if self.power_at(pos) > 0 {
                self.set_block_updating(pos, block::AIR);
                self.prime_tnt(pos, super::tnt::FUSE);
            }
        } else if is_repeater(state) {
            // A powered repeater or comparator pointing into its side locks it.
            let locked = self.side_input(pos, state, true) > 0;
            if locked != (prop(state, "locked") == Some("true")) {
                self.set_block(pos, with_bool(state, "locked", locked));
            }
            let input = self.repeater_input(pos, state) > 0;
            if !locked && input != (prop(state, "powered") == Some("true")) {
                self.schedule_once(pos, state, repeater_delay(state));
            }
        } else if is_comparator(state) {
            if self.comparator_target(pos, state) != self.comparator_output(pos, state) {
                self.schedule_once(pos, state, COMPARATOR_DELAY);
            }
        } else if attached_to(state).is_some() {
            // A redstone torch.
            if self.torch_should_light(pos, state) != (prop(state, "lit") == Some("true")) {
                self.schedule_once(pos, state, TORCH_DELAY);
            }
        }
    }

    fn play_open_sound(&mut self, pos: BlockPos, state: u16, open: bool) {
        let n = blocks::name(state);
        let kind = if n.contains("iron") {
            "iron"
        } else if n.contains("copper") {
            "copper"
        } else {
            "wooden"
        };
        let what = if blocks::is_door(state) {
            "door"
        } else if n.ends_with("trapdoor") {
            "trapdoor"
        } else {
            "fence_gate"
        };
        let action = if open { "open" } else { "close" };
        let sound = if what == "fence_gate" {
            format!("minecraft:block.fence_gate.{action}")
        } else {
            format!("minecraft:block.{kind}_{what}.{action}")
        };
        self.play_sound(&sound, pos, None);
    }

    /// Schedules a block tick unless one is already waiting there.
    fn schedule_once(&mut self, pos: BlockPos, state: u16, delay: u64) {
        if !self.is_scheduled(pos, state) {
            self.schedule(pos, state, delay);
        }
    }

    /// The power arriving at a repeater's back.
    fn repeater_input(&self, pos: BlockPos, state: u16) -> u8 {
        let Some(back) = blocks::facing(state) else {
            return 0;
        };
        self.power_from(offset(pos, back), back.opposite(), true)
    }

    /// Power coming into a repeater's or comparator's sides. Only dust, blocks of
    /// redstone and other diodes count, and only diodes when `diodes_only`.
    fn side_input(&self, pos: BlockPos, state: u16, diodes_only: bool) -> u8 {
        let Some(back) = blocks::facing(state) else {
            return 0;
        };
        [back.clockwise(), back.counter_clockwise()]
            .into_iter()
            .map(|side| {
                let at = offset(pos, side);
                let s = self.state_at(at);
                if is_diode(s) {
                    self.emitted(at, s, side.opposite(), false)
                } else if diodes_only {
                    0
                } else if is_wire(s) {
                    wire_power(s)
                } else if blocks::name(s) == "minecraft:redstone_block" {
                    MAX_POWER
                } else {
                    0
                }
            })
            .max()
            .unwrap_or(0)
    }

    /// How full a container is, as a comparator reads it: 0 when empty, else 1 to 15.
    fn container_signal(&self, pos: BlockPos) -> Option<u8> {
        let entity = self.block_entities.get(&pos)?;
        let items = entity.stacks();
        if items.is_empty() {
            return Some(0);
        }
        let fullness: f64 = items
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| f64::from(s.count) / f64::from(s.max_stack()))
            .sum::<f64>()
            / items.len() as f64;
        Some(if fullness <= 0.0 {
            0
        } else {
            1 + (fullness * 14.0).floor() as u8
        })
    }

    /// What a comparator should give out: its back input, compared with or less its
    /// side input.
    fn comparator_target(&self, pos: BlockPos, state: u16) -> u8 {
        let Some(back) = blocks::facing(state) else {
            return 0;
        };
        let behind = offset(pos, back);
        let behind_state = self.state_at(behind);
        // A container behind it, or behind the solid block behind it, is read for how
        // full it is.
        let read = self.container_signal(behind).or_else(|| {
            is_conductor(behind_state)
                .then(|| self.container_signal(offset(behind, back)))
                .flatten()
        });
        let input = read.unwrap_or_else(|| self.power_from(behind, back.opposite(), true));
        let side = self.side_input(pos, state, false);
        if prop(state, "mode") == Some("subtract") {
            input.saturating_sub(side)
        } else if input >= side {
            input
        } else {
            0
        }
    }

    /// A torch is lit unless the block it stands on or hangs from is powered.
    fn torch_should_light(&self, pos: BlockPos, state: u16) -> bool {
        let Some(support) = attached_to(state) else {
            return true;
        };
        self.power_from(offset(pos, support), support.opposite(), true) == 0
    }

    /// A scheduled tick of a redstone component: torches and repeaters switch, lamps go
    /// out, pressure plates come back up.
    pub(super) fn redstone_tick(&mut self, pos: BlockPos, state: u16) {
        let n = blocks::name(state);
        if n == "minecraft:redstone_lamp" {
            if self.power_at(pos) == 0 {
                self.set_block(pos, with_bool(state, "lit", false));
            }
        } else if is_repeater(state) {
            if prop(state, "locked") == Some("true") {
                return;
            }
            let input = self.repeater_input(pos, state) > 0;
            self.set_block(pos, with_bool(state, "powered", input));
        } else if is_comparator(state) {
            let output = self.comparator_target(pos, state);
            let before = self.comparator_output(pos, state);
            self.comparators.insert(pos, output);
            let new = with_bool(state, "powered", output > 0);
            if new != state {
                self.set_block(pos, new);
            } else if output != before {
                self.redstone_changed(pos);
            }
        } else if n == "minecraft:dropper" || n == "minecraft:dispenser" {
            self.dispense(pos, state);
        } else if is_observer(state) {
            if prop(state, "powered") == Some("true") {
                self.set_block(pos, with_bool(state, "powered", false));
            } else {
                let on = with_bool(state, "powered", true);
                self.set_block(pos, on);
                self.schedule(pos, on, OBSERVER_TICKS);
            }
        } else if n == "minecraft:redstone_torch" || n == "minecraft:redstone_wall_torch" {
            let lit = self.torch_should_light(pos, state);
            self.set_block(pos, with_bool(state, "lit", lit));
        } else if n.ends_with("_pressure_plate") {
            if !self.someone_on(pos) {
                self.set_plate(pos, state, false);
            } else {
                self.schedule(pos, state, PLATE_TICKS);
            }
        }
    }

    /// Whether a player stands on the block at `pos`.
    fn someone_on(&self, pos: BlockPos) -> bool {
        self.players.values().any(|p| {
            p.alive
                && p.pos[0].floor() as i32 == pos.x
                && p.pos[2].floor() as i32 == pos.z
                && (p.pos[1] - f64::from(pos.y)).abs() < 0.5
        })
    }

    fn set_plate(&mut self, pos: BlockPos, state: u16, down: bool) {
        let new = if prop(state, "power").is_some() {
            with(state, "power", if down { "15" } else { "0" })
        } else {
            with_bool(state, "powered", down)
        };
        if new == state {
            return;
        }
        self.set_block(pos, new);
        let wooden =
            !blocks::name(state).contains("stone") && !blocks::name(state).contains("weighted");
        let sound = match (wooden, down) {
            (true, true) => "minecraft:block.wooden_pressure_plate.click_on",
            (true, false) => "minecraft:block.wooden_pressure_plate.click_off",
            (false, true) => "minecraft:block.stone_pressure_plate.click_on",
            (false, false) => "minecraft:block.stone_pressure_plate.click_off",
        };
        self.play_sound(sound, pos, None);
    }

    /// Players press down the pressure plates they stand on.
    pub(super) fn tick_pressure_plates(&mut self) {
        let feet: Vec<BlockPos> = self
            .players
            .values()
            .filter(|p| p.alive)
            .map(|p| {
                BlockPos::new(
                    p.pos[0].floor() as i32,
                    p.pos[1].floor() as i32,
                    p.pos[2].floor() as i32,
                )
            })
            .collect();
        for pos in feet {
            let Some(state) = self.loaded_block(pos) else {
                continue;
            };
            if !blocks::name(state).ends_with("_pressure_plate") {
                continue;
            }
            let down = prop(state, "powered") == Some("true")
                || prop(state, "power").is_some_and(|p| p != "0");
            if !down {
                self.set_plate(pos, state, true);
                let state = self.state_at(pos);
                self.schedule_once(pos, state, PLATE_TICKS);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::redstone::wire_power;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-redstone-{}-{}",
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
        for cx in -1..=1 {
            for cz in -1..=1 {
                world.chunk(cx, cz);
            }
        }
        world
    }

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    /// Runs ticks of redstone and block ticks.
    fn settle(w: &mut World, ticks: u64) {
        for _ in 0..ticks {
            w.tick += 1;
            w.tick_scheduled();
            w.tick_redstone();
        }
    }

    #[test]
    fn dust_carries_power_fifteen_blocks_to_a_lamp() {
        let mut w = world();
        let y = -60;
        w.set_block(BlockPos::new(0, y, 0), state("minecraft:redstone_block"));
        for x in 1..=15 {
            w.set_block(BlockPos::new(x, y, 0), state("minecraft:redstone_wire"));
        }
        let lamp = BlockPos::new(16, y, 0);
        w.set_block(lamp, state("minecraft:redstone_lamp"));
        settle(&mut w, 2);
        assert_eq!(wire_power(w.block(BlockPos::new(1, y, 0))), 15);
        assert_eq!(wire_power(w.block(BlockPos::new(15, y, 0))), 1);
        assert_eq!(prop(w.block(lamp), "lit"), Some("true"));

        // Taking the power away turns the dust off and, a little later, the lamp.
        w.set_block(BlockPos::new(0, y, 0), block::AIR);
        settle(&mut w, 1);
        assert_eq!(wire_power(w.block(BlockPos::new(15, y, 0))), 0);
        settle(&mut w, 6);
        assert_eq!(prop(w.block(lamp), "lit"), Some("false"));
    }

    #[test]
    fn levers_open_iron_doors_and_light_tnt_through_blocks() {
        let mut w = world();
        let y = -60;
        let lever_pos = BlockPos::new(0, y, 0);
        let lever = with(state("minecraft:lever"), "face", "floor");
        w.set_block(lever_pos, lever);
        let door = BlockPos::new(1, y, 0);
        let lower = with(state("minecraft:iron_door"), "half", "lower");
        w.set_block(door, lower);
        w.set_block(offset(door, Dir::Up), with(lower, "half", "upper"));
        settle(&mut w, 2);
        assert_eq!(prop(w.block(door), "open"), Some("false"));

        w.set_block(lever_pos, with_bool(lever, "powered", true));
        settle(&mut w, 2);
        assert_eq!(prop(w.block(door), "open"), Some("true"));
        assert_eq!(prop(w.block(offset(door, Dir::Up)), "open"), Some("true"));

        // A lever on a block powers the block strongly, and TNT beside it goes off.
        let wall = BlockPos::new(5, y, 0);
        w.set_block(wall, block::STONE);
        let on_wall = with_bool(
            with(
                with(state("minecraft:lever"), "face", "wall"),
                "facing",
                "east",
            ),
            "powered",
            true,
        );
        w.set_block(BlockPos::new(6, y, 0), on_wall);
        w.set_block(BlockPos::new(4, y, 0), state("minecraft:tnt"));
        settle(&mut w, 2);
        assert!(is_air(w.block(BlockPos::new(4, y, 0))));
        assert_eq!(w.tnt.len(), 1);
    }

    #[test]
    fn a_torch_inverts_and_a_repeater_delays() {
        let mut w = world();
        let y = -60;
        // Lever on a block with a torch on its other side: the torch goes out.
        let block_pos = BlockPos::new(0, y, 0);
        w.set_block(block_pos, block::STONE);
        let torch_pos = BlockPos::new(1, y, 0);
        let torch = with(state("minecraft:redstone_wall_torch"), "facing", "east");
        w.set_block(torch_pos, torch);
        let lever_pos = BlockPos::new(-1, y, 0);
        let lever = with(
            with(state("minecraft:lever"), "face", "wall"),
            "facing",
            "west",
        );
        w.set_block(lever_pos, lever);
        settle(&mut w, 3);
        assert_eq!(prop(w.block(torch_pos), "lit"), Some("true"));
        w.set_block(lever_pos, with_bool(lever, "powered", true));
        settle(&mut w, 3);
        assert_eq!(prop(w.block(torch_pos), "lit"), Some("false"));

        // A repeater facing its input passes power on after its delay.
        let input = BlockPos::new(0, y, 5);
        w.set_block(input, state("minecraft:redstone_block"));
        let rep_pos = BlockPos::new(1, y, 5);
        let repeater = with(state("minecraft:repeater"), "facing", "west");
        w.set_block(rep_pos, repeater);
        let lamp = BlockPos::new(2, y, 5);
        w.set_block(lamp, state("minecraft:redstone_lamp"));
        settle(&mut w, 1);
        assert_eq!(prop(w.block(rep_pos), "powered"), Some("false"));
        settle(&mut w, 3);
        assert_eq!(prop(w.block(rep_pos), "powered"), Some("true"));
        assert_eq!(prop(w.block(lamp), "lit"), Some("true"));
    }

    #[test]
    fn observers_pulse_when_what_they_watch_changes() {
        let mut w = world();
        let y = -60;
        // Watching east (the block at x = 1), giving power west into a lamp.
        let observer = BlockPos::new(0, y, 0);
        w.set_block(
            observer,
            with(state("minecraft:observer"), "facing", "east"),
        );
        let lamp = BlockPos::new(-1, y, 0);
        w.set_block(lamp, state("minecraft:redstone_lamp"));
        settle(&mut w, 10);
        assert_eq!(prop(w.block(observer), "powered"), Some("false"));

        // Noticed in the next tick, the pulse comes two ticks later.
        w.set_block(BlockPos::new(1, y, 0), block::STONE);
        settle(&mut w, 3);
        assert_eq!(prop(w.block(observer), "powered"), Some("true"));
        assert_eq!(prop(w.block(lamp), "lit"), Some("true"));
        settle(&mut w, 2);
        assert_eq!(prop(w.block(observer), "powered"), Some("false"));
    }

    #[test]
    fn comparators_read_chests_and_subtract() {
        let mut w = world();
        let y = -60;
        let chest = BlockPos::new(0, y, 0);
        let chest_state = state("minecraft:chest");
        w.set_block(chest, chest_state);
        w.block_entities
            .insert(chest, BlockEntity::for_block(chest_state).unwrap());
        // Half a stack in a 27-slot chest is just over empty: signal 1.
        let comparator = BlockPos::new(1, y, 0);
        let cmp = with(state("minecraft:comparator"), "facing", "west");
        w.set_block(comparator, cmp);
        let dust = BlockPos::new(2, y, 0);
        w.set_block(dust, state("minecraft:redstone_wire"));
        settle(&mut w, 4);
        assert_eq!(wire_power(w.block(dust)), 0);

        let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
        let slots = w.block_entities.get_mut(&chest).unwrap().stacks_mut();
        for slot in &mut slots[..14] {
            *slot = Stack::new(dirt, 64);
        }
        w.redstone_changed(chest);
        settle(&mut w, 4);
        // 14 full slots of 27: 1 + floor(14 / 27 * 14) = 8.
        assert_eq!(wire_power(w.block(dust)), 8);

        // Subtract mode takes a side input away.
        let side = BlockPos::new(1, y, 1);
        w.set_block(side, state("minecraft:redstone_wire"));
        w.set_block(BlockPos::new(1, y, 2), state("minecraft:redstone_block"));
        let current = w.block(comparator);
        w.set_block(comparator, with(current, "mode", "subtract"));
        settle(&mut w, 4);
        assert_eq!(wire_power(w.block(dust)), 0, "8 - 15 is nothing");
    }

    #[test]
    fn a_repeater_pointing_into_another_locks_it() {
        let mut w = world();
        let y = -60;
        let input = BlockPos::new(0, y, 0);
        let locked = BlockPos::new(1, y, 0);
        let repeater = with(state("minecraft:repeater"), "facing", "west");
        w.set_block(locked, repeater);
        // A powered repeater at its side, pointing north into it.
        w.set_block(BlockPos::new(1, y, 2), state("minecraft:redstone_block"));
        w.set_block(
            BlockPos::new(1, y, 1),
            with(state("minecraft:repeater"), "facing", "south"),
        );
        settle(&mut w, 6);
        assert_eq!(prop(w.block(locked), "locked"), Some("true"));
        w.set_block(input, state("minecraft:redstone_block"));
        settle(&mut w, 6);
        assert_eq!(prop(w.block(locked), "powered"), Some("false"), "locked");
    }
}
