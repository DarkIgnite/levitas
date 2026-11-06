//! Potent sulfur (new in 26.3), as the Minecraft Wiki describes it. Under one to four
//! blocks of still water with air above, it gives off noxious gas: whoever is in that
//! water within three blocks of it gets Nausea. With magma under it the water above
//! becomes a geyser that erupts now and then, with lava under it one that never stops;
//! an erupting geyser throws whatever is in it upwards. The block's state shows which.

use levitas_registry::{mob_effect, Effect};

use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::blocks::{self, offset, prop, with, Dir};

/// How often the blocks around players are looked at.
const SCAN_TICKS: u64 = 10;
/// The most water potent sulfur works under.
const MAX_DEPTH: i32 = 4;
/// How far from the sulfur the gas reaches, sideways.
const GAS_RADIUS: i32 = 3;
/// Nausea from the gas, renewed while one stays in the water.
const NAUSEA_TICKS: i32 = 80;
/// Gravity on a player, in blocks per tick squared, for working out a geyser's push.
const GRAVITY: f64 = 0.08;

/// A magma-fed geyser's rhythm, from the wiki: each block has a dormant value (15 to 30)
/// and an eruption value (1 or 2) of its own, and with `depth` blocks of water it rests
/// for 10 x (depth - 1) + dormant seconds, then erupts for (depth - 1) + eruption seconds.
/// Returns the cycle and the eruption's length, in ticks.
fn geyser_rhythm(pos: BlockPos, depth: i32) -> (u64, u64) {
    let hash = (pos.x.wrapping_mul(73_428_767)
        ^ pos.z.wrapping_mul(912_931)
        ^ pos.y.wrapping_mul(438_289)) as u32;
    let dormant_value = 15 + u64::from(hash % 16);
    let eruption_value = 1 + u64::from((hash >> 8) % 2);
    let extra = u64::from((depth - 1).max(0) as u32);
    let dormant = (10 * extra + dormant_value) * 20;
    let erupting = (extra + eruption_value) * 20;
    (dormant + erupting, erupting)
}

/// The upward push that throws a player about 5 blocks per block of water, as high as
/// the wiki says a geyser reaches.
fn geyser_push(depth: i32) -> f64 {
    (2.0 * GRAVITY * 5.0 * f64::from(depth)).sqrt()
}

fn is_potent_sulfur(state: u16) -> bool {
    blocks::name(state) == "minecraft:potent_sulfur"
}

fn is_water_source(state: u16) -> bool {
    fluid_at(state).is_some_and(|f| f.fluid == Fluid::Water && f.source)
}

impl World {
    /// The depth of still water over the sulfur at `pos`, if the gas and geysers work:
    /// one to four water sources, then air.
    fn sulfur_water_depth(&self, pos: BlockPos) -> Option<i32> {
        let mut depth = 0;
        loop {
            let above = self.loaded_block_at(BlockPos::new(pos.x, pos.y + depth + 1, pos.z))?;
            if is_water_source(above) {
                depth += 1;
                if depth > MAX_DEPTH {
                    return None;
                }
            } else {
                return (depth > 0 && is_air(above)).then_some(depth);
            }
        }
    }

    /// What the sulfur at `pos` should be doing now.
    fn sulfur_state(&self, pos: BlockPos) -> &'static str {
        let Some(above) = self.loaded_block_at(offset(pos, Dir::Up)) else {
            return "dry";
        };
        if fluid_at(above).is_none_or(|f| f.fluid != Fluid::Water) {
            return "dry";
        }
        let Some(depth) = self.sulfur_water_depth(pos) else {
            return "wet";
        };
        let below = self
            .loaded_block_at(offset(pos, Dir::Down))
            .unwrap_or(block::AIR);
        if fluid_at(below).is_some_and(|f| f.fluid == Fluid::Lava) {
            return "continuous";
        }
        if blocks::name(below) == "minecraft:magma_block" {
            // Each geyser keeps its own rhythm.
            let (cycle, erupting) = geyser_rhythm(pos, depth);
            let offset = u64::from((pos.x.wrapping_mul(31) ^ pos.z) as u32);
            let phase = self.tick.wrapping_add(offset) % cycle;
            return if phase < erupting {
                "erupting"
            } else {
                "dormant"
            };
        }
        "wet"
    }

    /// Updates the potent sulfur near players and lets its gas and geysers act on them.
    pub(super) fn tick_potent_sulfur(&mut self) {
        if !self.tick.is_multiple_of(SCAN_TICKS) {
            return;
        }
        let players: Vec<(i32, [f64; 3])> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive)
            .map(|(&id, p)| (id, p.pos))
            .collect();
        for (id, pos) in players {
            let feet = BlockPos::new(
                pos[0].floor() as i32,
                pos[1].floor() as i32,
                pos[2].floor() as i32,
            );
            let in_water = self
                .loaded_block_at(feet)
                .and_then(fluid_at)
                .is_some_and(|f| f.fluid == Fluid::Water);
            for dy in -(MAX_DEPTH + 2)..=0 {
                for dx in -GAS_RADIUS..=GAS_RADIUS {
                    for dz in -GAS_RADIUS..=GAS_RADIUS {
                        let at = BlockPos::new(feet.x + dx, feet.y + dy, feet.z + dz);
                        let Some(state) = self.loaded_block_at(at) else {
                            continue;
                        };
                        if !is_potent_sulfur(state) {
                            continue;
                        }
                        let wanted = self.sulfur_state(at);
                        if prop(state, "potent_sulfur_state") != Some(wanted) {
                            self.set_block(at, with(state, "potent_sulfur_state", wanted));
                        }
                        let Some(depth) = self.sulfur_water_depth(at) else {
                            continue;
                        };
                        let surface = at.y + depth;
                        // Noxious gas: for whoever is in the water over and around it.
                        if in_water && (at.y + 1..=surface).contains(&feet.y) {
                            self.add_effect(
                                id,
                                Effect {
                                    id: mob_effect::NAUSEA,
                                    duration: NAUSEA_TICKS,
                                    amplifier: 0,
                                },
                            );
                        }
                        // A geyser: whoever is in the column above goes up.
                        let erupting = matches!(wanted, "erupting" | "continuous");
                        if erupting
                            && (dx, dz) == (0, 0)
                            && (at.y + 1..=surface + 2).contains(&feet.y)
                        {
                            self.push_player(id, [0.0, geyser_push(depth), 0.0]);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-sulfur-{}-{}",
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

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    #[test]
    fn potent_sulfur_follows_the_water_and_what_is_under_it() {
        let mut w = flat_world();
        let sulfur = BlockPos::new(4, -61, 4);
        w.set_block(sulfur, state("minecraft:potent_sulfur"));
        assert_eq!(w.sulfur_state(sulfur), "dry");
        // Two blocks of still water with air above: the gas works.
        let water = state("minecraft:water");
        w.set_block(offset(sulfur, Dir::Up), water);
        w.set_block(BlockPos::new(4, -59, 4), water);
        assert_eq!(w.sulfur_water_depth(sulfur), Some(2));
        assert_eq!(w.sulfur_state(sulfur), "wet");
        // Lava underneath: a geyser that never stops.
        w.set_block(offset(sulfur, Dir::Down), state("minecraft:lava"));
        assert_eq!(w.sulfur_state(sulfur), "continuous");
        // Magma: it erupts some of the time and rests the rest.
        w.set_block(offset(sulfur, Dir::Down), state("minecraft:magma_block"));
        // 15 to 30 seconds of rest and 1 to 2 of eruption under one block of water,
        // 45 to 60 and 4 to 5 under four.
        let (shallow, shallow_burst) = geyser_rhythm(sulfur, 1);
        assert!((320..=640).contains(&shallow) && (20..=40).contains(&shallow_burst));
        let (deep, deep_burst) = geyser_rhythm(sulfur, 4);
        assert!((980..=1300).contains(&deep) && (80..=100).contains(&deep_burst));
        let depth = w.sulfur_water_depth(sulfur).unwrap();
        let (cycle, _) = geyser_rhythm(sulfur, depth);
        let states: std::collections::HashSet<&str> = (0..cycle)
            .map(|t| {
                w.tick = t;
                w.sulfur_state(sulfur)
            })
            .collect();
        assert_eq!(states, ["erupting", "dormant"].into());
        // Five blocks of water is too deep.
        for y in -58..=-56 {
            w.set_block(BlockPos::new(4, y, 4), water);
        }
        assert_eq!(w.sulfur_water_depth(sulfur), None);
        assert_eq!(w.sulfur_state(sulfur), "wet");
    }
}
