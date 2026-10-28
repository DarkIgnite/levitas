//! Experience: orbs dropped by mobs and ores, and players' levels.
//!
//! Behaviour follows the Minecraft Wiki: orbs float towards a player within 8 blocks and
//! are absorbed on touch; reaching the next level takes 2 × level + 7 points up to level
//! 15, 5 × level − 38 up to 30, and 9 × level − 158 beyond. Killed animals drop 1 to 3
//! points and monsters 5 (only when a player killed them); ores drop what the wiki lists
//! when mined without silk touch. Dying drops 7 points per level, at most 100.

use levitas_registry::entity_type;

use super::*;

const GRAVITY: f64 = 0.03;
const DRAG: f64 = 0.98;
/// Orbs fly towards players this close.
const ATTRACT: f64 = 8.0;
/// Orbs disappear after five minutes.
const DESPAWN_TICKS: u32 = 6000;
/// Orb sizes: the largest that fits is split off first, like vanilla.
const ORB_SIZES: [i32; 11] = [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3, 1];
const DATA_ORB_VALUE: u8 = 8;
const SERIALIZER_INT: i32 = 1;

pub struct XpOrb {
    pos: [f64; 3],
    vel: [f64; 3],
    value: i32,
    age: u32,
    sent_pos: [f64; 3],
}

/// Points needed to go from `level` to the next.
pub fn points_for_level(level: i32) -> i32 {
    if level >= 30 {
        9 * level - 158
    } else if level >= 15 {
        5 * level - 38
    } else {
        2 * level + 7
    }
}

/// The experience a block gives when mined without silk touch, if any.
fn ore_experience(name: &str, rng: &mut Rng) -> i32 {
    let name = name.strip_prefix("minecraft:deepslate_").unwrap_or(name);
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    let (min, max) = match name {
        "coal_ore" => (0, 2),
        "nether_gold_ore" => (0, 1),
        "diamond_ore" | "emerald_ore" => (3, 7),
        "lapis_ore" | "nether_quartz_ore" => (2, 5),
        "redstone_ore" => (1, 5),
        _ => return 0,
    };
    min + rng.below((max - min + 1) as u32) as i32
}

pub fn xp_packet(p: &Player) -> Writer {
    let needed = points_for_level(p.xp_level);
    Writer::packet(clientbound::SET_EXPERIENCE)
        .f32(p.xp_points as f32 / needed as f32)
        .varint(p.xp_level)
        .varint(p.xp_total)
}

impl World {
    /// Drops `total` experience as orbs at `pos`.
    pub(super) fn spawn_xp(&mut self, pos: [f64; 3], mut total: i32) {
        while total > 0 {
            let value = ORB_SIZES
                .iter()
                .copied()
                .find(|&size| size <= total)
                .unwrap_or(1);
            total -= value;
            let vel = [
                (self.rng.f64() - 0.5) * 0.2,
                self.rng.f64() * 0.2,
                (self.rng.f64() - 0.5) * 0.2,
            ];
            let orb = XpOrb {
                pos,
                vel,
                value,
                age: 0,
                sent_pos: pos,
            };
            let id = next_entity_id();
            for packet in orb_packets(id, &orb) {
                self.broadcast(packet, None);
            }
            self.orbs.insert(id, orb);
        }
    }

    /// Experience from a block a player mined in survival with the right tool.
    pub(super) fn mined_xp(&mut self, pos: BlockPos, state: u16) {
        let xp = ore_experience(BlockInfo::of(state).name, &mut self.rng);
        if xp > 0 {
            let centre = [pos.x, pos.y, pos.z].map(|v| f64::from(v) + 0.5);
            self.spawn_xp(centre, xp);
        }
    }

    /// Experience from a mob a player killed.
    pub(super) fn mob_xp(&mut self, pos: [f64; 3], monster: bool) {
        let xp = if monster {
            5
        } else {
            1 + self.rng.below(3) as i32
        };
        self.spawn_xp(pos, xp);
    }

    /// Adds experience points to a player, levelling up as needed.
    pub(super) fn give_xp(&mut self, id: i32, points: i32) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let level_before = p.xp_level;
        p.xp_total = p.xp_total.saturating_add(points);
        p.xp_points += points;
        while p.xp_points >= points_for_level(p.xp_level) {
            p.xp_points -= points_for_level(p.xp_level);
            p.xp_level += 1;
        }
        let packet = xp_packet(p);
        let (pos, level) = (p.pos, p.xp_level);
        self.send(id, packet);
        // A chime every five levels.
        if level > level_before && level % 5 == 0 {
            self.play_sound_at("minecraft:entity.player.levelup", pos, 0.75, 1.0, None);
        }
    }

    /// Adds whole levels (`/xp add <n> levels`).
    pub(super) fn give_levels(&mut self, id: i32, levels: i32) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        p.xp_level = (p.xp_level + levels).max(0);
        p.xp_points = p.xp_points.min(points_for_level(p.xp_level) - 1);
        let packet = xp_packet(p);
        self.send(id, packet);
    }

    /// A dying player drops some of their experience and loses the rest.
    pub(super) fn drop_xp_on_death(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let dropped = (p.xp_level * 7).min(100);
        let pos = p.pos;
        p.xp_level = 0;
        p.xp_points = 0;
        p.xp_total = 0;
        let packet = xp_packet(p);
        self.send(id, packet);
        if dropped > 0 {
            self.spawn_xp(pos, dropped);
        }
    }

    pub(super) fn tick_orbs(&mut self) {
        let ids: Vec<i32> = self.orbs.keys().copied().collect();
        for id in ids {
            let Some(mut orb) = self.orbs.remove(&id) else {
                continue;
            };
            orb.age += 1;
            if orb.age > DESPAWN_TICKS || orb.pos[1] < f64::from(MIN_Y - 64) {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }
            // The nearest living player within reach pulls the orb in.
            let nearest = self
                .players
                .iter()
                .filter(|(_, p)| p.alive)
                .map(|(&pid, p)| {
                    let target = [p.pos[0], p.pos[1] + 0.8, p.pos[2]];
                    let d = [0, 1, 2].map(|i| target[i] - orb.pos[i]);
                    let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    (pid, d, distance)
                })
                .filter(|&(_, _, distance)| distance < ATTRACT)
                .min_by(|a, b| a.2.total_cmp(&b.2));
            if let Some((pid, d, distance)) = nearest {
                if distance < 1.0 {
                    self.collect_orb(id, &orb, pid);
                    continue;
                }
                let pull = (1.0 - distance / ATTRACT).powi(2) * 0.1;
                for (v, d) in orb.vel.iter_mut().zip(d) {
                    *v += d / distance * pull;
                }
            }
            orb.vel[1] -= GRAVITY;
            let dy = self.clip_move(orb.pos, 0.125, 0.25, 1, orb.vel[1]);
            if (dy - orb.vel[1]).abs() > 1e-9 {
                orb.vel[1] = 0.0;
                orb.vel[0] *= 0.6;
                orb.vel[2] *= 0.6;
            }
            orb.pos[1] += dy;
            for axis in [0, 2] {
                let d = self.clip_move(orb.pos, 0.125, 0.25, axis, orb.vel[axis]);
                orb.pos[axis] += d;
            }
            orb.vel = orb.vel.map(|v| v * DRAG);

            // Clients move orbs themselves; correct them now and then.
            let moved: f64 = (0..3).map(|i| (orb.pos[i] - orb.sent_pos[i]).powi(2)).sum();
            if orb.age.is_multiple_of(5) && moved > 0.01 {
                orb.sent_pos = orb.pos;
                self.broadcast(
                    Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                        .varint(id)
                        .f64(orb.pos[0])
                        .f64(orb.pos[1])
                        .f64(orb.pos[2])
                        .f64(orb.vel[0])
                        .f64(orb.vel[1])
                        .f64(orb.vel[2])
                        .f32(0.0)
                        .f32(0.0)
                        .bool(false),
                    None,
                );
            }
            self.orbs.insert(id, orb);
        }
    }

    fn collect_orb(&mut self, id: i32, orb: &XpOrb, player: i32) {
        self.broadcast(
            Writer::packet(clientbound::TAKE_ITEM_ENTITY)
                .varint(id)
                .varint(player)
                .varint(1),
            None,
        );
        // The client flies the orb into the player and plays the pickup sound itself,
        // but unlike items it only removes an orb when told to.
        self.broadcast(items::remove_entity(id), None);
        let left = self.mend(player, orb.value);
        if left > 0 {
            self.give_xp(player, left);
        }
    }

    /// Mending: experience repairs a damaged mending item in hand or worn, two points of
    /// durability per experience point. Returns the experience left over.
    fn mend(&mut self, id: i32, xp: i32) -> i32 {
        let Some(p) = self.players.get(&id) else {
            return xp;
        };
        let slots = [p.hand_slot(0), OFFHAND, 5, 6, 7, 8];
        let mendable: Vec<usize> = slots
            .into_iter()
            .filter(|&s| {
                let stack = p.inventory.slots[s];
                stack.damage > 0 && stack.enchants.level_of("minecraft:mending") > 0
            })
            .collect();
        if mendable.is_empty() {
            return xp;
        }
        let slot = mendable[self.rng.below(mendable.len() as u32) as usize];
        let p = self.players.get_mut(&id).expect("player exists");
        let stack = &mut p.inventory.slots[slot];
        let repair = (xp * 2).min(stack.damage);
        stack.damage -= repair;
        let update = p.inventory.set_slot_packet(slot);
        self.send(id, update);
        xp - (repair + 1) / 2
    }

    /// Orbs lying about, for a player who just joined.
    pub(super) fn orb_packets(&self) -> Vec<Writer> {
        self.orbs
            .iter()
            .flat_map(|(&id, orb)| orb_packets(id, orb))
            .collect()
    }
}

fn orb_packets(id: i32, orb: &XpOrb) -> [Writer; 2] {
    let v = |x: f64| (x.clamp(-3.9, 3.9) * 8000.0) as i16;
    [
        Writer::packet(clientbound::ADD_ENTITY)
            .varint(id)
            .uuid(id as u128)
            .varint(entity_type::EXPERIENCE_ORB)
            .f64(orb.pos[0])
            .f64(orb.pos[1])
            .f64(orb.pos[2])
            .u8(0)
            .u8(0)
            .u8(0)
            .varint(0)
            .i16(v(orb.vel[0]))
            .i16(v(orb.vel[1]))
            .i16(v(orb.vel[2])),
        Writer::packet(clientbound::SET_ENTITY_DATA)
            .varint(id)
            .u8(DATA_ORB_VALUE)
            .varint(SERIALIZER_INT)
            .varint(orb.value)
            .u8(0xFF),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_cost_more_as_they_rise() {
        assert_eq!(points_for_level(0), 7);
        assert_eq!(points_for_level(15), 37);
        assert_eq!(points_for_level(30), 112);
        // Seven points take a fresh player from level 0 to 1.
        let total_to_30: i32 = (0..30).map(points_for_level).sum();
        assert_eq!(total_to_30, 1395);
    }

    #[test]
    fn ores_give_experience_but_iron_does_not() {
        let mut rng = Rng::new(7);
        for _ in 0..50 {
            let d = ore_experience("minecraft:deepslate_diamond_ore", &mut rng);
            assert!((3..=7).contains(&d));
        }
        assert_eq!(ore_experience("minecraft:iron_ore", &mut rng), 0);
    }
}
