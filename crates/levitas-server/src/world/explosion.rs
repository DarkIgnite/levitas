//! Explosions, as the Minecraft Wiki describes them: rays cast from the centre in every
//! direction lose strength with distance and with the blast resistance of what they pass
//! through, and break the blocks they still have strength for. Entities nearby take
//! damage and are thrown back by how close they are.

use std::collections::HashSet;

use super::*;
use levitas_registry::sound_id;

use crate::blocks::fluid::fluid_at;
use crate::world::survival::Damage;

/// Rays start from the surface of a 16x16x16 cube of directions.
const RAYS_PER_SIDE: i32 = 16;
/// Distance between two samples along a ray, and the strength lost per sample.
const STEP: f64 = 0.3;
const DECAY_PER_STEP: f32 = 0.225;

impl World {
    /// An explosion of `power` at `centre` (a creeper is 3, TNT 4), caused by `by`.
    pub(super) fn explode(&mut self, centre: [f64; 3], power: f32, by: &'static str) {
        let destroyed = self.blast_rays(centre, power);

        // Blocks break; each drops what it would with a chance of 1 in `power`.
        for &pos in &destroyed {
            let state = self.block(pos);
            if self.rng.f64() < 1.0 / f64::from(power) {
                let drops = self.loot.block_drops(&mut LootContext {
                    state,
                    tool: 0,
                    rng: &mut self.rng,
                    entity: Default::default(),
                });
                for (item, count) in drops {
                    self.drop_at_block(pos, Stack::new(item, count));
                }
            }
            self.remove_block_entity(pos);
            self.set_block(pos, block::AIR);
        }
        for &pos in &destroyed {
            self.update_neighbors(pos);
        }

        // Entities within twice the power take damage and are pushed away.
        let reach = f64::from(power) * 2.0;
        let impact = |pos: [f64; 3]| -> Option<(f64, [f64; 3])> {
            let d = [pos[0] - centre[0], pos[1] - centre[1], pos[2] - centre[2]];
            let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if distance > reach {
                return None;
            }
            let strength = 1.0 - distance / reach;
            // Right at the centre there is no direction to be thrown in.
            let push = if distance < 1e-6 {
                [0.0; 3]
            } else {
                d.map(|v| v / distance * strength)
            };
            Some((strength, push))
        };
        let damage_of =
            |strength: f64| ((strength * strength + strength) / 2.0 * 7.0 * reach + 1.0) as f32;

        let mut pushed = Vec::new();
        let hit_players: Vec<(i32, f64, [f64; 3])> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive)
            .filter_map(|(&id, p)| impact(p.pos).map(|(s, push)| (id, s, push)))
            .collect();
        for (id, strength, push) in hit_players {
            self.damage(id, damage_of(strength), Damage::Explosion(by));
            pushed.push((id, push));
        }
        let hit_mobs: Vec<(i32, f64, [f64; 3])> = self
            .mobs
            .iter()
            .filter(|(_, m)| m.alive())
            .filter_map(|(&id, m)| impact(m.pos).map(|(s, push)| (id, s, push)))
            .collect();
        for (id, strength, push) in hit_mobs {
            let Some(mut mob) = self.mobs.remove(&id) else {
                continue;
            };
            mob.push(push);
            self.hurt_mob(id, &mut mob, damage_of(strength), "minecraft:explosion");
            self.mobs.insert(id, mob);
        }

        // Everyone near sees and hears it; players caught in it are thrown back.
        let particle = levitas_registry::particle::EXPLOSION_EMITTER;
        let sound = sound_id("minecraft:entity.generic.explode").map_or(0, |s| s + 1);
        let viewers: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| {
                let d: f64 = (0..3).map(|i| (p.pos[i] - centre[i]).powi(2)).sum();
                d.sqrt() < 64.0
            })
            .map(|(&id, _)| id)
            .collect();
        for id in viewers {
            let mut packet = Writer::packet(clientbound::EXPLODE)
                .f64(centre[0])
                .f64(centre[1])
                .f64(centre[2]);
            packet = match pushed.iter().find(|(p, _)| *p == id) {
                Some((_, push)) => packet.bool(true).f64(push[0]).f64(push[1]).f64(push[2]),
                None => packet.bool(false),
            };
            self.send(id, packet.varint(particle).varint(sound));
        }
    }

    /// The blocks an explosion breaks.
    fn blast_rays(&mut self, centre: [f64; 3], power: f32) -> HashSet<BlockPos> {
        let mut destroyed = HashSet::new();
        let last = RAYS_PER_SIDE - 1;
        for i in 0..RAYS_PER_SIDE {
            for j in 0..RAYS_PER_SIDE {
                for k in 0..RAYS_PER_SIDE {
                    let on_surface = [i, j, k].iter().any(|&v| v == 0 || v == last);
                    if !on_surface {
                        continue;
                    }
                    let mut dir = [i, j, k].map(|v| f64::from(v) / f64::from(last) * 2.0 - 1.0);
                    let length = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
                    dir = dir.map(|v| v / length);
                    let mut strength = power * (0.7 + self.rng.f64() as f32 * 0.6);
                    let mut at = centre;
                    while strength > 0.0 {
                        let pos = BlockPos::new(
                            at[0].floor() as i32,
                            at[1].floor() as i32,
                            at[2].floor() as i32,
                        );
                        if !(MIN_Y..=MAX_Y).contains(&pos.y)
                            || !self.chunks.contains_key(&pos.chunk())
                        {
                            break;
                        }
                        let state = self.block(pos);
                        if !is_air(state) {
                            let resistance = BlockInfo::of(state).resistance;
                            strength -= (resistance + 0.3) * STEP as f32;
                            if strength > 0.0 && fluid_at(state).is_none() {
                                destroyed.insert(pos);
                            }
                        }
                        at = [
                            at[0] + dir[0] * STEP,
                            at[1] + dir[1] * STEP,
                            at[2] + dir[2] * STEP,
                        ];
                        strength -= DECAY_PER_STEP;
                    }
                }
            }
        }
        // Unbreakable blocks (bedrock and the like) resist any explosion.
        destroyed.retain(|&pos| BlockInfo::of(self.block(pos)).hardness >= 0.0);
        destroyed
    }
}
