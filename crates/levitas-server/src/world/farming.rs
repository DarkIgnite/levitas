//! The rest of farming, from the Minecraft Wiki's pages on each plant: melon and pumpkin
//! stems and their fruit, cocoa, sweet berry bushes, kelp, bamboo, vines spreading down,
//! mushrooms spreading, composters, and trampled farmland.

use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::blocks::plants::{age, with_age};
#[cfg(test)]
use crate::blocks::plants::{detached_stem, stem_holds_fruit};
use crate::blocks::{self, offset, prop, with, Dir};

/// Ripe stems try to put out a fruit.
const STEM_MAX_AGE: u8 = 7;
const COCOA_MAX_AGE: u8 = 2;
const BERRY_MAX_AGE: u8 = 3;
/// Kelp stops growing at this age.
const KELP_MAX_AGE: u8 = 25;
/// How tall bamboo grows.
const BAMBOO_MAX_HEIGHT: i32 = 16;
/// A composter's levels: full at 7, ready to give bone meal at 8 a second later.
const COMPOSTER_FULL: u8 = 7;
const COMPOSTER_READY: u8 = 8;
const LEVEL_EVENT_COMPOSTER_FILL: i32 = 1500;

/// The fruit a stem grows, and the stem it then becomes.
fn stem_fruit(stem: &str) -> Option<(&'static str, &'static str)> {
    match stem {
        "minecraft:melon_stem" => Some(("minecraft:melon", "minecraft:attached_melon_stem")),
        "minecraft:pumpkin_stem" => Some(("minecraft:pumpkin", "minecraft:attached_pumpkin_stem")),
        _ => None,
    }
}

fn state_of(name: &str) -> Option<u16> {
    BlockInfo::by_name(name).map(|b| b.default_state)
}

/// Blocks with growth of their own on a random tick (on top of crops, saplings, ...).
pub fn ticks_randomly(state: u16) -> bool {
    matches!(
        blocks::name(state),
        "minecraft:melon_stem"
            | "minecraft:pumpkin_stem"
            | "minecraft:cocoa"
            | "minecraft:sweet_berry_bush"
            | "minecraft:kelp"
            | "minecraft:bamboo"
            | "minecraft:bamboo_sapling"
            | "minecraft:vine"
            | "minecraft:brown_mushroom"
            | "minecraft:red_mushroom"
    )
}

impl World {
    /// The random tick of the plants this module grows. Returns whether `state` was one.
    pub(super) fn farm_random_tick(&mut self, pos: BlockPos, state: u16) -> bool {
        let name = blocks::name(state);
        let above = offset(pos, Dir::Up);
        match name {
            "minecraft:melon_stem" | "minecraft:pumpkin_stem" => {
                if self.raw_light(pos) < 9 {
                    return true;
                }
                let speed = self.growth_speed(pos, state);
                if self.rng.below((25.0 / speed) as u32 + 1) != 0 {
                    return true;
                }
                if age(state) < STEM_MAX_AGE {
                    self.set_block(pos, with_age(state, age(state) + 1));
                } else {
                    self.grow_fruit(pos, state);
                }
            }
            "minecraft:cocoa" => {
                if age(state) < COCOA_MAX_AGE && self.rng.below(5) == 0 {
                    self.set_block(pos, with_age(state, age(state) + 1));
                }
            }
            "minecraft:sweet_berry_bush" => {
                if age(state) < BERRY_MAX_AGE
                    && self.rng.below(5) == 0
                    && self.raw_light(above) >= 10
                {
                    self.set_block(pos, with_age(state, age(state) + 1));
                }
            }
            "minecraft:kelp" => {
                if self.rng.below(7) == 0 {
                    self.grow_kelp(pos, state);
                }
            }
            "minecraft:bamboo" | "minecraft:bamboo_sapling" => {
                if self.rng.below(3) == 0 && self.raw_light(above) >= 9 {
                    self.grow_bamboo(pos, state);
                }
            }
            "minecraft:vine" => {
                if crate::rules::bool("spread_vines") && self.rng.below(4) == 0 {
                    self.spread_vine(pos, state);
                }
            }
            "minecraft:brown_mushroom" | "minecraft:red_mushroom" => {
                if self.rng.below(25) == 0 {
                    self.spread_mushroom(pos, state);
                }
            }
            _ => return false,
        }
        true
    }

    /// A ripe stem puts a fruit on a free side with fitting ground, and bends to it.
    fn grow_fruit(&mut self, pos: BlockPos, stem: u16) {
        let Some((fruit, attached)) = stem_fruit(blocks::name(stem)) else {
            return;
        };
        let dir = Dir::HORIZONTAL[self.rng.below(4) as usize];
        let spot = offset(pos, dir);
        let ground = self.block(offset(spot, Dir::Down));
        if !is_air(self.block(spot)) || !block_has_tag(ground, "minecraft:supports_stem_fruit") {
            return;
        }
        let (Some(fruit), Some(attached)) = (state_of(fruit), state_of(attached)) else {
            return;
        };
        self.set_block_updating(spot, fruit);
        self.set_block(pos, with(attached, "facing", dir.name()));
    }

    /// Kelp grows one block up into still water until it is old enough.
    fn grow_kelp(&mut self, pos: BlockPos, kelp: u16) -> bool {
        let above = offset(pos, Dir::Up);
        let water = self.block(above);
        if age(kelp) >= KELP_MAX_AGE
            || !fluid_at(water).is_some_and(|f| f.fluid == Fluid::Water && f.source)
            || blocks::name(water) != "minecraft:water"
        {
            return false;
        }
        let Some(plant) = state_of("minecraft:kelp_plant") else {
            return false;
        };
        self.set_block(above, with_age(kelp, age(kelp) + 1));
        self.set_block(pos, plant);
        true
    }

    /// Bamboo adds a block on top, up to its height; a shoot becomes a stalk.
    fn grow_bamboo(&mut self, pos: BlockPos, state: u16) -> bool {
        let Some(bamboo) = state_of("minecraft:bamboo") else {
            return false;
        };
        if blocks::name(state) == "minecraft:bamboo_sapling" {
            let above = offset(pos, Dir::Up);
            if !is_air(self.block(above)) {
                return false;
            }
            self.set_block(pos, with(bamboo, "leaves", "none"));
            self.set_block(above, with(bamboo, "leaves", "small"));
            return true;
        }
        // Only the top of the stalk grows.
        let above = offset(pos, Dir::Up);
        if !is_air(self.block(above)) {
            return false;
        }
        let mut height = 1;
        while height < BAMBOO_MAX_HEIGHT
            && blocks::name(self.block(BlockPos::new(pos.x, pos.y - height, pos.z)))
                == "minecraft:bamboo"
        {
            height += 1;
        }
        if height >= BAMBOO_MAX_HEIGHT {
            return false;
        }
        self.set_block(above, with(state, "leaves", "large"));
        self.set_block(pos, with(state, "leaves", "small"));
        // Leaves only near the top.
        let lower = BlockPos::new(pos.x, pos.y - 1, pos.z);
        let below = self.block(lower);
        if blocks::name(below) == "minecraft:bamboo" {
            self.set_block(lower, with(below, "leaves", "none"));
        }
        true
    }

    /// A vine spreads one way, picked at random, as the wiki describes: down into air or
    /// onto the vine below, up into air when the area is not crowded, or sideways onto
    /// a solid block next to it, into the air beside it (round a corner, too).
    fn spread_vine(&mut self, pos: BlockPos, vine: u16) {
        let dir = Dir::ALL[self.rng.below(6) as usize];
        match dir {
            Dir::Down => self.spread_vine_down(pos, vine),
            Dir::Up => {
                let above = offset(pos, Dir::Up);
                if above.y > MAX_Y || !is_air(self.block(above)) || self.vines_crowded(pos) {
                    return;
                }
                let Some(empty) = state_of("minecraft:vine") else {
                    return;
                };
                let mut grown = blocks::with_bool(empty, "up", false);
                let mut any = false;
                for side in Dir::HORIZONTAL {
                    let has = prop(vine, side.name()) == Some("true")
                        && self.rng.below(2) == 0
                        && blocks::sturdy(self.block(offset(above, side)), side.opposite());
                    grown = blocks::with_bool(grown, side.name(), has);
                    any |= has;
                }
                if any {
                    self.set_block(above, grown);
                }
            }
            side => {
                if prop(vine, side.name()) == Some("true") || self.vines_crowded(pos) {
                    return;
                }
                let target = offset(pos, side);
                let there = self.block(target);
                if is_air(there) {
                    self.vine_into_air(pos, vine, side, target);
                } else if blocks::sturdy(there, side.opposite()) {
                    self.set_block(pos, blocks::with_bool(vine, side.name(), true));
                }
            }
        }
    }

    /// A vine spreading `side` into the air at `target`.
    fn vine_into_air(&mut self, pos: BlockPos, vine: u16, side: Dir, target: BlockPos) {
        let Some(empty) = state_of("minecraft:vine") else {
            return;
        };
        let empty = blocks::with_bool(empty, "up", false);
        let backs = |w: &mut World, at: BlockPos, face: Dir| {
            let s = w.block(offset(at, face));
            blocks::sturdy(s, face.opposite())
        };
        for turn in [side.clockwise(), side.counter_clockwise()] {
            if prop(vine, turn.name()) != Some("true") {
                continue;
            }
            // Along the same wall.
            if backs(self, target, turn) {
                self.set_block(target, blocks::with_bool(empty, turn.name(), true));
                return;
            }
            // Round the corner of the block it grows on.
            let corner = offset(target, turn);
            if is_air(self.block(corner)) && backs(self, pos, turn) {
                self.set_block(
                    corner,
                    blocks::with_bool(empty, side.opposite().name(), true),
                );
                return;
            }
        }
        // Under a ceiling.
        if blocks::sturdy(self.block(offset(target, Dir::Up)), Dir::Down) {
            self.set_block(target, blocks::with_bool(empty, "up", true));
        }
    }

    /// Whether there are 4 or more other vines in the 9x3x9 area around `pos`.
    fn vines_crowded(&mut self, pos: BlockPos) -> bool {
        let mut count = 0;
        for dx in -4..=4 {
            for dy in -1..=1 {
                for dz in -4..=4 {
                    if (dx, dy, dz) == (0, 0, 0) {
                        continue;
                    }
                    let at = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                    if blocks::name(self.block(at)) == "minecraft:vine" {
                        count += 1;
                        if count >= 4 {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Vines grow down into the air below them, each side half of the time.
    fn spread_vine_down(&mut self, pos: BlockPos, vine: u16) {
        let below = offset(pos, Dir::Down);
        let under = self.block(below);
        if blocks::name(under) == "minecraft:vine" {
            let mut grown = under;
            for side in ["north", "east", "south", "west"] {
                if prop(vine, side) == Some("true") && self.rng.below(2) == 0 {
                    grown = blocks::with_bool(grown, side, true);
                }
            }
            if grown != under {
                self.set_block(below, grown);
            }
            return;
        }
        if !is_air(under) {
            return;
        }
        let Some(empty) = state_of("minecraft:vine") else {
            return;
        };
        let mut grown = blocks::with_bool(empty, "up", false);
        let mut any = false;
        for side in ["north", "east", "south", "west"] {
            let has = prop(vine, side) == Some("true") && self.rng.below(2) == 0;
            grown = blocks::with_bool(grown, side, has);
            any |= has;
        }
        if any {
            self.set_block(below, grown);
        }
    }

    /// A mushroom puts another nearby, if there are few around and the spot suits it.
    fn spread_mushroom(&mut self, pos: BlockPos, state: u16) {
        let mut near = 0;
        for dx in -4..=4 {
            for dy in -1..=1 {
                for dz in -4..=4 {
                    let at = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                    if self
                        .loaded_block(at)
                        .is_some_and(|s| blocks::same_block(s, state))
                    {
                        near += 1;
                    }
                }
            }
        }
        if near >= 5 {
            return;
        }
        let spot = BlockPos::new(
            pos.x + self.rng.below(3) as i32 - 1,
            pos.y + self.rng.below(2) as i32 - self.rng.below(2) as i32,
            pos.z + self.rng.below(3) as i32 - 1,
        );
        if !is_air(self.block(spot)) {
            return;
        }
        let ground = self.block(offset(spot, Dir::Down));
        let rich = matches!(
            blocks::name(ground),
            "minecraft:mycelium"
                | "minecraft:podzol"
                | "minecraft:crimson_nylium"
                | "minecraft:warped_nylium"
        );
        let dark = self.raw_light(spot) <= 12;
        let mut get = |p: BlockPos| self.block(p);
        if (rich || dark) && blocks::can_survive(state, spot, &mut get) {
            self.set_block(spot, state);
        }
    }

    /// Bone meal on the plants of this module. `None` if it is not one of them.
    pub(super) fn fertilize_farm(&mut self, pos: BlockPos, state: u16) -> Option<bool> {
        Some(match blocks::name(state) {
            "minecraft:melon_stem" | "minecraft:pumpkin_stem" => {
                if age(state) >= STEM_MAX_AGE {
                    return Some(false);
                }
                let boost = 2 + self.rng.below(4) as u8;
                self.set_block(pos, with_age(state, (age(state) + boost).min(STEM_MAX_AGE)));
                true
            }
            "minecraft:cocoa" => {
                if age(state) >= COCOA_MAX_AGE {
                    return Some(false);
                }
                self.set_block(pos, with_age(state, age(state) + 1));
                true
            }
            "minecraft:sweet_berry_bush" => {
                if age(state) >= BERRY_MAX_AGE {
                    return Some(false);
                }
                self.set_block(pos, with_age(state, age(state) + 1));
                true
            }
            "minecraft:kelp" | "minecraft:kelp_plant" => {
                // The top of the stalk grows.
                let mut top = pos;
                while blocks::name(self.block(offset(top, Dir::Up))).starts_with("minecraft:kelp") {
                    top = offset(top, Dir::Up);
                }
                let top_state = self.block(top);
                blocks::name(top_state) == "minecraft:kelp" && self.grow_kelp(top, top_state)
            }
            "minecraft:bamboo" | "minecraft:bamboo_sapling" => {
                let mut top = pos;
                while blocks::name(self.block(offset(top, Dir::Up))) == "minecraft:bamboo" {
                    top = offset(top, Dir::Up);
                }
                let mut grew = false;
                for _ in 0..1 + self.rng.below(2) {
                    let state = self.block(top);
                    if !self.grow_bamboo(top, state) {
                        break;
                    }
                    grew = true;
                    top = offset(top, Dir::Up);
                }
                grew
            }
            _ => return None,
        })
    }

    /// Using a block of this module with whatever is in hand: picking sweet berries,
    /// filling or emptying a composter. Returns whether it did something.
    pub(super) fn use_farm_block(
        &mut self,
        id: i32,
        slot: usize,
        pos: BlockPos,
        state: u16,
    ) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let held = p.inventory.slots[slot];
        let held_name = ItemInfo::get(held.item).map_or("", |i| i.name);
        match blocks::name(state) {
            "minecraft:sweet_berry_bush"
                if age(state) >= 2 && held_name != "minecraft:bone_meal" =>
            {
                let ripe = age(state) == BERRY_MAX_AGE;
                let count = 1 + self.rng.below(2) as i32 + i32::from(ripe);
                if let Some(berries) = ItemInfo::id_of("minecraft:sweet_berries") {
                    self.drop_at_block(pos, Stack::new(berries, count));
                }
                self.set_block(pos, with_age(state, 1));
                self.play_sound("minecraft:block.sweet_berry_bush.pick_berries", pos, None);
                true
            }
            "minecraft:composter" => self.use_composter(id, slot, pos, state),
            _ => false,
        }
    }

    fn use_composter(&mut self, id: i32, slot: usize, pos: BlockPos, state: u16) -> bool {
        let level: u8 = prop(state, "level")
            .and_then(|l| l.parse().ok())
            .unwrap_or(0);
        if level == COMPOSTER_READY {
            if let Some(bone_meal) = ItemInfo::id_of("minecraft:bone_meal") {
                self.drop_at_block(offset(pos, Dir::Up), Stack::new(bone_meal, 1));
            }
            self.set_block(pos, with(state, "level", "0"));
            self.play_sound("minecraft:block.composter.empty", pos, None);
            return true;
        }
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let held = p.inventory.slots[slot];
        let survival = p.survival();
        let chance = ItemInfo::get(held.item).map_or(0, |i| i.compost);
        if held.is_empty() || chance == 0 || level >= COMPOSTER_FULL {
            return false;
        }
        // The first layer always takes.
        let added = level == 0 || self.rng.below(100) < u32::from(chance);
        if survival {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        self.level_event(LEVEL_EVENT_COMPOSTER_FILL, pos, i32::from(added));
        if !added {
            self.play_sound("minecraft:block.composter.fill", pos, None);
            return true;
        }
        let level = level + 1;
        let full = with(state, "level", &level.to_string());
        self.set_block(pos, full);
        self.play_sound("minecraft:block.composter.fill_success", pos, None);
        if level == COMPOSTER_FULL {
            self.schedule(pos, full, 20);
        }
        true
    }

    /// A full composter is ready a second later.
    pub(super) fn composter_tick(&mut self, pos: BlockPos, state: u16) {
        if blocks::name(state) == "minecraft:composter"
            && prop(state, "level") == Some(&*COMPOSTER_FULL.to_string())
        {
            self.set_block(pos, with(state, "level", &COMPOSTER_READY.to_string()));
            self.play_sound("minecraft:block.composter.ready", pos, None);
        }
    }

    /// A fall onto farmland may trample it back to dirt: the chance is the fall height
    /// less half a block.
    pub(super) fn trample(&mut self, ground: BlockPos, fallen: f64) {
        let state = self.block(ground);
        if blocks::name(state) != "minecraft:farmland" || fallen <= 0.5 {
            return;
        }
        if self.rng.f64() < fallen - 0.5 {
            self.set_block_updating(ground, block::DIRT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-farming-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        w
    }

    fn state(name: &str) -> u16 {
        state_of(name).unwrap()
    }

    fn with_player(w: &mut World) -> (i32, mpsc::Receiver<Frame>) {
        let (out, frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, out, [0.5, -60.0, 0.5]));
        (id, frames)
    }

    fn use_at(w: &mut World, id: i32, pos: BlockPos) -> bool {
        let state = w.block(pos);
        w.use_farm_block(id, HOTBAR, pos, state)
    }

    #[test]
    fn vines_spread_up_along_and_around_walls() {
        let mut w = world();
        // A wall of stone, x = 5, from y -60 to -50, z 0 to 6; a vine on its west face.
        let stone = state("minecraft:stone");
        for y in -60..=-50 {
            for z in 0..=6 {
                w.set_block(BlockPos::new(5, y, z), stone);
            }
        }
        let vine = blocks::with_bool(
            blocks::with_bool(state("minecraft:vine"), "east", true),
            "up",
            false,
        );
        let start = BlockPos::new(4, -55, 3);
        w.set_block(start, vine);
        for _ in 0..400 {
            let s = w.block(start);
            w.spread_vine(start, s);
        }
        let vines = |w: &mut World| {
            let mut n = Vec::new();
            for y in -60..=-50 {
                for z in 0..=6 {
                    let p = BlockPos::new(4, y, z);
                    if blocks::name(w.block(p)) == "minecraft:vine" {
                        n.push(p);
                    }
                }
            }
            n
        };
        let found = vines(&mut w);
        // It went up and along the wall, and stopped when the area got crowded.
        assert!(found.iter().any(|p| p.y > start.y), "{found:?}");
        assert!(
            found.iter().any(|p| p.y == start.y && p.z != start.z),
            "{found:?}"
        );
        assert!(found.len() <= 6, "{found:?}");
        for p in &found {
            assert_eq!(prop(w.block(*p), "east"), Some("true"));
        }
    }

    #[test]
    fn ripe_stems_grow_fruit_and_take_it_back() {
        let mut w = world();
        let stem_pos = BlockPos::new(4, -60, 4);
        let farmland = state("minecraft:farmland");
        w.set_block(BlockPos::new(4, -61, 4), farmland);
        let stem = with_age(state("minecraft:pumpkin_stem"), STEM_MAX_AGE);
        w.set_block(stem_pos, stem);
        // Grass all around: a pumpkin comes on one side.
        for _ in 0..64 {
            let s = w.block(stem_pos);
            if blocks::name(s) != "minecraft:pumpkin_stem" {
                break;
            }
            w.grow_fruit(stem_pos, s);
        }
        let attached = w.block(stem_pos);
        assert_eq!(blocks::name(attached), "minecraft:attached_pumpkin_stem");
        let dir = Dir::from_name(prop(attached, "facing").unwrap()).unwrap();
        assert_eq!(
            blocks::name(w.block(offset(stem_pos, dir))),
            "minecraft:pumpkin"
        );
        // Taking the pumpkin away straightens the stem again.
        assert!(stem_holds_fruit(attached, w.block(offset(stem_pos, dir))));
        let back = detached_stem(attached, false).unwrap();
        assert_eq!(
            (blocks::name(back), age(back)),
            ("minecraft:pumpkin_stem", 7)
        );
    }

    #[test]
    fn composters_fill_up_and_give_bone_meal() {
        let mut w = world();
        let (id, _frames) = with_player(&mut w);
        let pos = BlockPos::new(2, -60, 2);
        w.set_block(pos, state("minecraft:composter"));
        let seeds = ItemInfo::id_of("minecraft:wheat_seeds").unwrap();
        let pie = ItemInfo::id_of("minecraft:pumpkin_pie").unwrap();
        w.players.get_mut(&id).unwrap().inventory.slots[HOTBAR] = Stack::new(seeds, 64);
        // The first item always makes a layer.
        assert!(use_at(&mut w, id, pos));
        assert_eq!(prop(w.block(pos), "level"), Some("1"));
        assert_eq!(w.players[&id].inventory.slots[HOTBAR].count, 63);
        // Pie always adds one: six more fill it.
        w.players.get_mut(&id).unwrap().inventory.slots[HOTBAR] = Stack::new(pie, 64);
        for _ in 0..6 {
            assert!(use_at(&mut w, id, pos));
        }
        assert_eq!(prop(w.block(pos), "level"), Some("7"));
        assert!(!use_at(&mut w, id, pos), "full");
        {
            let s = w.block(pos);
            w.composter_tick(pos, s)
        };
        assert_eq!(prop(w.block(pos), "level"), Some("8"));
        // Emptied by hand, it gives bone meal.
        w.players.get_mut(&id).unwrap().inventory.slots[HOTBAR] = Stack::EMPTY;
        assert!(use_at(&mut w, id, pos));
        assert_eq!(prop(w.block(pos), "level"), Some("0"));
        // Dirt is no compost.
        w.players.get_mut(&id).unwrap().inventory.slots[HOTBAR] =
            Stack::new(ItemInfo::id_of("minecraft:dirt").unwrap(), 1);
        assert!(!use_at(&mut w, id, pos));
    }

    #[test]
    fn berries_are_picked_and_plants_grow_with_bone_meal() {
        let mut w = world();
        let (id, _frames) = with_player(&mut w);
        let bush_pos = BlockPos::new(3, -60, 3);
        let bush = with_age(state("minecraft:sweet_berry_bush"), 3);
        w.set_block(bush_pos, bush);
        assert!(w.use_farm_block(id, HOTBAR, bush_pos, bush));
        assert_eq!(age(w.block(bush_pos)), 1);
        assert_eq!(
            {
                let s = w.block(bush_pos);
                w.fertilize_farm(bush_pos, s)
            },
            Some(true)
        );
        assert_eq!(age(w.block(bush_pos)), 2);
        // Bamboo: a shoot becomes a stalk with leaves on top.
        let shoot = BlockPos::new(6, -60, 6);
        w.set_block(shoot, state("minecraft:bamboo_sapling"));
        assert!({
            let s = w.block(shoot);
            w.grow_bamboo(shoot, s)
        });
        assert_eq!(
            blocks::name(w.block(offset(shoot, Dir::Up))),
            "minecraft:bamboo"
        );
        assert_eq!(
            w.fertilize_farm(BlockPos::new(0, -61, 0), block::STONE),
            None
        );
    }

    #[test]
    fn falls_on_farmland_trample_it() {
        let mut w = world();
        let ground = BlockPos::new(1, -61, 1);
        w.set_block(ground, state("minecraft:farmland"));
        w.trample(ground, 0.5);
        assert_eq!(
            blocks::name(w.block(ground)),
            "minecraft:farmland",
            "a step is safe"
        );
        w.trample(ground, 3.0);
        assert_eq!(w.block(ground), block::DIRT);
    }
}
