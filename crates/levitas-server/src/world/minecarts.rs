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
const SERIALIZER_BOOLEAN: i32 = 8;

pub struct Cart {
    uuid: u128,
    pub pos: [f64; 3],
    vel: [f64; 3],
    yaw: f32,
    pub rider: Option<i32>,
    /// A mob riding it.
    pub mob: Option<i32>,
    damage: f32,
    hurt_direction: i32,
    sent_pos: [f64; 3],
    /// What it carries: nothing (a rider), a chest, a hopper, a furnace or TNT.
    pub kind: CartKind,
    /// A chest minecart's 27 slots, or a hopper minecart's 5.
    pub chest: Option<Box<[Stack]>>,
    /// A furnace minecart's fuel left in ticks, and the way it pushes.
    fuel: i32,
    push: [f64; 2],
    /// A TNT minecart's fuse, once lit.
    fuse: Option<i32>,
    /// A hopper minecart moves items while not on a powered activator rail.
    enabled: bool,
    cooldown: u32,
    /// The loot table a structure's chest minecart still has to be filled from, and
    /// its seed.
    pub loot: Option<(String, i64)>,
}

/// Slots in a chest minecart, and a hopper minecart.
pub const CART_CHEST_SLOTS: usize = 27;
pub const CART_HOPPER_SLOTS: usize = 5;
/// Fuel a piece of coal gives a furnace minecart, and the most it holds.
const COAL_FUEL: i32 = 3600;
const MAX_FUEL: i32 = 32_000;
/// How fast a furnace minecart pushes itself along.
const FURNACE_SPEED: f64 = 0.2;
const FURNACE_PUSH: f64 = 0.04;
/// A TNT minecart's fuse once lit, and its blast.
const TNT_FUSE: i32 = 80;
const TNT_POWER: f32 = 4.0;
/// Ticks between a hopper minecart's moves.
const HOPPER_COOLDOWN: u32 = 4;
/// How close two minecarts come before they push each other apart.
const BUMP_DISTANCE: f64 = 1.0;

/// What a minecart carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CartKind {
    Ride,
    Chest,
    Hopper,
    Furnace,
    Tnt,
}

impl CartKind {
    /// The entity's and item's ID.
    pub fn name(self) -> &'static str {
        match self {
            CartKind::Ride => "minecraft:minecart",
            CartKind::Chest => "minecraft:chest_minecart",
            CartKind::Hopper => "minecraft:hopper_minecart",
            CartKind::Furnace => "minecraft:furnace_minecart",
            CartKind::Tnt => "minecraft:tnt_minecart",
        }
    }

    fn from_name(name: &str) -> Option<CartKind> {
        [
            CartKind::Ride,
            CartKind::Chest,
            CartKind::Hopper,
            CartKind::Furnace,
            CartKind::Tnt,
        ]
        .into_iter()
        .find(|k| k.name() == name)
    }

    /// The block it carries, which drops along with the minecart.
    fn block_item(self) -> Option<&'static str> {
        match self {
            CartKind::Ride => None,
            CartKind::Chest => Some("minecraft:chest"),
            CartKind::Hopper => Some("minecraft:hopper"),
            CartKind::Furnace => Some("minecraft:furnace"),
            CartKind::Tnt => Some("minecraft:tnt"),
        }
    }

    fn entity_type(self) -> i32 {
        entity_type::NAMES
            .iter()
            .position(|&n| n == self.name())
            .map_or(entity_type::MINECART, |i| i as i32)
    }

    fn slots(self) -> usize {
        match self {
            CartKind::Chest => CART_CHEST_SLOTS,
            CartKind::Hopper => CART_HOPPER_SLOTS,
            _ => 0,
        }
    }
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
            mob: None,
            damage: 0.0,
            hurt_direction: 1,
            sent_pos: pos,
            kind: CartKind::Ride,
            chest: None,
            loot: None,
            fuel: 0,
            push: [0.0; 2],
            fuse: None,
            enabled: true,
            cooldown: 0,
        }
    }

    fn of_kind(mut self, kind: CartKind) -> Self {
        self.kind = kind;
        let slots = kind.slots();
        self.chest = (slots > 0).then(|| vec![Stack::EMPTY; slots].into_boxed_slice());
        self
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
        let id = self.kind.name();
        let mut nbt = Compound::new()
            .with("id", id)
            .with("UUID", uuid.to_vec())
            .with(
                "Pos",
                self.pos.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with(
                "Motion",
                self.vel.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with("Rotation", vec![Tag::Float(self.yaw), Tag::Float(0.0)]);
        if let Some(items) = &self.chest {
            nbt.insert(
                "Items",
                crate::inventory::items_to_nbt(items.iter().copied().enumerate()),
            );
        }
        if self.kind == CartKind::Furnace {
            nbt.insert("Fuel", self.fuel.clamp(0, i32::from(i16::MAX)) as i16);
            nbt.insert("PushX", self.push[0]);
            nbt.insert("PushZ", self.push[1]);
        }
        if self.kind == CartKind::Tnt {
            nbt.insert("fuse", self.fuse.unwrap_or(-1));
        }
        if self.kind == CartKind::Hopper {
            nbt.insert("Enabled", self.enabled);
        }
        if let Some((table, seed)) = &self.loot {
            nbt.insert("LootTable", table.as_str());
            if *seed != 0 {
                nbt.insert("LootTableSeed", *seed);
            }
        }
        nbt
    }

    pub fn from_nbt(nbt: &Compound, rng: &mut Rng) -> Option<Cart> {
        let kind = CartKind::from_name(nbt.str("id")?)?;
        let read = |key: &str| -> Option<[f64; 3]> {
            let v: Vec<f64> = nbt.list(key)?.iter().filter_map(Tag::as_f64).collect();
            Some([*v.first()?, *v.get(1)?, *v.get(2)?])
        };
        let mut cart = Cart::new(read("Pos")?, rng).of_kind(kind);
        cart.fuel = nbt.int("Fuel").unwrap_or(0) as i32;
        cart.push = [
            nbt.float("PushX").unwrap_or(0.0),
            nbt.float("PushZ").unwrap_or(0.0),
        ];
        cart.fuse = nbt.int("fuse").filter(|&f| f >= 0).map(|f| f as i32);
        cart.enabled = nbt.int("Enabled").unwrap_or(1) != 0;
        if cart.chest.is_some() {
            if let Some(items) = cart.chest.as_mut() {
                for (slot, stack) in
                    crate::inventory::items_from_nbt(nbt.list("Items").unwrap_or_default())
                {
                    if let Some(s) = items.get_mut(slot) {
                        *s = stack;
                    }
                }
            }
            cart.loot = nbt
                .str("LootTable")
                .map(|t| (t.to_owned(), nbt.int("LootTableSeed").unwrap_or(0)));
        }
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
        .varint(cart.kind.entity_type())
        .f64(cart.pos[0])
        .f64(cart.pos[1])
        .f64(cart.pos[2])
        .lp_vec3([0.0, 0.0, 0.0])
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(0)];
    if let Some(rider) = cart.rider.or(cart.mob) {
        packets.push(passengers(id, Some(rider)));
    }
    if cart.kind == CartKind::Furnace && cart.fuel > 0 {
        packets.push(furnace_lit(id, true));
    }
    packets
}

/// Whether a furnace minecart shows its fire (entity data 13).
fn furnace_lit(id: i32, lit: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(13)
        .varint(SERIALIZER_BOOLEAN)
        .bool(lit)
        .u8(0xFF)
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

/// Whether `stack` fits somewhere in `slots`.
fn can_take(slots: &[Stack], stack: Stack) -> bool {
    slots
        .iter()
        .any(|s| s.is_empty() || (s.same_kind(&stack) && s.count < s.max_stack()))
}

/// Puts `stack` into `slots`, topping up first; returns what did not fit.
fn put_into(slots: &mut [Stack], mut stack: Stack) -> Stack {
    for s in slots.iter_mut() {
        if !s.is_empty() && s.same_kind(&stack) && s.count < s.max_stack() {
            let moved = (s.max_stack() - s.count).min(stack.count);
            *s = s.with_count(s.count + moved);
            stack = stack.with_count(stack.count - moved);
        }
    }
    for s in slots.iter_mut() {
        if stack.is_empty() {
            break;
        }
        if s.is_empty() {
            *s = stack;
            stack = Stack::EMPTY;
        }
    }
    stack
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
        let Some(kind) = CartKind::from_name(ItemInfo::get(stack.item).map_or("", |i| i.name))
        else {
            return false;
        };
        let survival = p.survival();
        let state = self.block(pos);
        if !is_rail(state) {
            return false;
        }
        let at = center(pos, shape_of(state));
        let cart = Cart::new(at, &mut self.rng).of_kind(kind);
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
        // A chest or hopper minecart opens instead; a furnace one takes coal.
        match cart.kind {
            CartKind::Chest | CartKind::Hopper => {
                if near && p.alive {
                    self.open_cart_chest(id, target);
                }
                return true;
            }
            CartKind::Furnace => {
                if near && p.alive {
                    self.fuel_cart(id, target);
                }
                return true;
            }
            CartKind::Tnt => return true,
            CartKind::Ride => {}
        }
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
            self.close_cart_viewers(target);
            if let Some(mut cart) = self.carts.remove(&target) {
                self.broadcast(items::remove_entity(target), None);
                // Its contents spill out (loot not yet rolled is rolled first).
                self.roll_cart_loot(&mut cart);
                for &s in cart.chest.iter().flat_map(|c| c.iter()) {
                    if !s.is_empty() {
                        self.scatter_item(cart.pos, s);
                    }
                }
                // A TNT minecart broken while moving fast, or burning, goes off.
                let speed = cart.vel[0].hypot(cart.vel[2]);
                if cart.kind == CartKind::Tnt && speed > 0.1 {
                    self.cart_explodes(&cart);
                } else if !creative {
                    if let Some(item) = ItemInfo::id_of("minecraft:minecart") {
                        self.scatter_item(cart.pos, Stack::new(item, 1));
                    }
                    if let Some(block) = cart.kind.block_item().and_then(ItemInfo::id_of) {
                        self.scatter_item(cart.pos, Stack::new(block, 1));
                    }
                }
            }
        }
        true
    }

    /// Fills a structure's chest minecart from its loot table, the first time.
    fn roll_cart_loot(&mut self, cart: &mut Cart) {
        if let (Some((table, seed)), Some(items)) = (cart.loot.take(), cart.chest.as_mut()) {
            self.loot.fill(&table, seed, items);
        }
    }

    /// Opens a chest minecart's 27 slots.
    fn open_cart_chest(&mut self, id: i32, cart_id: i32) {
        if let Some(mut cart) = self.carts.remove(&cart_id) {
            self.roll_cart_loot(&mut cart);
            self.carts.insert(cart_id, cart);
        }
        let hopper = self
            .carts
            .get(&cart_id)
            .is_some_and(|c| c.kind == CartKind::Hopper);
        let (view, menu, title) = if hopper {
            (
                super::containers::View::CartChest(cart_id, CART_HOPPER_SLOTS),
                levitas_registry::menu::HOPPER,
                "Minecart with Hopper",
            )
        } else {
            (
                super::containers::View::CartChest(cart_id, CART_CHEST_SLOTS),
                levitas_registry::menu::GENERIC_9X3,
                "Minecart with Chest",
            )
        };
        self.open_view(id, view, menu, title);
    }

    /// Players looking into a chest minecart that is gone have the window closed.
    pub(super) fn close_cart_viewers(&mut self, cart_id: i32) {
        let viewers: Vec<(i32, i32)> = self
            .players
            .iter()
            .filter_map(|(&pid, p)| {
                let open = p.open.as_ref()?;
                matches!(open.view, super::containers::View::CartChest(c, _) if c == cart_id)
                    .then_some((pid, open.window))
            })
            .collect();
        for (pid, window) in viewers {
            self.close_container(pid);
            self.send(
                pid,
                Writer::packet(clientbound::CONTAINER_CLOSE).varint(window),
            );
        }
    }

    pub(super) fn tick_minecarts(&mut self) {
        let ids: Vec<i32> = self.carts.keys().copied().collect();
        let mut occupied = HashSet::new();
        for id in ids {
            let mut cart = self.carts.remove(&id).expect("listed");
            cart.damage = (cart.damage - 1.0).max(0.0);
            if self.chunks.contains_key(&cart.chunk()) {
                self.move_cart(&mut cart);
                if self.cart_chores(id, &mut cart) {
                    continue; // blown up
                }
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
        self.bump_carts();
    }

    /// A player uses coal or charcoal on a furnace minecart: fuel for three minutes, and
    /// it sets off away from them.
    fn fuel_cart(&mut self, id: i32, cart_id: i32) {
        let p = &self.players[&id];
        let slot = p.hand_slot(0);
        let held = p.inventory.slots[slot];
        let coal = ItemInfo::get(held.item)
            .is_some_and(|i| matches!(i.name, "minecraft:coal" | "minecraft:charcoal"));
        let (from, survival) = (p.pos, p.survival());
        let Some(cart) = self.carts.get_mut(&cart_id) else {
            return;
        };
        if coal && cart.fuel + COAL_FUEL <= MAX_FUEL {
            let lit = cart.fuel > 0;
            cart.fuel += COAL_FUEL;
            if !lit {
                self.broadcast(furnace_lit(cart_id, true), None);
            }
            if survival {
                let p = self.players.get_mut(&id).expect("checked");
                p.inventory.take(slot, 1);
                let update = p.inventory.set_slot_packet(slot);
                self.send(id, update);
            }
        }
        let cart = self.carts.get_mut(&cart_id).expect("checked");
        let d = [cart.pos[0] - from[0], cart.pos[2] - from[2]];
        let len = d[0].hypot(d[1]).max(1e-6);
        cart.push = [d[0] / len, d[1] / len];
    }

    /// What each kind of minecart does as it goes: a furnace one burns fuel and pushes
    /// on, a TNT one burns its fuse, a hopper one moves items, and a powered activator
    /// rail lights TNT, stops hoppers and puts riders off. Returns whether it blew up.
    fn cart_chores(&mut self, id: i32, cart: &mut Cart) -> bool {
        // A mob that died or left is no longer aboard.
        if cart.mob.is_some_and(|m| {
            self.mobs
                .get(&m)
                .is_none_or(|mob| !mob.alive() || mob.vehicle != Some(id))
        }) {
            cart.mob = None;
        }
        let activated = self.rail_under(cart.pos).is_some_and(|(_, s)| {
            blocks::name(s) == "minecraft:activator_rail"
                && blocks::prop(s, "powered") == Some("true")
        });
        match cart.kind {
            CartKind::Furnace if cart.fuel > 0 => {
                cart.fuel -= 1;
                let speed = cart.vel[0].hypot(cart.vel[2]);
                if speed > 0.01 {
                    // It keeps pushing the way it goes, round curves too.
                    cart.push = [cart.vel[0] / speed, cart.vel[2] / speed];
                }
                if speed < FURNACE_SPEED {
                    cart.vel[0] += cart.push[0] * FURNACE_PUSH;
                    cart.vel[2] += cart.push[1] * FURNACE_PUSH;
                }
                if cart.fuel == 0 {
                    self.broadcast(furnace_lit(id, false), None);
                }
            }
            CartKind::Ride if cart.rider.is_none() && cart.mob.is_none() => {
                // A mob it runs into gets in.
                let moving = cart.vel[0].hypot(cart.vel[2]) > 0.01;
                if moving {
                    if let Some(m) = self.mob_boarding(cart.pos, 1.0, &[]) {
                        cart.mob = Some(m);
                        if let Some(mob) = self.mobs.get_mut(&m) {
                            mob.vehicle = Some(id);
                        }
                        self.broadcast(passengers(id, Some(m)), None);
                    }
                }
            }
            CartKind::Tnt => {
                let here = BlockPos::new(
                    cart.pos[0].floor() as i32,
                    cart.pos[1].floor() as i32,
                    cart.pos[2].floor() as i32,
                );
                let burning = self.loaded_state(here).is_some_and(|s| {
                    crate::blocks::fire::is_fire(s)
                        || crate::blocks::fluid::fluid_at(s)
                            .is_some_and(|f| f.fluid == crate::blocks::fluid::Fluid::Lava)
                });
                if (activated || burning) && cart.fuse.is_none() {
                    self.light_cart(id, cart);
                }
                if let Some(fuse) = cart.fuse.as_mut() {
                    *fuse -= 1;
                    if *fuse <= 0 {
                        self.broadcast(items::remove_entity(id), None);
                        self.cart_explodes(cart);
                        return true;
                    }
                }
            }
            CartKind::Hopper => {
                cart.enabled = !activated;
                cart.cooldown = cart.cooldown.saturating_sub(1);
                if cart.enabled && cart.cooldown == 0 && self.cart_hopper_pull(cart) {
                    cart.cooldown = HOPPER_COOLDOWN;
                }
            }
            CartKind::Ride if activated => {
                if let Some(m) = cart.mob.take() {
                    if let Some(mob) = self.mobs.get_mut(&m) {
                        mob.vehicle = None;
                        mob.pos[1] += 0.6;
                    }
                    self.broadcast(passengers(id, None), None);
                }
                if let Some(rider) = cart.rider.take() {
                    self.players.get_mut(&rider).map(|p| p.riding.take());
                    self.broadcast(passengers(id, None), None);
                    let at = [cart.pos[0], cart.pos[1] + 0.6, cart.pos[2]];
                    self.teleport(rider, at);
                }
            }
            _ => {}
        }
        false
    }

    /// Lights a TNT minecart's fuse (fire, lava, explosions or an activator rail).
    pub(super) fn light_cart(&mut self, id: i32, cart: &mut Cart) {
        cart.fuse = Some(TNT_FUSE);
        // The flashing, as primed TNT shows it.
        self.broadcast(
            Writer::packet(clientbound::ENTITY_EVENT).i32(id).i8(10),
            None,
        );
        self.play_sound_at("minecraft:entity.tnt.primed", cart.pos, 1.0, 1.0, None);
    }

    /// A TNT minecart's blast, stronger the faster it went.
    fn cart_explodes(&mut self, cart: &Cart) {
        let speed2 = (cart.vel[0] * cart.vel[0] + cart.vel[2] * cart.vel[2]).min(5.0);
        let power = TNT_POWER + self.rng.f64() as f32 * 1.5 * speed2 as f32;
        self.explode(cart.pos, power, "TNT");
    }

    /// A hopper minecart takes an item from the container above it, or picks up the
    /// items lying on it. Returns whether it took one.
    fn cart_hopper_pull(&mut self, cart: &mut Cart) -> bool {
        let Some(slots) = cart.chest.as_mut() else {
            return false;
        };
        let above = BlockPos::new(
            cart.pos[0].floor() as i32,
            cart.pos[1].floor() as i32 + 1,
            cart.pos[2].floor() as i32,
        );
        // From a container above.
        let taken = self.block_entities.get(&above).and_then(|e| {
            e.stacks()
                .iter()
                .enumerate()
                .find(|(_, s)| !s.is_empty() && can_take(slots, **s))
                .map(|(i, s)| (i, *s))
        });
        if let Some((i, s)) = taken {
            if let Some(e) = self.block_entities.get_mut(&above) {
                let left = s.with_count(s.count - 1);
                e.stacks_mut()[i] = left;
            }
            self.dirty.insert(above.chunk());
            self.redstone_dirty.push(above);
            put_into(slots, s.with_count(1));
            return true;
        }
        // Items lying on it.
        let near: Vec<i32> = self
            .items
            .iter()
            .filter(|(_, item)| {
                let p = item.pos();
                (p[0] - cart.pos[0]).abs() <= 0.75
                    && (p[2] - cart.pos[2]).abs() <= 0.75
                    && (-0.25..=1.5).contains(&(p[1] - cart.pos[1]))
            })
            .map(|(&id, _)| id)
            .collect();
        for item_id in near {
            let stack = self.items[&item_id].stack_ref();
            if !can_take(slots, stack) {
                continue;
            }
            let left = put_into(slots, stack);
            if left.is_empty() {
                self.items.remove(&item_id);
                self.broadcast(items::remove_entity(item_id), None);
            } else {
                self.set_item_stack(item_id, left);
            }
            return true;
        }
        false
    }

    /// A hopper takes an item from a chest or hopper minecart right above it. Returns
    /// whether it did.
    pub(super) fn hopper_pull_cart(&mut self, hopper: BlockPos) -> bool {
        let above = offset(hopper, Dir::Up);
        let cart_id = self.carts.iter().find_map(|(&id, c)| {
            let at = BlockPos::new(
                c.pos[0].floor() as i32,
                c.pos[1].floor() as i32,
                c.pos[2].floor() as i32,
            );
            (at == above && c.chest.is_some()).then_some(id)
        });
        let Some(cart_id) = cart_id else {
            return false;
        };
        let Some(slot) = self.carts[&cart_id]
            .chest
            .as_ref()
            .and_then(|c| c.iter().position(|s| !s.is_empty()))
        else {
            return false;
        };
        let stack = self.carts[&cart_id].chest.as_ref().expect("checked")[slot];
        let rest = self.insert_stack(hopper, Dir::Up, stack.with_count(1));
        if !rest.is_empty() {
            return false;
        }
        if let Some(c) = self.carts.get_mut(&cart_id).and_then(|c| c.chest.as_mut()) {
            c[slot] = stack.with_count(stack.count - 1);
        }
        true
    }

    /// A hopper pointing into a chest or hopper minecart fills it. Returns whether an
    /// item went.
    pub(super) fn hopper_push_cart(&mut self, hopper: BlockPos, target: BlockPos) -> bool {
        let cart_id = self.carts.iter().find_map(|(&id, c)| {
            let at = BlockPos::new(
                c.pos[0].floor() as i32,
                c.pos[1].floor() as i32,
                c.pos[2].floor() as i32,
            );
            (at == target && c.chest.is_some()).then_some(id)
        });
        let Some(cart_id) = cart_id else {
            return false;
        };
        let Some((slot, stack)) = self.block_entities.get(&hopper).and_then(|e| {
            e.stacks()
                .iter()
                .enumerate()
                .find(|(_, s)| !s.is_empty())
                .map(|(i, s)| (i, *s))
        }) else {
            return false;
        };
        let Some(slots) = self.carts.get_mut(&cart_id).and_then(|c| c.chest.as_mut()) else {
            return false;
        };
        if !can_take(slots, stack) {
            return false;
        }
        put_into(slots, stack.with_count(1));
        if let Some(e) = self.block_entities.get_mut(&hopper) {
            e.stacks_mut()[slot] = stack.with_count(stack.count - 1);
        }
        self.dirty.insert(hopper.chunk());
        true
    }

    /// Minecarts running into each other push apart.
    fn bump_carts(&mut self) {
        let list: Vec<(i32, [f64; 3])> = self.carts.iter().map(|(&id, c)| (id, c.pos)).collect();
        for (i, &(a, pa)) in list.iter().enumerate() {
            for &(b, pb) in &list[i + 1..] {
                let d = [pb[0] - pa[0], pb[2] - pa[2]];
                let len = d[0].hypot(d[1]);
                if len >= BUMP_DISTANCE || (pb[1] - pa[1]).abs() > 0.8 || len < 1e-6 {
                    continue;
                }
                let push = (BUMP_DISTANCE - len) * 0.1;
                let n = [d[0] / len * push, d[1] / len * push];
                if let Some(c) = self.carts.get_mut(&a) {
                    c.vel[0] -= n[0];
                    c.vel[2] -= n[1];
                }
                if let Some(c) = self.carts.get_mut(&b) {
                    c.vel[0] += n[0];
                    c.vel[2] += n[1];
                }
            }
        }
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

    #[test]
    fn mineshaft_chest_minecarts_roll_their_loot_when_opened_and_spill_when_broken() {
        let mut w = world();
        let nbt = Compound::new()
            .with("id", "minecraft:chest_minecart")
            .with(
                "Pos",
                vec![Tag::Double(4.5), Tag::Double(-60.0), Tag::Double(4.5)],
            )
            .with("LootTable", "minecraft:chests/abandoned_mineshaft")
            .with("LootTableSeed", 11i64);
        w.spawn_structure_mobs(&[nbt]);
        let (&cart_id, cart) = w.carts.iter().next().expect("the cart came out");
        assert!(cart.loot.is_some() && cart.chest.is_some());
        // Saved as it is, loot still to roll.
        let saved = cart.to_nbt();
        assert_eq!(saved.str("id"), Some("minecraft:chest_minecart"));
        assert!(saved.str("LootTable").is_some());

        let (out, _frames) = tokio::sync::mpsc::channel(100_000);
        let profile = crate::connection::Profile {
            name: "Miner".to_owned(),
            uuid: 9,
            properties: Vec::new(),
        };
        let pid = next_entity_id();
        w.players
            .insert(pid, Player::new(profile, out, [4.5, -60.0, 6.0]));
        assert!(w.board_cart(pid, cart_id));
        let cart = &w.carts[&cart_id];
        assert!(cart.loot.is_none());
        let filled = cart
            .chest
            .as_ref()
            .unwrap()
            .iter()
            .filter(|s| !s.is_empty())
            .count();
        assert!(filled > 0, "the loot was rolled");
        assert_eq!(
            w.players[&pid].open.as_ref().map(|o| o.view),
            Some(super::super::containers::View::CartChest(
                cart_id,
                CART_CHEST_SLOTS
            ))
        );

        // Broken: the window closes and everything drops, chest and minecart too.
        for _ in 0..10 {
            w.hit_cart(pid, cart_id);
        }
        assert!(!w.carts.contains_key(&cart_id));
        assert!(w.players[&pid].open.is_none());
        assert!(w.items.len() >= filled + 2);
    }

    #[test]
    fn furnace_hopper_and_tnt_minecarts_do_their_jobs() {
        let mut w = world();
        for x in 0..=15 {
            w.set_block(
                BlockPos::new(x, -60, 2),
                rail("minecraft:rail", "east_west"),
            );
        }
        // A fuelled furnace minecart sets off and keeps going.
        let mut furnace = Cart::new([2.5, -60.0, 2.5], &mut w.rng).of_kind(CartKind::Furnace);
        furnace.fuel = 200;
        furnace.push = [1.0, 0.0];
        let fid = next_entity_id();
        w.carts.insert(fid, furnace);
        for _ in 0..40 {
            w.tick_minecarts();
        }
        assert!(
            w.carts[&fid].pos[0] > 4.0,
            "pushed itself: {:?}",
            w.carts[&fid].pos
        );
        assert!(w.carts[&fid].fuel < 200);
        let saved = w.carts[&fid].to_nbt();
        assert_eq!(saved.str("id"), Some("minecraft:furnace_minecart"));
        let back = Cart::from_nbt(&saved, &mut w.rng).unwrap();
        assert_eq!(back.kind, CartKind::Furnace);
        w.carts.remove(&fid);

        // A hopper minecart takes from a chest above it; a hopper under the rail takes
        // from the minecart.
        let z = 6;
        w.set_block(
            BlockPos::new(3, -60, z),
            rail("minecraft:rail", "east_west"),
        );
        let chest = BlockPos::new(3, -59, z);
        w.set_block(
            chest,
            BlockInfo::by_name("minecraft:chest").unwrap().default_state,
        );
        let state = w.block(chest);
        w.block_entities
            .insert(chest, BlockEntity::for_block(state).unwrap());
        let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
        w.block_entities.get_mut(&chest).unwrap().stacks_mut()[0] = Stack::new(dirt, 2);
        let hid = next_entity_id();
        w.carts.insert(
            hid,
            Cart::new([3.5, -60.0, 6.5], &mut w.rng).of_kind(CartKind::Hopper),
        );
        for _ in 0..20 {
            w.tick_minecarts();
        }
        let carried: i32 = w.carts[&hid]
            .chest
            .as_ref()
            .unwrap()
            .iter()
            .map(|s| s.count)
            .sum();
        assert_eq!(carried, 2);
        let below = BlockPos::new(3, -61, z);
        w.set_block(
            below,
            BlockInfo::by_name("minecraft:hopper")
                .unwrap()
                .default_state,
        );
        let state = w.block(below);
        w.block_entities
            .insert(below, BlockEntity::for_block(state).unwrap());
        assert!(w.hopper_pull_cart(below));
        assert_eq!(w.block_entities[&below].stacks()[0].count, 1);

        // A powered activator rail lights a TNT minecart, which goes off.
        let mut tnt_rail = rail("minecraft:activator_rail", "east_west");
        tnt_rail = blocks::with_bool(tnt_rail, "powered", true);
        w.set_block(BlockPos::new(10, -60, 2), tnt_rail);
        let tid = next_entity_id();
        w.carts.insert(
            tid,
            Cart::new([10.5, -60.0, 2.5], &mut w.rng).of_kind(CartKind::Tnt),
        );
        w.tick_minecarts();
        assert!(w.carts[&tid].fuse.is_some(), "lit");
        for _ in 0..(TNT_FUSE + 2) {
            w.tick_minecarts();
        }
        assert!(!w.carts.contains_key(&tid), "blown up");
    }

    #[test]
    fn minecarts_push_each_other_apart() {
        let mut w = world();
        for x in 0..=10 {
            w.set_block(
                BlockPos::new(x, -60, 2),
                rail("minecraft:rail", "east_west"),
            );
        }
        let a = next_entity_id();
        let b = next_entity_id();
        w.carts.insert(a, Cart::new([5.0, -60.0, 2.5], &mut w.rng));
        w.carts.insert(b, Cart::new([5.4, -60.0, 2.5], &mut w.rng));
        for _ in 0..20 {
            w.tick_minecarts();
        }
        let gap = (w.carts[&b].pos[0] - w.carts[&a].pos[0]).abs();
        assert!(gap > 0.6, "they moved apart: {gap}");
    }

    #[test]
    fn a_moving_minecart_picks_up_a_mob() {
        let mut w = world();
        for x in 0..=10 {
            w.set_block(
                BlockPos::new(x, -60, 2),
                rail("minecraft:rail", "east_west"),
            );
        }
        let pig = next_entity_id();
        let mob = super::super::mobs::Mob::new(
            super::super::mobs::MobKind::Pig,
            [3.5, -60.0, 2.5],
            &mut w.rng,
        );
        w.mobs.insert(pig, mob);
        let mut cart = Cart::new([2.5, -60.0, 2.5], &mut w.rng);
        cart.vel = [0.2, 0.0, 0.0];
        let id = next_entity_id();
        w.carts.insert(id, cart);
        for _ in 0..10 {
            w.tick_minecarts();
        }
        assert_eq!(w.carts[&id].mob, Some(pig));
        assert_eq!(w.mobs[&pig].vehicle, Some(id));
    }
}
