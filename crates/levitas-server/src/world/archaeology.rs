//! Archaeology and decorated pots, as the Minecraft Wiki describes them. Suspicious
//! sand and gravel hold an item from an archaeology loot table: brushing one for 96
//! ticks (4.8 seconds) brings it out stage by stage (`dusted` 0 to 3), then it drops,
//! the block turns into sand or gravel and the brush wears by one. Stopping halfway
//! lets the block settle back a stage at a time. Decorated pots keep their four sides
//! (sherds) and hold one stack: items go in by using them on the pot, which wobbles,
//! and structure pots are filled from their loot table.

use levitas_nbt::{Compound, Tag};
use levitas_registry::block_entity_type;

use super::*;
use crate::blocks::{self, offset, prop, with, Dir};

/// Ticks between brush strokes, and strokes to bring the item out (96 ticks).
const STROKE_TICKS: u32 = 8;
const STROKES: u8 = 12;
/// After this many ticks without brushing, the block settles back a stage.
const SETTLE_TICKS: u64 = 40;
/// Block event that makes a decorated pot wobble (parameter 0 a gentle one, 1 a shake).
const POT_WOBBLE: u8 = 1;

/// Suspicious sand or gravel's block entity.
#[derive(Default)]
pub struct Brushable {
    pub loot_table: Option<String>,
    pub seed: i64,
    pub item: Stack,
    pub strokes: u8,
    pub last_brushed: u64,
    pub hit_face: Option<Dir>,
    pub viewers: HashSet<i32>,
}

/// A decorated pot's block entity.
pub struct Pot {
    /// The item on each side: back, left, right, front (bricks when plain).
    pub sherds: [i32; 4],
    pub items: [Stack; 1],
    pub viewers: HashSet<i32>,
}

impl Default for Pot {
    fn default() -> Self {
        let brick = ItemInfo::id_of("minecraft:brick").unwrap_or(0);
        Pot {
            sherds: [brick; 4],
            items: [Stack::EMPTY],
            viewers: HashSet::new(),
        }
    }
}

/// A player brushing a block: where, from which face, with which hand, how long.
#[derive(Clone, Copy, Debug)]
pub struct Brushing {
    pub pos: BlockPos,
    pub face: Dir,
    pub hand: i32,
    pub ticks: u32,
}

/// The direction's 3D data value (down 0, up 1, north 2, south 3, west 4, east 5).
fn face_index(d: Dir) -> i32 {
    match d {
        Dir::Down => 0,
        Dir::Up => 1,
        Dir::North => 2,
        Dir::South => 3,
        Dir::West => 4,
        Dir::East => 5,
    }
}

fn face_from_index(i: i64) -> Option<Dir> {
    Some(match i {
        0 => Dir::Down,
        1 => Dir::Up,
        2 => Dir::North,
        3 => Dir::South,
        4 => Dir::West,
        5 => Dir::East,
        _ => return None,
    })
}

fn is_suspicious(state: u16) -> bool {
    matches!(
        blocks::name(state),
        "minecraft:suspicious_sand" | "minecraft:suspicious_gravel"
    )
}

impl Brushable {
    pub fn read_nbt(nbt: &Compound) -> Self {
        Brushable {
            loot_table: nbt.str("LootTable").map(str::to_owned),
            seed: nbt.int("LootTableSeed").unwrap_or(0),
            item: nbt.compound("item").map_or(Stack::EMPTY, Stack::from_nbt),
            hit_face: nbt.int("hit_direction").and_then(face_from_index),
            ..Default::default()
        }
    }

    pub fn write_nbt(&self, nbt: &mut Compound) {
        if let Some(table) = &self.loot_table {
            nbt.insert("LootTable", table.as_str());
            if self.seed != 0 {
                nbt.insert("LootTableSeed", self.seed);
            }
        }
        if !self.item.is_empty() {
            nbt.insert("item", self.item.to_nbt());
        }
        if let Some(face) = self.hit_face {
            nbt.insert("hit_direction", face_index(face));
        }
    }

    /// What the client draws: the item coming out, and the face it comes out of.
    pub fn network_data(&self) -> Compound {
        let mut c = Compound::new();
        if !self.item.is_empty() {
            c.insert("item", self.item.to_nbt());
        }
        if let Some(face) = self.hit_face {
            c.insert("hit_direction", face_index(face));
        }
        c
    }
}

impl Pot {
    pub fn read_nbt(nbt: &Compound) -> Self {
        let mut pot = Pot::default();
        for (i, sherd) in nbt
            .list("sherds")
            .unwrap_or_default()
            .iter()
            .take(4)
            .enumerate()
        {
            if let Some(id) = sherd.as_str().and_then(ItemInfo::id_of) {
                pot.sherds[i] = id;
            }
        }
        pot.items[0] = nbt.compound("item").map_or(Stack::EMPTY, Stack::from_nbt);
        pot
    }

    fn sherd_names(&self) -> Vec<Tag> {
        self.sherds
            .iter()
            .map(|&s| {
                Tag::String(
                    ItemInfo::get(s)
                        .map_or("minecraft:brick", |i| i.name)
                        .into(),
                )
            })
            .collect()
    }

    pub fn write_nbt(&self, nbt: &mut Compound) {
        nbt.insert("sherds", self.sherd_names());
        if !self.items[0].is_empty() {
            nbt.insert("item", self.items[0].to_nbt());
        }
    }

    pub fn network_data(&self) -> Compound {
        let mut c = Compound::new().with("sherds", self.sherd_names());
        if !self.items[0].is_empty() {
            c.insert("item", self.items[0].to_nbt());
        }
        c
    }
}

impl World {
    /// Using a brush on suspicious sand or gravel starts brushing it. Returns whether
    /// it did.
    pub(super) fn start_brushing(
        &mut self,
        id: i32,
        hand: i32,
        pos: BlockPos,
        face: Dir,
        clicked: u16,
    ) -> bool {
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        let held = p.inventory.slots[p.hand_slot(hand)];
        if ItemInfo::get(held.item).is_none_or(|i| i.name != "minecraft:brush") {
            return false;
        }
        if is_suspicious(clicked) {
            p.brushing = Some(Brushing {
                pos,
                face,
                hand,
                ticks: 0,
            });
        }
        // Any other block just shows the brushing; the client does that by itself.
        true
    }

    /// Brushing goes on while the player holds the brush on the same block.
    pub(super) fn tick_brushing(&mut self) {
        let ids: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.brushing.is_some())
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let p = &self.players[&id];
            let Some(mut b) = p.brushing else {
                continue;
            };
            let held = p.inventory.slots[p.hand_slot(b.hand)];
            let (eye, look, reach) = p.eye_ray();
            let still_brush = ItemInfo::get(held.item).is_some_and(|i| i.name == "minecraft:brush");
            let looking = self.ray_cast(eye, look, reach, false).map(|(pos, _)| pos) == Some(b.pos);
            if !still_brush || !looking || !self.players[&id].alive {
                self.players.get_mut(&id).expect("listed").brushing = None;
                continue;
            }
            b.ticks += 1;
            self.players.get_mut(&id).expect("listed").brushing = Some(b);
            if b.ticks.is_multiple_of(STROKE_TICKS) && self.brush_stroke(id, b) {
                self.players.get_mut(&id).expect("listed").brushing = None;
            }
        }
    }

    /// One stroke of the brush. Returns whether the block is done.
    fn brush_stroke(&mut self, id: i32, b: Brushing) -> bool {
        let state = self.block(b.pos);
        if !is_suspicious(state) {
            return true;
        }
        let gravel = blocks::name(state) == "minecraft:suspicious_gravel";
        let sound = if gravel {
            "minecraft:item.brush.brushing.gravel"
        } else {
            "minecraft:item.brush.brushing.sand"
        };
        self.play_sound_at(sound, block_centre(b.pos), 1.0, 1.0, None);
        let tick = self.tick;
        let Some(BlockEntity::Brushable(e)) = self.block_entities.get_mut(&b.pos) else {
            return true;
        };
        // The first stroke rolls what is inside.
        if let Some(table) = e.loot_table.take() {
            let seed = e.seed;
            let mut rolled = [Stack::EMPTY; 1];
            self.loot.fill(&table, seed, &mut rolled);
            if let Some(BlockEntity::Brushable(e)) = self.block_entities.get_mut(&b.pos) {
                e.item = rolled[0];
            }
        }
        let Some(BlockEntity::Brushable(e)) = self.block_entities.get_mut(&b.pos) else {
            return true;
        };
        e.strokes += 1;
        e.last_brushed = tick;
        let first_face = e.hit_face.is_none();
        e.hit_face.get_or_insert(b.face);
        let strokes = e.strokes;
        let data = e.network_data();
        if first_face {
            self.send_block_entity_data(b.pos, block_entity_type::BRUSHABLE_BLOCK, data);
        }
        if strokes >= STROKES {
            self.finish_brushing(id, b, gravel);
            return true;
        }
        let dusted = (strokes / 3).min(3);
        if prop(state, "dusted") != Some(&*dusted.to_string()) {
            self.set_block(b.pos, with(state, "dusted", &dusted.to_string()));
        }
        false
    }

    /// The item comes out of the face brushed, the block becomes plain sand or gravel,
    /// and the brush wears.
    fn finish_brushing(&mut self, id: i32, b: Brushing, gravel: bool) {
        let item = match self.block_entities.remove(&b.pos) {
            Some(BlockEntity::Brushable(e)) => e.item,
            _ => Stack::EMPTY,
        };
        let plain = if gravel {
            "minecraft:gravel"
        } else {
            "minecraft:sand"
        };
        if let Some(info) = BlockInfo::by_name(plain) {
            self.set_block_updating(b.pos, info.default_state);
        }
        let sound = if gravel {
            "minecraft:item.brush.brushing.gravel.complete"
        } else {
            "minecraft:item.brush.brushing.sand.complete"
        };
        self.play_sound_at(sound, block_centre(b.pos), 1.0, 1.0, None);
        if !item.is_empty() {
            let out = offset(b.pos, b.face);
            let at = [
                f64::from(out.x) + 0.5,
                f64::from(out.y) + 0.2,
                f64::from(out.z) + 0.5,
            ];
            self.spawn_item_moving(item, at, [0.0, 0.0, 0.0]);
        }
        if self.players.get(&id).is_some_and(Player::survival) {
            let slot = self.players[&id].hand_slot(b.hand);
            let event = if b.hand == 0 { 47 } else { 48 };
            self.damage_item(id, slot, 1, event);
        }
    }

    /// Suspicious blocks left half brushed settle back a stage every two seconds.
    pub(super) fn tick_brushables(&mut self) {
        if !self.tick.is_multiple_of(SETTLE_TICKS) {
            return;
        }
        let now = self.tick;
        let settling: Vec<BlockPos> = self
            .block_entities
            .iter()
            .filter_map(|(&pos, e)| match e {
                BlockEntity::Brushable(b)
                    if b.strokes > 0 && now - b.last_brushed >= SETTLE_TICKS =>
                {
                    Some(pos)
                }
                _ => None,
            })
            .collect();
        for pos in settling {
            let Some(BlockEntity::Brushable(b)) = self.block_entities.get_mut(&pos) else {
                continue;
            };
            b.strokes = b.strokes.saturating_sub(3);
            let dusted = (b.strokes / 3).min(3);
            let state = self.block(pos);
            if is_suspicious(state) {
                self.set_block(pos, with(state, "dusted", &dusted.to_string()));
            }
        }
    }

    /// Using an item on a decorated pot puts one in, if it holds nothing or the same
    /// kind with room. Returns whether the pot was used.
    pub(super) fn use_decorated_pot(
        &mut self,
        id: i32,
        slot: usize,
        pos: BlockPos,
        state: u16,
    ) -> bool {
        if blocks::name(state) != "minecraft:decorated_pot" {
            return false;
        }
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let held = p.inventory.slots[slot];
        let survival = p.survival();
        let Some(BlockEntity::Pot(pot)) = self.block_entities.get_mut(&pos) else {
            return false;
        };
        let inside = pot.items[0];
        let fits = !held.is_empty()
            && (inside.is_empty()
                || (inside.same_kind(&held) && inside.count < inside.max_stack()));
        if !fits {
            self.block_event(pos, state, POT_WOBBLE, 1);
            self.play_sound_at(
                "minecraft:block.decorated_pot.insert_fail",
                block_centre(pos),
                1.0,
                1.0,
                None,
            );
            return true;
        }
        pot.items[0] = if inside.is_empty() {
            held.with_count(1)
        } else {
            inside.with_count(inside.count + 1)
        };
        let fullness = pot.items[0].count as f32 / pot.items[0].max_stack() as f32;
        if survival {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        self.dirty.insert(pos.chunk());
        self.redstone_dirty.push(pos);
        self.block_event(pos, state, POT_WOBBLE, 0);
        self.play_sound_at(
            "minecraft:block.decorated_pot.insert",
            block_centre(pos),
            1.0,
            0.7 + 0.5 * fullness,
            None,
        );
        true
    }

    /// The pot item a decorated pot drops when it is not shattered: with its sides.
    pub(super) fn pot_item(&self, pos: BlockPos) -> Option<Stack> {
        let Some(BlockEntity::Pot(pot)) = self.block_entities.get(&pos) else {
            return None;
        };
        let brick = ItemInfo::id_of("minecraft:brick").unwrap_or(0);
        let mut stack = Stack::new(ItemInfo::id_of("minecraft:decorated_pot")?, 1);
        stack.sherds = pot.sherds.iter().any(|&s| s != brick).then_some(pot.sherds);
        Some(stack)
    }

    /// A banner with layers drops keeping them.
    pub(super) fn banner_item(&self, pos: BlockPos, state: u16) -> Option<Stack> {
        let Some(BlockEntity::Banner(b)) = self.block_entities.get(&pos) else {
            return None;
        };
        b.patterns?;
        let name = blocks::name(state).replace("_wall_banner", "_banner");
        let mut stack = Stack::new(ItemInfo::id_of(&name)?, 1);
        stack.patterns = b.patterns;
        Some(stack)
    }

    /// Breaking a decorated pot with something that shatters it (a sword, axe, ...)
    /// drops its four sides instead of the pot. Returns the sherds to drop, if so.
    pub(super) fn pot_shards(&self, pos: BlockPos, tool: i32) -> Option<Vec<Stack>> {
        let Some(BlockEntity::Pot(pot)) = self.block_entities.get(&pos) else {
            return None;
        };
        let breaks = levitas_registry::TagRegistry::get("minecraft:item").is_some_and(|t| {
            t.members("minecraft:breaks_decorated_pots")
                .contains(&i64::from(tool))
        });
        breaks.then(|| pot.sherds.iter().map(|&s| Stack::new(s, 1)).collect())
    }

    fn send_block_entity_data(&mut self, pos: BlockPos, kind: i32, data: Compound) {
        let bytes = levitas_nbt::to_network(&Tag::Compound(data));
        self.broadcast(
            Writer::packet(clientbound::BLOCK_ENTITY_DATA)
                .position(pos)
                .varint(kind)
                .bytes(&bytes),
            None,
        );
    }
}

fn block_centre(pos: BlockPos) -> [f64; 3] {
    [pos.x, pos.y, pos.z].map(|v| f64::from(v) + 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-archaeology-{}-{}",
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

    fn player(w: &mut World, pos: [f64; 3]) -> (i32, tokio::sync::mpsc::Receiver<Frame>) {
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Indy".to_owned(),
            uuid: 7,
            properties: Vec::new(),
        };
        w.players.insert(id, Player::new(profile, out, pos));
        (id, frames)
    }

    #[test]
    fn brushing_suspicious_sand_brings_out_its_find() {
        let mut w = world();
        let pos = BlockPos::new(3, -61, 3);
        w.set_block(
            pos,
            BlockInfo::by_name("minecraft:suspicious_sand")
                .unwrap()
                .default_state,
        );
        let nbt = Compound::new()
            .with("id", "minecraft:brushable_block")
            .with("x", 3)
            .with("y", -61)
            .with("z", 3)
            .with("LootTable", "minecraft:archaeology/desert_pyramid")
            .with("LootTableSeed", 9i64);
        w.add_block_entity(&nbt, 0, 0);
        let (id, _frames) = player(&mut w, [3.5, -60.0, 3.5]);
        let brush = ItemInfo::id_of("minecraft:brush").unwrap();
        {
            let p = w.players.get_mut(&id).unwrap();
            p.pitch = 90.0; // looking straight down at the sand
            let slot = p.hand_slot(0);
            p.inventory.slots[slot] = Stack::new(brush, 1);
        }
        let state = w.block(pos);
        assert!(w.start_brushing(id, 0, pos, Dir::Up, state));
        for _ in 0..48 {
            w.tick += 1;
            w.tick_brushing();
        }
        // Halfway: dusted, and the find is decided.
        assert_eq!(prop(w.block(pos), "dusted"), Some("2"));
        for _ in 0..48 {
            w.tick += 1;
            w.tick_brushing();
        }
        assert_eq!(blocks::name(w.block(pos)), "minecraft:sand");
        assert!(!w.block_entities.contains_key(&pos));
        assert!(!w.items.is_empty(), "the find dropped");
        let p = &w.players[&id];
        assert_eq!(p.inventory.slots[p.hand_slot(0)].damage, 1);
    }

    #[test]
    fn decorated_pots_hold_a_stack_fill_from_loot_and_shatter() {
        let mut w = world();
        let pos = BlockPos::new(5, -60, 5);
        let pot_state = BlockInfo::by_name("minecraft:decorated_pot")
            .unwrap()
            .default_state;
        w.set_block(pos, pot_state);
        let nbt = Compound::new()
            .with("id", "minecraft:decorated_pot")
            .with("x", 5)
            .with("y", -60)
            .with("z", 5)
            .with("LootTable", "minecraft:pots/trial_chambers/corridor")
            .with("LootTableSeed", 3i64);
        assert!(w.add_block_entity(&nbt, 0, 0));
        let found = w.block_entities[&pos].stacks()[0];
        assert!(!found.is_empty(), "the pot was filled");

        // A plain pot takes items one at a time.
        let empty = BlockPos::new(6, -60, 5);
        w.set_block(empty, pot_state);
        w.block_entities
            .insert(empty, BlockEntity::Pot(Box::default()));
        let (id, _frames) = player(&mut w, [6.5, -60.0, 3.5]);
        let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
        let slot = w.players[&id].hand_slot(0);
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = Stack::new(dirt, 5);
        for _ in 0..3 {
            assert!(w.use_decorated_pot(id, slot, empty, pot_state));
        }
        assert_eq!(w.block_entities[&empty].stacks()[0].count, 3);
        assert_eq!(w.players[&id].inventory.slots[slot].count, 2);
        // Saved and read back.
        let saved = w.block_entities[&empty].to_nbt(empty);
        let (_, back) = BlockEntity::from_nbt(&saved).unwrap();
        assert_eq!(back.stacks()[0].count, 3);
        // A sword shatters it into its four sides.
        let sword = ItemInfo::id_of("minecraft:iron_sword").unwrap();
        assert_eq!(w.pot_shards(empty, sword).map(|s| s.len()), Some(4));
        assert!(w.pot_shards(empty, dirt).is_none());
    }
}
