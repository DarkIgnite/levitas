//! Shulkers and their bullets.
//!
//! Behaviour follows the Minecraft Wiki: a shulker sits fixed to a block face. It opens
//! its shell now and then, and when a player is within 16 blocks in sight it opens up
//! and fires a bullet every 1 to 5.5 seconds. Bullets home in on their target and on a
//! hit deal 4 damage and give Levitation for 10 seconds; a player can knock one out of
//! the air. With its shell closed a shulker has 20 armour. Hurt below half health it
//! may teleport to another block face within 8 blocks.

use super::mobs::{Mob, MobKind};
use super::*;
use crate::blocks::{self, Dir};
use crate::world::survival::Damage;
use levitas_registry::collision_boxes;

/// Entity data: the face it is attached to, how far its shell is open (0 to 100).
const DATA_ATTACH_FACE: u8 = 16;
const DATA_PEEK: u8 = 17;
const SERIALIZER_BYTE: i32 = 0;
const SERIALIZER_DIRECTION: i32 = 12;
const RANGE: f64 = 16.0;
const BULLET_SPEED: f64 = 0.15;
const BULLET_LIFE: u32 = 300;
const BULLET_DAMAGE: f32 = 4.0;
const LEVITATION_TICKS: i32 = 200;
/// Armour of a closed shell.
const SHELL_ARMOR: f32 = 20.0;

/// A shulker bullet in flight.
pub struct Bullet {
    pos: [f64; 3],
    vel: [f64; 3],
    shooter: i32,
    target: i32,
    age: u32,
}

fn peek_data(id: i32, peek: u8) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_PEEK)
        .varint(SERIALIZER_BYTE)
        .u8(peek)
        .u8(0xFF)
}

/// The packets showing a shulker as it is, after it spawned for someone.
pub fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    if mob.kind != MobKind::Shulker {
        return Vec::new();
    }
    let mut out = Vec::new();
    if mob.variant != 0 {
        out.push(
            Writer::packet(clientbound::SET_ENTITY_DATA)
                .varint(id)
                .u8(DATA_ATTACH_FACE)
                .varint(SERIALIZER_DIRECTION)
                .varint(i32::from(mob.variant))
                .u8(0xFF),
        );
    }
    if mob.charge > 0 {
        out.push(peek_data(id, mob.charge as u8));
    }
    out
}

/// Damage after a closed shell's armour (the usual armour formula, no toughness).
pub fn through_shell(mob: &Mob, damage: f32) -> f32 {
    if mob.kind != MobKind::Shulker || mob.charge > 0 {
        return damage;
    }
    let cut = (SHELL_ARMOR / 5.0)
        .max(SHELL_ARMOR - damage / 2.0)
        .min(20.0);
    damage * (1.0 - cut / 25.0)
}

fn bullet_spawn(id: i32, b: &Bullet) -> Writer {
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::SHULKER_BULLET)
        .f64(b.pos[0])
        .f64(b.pos[1])
        .f64(b.pos[2])
        .lp_vec3(b.vel)
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(b.shooter)
}

fn bullet_sync(id: i32, b: &Bullet) -> Writer {
    Writer::packet(clientbound::ENTITY_POSITION_SYNC)
        .varint(id)
        .varint(0)
        .f64(b.pos[0])
        .f64(b.pos[1])
        .f64(b.pos[2])
        .f32(0.0)
        .f32(0.0)
        .bool(false)
}

impl World {
    /// A shulker's turn: open up at players in sight and shoot at them, otherwise open
    /// and close now and then.
    pub(super) fn tick_shulker(&mut self, id: i32, mob: &mut Mob) {
        mob.attack_cooldown = mob.attack_cooldown.saturating_sub(1);
        let eye = [mob.pos[0], mob.pos[1] + 0.5, mob.pos[2]];
        let target = self
            .players
            .iter()
            .filter(|(_, p)| p.alive && p.survival())
            .map(|(&pid, p)| (pid, [p.pos[0], p.pos[1] + 1.0, p.pos[2]]))
            .filter(|&(_, at)| {
                (0..3).map(|i| (at[i] - eye[i]).powi(2)).sum::<f64>().sqrt() <= RANGE
                    && self.line_of_sight(eye, at)
            })
            .min_by(|a, b| {
                let d = |q: [f64; 3]| (0..3).map(|i| (q[i] - eye[i]).powi(2)).sum::<f64>();
                d(a.1).total_cmp(&d(b.1))
            });
        let peek = match target {
            Some(_) => 30,
            // Now and then it opens up for a look, and closes again.
            None if mob.charge == 0 && self.rng.below(200) == 0 => 30,
            None if mob.charge > 0 && self.rng.below(60) == 0 => 0,
            None => mob.charge,
        };
        if peek != mob.charge {
            mob.charge = peek;
            self.send_to_viewers(mob, peek_data(id, peek as u8));
            let sound = if peek > 0 {
                "minecraft:entity.shulker.open"
            } else {
                "minecraft:entity.shulker.close"
            };
            self.play_sound_at(sound, mob.pos, 1.0, 1.0, None);
        }
        let Some((pid, _)) = target else {
            return;
        };
        mob.hunting = Some(pid);
        if mob.attack_cooldown > 0 {
            return;
        }
        mob.attack_cooldown = 20 + self.rng.below(10) * 10;
        let bullet = Bullet {
            pos: [eye[0], eye[1] + 0.5, eye[2]],
            vel: [0.0, BULLET_SPEED, 0.0],
            shooter: id,
            target: pid,
            age: 0,
        };
        let bid = next_entity_id();
        self.broadcast(bullet_spawn(bid, &bullet), None);
        self.bullets.insert(bid, bullet);
        self.play_sound_at("minecraft:entity.shulker.shoot", mob.pos, 2.0, 1.0, None);
    }

    /// A shulker hurt below half health may teleport to a free spot on a solid block
    /// within 8 blocks.
    pub(super) fn shulker_hurt(&mut self, mob: &mut Mob) {
        if mob.kind != MobKind::Shulker
            || mob.health >= mob.max_health() / 2.0
            || self.rng.below(4) != 0
        {
            return;
        }
        for _ in 0..5 {
            let at = BlockPos::new(
                mob.pos[0].floor() as i32 + self.rng.below(17) as i32 - 8,
                mob.pos[1].floor() as i32 + self.rng.below(17) as i32 - 8,
                mob.pos[2].floor() as i32 + self.rng.below(17) as i32 - 8,
            );
            let below = BlockPos::new(at.x, at.y - 1, at.z);
            let free = self.loaded_block(at).is_some_and(is_air);
            let floor = self
                .loaded_block(below)
                .is_some_and(|s| blocks::sturdy(s, Dir::Up));
            if free && floor {
                mob.pos = [
                    f64::from(at.x) + 0.5,
                    f64::from(at.y),
                    f64::from(at.z) + 0.5,
                ];
                mob.variant = 0; // on the floor now
                self.play_sound_at("minecraft:entity.shulker.teleport", mob.pos, 1.0, 1.0, None);
                return;
            }
        }
    }

    /// Bullets fly after their targets.
    pub(super) fn tick_bullets(&mut self) {
        let ids: Vec<i32> = self.bullets.keys().copied().collect();
        for id in ids {
            let Some(mut b) = self.bullets.remove(&id) else {
                continue;
            };
            b.age += 1;
            let target = self
                .players
                .get(&b.target)
                .filter(|p| p.alive)
                .map(|p| [p.pos[0], p.pos[1] + 0.9, p.pos[2]]);
            let Some(aim) = target.filter(|_| b.age < BULLET_LIFE) else {
                self.broadcast(items::remove_entity(id), None);
                continue;
            };
            // Turning towards the target a little each tick.
            let d = [aim[0] - b.pos[0], aim[1] - b.pos[1], aim[2] - b.pos[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
            for (v, d) in b.vel.iter_mut().zip(d) {
                *v = *v * 0.8 + d / len * BULLET_SPEED * 0.2;
            }
            let next = [0, 1, 2].map(|i| b.pos[i] + b.vel[i]);
            if len < 0.8 {
                self.bullet_hits(id, &b);
                continue;
            }
            let cell = BlockPos::new(
                next[0].floor() as i32,
                next[1].floor() as i32,
                next[2].floor() as i32,
            );
            if self
                .loaded_block(cell)
                .is_none_or(|s| !collision_boxes(s).is_empty())
            {
                self.play_sound_at("minecraft:entity.shulker_bullet.hit", b.pos, 1.0, 1.0, None);
                self.broadcast(items::remove_entity(id), None);
                continue;
            }
            b.pos = next;
            self.broadcast(bullet_sync(id, &b), None);
            self.bullets.insert(id, b);
        }
    }

    fn bullet_hits(&mut self, id: i32, b: &Bullet) {
        self.broadcast(items::remove_entity(id), None);
        self.play_sound_at("minecraft:entity.shulker_bullet.hit", b.pos, 1.0, 1.0, None);
        if self.damage_from(
            b.target,
            BULLET_DAMAGE,
            Damage::Mob("Shulker"),
            Some(b.shooter),
        ) {
            self.add_effect(
                b.target,
                levitas_registry::Effect {
                    id: levitas_registry::mob_effect::LEVITATION,
                    duration: LEVITATION_TICKS,
                    amplifier: 0,
                },
            );
        }
    }

    /// A player knocks a bullet out of the air. Returns whether `target` was a bullet.
    pub(super) fn hit_bullet(&mut self, target: i32) -> bool {
        let Some(b) = self.bullets.remove(&target) else {
            return false;
        };
        self.broadcast(items::remove_entity(target), None);
        self.play_sound_at(
            "minecraft:entity.shulker_bullet.hurt",
            b.pos,
            1.0,
            1.0,
            None,
        );
        true
    }

    /// Bullets in flight, for a player who just arrived.
    pub(super) fn bullet_packets(&self) -> Vec<Writer> {
        self.bullets
            .iter()
            .map(|(&id, b)| bullet_spawn(id, b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shulkers_shoot_bullets_that_levitate_their_target() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-shulker-{}-{}",
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
        for cx in -1..=1 {
            for cz in -1..=1 {
                w.chunk(cx, cz);
            }
        }
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = crate::connection::Profile {
            name: "Alice".to_owned(),
            uuid: 3,
            properties: Vec::new(),
        };
        w.players
            .insert(alice, Player::new(profile, out, [6.5, -60.0, 0.5]));
        let id = next_entity_id();
        let mut shulker = Mob::new(MobKind::Shulker, [0.5, -60.0, 0.5], &mut w.rng);
        w.tick_shulker(id, &mut shulker);
        assert!(shulker.charge > 0, "opens up at Alice");
        assert_eq!(w.bullets.len(), 1);
        for _ in 0..200 {
            w.tick_bullets();
            if w.bullets.is_empty() {
                break;
            }
        }
        assert!(w.bullets.is_empty(), "the bullet found her");
        assert!(w.players[&alice].health < 20.0);
        assert!(w.players[&alice]
            .effect(levitas_registry::mob_effect::LEVITATION)
            .is_some());
    }

    #[test]
    fn a_closed_shell_takes_far_less_damage() {
        let mut rng = Rng::new(1);
        let mut shulker = Mob::new(MobKind::Shulker, [0.5, -60.0, 0.5], &mut rng);
        shulker.charge = 0;
        assert!(
            (through_shell(&shulker, 10.0) - 4.0).abs() < 1e-4,
            "{}",
            through_shell(&shulker, 10.0)
        );
        shulker.charge = 30;
        assert_eq!(through_shell(&shulker, 10.0), 10.0);
    }
}
