//! Fireballs: ghasts' large ones and blazes' small ones, following the Minecraft Wiki.
//!
//! Both fly straight, speeding up as they go (clients work the same flight out
//! themselves). A large fireball explodes with power 1 and starts fires where it hits;
//! hitting it sends it back the way the player looks. A small one sets what it hits
//! alight, or starts a fire on the block.

use levitas_registry::entity_type;

use super::survival::Damage;
use super::*;
use crate::blocks::{offset, Dir};

/// Speed added each tick along the flight, and what is kept of it.
const ACCELERATION: f64 = 0.1;
const INERTIA: f64 = 0.95;
/// Fireballs that hit nothing for this long are gone.
const LIFETIME: u32 = 600;
/// Steps per block when checking a flight for hits.
const SAMPLE: f64 = 0.25;
/// Ticks during which a fireball cannot hit whoever shot it.
const SAFE_FROM_SHOOTER: u32 = 5;
const LARGE_POWER: f32 = 1.0;
const LARGE_HIT: f32 = 6.0;
const SMALL_HIT: f32 = 5.0;
const SMALL_FIRE_TICKS: i32 = 100;

pub struct Fireball {
    pos: [f64; 3],
    vel: [f64; 3],
    large: bool,
    /// Entity ID of whoever shot it last, and the name it is credited to.
    shooter: i32,
    by: &'static str,
    age: u32,
    sent_pos: [f64; 3],
}

enum Hit {
    Block,
    Player(i32),
    Mob(i32),
}

fn spawn_packet(id: i32, f: &Fireball) -> Writer {
    let v = |x: f64| (x.clamp(-3.9, 3.9) * 8000.0) as i16;
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(if f.large {
            entity_type::FIREBALL
        } else {
            entity_type::SMALL_FIREBALL
        })
        .f64(f.pos[0])
        .f64(f.pos[1])
        .f64(f.pos[2])
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(f.shooter)
        .i16(v(f.vel[0]))
        .i16(v(f.vel[1]))
        .i16(v(f.vel[2]))
}

fn normalized(v: [f64; 3]) -> [f64; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    v.map(|x| x / length)
}

impl World {
    /// Shoots a fireball from `from` towards `dir`, credited to `by`.
    pub(super) fn shoot_fireball(
        &mut self,
        shooter: i32,
        by: &'static str,
        from: [f64; 3],
        dir: [f64; 3],
        large: bool,
    ) {
        let fireball = Fireball {
            pos: from,
            vel: normalized(dir).map(|v| v * ACCELERATION),
            large,
            shooter,
            by,
            age: 0,
            sent_pos: from,
        };
        let id = next_entity_id();
        self.broadcast(spawn_packet(id, &fireball), None);
        self.fireballs.insert(id, fireball);
    }

    /// Fireballs lying about, for a player who just arrived.
    pub(super) fn fireball_packets(&self) -> Vec<Writer> {
        self.fireballs
            .iter()
            .map(|(&id, f)| spawn_packet(id, f))
            .collect()
    }

    /// A player hits a large fireball back where they look. Returns whether `target`
    /// was a fireball.
    pub(super) fn deflect_fireball(&mut self, player: i32, target: i32) -> bool {
        let Some(p) = self.players.get(&player) else {
            return self.fireballs.contains_key(&target);
        };
        let (_, look, _) = p.eye_ray();
        let Some(f) = self.fireballs.get_mut(&target) else {
            return false;
        };
        if f.large {
            let speed = (f.vel[0] * f.vel[0] + f.vel[1] * f.vel[1] + f.vel[2] * f.vel[2])
                .sqrt()
                .max(ACCELERATION);
            f.vel = look.map(|v| v * speed);
            f.shooter = player;
            f.by = "a player";
            f.age = 0;
            f.sent_pos = [f64::NAN; 3]; // tell clients at once
        }
        true
    }

    pub(super) fn tick_fireballs(&mut self) {
        let ids: Vec<i32> = self.fireballs.keys().copied().collect();
        for id in ids {
            let Some(mut f) = self.fireballs.remove(&id) else {
                continue;
            };
            f.age += 1;
            let cell = BlockPos::new(
                f.pos[0].floor() as i32,
                f.pos[1].floor() as i32,
                f.pos[2].floor() as i32,
            );
            if f.age > LIFETIME || !self.chunks.contains_key(&cell.chunk()) {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }
            let before = f.pos;
            if let Some(hit) = self.fireball_flight(&mut f) {
                self.broadcast(items::remove_entity(id), None);
                self.fireball_hits(&f, before, hit);
                continue;
            }
            // Speeding up along the flight, as clients also do.
            let dir = normalized(f.vel);
            f.vel = [0, 1, 2].map(|i| (f.vel[i] + dir[i] * ACCELERATION) * INERTIA);
            let drift: f64 = (0..3).map(|i| (f.pos[i] - f.sent_pos[i]).powi(2)).sum();
            if f.age.is_multiple_of(10) || drift.is_nan() {
                f.sent_pos = f.pos;
                self.broadcast(
                    Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                        .varint(id)
                        .f64(f.pos[0])
                        .f64(f.pos[1])
                        .f64(f.pos[2])
                        .f64(f.vel[0])
                        .f64(f.vel[1])
                        .f64(f.vel[2])
                        .f32(0.0)
                        .f32(0.0)
                        .bool(false),
                    None,
                );
            }
            self.fireballs.insert(id, f);
        }
    }

    /// Moves a fireball along its velocity, stopping at the first block or entity.
    fn fireball_flight(&mut self, f: &mut Fireball) -> Option<Hit> {
        let speed = (f.vel[0] * f.vel[0] + f.vel[1] * f.vel[1] + f.vel[2] * f.vel[2]).sqrt();
        let steps = (speed / SAMPLE).ceil().max(1.0) as usize;
        let step = f.vel.map(|v| v / steps as f64);
        let reach = if f.large { 0.5 } else { 0.15 };
        for _ in 0..steps {
            let next = [0, 1, 2].map(|i| f.pos[i] + step[i]);
            let skip = f.age <= SAFE_FROM_SHOOTER;
            let inside = |pos: [f64; 3], width: f64, height: f64| {
                let half = width / 2.0 + reach;
                (next[0] - pos[0]).abs() <= half
                    && (next[2] - pos[2]).abs() <= half
                    && next[1] >= pos[1] - reach
                    && next[1] <= pos[1] + height + reach
            };
            let player = self.players.iter().find(|(&id, p)| {
                p.alive && p.survival() && !(skip && id == f.shooter) && inside(p.pos, 0.6, 1.8)
            });
            if let Some((&id, _)) = player {
                f.pos = next;
                return Some(Hit::Player(id));
            }
            let mob = self.mobs.iter().find(|(&id, m)| {
                let (w, h) = m.size();
                m.alive() && !(skip && id == f.shooter) && inside(m.pos, w, h)
            });
            if let Some((&id, _)) = mob {
                f.pos = next;
                return Some(Hit::Mob(id));
            }
            if self.point_in_block(next) {
                return Some(Hit::Block);
            }
            f.pos = next;
        }
        None
    }

    fn fireball_hits(&mut self, f: &Fireball, before: [f64; 3], hit: Hit) {
        let by_player = self.players.contains_key(&f.shooter).then_some(f.shooter);
        let damage = if f.large { LARGE_HIT } else { SMALL_HIT };
        match hit {
            Hit::Player(id) => {
                self.damage_from(id, damage, Damage::Fireball(f.by), Some(f.shooter));
                if !f.large {
                    self.ignite_player(id, SMALL_FIRE_TICKS);
                }
            }
            Hit::Mob(id) => {
                let immune = self.mobs.get(&id).is_some_and(|m| m.kind.fire_immune());
                if !(immune && !f.large) {
                    let away = normalized([f.vel[0], 0.0, f.vel[2]]);
                    self.hit_mob(
                        id,
                        damage,
                        by_player,
                        [away[0], away[2]],
                        "minecraft:fireball",
                    );
                }
                if !f.large && !immune {
                    self.ignite_mob(id, SMALL_FIRE_TICKS);
                }
            }
            Hit::Block => {
                if !f.large {
                    // A fire starts in the open space the fireball came from.
                    let at = BlockPos::new(
                        f.pos[0].floor() as i32,
                        f.pos[1].floor() as i32,
                        f.pos[2].floor() as i32,
                    );
                    let free = [at, offset(at, Dir::Up)]
                        .into_iter()
                        .chain(std::iter::once(BlockPos::new(
                            before[0].floor() as i32,
                            before[1].floor() as i32,
                            before[2].floor() as i32,
                        )))
                        .find(|&p| is_air(self.block(p)));
                    if let Some(p) = free {
                        if let Some(fire) = crate::blocks::fire::fire_for(p, &mut |q| self.block(q))
                        {
                            self.light_fire(p, fire);
                        }
                    }
                }
            }
        }
        if f.large {
            self.explode_with_fire(f.pos, LARGE_POWER, f.by);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;
    use crate::world::mobs::{Mob, MobKind};

    fn world_with_player(at: [f64; 3]) -> (World, i32, mpsc::Receiver<Frame>) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-fireballs-{}-{}",
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
        for cx in -2..=2 {
            for cz in -2..=2 {
                world.chunk(cx, cz);
            }
        }
        let (out, frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
        };
        world.players.insert(id, Player::new(profile, out, at));
        (world, id, frames)
    }

    #[test]
    fn ghasts_charge_then_spit_fireballs_that_blow_up_on_players() {
        let (mut w, player, _frames) = world_with_player([0.5, -60.0, 0.5]);
        let ghast_id = next_entity_id();
        let mut ghast = Mob::new(MobKind::Ghast, [0.5, -50.0, 16.5], &mut w.rng);
        ghast.hunting = Some(player);
        for _ in 0..19 {
            w.hunt(ghast_id, &mut ghast);
        }
        assert!(w.fireballs.is_empty(), "still charging");
        w.hunt(ghast_id, &mut ghast);
        assert_eq!(w.fireballs.len(), 1);
        for _ in 0..60 {
            w.tick_fireballs();
        }
        assert!(w.fireballs.is_empty(), "it hit");
        assert!(w.players[&player].health < 20.0);
    }

    #[test]
    fn players_can_hit_fireballs_back() {
        let (mut w, player, _frames) = world_with_player([0.5, -60.0, 0.5]);
        w.shoot_fireball(42, "Ghast", [0.5, -58.0, 5.5], [0.0, 0.0, -1.0], true);
        let id = *w.fireballs.keys().next().unwrap();
        assert!(w.deflect_fireball(player, id));
        assert_eq!(w.fireballs[&id].shooter, player);
        assert!(
            w.fireballs[&id].vel[2] > 0.0,
            "now flying the way she looks"
        );
    }

    #[test]
    fn blazes_throw_three_small_fireballs_after_glowing() {
        let (mut w, player, _frames) = world_with_player([0.5, -60.0, 0.5]);
        let blaze_id = next_entity_id();
        let mut blaze = Mob::new(MobKind::Blaze, [0.5, -58.0, 8.5], &mut w.rng);
        blaze.hunting = Some(player);
        let mut thrown = 0;
        for _ in 0..120 {
            w.hunt(blaze_id, &mut blaze);
            thrown = thrown.max(w.fireballs.len());
        }
        assert_eq!(thrown, 3);
        assert!(w.fireballs.values().all(|f| !f.large));
    }
}
