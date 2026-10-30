//! Thrown items: ender pearls, snowballs and eggs, following the Minecraft Wiki.
//!
//! They fly from the thrower's eyes at 1.5 blocks per tick, falling 0.03 and keeping 99%
//! of their speed each tick. A pearl teleports its thrower to where it lands, for 5
//! damage, and can be thrown once a second. A snowball pushes what it hits and hurts
//! blazes; an egg hatches a chick one time in eight. Splash potions (and, for now,
//! lingering ones) break where they land and give players near their effects.

use levitas_registry::entity_type;

use super::mobs::{Mob, MobKind};
use super::survival::Damage;
use super::*;

const SPEED: f64 = 1.5;
const GRAVITY: f64 = 0.03;
const DRAG: f64 = 0.99;
const SAMPLE: f64 = 0.25;
const SAFE_FROM_THROWER: u32 = 5;
const LIFETIME: u32 = 1200;
const PEARL_DAMAGE: f32 = 5.0;
const PEARL_COOLDOWN: u64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrownKind {
    EnderPearl,
    Snowball,
    Egg,
    SplashPotion,
    LingeringPotion,
}

impl ThrownKind {
    pub fn of_item(name: &str) -> Option<ThrownKind> {
        match name {
            "minecraft:ender_pearl" => Some(ThrownKind::EnderPearl),
            "minecraft:snowball" => Some(ThrownKind::Snowball),
            "minecraft:egg" => Some(ThrownKind::Egg),
            "minecraft:splash_potion" => Some(ThrownKind::SplashPotion),
            "minecraft:lingering_potion" => Some(ThrownKind::LingeringPotion),
            _ => None,
        }
    }

    fn entity_type(self) -> i32 {
        match self {
            ThrownKind::EnderPearl => entity_type::ENDER_PEARL,
            ThrownKind::Snowball => entity_type::SNOWBALL,
            ThrownKind::Egg => entity_type::EGG,
            ThrownKind::SplashPotion => entity_type::SPLASH_POTION,
            ThrownKind::LingeringPotion => entity_type::LINGERING_POTION,
        }
    }

    fn sound(self) -> &'static str {
        match self {
            ThrownKind::EnderPearl => "minecraft:entity.ender_pearl.throw",
            ThrownKind::Snowball => "minecraft:entity.snowball.throw",
            ThrownKind::Egg => "minecraft:entity.egg.throw",
            ThrownKind::SplashPotion | ThrownKind::LingeringPotion => {
                "minecraft:entity.splash_potion.throw"
            }
        }
    }
}

pub struct Thrown {
    kind: ThrownKind,
    pos: [f64; 3],
    vel: [f64; 3],
    thrower: i32,
    age: u32,
    /// The thrown item, shown in flight (a potion's bottle colour).
    stack: Stack,
}

#[derive(Clone, Copy)]
enum Hit {
    Block,
    Player(i32),
    Mob(i32),
}

fn spawn_packet(id: i32, t: &Thrown) -> Writer {
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(t.kind.entity_type())
        .f64(t.pos[0])
        .f64(t.pos[1])
        .f64(t.pos[2])
        .lp_vec3([t.vel[0], t.vel[1], t.vel[2]])
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(t.thrower)
}

/// The thrown item, which the client draws (data index 8, an item stack).
fn item_data(id: i32, t: &Thrown) -> Writer {
    const DATA_ITEM: u8 = 8;
    const SERIALIZER_ITEM_STACK: i32 = 7;
    let packet = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_ITEM)
        .varint(SERIALIZER_ITEM_STACK);
    crate::inventory::write_stack(packet, t.stack).u8(0xFF)
}

impl World {
    /// Throws what player `id` holds in `hand`, if it is something thrown. Returns
    /// whether it was.
    pub(super) fn throw_held(&mut self, id: i32, hand: i32) -> bool {
        let tick = self.tick;
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let slot = p.hand_slot(hand);
        let stack = p.inventory.slots[slot];
        let Some(kind) = ItemInfo::get(stack.item).and_then(|i| ThrownKind::of_item(i.name)) else {
            return false;
        };
        if !p.alive || stack.is_empty() {
            return true;
        }
        if kind == ThrownKind::EnderPearl && tick < p.pearl_ready {
            return true;
        }
        let (eye, look, _) = p.eye_ray();
        let survival = p.survival();
        let thrown = Thrown {
            kind,
            pos: eye,
            vel: look.map(|v| v * SPEED),
            thrower: id,
            age: 0,
            stack: stack.with_count(1),
        };
        let t_id = next_entity_id();
        self.broadcast(spawn_packet(t_id, &thrown), None);
        self.broadcast(item_data(t_id, &thrown), None);
        self.thrown.insert(t_id, thrown);
        let pitch = 0.4 / (self.rng.f64() as f32 * 0.4 + 0.8);
        self.play_sound_at(kind.sound(), eye, 0.5, pitch, None);
        let p = self.players.get_mut(&id).expect("player exists");
        if survival {
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        if kind == ThrownKind::EnderPearl {
            let p = self.players.get_mut(&id).expect("player exists");
            p.pearl_ready = tick + PEARL_COOLDOWN;
            // Greys the pearl out in the hotbar for the second it cannot be thrown.
            self.send(
                id,
                Writer::packet(clientbound::COOLDOWN)
                    .string("minecraft:ender_pearl")
                    .varint(PEARL_COOLDOWN as i32),
            );
        }
        true
    }

    /// Thrown things in flight, for a player who just arrived.
    pub(super) fn thrown_packets(&self) -> Vec<Writer> {
        self.thrown
            .iter()
            .flat_map(|(&id, t)| [spawn_packet(id, t), item_data(id, t)])
            .collect()
    }

    pub(super) fn tick_thrown(&mut self) {
        let ids: Vec<i32> = self.thrown.keys().copied().collect();
        for id in ids {
            let Some(mut t) = self.thrown.remove(&id) else {
                continue;
            };
            t.age += 1;
            if t.age > LIFETIME || t.pos[1] < f64::from(MIN_Y - 64) {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }
            match self.thrown_flight(&mut t) {
                Some(hit) => {
                    self.broadcast(items::remove_entity(id), None);
                    self.thrown_hits(&t, hit);
                }
                None => {
                    t.vel = t.vel.map(|v| v * DRAG);
                    t.vel[1] -= GRAVITY;
                    self.thrown.insert(id, t);
                }
            }
        }
    }

    fn thrown_flight(&mut self, t: &mut Thrown) -> Option<Hit> {
        let speed = (t.vel[0] * t.vel[0] + t.vel[1] * t.vel[1] + t.vel[2] * t.vel[2]).sqrt();
        let steps = (speed / SAMPLE).ceil().max(1.0) as usize;
        let step = t.vel.map(|v| v / steps as f64);
        for _ in 0..steps {
            let next = [0, 1, 2].map(|i| t.pos[i] + step[i]);
            let skip = t.age <= SAFE_FROM_THROWER;
            let inside = |pos: [f64; 3], width: f64, height: f64| {
                let half = width / 2.0 + 0.125;
                (next[0] - pos[0]).abs() <= half
                    && (next[2] - pos[2]).abs() <= half
                    && next[1] >= pos[1] - 0.125
                    && next[1] <= pos[1] + height + 0.125
            };
            let player = self
                .players
                .iter()
                .find(|(&id, p)| p.alive && !(skip && id == t.thrower) && inside(p.pos, 0.6, 1.8));
            if let Some((&id, _)) = player {
                t.pos = next;
                return Some(Hit::Player(id));
            }
            let mob = self.mobs.iter().find(|(_, m)| {
                let (w, h) = m.size();
                m.alive() && inside(m.pos, w, h)
            });
            if let Some((&id, _)) = mob {
                t.pos = next;
                return Some(Hit::Mob(id));
            }
            if self.point_in_block(next) {
                return Some(Hit::Block);
            }
            t.pos = next;
        }
        None
    }

    fn thrown_hits(&mut self, t: &Thrown, hit: Hit) {
        let thrower = self.players.contains_key(&t.thrower).then_some(t.thrower);
        let horizontal = t.vel[0].hypot(t.vel[2]).max(1e-9);
        let away = [t.vel[0] / horizontal, t.vel[2] / horizontal];
        match (t.kind, hit) {
            (ThrownKind::Snowball | ThrownKind::Egg, Hit::Mob(id)) => {
                // Snowballs hurt blazes; otherwise just a nudge.
                let blaze = self.mobs.get(&id).is_some_and(|m| m.kind == MobKind::Blaze);
                let damage = if blaze && t.kind == ThrownKind::Snowball {
                    3.0
                } else {
                    0.001
                };
                self.hit_mob(id, damage, thrower, away, "minecraft:thrown");
            }
            (ThrownKind::Snowball | ThrownKind::Egg, Hit::Player(id)) => {
                self.push_player(id, [away[0] * 0.2, 0.1, away[1] * 0.2]);
            }
            _ => {}
        }
        match t.kind {
            ThrownKind::SplashPotion | ThrownKind::LingeringPotion => {
                let direct = match hit {
                    Hit::Player(id) => Some(id),
                    _ => None,
                };
                self.play_sound_at(
                    "minecraft:entity.splash_potion.break",
                    t.pos,
                    1.0,
                    1.0,
                    None,
                );
                self.splash(t.pos, t.stack.potion, direct);
            }
            ThrownKind::EnderPearl => {
                let Some(id) = thrower.filter(|id| self.players[id].alive) else {
                    return;
                };
                self.play_sound_at("minecraft:entity.player.teleport", t.pos, 1.0, 1.0, None);
                self.teleport(id, t.pos);
                self.damage(id, PEARL_DAMAGE, Damage::Fall);
            }
            ThrownKind::Egg if self.rng.below(8) == 0 => {
                // One time in 32 of those, four chicks.
                let chicks = if self.rng.below(32) == 0 { 4 } else { 1 };
                for _ in 0..chicks {
                    let mut chick = Mob::new(MobKind::Chicken, t.pos, &mut self.rng);
                    chick.age = -24_000;
                    self.mobs.insert(next_entity_id(), chick);
                }
            }
            _ => {}
        }
    }
}
