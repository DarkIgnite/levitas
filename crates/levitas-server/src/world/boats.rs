//! Boats and rafts, following the Minecraft Wiki: placed on water or ground, ridden by
//! right-clicking them and left by sneaking, steered by their rider, and broken by
//! hitting them, which drops them as items. While a player rides one, their client
//! moves the boat and reports where it went, as in vanilla; the server checks the move
//! and shows it to everyone else. A boat no one rides floats up to the water's surface
//! and falls when there is nothing under it. Chest boats are not placed yet.

use levitas_nbt::{Compound, Tag};
use levitas_registry::entity_type;

use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::blocks::Dir;

/// Entity data: the shake when hit (power, direction, damage), and the paddles.
const DATA_HURT_TIME: u8 = 8;
const DATA_HURT_DIRECTION: u8 = 9;
const DATA_DAMAGE: u8 = 10;
const DATA_PADDLE_LEFT: u8 = 11;
const DATA_PADDLE_RIGHT: u8 = 12;
const SERIALIZER_INT: i32 = 1;
const SERIALIZER_FLOAT: i32 = 3;
const SERIALIZER_BOOLEAN: i32 = 8;
/// Damage that breaks a boat; each hit adds ten times its strength.
const BREAKS_AT: f32 = 40.0;
/// How far a rider's reported move may take the boat in one packet.
const MAX_MOVE: f64 = 10.0;
/// How far below the top of a water block a floating boat's bottom sits.
const FLOAT_DEPTH: f64 = 0.15;
const GRAVITY: f64 = 0.04;

pub struct Boat {
    /// The entity and item name (`minecraft:oak_boat`, `minecraft:bamboo_raft`, ...).
    kind: &'static str,
    uuid: u128,
    pub pos: [f64; 3],
    yaw: f32,
    fall: f64,
    pub rider: Option<i32>,
    /// A second player, in the back seat.
    pub second: Option<i32>,
    /// Mobs sitting in it (picked up as they wander in).
    pub mobs: Vec<i32>,
    /// A chest boat's 27 slots.
    pub chest: Option<Box<[Stack]>>,
    damage: f32,
    hurt_direction: i32,
    sent_pos: [f64; 3],
}

/// Slots of a chest boat.
const CHEST_SLOTS: usize = 27;
/// Mobs wider than this do not fit in a boat.
const BOAT_WIDTH: f64 = 1.375;

/// The boat a placed item makes, if it is one this server places.
fn boat_kind(item: &str) -> Option<&'static str> {
    let kind = levitas_registry::ITEMS
        .iter()
        .map(|i| i.name)
        .find(|&n| n == item)?;
    (kind.ends_with("_boat") || kind.ends_with("_raft")).then_some(kind)
}

fn entity_type_of(kind: &str) -> i32 {
    entity_type::NAMES
        .iter()
        .position(|&n| n == kind)
        .map_or(entity_type::OAK_BOAT, |i| i as i32)
}

impl Boat {
    fn new(kind: &'static str, pos: [f64; 3], yaw: f32, rng: &mut Rng) -> Self {
        Boat {
            kind,
            uuid: (u128::from(rng.next_u64()) << 64) | u128::from(rng.next_u64()),
            pos,
            yaw,
            fall: 0.0,
            rider: None,
            second: None,
            mobs: Vec::new(),
            chest: kind
                .contains("chest")
                .then(|| vec![Stack::EMPTY; CHEST_SLOTS].into_boxed_slice()),
            damage: 0.0,
            hurt_direction: 1,
            sent_pos: pos,
        }
    }

    /// Seats: two, one in a chest boat.
    fn seats(&self) -> usize {
        if self.chest.is_some() {
            1
        } else {
            2
        }
    }

    fn taken(&self) -> usize {
        usize::from(self.rider.is_some()) + usize::from(self.second.is_some()) + self.mobs.len()
    }

    /// Everyone aboard, the driver first.
    fn aboard(&self) -> Vec<i32> {
        self.rider
            .into_iter()
            .chain(self.second)
            .chain(self.mobs.iter().copied())
            .collect()
    }

    pub fn chunk(&self) -> (i32, i32) {
        (
            (self.pos[0] / 16.0).floor() as i32,
            (self.pos[2] / 16.0).floor() as i32,
        )
    }

    pub fn to_nbt(&self) -> Compound {
        let uuid = [
            (self.uuid >> 96) as i32,
            (self.uuid >> 64) as i32,
            (self.uuid >> 32) as i32,
            self.uuid as i32,
        ];
        Compound::new()
            .with("id", self.kind)
            .with("UUID", uuid.to_vec())
            .with(
                "Pos",
                self.pos.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with("Motion", vec![Tag::Double(0.0); 3])
            .with("Rotation", vec![Tag::Float(self.yaw), Tag::Float(0.0)])
            .with(
                "Items",
                self.chest
                    .as_ref()
                    .map(|c| crate::inventory::items_to_nbt(c.iter().copied().enumerate()))
                    .unwrap_or_default(),
            )
    }

    pub fn from_nbt(nbt: &Compound, rng: &mut Rng) -> Option<Boat> {
        let kind = boat_kind(nbt.str("id")?)?;
        let pos: Vec<f64> = nbt.list("Pos")?.iter().filter_map(Tag::as_f64).collect();
        let &[x, y, z] = &pos[..] else {
            return None;
        };
        let yaw = match nbt.list("Rotation").and_then(|r| r.first()) {
            Some(Tag::Float(yaw)) => *yaw,
            _ => 0.0,
        };
        let mut boat = Boat::new(kind, [x, y, z], yaw, rng);
        if let Some(items) = boat.chest.as_mut() {
            for (slot, stack) in
                crate::inventory::items_from_nbt(nbt.list("Items").unwrap_or_default())
            {
                if let Some(s) = items.get_mut(slot) {
                    *s = stack;
                }
            }
        }
        if let Some(&[a, b, c, d]) = nbt.int_array("UUID") {
            boat.uuid = (u128::from(a as u32) << 96)
                | (u128::from(b as u32) << 64)
                | (u128::from(c as u32) << 32)
                | u128::from(d as u32);
        }
        Some(boat)
    }
}

pub fn spawn_packets(id: i32, boat: &Boat) -> Vec<Writer> {
    let mut packets = vec![Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(boat.uuid)
        .varint(entity_type_of(boat.kind))
        .f64(boat.pos[0])
        .f64(boat.pos[1])
        .f64(boat.pos[2])
        .lp_vec3([0.0, 0.0, 0.0])
        .u8(0)
        .u8(angle(boat.yaw))
        .u8(angle(boat.yaw))
        .varint(0)];
    if boat.taken() > 0 {
        packets.push(passengers(id, boat));
    }
    packets
}

fn passengers(id: i32, boat: &Boat) -> Writer {
    let aboard = boat.aboard();
    let mut packet = Writer::packet(clientbound::SET_PASSENGERS)
        .varint(id)
        .varint(aboard.len() as i32);
    for e in aboard {
        packet = packet.varint(e);
    }
    packet
}

fn position_sync(id: i32, boat: &Boat) -> Writer {
    Writer::packet(clientbound::ENTITY_POSITION_SYNC)
        .varint(id)
        .varint(0) // a linear path
        .f64(boat.pos[0])
        .f64(boat.pos[1])
        .f64(boat.pos[2])
        .f32(boat.yaw)
        .f32(0.0)
        .bool(false)
}

fn angle(degrees: f32) -> u8 {
    (degrees.rem_euclid(360.0) / 360.0 * 256.0) as i32 as u8
}

impl World {
    /// Puts a boat down where the player looks: on water, or on the ground. Returns
    /// whether the player held a boat.
    pub(super) fn place_boat(&mut self, id: i32, hand: i32) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let slot = p.hand_slot(hand);
        let stack = p.inventory.slots[slot];
        let Some(kind) = ItemInfo::get(stack.item).and_then(|i| boat_kind(i.name)) else {
            return false;
        };
        let (eye, look, reach) = p.eye_ray();
        let (yaw, survival) = (p.yaw, p.survival());
        let Some((pos, face)) = self.ray_cast(eye, look, reach, true) else {
            return true;
        };
        let state = self.block(pos);
        let on_water = fluid_at(state).is_some_and(|f| f.fluid == Fluid::Water);
        let y = if on_water {
            f64::from(pos.y) + 1.0 - FLOAT_DEPTH
        } else if face == Dir::Up {
            f64::from(pos.y) + 1.0
        } else {
            return true;
        };
        let at = [f64::from(pos.x) + 0.5, y, f64::from(pos.z) + 0.5];
        let boat = Boat::new(kind, at, yaw, &mut self.rng);
        let boat_id = next_entity_id();
        for packet in spawn_packets(boat_id, &boat) {
            self.broadcast(packet, None);
        }
        self.boats.insert(boat_id, boat);
        self.play_sound_at("minecraft:entity.boat.paddle_water", at, 1.0, 1.0, None);
        if survival {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        true
    }

    /// A player right-clicks a boat: they get in if it is free. Returns whether
    /// `target` is a boat.
    pub(super) fn board_boat(&mut self, id: i32, target: i32) -> bool {
        let Some(boat) = self.boats.get(&target) else {
            return false;
        };
        let Some(p) = self.players.get(&id) else {
            return true;
        };
        let d: f64 = (0..3).map(|i| (p.pos[i] - boat.pos[i]).powi(2)).sum();
        // Sneaking at a chest boat opens its chest.
        if p.sneaking && boat.chest.is_some() && d <= 36.0 && p.alive {
            self.open_view(
                id,
                super::containers::View::CartChest(target, CHEST_SLOTS),
                levitas_registry::menu::GENERIC_9X3,
                "Boat with Chest",
            );
            return true;
        }
        let full = boat.taken() >= boat.seats();
        if full || !p.alive || p.sneaking || d > 36.0 || p.riding.is_some() {
            return true;
        }
        let at = boat.pos;
        let boat = self.boats.get_mut(&target).expect("checked above");
        if boat.rider.is_none() {
            boat.rider = Some(id);
        } else {
            boat.second = Some(id);
        }
        let packet = passengers(target, boat);
        self.players.get_mut(&id).expect("checked above").riding = Some(target);
        self.broadcast(packet, None);
        self.move_player(id, Some(at), None, true);
        true
    }

    /// The player gets out of their boat, if they are in one, beside it.
    pub(super) fn get_off(&mut self, id: i32) {
        let Some(boat_id) = self.players.get_mut(&id).and_then(|p| p.riding.take()) else {
            return;
        };
        if self.leave_cart(id, boat_id) || self.dismount_horse(id, boat_id) {
            return;
        }
        let Some(boat) = self.boats.get_mut(&boat_id) else {
            return;
        };
        if boat.rider == Some(id) {
            // The back-seat player moves up to steer.
            boat.rider = boat.second.take();
        } else if boat.second == Some(id) {
            boat.second = None;
        }
        let at = [boat.pos[0], boat.pos[1] + 0.6, boat.pos[2]];
        let packet = passengers(boat_id, boat);
        self.broadcast(packet, None);
        if self.players.get(&id).is_some_and(|p| p.alive) {
            self.teleport(id, at);
        }
    }

    /// Where the rider's client moved their boat.
    pub(super) fn boat_moved(&mut self, id: i32, pos: [f64; 3], yaw: f32) {
        let Some(boat_id) = self.players.get(&id).and_then(|p| p.riding) else {
            return;
        };
        let Some(boat) = self.boats.get_mut(&boat_id) else {
            return;
        };
        // Only the driver steers.
        if boat.rider != Some(id) {
            return;
        }
        let d: f64 = (0..3).map(|i| (pos[i] - boat.pos[i]).powi(2)).sum();
        if d > MAX_MOVE * MAX_MOVE || pos.iter().any(|v| !v.is_finite()) {
            // Too far: the client is put back where the server has the boat.
            let back = Writer::packet(clientbound::MOVE_VEHICLE)
                .f64(boat.pos[0])
                .f64(boat.pos[1])
                .f64(boat.pos[2])
                .f32(boat.yaw)
                .f32(0.0);
            self.send(id, back);
            return;
        }
        boat.pos = pos;
        boat.yaw = yaw;
        boat.sent_pos = pos;
        let sync = position_sync(boat_id, boat);
        let second = boat.second;
        self.broadcast(sync, Some(id));
        self.move_player(id, Some(pos), None, true);
        if let Some(back) = second {
            self.move_player(back, Some(pos), None, true);
        }
    }

    /// The rider's paddles, shown to everyone else.
    pub(super) fn paddle(&mut self, id: i32, left: bool, right: bool) {
        let Some(boat_id) = self.players.get(&id).and_then(|p| p.riding) else {
            return;
        };
        let packet = Writer::packet(clientbound::SET_ENTITY_DATA)
            .varint(boat_id)
            .u8(DATA_PADDLE_LEFT)
            .varint(SERIALIZER_BOOLEAN)
            .bool(left)
            .u8(DATA_PADDLE_RIGHT)
            .varint(SERIALIZER_BOOLEAN)
            .bool(right)
            .u8(0xFF);
        self.broadcast(packet, Some(id));
    }

    /// A player hits a boat: it shakes, and breaks after a few hits (at once in
    /// creative), dropping itself unless broken in creative. Returns whether `target`
    /// is a boat.
    pub(super) fn hit_boat(&mut self, attacker: i32, target: i32) -> bool {
        if !self.boats.contains_key(&target) {
            return false;
        }
        let Some(p) = self.players.get(&attacker) else {
            return true;
        };
        let creative = !p.survival();
        let strength =
            ItemInfo::get(p.inventory.slots[p.hand_slot(0)].item).map_or(1.0, |i| i.attack_damage);
        let boat = self.boats.get_mut(&target).expect("checked above");
        boat.hurt_direction = -boat.hurt_direction;
        boat.damage += strength * 10.0;
        let broken = creative || boat.damage > BREAKS_AT;
        let shake = Writer::packet(clientbound::SET_ENTITY_DATA)
            .varint(target)
            .u8(DATA_HURT_TIME)
            .varint(SERIALIZER_INT)
            .varint(10)
            .u8(DATA_HURT_DIRECTION)
            .varint(SERIALIZER_INT)
            .varint(boat.hurt_direction)
            .u8(DATA_DAMAGE)
            .varint(SERIALIZER_FLOAT)
            .f32(boat.damage)
            .u8(0xFF);
        self.broadcast(shake, None);
        if broken {
            self.remove_boat(target, !creative);
        }
        true
    }

    fn remove_boat(&mut self, boat_id: i32, drop: bool) {
        for player in [
            self.boats.get(&boat_id).and_then(|b| b.second),
            self.boats.get(&boat_id).and_then(|b| b.rider),
        ]
        .into_iter()
        .flatten()
        {
            self.get_off(player);
        }
        self.close_cart_viewers(boat_id);
        let Some(boat) = self.boats.remove(&boat_id) else {
            return;
        };
        for m in &boat.mobs {
            if let Some(mob) = self.mobs.get_mut(m) {
                mob.vehicle = None;
            }
        }
        self.broadcast(items::remove_entity(boat_id), None);
        for &s in boat.chest.iter().flat_map(|c| c.iter()) {
            if !s.is_empty() {
                self.scatter_item(boat.pos, s);
            }
        }
        if drop {
            if let Some(item) = ItemInfo::id_of(boat.kind) {
                self.scatter_item(boat.pos, Stack::new(item, 1));
            }
        }
    }

    /// Boats no one rides float up to the surface or fall; hits wear off.
    pub(super) fn tick_boats(&mut self) {
        let ids: Vec<i32> = self.boats.keys().copied().collect();
        for id in ids {
            let mut boat = self.boats.remove(&id).expect("listed");
            boat.damage = (boat.damage - 1.0).max(0.0);
            if boat.rider.is_none() && self.chunks.contains_key(&boat.chunk()) {
                self.drift(&mut boat);
            }
            // Mobs that wander in sit down, while there is a seat.
            if boat.taken() < boat.seats() && self.tick.is_multiple_of(4) {
                let boarding = self.mob_boarding(boat.pos, BOAT_WIDTH, &boat.mobs);
                if let Some(mob_id) = boarding {
                    boat.mobs.push(mob_id);
                    if let Some(m) = self.mobs.get_mut(&mob_id) {
                        m.vehicle = Some(id);
                    }
                    self.broadcast(passengers(id, &boat), None);
                }
            }
            boat.mobs.retain(|m| {
                self.mobs
                    .get(m)
                    .is_some_and(|m| m.alive() && m.vehicle == Some(id))
            });
            if boat.pos[1] < f64::from(MIN_Y - 64) {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }
            let moved = (0..3).any(|i| (boat.pos[i] - boat.sent_pos[i]).abs() > 1e-4);
            if moved && boat.rider.is_none() {
                boat.sent_pos = boat.pos;
                self.broadcast(position_sync(id, &boat), None);
            }
            self.boats.insert(id, boat);
        }
    }

    fn drift(&mut self, boat: &mut Boat) {
        let block_at = |w: &World, y: f64| {
            w.loaded_state(BlockPos::new(
                boat.pos[0].floor() as i32,
                y.floor() as i32,
                boat.pos[2].floor() as i32,
            ))
        };
        let is_water = |state: Option<u16>| {
            state
                .and_then(fluid_at)
                .is_some_and(|f| f.fluid == Fluid::Water)
        };
        let here = block_at(self, boat.pos[1] + 0.05);
        if is_water(here) {
            // Under water: rise to the surface of the column.
            boat.fall = 0.0;
            let top = (boat.pos[1] + 0.05).floor();
            if is_water(block_at(self, top + 1.0)) {
                boat.pos[1] = (boat.pos[1] + 0.1).min(top + 1.0);
            } else {
                boat.pos[1] = top + 1.0 - FLOAT_DEPTH;
            }
            return;
        }
        let below = block_at(self, boat.pos[1] - 0.01);
        let resting = is_water(below)
            || below.is_some_and(|s| !levitas_registry::collision_boxes(s).is_empty());
        if resting {
            boat.fall = 0.0;
            return;
        }
        boat.fall = (boat.fall + GRAVITY).min(2.0);
        let target = boat.pos[1] - boat.fall;
        // Stop on the first thing underneath.
        let mut y = boat.pos[1];
        while y > target {
            let next = (y - 0.25).max(target);
            let s = block_at(self, next);
            if is_water(s) || s.is_some_and(|s| !levitas_registry::collision_boxes(s).is_empty()) {
                y = next.floor() + 1.0;
                if is_water(s) {
                    y -= FLOAT_DEPTH;
                }
                boat.fall = 0.0;
                break;
            }
            y = next;
        }
        boat.pos[1] = y;
    }

    /// A mob close enough to `at` to get in (narrower than `width`, not swimming or
    /// flying, not a golem, not already riding).
    pub(super) fn mob_boarding(&self, at: [f64; 3], width: f64, aboard: &[i32]) -> Option<i32> {
        self.mobs.iter().find_map(|(&id, m)| {
            let (w, _) = m.size();
            let near =
                (m.pos[0] - at[0]).hypot(m.pos[2] - at[2]) < 0.9 && (m.pos[1] - at[1]).abs() < 0.7;
            let fits = w < width
                && !m.kind.swims()
                && !m.kind.flies()
                && m.kind != super::mobs::MobKind::IronGolem
                && m.alive()
                && m.vehicle.is_none()
                && m.villager.as_ref().is_none_or(|v| v.customer.is_none());
            (near && fits && !aboard.contains(&id)).then_some(id)
        })
    }

    pub(super) fn boat_packets(&self) -> Vec<Writer> {
        self.boats
            .iter()
            .flat_map(|(&id, b)| spawn_packets(id, b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn boats_are_ridden_steered_left_and_broken() {
        let dir = std::env::temp_dir().join(format!("levitas-boat-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let (out, frames) = mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        w.players
            .insert(alice, Player::new(profile, out, [2.5, -60.0, 2.5]));

        // A boat on the ground, saved and loaded the way vanilla stores it.
        let boat = Boat::new("minecraft:oak_boat", [4.5, -60.0, 2.5], 0.0, &mut w.rng);
        let back = Boat::from_nbt(&boat.to_nbt(), &mut w.rng).unwrap();
        assert_eq!(
            (back.kind, back.pos, back.uuid),
            (boat.kind, boat.pos, boat.uuid)
        );
        let id = next_entity_id();
        w.boats.insert(id, boat);

        assert!(w.board_boat(alice, id));
        assert_eq!(w.boats[&id].rider, Some(alice));
        w.boat_moved(alice, [6.5, -60.0, 2.5], 90.0);
        assert_eq!(w.boats[&id].pos, [6.5, -60.0, 2.5]);
        assert_eq!(
            w.players[&alice].pos,
            [6.5, -60.0, 2.5],
            "the rider goes along"
        );
        // A move too far is refused.
        w.boat_moved(alice, [60.5, -60.0, 2.5], 90.0);
        assert_eq!(w.boats[&id].pos[0], 6.5);
        w.get_off(alice);
        assert_eq!(w.boats[&id].rider, None);
        assert!(w.players[&alice].riding.is_none());

        // Five punches break it into an item.
        for _ in 0..5 {
            assert!(w.hit_boat(alice, id));
        }
        assert!(!w.boats.contains_key(&id));
        let oak_boat = ItemInfo::id_of("minecraft:oak_boat").unwrap();
        assert!(w.items.values().any(|i| i.stack().item == oak_boat));
    }

    #[test]
    fn empty_boats_float_up_to_the_surface() {
        let dir = std::env::temp_dir().join(format!("levitas-float-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        // A pool three deep on top of the ground at y = -61.
        for y in -60..=-58 {
            w.set_block(BlockPos::new(8, y, 8), levitas_registry::block::WATER);
        }
        let boat = Boat::new("minecraft:oak_boat", [8.5, -60.0, 8.5], 0.0, &mut w.rng);
        let id = next_entity_id();
        w.boats.insert(id, boat);
        for _ in 0..60 {
            w.tick_boats();
        }
        assert!((w.boats[&id].pos[1] - (-57.0 - FLOAT_DEPTH)).abs() < 1e-9);
        // In the air, it falls back down onto the water.
        w.boats.get_mut(&id).unwrap().pos[1] = -50.0;
        for _ in 0..60 {
            w.tick_boats();
        }
        assert!((w.boats[&id].pos[1] - (-57.0 - FLOAT_DEPTH)).abs() < 1e-9);
    }

    #[test]
    fn boats_and_rafts_are_placed() {
        assert_eq!(
            boat_kind("minecraft:birch_boat"),
            Some("minecraft:birch_boat")
        );
        assert_eq!(
            boat_kind("minecraft:bamboo_raft"),
            Some("minecraft:bamboo_raft")
        );
        assert_eq!(
            boat_kind("minecraft:oak_chest_boat"),
            Some("minecraft:oak_chest_boat")
        );
        assert_eq!(boat_kind("minecraft:oak_planks"), None);
        assert_ne!(
            entity_type_of("minecraft:bamboo_raft"),
            entity_type::OAK_BOAT
        );
    }

    #[test]
    fn boats_seat_two_chest_boats_open_and_mobs_get_in() {
        let dir = std::env::temp_dir().join(format!("levitas-boat2-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let player = |w: &mut World, name: &str| {
            let (out, frames) = mpsc::channel(100_000);
            std::mem::forget(frames);
            let id = next_entity_id();
            let profile = crate::connection::Profile {
                name: name.to_owned(),
                uuid: id as u128,
                properties: Vec::new(),
            };
            w.players
                .insert(id, Player::new(profile, out, [2.0, -60.0, 2.0]));
            id
        };
        let alice = player(&mut w, "Alice");
        let bob = player(&mut w, "Bob");
        let carol = player(&mut w, "Carol");
        let boat = next_entity_id();
        w.boats.insert(
            boat,
            Boat::new("minecraft:oak_boat", [2.5, -60.0, 2.5], 0.0, &mut w.rng),
        );
        assert!(w.board_boat(alice, boat));
        assert!(w.board_boat(bob, boat));
        assert!(w.board_boat(carol, boat));
        assert_eq!(w.boats[&boat].rider, Some(alice));
        assert_eq!(w.boats[&boat].second, Some(bob));
        assert!(w.players[&carol].riding.is_none(), "only two seats");
        // The driver gets off: the back seat moves up.
        w.get_off(alice);
        assert_eq!(w.boats[&boat].rider, Some(bob));

        // A chest boat: sneaking opens its chest, which is saved with it.
        let chest_boat = next_entity_id();
        let mut cb = Boat::new(
            "minecraft:oak_chest_boat",
            [6.5, -60.0, 2.5],
            0.0,
            &mut w.rng,
        );
        let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
        cb.chest.as_mut().unwrap()[3] = Stack::new(dirt, 9);
        let back = Boat::from_nbt(&cb.to_nbt(), &mut w.rng).unwrap();
        assert_eq!(back.chest.as_ref().unwrap()[3].count, 9);
        w.boats.insert(chest_boat, cb);
        w.players.get_mut(&carol).unwrap().pos = [6.0, -60.0, 2.0];
        w.players.get_mut(&carol).unwrap().sneaking = true;
        assert!(w.board_boat(carol, chest_boat));
        assert!(w.players[&carol].open.is_some(), "the chest opened");

        // A pig wandering into an empty boat sits down and goes along.
        let empty = next_entity_id();
        w.boats.insert(
            empty,
            Boat::new("minecraft:oak_boat", [10.5, -60.0, 10.5], 0.0, &mut w.rng),
        );
        let pig = next_entity_id();
        let mob = super::super::mobs::Mob::new(
            super::super::mobs::MobKind::Pig,
            [10.6, -60.0, 10.4],
            &mut w.rng,
        );
        w.mobs.insert(pig, mob);
        for _ in 0..8 {
            w.tick += 1;
            w.tick_boats();
        }
        assert_eq!(w.boats[&empty].mobs, vec![pig]);
        assert_eq!(w.mobs[&pig].vehicle, Some(empty));
        // Breaking the boat lets it out.
        w.remove_boat(empty, false);
        assert_eq!(w.mobs[&pig].vehicle, None);
    }
}
