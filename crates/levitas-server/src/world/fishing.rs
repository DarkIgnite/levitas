//! Fishing, following the Minecraft Wiki: a fishing rod casts a bobber, which floats once
//! it lands in water. After 5 to 30 seconds (5 seconds less per level of Lure) a fish
//! bites: the bobber dips, and reeling in within the next second or two catches
//! something from the `gameplay/fishing` loot table (fish mostly, junk sometimes,
//! treasure in open water; Luck of the Sea shifts the odds towards treasure), with 1 to 6
//! experience; the catch flies to the angler. Rain over open water brings fish sooner,
//! a roof over the water later, and a trail of bubbles shows a fish coming. A cast can
//! also hook a mob or a dropped item, which reeling in pulls towards the angler. Reeling
//! in wears the rod: 1 for a catch, 2 from the ground, 3 for an item, 5 for a mob. The
//! bobber goes when its owner stops holding the rod, dies or gets more than 32 blocks
//! away.

use levitas_registry::{custom_stat, entity_type, stat_type};

use super::*;

const DATA_HOOKED: u8 = 8;
const DATA_BITING: u8 = 9;
const SERIALIZER_VARINT: i32 = 1;
const SERIALIZER_BOOLEAN: i32 = 8;
const CAST_SPEED: f64 = 0.9;
const GRAVITY: f64 = 0.03;
const DRAG: f64 = 0.92;
/// Ticks before a fish bites: 5 to 30 seconds, less 5 seconds a level of Lure.
const WAIT_MIN: u32 = 100;
const WAIT_SPREAD: u32 = 500;
const LURE_TICKS: u32 = 100;
/// How long the bobber stays down for the catch.
const BITE_MIN: u32 = 20;
const BITE_SPREAD: u32 = 20;
const MAX_DISTANCE: f64 = 32.0;
/// How long before a bite the fish's bubble trail shows.
const TRAIL_TICKS: u32 = 60;

/// What a bobber caught on its way: a mob or a dropped item, by entity ID.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hooked {
    Mob(i32),
    Item(i32),
}

/// A cast bobber.
pub struct Bobber {
    pub owner: i32,
    pub pos: [f64; 3],
    vel: [f64; 3],
    /// Floating in water.
    afloat: bool,
    /// Ticks until a fish bites, and ticks left of a bite.
    wait: u32,
    biting: u32,
    lure: u32,
    luck: u32,
    age: u32,
    hooked: Option<Hooked>,
    /// Lying on a block.
    grounded: bool,
    /// The way the next fish comes from.
    trail_angle: f64,
}

fn spawn_packets(id: i32, b: &Bobber) -> Vec<Writer> {
    vec![Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::FISHING_BOBBER)
        .f64(b.pos[0])
        .f64(b.pos[1])
        .f64(b.pos[2])
        .lp_vec3(b.vel)
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(b.owner)] // the bobber's data is its owner
}

/// The bobber dipping as a fish bites (or coming back up).
fn biting_data(id: i32, biting: bool) -> Writer {
    bobber_data(id, 0, biting)
}

/// What the bobber holds (entity ID + 1, 0 for nothing) and whether a fish bites.
fn bobber_data(id: i32, hooked: i32, biting: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_HOOKED)
        .varint(SERIALIZER_VARINT)
        .varint(hooked)
        .u8(DATA_BITING)
        .varint(SERIALIZER_BOOLEAN)
        .bool(biting)
        .u8(0xFF)
}

/// A few particles of `kind` at `at` (the 26.3 Particle packet).
fn particles(kind: i32, at: [f64; 3], count: i32, spread: f32, speed: f32) -> Writer {
    Writer::packet(clientbound::LEVEL_PARTICLES)
        .varint(kind)
        .bool(false) // override limiter
        .bool(false) // always show
        .f64(at[0])
        .f64(at[1])
        .f64(at[2])
        .f32(spread)
        .f32(0.0)
        .f32(spread)
        .f32(speed)
        .f32(speed)
        .f32(speed)
        .varint(count)
        .varint(0) // default randomization
}

fn sync(id: i32, b: &Bobber) -> Writer {
    Writer::packet(clientbound::ENTITY_POSITION_SYNC)
        .varint(id)
        .varint(0) // a linear path
        .f64(b.pos[0])
        .f64(b.pos[1])
        .f64(b.pos[2])
        .f32(0.0)
        .f32(0.0)
        .bool(false)
}

impl World {
    fn holds_rod(&self, id: i32) -> Option<usize> {
        let p = self.players.get(&id)?;
        [0, 1]
            .into_iter()
            .map(|hand| p.hand_slot(hand))
            .find(|&slot| {
                ItemInfo::get(p.inventory.slots[slot].item).map(|i| i.name)
                    == Some("minecraft:fishing_rod")
            })
    }

    fn bobber_of(&self, owner: i32) -> Option<i32> {
        self.bobbers
            .iter()
            .find(|(_, b)| b.owner == owner)
            .map(|(&id, _)| id)
    }

    /// Using a fishing rod: casts a bobber, or reels the one out in. Returns whether the
    /// item was a rod.
    pub(super) fn use_fishing_rod(&mut self, id: i32, hand: i32) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let slot = p.hand_slot(hand);
        let rod = p.inventory.slots[slot];
        if ItemInfo::get(rod.item).map(|i| i.name) != Some("minecraft:fishing_rod") {
            return false;
        }
        if !p.alive {
            return true;
        }
        if let Some(bobber) = self.bobber_of(id) {
            self.reel_in(id, slot, bobber);
            return true;
        }
        let (eye, look, _) = p.eye_ray();
        let bobber = Bobber {
            owner: id,
            pos: eye,
            vel: [
                look[0] * CAST_SPEED,
                look[1] * CAST_SPEED + 0.1,
                look[2] * CAST_SPEED,
            ],
            afloat: false,
            wait: 0,
            biting: 0,
            lure: rod.enchants.level_of("minecraft:lure"),
            luck: rod.enchants.level_of("minecraft:luck_of_the_sea"),
            age: 0,
            hooked: None,
            grounded: false,
            trail_angle: 0.0,
        };
        let bobber_id = next_entity_id();
        for packet in spawn_packets(bobber_id, &bobber) {
            self.broadcast(packet, None);
        }
        self.bobbers.insert(bobber_id, bobber);
        let pitch = 0.4 / (self.rng.f64() as f32 * 0.4 + 0.8);
        self.play_sound_at(
            "minecraft:entity.fishing_bobber.throw",
            eye,
            0.5,
            pitch,
            None,
        );
        true
    }

    /// Reels the bobber in: a fish that is biting is caught.
    fn reel_in(&mut self, id: i32, slot: usize, bobber_id: i32) {
        let Some(b) = self.bobbers.remove(&bobber_id) else {
            return;
        };
        self.broadcast(items::remove_entity(bobber_id), None);
        let at = self.players.get(&id).map_or(b.pos, |p| p.pos);
        self.play_sound_at(
            "minecraft:entity.fishing_bobber.retrieve",
            at,
            1.0,
            1.0,
            None,
        );
        let survival = self.players.get(&id).is_some_and(|p| p.survival());
        let event = if slot == OFFHAND {
            48 // off hand broke
        } else {
            ENTITY_EVENT_MAINHAND_BREAK
        };
        // What was hooked comes towards the angler.
        let pull = |from: [f64; 3]| {
            [
                (at[0] - from[0]) * 0.1,
                (at[1] - from[1]) * 0.1,
                (at[2] - from[2]) * 0.1,
            ]
        };
        let wear = match b.hooked {
            Some(Hooked::Mob(m)) => {
                if let Some(mob) = self.mobs.get_mut(&m) {
                    let by = pull(mob.pos);
                    mob.push(by);
                }
                5
            }
            Some(Hooked::Item(i)) => {
                self.pull_item(i, pull(b.pos));
                3
            }
            None if b.grounded => 2,
            None => 0,
        };
        if wear > 0 {
            if survival {
                self.damage_item(id, slot, wear, event);
            }
            return;
        }
        if b.biting == 0 {
            return;
        }
        let open_water = self.open_water(b.pos);
        let mut ctx = crate::loot::LootContext {
            state: 0,
            tool: self.players[&id].inventory.slots[slot].item,
            rng: &mut self.rng,
            enchants: self.players[&id].inventory.slots[slot].enchants,
            entity: crate::loot::EntityProps {
                in_open_water: open_water,
                luck: b.luck as i32,
                ..Default::default()
            },
        };
        let catch = self.loot.fishing(&mut ctx);
        // The catch flies from the water to the angler, as vanilla throws it.
        let d = [at[0] - b.pos[0], at[1] - b.pos[1], at[2] - b.pos[2]];
        let lift = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().sqrt() * 0.08;
        let vel = [d[0] * 0.1, d[1] * 0.1 + lift, d[2] * 0.1];
        for stack in catch {
            self.spawn_item_moving(stack, b.pos, vel);
            self.advance(id, super::advancements::Trigger::FishingRodHooked(stack));
        }
        let xp = 1 + self.rng.below(6) as i32;
        self.spawn_xp(at, xp);
        self.stat(id, stat_type::CUSTOM, custom_stat::FISH_CAUGHT, 1);
        if survival {
            self.damage_item(id, slot, 1, event);
        }
    }

    /// Whether a bobber floats in open water: the water around it, two blocks out, holds
    /// nothing but water and air (vanilla's treasure condition, roughly).
    fn open_water(&self, at: [f64; 3]) -> bool {
        let centre = BlockPos::new(
            at[0].floor() as i32,
            at[1].floor() as i32,
            at[2].floor() as i32,
        );
        for dx in -2..=2 {
            for dz in -2..=2 {
                for dy in -1..=1 {
                    let pos = BlockPos::new(centre.x + dx, centre.y + dy, centre.z + dz);
                    let Some(state) = self.loaded_block(pos) else {
                        return false;
                    };
                    let water = crate::blocks::fluid::fluid_at(state)
                        .is_some_and(|f| f.fluid == crate::blocks::fluid::Fluid::Water);
                    if !water && !is_air(state) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Bobbers flying, floating, waiting for a bite and going when their owner moves on.
    pub(super) fn tick_bobbers(&mut self) {
        let ids: Vec<i32> = self.bobbers.keys().copied().collect();
        for id in ids {
            let Some(mut b) = self.bobbers.remove(&id) else {
                continue;
            };
            b.age += 1;
            let owner_ok = self.players.get(&b.owner).is_some_and(|p| {
                p.alive
                    && (0..3).map(|i| (p.pos[i] - b.pos[i]).powi(2)).sum::<f64>()
                        <= MAX_DISTANCE * MAX_DISTANCE
            }) && self.holds_rod(b.owner).is_some();
            if !owner_ok || b.pos[1] < f64::from(MIN_Y - 64) {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }
            if let Some(hooked) = b.hooked {
                self.follow_hooked(id, &mut b, hooked);
            } else if b.afloat {
                self.tick_afloat(id, &mut b);
            } else {
                self.fly_bobber(id, &mut b);
            }
            self.bobbers.insert(id, b);
        }
    }

    /// A hooked bobber hangs on to its mob or item, and drops off when that is gone.
    fn follow_hooked(&mut self, id: i32, b: &mut Bobber, hooked: Hooked) {
        let at = match hooked {
            Hooked::Mob(m) => self.mobs.get(&m).filter(|m| m.alive()).map(|m| {
                let (_, h) = m.size();
                [m.pos[0], m.pos[1] + h * 0.8, m.pos[2]]
            }),
            Hooked::Item(i) => self.items.get(&i).map(|item| item.position()),
        };
        match at {
            Some(at) => b.pos = at,
            None => {
                b.hooked = None;
                self.broadcast(bobber_data(id, 0, false), None);
            }
        }
    }

    fn fly_bobber(&mut self, id: i32, b: &mut Bobber) {
        let next = [0, 1, 2].map(|i| b.pos[i] + b.vel[i]);
        // A mob or a dropped item in the way is hooked (not the angler's own feet).
        if b.age > 2 {
            let near = |pos: [f64; 3], width: f64, height: f64| {
                let half = width / 2.0 + 0.125;
                (next[0] - pos[0]).abs() <= half
                    && (next[2] - pos[2]).abs() <= half
                    && next[1] >= pos[1] - 0.125
                    && next[1] <= pos[1] + height + 0.125
            };
            let mob = self.mobs.iter().find(|(_, m)| {
                let (w, h) = m.size();
                m.alive() && near(m.pos, w, h)
            });
            let hooked = match mob {
                Some((&m, _)) => Some((Hooked::Mob(m), m)),
                None => self
                    .items
                    .iter()
                    .find(|(_, item)| near(item.position(), 0.25, 0.25))
                    .map(|(&i, _)| (Hooked::Item(i), i)),
            };
            if let Some((hooked, target)) = hooked {
                b.hooked = Some(hooked);
                b.vel = [0.0; 3];
                self.broadcast(bobber_data(id, target + 1, false), None);
                return;
            }
        }
        if self.in_water(next) {
            // It lands and floats at the top of the water.
            b.pos = next;
            while self.in_water([b.pos[0], b.pos[1] + 1.0, b.pos[2]]) {
                b.pos[1] += 1.0;
            }
            b.pos[1] = b.pos[1].floor() + 0.85;
            b.vel = [0.0; 3];
            b.afloat = true;
            b.wait = self.bite_wait(b.lure);
            self.broadcast(sync(id, b), None);
            self.play_sound_at(
                "minecraft:entity.fishing_bobber.splash",
                b.pos,
                0.25,
                1.0,
                None,
            );
            return;
        }
        if self.point_in_block(next) {
            // It lies where it hit.
            b.vel = [0.0; 3];
            b.grounded = true;
            if b.age.is_multiple_of(20) {
                self.broadcast(sync(id, b), None);
            }
            return;
        }
        b.pos = next;
        b.vel = b.vel.map(|v| v * DRAG);
        b.vel[1] -= GRAVITY;
    }

    fn tick_afloat(&mut self, id: i32, b: &mut Bobber) {
        if !self.in_water(b.pos) {
            // The water went: it falls again.
            b.afloat = false;
            b.biting = 0;
            return;
        }
        if b.biting > 0 {
            b.biting -= 1;
            if b.biting == 0 {
                // Got away: wait for the next one.
                self.broadcast(biting_data(id, false), None);
                b.wait = self.bite_wait(b.lure);
            }
            return;
        }
        // Rain over open water brings fish sooner, a roof over the water later.
        let above = BlockPos::new(
            b.pos[0].floor() as i32,
            b.pos[1].floor() as i32 + 1,
            b.pos[2].floor() as i32,
        );
        let open_sky = self.light_at(above).is_some_and(|(sky, _)| sky >= 15);
        let mut step = 1;
        if open_sky && self.weather.is_raining() && self.rng.below(4) == 0 {
            step += 1;
        }
        if !open_sky && self.rng.below(2) == 0 {
            step -= 1;
        }
        b.wait = b.wait.saturating_sub(step);
        if b.wait == TRAIL_TICKS {
            b.trail_angle = self.rng.f64() * std::f64::consts::TAU;
        }
        if b.wait > 0 && b.wait < TRAIL_TICKS && b.wait.is_multiple_of(2) {
            // A fish swimming up: bubbles coming closer.
            let off = f64::from(b.wait) / 20.0;
            let at = [
                b.pos[0] + b.trail_angle.sin() * off,
                b.pos[1].floor() + 0.9,
                b.pos[2] + b.trail_angle.cos() * off,
            ];
            self.broadcast(
                particles(levitas_registry::particle::FISHING, at, 2, 0.05, 0.0),
                None,
            );
        }
        if b.wait == 0 {
            b.biting = BITE_MIN + self.rng.below(BITE_SPREAD);
            self.broadcast(biting_data(id, true), None);
            self.broadcast(
                particles(levitas_registry::particle::BUBBLE, b.pos, 8, 0.2, 0.1),
                None,
            );
            self.broadcast(
                particles(levitas_registry::particle::FISHING, b.pos, 8, 0.2, 0.1),
                None,
            );
            let pitch = 1.0 + (self.rng.f64() as f32 - self.rng.f64() as f32) * 0.4;
            self.play_sound_at(
                "minecraft:entity.fishing_bobber.splash",
                b.pos,
                0.25,
                pitch,
                None,
            );
        }
    }

    fn bite_wait(&mut self, lure: u32) -> u32 {
        (WAIT_MIN + self.rng.below(WAIT_SPREAD))
            .saturating_sub(lure * LURE_TICKS)
            .max(BITE_MIN)
    }

    /// Bobbers out, for a player who just arrived.
    pub(super) fn bobber_packets(&self) -> Vec<Writer> {
        self.bobbers
            .iter()
            .flat_map(|(&id, b)| spawn_packets(id, b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    const ANGLER: i32 = 9;

    /// A flat world with a 9 x 9 pond, three blocks deep, and an angler at its edge
    /// looking out over it with a fishing rod.
    fn pond() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-fishing-{}-{}",
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
        for x in 2..11 {
            for z in -4..5 {
                for y in -63..-60 {
                    w.set_block(BlockPos::new(x, y, z), block::WATER);
                }
            }
        }
        let (tx, _rx) = tokio::sync::mpsc::channel(4096);
        let profile = Profile {
            uuid: 9,
            name: "Angler".to_owned(),
            properties: Vec::new(),
        };
        let mut p = Player::new(profile, tx, [0.5, -60.0, 0.5]);
        p.yaw = -90.0; // facing +x, over the pond
        p.pitch = 30.0;
        let rod = ItemInfo::id_of("minecraft:fishing_rod").unwrap();
        let slot = p.hand_slot(0);
        p.inventory.slots[slot] = Stack::new(rod, 1);
        w.players.insert(ANGLER, p);
        w
    }

    #[test]
    fn a_cast_floats_bites_and_is_reeled_in_with_a_catch() {
        let mut w = pond();
        assert!(w.use_fishing_rod(ANGLER, 0));
        let bobber = w.bobber_of(ANGLER).expect("a bobber is out");
        for _ in 0..100 {
            w.tick_bobbers();
            if w.bobbers[&bobber].afloat {
                break;
            }
        }
        assert!(w.bobbers[&bobber].afloat, "it lands in the pond");
        assert!(w.in_water(w.bobbers[&bobber].pos));
        // A fish bites at once.
        w.bobbers.get_mut(&bobber).unwrap().wait = 1;
        w.tick_bobbers();
        assert!(w.bobbers[&bobber].biting > 0);
        let items_before = w.items.len();
        w.use_fishing_rod(ANGLER, 0);
        assert!(w.bobbers.is_empty(), "reeled in");
        assert!(w.items.len() > items_before, "something was caught");
        assert!(!w.orbs.is_empty(), "with experience");
        let slot = w.players[&ANGLER].hand_slot(0);
        assert_eq!(
            w.players[&ANGLER].inventory.slots[slot].damage, 1,
            "the rod wears"
        );
        assert_eq!(
            w.players[&ANGLER]
                .stats
                .get(stat_type::CUSTOM, custom_stat::FISH_CAUGHT),
            1
        );
    }

    #[test]
    fn a_hooked_mob_is_pulled_in_and_wears_the_rod_more() {
        let mut w = pond();
        let cow = next_entity_id();
        w.mobs.insert(
            cow,
            super::super::mobs::Mob::new(
                super::super::mobs::MobKind::Cow,
                [3.5, -60.0, 0.5],
                &mut w.rng,
            ),
        );
        w.use_fishing_rod(ANGLER, 0);
        let bobber = w.bobber_of(ANGLER).unwrap();
        let b = w.bobbers.get_mut(&bobber).unwrap();
        b.pos = [3.5, -59.5, 0.5];
        b.vel = [0.1, 0.0, 0.0];
        b.age = 5;
        w.tick_bobbers();
        assert_eq!(w.bobbers[&bobber].hooked, Some(Hooked::Mob(cow)));
        w.use_fishing_rod(ANGLER, 0);
        let slot = w.players[&ANGLER].hand_slot(0);
        assert_eq!(w.players[&ANGLER].inventory.slots[slot].damage, 5);
    }

    #[test]
    fn reeling_in_a_fish_counts_for_fishy_business() {
        let mut w = pond();
        let cod = ItemInfo::id_of("minecraft:cod").unwrap();
        w.advance(
            ANGLER,
            super::super::advancements::Trigger::FishingRodHooked(Stack::new(cod, 1)),
        );
        let a = super::super::advancements::tree()
            .index("minecraft:husbandry/fishy_business")
            .unwrap();
        assert!(w.players[&ANGLER].advancements.is_done(a));
    }

    #[test]
    fn reeling_in_without_a_bite_catches_nothing_and_a_bobber_follows_its_rod() {
        let mut w = pond();
        w.use_fishing_rod(ANGLER, 0);
        let items_before = w.items.len();
        w.use_fishing_rod(ANGLER, 0);
        assert!(w.bobbers.is_empty());
        assert_eq!(w.items.len(), items_before);
        // Putting the rod away takes the bobber back.
        w.use_fishing_rod(ANGLER, 0);
        let slot = w.players[&ANGLER].hand_slot(0);
        w.players.get_mut(&ANGLER).unwrap().inventory.slots[slot] = Stack::EMPTY;
        w.tick_bobbers();
        assert!(w.bobbers.is_empty());
    }

    #[test]
    fn the_fishing_table_gives_fish_mostly_and_treasure_only_in_open_water() {
        let loot = crate::loot::LootTables::load();
        let mut rng = Rng::new(3);
        let fish = ["cod", "salmon", "tropical_fish", "pufferfish"]
            .map(|f| ItemInfo::id_of(&format!("minecraft:{f}")).unwrap());
        // Only in the treasure table (junk has rods too).
        let treasure = ["name_tag", "saddle", "nautilus_shell"]
            .map(|f| ItemInfo::id_of(&format!("minecraft:{f}")).unwrap());
        let catch = |open: bool, rng: &mut Rng| {
            let mut ctx = crate::loot::LootContext {
                state: 0,
                tool: 0,
                rng,
                enchants: crate::enchanting::Enchantments::NONE,
                entity: crate::loot::EntityProps {
                    in_open_water: open,
                    ..Default::default()
                },
            };
            loot.fishing(&mut ctx)
        };
        let (mut fished, mut treasures) = (0, 0);
        for _ in 0..400 {
            for s in catch(false, &mut rng) {
                assert!(
                    !treasure.contains(&s.item),
                    "no treasure in a puddle: {:?}",
                    ItemInfo::get(s.item).map(|i| i.name)
                );
                fished += i32::from(fish.contains(&s.item));
            }
            for s in catch(true, &mut rng) {
                treasures += i32::from(treasure.contains(&s.item));
            }
        }
        assert!(fished > 250, "mostly fish: {fished} of 400");
        assert!(treasures > 0, "open water gives treasure now and then");
    }
}
