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
        if self.farm_random_tick(pos, state) || self.melt(pos, state) {
            return;
        }
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

    /// Ice melts in block light above 11 on any side, into water (into nothing in the
    /// Nether); snow layers melt all at once in block light of 12 or more. Lava sets
    /// fires. Returns whether the block was one of these.
    fn melt(&mut self, pos: BlockPos, state: u16) -> bool {
        match blocks::name(state) {
            "minecraft:ice" => {
                let bright = Dir::ALL.iter().any(|&d| {
                    self.light_at(offset(pos, d))
                        .is_some_and(|(_, block)| block > 11)
                });
                if bright {
                    let left = if self.dimension.ultrawarm() {
                        block::AIR
                    } else {
                        block::WATER
                    };
                    self.set_block_updating(pos, left);
                }
                true
            }
            "minecraft:snow" => {
                if self.light_at(pos).is_some_and(|(_, block)| block >= 12) {
                    self.set_block_updating(pos, block::AIR);
                }
                true
            }
            "minecraft:lava" => {
                self.lava_sets_fire(pos);
                true
            }
            _ => false,
        }
    }

    /// Light at a block ignoring the time of day: the brighter of sky and block light.
    pub(super) fn raw_light(&self, pos: BlockPos) -> u8 {
        self.light_at(pos).map_or(0, |(sky, block)| sky.max(block))
    }

    /// How fast a crop grows (the wiki's "growth points"): more for farmland around it,
    /// more still when moist, halved when crops of the same kind crowd it in rows.
    pub(super) fn growth_speed(&self, pos: BlockPos, crop: u16) -> f64 {
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
        if held == "minecraft:filled_map" && blocks::name(clicked).ends_with("_banner") {
            let map = self
                .players
                .get(&id)
                .and_then(|p| p.inventory.slots[slot].map_id);
            if let Some(map) = map {
                if self.mark_banner(map, pos, clicked) {
                    return true;
                }
            }
        }
        if held == "minecraft:compass" && blocks::name(clicked) == "minecraft:lodestone" {
            return self.bind_lodestone(id, slot, pos);
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

    /// Frost walker boots freeze the still water around a player walking on the ground:
    /// within 2 blocks plus the level, water sources with air above become frosted ice.
    pub(super) fn frost_walk(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let level = p.inventory.slots[8]
            .enchants
            .level_of("minecraft:frost_walker");
        if level == 0 {
            return;
        }
        let radius = (2 + level).min(16) as i32;
        let under = BlockPos::new(
            p.pos[0].floor() as i32,
            (p.pos[1] - 0.5).floor() as i32,
            p.pos[2].floor() as i32,
        );
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx * dx + dz * dz > radius * radius {
                    continue;
                }
                let pos = BlockPos::new(under.x + dx, under.y, under.z + dz);
                let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
                if self.loaded_block(pos) != Some(block::WATER)
                    || self.loaded_block(above) != Some(block::AIR)
                {
                    continue;
                }
                self.set_block_updating(pos, block::FROSTED_ICE);
                let delay = 60 + u64::from(self.rng.below(60));
                self.schedule(pos, block::FROSTED_ICE, delay);
            }
        }
    }

    /// Frosted ice cracks a little now and then, and melts back into water at the end.
    pub(super) fn frosted_ice_tick(&mut self, pos: BlockPos, state: u16) {
        if blocks::name(state) != "minecraft:frosted_ice" {
            return;
        }
        let age: u8 = blocks::prop(state, "age")
            .and_then(|a| a.parse().ok())
            .unwrap_or(0);
        if age >= 3 {
            self.set_block_updating(pos, block::WATER);
            return;
        }
        let older = blocks::with(state, "age", &(age + 1).to_string());
        self.set_block_updating(pos, older);
        let delay = 20 + u64::from(self.rng.below(20));
        self.schedule(pos, older, delay);
    }

    /// Every second: compasses tracking a lodestone of this dimension that is no longer
    /// there lose their target and spin, as in vanilla.
    pub(super) fn tick_lodestone_compasses(&mut self) {
        if !self.tick.is_multiple_of(20) {
            return;
        }
        let here = self.dimension.name();
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for id in ids {
            let gone: Vec<usize> = self.players[&id]
                .inventory
                .slots
                .iter()
                .enumerate()
                .filter_map(|(slot, s)| {
                    let t = s.lodestone?;
                    let checkable = t.tracked && !t.lost && t.dimension == here;
                    let state = self.loaded_block(t.pos).filter(|_| checkable)?;
                    (blocks::name(state) != "minecraft:lodestone").then_some(slot)
                })
                .collect();
            for slot in gone {
                let p = self.players.get_mut(&id).expect("listed");
                if let Some(t) = p.inventory.slots[slot].lodestone.as_mut() {
                    t.lost = true;
                }
                let update = p.inventory.set_slot_packet(slot);
                self.send(id, update);
            }
        }
    }

    /// Right-clicking a lodestone with a compass locks the compass onto that block.
    pub(super) fn bind_lodestone(&mut self, id: i32, slot: usize, pos: BlockPos) -> bool {
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        let compass = p.inventory.slots[slot];
        if compass.is_empty() {
            return false;
        }
        let tracker = crate::inventory::LodestoneTracker {
            dimension: self.dimension.name(),
            pos,
            tracked: true,
            lost: false,
        };
        let sound_pos = pos;
        let player_pos = p.pos;
        let survival = p.survival();
        if compass.count == 1 {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.slots[slot].lodestone = Some(tracker);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        } else if survival {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.take(slot, 1);
            let mut bound = compass.with_count(1);
            bound.lodestone = Some(tracker);
            let (changed, over) = p.inventory.add(bound);
            let mut updates: Vec<Writer> = changed
                .into_iter()
                .map(|s| p.inventory.set_slot_packet(s))
                .collect();
            updates.push(p.inventory.set_slot_packet(slot));
            for update in updates {
                self.send(id, update);
            }
            if over > 0 {
                self.scatter_item(player_pos, bound.with_count(over));
            }
        } else {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.slots[slot].lodestone = Some(tracker);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        self.play_sound("minecraft:item.lodestone_compass.lock", sound_pos, None);
        true
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
        if let Some(done) = self.fertilize_farm(pos, state) {
            return done;
        }
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
        // A small shelf mushroom grows into a large one; large ones take no bone meal.
        if blocks::name(state) == "minecraft:shelf_mushroom" {
            if prop(state, "age") != Some("0") {
                return false;
            }
            self.set_block(pos, with(state, "age", "1"));
            return true;
        }
        // A red shrub tries to put another one next to, above or below itself.
        if blocks::name(state) == "minecraft:red_shrub" {
            let dirs = [
                Dir::North,
                Dir::South,
                Dir::West,
                Dir::East,
                Dir::Up,
                Dir::Down,
            ];
            let spot = offset(pos, dirs[self.rng.below(6) as usize]);
            let mut get = |p: BlockPos| self.block(p);
            if is_air(get(spot)) && blocks::can_survive(state, spot, &mut get) {
                self.set_block(spot, state);
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
                | "minecraft:ice"
                | "minecraft:snow"
                | "minecraft:lava"
        )
        || block_has_tag(state, "minecraft:saplings")
        || super::farming::ticks_randomly(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ice_and_snow_melt_by_bright_block_light_only() {
        let mut w = flat_world();
        let ice = BlockPos::new(0, -60, 0);
        let snow = BlockPos::new(4, -60, 0);
        w.set_block_updating(ice, block::ICE);
        w.set_block_updating(snow, block::SNOW);
        // Daylight alone melts nothing.
        assert!(w.melt(ice, block::ICE));
        assert!(w.melt(snow, block::SNOW));
        assert_eq!(w.block(ice), block::ICE);
        assert_eq!(w.block(snow), block::SNOW);
        // A torch (14) two blocks off gives 13 beside the ice: it melts into water.
        w.set_block_updating(BlockPos::new(0, -60, 2), block::TORCH);
        w.melt(ice, block::ICE);
        assert_eq!(w.block(ice), block::WATER);
        // Glowstone beside the snow (14 there) melts it away.
        w.set_block_updating(BlockPos::new(5, -60, 0), block::GLOWSTONE);
        w.melt(snow, block::SNOW);
        assert_eq!(w.block(snow), block::AIR);
    }

    #[test]
    fn lava_sets_fire_to_wood_nearby() {
        let mut w = flat_world();
        let lava = BlockPos::new(0, -60, 0);
        w.set_block(lava, block::LAVA);
        for dx in -2..=2 {
            for dz in -2..=2 {
                if dx != 0 || dz != 0 {
                    w.set_block(BlockPos::new(dx, -60, dz), block::OAK_PLANKS);
                }
            }
        }
        let burning = |w: &World| {
            (-2..=2).any(|dx| {
                (-2..=2).any(|dz| {
                    (-59..=-58).any(|y| {
                        w.loaded_block(BlockPos::new(dx, y, dz))
                            .is_some_and(|s| blocks::name(s) == "minecraft:fire")
                    })
                })
            })
        };
        for _ in 0..200 {
            w.lava_sets_fire(lava);
            if burning(&w) {
                return;
            }
        }
        panic!("lava never set the planks alight");
    }

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

    #[test]
    fn frost_walker_freezes_water_that_melts_again() {
        let mut w = flat_world();
        for x in -4..5 {
            for z in -4..5 {
                w.set_block(BlockPos::new(x, -61, z), block::WATER);
            }
        }
        let id = next_entity_id();
        let (tx, _rx) = tokio::sync::mpsc::channel(4096);
        let profile = crate::connection::Profile {
            uuid: 78,
            name: "Skater".to_owned(),
            properties: Vec::new(),
        };
        let mut p = Player::new(profile, tx, [0.5, -60.0, 0.5]);
        let boots = ItemInfo::id_of("minecraft:diamond_boots").unwrap();
        let mut b = Stack::new(boots, 1);
        b.enchants.set(
            crate::enchanting::enchantment_id("minecraft:frost_walker").unwrap(),
            1,
        );
        p.inventory.slots[8] = b;
        w.players.insert(id, p);
        w.frost_walk(id);
        assert_eq!(w.block(BlockPos::new(2, -61, 2)), block::FROSTED_ICE);
        assert_eq!(
            w.block(BlockPos::new(4, -61, 0)),
            block::WATER,
            "only within 3 blocks"
        );
        // Left alone, it cracks and melts back into water.
        for _ in 0..400 {
            w.tick += 1;
            w.tick_scheduled();
        }
        assert_eq!(w.block(BlockPos::new(2, -61, 2)), block::WATER);
    }

    #[test]
    fn a_compass_spins_once_its_lodestone_is_gone() {
        let mut w = flat_world();
        let id = next_entity_id();
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        let profile = crate::connection::Profile {
            uuid: 77,
            name: "Pointer".to_owned(),
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, tx, [0.5, -60.0, 0.5]));
        let lodestone = BlockPos::new(3, -60, 3);
        w.set_block(
            lodestone,
            BlockInfo::by_name("minecraft:lodestone")
                .unwrap()
                .default_state,
        );
        let compass = ItemInfo::id_of("minecraft:compass").unwrap();
        let mut stack = Stack::new(compass, 1);
        stack.lodestone = Some(crate::inventory::LodestoneTracker {
            dimension: w.dimension.name(),
            pos: lodestone,
            tracked: true,
            lost: false,
        });
        w.players.get_mut(&id).unwrap().inventory.slots[crate::inventory::HOTBAR] = stack;
        w.tick = 20;
        w.tick_lodestone_compasses();
        assert!(
            !w.players[&id].inventory.slots[crate::inventory::HOTBAR]
                .lodestone
                .unwrap()
                .lost
        );
        w.set_block(lodestone, block::AIR);
        w.tick_lodestone_compasses();
        let t = w.players[&id].inventory.slots[crate::inventory::HOTBAR]
            .lodestone
            .unwrap();
        assert!(t.lost);
        // Saved without a target, and read back spinning.
        let back =
            Stack::from_nbt(&w.players[&id].inventory.slots[crate::inventory::HOTBAR].to_nbt());
        assert!(back.lodestone.unwrap().lost);
    }

    #[test]
    fn lodestone_compass_binding() {
        let mut world = flat_world();
        let (out, _frames) = mpsc::channel(100);
        let id = next_entity_id();
        let profile = crate::connection::Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        let mut player = Player::new(profile, out, [0.0, 0.0, 0.0]);
        let compass = ItemInfo::id_of("minecraft:compass").unwrap();
        player.inventory.slots[crate::inventory::HOTBAR] = Stack::new(compass, 1);
        world.players.insert(id, player);

        let lodestone_pos = BlockPos::new(5, -60, 5);
        world.set_block(lodestone_pos, state("minecraft:lodestone"));

        assert!(world.bind_lodestone(id, crate::inventory::HOTBAR, lodestone_pos));
        let bound = world.players[&id].inventory.slots[crate::inventory::HOTBAR];
        assert_eq!(bound.item, compass);
        let tracker = bound
            .lodestone
            .expect("compass should be bound to lodestone");
        assert_eq!(tracker.pos, lodestone_pos);
        assert_eq!(tracker.dimension, "minecraft:overworld");
        assert!(tracker.tracked);
    }
}
