//! Firework rockets in flight, as the Minecraft Wiki describes them: launched from a
//! block, a dispenser or a crossbow, or used while gliding to boost the player. A
//! rocket climbs faster and faster for 10 ticks a level of flight (plus a little) and
//! bursts; with stars it hurts what is within five blocks, 5 damage plus 2 a star at
//! the centre, less further out.

use levitas_registry::entity_type;

use super::survival::Damage;
use super::*;
use crate::blocks::{offset, Dir};

const DATA_ITEM: u8 = 8;
const DATA_ATTACHED: u8 = 9;
const DATA_ANGLED: u8 = 10;
const SERIALIZER_SLOT: i32 = 7;
const SERIALIZER_BOOLEAN: i32 = 8;
const SERIALIZER_OPTIONAL_VARINT: i32 = 19;
const ENTITY_EVENT_EXPLODE: i8 = 17;
/// How far a burst reaches.
const BLAST_RANGE: f64 = 5.0;

pub struct Rocket {
    pub pos: [f64; 3],
    vel: [f64; 3],
    stack: Stack,
    age: u32,
    life: u32,
    shooter: Option<i32>,
    /// The gliding player it pulls along.
    attached: Option<i32>,
    /// Shot from a crossbow or dispenser: flies straight instead of climbing.
    angled: bool,
}

fn spawn_packets(id: i32, r: &Rocket) -> Vec<Writer> {
    let add = Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::FIREWORK_ROCKET)
        .f64(r.pos[0])
        .f64(r.pos[1])
        .f64(r.pos[2])
        .lp_vec3(r.vel)
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(r.shooter.map_or(0, |s| s + 1));
    let data = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_ITEM)
        .varint(SERIALIZER_SLOT);
    let data = crate::inventory::write_stack(data, r.stack)
        .u8(DATA_ATTACHED)
        .varint(SERIALIZER_OPTIONAL_VARINT)
        .varint(r.attached.map_or(0, |a| a + 1))
        .u8(DATA_ANGLED)
        .varint(SERIALIZER_BOOLEAN)
        .bool(r.angled)
        .u8(0xFF);
    vec![add, data]
}

impl World {
    /// Launches a rocket.
    pub(super) fn launch_rocket(
        &mut self,
        stack: Stack,
        pos: [f64; 3],
        vel: [f64; 3],
        shooter: Option<i32>,
        attached: Option<i32>,
        angled: bool,
    ) {
        let flight = u32::from(crate::fireworks::get(stack.firework).flight.max(1));
        let life = 10 * flight + self.rng.below(6) + self.rng.below(7);
        let rocket = Rocket {
            pos,
            vel,
            stack: stack.with_count(1),
            age: 0,
            life,
            shooter,
            attached,
            angled,
        };
        let id = next_entity_id();
        for p in spawn_packets(id, &rocket) {
            self.broadcast(p, None);
        }
        self.play_sound_at(
            "minecraft:entity.firework_rocket.launch",
            pos,
            3.0,
            1.0,
            None,
        );
        self.rockets.insert(id, rocket);
    }

    /// A rocket used on a block goes up from the face clicked. Returns whether the
    /// held item was a rocket.
    pub(super) fn use_rocket_on(&mut self, id: i32, slot: usize, pos: BlockPos, face: Dir) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let held = p.inventory.slots[slot];
        if ItemInfo::get(held.item).is_none_or(|i| i.name != "minecraft:firework_rocket") {
            return false;
        }
        let at = offset(pos, face);
        let from = [
            f64::from(at.x) + 0.5,
            f64::from(at.y) + 0.1,
            f64::from(at.z) + 0.5,
        ];
        let vel = [
            (self.rng.f64() - 0.5) * 0.002,
            0.05,
            (self.rng.f64() - 0.5) * 0.002,
        ];
        self.use_up(id, slot);
        self.launch_rocket(held, from, vel, Some(id), None, false);
        true
    }

    /// A rocket used while gliding pulls the player along. Returns whether it did.
    pub(super) fn boost_with_rocket(&mut self, id: i32, hand: i32) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let slot = p.hand_slot(hand);
        let held = p.inventory.slots[slot];
        if ItemInfo::get(held.item).is_none_or(|i| i.name != "minecraft:firework_rocket") {
            return false;
        }
        if !p.gliding {
            return true;
        }
        let at = p.pos;
        self.use_up(id, slot);
        self.launch_rocket(held, at, [0.0; 3], Some(id), Some(id), false);
        true
    }

    fn use_up(&mut self, id: i32, slot: usize) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if p.survival() {
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
    }

    pub(super) fn tick_rockets(&mut self) {
        let ids: Vec<i32> = self.rockets.keys().copied().collect();
        for id in ids {
            let mut r = self.rockets.remove(&id).expect("listed");
            r.age += 1;
            let mut burst = r.age >= r.life;
            if let Some(player) = r.attached {
                match self.players.get(&player) {
                    Some(p) if p.alive && p.gliding => r.pos = p.pos,
                    _ => r.attached = None,
                }
            } else {
                if !r.angled {
                    r.vel[0] *= 1.15;
                    r.vel[2] *= 1.15;
                    r.vel[1] += 0.04;
                }
                let next = [0, 1, 2].map(|i| r.pos[i] + r.vel[i]);
                if self.point_in_block(next) {
                    burst = true;
                } else {
                    r.pos = next;
                }
                // A crossbow's rocket bursts on what it hits.
                if r.angled && self.rocket_hits(&r).is_some() {
                    burst = true;
                }
            }
            if burst {
                self.burst(id, &r);
                continue;
            }
            self.rockets.insert(id, r);
        }
    }

    fn rocket_hits(&self, r: &Rocket) -> Option<i32> {
        let near = |pos: [f64; 3], h: f64| {
            (pos[0] - r.pos[0]).abs() < 0.6
                && (pos[2] - r.pos[2]).abs() < 0.6
                && (r.pos[1] - pos[1]) > -0.2
                && (r.pos[1] - pos[1]) < h
        };
        self.mobs
            .iter()
            .find(|(_, m)| m.alive() && near(m.pos, m.size().1))
            .map(|(&id, _)| id)
            .or_else(|| {
                self.players
                    .iter()
                    .find(|(&pid, p)| p.alive && Some(pid) != r.shooter && near(p.pos, 1.8))
                    .map(|(&pid, _)| pid)
            })
    }

    /// The burst: the client shows the stars; what is near is hurt if there were any.
    fn burst(&mut self, id: i32, r: &Rocket) {
        self.broadcast(
            Writer::packet(clientbound::ENTITY_EVENT)
                .i32(id)
                .i8(ENTITY_EVENT_EXPLODE),
            None,
        );
        self.broadcast(super::items::remove_entity(id), None);
        let stars = crate::fireworks::get(r.stack.firework).explosions.len();
        if stars == 0 {
            return;
        }
        let full = 5.0 + 2.0 * stars as f64;
        let at = r.pos;
        let dist = |p: [f64; 3]| {
            let d = [p[0] - at[0], p[1] + 0.9 - at[1], p[2] - at[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        };
        let players: Vec<(i32, f64)> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive)
            .map(|(&pid, p)| (pid, dist(p.pos)))
            .filter(|&(_, d)| d <= BLAST_RANGE)
            .collect();
        for (pid, d) in players {
            let damage = (full * ((BLAST_RANGE - d) / BLAST_RANGE).sqrt()) as f32;
            if damage > 0.0 {
                self.damage_from(pid, damage, Damage::Explosion("Firework"), None);
            }
        }
        let mobs: Vec<(i32, f64)> = self
            .mobs
            .iter()
            .filter(|(_, m)| m.alive())
            .map(|(&mid, m)| (mid, dist(m.pos)))
            .filter(|&(_, d)| d <= BLAST_RANGE)
            .collect();
        for (mid, d) in mobs {
            let damage = (full * ((BLAST_RANGE - d) / BLAST_RANGE).sqrt()) as f32;
            if damage > 0.0 {
                let by = r.shooter.filter(|s| self.players.contains_key(s));
                self.hit_mob(mid, damage, by, [0.0, 0.0], "minecraft:fireworks");
            }
        }
    }

    pub(super) fn rocket_packets(&self) -> Vec<Writer> {
        self.rockets
            .iter()
            .flat_map(|(&id, r)| spawn_packets(id, r))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn stack(name: &str) -> Stack {
        Stack::new(ItemInfo::id_of(name).unwrap(), 1)
    }

    #[test]
    fn stars_and_rockets_are_crafted_fly_and_burst() {
        let r = crate::recipes::Recipes::load();
        // A red creeper star with a trail.
        let mut g = vec![Stack::EMPTY; 9];
        g[0] = stack("minecraft:gunpowder");
        g[1] = stack("minecraft:red_dye");
        g[2] = stack("minecraft:creeper_head");
        g[3] = stack("minecraft:diamond");
        let star = r.craft(&g, 3);
        assert!(star.is_named("minecraft:firework_star"));
        let e = crate::fireworks::get(star.firework).explosions[0].clone();
        assert_eq!((e.shape, e.trail, e.twinkle), (3, true, false));
        assert_eq!(e.colors, vec![crate::fireworks::DYE_COLORS[14]]);
        // Faded to blue.
        let mut g = vec![Stack::EMPTY; 9];
        g[0] = star;
        g[1] = stack("minecraft:blue_dye");
        let faded = r.craft(&g, 3);
        assert_eq!(
            crate::fireworks::get(faded.firework).explosions[0]
                .fade
                .len(),
            1
        );
        // Three rockets of flight 2 with the star.
        let mut g = vec![Stack::EMPTY; 9];
        g[0] = stack("minecraft:paper");
        g[1] = stack("minecraft:gunpowder");
        g[2] = stack("minecraft:gunpowder");
        g[3] = faded;
        let rocket = r.craft(&g, 3);
        assert_eq!(rocket.count, 3);
        let f = crate::fireworks::get(rocket.firework);
        assert_eq!((f.flight, f.explosions.len()), (2, 1));
        assert_eq!(Stack::from_nbt(&rocket.to_nbt()), rocket);

        // Launched from the ground, it climbs and bursts, hurting a nearby cow.
        let dir = std::env::temp_dir().join(format!("levitas-rockets-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let id = next_entity_id();
        let profile = Profile {
            name: "Pyro".to_owned(),
            uuid: 9,
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, out, [2.5, -60.0, 2.5]));
        let slot = w.players[&id].hand_slot(0);
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = rocket.with_count(1);
        assert!(w.use_rocket_on(id, slot, BlockPos::new(6, -61, 6), Dir::Up));
        let (&rid, launched) = w.rockets.iter().next().expect("in flight");
        let start = launched.pos[1];
        let cow = next_entity_id();
        let mob = super::super::mobs::Mob::new(
            super::super::mobs::MobKind::Cow,
            [6.5, -60.0, 6.5],
            &mut w.rng,
        );
        w.mobs.insert(cow, mob);
        w.tick_rockets();
        assert!(w.rockets[&rid].pos[1] > start, "it climbs");
        // Burst right away next to the cow.
        w.rockets.get_mut(&rid).unwrap().pos = [6.5, -59.0, 6.5];
        let r2 = w.rockets.remove(&rid).unwrap();
        w.burst(rid, &r2);
        assert!(w.mobs[&cow].health < super::super::mobs::MobKind::Cow.max_health());
    }
}
