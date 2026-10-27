//! Arrows: shot from bows by players and skeletons, flying under gravity, hurting what
//! they hit and sticking in blocks.
//!
//! Behaviour follows the Minecraft Wiki: an arrow loses 1% of its speed each tick and
//! falls 0.05 blocks per tick faster; on a hit it deals its base damage (2) times its
//! speed, rounded up, plus a random bonus when the bow was fully drawn. Arrows stuck in
//! blocks can be picked up when a player shot them, and vanish after a minute.

use levitas_protocol::angle;
use levitas_registry::{collision_boxes, entity_type};

use super::*;
use crate::world::survival::Damage;

const GRAVITY: f64 = 0.05;
const DRAG: f64 = 0.99;
const BASE_DAMAGE: f64 = 2.0;
/// Ticks an arrow stays stuck in a block.
const STUCK_TICKS: u32 = 1200;
/// Ticks before an arrow can hit the one who shot it (it starts inside them).
const SAFE_FROM_SHOOTER: u32 = 5;
/// Fraction of a block between two collision checks along the flight.
const SAMPLE: f64 = 0.1;
/// Ticks of drawing that make a fully drawn bow.
const FULL_DRAW: f64 = 20.0;

/// How an arrow leaves the bow: from where, which way, how fast (blocks per tick), how
/// wildly (0 is perfect aim), and whether the bow was fully drawn.
pub struct Shot {
    pub from: [f64; 3],
    pub dir: [f64; 3],
    pub speed: f64,
    pub inaccuracy: f64,
    pub critical: bool,
}

pub struct Arrow {
    pos: [f64; 3],
    vel: [f64; 3],
    /// Entity ID of whoever shot it, and whether that was a player (who may pick it up).
    shooter: i32,
    by_player: bool,
    critical: bool,
    stuck: Option<u32>,
    age: u32,
}

/// Who an arrow hits.
enum Hit {
    Block,
    Player(i32),
    Mob(i32),
}

impl World {
    /// Shoots an arrow for `shooter` (a player when `by_player`).
    pub(super) fn shoot_arrow(&mut self, shooter: i32, by_player: bool, shot: Shot) {
        let Shot {
            from,
            dir,
            speed,
            inaccuracy,
            critical,
        } = shot;
        let length = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2])
            .sqrt()
            .max(1e-9);
        let mut d = dir.map(|v| v / length);
        for v in &mut d {
            // A rough normal spread from the sum of uniform rolls.
            let spread: f64 = (0..4).map(|_| self.rng.f64() - 0.5).sum::<f64>() * 2.0;
            *v += spread * 0.0075 * inaccuracy;
        }
        let vel = d.map(|v| v * speed);
        let id = next_entity_id();
        let arrow = Arrow {
            pos: from,
            vel,
            shooter,
            by_player,
            critical,
            stuck: None,
            age: 0,
        };
        self.broadcast(arrow_spawn(id, &arrow), None);
        self.arrows.insert(id, arrow);
        let pitch = 1.0 / (self.rng.f64() as f32 * 0.4 + 1.2) + 0.5;
        self.play_sound_at("minecraft:entity.arrow.shoot", from, 1.0, pitch, None);
    }

    pub(super) fn tick_arrows(&mut self) {
        let ids: Vec<i32> = self.arrows.keys().copied().collect();
        for id in ids {
            let Some(mut arrow) = self.arrows.remove(&id) else {
                continue;
            };
            arrow.age += 1;
            if let Some(ticks) = arrow.stuck.as_mut() {
                *ticks += 1;
                if *ticks >= STUCK_TICKS || self.pick_up_arrow(id, &arrow) {
                    self.broadcast(items::remove_entity(id), None);
                    continue;
                }
                self.arrows.insert(id, arrow);
                continue;
            }
            if arrow.age > STUCK_TICKS || arrow.pos[1] < f64::from(MIN_Y - 64) {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }
            match self.fly(&mut arrow) {
                None => {
                    arrow.vel = arrow.vel.map(|v| v * DRAG);
                    arrow.vel[1] -= GRAVITY;
                    self.arrows.insert(id, arrow);
                }
                Some(Hit::Block) => {
                    arrow.stuck = Some(0);
                    arrow.vel = [0.0; 3];
                    self.play_sound_at("minecraft:entity.arrow.hit", arrow.pos, 1.0, 1.1, None);
                    // Tell clients where it ended up, in case their flight differed.
                    self.broadcast(
                        Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                            .varint(id)
                            .f64(arrow.pos[0])
                            .f64(arrow.pos[1])
                            .f64(arrow.pos[2])
                            .f64(0.0)
                            .f64(0.0)
                            .f64(0.0)
                            .f32(0.0)
                            .f32(0.0)
                            .bool(false),
                        None,
                    );
                    self.arrows.insert(id, arrow);
                }
                Some(hit) => {
                    self.arrow_hits(&arrow, hit);
                    self.broadcast(items::remove_entity(id), None);
                }
            }
        }
    }

    /// Moves an arrow along its velocity, stopping at the first block or entity in the
    /// way.
    fn fly(&mut self, arrow: &mut Arrow) -> Option<Hit> {
        let speed = arrow.vel.iter().map(|v| v * v).sum::<f64>().sqrt();
        let steps = (speed / SAMPLE).ceil().max(1.0) as usize;
        let step = arrow.vel.map(|v| v / steps as f64);
        for _ in 0..steps {
            let next = [0, 1, 2].map(|i| arrow.pos[i] + step[i]);
            if let Some(hit) = self.entity_at(next, arrow) {
                arrow.pos = next;
                return Some(hit);
            }
            if self.point_in_block(next) {
                return Some(Hit::Block);
            }
            arrow.pos = next;
        }
        None
    }

    /// Whether a point lies inside a block's collision shape (or an unloaded chunk).
    pub(super) fn point_in_block(&self, at: [f64; 3]) -> bool {
        let pos = BlockPos::new(
            at[0].floor() as i32,
            at[1].floor() as i32,
            at[2].floor() as i32,
        );
        if !(MIN_Y..=MAX_Y).contains(&pos.y) {
            return false;
        }
        let Some(chunk) = self.chunks.get(&pos.chunk()) else {
            return true;
        };
        let state = chunk.get((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize);
        let local = [
            at[0] - f64::from(pos.x),
            at[1] - f64::from(pos.y),
            at[2] - f64::from(pos.z),
        ];
        collision_boxes(state)
            .iter()
            .any(|b| (0..3).all(|i| local[i] >= f64::from(b[i]) && local[i] <= f64::from(b[i + 3])))
    }

    /// A player or mob whose hitbox contains `at`, other than the shooter early on.
    fn entity_at(&self, at: [f64; 3], arrow: &Arrow) -> Option<Hit> {
        let skip_shooter = arrow.age <= SAFE_FROM_SHOOTER;
        let inside = |pos: [f64; 3], width: f64, height: f64| {
            let half = width / 2.0 + 0.15; // the arrow's own size
            (at[0] - pos[0]).abs() <= half
                && (at[2] - pos[2]).abs() <= half
                && at[1] >= pos[1] - 0.15
                && at[1] <= pos[1] + height + 0.15
        };
        for (&id, p) in &self.players {
            if p.alive && !(skip_shooter && id == arrow.shooter) && inside(p.pos, 0.6, 1.8) {
                return Some(Hit::Player(id));
            }
        }
        for (&id, m) in &self.mobs {
            let (width, height) = m.kind.size();
            if m.alive() && !(skip_shooter && id == arrow.shooter) && inside(m.pos, width, height) {
                return Some(Hit::Mob(id));
            }
        }
        None
    }

    fn arrow_hits(&mut self, arrow: &Arrow, hit: Hit) {
        let speed = arrow.vel.iter().map(|v| v * v).sum::<f64>().sqrt();
        let mut damage = (speed * BASE_DAMAGE).ceil();
        if arrow.critical {
            damage += f64::from(self.rng.below(damage as u32 / 2 + 2));
        }
        let damage = damage.min(f64::from(i32::MAX)) as f32;
        let horizontal = arrow.vel[0].hypot(arrow.vel[2]).max(1e-9);
        let away = [arrow.vel[0] / horizontal, arrow.vel[2] / horizontal];
        let shooter_name = if arrow.by_player {
            self.players
                .get(&arrow.shooter)
                .map_or("an arrow", |_| "a player")
        } else {
            self.mobs
                .get(&arrow.shooter)
                .map_or("Skeleton", |m| m.kind.display())
        };
        match hit {
            Hit::Player(id) => {
                self.damage_from(id, damage, Damage::Arrow(shooter_name), Some(arrow.shooter));
                self.push_player(id, [away[0] * 0.3, 0.1, away[1] * 0.3]);
            }
            Hit::Mob(id) => {
                let by = arrow.by_player.then_some(arrow.shooter);
                self.hit_mob(id, damage, by, away, "minecraft:arrow");
            }
            Hit::Block => {}
        }
        self.play_sound_at("minecraft:entity.arrow.hit", arrow.pos, 1.0, 1.1, None);
    }

    /// A player walking over an arrow they may take picks it up. Returns whether one did.
    fn pick_up_arrow(&mut self, id: i32, arrow: &Arrow) -> bool {
        if !arrow.by_player {
            return false;
        }
        let Some(item) = ItemInfo::id_of("minecraft:arrow") else {
            return false;
        };
        let taker = self.players.iter().find(|(_, p)| {
            p.alive
                && (p.pos[0] - arrow.pos[0]).abs() < 1.0
                && (p.pos[2] - arrow.pos[2]).abs() < 1.0
                && (p.pos[1] + 0.9 - arrow.pos[1]).abs() < 1.5
        });
        let Some((&pid, _)) = taker else {
            return false;
        };
        let p = self.players.get_mut(&pid).expect("found");
        if p.survival() {
            let (changed, left) = p.inventory.add(Stack::new(item, 1));
            if left > 0 {
                return false;
            }
            let updates: Vec<Writer> = changed
                .into_iter()
                .map(|slot| p.inventory.set_slot_packet(slot))
                .collect();
            for update in updates {
                self.send(pid, update);
            }
        }
        self.broadcast(
            Writer::packet(clientbound::TAKE_ITEM_ENTITY)
                .varint(id)
                .varint(pid)
                .varint(1),
            None,
        );
        true
    }

    /// Arrows in flight or stuck, for a player who just joined.
    pub(super) fn arrow_packets(&self) -> Vec<Writer> {
        self.arrows
            .iter()
            .map(|(&id, arrow)| arrow_spawn(id, arrow))
            .collect()
    }

    /// A player starts drawing a bow; it shoots when they let go. Returns whether the
    /// item was a bow they can use (they need arrows unless in creative).
    pub(super) fn start_drawing(&mut self, id: i32, hand: i32) -> bool {
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        let held = p.inventory.slots[p.hand_slot(hand)];
        if ItemInfo::get(held.item).map(|i| i.name) != Some("minecraft:bow") {
            return false;
        }
        let has_arrows = !p.survival() || find_arrows(p).is_some();
        if p.alive && has_arrows {
            p.drawing = Some((hand, tick));
        }
        true
    }

    /// A player lets go of a drawn bow: the longer it was drawn (up to a second), the
    /// faster the arrow flies.
    pub(super) fn release_bow(&mut self, id: i32) {
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let Some((hand, start)) = p.drawing.take() else {
            return;
        };
        let f = (tick.saturating_sub(start) as f64) / FULL_DRAW;
        let power = ((f * f + f * 2.0) / 3.0).min(1.0);
        if power < 0.1 {
            return;
        }
        let survival = p.survival();
        let arrow_slot = find_arrows(p);
        if survival && arrow_slot.is_none() {
            return;
        }
        let bow_slot = p.hand_slot(hand);
        let (eye, look, _) = p.eye_ray();
        if survival {
            let slot = arrow_slot.expect("checked");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
            self.damage_item(id, bow_slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
        }
        let shot = Shot {
            from: eye,
            dir: look,
            speed: power * 3.0,
            inaccuracy: 1.0,
            critical: power >= 1.0,
        };
        self.shoot_arrow(id, true, shot);
    }
}

/// The inventory slot of the arrows a bow uses: offhand first, then the hotbar and the
/// rest of the inventory.
fn find_arrows(p: &Player) -> Option<usize> {
    let arrow = ItemInfo::id_of("minecraft:arrow")?;
    std::iter::once(OFFHAND)
        .chain(HOTBAR..HOTBAR + 9)
        .chain(9..HOTBAR)
        .find(|&slot| p.inventory.slots[slot].item == arrow && !p.inventory.slots[slot].is_empty())
}

fn arrow_spawn(id: i32, arrow: &Arrow) -> Writer {
    let v = |x: f64| (x.clamp(-3.9, 3.9) * 8000.0) as i16;
    let yaw = (arrow.vel[0].atan2(arrow.vel[2])).to_degrees() as f32;
    let pitch = (arrow.vel[1].atan2(arrow.vel[0].hypot(arrow.vel[2]))).to_degrees() as f32;
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::ARROW)
        .f64(arrow.pos[0])
        .f64(arrow.pos[1])
        .f64(arrow.pos[2])
        .u8(angle(pitch))
        .u8(angle(yaw))
        .u8(0)
        .varint(arrow.shooter + 1)
        .i16(v(arrow.vel[0]))
        .i16(v(arrow.vel[1]))
        .i16(v(arrow.vel[2]))
}
