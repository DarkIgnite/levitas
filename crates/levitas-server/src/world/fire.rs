//! Fire: lighting it with flint and steel, burning out, being rained out, burning the
//! blocks around it away and spreading to new ones.
//!
//! Behaviour follows the Minecraft Wiki's fire page: fire ticks every 30 to 40 game
//! ticks and slowly ages; without anything to burn it goes out, and old fire may go out
//! on its own unless it sits on netherrack or magma. Each tick it may burn away its
//! neighbours by their flammability (TNT is lit instead) and light the air around it,
//! up to four blocks above and one below, by how readily the blocks there catch fire.

use super::*;
use crate::blocks::fire::{burns_forever_on, fire_for, fire_survives, flammability};
use crate::blocks::{self, offset, prop, with, Dir};

/// Fire ticks every this many game ticks, plus up to `FIRE_TICK_SPREAD` more.
const FIRE_TICK: u64 = 30;
const FIRE_TICK_SPREAD: u32 = 10;
const MAX_AGE: u32 = 15;
/// Normal difficulty, which adds to how readily fire spreads.
const DIFFICULTY: u32 = 2;

impl World {
    /// Flint and steel on a block: lights TNT, unlit campfires and candles, or sets fire
    /// to the side that was clicked. Returns whether it did anything.
    pub(super) fn strike_flint(&mut self, id: i32, slot: usize, pos: BlockPos, face: Dir) -> bool {
        let clicked = self.block(pos);
        let n = blocks::name(clicked);
        if n == "minecraft:tnt" {
            self.set_block_updating(pos, block::AIR);
            self.prime_tnt(pos, super::tnt::FUSE);
        } else if (n.ends_with("campfire") || n.ends_with("candle") || n.ends_with("candle_cake"))
            && prop(clicked, "lit") == Some("false")
            && prop(clicked, "waterlogged") != Some("true")
        {
            self.set_block_updating(pos, with(clicked, "lit", "true"));
        } else {
            let target = offset(pos, face);
            let Some(fire) = fire_for(target, &mut |q| self.block(q)) else {
                return false;
            };
            self.light_fire(target, fire);
        }
        self.play_sound("minecraft:item.flintandsteel.use", pos, None);
        if self.players.get(&id).is_some_and(|p| p.survival()) {
            self.damage_item(id, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
        }
        true
    }

    /// Places fire and sets it ticking.
    pub(super) fn light_fire(&mut self, pos: BlockPos, fire: u16) {
        self.set_block_updating(pos, fire);
        self.schedule_fire(pos, fire);
    }

    fn schedule_fire(&mut self, pos: BlockPos, fire: u16) {
        let delay = FIRE_TICK + u64::from(self.rng.below(FIRE_TICK_SPREAD));
        self.schedule(pos, fire, delay);
    }

    /// A fire's scheduled tick.
    pub(super) fn fire_tick(&mut self, pos: BlockPos, state: u16) {
        if !fire_survives(state, pos, &mut |q| self.block(q)) {
            self.set_block_updating(pos, block::AIR);
            return;
        }
        // Soul fire neither ages nor spreads.
        if blocks::name(state) == "minecraft:soul_fire" {
            return;
        }
        let below = self.block(offset(pos, Dir::Down));
        let forever = burns_forever_on(below);
        let age: u32 = prop(state, "age").and_then(|a| a.parse().ok()).unwrap_or(0);

        if !forever && self.rained_on(pos) {
            let chance = 0.2 + f64::from(age) * 0.03;
            if self.rng.f64() < chance {
                self.set_block_updating(pos, block::AIR);
                return;
            }
        }
        let age = (age + self.rng.below(3) / 2).min(MAX_AGE);
        let state = with(state, "age", &age.to_string());
        self.set_block(pos, state);

        if !forever {
            let fuel_nearby = Dir::ALL
                .iter()
                .any(|&d| flammability(self.block(offset(pos, d))).is_some());
            // With nothing to burn, fire on bare ground soon goes out and spreads nowhere.
            if !fuel_nearby {
                if !blocks::sturdy(below, Dir::Up) || age > 3 {
                    self.set_block_updating(pos, block::AIR);
                } else {
                    self.schedule_fire(pos, state);
                }
                return;
            }
            if age == MAX_AGE && self.rng.below(4) == 0 && flammability(below).is_none() {
                self.set_block_updating(pos, block::AIR);
                return;
            }
        }

        // Burn the neighbours away; sideways less readily than up and down.
        for d in Dir::ALL {
            let odds = if matches!(d, Dir::Up | Dir::Down) {
                250
            } else {
                300
            };
            self.burn_away(offset(pos, d), odds, age);
        }

        // Light the air around.
        for dx in -1..=1 {
            for dz in -1..=1 {
                for dy in -1..=4 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let target = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                    let divisor = 100 + if dy > 1 { (dy as u32 - 1) * 100 } else { 0 };
                    self.spread_to(target, divisor, age);
                }
            }
        }
        if self.block(pos) == state {
            self.schedule_fire(pos, state);
        }
    }

    /// Fire next door may burn the block at `pos` away, leaving fire or nothing.
    fn burn_away(&mut self, pos: BlockPos, odds: u32, age: u32) {
        let Some(state) = self.loaded_block(pos) else {
            return;
        };
        let Some(f) = flammability(state) else {
            return;
        };
        if self.rng.below(odds) >= f.burn {
            return;
        }
        self.remove_block_entity(pos);
        if blocks::name(state) == "minecraft:tnt" {
            self.set_block_updating(pos, block::AIR);
            self.prime_tnt(pos, super::tnt::FUSE);
            return;
        }
        if self.rng.below(age + 10) < 5 && !self.rained_on(pos) {
            self.set_block_updating(pos, block::AIR);
            let age = (age + self.rng.below(5) / 4).min(MAX_AGE);
            if let Some(fire) = fire_for(pos, &mut |q| self.block(q)) {
                self.light_fire(pos, with(fire, "age", &age.to_string()));
            }
        } else {
            self.set_block_updating(pos, block::AIR);
        }
    }

    /// Fire nearby may light the air at `pos` if something next to it burns.
    fn spread_to(&mut self, pos: BlockPos, divisor: u32, age: u32) {
        if self.loaded_block(pos).is_none_or(|s| !is_air(s)) {
            return;
        }
        let encouragement = Dir::ALL
            .iter()
            .filter_map(|&d| self.loaded_block(offset(pos, d)).and_then(flammability))
            .map(|f| f.ignite)
            .max()
            .unwrap_or(0);
        if encouragement == 0 {
            return;
        }
        let odds = (encouragement + 40 + DIFFICULTY * 7) / (age + 30);
        if odds == 0 || self.rng.below(divisor) > odds || self.rained_on(pos) {
            return;
        }
        let age = (age + self.rng.below(5) / 4).min(MAX_AGE);
        if let Some(fire) = fire_for(pos, &mut |q| self.block(q)) {
            self.light_fire(pos, with(fire, "age", &age.to_string()));
        }
    }

    /// Whether rain falls on `pos` or right beside it.
    fn rained_on(&self, pos: BlockPos) -> bool {
        self.weather.is_raining()
            && [pos]
                .into_iter()
                .chain([Dir::North, Dir::East, Dir::South, Dir::West].map(|d| offset(pos, d)))
                .any(|q| self.light_at(q).is_some_and(|(sky, _)| sky == 15))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::fire::{fire_for, flammability};

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-fire-{}-{}",
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
        levitas_registry::BlockInfo::by_name(name)
            .unwrap()
            .default_state
    }

    #[test]
    fn wood_and_leaves_burn_but_stone_and_nether_wood_do_not() {
        assert!(flammability(state("minecraft:oak_planks")).is_some());
        assert!(flammability(state("minecraft:oak_log")).is_some());
        assert!(flammability(state("minecraft:oak_leaves")).is_some());
        assert!(flammability(state("minecraft:white_wool")).is_some());
        assert!(flammability(state("minecraft:stone")).is_none());
        assert!(flammability(state("minecraft:crimson_planks")).is_none());
    }

    #[test]
    fn fire_needs_a_floor_or_fuel() {
        let mut w = world();
        let ground = BlockPos::new(0, -60, 0);
        assert!(fire_for(ground, &mut |q| w.block(q)).is_some());
        let floating = BlockPos::new(0, -50, 0);
        assert!(fire_for(floating, &mut |q| w.block(q)).is_none());
        w.set_block(BlockPos::new(1, -50, 0), state("minecraft:oak_planks"));
        let clinging = fire_for(floating, &mut |q| w.block(q)).unwrap();
        assert_eq!(prop(clinging, "east"), Some("true"));
    }

    #[test]
    fn fire_on_bare_ground_goes_out() {
        let mut w = world();
        let pos = BlockPos::new(0, -60, 0);
        let fire = fire_for(pos, &mut |q| w.block(q)).unwrap();
        w.light_fire(pos, fire);
        for _ in 0..200 {
            let s = w.block(pos);
            if is_air(s) {
                return;
            }
            w.fire_tick(pos, s);
        }
        panic!("fire never went out");
    }

    #[test]
    fn fire_burns_wood_away_and_spreads() {
        let mut w = world();
        let planks = state("minecraft:oak_planks");
        for x in 1..=3 {
            w.set_block(BlockPos::new(x, -60, 0), planks);
        }
        let pos = BlockPos::new(0, -60, 0);
        let fire = fire_for(pos, &mut |q| w.block(q)).unwrap();
        w.light_fire(pos, fire);
        let first = BlockPos::new(1, -60, 0);
        for _ in 0..500 {
            if w.block(first) != planks {
                return;
            }
            let s = w.block(pos);
            if !crate::blocks::fire::is_fire(s) {
                w.light_fire(pos, fire);
            }
            let s = w.block(pos);
            w.fire_tick(pos, s);
        }
        panic!("the planks never burned");
    }

    #[test]
    fn lit_tnt_explodes_and_sets_off_tnt_nearby() {
        let mut w = world();
        let tnt = state("minecraft:tnt");
        let other = BlockPos::new(2, -60, 0);
        w.set_block(other, tnt);
        w.prime_tnt(BlockPos::new(0, -60, 0), crate::world::tnt::FUSE);
        for _ in 0..crate::world::tnt::FUSE {
            w.tick_tnt();
        }
        assert!(is_air(w.block(other)), "the other TNT was lit");
        assert_eq!(w.tnt.len(), 1, "and is now burning");
        assert!(
            is_air(w.block(BlockPos::new(0, -61, 0))),
            "the blast broke the ground"
        );
    }
}
