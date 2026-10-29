//! Item frames, glow item frames and paintings, following the Minecraft Wiki.
//!
//! An item frame hangs on any face of a block. Right-clicking an empty one puts the held
//! item in; right-clicking a full one turns the item an eighth; hitting it takes the
//! item out, and hitting the empty frame breaks it. A painting is hung on the side of a
//! block: the biggest placeable paintings that fit the wall there are chosen from at
//! random. Both break when their wall goes, dropping as items (except in creative).

use levitas_nbt::{Compound, Tag};
use levitas_registry::{entity_type, SyncedRegistry, TagRegistry, PAINTING_VARIANTS_JSON};

use super::*;
use crate::blocks::{offset, Dir};

/// Entity data of item frames and paintings: the facing, then the item and its rotation
/// (frames) or the painting (paintings).
const DATA_DIRECTION: u8 = 8;
const DATA_ITEM: u8 = 9;
const DATA_ROTATION: u8 = 10;
const DATA_VARIANT: u8 = 9;
const SERIALIZER_VARINT: i32 = 1;
const SERIALIZER_ITEM: i32 = 7;
const SERIALIZER_DIRECTION: i32 = 12;
const SERIALIZER_PAINTING: i32 = 30;
/// Ticks between checks that hanging things still have their wall.
const SUPPORT_CHECK_TICKS: u64 = 20;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum What {
    Frame {
        glow: bool,
        item: Stack,
        rotation: u8,
    },
    /// A painting, by its ID in the painting variant registry.
    Painting { variant: u16 },
}

pub struct Hanging {
    pub what: What,
    /// The block it hangs in (for paintings, the one their size is counted from).
    pub block: BlockPos,
    /// The way it faces: away from its wall.
    pub facing: Dir,
    uuid: u128,
}

/// Directions as the protocol numbers them.
fn dir_id(d: Dir) -> i32 {
    match d {
        Dir::Down => 0,
        Dir::Up => 1,
        Dir::North => 2,
        Dir::South => 3,
        Dir::West => 4,
        Dir::East => 5,
    }
}

fn dir_from_id(id: i64) -> Dir {
    match id {
        0 => Dir::Down,
        1 => Dir::Up,
        2 => Dir::North,
        4 => Dir::West,
        5 => Dir::East,
        _ => Dir::South,
    }
}

/// Paintings save their facing as a horizontal direction: south, west, north, east.
fn painting_facing(d: Dir) -> i8 {
    match d {
        Dir::West => 1,
        Dir::North => 2,
        Dir::East => 3,
        _ => 0,
    }
}

fn painting_facing_from(id: i64) -> Dir {
    match id {
        1 => Dir::West,
        2 => Dir::North,
        3 => Dir::East,
        _ => Dir::South,
    }
}

/// The direction to the left of something facing `d`, along which a painting is wide.
fn counter_clockwise(d: Dir) -> Dir {
    match d {
        Dir::North => Dir::West,
        Dir::West => Dir::South,
        Dir::South => Dir::East,
        _ => Dir::North,
    }
}

fn registry() -> Option<&'static SyncedRegistry> {
    SyncedRegistry::get("minecraft:painting_variant")
}

/// Width and height in blocks of each painting, by registry ID.
fn painting_sizes() -> &'static [(u8, u8)] {
    static SIZES: std::sync::OnceLock<Vec<(u8, u8)>> = std::sync::OnceLock::new();
    SIZES.get_or_init(|| {
        let all: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(PAINTING_VARIANTS_JSON).expect("painting variants are JSON");
        registry()
            .map(|r| r.entries)
            .unwrap_or_default()
            .iter()
            .map(|name| {
                let v = &all[*name];
                let get = |k: &str| v[k].as_u64().unwrap_or(1).clamp(1, 16) as u8;
                (get("width"), get("height"))
            })
            .collect()
    })
}

/// The blocks a painting of `width` x `height` facing `facing` covers, from `block`.
fn painting_cells(block: BlockPos, facing: Dir, width: u8, height: u8) -> Vec<BlockPos> {
    let side = counter_clockwise(facing).step();
    let (w, h) = (i32::from(width), i32::from(height));
    let mut cells = Vec::new();
    for k in -((w - 1) / 2)..=w / 2 {
        for up in -((h - 1) / 2)..=h / 2 {
            cells.push(BlockPos::new(
                block.x + side[0] * k,
                block.y + up,
                block.z + side[2] * k,
            ));
        }
    }
    cells
}

impl Hanging {
    fn new(what: What, block: BlockPos, facing: Dir, rng: &mut Rng) -> Self {
        Hanging {
            what,
            block,
            facing,
            uuid: (u128::from(rng.next_u64()) << 64) | u128::from(rng.next_u64()),
        }
    }

    pub fn chunk(&self) -> (i32, i32) {
        self.block.chunk()
    }

    /// The blocks it takes up.
    fn cells(&self) -> Vec<BlockPos> {
        match self.what {
            What::Frame { .. } => vec![self.block],
            What::Painting { variant } => {
                let (w, h) = painting_sizes()
                    .get(usize::from(variant))
                    .copied()
                    .unwrap_or((1, 1));
                painting_cells(self.block, self.facing, w, h)
            }
        }
    }

    fn item_name(&self) -> &'static str {
        match self.what {
            What::Frame { glow: true, .. } => "minecraft:glow_item_frame",
            What::Frame { .. } => "minecraft:item_frame",
            What::Painting { .. } => "minecraft:painting",
        }
    }

    pub fn to_nbt(&self) -> Compound {
        let uuid = [
            (self.uuid >> 96) as i32,
            (self.uuid >> 64) as i32,
            (self.uuid >> 32) as i32,
            self.uuid as i32,
        ];
        let b = self.block;
        let pos = [
            f64::from(b.x) + 0.5,
            f64::from(b.y) + 0.5,
            f64::from(b.z) + 0.5,
        ];
        let mut nbt = Compound::new()
            .with("id", self.item_name())
            .with("UUID", uuid.to_vec())
            .with(
                "Pos",
                pos.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with("block_pos", vec![b.x, b.y, b.z])
            .with("TileX", b.x)
            .with("TileY", b.y)
            .with("TileZ", b.z);
        match self.what {
            What::Frame { item, rotation, .. } => {
                nbt.insert("Facing", dir_id(self.facing) as i8);
                nbt.insert("ItemRotation", rotation as i8);
                if !item.is_empty() {
                    nbt.insert("Item", item.to_nbt());
                }
            }
            What::Painting { variant } => {
                nbt.insert("facing", painting_facing(self.facing));
                let name = registry()
                    .and_then(|r| r.entries.get(usize::from(variant)))
                    .copied()
                    .unwrap_or("minecraft:kebab");
                nbt.insert("variant", name);
            }
        }
        nbt
    }

    pub fn from_nbt(nbt: &Compound, rng: &mut Rng) -> Option<Hanging> {
        let id = nbt.str("id")?;
        let block = match nbt.int_array("block_pos") {
            Some(&[x, y, z]) => BlockPos::new(x, y, z),
            _ => BlockPos::new(
                nbt.int("TileX")? as i32,
                nbt.int("TileY")? as i32,
                nbt.int("TileZ")? as i32,
            ),
        };
        let (what, facing) = match id {
            "minecraft:item_frame" | "minecraft:glow_item_frame" => (
                What::Frame {
                    glow: id == "minecraft:glow_item_frame",
                    item: nbt.compound("Item").map_or(Stack::EMPTY, Stack::from_nbt),
                    rotation: nbt.int("ItemRotation").unwrap_or(0).clamp(0, 7) as u8,
                },
                dir_from_id(nbt.int("Facing").unwrap_or(3)),
            ),
            "minecraft:painting" => {
                let variant = nbt
                    .str("variant")
                    .and_then(|v| registry()?.id_of(v))
                    .unwrap_or(0) as u16;
                let facing = nbt.int("facing").or_else(|| nbt.int("Facing")).unwrap_or(0);
                (What::Painting { variant }, painting_facing_from(facing))
            }
            _ => return None,
        };
        let mut h = Hanging::new(what, block, facing, rng);
        if let Some(&[a, b, c, d]) = nbt.int_array("UUID") {
            h.uuid = (u128::from(a as u32) << 96)
                | (u128::from(b as u32) << 64)
                | (u128::from(c as u32) << 32)
                | u128::from(d as u32);
        }
        Some(h)
    }
}

pub fn spawn_packets(id: i32, h: &Hanging) -> Vec<Writer> {
    let kind = match h.what {
        What::Frame { glow: true, .. } => entity_type::GLOW_ITEM_FRAME,
        What::Frame { .. } => entity_type::ITEM_FRAME,
        What::Painting { .. } => entity_type::PAINTING,
    };
    // Hanging things are placed by their block position and facing.
    let spawn = Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(h.uuid)
        .varint(kind)
        .f64(f64::from(h.block.x))
        .f64(f64::from(h.block.y))
        .f64(f64::from(h.block.z))
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(dir_id(h.facing))
        .i16(0)
        .i16(0)
        .i16(0);
    let data = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_DIRECTION)
        .varint(SERIALIZER_DIRECTION)
        .varint(dir_id(h.facing));
    let data = match h.what {
        What::Frame { item, rotation, .. } => {
            let data = data.u8(DATA_ITEM).varint(SERIALIZER_ITEM);
            crate::inventory::write_stack(data, item)
                .u8(DATA_ROTATION)
                .varint(SERIALIZER_VARINT)
                .varint(i32::from(rotation))
        }
        // A registry painting: its ID plus one (zero would be one defined inline).
        What::Painting { variant } => data
            .u8(DATA_VARIANT)
            .varint(SERIALIZER_PAINTING)
            .varint(i32::from(variant) + 1),
    };
    vec![spawn, data.u8(0xFF)]
}

fn frame_data(id: i32, item: Stack, rotation: u8) -> Writer {
    let data = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_ITEM)
        .varint(SERIALIZER_ITEM);
    crate::inventory::write_stack(data, item)
        .u8(DATA_ROTATION)
        .varint(SERIALIZER_VARINT)
        .varint(i32::from(rotation))
        .u8(0xFF)
}

impl World {
    /// Whether a hanging thing could cover `cells` facing `facing`: the blocks free and
    /// a solid wall behind each, and nothing else hanging there.
    fn room_to_hang(&self, cells: &[BlockPos], facing: Dir) -> bool {
        let taken: Vec<BlockPos> = self
            .hanging
            .values()
            .filter(|h| h.facing == facing)
            .flat_map(Hanging::cells)
            .collect();
        cells.iter().all(|&c| {
            let free = self
                .loaded_state(c)
                .is_some_and(|s| levitas_registry::collision_boxes(s).is_empty());
            let wall = self
                .loaded_state(offset(c, facing.opposite()))
                .is_some_and(|s| !levitas_registry::collision_boxes(s).is_empty());
            free && wall && !taken.contains(&c)
        })
    }

    /// An item frame or painting used on a block face hangs there. Returns whether
    /// the held item was one.
    pub(super) fn place_hanging(
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
        let item = ItemInfo::get(stack.item).map_or("", |i| i.name);
        let survival = p.survival();
        let block = offset(clicked, face);
        let what = match item {
            "minecraft:item_frame" | "minecraft:glow_item_frame" => {
                if !self.room_to_hang(&[block], face) {
                    return true;
                }
                What::Frame {
                    glow: item == "minecraft:glow_item_frame",
                    item: Stack::EMPTY,
                    rotation: 0,
                }
            }
            "minecraft:painting" => {
                if matches!(face, Dir::Up | Dir::Down) {
                    return true;
                }
                let placeable = TagRegistry::get("minecraft:painting_variant")
                    .map_or(&[][..], |t| t.members("minecraft:placeable"));
                let fitting: Vec<(u16, u32)> = placeable
                    .iter()
                    .filter_map(|&v| {
                        let (w, h) = *painting_sizes().get(v as usize)?;
                        let cells = painting_cells(block, face, w, h);
                        self.room_to_hang(&cells, face)
                            .then_some((v as u16, u32::from(w) * u32::from(h)))
                    })
                    .collect();
                // The biggest that fit, one of them at random.
                let biggest = fitting.iter().map(|&(_, a)| a).max();
                let choices: Vec<u16> = fitting
                    .iter()
                    .filter(|&&(_, a)| Some(a) == biggest)
                    .map(|&(v, _)| v)
                    .collect();
                if choices.is_empty() {
                    return true;
                }
                let variant = choices[self.rng.below(choices.len() as u32) as usize];
                What::Painting { variant }
            }
            _ => return false,
        };
        let hanging = Hanging::new(what, block, face, &mut self.rng);
        let hid = next_entity_id();
        for packet in spawn_packets(hid, &hanging) {
            self.broadcast(packet, None);
        }
        let sound = match what {
            What::Frame { .. } => "minecraft:entity.item_frame.place",
            What::Painting { .. } => "minecraft:entity.painting.place",
        };
        self.play_sound(sound, block, None);
        self.hanging.insert(hid, hanging);
        if survival {
            let p = self.players.get_mut(&id).expect("checked above");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        true
    }

    /// A player right-clicks a hanging thing: an empty frame takes the held item, a
    /// full one turns it. Returns whether `target` hangs.
    pub(super) fn use_hanging(&mut self, id: i32, target: i32, hand: i32) -> bool {
        let Some(h) = self.hanging.get(&target) else {
            return false;
        };
        let What::Frame {
            glow,
            item,
            rotation,
        } = h.what
        else {
            return true;
        };
        let block = h.block;
        let Some(p) = self.players.get(&id) else {
            return true;
        };
        let slot = p.hand_slot(hand);
        let held = p.inventory.slots[slot];
        let survival = p.survival();
        let (item, rotation, sound) = if item.is_empty() {
            if held.is_empty() {
                return true;
            }
            if survival {
                let p = self.players.get_mut(&id).expect("checked above");
                p.inventory.take(slot, 1);
                let update = p.inventory.set_slot_packet(slot);
                self.send(id, update);
            }
            (
                held.with_count(1),
                0,
                "minecraft:entity.item_frame.add_item",
            )
        } else {
            (
                item,
                (rotation + 1) % 8,
                "minecraft:entity.item_frame.rotate_item",
            )
        };
        self.hanging.get_mut(&target).expect("checked above").what = What::Frame {
            glow,
            item,
            rotation,
        };
        self.broadcast(frame_data(target, item, rotation), None);
        self.play_sound(sound, block, None);
        true
    }

    /// A player hits a hanging thing: a full frame gives up its item, anything else
    /// breaks. Returns whether `target` hangs.
    pub(super) fn hit_hanging(&mut self, attacker: i32, target: i32) -> bool {
        let Some(h) = self.hanging.get(&target) else {
            return false;
        };
        let creative = self.players.get(&attacker).is_some_and(|p| !p.survival());
        if let What::Frame { glow, item, .. } = h.what {
            if !item.is_empty() {
                let block = h.block;
                let at = [
                    f64::from(block.x) + 0.5,
                    f64::from(block.y) + 0.5,
                    f64::from(block.z) + 0.5,
                ];
                if !creative {
                    self.scatter_item(at, item);
                }
                self.hanging.get_mut(&target).expect("checked above").what = What::Frame {
                    glow,
                    item: Stack::EMPTY,
                    rotation: 0,
                };
                self.broadcast(frame_data(target, Stack::EMPTY, 0), None);
                self.play_sound("minecraft:entity.item_frame.remove_item", block, None);
                return true;
            }
        }
        self.break_hanging(target, !creative);
        true
    }

    fn break_hanging(&mut self, id: i32, drop: bool) {
        let Some(h) = self.hanging.remove(&id) else {
            return;
        };
        self.broadcast(items::remove_entity(id), None);
        let sound = match h.what {
            What::Frame { .. } => "minecraft:entity.item_frame.break",
            What::Painting { .. } => "minecraft:entity.painting.break",
        };
        self.play_sound(sound, h.block, None);
        if !drop {
            return;
        }
        let at = [
            f64::from(h.block.x) + 0.5,
            f64::from(h.block.y) + 0.5,
            f64::from(h.block.z) + 0.5,
        ];
        if let What::Frame { item, .. } = h.what {
            if !item.is_empty() {
                self.scatter_item(at, item);
            }
        }
        if let Some(item) = ItemInfo::id_of(h.item_name()) {
            self.scatter_item(at, Stack::new(item, 1));
        }
    }

    /// Hanging things whose wall is gone, or whose space filled up, fall off.
    pub(super) fn tick_hanging(&mut self) {
        if !self.tick.is_multiple_of(SUPPORT_CHECK_TICKS) {
            return;
        }
        let fallen: Vec<i32> = self
            .hanging
            .iter()
            .filter(|(_, h)| self.chunks.contains_key(&h.chunk()))
            .filter(|(_, h)| {
                h.cells().iter().any(|&c| {
                    let wall = self.loaded_state(offset(c, h.facing.opposite()));
                    let here = self.loaded_state(c);
                    wall.is_some_and(|s| levitas_registry::collision_boxes(s).is_empty())
                        || here.is_some_and(|s| !levitas_registry::collision_boxes(s).is_empty())
                })
            })
            .map(|(&id, _)| id)
            .collect();
        for id in fallen {
            self.break_hanging(id, true);
        }
    }

    pub(super) fn hanging_packets(&self) -> Vec<Writer> {
        self.hanging
            .iter()
            .flat_map(|(&id, h)| spawn_packets(id, h))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn setup() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-hanging-{}-{}",
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
        // A stone wall along x = 4, five wide and four high.
        for z in 0..5 {
            for y in -60..=-57 {
                w.set_block(BlockPos::new(4, y, z), levitas_registry::block::STONE);
            }
        }
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
        (w, alice)
    }

    fn hold(w: &mut World, id: i32, item: &str) -> usize {
        let p = w.players.get_mut(&id).unwrap();
        let slot = p.hand_slot(0);
        p.inventory.slots[slot] =
            Stack::new(ItemInfo::id_of(&format!("minecraft:{item}")).unwrap(), 1);
        slot
    }

    #[test]
    fn item_frames_hold_turn_and_give_back_items() {
        let (mut w, alice) = setup();
        let slot = hold(&mut w, alice, "item_frame");
        assert!(w.place_hanging(alice, slot, BlockPos::new(4, -59, 2), Dir::West));
        let (&fid, frame) = w.hanging.iter().next().unwrap();
        assert_eq!(
            (frame.block, frame.facing),
            (BlockPos::new(3, -59, 2), Dir::West)
        );
        // Not twice in the same spot.
        hold(&mut w, alice, "item_frame");
        w.place_hanging(alice, slot, BlockPos::new(4, -59, 2), Dir::West);
        assert_eq!(w.hanging.len(), 1);

        hold(&mut w, alice, "diamond");
        w.use_hanging(alice, fid, 0);
        w.use_hanging(alice, fid, 0);
        let diamond = ItemInfo::id_of("minecraft:diamond").unwrap();
        assert!(matches!(
            w.hanging[&fid].what,
            What::Frame { item, rotation: 1, .. } if item.item == diamond
        ));
        let back = Hanging::from_nbt(&w.hanging[&fid].to_nbt(), &mut w.rng).unwrap();
        assert_eq!(
            (back.what, back.block, back.facing),
            (
                w.hanging[&fid].what,
                w.hanging[&fid].block,
                w.hanging[&fid].facing
            )
        );

        // A hit takes the diamond out, the next breaks the frame.
        w.hit_hanging(alice, fid);
        assert!(matches!(w.hanging[&fid].what, What::Frame { item, .. } if item.is_empty()));
        w.hit_hanging(alice, fid);
        assert!(w.hanging.is_empty());
    }

    #[test]
    fn paintings_fill_the_wall_and_fall_without_it() {
        let (mut w, alice) = setup();
        let slot = hold(&mut w, alice, "painting");
        assert!(w.place_hanging(alice, slot, BlockPos::new(4, -59, 2), Dir::West));
        let (&pid, painting) = w.hanging.iter().next().unwrap();
        let cells = painting.cells();
        assert!(cells.len() >= 4, "the biggest that fit: {cells:?}");
        assert!(cells.iter().all(|c| c.x == 3 && (0..5).contains(&c.z)));
        let back = Hanging::from_nbt(&painting.to_nbt(), &mut w.rng).unwrap();
        assert_eq!(back.what, painting.what);
        // Take the wall away: it drops.
        for z in 0..5 {
            for y in -60..=-57 {
                w.set_block(BlockPos::new(4, y, z), levitas_registry::block::AIR);
            }
        }
        while !w.tick.is_multiple_of(SUPPORT_CHECK_TICKS) {
            w.tick += 1;
        }
        w.tick_hanging();
        assert!(!w.hanging.contains_key(&pid));
    }
}
