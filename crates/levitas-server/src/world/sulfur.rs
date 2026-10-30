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
/// A magma-fed geyser erupts for `ERUPT_TICKS` of every `GEYSER_CYCLE` ticks.
const GEYSER_CYCLE: u64 = 200;
const ERUPT_TICKS: u64 = 60;
/// The upward push of an erupting geyser, in blocks per tick.
const GEYSER_PUSH: f64 = 1.2;

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
        if self.sulfur_water_depth(pos).is_none() {
            return "wet";
        }
        let below = self
            .loaded_block_at(offset(pos, Dir::Down))
            .unwrap_or(block::AIR);
        if fluid_at(below).is_some_and(|f| f.fluid == Fluid::Lava) {
            return "continuous";
        }
        if blocks::name(below) == "minecraft:magma_block" {
            // Each geyser keeps its own rhythm.
            let phase = (self.tick + (pos.x.wrapping_mul(31) ^ pos.z) as u64) % GEYSER_CYCLE;
            return if phase < ERUPT_TICKS {
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
                            self.push_player(id, [0.0, GEYSER_PUSH, 0.0]);
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
        let states: std::collections::HashSet<&str> = (0..GEYSER_CYCLE)
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
