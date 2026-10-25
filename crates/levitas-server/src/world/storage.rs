//! Saving and loading the world in the vanilla 1.21.8 formats: chunks in region files,
//! `level.dat`, and one `playerdata/<uuid>.dat` per player.
//!
//! Only chunks that changed are written. Dropped items are not saved yet (they come with
//! the entity framework).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use levitas_nbt::{Compound, Tag};
use levitas_registry::DATA_VERSION;
use levitas_world::anvil;
use tracing::{error, info, warn};

use super::*;
use crate::inventory::{items_from_nbt, items_to_nbt};

/// Autosave every 5 minutes, like vanilla.
pub const AUTOSAVE_TICKS: u64 = 6000;
/// How often chunks no player can see are saved and dropped from memory.
pub const UNLOAD_TICKS: u64 = 100;

/// What `level.dat` holds that Levitas uses.
pub struct LevelData {
    pub spawn: [i32; 3],
    pub time: i64,
}

impl LevelData {
    /// Reads `level.dat`, falling back to `level.dat_old`, then to a new world.
    pub fn load(dir: &Path) -> Self {
        for name in ["level.dat", "level.dat_old"] {
            let path = dir.join(name);
            let Ok(bytes) = fs::read(&path) else { continue };
            match levitas_nbt::from_gzip(&bytes) {
                Ok((_, root)) => {
                    let data = root.compound("Data").cloned().unwrap_or_default();
                    let int = |key: &str, default: i64| data.int(key).unwrap_or(default) as i32;
                    return LevelData {
                        spawn: [
                            int("SpawnX", 0),
                            int("SpawnY", SURFACE_Y as i64),
                            int("SpawnZ", 0),
                        ],
                        time: data.int("Time").unwrap_or(0),
                    };
                }
                Err(e) => error!("{} is unreadable: {e}", path.display()),
            }
        }
        LevelData {
            spawn: [0, SURFACE_Y as i32, 0],
            time: 0,
        }
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let layer = |block: &str, height: i32| {
            Tag::Compound(Compound::new().with("block", block).with("height", height))
        };
        let flat = Compound::new().with("type", "minecraft:flat").with(
            "settings",
            Compound::new()
                .with("biome", "minecraft:plains")
                .with("features", false)
                .with("lakes", false)
                .with("structure_overrides", Vec::<Tag>::new())
                .with(
                    "layers",
                    vec![
                        layer("minecraft:bedrock", 1),
                        layer("minecraft:dirt", 2),
                        layer("minecraft:grass_block", 1),
                    ],
                ),
        );
        let noise = |settings: &str, biomes: Compound| {
            Compound::new()
                .with("type", "minecraft:noise")
                .with("settings", settings)
                .with("biome_source", biomes)
        };
        let dimension = |kind: &str, generator: Compound| {
            Tag::Compound(
                Compound::new()
                    .with("type", kind)
                    .with("generator", generator),
            )
        };
        let dimensions = Compound::new()
            .with(
                "minecraft:overworld",
                dimension("minecraft:overworld", flat),
            )
            .with(
                "minecraft:the_nether",
                dimension(
                    "minecraft:the_nether",
                    noise(
                        "minecraft:nether",
                        Compound::new()
                            .with("type", "minecraft:multi_noise")
                            .with("preset", "minecraft:nether"),
                    ),
                ),
            )
            .with(
                "minecraft:the_end",
                dimension(
                    "minecraft:the_end",
                    noise(
                        "minecraft:end",
                        Compound::new().with("type", "minecraft:the_end"),
                    ),
                ),
            );
        let data = Compound::new()
            .with("DataVersion", DATA_VERSION)
            .with("version", 19133) // the Anvil format
            .with(
                "Version",
                Compound::new()
                    .with("Id", DATA_VERSION)
                    .with("Name", levitas_protocol::GAME_VERSION)
                    .with("Series", "main")
                    .with("Snapshot", false),
            )
            .with("LevelName", "world")
            .with("GameType", 0)
            .with("Difficulty", 2i8)
            .with("hardcore", false)
            .with("allowCommands", true)
            .with("initialized", true)
            .with("SpawnX", self.spawn[0])
            .with("SpawnY", self.spawn[1])
            .with("SpawnZ", self.spawn[2])
            .with("SpawnAngle", 0.0f32)
            .with("Time", self.time)
            .with("DayTime", self.time)
            .with(
                "LastPlayed",
                std::time::UNIX_EPOCH
                    .elapsed()
                    .unwrap_or_default()
                    .as_millis() as i64,
            )
            .with(
                "WorldGenSettings",
                Compound::new()
                    .with("seed", 0i64)
                    .with("generate_features", false)
                    .with("bonus_chest", false)
                    .with("dimensions", dimensions),
            );
        let root = Compound::new().with("Data", data);
        // A world where nothing has changed yet has no folder so far.
        fs::create_dir_all(dir)?;
        let path = dir.join("level.dat");
        if path.exists() {
            // Keep the previous copy as a backup, as vanilla does.
            fs::copy(&path, dir.join("level.dat_old"))?;
        }
        write_atomic(&path, &levitas_nbt::to_gzip("", &root))
    }
}

/// Writes through a temporary file so a crash never leaves a half-written file.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// A UUID in the usual hyphenated form, as player data files are named.
pub fn uuid_string(uuid: u128) -> String {
    let h = format!("{uuid:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Equipment slots in saved player data, with their inventory menu slots.
const EQUIPMENT: [(&str, usize); 5] = [
    ("head", 5),
    ("chest", 6),
    ("legs", 7),
    ("feet", 8),
    ("offhand", OFFHAND),
];

impl Player {
    /// The vanilla `playerdata/<uuid>.dat` contents.
    pub(super) fn to_nbt(&self) -> Compound {
        // Saved slots: 0-8 hotbar, 9-35 main inventory.
        let saved_slots = (0..9)
            .map(|i| (i, HOTBAR + i))
            .chain((9..36).map(|i| (i, i)));
        let inventory =
            items_to_nbt(saved_slots.map(|(saved, menu)| (saved, self.inventory.slots[menu])));
        let mut equipment = Compound::new();
        for (name, slot) in EQUIPMENT {
            let stack = self.inventory.slots[slot];
            if !stack.is_empty() {
                equipment.insert(name, stack.to_nbt());
            }
        }
        Compound::new()
            .with("DataVersion", DATA_VERSION)
            .with(
                "Pos",
                self.pos.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with(
                "Rotation",
                vec![Tag::Float(self.yaw), Tag::Float(self.pitch)],
            )
            .with("OnGround", self.on_ground)
            .with("Dimension", "minecraft:overworld")
            .with("Health", self.health)
            .with("foodLevel", self.food)
            .with("foodSaturationLevel", self.saturation)
            .with("foodExhaustionLevel", self.exhaustion)
            .with("foodTickTimer", self.food_timer as i32)
            .with("playerGameType", i32::from(self.mode))
            .with("SelectedItemSlot", self.held as i32)
            .with("Inventory", inventory)
            .with("equipment", equipment)
    }

    /// Applies saved data. A player saved while dead comes back alive at spawn.
    pub(super) fn load_nbt(&mut self, nbt: &Compound) {
        if let Some(mode) = nbt.int("playerGameType").filter(|m| (0..=1).contains(m)) {
            self.mode = mode as u8;
        }
        for (saved, stack) in items_from_nbt(nbt.list("Inventory").unwrap_or_default()) {
            let menu = match saved {
                0..=8 => HOTBAR + saved,
                9..=35 => saved,
                _ => continue,
            };
            self.inventory.slots[menu] = stack;
        }
        if let Some(equipment) = nbt.compound("equipment") {
            for (name, slot) in EQUIPMENT {
                if let Some(stack) = equipment.compound(name) {
                    self.inventory.slots[slot] = Stack::from_nbt(stack);
                }
            }
        }
        self.held = nbt.int("SelectedItemSlot").unwrap_or(0).clamp(0, 8) as usize;

        let health = nbt.float("Health").unwrap_or(f64::from(MAX_HEALTH)) as f32;
        if health <= 0.0 {
            return;
        }
        self.health = health.min(MAX_HEALTH);
        self.food = nbt.int("foodLevel").unwrap_or(20).clamp(0, 20) as i32;
        self.saturation = nbt.float("foodSaturationLevel").unwrap_or(5.0) as f32;
        self.exhaustion = nbt.float("foodExhaustionLevel").unwrap_or(0.0) as f32;
        self.food_timer = nbt.int("foodTickTimer").unwrap_or(0).max(0) as u32;
        let overworld = nbt
            .str("Dimension")
            .map_or(true, |d| d == "minecraft:overworld");
        let pos: Vec<f64> = nbt
            .list("Pos")
            .unwrap_or_default()
            .iter()
            .filter_map(Tag::as_f64)
            .collect();
        if let (&[x, y, z], true) = (&pos[..], overworld) {
            if [x, y, z].iter().all(|v| v.is_finite()) {
                self.pos = [x, y, z];
                self.sent_pos = self.pos;
                self.center = ((x / 16.0).floor() as i32, (z / 16.0).floor() as i32);
            }
        }
        if let [yaw, pitch] = nbt.list("Rotation").unwrap_or_default() {
            self.yaw = yaw.as_f64().unwrap_or(0.0) as f32;
            self.pitch = pitch.as_f64().unwrap_or(0.0) as f32;
        }
        self.on_ground = nbt.int("OnGround").unwrap_or(1) != 0;
    }
}

impl World {
    fn player_path(&self, uuid: u128) -> PathBuf {
        self.dir
            .join("playerdata")
            .join(format!("{}.dat", uuid_string(uuid)))
    }

    /// Applies a joining player's saved data, if there is any.
    pub(super) fn load_player(&mut self, id: i32) {
        let uuid = self.players[&id].profile.uuid;
        let path = self.player_path(uuid);
        let Ok(bytes) = fs::read(&path) else {
            return;
        };
        match levitas_nbt::from_gzip(&bytes) {
            Ok((_, nbt)) => self
                .players
                .get_mut(&id)
                .expect("player exists")
                .load_nbt(&nbt),
            Err(e) => error!("{} is unreadable, starting fresh: {e}", path.display()),
        }
    }

    pub(super) fn save_player(&self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let path = self.player_path(p.profile.uuid);
        let result = fs::create_dir_all(path.parent().expect("has a parent"))
            .and_then(|()| write_atomic(&path, &levitas_nbt::to_gzip("", &p.to_nbt())));
        if let Err(e) = result {
            error!("could not save {}: {e}", path.display());
        }
    }

    /// Reads a chunk from disk and registers its block entities.
    pub(super) fn load_chunk(&mut self, x: i32, z: i32) -> Option<Chunk> {
        let nbt = match self.storage.read(x, z) {
            Ok(nbt) => nbt?,
            Err(e) => {
                error!("chunk {x},{z} is unreadable and will be regenerated: {e}");
                return None;
            }
        };
        let loaded = match anvil::from_nbt(&nbt) {
            Ok(loaded) => loaded?,
            Err(e) => {
                error!("chunk {x},{z} cannot be loaded and will be regenerated: {e:?}");
                return None;
            }
        };
        for nbt in &loaded.block_entities {
            match BlockEntity::from_nbt(nbt) {
                Some((pos, entity)) if pos.chunk() == (x, z) => {
                    self.block_entities.insert(pos, entity);
                }
                Some(_) => warn!("chunk {x},{z} lists a block entity outside itself"),
                None => {} // a kind of block entity Levitas does not handle yet
            }
        }
        Some(loaded.chunk)
    }

    fn save_chunk(&mut self, x: i32, z: i32) {
        self.dirty.remove(&(x, z));
        let Some(chunk) = self.chunks.get(&(x, z)) else {
            return;
        };
        let entities = self
            .block_entities
            .iter()
            .filter(|(pos, _)| pos.chunk() == (x, z))
            .map(|(&pos, e)| e.to_nbt(pos))
            .collect();
        let nbt = anvil::to_nbt(chunk, x, z, entities, self.tick as i64);
        if let Err(e) = self.storage.write(x, z, &nbt) {
            error!("could not save chunk {x},{z}: {e}");
        }
    }

    /// Saves every changed chunk, every player and level.dat.
    pub(super) fn save_all(&mut self) {
        let dirty: Vec<(i32, i32)> = self.dirty.iter().copied().collect();
        for (x, z) in &dirty {
            self.save_chunk(*x, *z);
        }
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for id in ids {
            self.save_player(id);
        }
        let level = LevelData {
            spawn: [
                self.spawn[0].floor() as i32,
                self.spawn[1] as i32,
                self.spawn[2].floor() as i32,
            ],
            time: self.tick as i64,
        };
        if let Err(e) = level.save(&self.dir) {
            error!("could not save level.dat: {e}");
        }
        info!("saved the world ({} chunks changed)", dirty.len());
    }

    /// Saves and forgets chunks that no player has loaded.
    pub(super) fn unload_unseen_chunks(&mut self) {
        let seen: HashSet<(i32, i32)> = self
            .players
            .values()
            .flat_map(|p| p.loaded.iter().copied())
            .collect();
        let unseen: Vec<(i32, i32)> = self
            .chunks
            .keys()
            .copied()
            .filter(|c| !seen.contains(c))
            .collect();
        for (x, z) in unseen {
            if self.dirty.contains(&(x, z)) {
                self.save_chunk(x, z);
            }
            self.chunks.remove(&(x, z));
            self.block_entities.retain(|pos, _| pos.chunk() != (x, z));
        }
    }

    /// Disconnects everyone and saves everything, before the server exits.
    pub(super) fn shutdown(&mut self) {
        let message = Writer::packet(clientbound::DISCONNECT).text("Server closed");
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for &id in &ids {
            self.close_container(id);
        }
        self.broadcast(message, None);
        self.save_all();
        // Dropping the players closes their connections once the message is sent.
        self.players.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_data_in_a_new_folder() {
        let dir = std::env::temp_dir().join(format!("levitas-level-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let level = LevelData {
            spawn: [3, -60, -7],
            time: 12_345,
        };
        level.save(&dir).unwrap();
        // Saving twice keeps the previous copy as level.dat_old.
        level.save(&dir).unwrap();
        assert!(dir.join("level.dat_old").is_file());
        let loaded = LevelData::load(&dir);
        assert_eq!((loaded.spawn, loaded.time), ([3, -60, -7], 12_345));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn uuid_format() {
        assert_eq!(
            uuid_string(0x069a79f444e94726a5befca90e38aaf5),
            "069a79f4-44e9-4726-a5be-fca90e38aaf5"
        );
    }

    #[test]
    fn player_data_round_trip() {
        let (out, _rx) = mpsc::channel(1);
        let profile = Profile {
            name: "Alice".into(),
            uuid: 1,
        };
        let mut p = Player::new(profile, out, [0.5, -60.0, 0.5]);
        p.pos = [10.5, -40.0, -3.25];
        p.yaw = 90.0;
        p.health = 7.5;
        p.food = 12;
        p.mode = GAME_MODE_CREATIVE;
        p.held = 4;
        let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
        p.inventory.slots[HOTBAR + 2] = Stack::new(dirt, 5);
        p.inventory.slots[20] = Stack::new(dirt, 64);
        p.inventory.slots[OFFHAND] = Stack::new(dirt, 1);
        let nbt = levitas_nbt::from_gzip(&levitas_nbt::to_gzip("", &p.to_nbt()))
            .unwrap()
            .1;

        let (out, _rx) = mpsc::channel(1);
        let profile = Profile {
            name: "Alice".into(),
            uuid: 1,
        };
        let mut q = Player::new(profile, out, [0.5, -60.0, 0.5]);
        q.load_nbt(&nbt);
        assert_eq!(q.pos, p.pos);
        assert_eq!(
            (q.yaw, q.health, q.food, q.mode, q.held),
            (90.0, 7.5, 12, GAME_MODE_CREATIVE, 4)
        );
        assert_eq!(q.inventory.slots, p.inventory.slots);
        assert_eq!(q.center, (0, -1));
    }
}
