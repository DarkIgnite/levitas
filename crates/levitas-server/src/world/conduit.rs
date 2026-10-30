//! Conduits, following the Minecraft Wiki: built underwater inside a frame of 16 to 42
//! prismarine or sea lantern blocks. When active, grants the Conduit Power status effect
//! (underwater breathing, night vision, increased mining speed) to players in water or
//! rain within range, and attacks nearby hostile mobs when framed by a full 42 blocks.

use std::collections::HashSet;

use levitas_nbt::{Compound, Tag};
use levitas_registry::packet::play::clientbound;
use levitas_registry::{block, block_entity_type, mob_effect, BlockInfo, Effect};

use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};

pub const CONDUIT_PULSE_TICKS: u8 = 80;
pub const CONDUIT_ATTACK_TICKS: u8 = 40;
pub const CONDUIT_MIN_FRAME: u8 = 16;
pub const CONDUIT_MAX_FRAME: u8 = 42;

#[derive(Debug, Clone, Default)]
pub struct Conduit {
    pub active: bool,
    pub frame_count: u8,
    pub target_mob: Option<i32>,
    pub viewers: HashSet<i32>,
    pub tick_timer: u8,
}

impl Conduit {
    pub fn write_nbt(&self, nbt: &mut Compound) {
        nbt.insert("active", if self.active { 1i8 } else { 0i8 });
    }

    pub fn read_nbt(&mut self, nbt: &Compound) {
        self.active = nbt.int("active") != Some(0);
    }

    pub fn network_data(&self) -> Compound {
        let mut nbt = Compound::default();
        nbt.insert("active", if self.active { 1i8 } else { 0i8 });
        nbt
    }

    pub fn type_id(&self) -> i32 {
        block_entity_type::CONDUIT
    }
}

/// Checks whether a block contains water: either a water block, or a waterlogged block.
fn is_water_block(world: &World, pos: BlockPos) -> bool {
    let state = world.loaded_block(pos).unwrap_or(block::AIR);
    crate::blocks::prop(state, "waterlogged") == Some("true")
        || fluid_at(state).is_some_and(|f| f.fluid == Fluid::Water)
}

/// Checks whether a block is valid for a conduit frame: prismarine, prismarine bricks,
/// dark prismarine, or sea lantern.
fn is_frame_block(world: &World, pos: BlockPos) -> bool {
    let state = world.loaded_block(pos).unwrap_or(block::AIR);
    let name = BlockInfo::of(state).name;
    matches!(
        name,
        "minecraft:prismarine"
            | "minecraft:prismarine_bricks"
            | "minecraft:dark_prismarine"
            | "minecraft:sea_lantern"
    )
}

/// Checks if a conduit is in a valid underwater environment and counts frame blocks.
/// Returns `(is_active, frame_block_count)`.
pub fn check_conduit_frame(world: &World, pos: BlockPos) -> (bool, u8) {
    // 1. The conduit itself must be in water.
    if !is_water_block(world, pos) {
        return (false, 0);
    }

    // 2. All 26 surrounding blocks in the 3x3x3 area around the conduit must be water.
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let p = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                if !is_water_block(world, p) {
                    return (false, 0);
                }
            }
        }
    }

    // 3. Count frame blocks across the 3 intersecting 5x5 rings (42 blocks total).
    let mut frame_count = 0u8;
    for dx in -2..=2i32 {
        for dy in -2..=2i32 {
            for dz in -2..=2i32 {
                // Must be on one of the three 5x5 planes (XY, XZ, YZ) at radius 2:
                let on_frame = (dz == 0 && (dx.abs() == 2 || dy.abs() == 2))
                    || (dy == 0 && (dx.abs() == 2 || dz.abs() == 2))
                    || (dx == 0 && (dy.abs() == 2 || dz.abs() == 2));
                if on_frame
                    && is_frame_block(world, BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz))
                {
                    frame_count += 1;
                }
            }
        }
    }

    (frame_count >= CONDUIT_MIN_FRAME, frame_count)
}

impl World {
    /// Ticks conduits: applies Conduit Power to nearby submerged/raining players every 80 ticks,
    /// and attacks hostile mobs every 40 ticks when fully framed (42 blocks).
    pub(super) fn tick_conduits(&mut self) {
        let conduit_positions: Vec<BlockPos> = self
            .block_entities
            .iter()
            .filter_map(|(&pos, entity)| match entity {
                BlockEntity::Conduit(_) => Some(pos),
                _ => None,
            })
            .collect();

        for pos in conduit_positions {
            let mut tick_timer = 0;
            if let Some(BlockEntity::Conduit(c)) = self.block_entities.get_mut(&pos) {
                c.tick_timer = (c.tick_timer + 1) % CONDUIT_PULSE_TICKS;
                tick_timer = c.tick_timer;
            }

            // Every 80 ticks (4 seconds): check activation and pulse Conduit Power.
            if tick_timer == 0 {
                let (active, frame_count) = check_conduit_frame(self, pos);
                let mut state_changed = false;
                let mut became_active = false;
                let mut became_inactive = false;

                if let Some(BlockEntity::Conduit(c)) = self.block_entities.get_mut(&pos) {
                    if c.active != active || c.frame_count != frame_count {
                        state_changed = true;
                        became_active = !c.active && active;
                        became_inactive = c.active && !active;
                        c.active = active;
                        c.frame_count = frame_count;
                    }
                }

                if state_changed {
                    self.send_conduit_update(pos);
                    self.dirty.insert(pos.chunk());
                }
                if became_active {
                    self.play_sound("minecraft:block.conduit.activate", pos, None);
                } else if became_inactive {
                    self.play_sound("minecraft:block.conduit.deactivate", pos, None);
                }

                if active {
                    self.play_sound("minecraft:block.conduit.ambient", pos, None);

                    // Range is 16 blocks per 7 frame blocks (32 to 96 blocks spherical).
                    let range = ((16 * (frame_count as usize / 7)) as f64).min(96.0);
                    let range_sq = range * range;
                    let center = [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5];

                    let near_players: Vec<i32> = self
                        .players
                        .iter()
                        .filter(|(_, p)| p.alive)
                        .filter_map(|(&id, p)| {
                            let dx = p.pos[0] - center[0];
                            let dy = p.pos[1] - center[1];
                            let dz = p.pos[2] - center[2];
                            if dx * dx + dy * dy + dz * dz > range_sq {
                                return None;
                            }
                            // Must be in water or in rain.
                            let eye = p.pos[1] + if p.sneaking { 1.27 } else { 1.62 };
                            let at = |y: f64| {
                                BlockPos::new(
                                    p.pos[0].floor() as i32,
                                    y.floor() as i32,
                                    p.pos[2].floor() as i32,
                                )
                            };
                            let head = self.loaded_block(at(eye)).unwrap_or(block::AIR);
                            let feet = self.loaded_block(at(p.pos[1])).unwrap_or(block::AIR);
                            let in_water = fluid_at(head).is_some_and(|f| f.fluid == Fluid::Water)
                                || fluid_at(feet).is_some_and(|f| f.fluid == Fluid::Water);
                            let in_rain = self.weather.is_raining()
                                && self.light_at(at(eye)).is_some_and(|(sky, _)| sky == 15);
                            if in_water || in_rain {
                                Some(id)
                            } else {
                                None
                            }
                        })
                        .collect();

                    for pid in near_players {
                        self.add_effect(
                            pid,
                            Effect {
                                id: mob_effect::CONDUIT_POWER,
                                amplifier: 0,
                                duration: 260, // 13 seconds
                            },
                        );
                    }
                }
            }

            // Every 40 ticks (2 seconds): if fully framed (42 blocks), attack a nearby hostile mob in water.
            if tick_timer % CONDUIT_ATTACK_TICKS == 0 {
                let mut target_candidate: Option<i32> = None;
                if let Some(BlockEntity::Conduit(c)) = self.block_entities.get(&pos) {
                    if c.active && c.frame_count >= CONDUIT_MAX_FRAME {
                        let center = [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5];
                        let attack_range_sq = 8.0 * 8.0;

                        target_candidate = self
                            .mobs
                            .iter()
                            .filter(|(_, m)| m.alive() && m.kind.is_monster())
                            .find_map(|(&id, m)| {
                                let dx = m.pos[0] - center[0];
                                let dy = m.pos[1] - center[1];
                                let dz = m.pos[2] - center[2];
                                if dx * dx + dy * dy + dz * dz <= attack_range_sq {
                                    let block_pos = BlockPos::new(
                                        m.pos[0].floor() as i32,
                                        m.pos[1].floor() as i32,
                                        m.pos[2].floor() as i32,
                                    );
                                    let state = self.loaded_block(block_pos).unwrap_or(block::AIR);
                                    let in_water = fluid_at(state)
                                        .is_some_and(|f| f.fluid == Fluid::Water)
                                        || crate::blocks::prop(state, "waterlogged")
                                            == Some("true");
                                    let in_rain = self.weather.is_raining()
                                        && self
                                            .light_at(block_pos)
                                            .is_some_and(|(sky, _)| sky == 15);
                                    if in_water || in_rain {
                                        Some(id)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            });
                    }
                }

                if let Some(mob_id) = target_candidate {
                    if let Some(BlockEntity::Conduit(c)) = self.block_entities.get_mut(&pos) {
                        c.target_mob = Some(mob_id);
                    }
                    self.hit_mob(mob_id, 4.0, None, [0.0, 0.0], "minecraft:magic");
                    self.play_sound("minecraft:block.conduit.attack.target", pos, None);
                } else if let Some(BlockEntity::Conduit(c)) = self.block_entities.get_mut(&pos) {
                    c.target_mob = None;
                }
            }
        }
    }

    /// Broadcasts the conduit block entity data to clients.
    pub(super) fn send_conduit_update(&mut self, pos: BlockPos) {
        let Some(BlockEntity::Conduit(c)) = self.block_entities.get(&pos) else {
            return;
        };
        let data = levitas_nbt::to_network(&Tag::Compound(c.network_data()));
        self.broadcast(
            Writer::packet(clientbound::BLOCK_ENTITY_DATA)
                .position(pos)
                .varint(block_entity_type::CONDUIT)
                .bytes(&data),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;
    use crate::world::mobs::{Mob, MobKind};

    #[test]
    fn conduit_activation_and_frame_counts() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-conduit-frame-{}-{}",
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

        let pos = BlockPos::new(0, -40, 0);
        let water = BlockInfo::by_name("minecraft:water").unwrap().default_state;
        let conduit = BlockInfo::by_name("minecraft:conduit")
            .unwrap()
            .default_state;

        // Place 3x3x3 water area.
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    world.set_block(BlockPos::new(dx, -40 + dy, dz), water);
                }
            }
        }
        world.set_block(pos, conduit);

        // Without frame blocks, inactive.
        let (active, count) = check_conduit_frame(&world, pos);
        assert!(!active);
        assert_eq!(count, 0);

        // Build one ring on the XY plane (dz = 0, dx/dy at radius 2) -> 16 blocks.
        let prismarine = BlockInfo::by_name("minecraft:prismarine")
            .unwrap()
            .default_state;
        for dx in -2..=2i32 {
            for dy in -2..=2i32 {
                if dx.abs() == 2 || dy.abs() == 2 {
                    world.set_block(BlockPos::new(dx, -40 + dy, 0), prismarine);
                }
            }
        }
        let (active, count) = check_conduit_frame(&world, pos);
        assert!(active);
        assert_eq!(count, 16);

        // Build full 42 frame blocks across all 3 rings.
        for dx in -2..=2i32 {
            for dy in -2..=2i32 {
                for dz in -2..=2i32 {
                    let on_frame = (dz == 0 && (dx.abs() == 2 || dy.abs() == 2))
                        || (dy == 0 && (dx.abs() == 2 || dz.abs() == 2))
                        || (dx == 0 && (dy.abs() == 2 || dz.abs() == 2));
                    if on_frame {
                        world.set_block(BlockPos::new(dx, -40 + dy, dz), prismarine);
                    }
                }
            }
        }
        let (active, count) = check_conduit_frame(&world, pos);
        assert!(active);
        assert_eq!(count, 42);

        // If one water block around conduit is replaced with stone, becomes inactive.
        let stone = BlockInfo::by_name("minecraft:stone").unwrap().default_state;
        world.set_block(BlockPos::new(1, -40, 0), stone);
        let (active, _) = check_conduit_frame(&world, pos);
        assert!(!active);
    }

    #[test]
    fn conduit_nbt_round_trip() {
        let c = Conduit {
            active: true,
            frame_count: 42,
            target_mob: Some(123),
            viewers: HashSet::new(),
            tick_timer: 0,
        };

        let mut nbt = Compound::default();
        c.write_nbt(&mut nbt);
        assert_eq!(nbt.int("active"), Some(1));

        let mut restored = Conduit::default();
        restored.read_nbt(&nbt);
        assert!(restored.active);

        let net = restored.network_data();
        assert_eq!(net.int("active"), Some(1));
    }

    #[test]
    fn conduit_power_and_mob_attack() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-conduit-power-{}-{}",
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

        let (out, _frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let p = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        // Alice is submerged at (3.0, -40.0, 0.0) in water.
        world
            .players
            .insert(id, Player::new(p, out, [3.0, -40.0, 0.0]));

        let pos = BlockPos::new(0, -40, 0);
        let water = BlockInfo::by_name("minecraft:water").unwrap().default_state;
        let conduit = BlockInfo::by_name("minecraft:conduit")
            .unwrap()
            .default_state;
        let prismarine = BlockInfo::by_name("minecraft:prismarine")
            .unwrap()
            .default_state;

        // Surround with water.
        for dx in -3..=3 {
            for dy in -3..=3 {
                for dz in -3..=3 {
                    world.set_block(BlockPos::new(dx, -40 + dy, dz), water);
                }
            }
        }
        world.set_block(pos, conduit);

        // Build full 42 frame blocks.
        for dx in -2..=2i32 {
            for dy in -2..=2i32 {
                for dz in -2..=2i32 {
                    let on_frame = (dz == 0 && (dx.abs() == 2 || dy.abs() == 2))
                        || (dy == 0 && (dx.abs() == 2 || dz.abs() == 2))
                        || (dx == 0 && (dy.abs() == 2 || dz.abs() == 2));
                    if on_frame {
                        world.set_block(BlockPos::new(dx, -40 + dy, dz), prismarine);
                    }
                }
            }
        }

        world.block_entities.insert(
            pos,
            BlockEntity::Conduit(Box::new(Conduit {
                active: false,
                frame_count: 0,
                target_mob: None,
                viewers: HashSet::new(),
                tick_timer: CONDUIT_PULSE_TICKS - 1,
            })),
        );

        // Spawn a zombie in water near the conduit (at [1.0, -40.0, 0.0]).
        let zombie_id = next_entity_id();
        let mut rng = crate::rng::Rng::new(12345);
        let zombie = Mob::new(MobKind::Zombie, [1.0, -40.0, 0.0], &mut rng);
        let initial_health = zombie.health;
        assert_eq!(initial_health, 20.0);
        world.mobs.insert(zombie_id, zombie);

        // Tick conduits: activation + pulse Conduit Power to Alice + attack hostile mob.
        world.tick_conduits();

        // Alice receives Conduit Power!
        let effect = world.players[&id].effect(mob_effect::CONDUIT_POWER);
        assert!(effect.is_some());
        assert_eq!(effect.unwrap().duration, 260);

        // Zombie took 4 magic damage from complete conduit frame.
        let zombie_health = world.mobs[&zombie_id].health;
        assert_eq!(zombie_health, 16.0);
    }
}
