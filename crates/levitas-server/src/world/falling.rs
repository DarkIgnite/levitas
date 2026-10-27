//! Falling block entities: gravity, movement, concrete hardening, anvil degradation,
//! stalactite damage and landing.

use levitas_protocol::{BlockPos, Writer};
use levitas_registry::packet::play::clientbound;
use levitas_registry::{block, block_has_tag, collision_boxes, entity_type, BlockInfo, ItemInfo};
use levitas_world::{is_air, MAX_Y, MIN_Y};

use super::*;
use crate::blocks::fluid::{self, Fluid};
use crate::blocks::{self, concrete_from_powder, concrete_hardens, is_stalactite, Dir};
use crate::loot::LootContext;
use crate::world::survival::Damage;

pub const LEVEL_EVENT_ANVIL_DESTROY: i32 = 1029;
pub const LEVEL_EVENT_ANVIL_LAND: i32 = 1031;
pub const LEVEL_EVENT_DRIPSTONE_LAND: i32 = 1045;
pub const LEVEL_EVENT_BLOCK_BREAK: i32 = 2001;

pub struct FallingBlock {
    pub state: u16,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub fall_distance: f64,
    pub age: u32,
}

pub fn spawn_packet(id: i32, falling: &FallingBlock) -> Writer {
    let vel = |v: f64| (v.clamp(-3.9, 3.9) * 8000.0) as i16;
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::FALLING_BLOCK)
        .f64(falling.pos[0])
        .f64(falling.pos[1])
        .f64(falling.pos[2])
        .u8(0) // pitch
        .u8(0) // yaw
        .u8(0) // head yaw
        .varint(falling.state as i32) // entity data: block state ID
        .i16(vel(falling.vel[0]))
        .i16(vel(falling.vel[1]))
        .i16(vel(falling.vel[2]))
}

fn next_anvil_state(state: u16) -> Option<u16> {
    let n = blocks::name(state);
    let facing = blocks::facing(state).unwrap_or(Dir::North);
    let next_name = match n {
        "minecraft:anvil" => "minecraft:chipped_anvil",
        "minecraft:chipped_anvil" => "minecraft:damaged_anvil",
        _ => return None,
    };
    let info = BlockInfo::by_name(next_name)?;
    Some(blocks::with(info.default_state, "facing", facing.name()))
}

impl World {
    fn degrade_anvil(&mut self, state: u16, fall_distance: f64) -> Option<u16> {
        let chance = (0.05 * fall_distance).min(1.0);
        if self.rng.f64() < chance {
            next_anvil_state(state)
        } else {
            Some(state)
        }
    }

    pub(super) fn damage_players_in_column(&mut self, pos: BlockPos, damage: f32, kind: Damage) {
        let hit: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| {
                p.alive
                    && p.survival()
                    && (p.pos[0] - (f64::from(pos.x) + 0.5)).abs() < 0.8
                    && (p.pos[2] - (f64::from(pos.z) + 0.5)).abs() < 0.8
                    && p.pos[1] >= f64::from(pos.y - 1)
                    && p.pos[1] <= f64::from(pos.y + 2)
            })
            .map(|(&id, _)| id)
            .collect();
        for id in hit {
            self.damage(id, damage, kind);
        }
    }

    pub(super) fn spawn_falling_block(&mut self, pos: BlockPos, state: u16) {
        let id = next_entity_id();
        let falling = FallingBlock {
            state,
            pos: [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y),
                f64::from(pos.z) + 0.5,
            ],
            vel: [0.0, 0.0, 0.0],
            fall_distance: 0.0,
            age: 0,
        };
        self.broadcast(spawn_packet(id, &falling), None);
        self.falling_blocks.insert(id, falling);
        self.set_block_updating(pos, block::AIR);
    }

    pub(super) fn land_falling_block(
        &mut self,
        falling: FallingBlock,
        landing_pos: BlockPos,
        in_water: bool,
    ) {
        if in_water {
            let concrete = concrete_from_powder(falling.state).unwrap_or(falling.state);
            self.set_block_updating(landing_pos, concrete);
            self.play_sound("minecraft:block.sand.fall", landing_pos, None);
            return;
        }

        if is_stalactite(falling.state) {
            self.broadcast(
                Writer::packet(clientbound::LEVEL_EVENT)
                    .i32(LEVEL_EVENT_DRIPSTONE_LAND)
                    .position(landing_pos)
                    .i32(0)
                    .bool(false),
                None,
            );
            let damage = (falling.fall_distance * 1.0).min(40.0) as f32;
            if damage > 0.0 {
                self.damage_players_in_column(landing_pos, damage, Damage::FallingStalactite);
            }
            if let Some(item) = ItemInfo::id_of("minecraft:pointed_dripstone") {
                self.drop_at_block(landing_pos, Stack::new(item, 1));
            }
            return;
        }

        if matches!(
            blocks::name(falling.state),
            "minecraft:suspicious_sand" | "minecraft:suspicious_gravel"
        ) {
            self.broadcast(
                Writer::packet(clientbound::LEVEL_EVENT)
                    .i32(LEVEL_EVENT_BLOCK_BREAK)
                    .position(landing_pos)
                    .i32(i32::from(falling.state))
                    .bool(false),
                None,
            );
            return;
        }

        if block_has_tag(falling.state, "minecraft:anvil") {
            if falling.fall_distance > 1.0 {
                let damage = ((falling.fall_distance - 1.0) * 2.0).min(40.0) as f32;
                self.damage_players_in_column(landing_pos, damage, Damage::FallingAnvil);
            }
            let old = self.block(landing_pos);
            if !is_air(old) && collision_boxes(old).is_empty() {
                self.pop_block(landing_pos, old);
            }
            if !collision_boxes(self.block(landing_pos)).is_empty() {
                let drops = self.loot.block_drops(&mut LootContext {
                    state: falling.state,
                    tool: 0,
                    rng: &mut self.rng,
                    entity: Default::default(),
                });
                for (item, count) in drops {
                    self.drop_at_block(landing_pos, Stack::new(item, count));
                }
                return;
            }
            if let Some(new_state) = self.degrade_anvil(falling.state, falling.fall_distance) {
                self.broadcast(
                    Writer::packet(clientbound::LEVEL_EVENT)
                        .i32(LEVEL_EVENT_ANVIL_LAND)
                        .position(landing_pos)
                        .i32(0)
                        .bool(false),
                    None,
                );
                self.set_block_updating(landing_pos, new_state);
            } else {
                self.broadcast(
                    Writer::packet(clientbound::LEVEL_EVENT)
                        .i32(LEVEL_EVENT_ANVIL_DESTROY)
                        .position(landing_pos)
                        .i32(0)
                        .bool(false),
                    None,
                );
            }
            return;
        }

        // General gravity block (sand, gravel, concrete powder, dragon egg)
        let old = self.block(landing_pos);
        if blocks::can_fall_through(old) {
            if !is_air(old) && fluid::fluid_at(old).is_none() {
                self.pop_block(landing_pos, old);
            }
            let placed_state = concrete_hardens(falling.state, landing_pos, &mut |q| self.block(q));
            self.set_block_updating(landing_pos, placed_state);
            let sound = if blocks::name(placed_state).contains("gravel") {
                "minecraft:block.gravel.fall"
            } else {
                "minecraft:block.sand.fall"
            };
            self.play_sound(sound, landing_pos, None);
        } else {
            let drops = self.loot.block_drops(&mut LootContext {
                state: falling.state,
                tool: 0,
                rng: &mut self.rng,
                entity: Default::default(),
            });
            for (item, count) in drops {
                self.drop_at_block(landing_pos, Stack::new(item, count));
            }
            self.broadcast(
                Writer::packet(clientbound::LEVEL_EVENT)
                    .i32(LEVEL_EVENT_BLOCK_BREAK)
                    .position(landing_pos)
                    .i32(i32::from(falling.state))
                    .bool(false),
                None,
            );
        }
    }

    pub(super) fn tick_falling_blocks(&mut self) {
        let mut ids: Vec<i32> = self.falling_blocks.keys().copied().collect();
        ids.sort_by(|&a, &b| {
            self.falling_blocks[&a].pos[1].total_cmp(&self.falling_blocks[&b].pos[1])
        });

        for id in ids {
            let mut falling = match self.falling_blocks.remove(&id) {
                Some(f) => f,
                None => continue,
            };
            falling.age += 1;

            if falling.age > 600 || falling.pos[1] < f64::from(MIN_Y - 64) {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }

            let cell = BlockPos::new(
                falling.pos[0].floor() as i32,
                falling.pos[1].floor() as i32,
                falling.pos[2].floor() as i32,
            );
            if !self.chunks.contains_key(&cell.chunk()) {
                self.falling_blocks.insert(id, falling);
                continue;
            }

            falling.vel[1] -= 0.04;
            falling.vel[0] *= 0.98;
            falling.vel[1] *= 0.98;
            falling.vel[2] *= 0.98;

            let prev_y = falling.pos[1];
            let next_y = prev_y + falling.vel[1];
            falling.fall_distance += (prev_y - next_y).max(0.0);

            let x = falling.pos[0].floor() as i32;
            let z = falling.pos[2].floor() as i32;
            let start_y = (prev_y.floor() as i32).min(MAX_Y);
            let end_y = (next_y.floor() as i32).max(MIN_Y);

            let mut landed = None;
            for check_y in (end_y..=start_y).rev() {
                let check_pos = BlockPos::new(x, check_y, z);
                let state = self.block(check_pos);

                // Concrete powder solidifies as soon as it enters water.
                if block_has_tag(falling.state, "minecraft:concrete_powder")
                    && fluid::fluid_at(state).is_some_and(|f| f.fluid == Fluid::Water)
                {
                    landed = Some((check_pos, true));
                    break;
                }

                let boxes = collision_boxes(state);
                if !boxes.is_empty() {
                    let max_top = boxes.iter().map(|b| f64::from(b[4])).fold(0.0, f64::max);
                    let surface_y = f64::from(check_y) + max_top;
                    if next_y <= surface_y {
                        let is_full_cube = (max_top - 1.0).abs() < 1e-4;
                        let landing_pos = if is_full_cube {
                            BlockPos::new(x, check_y + 1, z)
                        } else {
                            BlockPos::new(x, check_y, z)
                        };
                        landed = Some((landing_pos, false));
                        break;
                    }
                }
            }

            if let Some((landing_pos, in_water)) = landed {
                self.broadcast(items::remove_entity(id), None);
                self.land_falling_block(falling, landing_pos, in_water);
            } else {
                falling.pos[1] = next_y;
                self.falling_blocks.insert(id, falling);
            }
        }
    }
}
