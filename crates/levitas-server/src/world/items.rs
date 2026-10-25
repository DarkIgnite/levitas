//! Dropped item entities: spawning, simple physics, pickup and despawning. The client
//! simulates item motion itself; the server runs a rough copy so pickups happen where the
//! item visibly lies.

use super::*;

/// Ticks before an item can be picked up.
const PICKUP_DELAY_BROKEN: u32 = 10;
const PICKUP_DELAY_THROWN: u32 = 40;
/// Items vanish after five minutes.
const DESPAWN_AGE: u32 = 6000;
const GRAVITY: f64 = 0.04;
/// Entity data index of the item stack, and the item stack serializer.
const DATA_ITEM: u8 = 8;
const SERIALIZER_ITEM_STACK: i32 = 7;

pub struct ItemEntity {
    stack: Stack,
    pos: [f64; 3],
    vel: [f64; 3],
    on_ground: bool,
    age: u32,
    pickup_delay: u32,
}

pub fn spawn_packets(id: i32, item: &ItemEntity) -> [Writer; 2] {
    // Velocity is sent in 1/8000 blocks per tick.
    let vel = |v: f64| (v.clamp(-3.9, 3.9) * 8000.0) as i16;
    [
        Writer::packet(clientbound::ADD_ENTITY)
            .varint(id)
            .uuid(id as u128) // unique enough for entities that are never persisted
            .varint(entity_type::ITEM)
            .f64(item.pos[0])
            .f64(item.pos[1])
            .f64(item.pos[2])
            .u8(0)
            .u8(0)
            .u8(0) // pitch, yaw, head yaw
            .varint(0) // entity data
            .i16(vel(item.vel[0]))
            .i16(vel(item.vel[1]))
            .i16(vel(item.vel[2])),
        item_data(id, item.stack),
    ]
}

fn item_data(id: i32, stack: Stack) -> Writer {
    let packet = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_ITEM)
        .varint(SERIALIZER_ITEM_STACK);
    crate::inventory::write_stack(packet, stack).u8(0xFF) // end of entity data
}

fn remove_entity(id: i32) -> Writer {
    Writer::packet(clientbound::REMOVE_ENTITIES)
        .varint(1)
        .varint(id)
}

/// Whether an item rests on (or is blocked by) this block.
fn solid(state: u16) -> bool {
    !is_air(state) && !block_has_tag(state, "minecraft:replaceable")
}

impl World {
    fn spawn_item(&mut self, stack: Stack, pos: [f64; 3], vel: [f64; 3], pickup_delay: u32) {
        if stack.is_empty() {
            return;
        }
        let id = next_entity_id();
        let item = ItemEntity {
            stack,
            pos,
            vel,
            on_ground: false,
            age: 0,
            pickup_delay,
        };
        for packet in spawn_packets(id, &item) {
            self.broadcast(packet, None);
        }
        self.items.insert(id, item);
    }

    /// Pops an item out of a broken block, like vanilla's block drops.
    pub(super) fn drop_at_block(&mut self, pos: BlockPos, stack: Stack) {
        let jitter = |rng: &mut Rng| rng.f64() * 0.5 - 0.25;
        let at = [
            f64::from(pos.x) + 0.5 + jitter(&mut self.rng),
            f64::from(pos.y) + 0.5 + jitter(&mut self.rng),
            f64::from(pos.z) + 0.5 + jitter(&mut self.rng),
        ];
        let vel = [self.rng.f64() * 0.2 - 0.1, 0.2, self.rng.f64() * 0.2 - 0.1];
        self.spawn_item(stack, at, vel, PICKUP_DELAY_BROKEN);
    }

    /// Scatters an item in a random direction, as a dying player's inventory does.
    pub(super) fn scatter_item(&mut self, pos: [f64; 3], stack: Stack) {
        let speed = self.rng.f64() * 0.5;
        let angle = self.rng.f64() * std::f64::consts::TAU;
        let vel = [-angle.sin() * speed, 0.2, angle.cos() * speed];
        let at = [pos[0], pos[1] + 1.32, pos[2]];
        self.spawn_item(stack, at, vel, PICKUP_DELAY_THROWN);
    }

    /// Throws an item from a player's eyes in the direction they look.
    pub(super) fn throw_from_player(&mut self, id: i32, stack: Stack) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let (yaw, pitch) = (
            f64::from(p.yaw).to_radians(),
            f64::from(p.pitch).to_radians(),
        );
        let at = [p.pos[0], p.pos[1] + 1.32, p.pos[2]];
        let vel = [
            -yaw.sin() * pitch.cos() * 0.3,
            -pitch.sin() * 0.3 + 0.1,
            yaw.cos() * pitch.cos() * 0.3,
        ];
        self.spawn_item(stack, at, vel, PICKUP_DELAY_THROWN);
    }

    /// Drops one item, or the whole stack, from the selected hotbar slot (the Q key).
    pub(super) fn drop_held(&mut self, id: i32, whole_stack: bool) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let slot = p.hand_slot(0);
        let amount = if whole_stack { i32::MAX } else { 1 };
        let stack = p.inventory.take(slot, amount);
        if stack.is_empty() {
            return;
        }
        let update = p.inventory.set_slot_packet(slot);
        self.send(id, update);
        self.throw_from_player(id, stack);
    }

    pub(super) fn tick_items(&mut self) {
        let ids: Vec<i32> = self.items.keys().copied().collect();
        for id in ids {
            let mut item = self.items.remove(&id).expect("item exists");
            item.age += 1;
            item.pickup_delay = item.pickup_delay.saturating_sub(1);
            self.move_item(&mut item);

            if item.age > DESPAWN_AGE || item.pos[1] < f64::from(MIN_Y - 64) {
                self.broadcast(remove_entity(id), None);
                continue;
            }
            if item.pickup_delay == 0 && self.pick_up(id, &mut item) {
                continue;
            }
            self.items.insert(id, item);
        }
    }

    fn move_item(&mut self, item: &mut ItemEntity) {
        let cell = |p: [f64; 3]| {
            BlockPos::new(
                p[0].floor() as i32,
                p[1].floor() as i32,
                p[2].floor() as i32,
            )
        };
        // Items in chunks nobody has loaded stay frozen, as in vanilla.
        if !self.chunks.contains_key(&cell(item.pos).chunk()) {
            return;
        }
        if item.on_ground {
            let below = cell([item.pos[0], item.pos[1] - 0.01, item.pos[2]]);
            if !solid(self.block(below)) {
                item.on_ground = false;
            }
        }
        if item.on_ground && item.vel[0].abs() < 1e-3 && item.vel[2].abs() < 1e-3 {
            return;
        }
        if !item.on_ground {
            item.vel[1] -= GRAVITY;
        }
        for axis in [0, 2] {
            let mut next = item.pos;
            next[axis] += item.vel[axis];
            if solid(self.block(cell(next))) {
                item.vel[axis] = 0.0;
            } else {
                item.pos = next;
            }
        }
        let mut next = item.pos;
        next[1] += item.vel[1];
        let landing = cell(next);
        if item.vel[1] <= 0.0 && solid(self.block(landing)) {
            item.pos[1] = f64::from(landing.y) + 1.0;
            item.vel[1] = 0.0;
            item.on_ground = true;
        } else {
            item.pos = next;
        }
        let friction = if item.on_ground { 0.6 * 0.98 } else { 0.98 };
        item.vel[0] *= friction;
        item.vel[1] *= 0.98;
        item.vel[2] *= friction;
    }

    /// Gives the item to a player touching it. Returns whether it was used up.
    fn pick_up(&mut self, id: i32, item: &mut ItemEntity) -> bool {
        // A player's pickup box is their hitbox (0.6 x 1.8) grown by 1 x 0.5 x 1.
        let collector = self.players.iter_mut().find(|(_, p)| {
            let d = [0, 1, 2].map(|i| item.pos[i] - p.pos[i]);
            p.alive && d[0].abs() <= 1.3 && d[2].abs() <= 1.3 && (-0.5..=2.3).contains(&d[1])
        });
        let Some((&player_id, p)) = collector else {
            return false;
        };
        let (changed, left) = p.inventory.add(item.stack);
        let picked = item.stack.count - left;
        if picked == 0 {
            return false;
        }
        let updates: Vec<Writer> = changed
            .into_iter()
            .map(|slot| p.inventory.set_slot_packet(slot))
            .collect();
        // The pickup animation first, then the inventory change, as vanilla orders them.
        self.broadcast(
            Writer::packet(clientbound::TAKE_ITEM_ENTITY)
                .varint(id)
                .varint(player_id)
                .varint(picked),
            None,
        );
        for update in updates {
            self.send(player_id, update);
        }
        if left == 0 {
            self.broadcast(remove_entity(id), None);
            return true;
        }
        item.stack.count = left;
        self.broadcast(item_data(id, item.stack), None);
        false
    }
}
