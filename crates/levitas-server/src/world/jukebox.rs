//! Jukeboxes and flower pots, as the Minecraft Wiki describes them.
//!
//! A music disc put into a jukebox plays its song (level event 1010) for the song's
//! length; using the jukebox again ejects the disc and stops it (1011). While it plays,
//! a comparator reads the song's own value; hoppers put discs into an empty jukebox and
//! take one out once its song is over. A flower pot takes a flower, sapling, mushroom,
//! fern, bamboo or cactus, which comes back out when the pot is used again.

use std::collections::HashMap;
use std::sync::OnceLock;

use levitas_nbt::Compound;
use levitas_registry::SyncedRegistry;
use serde_json::Value;

use super::*;
use crate::blocks;

const LEVEL_EVENT_PLAY_SONG: i32 = 1010;
const LEVEL_EVENT_STOP_SONG: i32 = 1011;

/// A jukebox's block entity.
#[derive(Default)]
pub struct Jukebox {
    pub items: [Stack; 1],
    /// Whether its song is playing, and for how many ticks it has.
    playing: bool,
    played: u64,
    /// The song played to its end; the disc waits to be taken.
    finished: bool,
    pub viewers: HashSet<i32>,
}

/// A song: its registry ID, length in ticks and comparator value.
#[derive(Clone, Copy)]
struct Song {
    id: i32,
    ticks: u64,
    comparator: u8,
}

fn songs() -> &'static HashMap<String, (u64, u8)> {
    static SONGS: OnceLock<HashMap<String, (u64, u8)>> = OnceLock::new();
    SONGS.get_or_init(|| {
        let all: serde_json::Map<String, Value> =
            serde_json::from_str(levitas_registry::JUKEBOX_SONGS_JSON).unwrap_or_default();
        all.into_iter()
            .map(|(name, s)| {
                let seconds = s["length_in_seconds"].as_f64().unwrap_or(0.0);
                let comparator = s["comparator_output"].as_u64().unwrap_or(0).min(15) as u8;
                (name, ((seconds * 20.0).ceil() as u64, comparator))
            })
            .collect()
    })
}

/// The song on a music disc: `minecraft:music_disc_cat` plays `minecraft:cat`.
fn song_of(disc: Stack) -> Option<Song> {
    let name = ItemInfo::get(disc.item)?
        .name
        .strip_prefix("minecraft:music_disc_")?;
    let key = format!("minecraft:{name}");
    let &(ticks, comparator) = songs().get(&key)?;
    let id = SyncedRegistry::get("minecraft:jukebox_song")?.id_of(&key)? as i32;
    Some(Song {
        id,
        ticks,
        comparator,
    })
}

/// The pot a plant goes into: `minecraft:poppy` into `minecraft:potted_poppy`.
fn potted(plant: &str) -> Option<u16> {
    let name = plant.strip_prefix("minecraft:")?;
    BlockInfo::by_name(&format!("minecraft:potted_{name}"))
        .or_else(|| BlockInfo::by_name(&format!("minecraft:potted_{name}_bush")))
        .map(|b| b.default_state)
}

/// The plant in a potted block.
fn plant_in(pot: u16) -> Option<&'static str> {
    let name = blocks::name(pot).strip_prefix("minecraft:potted_")?;
    let plant = name
        .strip_suffix("_bush")
        .filter(|n| n.ends_with("azalea"))
        .unwrap_or(name);
    ItemInfo::id_of(&format!("minecraft:{plant}")).and_then(|id| ItemInfo::get(id).map(|i| i.name))
}

impl Jukebox {
    pub fn read_nbt(nbt: &Compound) -> Self {
        let mut j = Jukebox {
            items: [nbt
                .compound("RecordItem")
                .map_or(Stack::EMPTY, Stack::from_nbt)],
            ..Default::default()
        };
        j.played = nbt.int("ticks_since_song_started").unwrap_or(0).max(0) as u64;
        j
    }

    pub fn write_nbt(&self, nbt: &mut Compound) {
        if !self.items[0].is_empty() {
            nbt.insert("RecordItem", self.items[0].to_nbt());
            nbt.insert("ticks_since_song_started", self.played as i64);
        }
    }

    /// What a comparator reads: the song's value while it plays.
    pub fn signal(&self) -> u8 {
        match song_of(self.items[0]) {
            Some(song) if self.playing && !self.finished => song.comparator,
            _ => 0,
        }
    }

    /// Whether a hopper may take the disc: only once its song is over.
    pub fn done(&self) -> bool {
        self.finished || self.items[0].is_empty()
    }
}

impl World {
    /// Using a jukebox: a disc goes in and plays, or the one inside comes out. Returns
    /// whether the block was a jukebox.
    pub(super) fn use_jukebox(&mut self, id: i32, slot: usize, pos: BlockPos, state: u16) -> bool {
        if blocks::name(state) != "minecraft:jukebox" {
            return false;
        }
        self.block_entities
            .entry(pos)
            .or_insert_with(|| BlockEntity::Jukebox(Box::default()));
        let Some(BlockEntity::Jukebox(j)) = self.block_entities.get_mut(&pos) else {
            return false;
        };
        let inside = j.items[0];
        if !inside.is_empty() {
            // Out it comes, and the music stops.
            **j = Jukebox {
                viewers: std::mem::take(&mut j.viewers),
                ..Default::default()
            };
            self.level_event(LEVEL_EVENT_STOP_SONG, pos, 0);
            let at = [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 1.01,
                f64::from(pos.z) + 0.5,
            ];
            self.spawn_item_moving(inside, at, [0.0, 0.1, 0.0]);
            self.set_block(pos, blocks::with_bool(state, "has_record", false));
            self.jukebox_changed(pos);
            return true;
        }
        let Some(p) = self.players.get(&id) else {
            return true;
        };
        let held = p.inventory.slots[slot];
        if song_of(held).is_none() {
            return false;
        }
        if p.survival() {
            let p = self.players.get_mut(&id).expect("checked");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        self.stat(
            id,
            levitas_registry::stat_type::CUSTOM,
            levitas_registry::custom_stat::PLAY_RECORD,
            1,
        );
        self.insert_disc(pos, held.with_count(1));
        true
    }

    /// Puts a disc into the jukebox at `pos` and starts its song.
    fn insert_disc(&mut self, pos: BlockPos, disc: Stack) {
        let Some(BlockEntity::Jukebox(j)) = self.block_entities.get_mut(&pos) else {
            return;
        };
        j.items[0] = disc;
        j.playing = true;
        j.played = 0;
        j.finished = false;
        if let Some(song) = song_of(disc) {
            self.level_event(LEVEL_EVENT_PLAY_SONG, pos, song.id);
        }
        let state = self.block(pos);
        self.set_block(pos, blocks::with_bool(state, "has_record", true));
        self.jukebox_changed(pos);
    }

    fn jukebox_changed(&mut self, pos: BlockPos) {
        self.dirty.insert(pos.chunk());
        self.redstone_dirty.push(pos);
    }

    /// Songs end after their length; a disc a hopper put in (or one loaded with the
    /// chunk) starts playing.
    pub(super) fn tick_jukeboxes(&mut self) {
        const STEP: u64 = 10;
        if !self.tick.is_multiple_of(STEP) {
            return;
        }
        let mut start = Vec::new();
        let mut ended = Vec::new();
        for (&pos, e) in &mut self.block_entities {
            let BlockEntity::Jukebox(j) = e else {
                continue;
            };
            let Some(song) = song_of(j.items[0]) else {
                continue;
            };
            if j.finished {
                continue;
            }
            if !j.playing {
                j.playing = true;
                start.push((pos, song.id));
                continue;
            }
            j.played += STEP;
            if j.played >= song.ticks {
                j.finished = true;
                ended.push(pos);
            }
        }
        for (pos, song) in start {
            self.level_event(LEVEL_EVENT_PLAY_SONG, pos, song);
            let state = self.block(pos);
            self.set_block(pos, blocks::with_bool(state, "has_record", true));
            self.jukebox_changed(pos);
        }
        for pos in ended {
            self.jukebox_changed(pos);
        }
    }

    /// Using a flower pot: a plant goes in, or the one inside comes out. Returns
    /// whether the block was a pot.
    pub(super) fn use_flower_pot(
        &mut self,
        id: i32,
        slot: usize,
        pos: BlockPos,
        state: u16,
    ) -> bool {
        let name = blocks::name(state);
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let held = p.inventory.slots[slot];
        let survival = p.survival();
        if name == "minecraft:flower_pot" {
            let Some(pot) = ItemInfo::get(held.item).and_then(|i| potted(i.name)) else {
                return false;
            };
            self.set_block_updating(pos, pot);
            if survival {
                let p = self.players.get_mut(&id).expect("checked");
                p.inventory.take(slot, 1);
                let update = p.inventory.set_slot_packet(slot);
                self.send(id, update);
            }
            self.stat(
                id,
                levitas_registry::stat_type::CUSTOM,
                levitas_registry::custom_stat::POT_FLOWER,
                1,
            );
            return true;
        }
        let Some(plant) = plant_in(state).and_then(ItemInfo::id_of) else {
            return false;
        };
        if let Some(empty) = BlockInfo::by_name("minecraft:flower_pot") {
            self.set_block_updating(pos, empty.default_state);
        }
        let p = self.players.get_mut(&id).expect("checked");
        let stack = Stack::new(plant, 1);
        if p.inventory.slots[slot].is_empty() {
            p.inventory.slots[slot] = stack;
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        } else {
            let (changed, left) = p.inventory.add(stack);
            let updates: Vec<Writer> = changed
                .into_iter()
                .map(|s| p.inventory.set_slot_packet(s))
                .collect();
            let at = p.pos;
            for u in updates {
                self.send(id, u);
            }
            if left > 0 {
                self.scatter_item(at, stack);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-jukebox-{}-{}",
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
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let id = next_entity_id();
        let profile = Profile {
            name: "DJ".to_owned(),
            uuid: 4,
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, out, [2.5, -60.0, 2.5]));
        (w, id)
    }

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    #[test]
    fn jukeboxes_play_discs_to_the_end_and_give_them_back() {
        let (mut w, id) = world();
        let pos = BlockPos::new(3, -60, 3);
        w.set_block(pos, state("minecraft:jukebox"));
        let slot = w.players[&id].hand_slot(0);
        let disc = ItemInfo::id_of("minecraft:music_disc_cat").unwrap();
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = Stack::new(disc, 1);
        assert!(w.use_jukebox(id, slot, pos, state("minecraft:jukebox")));
        assert!(w.players[&id].inventory.slots[slot].is_empty());
        assert_eq!(blocks::prop(w.block(pos), "has_record"), Some("true"));
        // While cat plays a comparator reads 2.
        assert_eq!(w.container_signal(pos), Some(2));
        // After its 185 seconds it is over, and a hopper may take the disc.
        for _ in 0..(185 * 20 / 10 + 2) {
            w.tick += 10;
            w.tick_jukeboxes();
        }
        assert_eq!(w.container_signal(pos), Some(0));
        let Some(BlockEntity::Jukebox(j)) = w.block_entities.get(&pos) else {
            panic!("jukebox");
        };
        assert!(j.done());
        // Saved with its disc.
        let saved = w.block_entities[&pos].to_nbt(pos);
        assert!(saved.compound("RecordItem").is_some());
        // Using it again ejects the disc.
        let now = w.block(pos);
        assert!(w.use_jukebox(id, slot, pos, now));
        assert_eq!(blocks::prop(w.block(pos), "has_record"), Some("false"));
        assert!(!w.items.is_empty());
    }

    #[test]
    fn flower_pots_take_and_give_back_plants() {
        let (mut w, id) = world();
        let pos = BlockPos::new(4, -60, 4);
        w.set_block(pos, state("minecraft:flower_pot"));
        let slot = w.players[&id].hand_slot(0);
        let poppy = ItemInfo::id_of("minecraft:poppy").unwrap();
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = Stack::new(poppy, 2);
        let empty_pot = w.block(pos);
        assert!(w.use_flower_pot(id, slot, pos, empty_pot));
        assert_eq!(blocks::name(w.block(pos)), "minecraft:potted_poppy");
        assert_eq!(w.players[&id].inventory.slots[slot].count, 1);
        let potted = w.block(pos);
        assert!(w.use_flower_pot(id, slot, pos, potted));
        assert_eq!(blocks::name(w.block(pos)), "minecraft:flower_pot");
        assert_eq!(w.players[&id].inventory.slots[slot].count, 2);
        assert_eq!(
            plant_in(state("minecraft:potted_azalea_bush")),
            Some("minecraft:azalea")
        );
        // Dirt does not go in a pot.
        let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = Stack::new(dirt, 1);
        let empty_pot = w.block(pos);
        assert!(!w.use_flower_pot(id, slot, pos, empty_pot));
    }

    #[test]
    fn placed_blocks_sound_like_what_they_are() {
        let sound = |n: &str| blocks::place_sound(state(n));
        assert_eq!(sound("minecraft:oak_planks"), "minecraft:block.wood.place");
        assert_eq!(
            sound("minecraft:cherry_planks"),
            "minecraft:block.cherry_wood.place"
        );
        assert_eq!(sound("minecraft:white_wool"), "minecraft:block.wool.place");
        assert_eq!(sound("minecraft:glass"), "minecraft:block.glass.place");
        assert_eq!(sound("minecraft:anvil"), "minecraft:block.anvil.place");
        assert_eq!(sound("minecraft:sand"), "minecraft:block.sand.place");
        assert_eq!(
            sound("minecraft:cobblestone"),
            "minecraft:block.stone.place"
        );
        assert_eq!(
            sound("minecraft:exposed_copper"),
            "minecraft:block.copper.place"
        );
        assert_eq!(
            sound("minecraft:deepslate_bricks"),
            "minecraft:block.deepslate_bricks.place"
        );
    }
}
