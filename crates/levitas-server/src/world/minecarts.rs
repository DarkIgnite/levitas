//! Minecarts and rails, following the Minecraft Wiki.
//!
//! A minecart is placed on a rail, ridden by right-clicking it and left by sneaking, and
//! broken by hits into its item. The server moves it: along straight rails, round
//! curves, up and down slopes (slowing uphill, speeding downhill), pushed on by powered
//! rails and braked by unpowered ones, at most 8 blocks a second. A rider holding
//! forward nudges it along. Off the rails it falls and slides to a stop. Detector rails
//! give off power while a cart is on them; powered and activator rails take power from
//! redstone, and pass it along up to 8 rails of their kind.

use std::collections::HashSet;

use levitas_nbt::{Compound, Tag};
use levitas_registry::entity_type;

use super::*;
use crate::blocks::{self, offset, Dir};

const MAX_SPEED: f64 = 0.4;
const SLOPE: f64 = 0.0078125;
const BOOST: f64 = 0.06;
const GRAVITY: f64 = 0.04;
/// How much speed a cart keeps each tick, with and without a rider.
const DRAG_RIDDEN: f64 = 0.997;
const DRAG_EMPTY: f64 = 0.96;
/// The push a rider holding forward gives a slow cart.
const RIDER_PUSH: f64 = 0.01;
const BREAKS_AT: f32 = 40.0;
/// Below this speed a cart has come to rest.
const AT_REST: f64 = 0.003;
/// Rails of its kind a powered or activator rail passes power along.
const RAIL_REACH: i32 = 8;

const DATA_HURT_TIME: u8 = 8;
const DATA_HURT_DIRECTION: u8 = 9;
const DATA_DAMAGE: u8 = 10;
const SERIALIZER_INT: i32 = 1;
const SERIALIZER_FLOAT: i32 = 3;

pub struct Cart {
    uuid: u128,
    pub pos: [f64; 3],
    vel: [f64; 3],
    yaw: f32,
    pub rider: Option<i32>,
    damage: f32,
    hurt_direction: i32,
    sent_pos: [f64; 3],
}

/// One end of a rail piece: the side it leaves the block by, and whether that end is
/// the raised end of a slope.
#[derive(Debug, Clone, Copy, PartialEq)]
struct End {
    dx: i32,
    dz: i32,
    raised: bool,
}

const fn end(dx: i32, dz: i32, raised: bool) -> End {
    End { dx, dz, raised }
}

pub fn is_rail(state: u16) -> bool {
    block_has_tag(state, "minecraft:rails")
}

/// The two ends of a rail of `shape`.
fn ends(shape: &str) -> Option<[End; 2]> {
    Some(match shape {
        "north_south" => [end(0, -1, false), end(0, 1, false)],
        "east_west" => [end(-1, 0, false), end(1, 0, false)],
        "ascending_east" => [end(-1, 0, false), end(1, 0, true)],
        "ascending_west" => [end(1, 0, false), end(-1, 0, true)],
        "ascending_north" => [end(0, 1, false), end(0, -1, true)],
        "ascending_south" => [end(0, -1, false), end(0, 1, true)],
        "south_east" => [end(0, 1, false), end(1, 0, false)],
        "south_west" => [end(0, 1, false), end(-1, 0, false)],
        "north_west" => [end(0, -1, false), end(-1, 0, false)],
        "north_east" => [end(0, -1, false), end(1, 0, false)],
        _ => return None,
    })
}

fn is_slope(shape: &str) -> bool {
    shape.starts_with("ascending")
}

fn end_point(rail: BlockPos, e: End) -> [f64; 3] {
    [
        f64::from(rail.x) + 0.5 + f64::from(e.dx) * 0.5,
        f64::from(rail.y) + if e.raised { 1.0 } else { 0.0 },
        f64::from(rail.z) + 0.5 + f64::from(e.dz) * 0.5,
    ]
}

fn center(rail: BlockPos, shape: &str) -> [f64; 3] {
    [
        f64::from(rail.x) + 0.5,
        f64::from(rail.y) + if is_slope(shape) { 0.5 } else { 0.0 },
        f64::from(rail.z) + 0.5,
    ]
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn shape_of(state: u16) -> &'static str {
    blocks::prop(state, "shape").unwrap_or("north_south")
}

impl Cart {
    fn new(pos: [f64; 3], rng: &mut Rng) -> Self {
        Cart {
            uuid: (u128::from(rng.next_u64()) << 64) | u128::from(rng.next_u64()),
            pos,
            vel: [0.0; 3],
            yaw: 0.0,
            rider: None,
            damage: 0.0,
            hurt_direction: 1,
            sent_pos: pos,
        }
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
            .with("id", "minecraft:minecart")
            .with("UUID", uuid.to_vec())
            .with(
                "Pos",
                self.pos.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with(
                "Motion",
                self.vel.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with("Rotation", vec![Tag::Float(self.yaw), Tag::Float(0.0)])
    }

    pub fn from_nbt(nbt: &Compound, rng: &mut Rng) -> Option<Cart> {
        if nbt.str("id")? != "minecraft:minecart" {
            return None;
        }
        let read = |key: &str| -> Option<[f64; 3]> {
            let v: Vec<f64> = nbt.list(key)?.iter().filter_map(Tag::as_f64).collect();
            Some([*v.first()?, *v.get(1)?, *v.get(2)?])
        };
        let mut cart = Cart::new(read("Pos")?, rng);
        cart.vel = read("Motion").unwrap_or([0.0; 3]);
        if let Some(&[a, b, c, d]) = nbt.int_array("UUID") {
            cart.uuid = (u128::from(a as u32) << 96)
                | (u128::from(b as u32) << 64)
                | (u128::from(c as u32) << 32)
                | u128::from(d as u32);
        }
        Some(cart)
    }
}

pub fn spawn_packets(id: i32, cart: &Cart) -> Vec<Writer> {
    let mut packets = vec![Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(cart.uuid)
        .varint(entity_type::MINECART)
        .f64(cart.pos[0])
        .f64(cart.pos[1])
        .f64(cart.pos[2])
        .lp_vec3([0.0, 0.0, 0.0])
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(0)];
    if let Some(rider) = cart.rider {
        packets.push(passengers(id, Some(rider)));
    }
    packets
}

fn passengers(cart: i32, rider: Option<i32>) -> Writer {
    let packet = Writer::packet(clientbound::SET_PASSENGERS).varint(cart);
    match rider {
        Some(r) => packet.varint(1).varint(r),
        None => packet.varint(0),
    }
}

fn position_sync(id: i32, cart: &Cart) -> Writer {
    Writer::packet(clientbound::ENTITY_POSITION_SYNC)
        .varint(id)
        .varint(0) // a linear path
        .f64(cart.pos[0])
        .f64(cart.pos[1])
        .f64(cart.pos[2])
        .f32(cart.yaw)
        .f32(0.0)
        .bool(true)
}

impl World {
    /// The rail a cart at `pos` runs on: in its block, or the one under it.
    fn rail_under(&self, pos: [f64; 3]) -> Option<(BlockPos, u16)> {
        let at = BlockPos::new(
            pos[0].floor() as i32,
            (pos[1] + 0.01).floor() as i32,
            pos[2].floor() as i32,
        );
        [at, offset(at, Dir::Down)]
            .into_iter()
            .find_map(|q| self.loaded_state(q).filter(|&s| is_rail(s)).map(|s| (q, s)))
    }

    /// A minecart used on a rail is set down on it. Returns whether it was.
    pub(super) fn place_minecart(&mut self, id: i32, slot: usize, pos: BlockPos) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let stack = p.inventory.slots[slot];
        if ItemInfo::get(stack.item).is_none_or(|i| i.name != "minecraft:minecart") {
            return false;
        }
        let survival = p.survival();
        let state = self.block(pos);
        if !is_rail(state) {
            return false;
        }
        let at = center(pos, shape_of(state));
        let cart = Cart::new(at, &mut self.rng);
        let cart_id = next_entity_id();
        for packet in spawn_packets(cart_id, &cart) {
            self.broadcast(packet, None);
        }
        self.carts.insert(cart_id, cart);
        if survival {
            let p = self.players.get_mut(&id).expect("checked above");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        true
    }

    /// A player right-clicks a minecart: they get in if it is free. Returns whether
    /// `target` is a minecart.
    pub(super) fn board_cart(&mut self, id: i32, target: i32) -> bool {
        let Some(cart) = self.carts.get(&target) else {
            return false;
        };
        let Some(p) = self.players.get(&id) else {
            return true;
        };
        let near = distance(p.pos, cart.pos) <= 6.0;
        if cart.rider.is_some() || !p.alive || p.sneaking || !near || p.riding.is_some() {
            return true;
        }
        let at = cart.pos;
        self.carts.get_mut(&target).expect("checked above").rider = Some(id);
        self.players.get_mut(&id).expect("checked above").riding = Some(target);
        self.broadcast(passengers(target, Some(id)), None);
        self.move_player(id, Some(at), None, true);
        true
    }

    /// The rider gets out of their minecart beside it. Returns whether they were in one.
    pub(super) fn leave_cart(&mut self, id: i32, cart_id: i32) -> bool {
        let Some(cart) = self.carts.get_mut(&cart_id) else {
            return false;
        };
        cart.rider = None;
        let at = [cart.pos[0], cart.pos[1] + 0.6, cart.pos[2]];
        self.broadcast(passengers(cart_id, None), None);
        if self.players.get(&id).is_some_and(|p| p.alive) {
            self.teleport(id, at);
        }
        true
    }

    /// A player hits a minecart: it wobbles, and breaks into its item after a few hits
    /// (at once, and without a drop, in creative). Returns whether `target` is one.
    pub(super) fn hit_cart(&mut self, attacker: i32, target: i32) -> bool {
        if !self.carts.contains_key(&target) {
            return false;
        }
        let Some(p) = self.players.get(&attacker) else {
            return true;
        };
        let creative = !p.survival();
        let strength =
            ItemInfo::get(p.inventory.slots[p.hand_slot(0)].item).map_or(1.0, |i| i.attack_damage);
        let cart = self.carts.get_mut(&target).expect("checked above");
        cart.hurt_direction = -cart.hurt_direction;
        cart.damage += strength * 10.0;
        let broken = creative || cart.damage > BREAKS_AT;
        let wobble = Writer::packet(clientbound::SET_ENTITY_DATA)
            .varint(target)
            .u8(DATA_HURT_TIME)
            .varint(SERIALIZER_INT)
            .varint(10)
            .u8(DATA_HURT_DIRECTION)
            .varint(SERIALIZER_INT)
            .varint(cart.hurt_direction)
            .u8(DATA_DAMAGE)
            .varint(SERIALIZER_FLOAT)
            .f32(cart.damage)
            .u8(0xFF);
        self.broadcast(wobble, None);
        if broken {
            if let Some(rider) = self.carts.get(&target).and_then(|c| c.rider) {
                self.get_off(rider);
            }
            if let Some(cart) = self.carts.remove(&target) {
                self.broadcast(items::remove_entity(target), None);
                if !creative {
                    if let Some(item) = ItemInfo::id_of("minecraft:minecart") {
                        self.scatter_item(cart.pos, Stack::new(item, 1));
                    }
                }
            }
        }
        true
    }

    pub(super) fn tick_minecarts(&mut self) {
        let ids: Vec<i32> = self.carts.keys().copied().collect();
        let mut occupied = HashSet::new();
        for id in ids {
            let mut cart = self.carts.remove(&id).expect("listed");
            cart.damage = (cart.damage - 1.0).max(0.0);
            if self.chunks.contains_key(&cart.chunk()) {
                self.move_cart(&mut cart);
            }
            if let Some((rail, state)) = self.rail_under(cart.pos) {
                if blocks::name(state) == "minecraft:detector_rail" {
                    occupied.insert(rail);
                }
            }
            if cart.pos[1] < f64::from(MIN_Y - 64) {
                if let Some(rider) = cart.rider {
                    self.players.get_mut(&rider).map(|p| p.riding.take());
                }
                self.broadcast(items::remove_entity(id), None);
                continue;
            }
            let moved = distance(cart.pos, cart.sent_pos) > 1e-4;
            if moved {
                cart.sent_pos = cart.pos;
                self.broadcast(position_sync(id, &cart), None);
                if let Some(rider) = cart.rider {
                    self.move_player(rider, Some(cart.pos), None, true);
                }
            }
            self.carts.insert(id, cart);
        }
        self.update_detectors(occupied);
    }

    /// Detector rails are powered while a cart is on them.
    fn update_detectors(&mut self, occupied: HashSet<BlockPos>) {
        let released: Vec<BlockPos> = self.detectors.difference(&occupied).copied().collect();
        for pos in released {
            let state = self.block(pos);
            if blocks::name(state) == "minecraft:detector_rail" {
                self.set_block(pos, blocks::with_bool(state, "powered", false));
                self.redstone_changed(pos);
            }
        }
        for &pos in occupied.difference(&self.detectors.clone()) {
            let state = self.block(pos);
            self.set_block(pos, blocks::with_bool(state, "powered", true));
            self.redstone_changed(pos);
        }
        self.detectors = occupied;
    }

    fn move_cart(&mut self, cart: &mut Cart) {
        let Some((mut rail, mut state)) = self.rail_under(cart.pos) else {
            self.cart_falls(cart);
            return;
        };
        let mut shape = shape_of(state);
        let Some(mut rail_ends) = ends(shape) else {
            return;
        };
        let horizontal = [cart.vel[0], cart.vel[2]];
        let mut speed = horizontal[0].hypot(horizontal[1]);
        // The end the cart is heading for: the one its motion points to.
        let toward = |e: &End| f64::from(e.dx) * horizontal[0] + f64::from(e.dz) * horizontal[1];
        let mut exit = if toward(&rail_ends[0]) >= toward(&rail_ends[1]) {
            rail_ends[0]
        } else {
            rail_ends[1]
        };
        if speed < AT_REST {
            speed = 0.0;
        }
        // A rider holding forward pushes a slow cart the way they look.
        if let Some(p) = cart.rider.and_then(|r| self.players.get(&r)) {
            if p.forward && speed < 0.1 {
                let yaw = f64::from(p.yaw).to_radians();
                let look = [-yaw.sin(), yaw.cos()];
                let along = |e: &End| f64::from(e.dx) * look[0] + f64::from(e.dz) * look[1];
                let best = if along(&rail_ends[0]) >= along(&rail_ends[1]) {
                    rail_ends[0]
                } else {
                    rail_ends[1]
                };
                if speed == 0.0 || best == exit {
                    exit = best;
                    speed += RIDER_PUSH;
                }
            }
        }
        // Slopes: slower uphill, faster downhill, and a still cart rolls down.
        if is_slope(shape) {
            let low = rail_ends
                .iter()
                .copied()
                .find(|e| !e.raised)
                .expect("a low end");
            if speed == 0.0 {
                exit = low;
                speed = SLOPE;
            } else if exit.raised {
                speed -= SLOPE;
                if speed < 0.0 {
                    exit = low;
                    speed = -speed;
                }
            } else {
                speed += SLOPE;
            }
        }
        // Powered rails push on, unpowered ones brake.
        if blocks::name(state) == "minecraft:powered_rail" {
            if blocks::prop(state, "powered") == Some("true") {
                if speed > 0.01 {
                    speed += BOOST;
                } else if let Some(away) = self.kick_off_wall(rail, &rail_ends) {
                    exit = away;
                    speed = BOOST / 3.0;
                }
            } else {
                speed *= 0.5;
                if speed < 0.03 {
                    speed = 0.0;
                }
            }
        }
        let drag = if cart.rider.is_some() {
            DRAG_RIDDEN
        } else {
            DRAG_EMPTY
        };
        speed = (speed * drag).min(MAX_SPEED);
        if speed < AT_REST {
            cart.vel = [0.0; 3];
            // Settle onto the rail's line.
            if distance(cart.pos, center(rail, shape)) < 0.5 && !is_slope(shape) {
                cart.pos[1] = f64::from(rail.y);
            }
            return;
        }

        // Travel `speed` along the track, from block to block.
        let mut left = speed;
        for _ in 0..8 {
            let exit_at = end_point(rail, exit);
            let mid = center(rail, shape);
            let curve = rail_ends[0].dx != -rail_ends[1].dx || rail_ends[0].dz != -rail_ends[1].dz;
            let goal = if curve && distance(cart.pos, exit_at) > 0.5 + 1e-6 {
                mid
            } else {
                exit_at
            };
            let to_goal = distance(cart.pos, goal);
            if to_goal > left {
                let f = left / to_goal;
                for (p, g) in cart.pos.iter_mut().zip(goal) {
                    *p += (g - *p) * f;
                }
                break;
            }
            cart.pos = goal;
            left -= to_goal;
            if goal != exit_at {
                continue;
            }
            // On to the next rail, which must link back to this one.
            let next = BlockPos::new(
                rail.x + exit.dx,
                rail.y + i32::from(exit.raised),
                rail.z + exit.dz,
            );
            let back = |s: u16| {
                ends(shape_of(s)).and_then(|es| {
                    let i = es
                        .iter()
                        .position(|e| e.dx == -exit.dx && e.dz == -exit.dz)?;
                    Some(es[1 - i])
                })
            };
            let found = [next, offset(next, Dir::Down)]
                .into_iter()
                .filter_map(|q| self.loaded_state(q).filter(|&s| is_rail(s)).map(|s| (q, s)))
                .find_map(|(q, s)| back(s).map(|e| (q, s, e)));
            let Some((q, s, e)) = found else {
                // The track ends: the cart rolls off.
                cart.vel = [f64::from(exit.dx) * speed, 0.0, f64::from(exit.dz) * speed];
                cart.pos[0] += f64::from(exit.dx) * left;
                cart.pos[2] += f64::from(exit.dz) * left;
                return;
            };
            rail = q;
            state = s;
            shape = shape_of(state);
            rail_ends = ends(shape).expect("checked in back()");
            exit = e;
            if left <= 0.0 {
                break;
            }
        }
        cart.vel = [f64::from(exit.dx) * speed, 0.0, f64::from(exit.dz) * speed];
        cart.yaw = (f64::from(exit.dz).atan2(f64::from(exit.dx)).to_degrees()) as f32;
    }

    /// A powered rail sends a still cart away from a solid block at one of its ends.
    fn kick_off_wall(&self, rail: BlockPos, rail_ends: &[End; 2]) -> Option<End> {
        let solid = |e: &End| {
            let q = BlockPos::new(rail.x + e.dx, rail.y, rail.z + e.dz);
            self.loaded_state(q)
                .is_some_and(levitas_registry::is_full_cube)
        };
        if solid(&rail_ends[0]) {
            Some(rail_ends[1])
        } else if solid(&rail_ends[1]) {
            Some(rail_ends[0])
        } else {
            None
        }
    }

    /// Off the rails: the cart falls, and slides to a stop on the ground.
    fn cart_falls(&mut self, cart: &mut Cart) {
        cart.vel[1] -= GRAVITY;
        let mut pos = cart.pos;
        let mut on_ground = false;
        for axis in 0..3 {
            let delta = self.clip_move(pos, 0.49, 0.7, axis, cart.vel[axis]);
            if axis == 1 && delta > cart.vel[1] {
                on_ground = true;
                cart.vel[1] = 0.0;
            }
            pos[axis] += delta;
        }
        cart.pos = pos;
        if on_ground {
            cart.vel[0] *= 0.5;
            cart.vel[2] *= 0.5;
        } else {
            cart.vel[0] *= 0.95;
            cart.vel[2] *= 0.95;
        }
    }

    /// A powered or activator rail is powered by redstone at it, or through a line of
    /// up to eight rails of its kind leading to some.
    pub(super) fn react_rail(&mut self, pos: BlockPos, state: u16) {
        let kind = blocks::name(state);
        let powered = self.power_at(pos) > 0 || self.rail_line_powered(pos, state, kind);
        if (blocks::prop(state, "powered") == Some("true")) != powered {
            self.set_block(pos, blocks::with_bool(state, "powered", powered));
            self.redstone_changed(pos);
        }
    }

    fn rail_line_powered(&self, start: BlockPos, state: u16, kind: &str) -> bool {
        let Some(rail_ends) = ends(shape_of(state)) else {
            return false;
        };
        for e in rail_ends {
            let mut at = start;
            let mut going = e;
            for _ in 0..RAIL_REACH {
                let next = BlockPos::new(
                    at.x + going.dx,
                    at.y + i32::from(going.raised),
                    at.z + going.dz,
                );
                let found = [next, offset(next, Dir::Down)].into_iter().find_map(|q| {
                    let s = self.loaded_state(q)?;
                    if blocks::name(s) != kind {
                        return None;
                    }
                    let es = ends(shape_of(s))?;
                    let i = es
                        .iter()
                        .position(|x| x.dx == -going.dx && x.dz == -going.dz)?;
                    Some((q, es[1 - i]))
                });
                let Some((q, onward)) = found else {
                    break;
                };
                if self.power_at(q) > 0 {
                    return true;
                }
                at = q;
                going = onward;
            }
        }
        false
    }

    pub(super) fn cart_packets(&self) -> Vec<Writer> {
        self.carts
            .iter()
            .flat_map(|(&id, c)| spawn_packets(id, c))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-cart-{}-{}",
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

    fn rail(name: &str, shape: &str) -> u16 {
        let base = BlockInfo::by_name(name).unwrap().default_state;
        blocks::with(base, "shape", shape)
    }

    #[test]
    fn carts_follow_curves_and_stop_on_their_own() {
        let mut w = world();
        // East along z = 2 from x = 1 to 5, then a curve turning south at x = 6.
        for x in 1..=5 {
            w.set_block(
                BlockPos::new(x, -60, 2),
                rail("minecraft:rail", "east_west"),
            );
        }
        w.set_block(
            BlockPos::new(6, -60, 2),
            rail("minecraft:rail", "south_west"),
        );
        for z in 3..=8 {
            w.set_block(
                BlockPos::new(6, -60, z),
                rail("minecraft:rail", "north_south"),
            );
        }
        let mut cart = Cart::new([1.5, -60.0, 2.5], &mut w.rng);
        cart.vel = [0.4, 0.0, 0.0];
        let id = next_entity_id();
        w.carts.insert(id, cart);
        let mut furthest_z: f64 = 0.0;
        for _ in 0..200 {
            w.tick_minecarts();
            furthest_z = furthest_z.max(w.carts[&id].pos[2]);
        }
        let c = &w.carts[&id];
        assert!(furthest_z > 5.0, "went round the curve: {:?}", c.pos);
        assert!(
            (c.pos[0] - 6.5).abs() < 1e-6,
            "on the southward line: {:?}",
            c.pos
        );
        assert_eq!(c.vel, [0.0; 3], "came to a stop");
    }

    #[test]
    fn powered_rails_boost_and_detector_rails_give_power() {
        let mut w = world();
        // A redstone block under the start powers the boosters through the line.
        let line = 1..=12;
        for x in line.clone() {
            let kind = if x <= 5 {
                "minecraft:powered_rail"
            } else if x == 10 {
                "minecraft:detector_rail"
            } else {
                "minecraft:rail"
            };
            w.set_block(BlockPos::new(x, -60, 4), rail(kind, "east_west"));
        }
        w.set_block(BlockPos::new(0, -60, 4), levitas_registry::block::STONE);
        w.set_block(
            BlockPos::new(1, -61, 4),
            BlockInfo::by_name("minecraft:redstone_block")
                .unwrap()
                .default_state,
        );
        for x in line {
            w.redstone_changed(BlockPos::new(x, -60, 4));
        }
        w.tick_redstone();
        for x in 1..=5 {
            let s = w.block(BlockPos::new(x, -60, 4));
            assert_eq!(blocks::prop(s, "powered"), Some("true"), "rail {x}");
        }
        // A still cart against the stone is kicked off and boosted along.
        let id = next_entity_id();
        let cart = Cart::new([1.5, -60.0, 4.5], &mut w.rng);
        w.carts.insert(id, cart);
        let mut saw_detector = false;
        for _ in 0..80 {
            w.tick_minecarts();
            w.tick_redstone();
            let detector = w.block(BlockPos::new(10, -60, 4));
            saw_detector |= blocks::prop(detector, "powered") == Some("true");
        }
        assert!(w.carts[&id].pos[0] > 10.0, "{:?}", w.carts[&id].pos);
        assert!(saw_detector, "the detector rail switched on");
        let detector = w.block(BlockPos::new(10, -60, 4));
        assert_eq!(
            blocks::prop(detector, "powered"),
            Some("false"),
            "and off again"
        );
    }

    #[test]
    fn carts_roll_down_slopes() {
        let mut w = world();
        w.set_block(
            BlockPos::new(3, -60, 6),
            rail("minecraft:rail", "ascending_east"),
        );
        w.set_block(
            BlockPos::new(2, -60, 6),
            rail("minecraft:rail", "east_west"),
        );
        w.set_block(
            BlockPos::new(1, -60, 6),
            rail("minecraft:rail", "east_west"),
        );
        let id = next_entity_id();
        w.carts.insert(id, Cart::new([3.5, -59.5, 6.5], &mut w.rng));
        for _ in 0..30 {
            w.tick_minecarts();
        }
        assert!(w.carts[&id].pos[0] < 3.0, "{:?}", w.carts[&id].pos);
    }
}
