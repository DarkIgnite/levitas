//! Random ticks: crops ripen, saplings grow into trees, grass spreads, leaves rot,
//! farmland dries out, sugar cane and cacti grow. Also bone meal and tilling with hoes.
//!
//! Behaviour follows the Minecraft Wiki: every game tick three random blocks of every
//! 16x16x16 section of the loaded world get a random tick. Crops need light level 9 and
//! grow faster on moist farmland among more farmland; saplings need light 9 above them;
//! grass spreads to dirt nearby with light 4 above it and dies under opaque blocks.

use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::blocks::plants::{
    self, age, crop_max_age, grow, leaves_decay, tree_can_replace, tree_of, with_age, Tree,
};
use crate::blocks::{self, offset, prop, with, Dir};

/// How far water keeps farmland moist.
const WATER_REACH: i32 = 4;
/// Level event for bone meal's green sparkles.
const LEVEL_EVENT_BONE_MEAL: i32 = 1505;

impl World {
    pub(super) fn tick_random(&mut self) {
        let mut picks = Vec::new();
        let speed = crate::rules::int("random_tick_speed").clamp(0, 4096);
        for (&(cx, cz), chunk) in &self.chunks {
            for (s, section) in chunk.sections().iter().enumerate() {
                if section.is_empty() {
                    continue;
                }
                for _ in 0..speed {
                    let r = self.rng.below(4096) as usize;
                    let (x, y, z) = (r & 15, r >> 8, (r >> 4) & 15);
                    let state = section.get(x, y, z);
                    if ticks_randomly(state) {
                        let pos = BlockPos::new(
                            cx * 16 + x as i32,
                            MIN_Y + (s * 16 + y) as i32,
                            cz * 16 + z as i32,
                        );
                        picks.push((pos, state));
                    }
                }
            }
        }
        for (pos, state) in picks {
            self.random_tick(pos, state);
        }
    }

    fn random_tick(&mut self, pos: BlockPos, state: u16) {
        let above = offset(pos, Dir::Up);
        let block = blocks::name(state);
        if let Some(max) = crop_max_age(state) {
            let ripe = age(state) >= max;
            let beetroot_pause = block == "minecraft:beetroots" && self.rng.below(3) == 0;
            if ripe || beetroot_pause || self.raw_light(pos) < 9 {
                return;
            }
            let speed = self.growth_speed(pos, state);
            if self.rng.below((25.0 / speed) as u32 + 1) == 0 {
                self.set_block_updating(pos, with_age(state, age(state) + 1));
            }
        } else if block == "minecraft:farmland" {
            let moisture: u8 = prop(state, "moisture")
                .and_then(|m| m.parse().ok())
                .unwrap_or(0);
            if self.water_near(pos) {
                if moisture < 7 {
                    self.set_block(pos, with(state, "moisture", "7"));
                }
            } else if moisture > 0 {
                self.set_block(pos, with(state, "moisture", &(moisture - 1).to_string()));
            } else if !block_has_tag(self.block(above), "minecraft:maintains_farmland") {
                self.set_block_updating(pos, block::DIRT);
            }
        } else if block == "minecraft:grass_block" || block == "minecraft:mycelium" {
            if !self.can_be_grass(pos) {
                self.set_block_updating(pos, block::DIRT);
                return;
            }
            if self.raw_light(above) < 9 {
                return;
            }
            let spread = blocks::with_bool(BlockInfo::of(state).default_state, "snowy", false);
            for _ in 0..4 {
                let target = BlockPos::new(
                    pos.x + self.rng.below(3) as i32 - 1,
                    pos.y + self.rng.below(5) as i32 - 3,
                    pos.z + self.rng.below(3) as i32 - 1,
                );
                if self.loaded_block(target) == Some(block::DIRT)
                    && self.can_be_grass(target)
                    && self.raw_light(offset(target, Dir::Up)) >= 4
                {
                    self.set_block(target, spread);
                }
            }
        } else if block == "minecraft:sugar_cane" || block == "minecraft:cactus" {
            if !is_air(self.block(above)) {
                return;
            }
            let mut height = 1;
            while height < 3
                && blocks::same_block(
                    self.block(BlockPos::new(pos.x, pos.y - height, pos.z)),
                    state,
                )
            {
                height += 1;
            }
            if height >= 3 {
                return;
            }
            let grown = age(state);
            if grown >= 15 {
                self.set_block(pos, with_age(state, 0));
                self.set_block_updating(above, BlockInfo::of(state).default_state);
            } else {
                self.set_block(pos, with_age(state, grown + 1));
            }
        } else if block_has_tag(state, "minecraft:saplings") {
            if self.raw_light(above) >= 9 && self.rng.below(7) == 0 {
                self.advance_sapling(pos, state);
            }
        } else if leaves_decay(state) {
            self.pop_block(pos, state);
        }
    }

    /// Light at a block ignoring the time of day: the brighter of sky and block light.
    fn raw_light(&self, pos: BlockPos) -> u8 {
        self.light_at(pos).map_or(0, |(sky, block)| sky.max(block))
    }

    /// How fast a crop grows (the wiki's "growth points"): more for farmland around it,
    /// more still when moist, halved when crops of the same kind crowd it in rows.
    fn growth_speed(&self, pos: BlockPos, crop: u16) -> f64 {
        let mut speed = 1.0;
        for dx in -1..=1 {
            for dz in -1..=1 {
                let soil = self
                    .loaded_block(BlockPos::new(pos.x + dx, pos.y - 1, pos.z + dz))
                    .unwrap_or(block::AIR);
                if blocks::name(soil) != "minecraft:farmland" {
                    continue;
                }
                let moist = prop(soil, "moisture").is_some_and(|m| m != "0");
                let mut points = if moist { 3.0 } else { 1.0 };
                if dx != 0 || dz != 0 {
                    points /= 4.0;
                }
                speed += points;
            }
        }
        let same = |dx: i32, dz: i32| {
            self.loaded_block(BlockPos::new(pos.x + dx, pos.y, pos.z + dz))
                .is_some_and(|s| blocks::same_block(s, crop))
        };
        let row_x = same(-1, 0) || same(1, 0);
        let row_z = same(0, -1) || same(0, 1);
        let diagonal = same(-1, -1) || same(1, -1) || same(1, 1) || same(-1, 1);
        if diagonal || (row_x && row_z) {
            speed /= 2.0;
        }
        speed
    }

    /// Whether water is within four blocks of farmland, level with it or one block up.
    fn water_near(&self, pos: BlockPos) -> bool {
        (-WATER_REACH..=WATER_REACH).any(|dx| {
            (-WATER_REACH..=WATER_REACH).any(|dz| {
                (0..=1).any(|dy| {
                    self.loaded_block(BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz))
                        .and_then(fluid_at)
                        .is_some_and(|f| f.fluid == Fluid::Water)
                })
            })
        })
    }

    /// Whether grass can live at `pos`: light gets through what is above it.
    fn can_be_grass(&self, pos: BlockPos) -> bool {
        let above = self
            .loaded_block(offset(pos, Dir::Up))
            .unwrap_or(block::AIR);
        if blocks::name(above) == "minecraft:snow" {
            return true;
        }
        fluid_at(above).is_none() && BlockInfo::of(above).light_filter < 15
    }

    /// A sapling's next step: its first random tick readies it, its second grows a tree.
    fn advance_sapling(&mut self, pos: BlockPos, state: u16) {
        if prop(state, "stage") == Some("0") {
            self.set_block(pos, with(state, "stage", "1"));
        } else {
            self.grow_tree(pos, state);
        }
    }

    /// Grows the tree of the sapling at `pos`, if there is room for it.
    pub(super) fn grow_tree(&mut self, pos: BlockPos, sapling: u16) -> bool {
        let Some(tree) = tree_of(sapling) else {
            return false;
        };
        let base = if tree == Tree::DarkOak {
            match self.dark_oak_square(pos, sapling) {
                Some(corner) => corner,
                None => return false, // dark oaks need four saplings
            }
        } else {
            pos
        };
        let rng = &mut self.rng;
        let blocks = grow(tree, base, &mut |n| rng.below(n));
        let fits = blocks.iter().all(|&(p, s)| {
            let here = self.block(p);
            if plants::is_log(s) {
                tree_can_replace(here)
            } else {
                true
            }
        });
        if !fits {
            return false;
        }
        for &(p, s) in &blocks {
            let here = self.block(p);
            if plants::is_log(s) || (is_air(here) || block_has_tag(here, "minecraft:replaceable")) {
                self.set_block(p, s);
            }
        }
        // The leaves work out their distance from the trunk.
        for &(p, _) in &blocks {
            self.update_neighbors(p);
        }
        true
    }

    /// The north-west corner of a 2x2 square of dark oak saplings containing `pos`.
    fn dark_oak_square(&self, pos: BlockPos, sapling: u16) -> Option<BlockPos> {
        for (ox, oz) in [(0, 0), (-1, 0), (0, -1), (-1, -1)] {
            let corner = BlockPos::new(pos.x + ox, pos.y, pos.z + oz);
            let all = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().all(|&(dx, dz)| {
                self.loaded_block(BlockPos::new(corner.x + dx, corner.y, corner.z + dz))
                    .is_some_and(|s| blocks::same_block(s, sapling))
            });
            if all {
                return Some(corner);
            }
        }
        None
    }

    /// Using an item on a block that the item works on rather than places: bone meal on
    /// plants, a hoe on dirt. Returns whether it did something.
    pub(super) fn use_tool_on_block(
        &mut self,
        id: i32,
        slot: usize,
        pos: BlockPos,
        face: Dir,
        clicked: u16,
    ) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let survival = p.survival();
        let held = ItemInfo::get(p.inventory.slots[slot].item).map_or("", |i| i.name);
        if held == "minecraft:bone_meal" {
            if !self.fertilize(pos, clicked) {
                return false;
            }
            self.broadcast(
                Writer::packet(clientbound::LEVEL_EVENT)
                    .i32(LEVEL_EVENT_BONE_MEAL)
                    .position(pos)
                    .i32(15)
                    .bool(false),
                None,
            );
            if survival {
                let p = self.players.get_mut(&id).expect("player exists");
                p.inventory.take(slot, 1);
                let update = p.inventory.set_slot_packet(slot);
                self.send(id, update);
            }
            return true;
        }
        if self.use_on_campfire(id, slot, pos) || self.fill_frame(id, slot, pos) {
            return true;
        }
        if held == "minecraft:flint_and_steel" {
            return self.strike_flint(id, slot, pos, face);
        }
        if held.ends_with("_hoe") && face != Dir::Down {
            let tilled = match blocks::name(clicked) {
                "minecraft:grass_block" | "minecraft:dirt" | "minecraft:dirt_path" => {
                    BlockInfo::by_name("minecraft:farmland").map(|b| b.default_state)
                }
                "minecraft:coarse_dirt" => Some(block::DIRT),
                _ => None,
            };
            let above_free = is_air(self.block(offset(pos, Dir::Up)));
            let Some(tilled) = tilled.filter(|_| above_free) else {
                return false;
            };
            self.set_block_updating(pos, tilled);
            self.play_sound("minecraft:item.hoe.till", pos, None);
            if survival {
                self.damage_item(id, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
            }
            return true;
        }
        false
    }

    /// Bone meal on a block: crops jump ahead, saplings may grow at once, grass sprouts
    /// plants around it. Returns whether it had any effect.
    /// Bone meal from a dispenser. Returns whether it did anything.
    pub(super) fn fertilize_from_dispenser(&mut self, pos: BlockPos, state: u16) -> bool {
        if !self.fertilize(pos, state) {
            return false;
        }
        self.broadcast(
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(LEVEL_EVENT_BONE_MEAL)
                .position(pos)
                .i32(15)
                .bool(false),
            None,
        );
        true
    }

    fn fertilize(&mut self, pos: BlockPos, state: u16) -> bool {
        if let Some(max) = crop_max_age(state) {
            if age(state) >= max {
                return false;
            }
            let boost = 2 + self.rng.below(4) as u8;
            self.set_block_updating(pos, with_age(state, (age(state) + boost).min(max)));
            return true;
        }
        if block_has_tag(state, "minecraft:saplings") {
            // Bone meal grows a sapling 45% of the time.
            if self.rng.f64() < 0.45 {
                self.advance_sapling(pos, with(state, "stage", "1"));
            }
            return true;
        }
        if blocks::name(state) == "minecraft:grass_block" {
            for _ in 0..16 {
                let spot = BlockPos::new(
                    pos.x + self.rng.below(7) as i32 - 3,
                    pos.y + 1,
                    pos.z + self.rng.below(7) as i32 - 3,
                );
                let ground = self.block(offset(spot, Dir::Down));
                if ground != block::GRASS_BLOCK || !is_air(self.block(spot)) {
                    continue;
                }
                let plant = match self.rng.below(10) {
                    0 => block::DANDELION,
                    1 => block::POPPY,
                    _ => block::SHORT_GRASS,
                };
                self.set_block(spot, plant);
            }
            return true;
        }
        false
    }
}

/// Blocks that do something on a random tick.
fn ticks_randomly(state: u16) -> bool {
    crop_max_age(state).is_some()
        || leaves_decay(state)
        || matches!(
            blocks::name(state),
            "minecraft:farmland"
                | "minecraft:grass_block"
                | "minecraft:mycelium"
                | "minecraft:sugar_cane"
                | "minecraft:cactus"
        )
        || block_has_tag(state, "minecraft:saplings")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat world (grass at y = -61) in full daylight, chunks around the origin loaded.
    fn flat_world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-nature-{}-{}",
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

    #[test]
    fn saplings_grow_into_trees_whose_leaves_rot_when_cut() {
        let mut world = flat_world();
        let sapling = BlockPos::new(4, -60, 4);
        world.set_block(sapling, state("minecraft:oak_sapling"));
        assert!(world.grow_tree(sapling, state("minecraft:oak_sapling")));
        assert!(plants::is_log(world.block(sapling)));
        // Leaves next to the trunk know they are one block from it.
        let crown: Vec<BlockPos> = (-60..-50)
            .flat_map(|y| [BlockPos::new(5, y, 4), BlockPos::new(6, y, 4)])
            .filter(|&p| plants::is_leaves(world.block(p)))
            .collect();
        assert!(!crown.is_empty());
        let near = crown.iter().find(|p| p.x == 5).unwrap();
        assert_eq!(prop(world.block(*near), "distance"), Some("1"));

        // Cut the whole trunk: every leaf ends up out of reach and rots.
        for y in -60..-50 {
            let p = BlockPos::new(4, y, 4);
            if plants::is_log(world.block(p)) {
                world.set_block_updating(p, block::AIR);
            }
        }
        let leaf = world.block(*near);
        assert!(
            leaves_decay(leaf),
            "{:?}",
            BlockInfo::of(leaf).properties_of(leaf)
        );
        world.random_tick(*near, leaf);
        assert!(is_air(world.block(*near)));
    }

    #[test]
    fn wheat_ripens_on_farmland_in_the_light() {
        let mut world = flat_world();
        let soil = BlockPos::new(2, -61, 2);
        let crop = offset(soil, Dir::Up);
        world.set_block(soil, with(state("minecraft:farmland"), "moisture", "7"));
        world.set_block(crop, state("minecraft:wheat"));
        for _ in 0..2000 {
            let wheat = world.block(crop);
            world.random_tick(crop, wheat);
        }
        assert_eq!(age(world.block(crop)), 7);
    }

    #[test]
    fn dry_farmland_turns_back_into_dirt() {
        let mut world = flat_world();
        let soil = BlockPos::new(2, -61, 2);
        world.set_block(soil, with(state("minecraft:farmland"), "moisture", "2"));
        for _ in 0..3 {
            let s = world.block(soil);
            world.random_tick(soil, s);
        }
        assert_eq!(world.block(soil), block::DIRT);
    }

    #[test]
    fn grass_spreads_to_dirt_nearby() {
        let mut world = flat_world();
        let dirt = BlockPos::new(3, -61, 3);
        world.set_block(dirt, block::DIRT);
        let grass = BlockPos::new(2, -61, 3);
        for _ in 0..200 {
            let g = world.block(grass);
            world.random_tick(grass, g);
        }
        assert_eq!(blocks::name(world.block(dirt)), "minecraft:grass_block");
    }
}
