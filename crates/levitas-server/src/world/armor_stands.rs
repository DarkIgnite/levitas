//! Armor stands, following the Minecraft Wiki: placed facing the player (to the nearest
//! eighth of a turn), dressed by right-clicking with armor, which goes to its slot
//! (swapping with what was there), and undressed by right-clicking the part with an
//! empty hand. Two quick hits break one, dropping it and what it wore; in creative one
//! hit removes it. It falls when nothing holds it up.

use levitas_nbt::{Compound, Tag};
use levitas_registry::entity_type;

use super::*;
use crate::blocks::{offset, Dir};

/// Equipment slots, as the protocol numbers them.
const MAINHAND: usize = 0;
const OFFHAND: usize = 1;
const FEET: usize = 2;
const LEGS: usize = 3;
const CHEST: usize = 4;
const HEAD: usize = 5;
/// The names `equipment` is saved under, by slot.
const SLOT_NAMES: [&str; 6] = ["mainhand", "offhand", "feet", "legs", "chest", "head"];
/// A second hit within this many ticks breaks it.
const BREAK_TICKS: u64 = 5;
const ENTITY_EVENT_HIT: i8 = 32;
const GRAVITY: f64 = 0.04;

pub struct ArmorStand {
    uuid: u128,
    pub pos: [f64; 3],
    yaw: f32,
    pub equipment: [Stack; 6],
    last_hit: u64,
    fall: f64,
    sent_pos: [f64; 3],
    /// Floats where it was put (NoGravity).
    no_gravity: bool,
    /// The saved entity as read: what Levitas does not manage (poses, small, arms,
    /// invisible, marker, names, ...) is written back unchanged.
    saved: Compound,
}

impl ArmorStand {
    fn new(pos: [f64; 3], yaw: f32, rng: &mut Rng) -> Self {
        ArmorStand {
            uuid: (u128::from(rng.next_u64()) << 64) | u128::from(rng.next_u64()),
            pos,
            yaw,
            equipment: [Stack::EMPTY; 6],
            last_hit: 0,
            fall: 0.0,
            sent_pos: pos,
            no_gravity: false,
            saved: Compound::new(),
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
        let mut equipment = Compound::new();
        for (slot, stack) in self.equipment.iter().enumerate() {
            if !stack.is_empty() {
                equipment.insert(SLOT_NAMES[slot], stack.to_nbt());
            }
        }
        let mut base = self.saved.clone();
        // The old ways of saving equipment are now in `equipment`.
        base.remove("ArmorItems");
        base.remove("HandItems");
        base.with("id", "minecraft:armor_stand")
            .with("UUID", uuid.to_vec())
            .with(
                "Pos",
                self.pos.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with("Rotation", vec![Tag::Float(self.yaw), Tag::Float(0.0)])
            .with("equipment", equipment)
    }

    pub fn from_nbt(nbt: &Compound, rng: &mut Rng) -> Option<ArmorStand> {
        if nbt.str("id")? != "minecraft:armor_stand" {
            return None;
        }
        let pos: Vec<f64> = nbt.list("Pos")?.iter().filter_map(Tag::as_f64).collect();
        let &[x, y, z] = &pos[..] else {
            return None;
        };
        let yaw = match nbt.list("Rotation").and_then(|r| r.first()) {
            Some(Tag::Float(yaw)) => *yaw,
            _ => 0.0,
        };
        let mut stand = ArmorStand::new([x, y, z], yaw, rng);
        stand.no_gravity = nbt.int("NoGravity").unwrap_or(0) != 0;
        stand.saved = nbt.clone();
        if let Some(&[a, b, c, d]) = nbt.int_array("UUID") {
            stand.uuid = (u128::from(a as u32) << 96)
                | (u128::from(b as u32) << 64)
                | (u128::from(c as u32) << 32)
                | u128::from(d as u32);
        }
        if let Some(e) = nbt.compound("equipment") {
            for (slot, name) in SLOT_NAMES.iter().enumerate() {
                if let Some(s) = e.compound(name) {
                    stand.equipment[slot] = Stack::from_nbt(s);
                }
            }
        } else {
            // Before 1.21.5: armor feet first, then the two hands.
            let list = |key: &str| -> Vec<Stack> {
                nbt.list(key)
                    .unwrap_or_default()
                    .iter()
                    .map(|t| t.as_compound().map_or(Stack::EMPTY, Stack::from_nbt))
                    .collect()
            };
            for (i, s) in list("ArmorItems").into_iter().take(4).enumerate() {
                stand.equipment[FEET + i] = s;
            }
            for (i, s) in list("HandItems").into_iter().take(2).enumerate() {
                stand.equipment[MAINHAND + i] = s;
            }
        }
        Some(stand)
    }
}

pub fn spawn_packets(id: i32, stand: &ArmorStand) -> Vec<Writer> {
    let angle = (stand.yaw.rem_euclid(360.0) / 360.0 * 256.0) as i32 as u8;
    let mut packets = vec![Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(stand.uuid)
        .varint(entity_type::ARMOR_STAND)
        .f64(stand.pos[0])
        .f64(stand.pos[1])
        .f64(stand.pos[2])
        .lp_vec3([0.0, 0.0, 0.0])
        .u8(0)
        .u8(angle)
        .u8(angle)
        .varint(0)];
    packets.push(equipment_packet(id, stand));
    if let Some(data) = stand_data(id, stand) {
        packets.push(data);
    }
    packets
}

/// Entity data of a stand's looks as saved: small, with arms, without its base plate,
/// a marker, invisible, and the pose of each part.
fn stand_data(id: i32, stand: &ArmorStand) -> Option<Writer> {
    const DATA_SHARED: u8 = 0;
    const DATA_FLAGS: u8 = 15;
    const SERIALIZER_BYTE: i32 = 0;
    const SERIALIZER_ROTATIONS: i32 = 9;
    let nbt = &stand.saved;
    let on = |key: &str| nbt.int(key).unwrap_or(0) != 0;
    let flags = u8::from(on("Small"))
        | if on("ShowArms") { 0x04 } else { 0 }
        | if on("NoBasePlate") { 0x08 } else { 0 }
        | if on("Marker") { 0x10 } else { 0 };
    let pose = nbt.compound("Pose");
    let parts = ["Head", "Body", "LeftArm", "RightArm", "LeftLeg", "RightLeg"];
    let posed = pose.is_some_and(|p| parts.iter().any(|k| p.get(k).is_some()));
    if flags == 0 && !posed && !on("Invisible") {
        return None;
    }
    let mut packet = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_FLAGS)
        .varint(SERIALIZER_BYTE)
        .u8(flags);
    if on("Invisible") {
        packet = packet.u8(DATA_SHARED).varint(SERIALIZER_BYTE).u8(0x20);
    }
    if let Some(pose) = pose {
        for (i, part) in parts.iter().enumerate() {
            let angles: Vec<f32> = pose
                .list(part)
                .unwrap_or_default()
                .iter()
                .filter_map(|t| t.as_f64().map(|v| v as f32))
                .collect();
            if let [x, y, z] = angles[..] {
                packet = packet
                    .u8(16 + i as u8)
                    .varint(SERIALIZER_ROTATIONS)
                    .f32(x)
                    .f32(y)
                    .f32(z);
            }
        }
    }
    Some(packet.u8(0xFF))
}

impl ArmorStand {
    /// Whether the stand has arms to hold items.
    fn has_arms(&self) -> bool {
        self.saved.int("ShowArms").unwrap_or(0) != 0
    }

    /// Whether it is a marker: no hitbox, nothing to click or hit.
    fn is_marker(&self) -> bool {
        self.saved.int("Marker").unwrap_or(0) != 0
    }
}

/// Every slot of the stand, worn or empty.
fn equipment_packet(id: i32, stand: &ArmorStand) -> Writer {
    let mut packet = Writer::packet(clientbound::SET_EQUIPMENT).varint(id);
    for (slot, stack) in stand.equipment.iter().enumerate() {
        // The top bit says another entry follows.
        let more = if slot + 1 < stand.equipment.len() {
            0x80
        } else {
            0
        };
        packet = crate::inventory::write_stack(packet.u8(slot as u8 | more), *stack);
    }
    packet
}

/// The slot an item is worn in on a stand, if it is worn at all.
fn worn_slot(item: i32) -> Option<usize> {
    match ItemInfo::get(item)?.equip_slot? {
        "head" => Some(HEAD),
        "chest" => Some(CHEST),
        "legs" => Some(LEGS),
        "feet" => Some(FEET),
        _ => None,
    }
}

/// The slot of the part a player clicked, `y` blocks above the stand's feet.
fn clicked_slot(stand: &ArmorStand, y: f64) -> usize {
    let has = |s: usize| !stand.equipment[s].is_empty();
    if (0.1..0.55).contains(&y) && has(FEET) {
        FEET
    } else if (0.9..1.6).contains(&y) && has(CHEST) {
        CHEST
    } else if (0.4..1.2).contains(&y) && has(LEGS) {
        LEGS
    } else if y >= 1.6 && has(HEAD) {
        HEAD
    } else if !has(MAINHAND) && has(OFFHAND) {
        OFFHAND
    } else {
        MAINHAND
    }
}

impl World {
    /// An armor stand used on a block face stands there, facing the player. Returns
    /// whether the held item was one.
    pub(super) fn place_armor_stand(
        &mut self,
        id: i32,
        slot: usize,
        clicked: BlockPos,
        face: Dir,
    ) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let stack = p.inventory.slots[slot];
        if ItemInfo::get(stack.item).is_none_or(|i| i.name != "minecraft:armor_stand") {
            return false;
        }
        let (yaw, survival) = (p.yaw, p.survival());
        let foot = offset(clicked, face);
        let free = |w: &World, q: BlockPos| {
            w.loaded_state(q)
                .is_some_and(|s| levitas_registry::collision_boxes(s).is_empty())
        };
        if !free(self, foot) || !free(self, offset(foot, Dir::Up)) {
            return true;
        }
        // Facing the player, to the nearest eighth of a turn.
        let facing = (((yaw - 180.0).rem_euclid(360.0) + 22.5) / 45.0).floor() * 45.0;
        let at = [
            f64::from(foot.x) + 0.5,
            f64::from(foot.y),
            f64::from(foot.z) + 0.5,
        ];
        // Not inside another entity: a stand, a player or a mob in that space.
        let overlaps = |pos: [f64; 3], width: f64, height: f64| {
            let reach = (width + 0.5) / 2.0;
            (pos[0] - at[0]).abs() < reach
                && (pos[2] - at[2]).abs() < reach
                && pos[1] < at[1] + 1.975
                && pos[1] + height > at[1]
        };
        let occupied = self
            .armor_stands
            .values()
            .any(|s| overlaps(s.pos, 0.5, 1.975))
            || self
                .players
                .values()
                .any(|p| p.alive && overlaps(p.pos, 0.6, 1.8))
            || self.mobs.values().any(|m| {
                let (w, h) = m.size();
                m.alive() && overlaps(m.pos, w, h)
            });
        if occupied {
            return true;
        }
        let stand = ArmorStand::new(at, facing, &mut self.rng);
        let sid = next_entity_id();
        for packet in spawn_packets(sid, &stand) {
            self.broadcast(packet, None);
        }
        self.armor_stands.insert(sid, stand);
        self.play_sound("minecraft:entity.armor_stand.place", foot, None);
        if survival {
            let p = self.players.get_mut(&id).expect("checked above");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        true
    }

    /// A player right-clicks a stand at height `y` on it: armor in hand goes on (and
    /// what was there comes off), an empty hand takes off the part clicked. Returns
    /// whether `target` is a stand.
    pub(super) fn use_armor_stand(&mut self, id: i32, target: i32, hand: i32, y: f64) -> bool {
        let Some(stand) = self.armor_stands.get(&target) else {
            return false;
        };
        let Some(p) = self.players.get(&id) else {
            return true;
        };
        if !super::hanging::reaches(p, stand.pos) || stand.is_marker() {
            return true;
        }
        let hand_slot = p.hand_slot(hand);
        let held = p.inventory.slots[hand_slot];
        // A small stand is half the size.
        let y = if stand.saved.int("Small").unwrap_or(0) != 0 {
            y * 2.0
        } else {
            y
        };
        let slot = if held.is_empty() {
            clicked_slot(stand, y)
        } else {
            match worn_slot(held.item) {
                Some(slot) => slot,
                // A stand with arms holds anything else in its hand (the one used).
                None if stand.has_arms() => {
                    if hand == 1 {
                        OFFHAND
                    } else {
                        MAINHAND
                    }
                }
                // Stands without arms only wear armor.
                None => return true,
            }
        };
        let worn = stand.equipment[slot];
        if held.is_empty() && worn.is_empty() {
            return true;
        }
        // One item goes on; what was worn comes back to the hand, or the inventory.
        let p = self.players.get_mut(&id).expect("checked above");
        let mut updates = Vec::new();
        let put = held.with_count(held.count.min(1));
        let mut left = 0;
        if held.count > 1 {
            p.inventory.slots[hand_slot] = held.with_count(held.count - 1);
            updates.push(p.inventory.set_slot_packet(hand_slot));
            if !worn.is_empty() {
                let (changed, leftover) = p.inventory.add(worn);
                left = leftover;
                updates.extend(changed.into_iter().map(|s| p.inventory.set_slot_packet(s)));
            }
        } else {
            p.inventory.slots[hand_slot] = worn;
            updates.push(p.inventory.set_slot_packet(hand_slot));
        }
        let player_pos = p.pos;
        for update in updates {
            self.send(id, update);
        }
        // What does not fit in a full inventory is dropped at the player.
        if left > 0 {
            self.scatter_item(player_pos, worn.with_count(left));
        }
        let stand = self.armor_stands.get_mut(&target).expect("checked above");
        stand.equipment[slot] = put;
        let packet = equipment_packet(target, stand);
        let at = stand.pos;
        self.broadcast(packet, None);
        self.play_sound_at("minecraft:item.armor.equip_generic", at, 1.0, 1.0, None);
        true
    }

    /// A player hits a stand: two quick hits break it, dropping it and what it wore; in
    /// creative one hit removes it. Returns whether `target` is a stand.
    pub(super) fn hit_armor_stand(&mut self, attacker: i32, target: i32) -> bool {
        let tick = self.tick;
        let Some(stand) = self.armor_stands.get_mut(&target) else {
            return false;
        };
        let creative = self.players.get(&attacker).is_some_and(|p| !p.survival());
        let near = self
            .players
            .get(&attacker)
            .is_some_and(|p| super::hanging::reaches(p, stand.pos));
        if !near {
            return true;
        }
        let quick = stand.last_hit > 0 && tick - stand.last_hit <= BREAK_TICKS;
        if !creative && !quick {
            stand.last_hit = tick.max(1);
            let at = stand.pos;
            self.broadcast(entity_event(target, ENTITY_EVENT_HIT), None);
            self.play_sound_at("minecraft:entity.armor_stand.hit", at, 1.0, 1.0, None);
            return true;
        }
        let stand = self.armor_stands.remove(&target).expect("checked above");
        self.broadcast(items::remove_entity(target), None);
        self.play_sound_at(
            "minecraft:entity.armor_stand.break",
            stand.pos,
            1.0,
            1.0,
            None,
        );
        if !creative {
            let at = [stand.pos[0], stand.pos[1] + 0.5, stand.pos[2]];
            for worn in stand.equipment.into_iter().filter(|s| !s.is_empty()) {
                self.scatter_item(at, worn);
            }
            if let Some(item) = ItemInfo::id_of("minecraft:armor_stand") {
                self.scatter_item(at, Stack::new(item, 1));
            }
        }
        true
    }

    /// Stands with nothing under them fall.
    pub(super) fn tick_armor_stands(&mut self) {
        let ids: Vec<i32> = self.armor_stands.keys().copied().collect();
        for id in ids {
            let mut stand = self.armor_stands.remove(&id).expect("listed");
            if self.chunks.contains_key(&stand.chunk()) && !stand.no_gravity {
                let below = BlockPos::new(
                    stand.pos[0].floor() as i32,
                    (stand.pos[1] - 0.01).floor() as i32,
                    stand.pos[2].floor() as i32,
                );
                let supported = self
                    .loaded_state(below)
                    .is_some_and(|s| !levitas_registry::collision_boxes(s).is_empty());
                if supported {
                    stand.fall = 0.0;
                } else {
                    stand.fall = (stand.fall + GRAVITY).min(2.0);
                    let delta = self.clip_move(stand.pos, 0.25, 1.975, 1, -stand.fall);
                    stand.pos[1] += delta;
                }
            }
            if stand.pos[1] < f64::from(MIN_Y - 64) {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }
            if (stand.pos[1] - stand.sent_pos[1]).abs() > 1e-4 {
                stand.sent_pos = stand.pos;
                let sync = Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                    .varint(id)
                    .varint(0) // a linear path
                    .f64(stand.pos[0])
                    .f64(stand.pos[1])
                    .f64(stand.pos[2])
                    .f32(stand.yaw)
                    .f32(0.0)
                    .bool(false);
                self.broadcast(sync, None);
            }
            self.armor_stands.insert(id, stand);
        }
    }

    pub(super) fn armor_stand_packets(&self) -> Vec<Writer> {
        self.armor_stands
            .iter()
            .flat_map(|(&id, s)| spawn_packets(id, s))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn saved_stands_keep_their_pose_and_float() {
        let saved = Compound::new()
            .with("id", "minecraft:armor_stand")
            .with(
                "Pos",
                vec![Tag::Double(0.5), Tag::Double(70.0), Tag::Double(0.5)],
            )
            .with("NoGravity", true)
            .with("Small", true)
            .with("ShowArms", true)
            .with(
                "Pose",
                Compound::new().with(
                    "Head",
                    vec![Tag::Float(10.0), Tag::Float(0.0), Tag::Float(0.0)],
                ),
            );
        let stand = ArmorStand::from_nbt(&saved, &mut Rng::new(1)).unwrap();
        assert!(stand.no_gravity);
        let out = stand.to_nbt();
        for key in ["NoGravity", "Small", "ShowArms", "Pose"] {
            assert!(out.get(key).is_some(), "{key} kept");
        }
    }

    #[test]
    fn stands_wear_armor_give_it_back_and_break() {
        let dir = std::env::temp_dir().join(format!("levitas-stand-{}", std::process::id()));
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
        let hand = w.players[&alice].hand_slot(0);
        let give = |w: &mut World, item: &str| {
            w.players.get_mut(&alice).unwrap().inventory.slots[hand] =
                Stack::new(ItemInfo::id_of(&format!("minecraft:{item}")).unwrap(), 1);
        };
        give(&mut w, "armor_stand");
        assert!(w.place_armor_stand(alice, hand, BlockPos::new(4, -61, 2), Dir::Up));
        let (&sid, stand) = w.armor_stands.iter().next().unwrap();
        assert_eq!(stand.pos, [4.5, -60.0, 2.5]);

        give(&mut w, "iron_helmet");
        w.use_armor_stand(alice, sid, 0, 1.8);
        give(&mut w, "diamond_chestplate");
        w.use_armor_stand(alice, sid, 0, 1.2);
        let helmet = ItemInfo::id_of("minecraft:iron_helmet").unwrap();
        assert_eq!(w.armor_stands[&sid].equipment[HEAD].item, helmet);
        assert!(w.players[&alice].inventory.slots[hand].is_empty());
        // Saved and loaded, it keeps its armor.
        let back = ArmorStand::from_nbt(&w.armor_stands[&sid].to_nbt(), &mut w.rng).unwrap();
        assert_eq!(back.equipment, w.armor_stands[&sid].equipment);
        // An empty hand on its head takes the helmet.
        w.use_armor_stand(alice, sid, 0, 1.8);
        assert_eq!(w.players[&alice].inventory.slots[hand].item, helmet);
        assert!(w.armor_stands[&sid].equipment[HEAD].is_empty());

        // One hit only wobbles it; a quick second breaks it.
        w.tick = 100;
        w.hit_armor_stand(alice, sid);
        assert!(w.armor_stands.contains_key(&sid));
        w.tick = 103;
        w.hit_armor_stand(alice, sid);
        assert!(!w.armor_stands.contains_key(&sid));
    }

    #[test]
    fn stands_with_arms_hold_items_and_show_their_pose() {
        let dir = std::env::temp_dir().join(format!("levitas-stand2-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let nbt = Compound::new()
            .with("id", "minecraft:armor_stand")
            .with(
                "Pos",
                vec![Tag::Double(3.5), Tag::Double(-60.0), Tag::Double(3.5)],
            )
            .with("ShowArms", true)
            .with("Small", true)
            .with(
                "Pose",
                Compound::new().with(
                    "RightArm",
                    vec![Tag::Float(-90.0), Tag::Float(0.0), Tag::Float(0.0)],
                ),
            );
        let mut stand = ArmorStand::from_nbt(&nbt, &mut w.rng).unwrap();
        let packets = spawn_packets(7, &stand);
        assert_eq!(packets.len(), 3, "spawn, equipment and looks");
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
        stand.pos = [3.5, -60.0, 3.5];
        let sid = next_entity_id();
        w.armor_stands.insert(sid, stand);
        let hand = w.players[&alice].hand_slot(0);
        let sword = ItemInfo::id_of("minecraft:iron_sword").unwrap();
        w.players.get_mut(&alice).unwrap().inventory.slots[hand] = Stack::new(sword, 1);
        w.use_armor_stand(alice, sid, 0, 0.5);
        assert_eq!(w.armor_stands[&sid].equipment[MAINHAND].item, sword);
    }
}
