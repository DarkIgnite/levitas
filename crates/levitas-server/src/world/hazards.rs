//! Dangers of the world around players: drowning, lava, fire and magma, and burning.
//!
//! Behaviour follows the Minecraft Wiki: a player holds 300 ticks of air under water and
//! then takes 2 damage a second; lava deals 4 and sets players alight for 15 seconds;
//! fire blocks deal 1 and set them alight for 8; magma blocks hurt players who walk on
//! them without sneaking; burning deals 1 each second until it runs out or water or rain
//! puts it out. Hits from blocks come at most every half second.

use super::*;
use crate::blocks::{
    self,
    fluid::{fluid_at, Fluid},
};
use crate::world::survival::Damage;

const MAX_AIR: i32 = 300;
/// Air regained each tick out of water.
const AIR_REGAIN: i32 = 4;
/// Ticks between hits from lava, fire and magma.
const BLOCK_HIT_TICKS: u32 = 10;
const LAVA_FIRE_TICKS: i32 = 300;
const FIRE_BLOCK_TICKS: i32 = 160;

const DATA_FLAGS: u8 = 0;
const DATA_AIR: u8 = 1;
const SERIALIZER_BYTE: i32 = 0;
const SERIALIZER_INT: i32 = 1;
const FLAG_ON_FIRE: u8 = 0x01;
const FLAG_SNEAKING: u8 = 0x02;
const FLAG_SPRINTING: u8 = 0x08;

impl World {
    pub(super) fn tick_hazards(&mut self) {
        let ids: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.player_hazards(id);
        }
    }

    fn player_hazards(&mut self, id: i32) {
        let p = &self.players[&id];
        let (pos, sneaking, survival) = (p.pos, p.sneaking, p.survival());
        let eye = pos[1] + if sneaking { 1.27 } else { 1.62 };
        let at = |y: f64| {
            BlockPos::new(
                pos[0].floor() as i32,
                y.floor() as i32,
                pos[2].floor() as i32,
            )
        };
        let state = |w: &World, y: f64| w.loaded_block(at(y)).unwrap_or(block::AIR);
        let (feet, head, below) = (
            state(self, pos[1]),
            state(self, eye),
            state(self, pos[1] - 0.2),
        );
        let fluid = |s: u16| fluid_at(s).map(|f| f.fluid);
        let underwater = fluid(head) == Some(Fluid::Water);
        let in_water = underwater || fluid(feet) == Some(Fluid::Water);
        let in_lava = fluid(feet) == Some(Fluid::Lava) || fluid(head) == Some(Fluid::Lava);
        let fire_block = matches!(blocks::name(feet), "minecraft:fire" | "minecraft:soul_fire");
        let on_magma = blocks::name(below) == "minecraft:magma_block" && !sneaking;
        let rained_on =
            self.weather.is_raining() && self.light_at(at(eye)).is_some_and(|(sky, _)| sky == 15);

        let mut hits = Vec::new();
        let p = self.players.get_mut(&id).expect("listed");
        let was_burning = p.fire_ticks > 0;
        let air_before = p.air;
        p.hazard_cooldown = p.hazard_cooldown.saturating_sub(1);
        p.hurt_ticks = p.hurt_ticks.saturating_sub(1);

        // Air: used up under water, then drowning; refilled quickly above it.
        let breathes_water = p
            .effect(levitas_registry::mob_effect::WATER_BREATHING)
            .is_some();
        if underwater && survival && !breathes_water {
            p.air -= 1;
            if p.air <= -20 {
                p.air = 0;
                hits.push((2.0, Damage::Drown));
            }
        } else {
            p.air = (p.air + AIR_REGAIN).min(MAX_AIR);
        }

        if survival {
            let block_hit = p.hazard_cooldown == 0;
            if in_lava {
                p.fire_ticks = p.fire_ticks.max(LAVA_FIRE_TICKS);
                if block_hit {
                    hits.push((4.0, Damage::Lava));
                }
            } else if fire_block {
                p.fire_ticks = p.fire_ticks.max(FIRE_BLOCK_TICKS);
                if block_hit {
                    let soul = blocks::name(feet) == "minecraft:soul_fire";
                    hits.push((if soul { 2.0 } else { 1.0 }, Damage::InFire));
                }
            } else if on_magma && block_hit {
                hits.push((1.0, Damage::HotFloor));
            }
            if hits.iter().any(|(_, k)| !matches!(k, Damage::Drown)) {
                p.hazard_cooldown = BLOCK_HIT_TICKS;
            }
        }

        // Burning: water and rain put it out; otherwise it hurts once a second.
        if in_water || rained_on || !survival {
            p.fire_ticks = 0;
        } else if p.fire_ticks > 0 {
            p.fire_ticks -= 1;
            if p.fire_ticks % 20 == 0 && !in_lava {
                hits.push((1.0, Damage::OnFire));
            }
        }

        let air_changed = p.air != air_before;
        let burning_changed = was_burning != (p.fire_ticks > 0);
        let air = p.air;
        let flags = self.player_flags(id);
        if air_changed {
            self.send(
                id,
                Writer::packet(clientbound::SET_ENTITY_DATA)
                    .varint(id)
                    .u8(DATA_AIR)
                    .varint(SERIALIZER_INT)
                    .varint(air)
                    .u8(0xFF),
            );
        }
        if burning_changed {
            self.broadcast(
                Writer::packet(clientbound::SET_ENTITY_DATA)
                    .varint(id)
                    .u8(DATA_FLAGS)
                    .varint(SERIALIZER_BYTE)
                    .u8(flags)
                    .u8(0xFF),
                None,
            );
        }
        for (amount, kind) in hits {
            self.damage(id, amount, kind);
        }
    }

    /// A player's entity flags: burning, sneaking and sprinting.
    fn player_flags(&self, id: i32) -> u8 {
        let p = &self.players[&id];
        let mut flags = 0;
        if p.fire_ticks > 0 {
            flags |= FLAG_ON_FIRE;
        }
        if p.sneaking {
            flags |= FLAG_SNEAKING;
        }
        if p.sprinting {
            flags |= FLAG_SPRINTING;
        }
        flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    /// A flat world with one survival player standing at (0.5, -60, 0.5).
    fn world_with_player() -> (World, i32, mpsc::Receiver<Frame>) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-hazards-{}-{}",
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
        let (out, frames) = mpsc::channel(100_000);
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
        };
        let id = next_entity_id();
        world
            .players
            .insert(id, Player::new(profile, out, [0.5, -60.0, 0.5]));
        (world, id, frames)
    }

    #[test]
    fn players_drown_after_their_air_runs_out_and_breathe_again_above_water() {
        let (mut world, id, _frames) = world_with_player();
        for y in [-60, -59] {
            world.set_block(BlockPos::new(0, y, 0), block::WATER);
        }
        for _ in 0..300 {
            world.player_hazards(id);
        }
        assert!(world.players[&id].air <= 0);
        assert_eq!(world.players[&id].health, 20.0, "no damage while air lasts");
        for _ in 0..40 {
            world.player_hazards(id);
        }
        assert!(world.players[&id].health < 20.0, "drowning hurts");

        for y in [-60, -59] {
            world.set_block(BlockPos::new(0, y, 0), block::AIR);
        }
        for _ in 0..100 {
            world.player_hazards(id);
        }
        assert_eq!(world.players[&id].air, MAX_AIR);
    }

    #[test]
    fn fire_burns_on_and_water_puts_it_out() {
        let (mut world, id, _frames) = world_with_player();
        world.players.get_mut(&id).unwrap().fire_ticks = 100;
        for _ in 0..40 {
            world.player_hazards(id);
        }
        assert!(world.players[&id].health < 20.0, "burning hurts");
        world.set_block(BlockPos::new(0, -60, 0), block::WATER);
        world.player_hazards(id);
        assert_eq!(world.players[&id].fire_ticks, 0);
    }
}
