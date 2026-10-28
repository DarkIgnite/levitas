//! Players attacking: mobs and other players, critical hits, sprint knockback and
//! sweeping blows.
//!
//! Behaviour follows the Minecraft Wiki: damage comes from the held item and how far the
//! attack has recharged; a fully charged hit while falling is critical (x1.5); a charged
//! hit while sprinting knocks the target back further; a charged sword hit on the ground
//! that is neither also sweeps everything within a block of the target for 1 damage.

use levitas_registry::particle;

use super::*;
use crate::world::survival::Damage;

/// How far a player can reach an entity.
const REACH: f64 = 6.0;
/// The base knockback of a hit, and what sprinting adds.
const KNOCKBACK: f64 = 0.4;
const SPRINT_KNOCKBACK: f64 = 0.5;
/// Sweeping hits land within this many blocks of the target (vertically less) and of the
/// attacker.
const SWEEP_SPREAD: f64 = 1.0;
const SWEEP_SPREAD_UP: f64 = 0.25;
const SWEEP_RANGE: f64 = 3.0;
const SWEEP_DAMAGE: f32 = 1.0;
const ANIMATE_CRITICAL: u8 = 4;
const PLAYER_HEIGHT: f64 = 1.8;

/// Something a player can hit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Player(i32),
    Mob(i32),
}

impl World {
    /// A player hits an entity with what they hold.
    pub(super) fn attack(&mut self, attacker: i32, target: i32) {
        let Some(p) = self.players.get(&attacker) else {
            return;
        };
        let target = if self.players.contains_key(&target) && target != attacker {
            Target::Player(target)
        } else if self.mobs.contains_key(&target) {
            Target::Mob(target)
        } else {
            return;
        };
        let Some((pos, height)) = self.target_body(target) else {
            return;
        };
        if !p.alive || !within_reach(p, pos, height) {
            return;
        }

        let slot = p.hand_slot(0);
        let item = ItemInfo::get(p.inventory.slots[slot].item);
        let (damage, speed) = item.map_or((1.0, 4.0), |i| (i.attack_damage, i.attack_speed));
        let cooldown = 20.0 / f64::from(speed);
        let charge = ((self.tick.saturating_sub(p.attack_reset) as f64 + 0.5) / cooldown).min(1.0);
        let damage = (damage + p.attack_bonus()).max(0.0);
        let mut damage = damage * (0.2 + (charge * charge) as f32 * 0.8);
        let charged = charge > 0.9;
        let critical = charged && p.fall_distance > 0.0 && !p.on_ground && !p.sprinting;
        if critical {
            damage *= 1.5;
        }
        let sprint_hit = charged && p.sprinting;
        let sweep = charged
            && !critical
            && !sprint_hit
            && p.on_ground
            && item.is_some_and(|i| i.name.ends_with("_sword"));
        let is_weapon = item.is_some_and(|i| {
            i.name.ends_with("_sword") || i.name.ends_with("_axe") || i.name.ends_with("_trident")
        });
        let yaw = f64::from(p.yaw).to_radians();
        let away = [-yaw.sin(), yaw.cos()];
        let attacker_pos = p.pos;

        let p = self.players.get_mut(&attacker).expect("player exists");
        p.attack_reset = self.tick;
        p.exhaustion += 0.1;

        let knockback = KNOCKBACK + if sprint_hit { SPRINT_KNOCKBACK } else { 0.0 };
        if !self.hit_target(target, damage, attacker, away, knockback) {
            self.attack_sound(attacker_pos, "minecraft:entity.player.attack.nodamage");
            return;
        }
        if sprint_hit {
            // A knockback hit ends the sprint.
            if let Some(p) = self.players.get_mut(&attacker) {
                p.sprinting = false;
            }
        }
        let sound = if critical {
            "minecraft:entity.player.attack.crit"
        } else if sprint_hit {
            "minecraft:entity.player.attack.knockback"
        } else if sweep {
            "minecraft:entity.player.attack.sweep"
        } else if charged {
            "minecraft:entity.player.attack.strong"
        } else {
            "minecraft:entity.player.attack.weak"
        };
        self.attack_sound(attacker_pos, sound);
        if critical {
            let id = match target {
                Target::Player(id) | Target::Mob(id) => id,
            };
            self.broadcast(
                Writer::packet(clientbound::ANIMATE)
                    .varint(id)
                    .u8(ANIMATE_CRITICAL),
                None,
            );
        }
        if sweep {
            self.sweep(attacker, target, pos, attacker_pos, away);
        }
        if is_weapon {
            self.damage_item(attacker, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
        }
    }

    /// Where an entity stands and how tall it is.
    fn target_body(&self, target: Target) -> Option<([f64; 3], f64)> {
        match target {
            Target::Player(id) => self
                .players
                .get(&id)
                .filter(|p| p.alive)
                .map(|p| (p.pos, PLAYER_HEIGHT)),
            Target::Mob(id) => self
                .mobs
                .get(&id)
                .filter(|m| m.alive())
                .map(|m| (m.pos, m.size().1)),
        }
    }

    /// Hurts and knocks back what was hit. Returns whether the hit did anything.
    fn hit_target(
        &mut self,
        target: Target,
        damage: f32,
        attacker: i32,
        away: [f64; 2],
        knockback: f64,
    ) -> bool {
        match target {
            Target::Mob(id) => {
                // `hit_mob` pushes by the base knockback along `away`.
                let away = away.map(|v| v * knockback / KNOCKBACK);
                self.hit_mob(id, damage, Some(attacker), away, "minecraft:player_attack")
            }
            Target::Player(id) => {
                if !self.damage_from(id, damage, Damage::Player(attacker), Some(attacker)) {
                    return false;
                }
                self.push_player(id, [away[0] * knockback, KNOCKBACK, away[1] * knockback]);
                true
            }
        }
    }

    /// A sweeping blow: everything else close to the target takes a light hit.
    fn sweep(
        &mut self,
        attacker: i32,
        target: Target,
        at: [f64; 3],
        attacker_pos: [f64; 3],
        away: [f64; 2],
    ) {
        let near = |pos: [f64; 3], height: f64| {
            let beside = (pos[0] - at[0]).abs() <= SWEEP_SPREAD + 0.6
                && (pos[2] - at[2]).abs() <= SWEEP_SPREAD + 0.6
                && pos[1] <= at[1] + PLAYER_HEIGHT + SWEEP_SPREAD_UP
                && pos[1] + height >= at[1] - SWEEP_SPREAD_UP;
            let d: f64 = (0..3).map(|i| (pos[i] - attacker_pos[i]).powi(2)).sum();
            beside && d < SWEEP_RANGE * SWEEP_RANGE
        };
        let mut hit: Vec<Target> = self
            .mobs
            .iter()
            .filter(|(_, m)| m.alive() && near(m.pos, m.size().1))
            .map(|(&id, _)| Target::Mob(id))
            .collect();
        hit.extend(
            self.players
                .iter()
                .filter(|(&id, p)| id != attacker && p.alive && near(p.pos, PLAYER_HEIGHT))
                .map(|(&id, _)| Target::Player(id)),
        );
        for other in hit.into_iter().filter(|&t| t != target) {
            self.hit_target(other, SWEEP_DAMAGE, attacker, away, KNOCKBACK);
        }

        let at = [
            attacker_pos[0] + away[0],
            attacker_pos[1] + PLAYER_HEIGHT * 0.5,
            attacker_pos[2] + away[1],
        ];
        self.broadcast(
            Writer::packet(clientbound::LEVEL_PARTICLES)
                .bool(false) // override limiter
                .bool(false) // always show
                .f64(at[0])
                .f64(at[1])
                .f64(at[2])
                .f32(away[0] as f32)
                .f32(0.0)
                .f32(away[1] as f32)
                .f32(0.0) // speed
                .i32(0) // count: the offset is the direction
                .varint(particle::SWEEP_ATTACK),
            None,
        );
    }

    fn attack_sound(&mut self, at: [f64; 3], sound: &str) {
        self.play_sound_at(sound, at, 1.0, 1.0, None);
    }
}

/// Whether an entity standing at `pos`, `height` tall, is within the player's reach.
fn within_reach(p: &Player, pos: [f64; 3], height: f64) -> bool {
    let (eye, _, _) = p.eye_ray();
    let centre = [pos[0], pos[1] + height / 2.0, pos[2]];
    let d: f64 = (0..3).map(|i| (eye[i] - centre[i]).powi(2)).sum();
    d.sqrt() <= REACH
}
