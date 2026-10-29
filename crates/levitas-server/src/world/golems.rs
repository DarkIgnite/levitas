//! Iron golems, following the Minecraft Wiki: they guard villages, going after monsters
//! (creepers excepted) within 16 blocks and flinging them up with a heavy blow, and turn
//! on a player who hits them, unless a player built them. Four iron blocks in a T with a
//! carved pumpkin on top make one. An iron ingot mends 25 health. They take no fall
//! damage and shrug off knockback, and drop iron ingots and poppies.

use levitas_registry::block;

use super::mobs::{Mob, MobKind};
use super::survival::Damage;
use super::*;
use crate::blocks::{self, offset, Dir};

/// Entity data: whether a player built the golem.
const DATA_GOLEM_FLAGS: u8 = 16;
const SERIALIZER_BYTE: i32 = 0;
const PLAYER_CREATED: u8 = 0x01;
const ENTITY_EVENT_ATTACK: i8 = 4;
const RANGE: f64 = 16.0;
const REACH: f64 = 2.5;
const ATTACK_TICKS: u32 = 20;
/// Ticks a golem stays angry at a player who hit it.
const ANGER_TICKS: u32 = 600;
const REPAIR: f32 = 25.0;

/// The data telling clients whether a golem was built by a player.
pub fn golem_data(id: i32, player_created: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_GOLEM_FLAGS)
        .varint(SERIALIZER_BYTE)
        .u8(if player_created { PLAYER_CREATED } else { 0 })
        .u8(0xFF)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

impl World {
    /// A golem's turn: keep after its target or find one, and strike when close.
    pub(super) fn tick_golem(&mut self, id: i32, mob: &mut Mob) {
        mob.attack_cooldown = mob.attack_cooldown.saturating_sub(1);
        mob.anger = mob.anger.saturating_sub(1);
        if mob.anger == 0 {
            mob.hunting = None;
        }
        let angry_at = mob
            .hunting
            .and_then(|pid| self.players.get(&pid))
            .filter(|p| p.alive && p.survival() && distance(p.pos, mob.pos) <= RANGE)
            .map(|p| p.pos);
        if angry_at.is_none() {
            mob.hunting = None;
        }
        let prey_valid = mob
            .prey
            .and_then(|m| self.mobs.get(&m))
            .is_some_and(|m| m.alive() && distance(m.pos, mob.pos) <= RANGE);
        if !prey_valid {
            mob.prey = self
                .mobs
                .iter()
                .filter(|(_, m)| {
                    m.alive()
                        && m.kind.is_monster()
                        && m.kind != MobKind::Creeper
                        && distance(m.pos, mob.pos) <= RANGE
                })
                .min_by(|a, b| distance(a.1.pos, mob.pos).total_cmp(&distance(b.1.pos, mob.pos)))
                .map(|(&m, _)| m);
        }
        let target = match (angry_at, mob.prey) {
            (Some(pos), _) => pos,
            (None, Some(m)) => self.mobs[&m].pos,
            (None, None) => return,
        };
        mob.head_yaw = super::mobs::yaw_towards(mob.pos, target);
        let goal = BlockPos::new(
            target[0].floor() as i32,
            (target[1] + 0.01).floor() as i32,
            target[2].floor() as i32,
        );
        self.follow_path(mob, goal);
        let close = distance(target, mob.pos) <= REACH && (target[1] - mob.pos[1]).abs() < 2.0;
        if !close || mob.attack_cooldown > 0 {
            return;
        }
        mob.attack_cooldown = ATTACK_TICKS;
        // 7.5 to 21.5 on Normal; players get it scaled by the difficulty.
        let damage = 7.5 + self.rng.below(15) as f32;
        self.send_to_viewers(mob, entity_event(id, ENTITY_EVENT_ATTACK));
        self.play_sound_at(
            "minecraft:entity.iron_golem.attack",
            mob.pos,
            1.0,
            1.0,
            None,
        );
        match (mob.hunting.filter(|_| angry_at.is_some()), mob.prey) {
            (Some(pid), _) => {
                if self.damage_from(pid, damage, Damage::Mob("Iron Golem"), Some(id)) {
                    self.push_player(pid, [0.0, 0.4, 0.0]);
                }
            }
            (None, Some(prey)) => {
                let away = [0.0, 0.0];
                if self.hit_mob(prey, damage, None, away, "minecraft:mob_attack") {
                    if let Some(m) = self.mobs.get_mut(&prey) {
                        m.push([0.0, 0.4, 0.0]);
                    }
                }
            }
            (None, None) => {}
        }
    }

    /// A player hit a golem: one a player built shrugs it off, any other turns on them.
    pub(super) fn golem_hit_by(mob: &mut Mob, player: i32) {
        if !mob.player_created {
            mob.hunting = Some(player);
            mob.anger = ANGER_TICKS;
        }
    }

    /// An iron ingot used on a hurt golem mends it. Returns whether it did.
    pub(super) fn repair_golem(&mut self, id: i32, target: i32, slot: usize) -> bool {
        let Some(mob) = self.mobs.get_mut(&target) else {
            return false;
        };
        let max = MobKind::IronGolem.max_health();
        if mob.kind != MobKind::IronGolem || mob.health >= max {
            return false;
        }
        mob.health = (mob.health + REPAIR).min(max);
        let pos = mob.pos;
        let pitch = 1.0 + (self.rng.f64() as f32 - self.rng.f64() as f32) * 0.2;
        self.play_sound_at("minecraft:entity.iron_golem.repair", pos, 1.0, pitch, None);
        let p = self.players.get_mut(&id).expect("the user exists");
        if p.survival() {
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        true
    }

    /// A carved pumpkin just placed on top of four iron blocks in a T (the space under
    /// the arms empty) turns them into an iron golem.
    pub(super) fn try_build_golem(&mut self, head: BlockPos) {
        let name = blocks::name(self.block(head));
        if !matches!(
            name,
            "minecraft:carved_pumpkin" | "minecraft:jack_o_lantern"
        ) {
            return;
        }
        let body = offset(head, Dir::Down);
        let legs = offset(body, Dir::Down);
        if self.block(body) != block::IRON_BLOCK || self.block(legs) != block::IRON_BLOCK {
            return;
        }
        for (a, b) in [(Dir::West, Dir::East), (Dir::North, Dir::South)] {
            let arms = [offset(body, a), offset(body, b)];
            let under = [offset(arms[0], Dir::Down), offset(arms[1], Dir::Down)];
            let built = arms.iter().all(|&q| self.block(q) == block::IRON_BLOCK)
                && under.iter().all(|&q| is_air(self.block(q)));
            if !built {
                continue;
            }
            for q in [head, body, legs, arms[0], arms[1]] {
                self.set_block_updating(q, block::AIR);
            }
            let at = [
                f64::from(legs.x) + 0.5,
                f64::from(legs.y),
                f64::from(legs.z) + 0.5,
            ];
            let mut golem = Mob::new(MobKind::IronGolem, at, &mut self.rng);
            golem.player_created = true;
            self.mobs.insert(next_entity_id(), golem);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-golem-{}-{}",
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
        w.chunk(0, 0);
        w
    }

    #[test]
    fn iron_blocks_and_a_pumpkin_make_a_golem() {
        let mut w = world();
        let base = BlockPos::new(5, -60, 5);
        let body = offset(base, Dir::Up);
        for q in [base, body, offset(body, Dir::East), offset(body, Dir::West)] {
            w.set_block(q, block::IRON_BLOCK);
        }
        let head = offset(body, Dir::Up);
        w.set_block(head, block::CARVED_PUMPKIN);
        w.try_build_golem(head);
        let golem = w.mobs.values().find(|m| m.kind == MobKind::IronGolem);
        assert!(golem.is_some_and(|g| g.player_created));
        assert!(is_air(w.block(body)) && is_air(w.block(head)));
    }

    #[test]
    fn golems_go_after_monsters_and_leave_creepers_be() {
        let mut w = world();
        let golem_id = next_entity_id();
        w.mobs.insert(
            golem_id,
            Mob::new(MobKind::IronGolem, [2.5, -60.0, 2.5], &mut w.rng),
        );
        let creeper = next_entity_id();
        w.mobs.insert(
            creeper,
            Mob::new(MobKind::Creeper, [3.5, -60.0, 2.5], &mut w.rng),
        );
        let zombie = next_entity_id();
        w.mobs.insert(
            zombie,
            Mob::new(MobKind::Zombie, [4.0, -60.0, 2.5], &mut w.rng),
        );
        let mut g = w.mobs.remove(&golem_id).unwrap();
        w.tick_golem(golem_id, &mut g);
        assert_eq!(g.prey, Some(zombie));
        assert!(w.mobs[&zombie].health < MobKind::Zombie.max_health());
        assert_eq!(w.mobs[&creeper].health, MobKind::Creeper.max_health());
    }
}
