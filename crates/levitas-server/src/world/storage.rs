//! Saving and loading the world in the vanilla 26.3 formats: chunks in region files
//! under `dimensions/`, `level.dat` with the world's state split into `data/minecraft/`,
//! and one `players/data/<uuid>.dat` per player. Worlds saved by 1.21.8 are moved into
//! that layout when they are opened.
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
use crate::flat::SURFACE_Y;
use crate::inventory::{items_from_nbt, items_to_nbt};

/// Autosave every 5 minutes, like vanilla.
pub const AUTOSAVE_TICKS: u64 = 6000;
/// How often chunks no player can see are saved and dropped from memory.
pub const UNLOAD_TICKS: u64 = 100;

/// How a world's new chunks are made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldKind {
    /// Superflat test terrain.
    Flat,
    /// Vanilla-style generated terrain.
    Noise,
}

/// What `level.dat` holds that Levitas uses.
pub struct LevelData {
    pub spawn: [i32; 3],
    pub time: i64,
    pub day_time: i64,
    pub weather: super::time::Weather,
    pub seed: i64,
    pub kind: WorldKind,
    pub next_map_id: i32,
}

impl LevelData {
    /// Reads `level.dat`, falling back to `level.dat_old`. `None` for a new world.
    pub fn load(dir: &Path) -> Option<Self> {
        for name in ["level.dat", "level.dat_old"] {
            let path = dir.join(name);
            let Ok(bytes) = fs::read(&path) else { continue };
            match levitas_nbt::from_gzip(&bytes) {
                Ok((_, root)) => {
                    let data = root.compound("Data").cloned().unwrap_or_default();
                    // 26.3 keeps these apart from level.dat; 1.21.8 kept them inside.
                    let rules = saved_data(dir, "game_rules")
                        .or_else(|| data.compound("GameRules").cloned());
                    if let Some(rules) = rules {
                        crate::rules::load_nbt(&rules);
                    }
                    let gen = saved_data(dir, "world_gen_settings")
                        .or_else(|| data.compound("WorldGenSettings").cloned())
                        .unwrap_or_default();
                    let weather = saved_data(dir, "weather");
                    let day_time = saved_data(dir, "world_clocks")
                        .and_then(|c| c.compound("minecraft:overworld")?.int("total_ticks"));
                    let int = |key: &str, default: i64| data.int(key).unwrap_or(default) as i32;
                    let spawn = match data.compound("spawn").and_then(|s| s.int_array("pos")) {
                        Some(&[x, y, z]) => [x, y, z],
                        _ => [
                            int("SpawnX", 0),
                            int("SpawnY", SURFACE_Y as i64),
                            int("SpawnZ", 0),
                        ],
                    };
                    let overworld_type = gen
                        .compound("dimensions")
                        .and_then(|d| d.compound("minecraft:overworld"))
                        .and_then(|o| o.compound("generator"))
                        .and_then(|g| g.str("type"))
                        .unwrap_or("minecraft:flat");
                    // `map` is the last ID given out (26.3's maps/last_id.dat, or the
                    // idcounts.dat of older worlds).
                    let next_map_id = saved_data(dir, "maps/last_id")
                        .or_else(|| saved_data(dir, "idcounts"))
                        .or_else(|| old_saved_data(dir, "idcounts"))
                        .and_then(|c| c.int("map"))
                        .map_or(0, |last| last as i32 + 1);
                    // Map IDs are shared by every dimension; none is given twice.
                    super::maps::reserve_ids(next_map_id);
                    return Some(LevelData {
                        spawn,
                        time: data.int("Time").unwrap_or(0),
                        day_time: day_time
                            .or_else(|| data.int("DayTime"))
                            .or_else(|| data.int("Time"))
                            .unwrap_or(0),
                        weather: {
                            // 26.3's names, else the ones 1.21.8 kept in level.dat.
                            let get = |new: &str, old: &str| match &weather {
                                Some(w) => w.int(new).unwrap_or(0),
                                None => data.int(old).unwrap_or(0),
                            };
                            let raining = get("raining", "raining") != 0;
                            let thundering = get("thundering", "thundering") != 0;
                            super::time::Weather {
                                raining,
                                thundering,
                                rain_time: get("rain_time", "rainTime") as i32,
                                thunder_time: get("thunder_time", "thunderTime") as i32,
                                clear_time: get("clear_weather_time", "clearWeatherTime") as i32,
                                rain_level: if raining { 1.0 } else { 0.0 },
                                thunder_level: if thundering { 1.0 } else { 0.0 },
                            }
                        },
                        seed: gen.int("seed").unwrap_or(0),
                        kind: if overworld_type == "minecraft:noise" {
                            WorldKind::Noise
                        } else {
                            WorldKind::Flat
                        },
                        next_map_id,
                    });
                }
                Err(e) => error!("{} is unreadable: {e}", path.display()),
            }
        }
        None
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let layer = |block: &str, height: i32| {
            Tag::Compound(Compound::new().with("block", block).with("height", height))
        };
        let overworld_noise = Compound::new()
            .with("type", "minecraft:noise")
            .with("settings", "minecraft:overworld")
            .with(
                "biome_source",
                Compound::new()
                    .with("type", "minecraft:multi_noise")
                    .with("preset", "minecraft:overworld"),
            );
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
                dimension(
                    "minecraft:overworld",
                    match self.kind {
                        WorldKind::Flat => flat,
                        WorldKind::Noise => overworld_noise,
                    },
                ),
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
        // Every data version that has saved this world, oldest first.
        let mut history: Vec<i32> = fs::read(dir.join("level.dat"))
            .ok()
            .and_then(|bytes| levitas_nbt::from_gzip(&bytes).ok())
            .and_then(|(_, root)| {
                let list = root.compound("Data")?.list("version_history")?;
                Some(
                    list.iter()
                        .filter_map(|t| Some(t.as_i64()? as i32))
                        .collect(),
                )
            })
            .unwrap_or_default();
        if history.last() != Some(&DATA_VERSION) {
            history.push(DATA_VERSION);
        }
        let data = Compound::new()
            .with("DataVersion", DATA_VERSION)
            .with("version", 19133) // the Anvil format
            .with(
                "version_history",
                history.into_iter().map(Tag::Int).collect::<Vec<Tag>>(),
            )
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
            .with(
                "difficulty_settings",
                Compound::new()
                    .with("difficulty", crate::rules::difficulty().name())
                    .with("hardcore", false)
                    .with("locked", false),
            )
            .with("allowCommands", true)
            .with("initialized", true)
            .with(
                "spawn",
                Compound::new()
                    .with("pos", self.spawn.to_vec())
                    .with("dimension", "minecraft:overworld")
                    .with("yaw", 0.0f32)
                    .with("pitch", 0.0f32),
            )
            .with("Time", self.time)
            .with("ServerBrands", vec![Tag::from("Levitas")])
            .with(
                "LastPlayed",
                std::time::UNIX_EPOCH
                    .elapsed()
                    .unwrap_or_default()
                    .as_millis() as i64,
            );
        let root = Compound::new().with("Data", data);
        // A world where nothing has changed yet has no folder so far.
        fs::create_dir_all(dir)?;
        save_data(dir, "game_rules", crate::rules::to_nbt())?;
        save_data(
            dir,
            "weather",
            Compound::new()
                .with("raining", self.weather.raining)
                .with("thundering", self.weather.thundering)
                .with("rain_time", self.weather.rain_time)
                .with("thunder_time", self.weather.thunder_time)
                .with("clear_weather_time", self.weather.clear_time),
        )?;
        // Both clocks run on the overworld's day.
        let clock = || Tag::Compound(Compound::new().with("total_ticks", self.day_time));
        save_data(
            dir,
            "world_clocks",
            Compound::new()
                .with("minecraft:overworld", clock())
                .with("minecraft:the_end", clock()),
        )?;
        if self.next_map_id > 0 {
            save_data(
                dir,
                "maps/last_id",
                Compound::new().with("map", self.next_map_id - 1),
            )?;
        }
        save_data(
            dir,
            "world_gen_settings",
            Compound::new()
                .with("seed", self.seed)
                .with("generate_structures", false)
                .with("bonus_chest", false)
                .with("dimensions", dimensions),
        )?;
        let path = dir.join("level.dat");
        if path.exists() {
            // Keep the previous copy as a backup, as vanilla does.
            fs::copy(&path, dir.join("level.dat_old"))?;
        }
        write_atomic(&path, &levitas_nbt::to_gzip("", &root))
    }
}

/// The `data` compound of `data/minecraft/<name>.dat`, if the world has that file.
fn saved_data(dir: &Path, name: &str) -> Option<Compound> {
    let path = dir
        .join("data")
        .join("minecraft")
        .join(format!("{name}.dat"));
    let bytes = fs::read(&path).ok()?;
    match levitas_nbt::from_gzip(&bytes) {
        Ok((_, root)) => root.compound("data").cloned(),
        Err(e) => {
            error!("{} is unreadable: {e}", path.display());
            None
        }
    }
}

/// Writes `data/minecraft/<name>.dat` as 26.3 does: the data, and its data version.
fn save_data(dir: &Path, name: &str, data: Compound) -> io::Result<()> {
    let path = dir
        .join("data")
        .join("minecraft")
        .join(format!("{name}.dat"));
    fs::create_dir_all(path.parent().expect("has a parent"))?;
    let root = Compound::new()
        .with("data", data)
        .with("DataVersion", DATA_VERSION);
    write_atomic(&path, &levitas_nbt::to_gzip("", &root))
}

/// Reads `data/<name>.dat`, where 1.21.8 and Leaf kept saved data.
fn old_saved_data(dir: &Path, name: &str) -> Option<Compound> {
    let bytes = fs::read(dir.join("data").join(format!("{name}.dat"))).ok()?;
    let (_, root) = levitas_nbt::from_gzip(&bytes).ok()?;
    root.compound("data").cloned()
}

/// Moves a world saved by 1.21.8 into 26.3's layout: each dimension's regions, entities
/// and Levitas files under `dimensions/minecraft/`, and player data under
/// `players/data/`. Returns what could not be moved: an old folder whose new place
/// already exists (both would hold part of the world), or a move that failed. The
/// server must not start then, or the old data would be hidden for good.
pub fn upgrade_layout(dir: &Path) -> Vec<String> {
    use super::dimension::Dimension;
    let players = dir.join("players");
    let mut moves = vec![
        (dir.join("playerdata"), players.join("data")),
        (dir.join("stats"), players.join("stats")),
        (dir.join("advancements"), players.join("advancements")),
    ];
    // Paper and Leaf keep the Nether and the End in folders of their own next to the
    // world: `world_nether/DIM-1` and `world_the_end/DIM1`.
    if let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) {
        let name = name.to_string_lossy();
        for (suffix, inner, dimension) in [
            ("_nether", "DIM-1", Dimension::Nether),
            ("_the_end", "DIM1", Dimension::End),
        ] {
            let old = parent.join(format!("{name}{suffix}")).join(inner);
            for part in ["region", "entities", "poi"] {
                moves.push((old.join(part), dimension.folder(dir).join(part)));
            }
        }
    }
    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let (old, new) = (dimension.old_folder(dir), dimension.folder(dir));
        for name in [
            "region",
            "entities",
            "poi",
            "levitas_portals.dat",
            "levitas_dragon.dat",
        ] {
            moves.push((old.join(name), new.join(name)));
        }
    }
    let mut problems = Vec::new();
    for (from, to) in moves {
        if !from.exists() {
            continue;
        }
        if to.exists() {
            // An empty old folder holds nothing to lose.
            if from.is_dir() && fs::read_dir(&from).is_ok_and(|mut d| d.next().is_none()) {
                let _ = fs::remove_dir(&from);
                continue;
            }
            problems.push(format!(
                "{} and {} both exist: keep the one you want and move the other away",
                from.display(),
                to.display()
            ));
            continue;
        }
        let moved = to
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::rename(&from, &to));
        match moved {
            Ok(()) => info!("moved {} to {}", from.display(), to.display()),
            Err(e) => problems.push(format!("could not move {}: {e}", from.display())),
        }
    }
    // The old dimension folders are empty now.
    for old in ["DIM-1", "DIM1"] {
        let _ = fs::remove_dir(dir.join(old));
    }
    problems
}

/// A player's JSON file (statistics, advancements). One that exists but cannot be read
/// is kept aside as `<name>.corrupt`, so starting afresh does not write over it.
pub(super) fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(json) => Some(json),
        Err(e) => {
            let aside = path.with_extension("json.corrupt");
            error!(
                "{} is unreadable ({e}); kept as {}",
                path.display(),
                aside.display()
            );
            let _ = fs::rename(path, aside);
            None
        }
    }
}

/// Writes through a temporary file so a crash never leaves a half-written file.
pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
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

/// Parts of a saved chunk Levitas does not handle: kept to be written back unchanged.
#[derive(Default)]
pub struct KeptChunkData {
    /// Structure starts and references (fortresses, outposts, ...).
    structures: Option<Tag>,
    inhabited_time: Option<Tag>,
    /// Block entities of kinds Levitas does not know yet (shulker boxes, beehives,
    /// lecterns, banners, ...), with the block they belong to: one is dropped once its
    /// block is gone.
    block_entities: Vec<(BlockPos, &'static str, Compound)>,
}

impl KeptChunkData {
    /// The chunk's `InhabitedTime`, in ticks (0 if it never had one).
    pub(super) fn inhabited_time(&self) -> i64 {
        match self.inhabited_time {
            Some(Tag::Long(t)) => t,
            _ => 0,
        }
    }
}

impl Player {
    /// The vanilla `players/data/<uuid>.dat` contents.
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
        let needed = super::experience::points_for_level(self.xp_level);
        let nbt = Compound::new()
            .with("DataVersion", DATA_VERSION)
            .with("XpLevel", self.xp_level)
            .with("XpP", self.xp_points as f32 / needed as f32)
            .with("XpTotal", self.xp_total)
            .with(
                "active_effects",
                super::effects::effects_to_nbt(&self.effects),
            )
            .with("AbsorptionAmount", self.absorption)
            .with("XpSeed", self.enchant_seed)
            .with("Air", self.air.clamp(0, 300) as i16)
            .with("Fire", self.fire_ticks.clamp(0, i32::from(i16::MAX)) as i16)
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
            .with("equipment", equipment);
        let mut nbt = match self.respawn {
            Some(bed) => nbt.with(
                "respawn",
                Compound::new()
                    .with("pos", vec![bed.x, bed.y, bed.z])
                    .with("dimension", "minecraft:overworld")
                    .with("angle", 0.0f32)
                    .with("forced", self.respawn_forced),
            ),
            None => nbt,
        };
        if let Some((dim, pos)) = self.last_death {
            nbt.insert(
                "LastDeathLocation",
                Compound::new()
                    .with("dimension", dim)
                    .with("pos", vec![pos.x, pos.y, pos.z]),
            );
        }
        // Keep what Levitas does not manage (the ender chest, ...), minus what it
        // decides itself.
        let mut out = self.saved_extra.clone();
        for key in ["respawn", "LastDeathLocation"] {
            out.remove(key);
        }
        for (key, tag) in nbt.iter() {
            out.insert(key, tag.clone());
        }
        out
    }

    /// Applies saved data. A player saved while dead comes back alive at spawn.
    pub(super) fn load_nbt(&mut self, nbt: &Compound) {
        self.saved_extra = nbt.clone();
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
        } else {
            // Before 25w03a armour and the offhand were inventory slots 100-103 and -106.
            for item in nbt.list("Inventory").unwrap_or_default() {
                let Some(c) = item.as_compound() else {
                    continue;
                };
                let slot = match c.int("Slot").map(|s| s as i8) {
                    Some(100) => 8,
                    Some(101) => 7,
                    Some(102) => 6,
                    Some(103) => 5,
                    Some(-106) => OFFHAND,
                    _ => continue,
                };
                self.inventory.slots[slot] = Stack::from_nbt(c);
            }
        }
        self.held = nbt.int("SelectedItemSlot").unwrap_or(0).clamp(0, 8) as usize;
        self.xp_level = nbt.int("XpLevel").unwrap_or(0).clamp(0, 1_000_000) as i32;
        let needed = super::experience::points_for_level(self.xp_level);
        let progress = nbt.float("XpP").unwrap_or(0.0).clamp(0.0, 1.0);
        self.xp_points = ((progress * f64::from(needed)) as i32).min(needed - 1);
        self.xp_total = nbt.int("XpTotal").unwrap_or(0).max(0) as i32;
        self.effects = super::effects::effects_from_nbt(nbt.get("active_effects"));
        self.absorption = nbt.float("AbsorptionAmount").unwrap_or(0.0).max(0.0) as f32;
        if let Some(seed) = nbt.int("XpSeed") {
            self.enchant_seed = seed as i32;
        }
        self.air = nbt.int("Air").unwrap_or(300).clamp(0, 300) as i32;
        self.fire_ticks = nbt.int("Fire").unwrap_or(0).max(0) as i32;
        self.respawn_forced = nbt
            .compound("respawn")
            .is_some_and(|r| r.int("forced").unwrap_or(0) != 0);
        self.respawn = nbt
            .compound("respawn")
            .filter(|r| {
                r.str("dimension")
                    .is_none_or(|d| d == "minecraft:overworld")
            })
            .and_then(|r| r.int_array("pos"))
            .and_then(|pos| match pos {
                &[x, y, z] => Some(BlockPos::new(x, y, z)),
                _ => None,
            });
        self.last_death = nbt.compound("LastDeathLocation").and_then(|c| {
            let dim = match c.str("dimension") {
                Some("minecraft:the_nether") => "minecraft:the_nether",
                Some("minecraft:the_end") => "minecraft:the_end",
                _ => "minecraft:overworld",
            };
            let pos = match c.int_array("pos")? {
                &[x, y, z] => BlockPos::new(x, y, z),
                _ => return None,
            };
            Some((dim, pos))
        });

        let health = nbt.float("Health").unwrap_or(f64::from(MAX_HEALTH)) as f32;
        if health <= 0.0 {
            return;
        }
        self.health = health.min(MAX_HEALTH);
        self.food = nbt.int("foodLevel").unwrap_or(20).clamp(0, 20) as i32;
        self.saturation = nbt.float("foodSaturationLevel").unwrap_or(5.0) as f32;
        self.exhaustion = nbt.float("foodExhaustionLevel").unwrap_or(0.0) as f32;
        self.food_timer = nbt.int("foodTickTimer").unwrap_or(0).max(0) as u32;
        let pos: Vec<f64> = nbt
            .list("Pos")
            .unwrap_or_default()
            .iter()
            .filter_map(Tag::as_f64)
            .collect();
        // The position counts in whichever dimension the player was saved in, and that is
        // where they are handed over to.
        if let &[x, y, z] = &pos[..] {
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
            .join("players")
            .join("data")
            .join(format!("{}.dat", uuid_string(uuid)))
    }

    /// Applies a joining player's saved data, if there is any. Returns the dimension
    /// they were saved in.
    pub(super) fn load_player(&mut self, id: i32) -> Option<super::dimension::Dimension> {
        let uuid = self.players[&id].profile.uuid;
        let path = self.player_path(uuid);
        let bytes = fs::read(&path).ok()?;
        match levitas_nbt::from_gzip(&bytes) {
            Ok((_, nbt)) => {
                self.players
                    .get_mut(&id)
                    .expect("player exists")
                    .load_nbt(&nbt);
                nbt.str("Dimension")
                    .and_then(super::dimension::Dimension::from_name)
            }
            Err(e) => {
                error!("{} is unreadable, starting fresh: {e}", path.display());
                None
            }
        }
    }

    pub(super) fn save_player(&self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let path = self.player_path(p.profile.uuid);
        let nbt = p.to_nbt().with("Dimension", self.dimension.name());
        let result = fs::create_dir_all(path.parent().expect("has a parent"))
            .and_then(|()| write_atomic(&path, &levitas_nbt::to_gzip("", &nbt)));
        if let Err(e) = result {
            error!("could not save {}: {e}", path.display());
        }
        self.save_advancements(id);
        self.save_stats(id);
    }

    /// Reads a chunk from disk and registers its block entities.
    pub(super) fn load_chunk(&mut self, x: i32, z: i32) -> Option<Chunk> {
        let nbt = match self.storage.read(x, z) {
            Ok(nbt) => nbt?,
            Err(e) => {
                error!("chunk {x},{z} is unreadable; it is regenerated for now but not saved: {e}");
                self.unreadable.insert((x, z));
                return None;
            }
        };
        let loaded = match anvil::from_nbt(&nbt) {
            Ok(loaded) => loaded?,
            Err(e) => {
                error!("chunk {x},{z} cannot be loaded; it is regenerated for now but not saved: {e:?}");
                self.unreadable.insert((x, z));
                return None;
            }
        };
        let mut kept = KeptChunkData {
            structures: nbt.get("structures").cloned(),
            inhabited_time: nbt.get("InhabitedTime").cloned(),
            block_entities: Vec::new(),
        };
        for be in &loaded.block_entities {
            if BlockEntity::from_nbt(be).is_some() {
                continue;
            }
            let (Some(bx), Some(by), Some(bz)) = (be.int("x"), be.int("y"), be.int("z")) else {
                continue;
            };
            let pos = BlockPos::new(bx as i32, by as i32, bz as i32);
            if pos.chunk() == (x, z) {
                let block = loaded
                    .chunk
                    .get((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize);
                kept.block_entities
                    .push((pos, crate::blocks::name(block), be.clone()));
            }
        }
        self.kept.insert((x, z), kept);
        let mut rolled = false;
        for nbt in &loaded.block_entities {
            rolled |= self.add_block_entity(nbt, x, z);
        }
        if rolled {
            self.dirty.insert((x, z));
        }
        self.load_scheduled_ticks(&loaded.block_ticks);
        self.load_fluid_ticks(&loaded.fluid_ticks);
        Some(loaded.chunk)
    }

    /// Registers a saved or generated block entity of chunk (`x`, `z`). A container
    /// still naming a loot table (a structure chest no one has opened yet) is filled
    /// from it; returns whether that happened, so the chunk gets saved with its loot.
    pub(super) fn add_block_entity(&mut self, nbt: &Compound, x: i32, z: i32) -> bool {
        match BlockEntity::from_nbt(nbt) {
            Some((pos, mut entity)) if pos.chunk() == (x, z) => {
                let rolled = nbt.str("LootTable").is_some_and(|table| {
                    let seed = nbt.int("LootTableSeed").unwrap_or(0);
                    self.loot.fill(table, seed, entity.items_mut())
                });
                self.block_entities.insert(pos, entity);
                if rolled {
                    self.draw_loot_maps(pos);
                }
                rolled
            }
            Some(_) => {
                warn!("chunk {x},{z} lists a block entity outside itself");
                false
            }
            None => false, // a kind of block entity Levitas does not handle yet
        }
    }

    fn save_chunk(&mut self, x: i32, z: i32) {
        self.dirty.remove(&(x, z));
        if self.unreadable.contains(&(x, z)) {
            return; // the original on disk stays as it was
        }
        let Some(chunk) = self.chunks.get(&(x, z)) else {
            return;
        };
        let mut entities: Vec<Compound> = self
            .block_entities
            .iter()
            .filter(|(pos, _)| pos.chunk() == (x, z))
            .map(|(&pos, e)| e.to_nbt(pos))
            .collect();
        if let Some(kept) = self.kept.get_mut(&(x, z)) {
            // Unknown block entities go back while their block is still there.
            kept.block_entities.retain(|(pos, block, _)| {
                let now = chunk.get((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize);
                crate::blocks::name(now) == *block
            });
            entities.extend(kept.block_entities.iter().map(|(_, _, nbt)| nbt.clone()));
        }
        let ticks = self.scheduled_ticks_in(x, z);
        let fluid_ticks = self.fluid_ticks_in(x, z);
        let nbt = anvil::to_nbt(
            chunk,
            x,
            z,
            entities,
            ticks,
            fluid_ticks,
            self.tick as i64,
            self.dimension.span(),
        );
        let mut nbt = nbt;
        if let Some(kept) = self.kept.get(&(x, z)) {
            if let Some(structures) = &kept.structures {
                nbt.insert("structures", structures.clone());
            }
            if let Some(time) = &kept.inhabited_time {
                nbt.insert("InhabitedTime", time.clone());
            }
        }
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
        self.save_mobs();
        self.save_maps();
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for id in ids {
            self.save_player(id);
        }
        self.save_portals();
        self.save_fight();
        self.save_raids();
        if self.dimension != super::dimension::Dimension::Overworld {
            return; // level.dat is the overworld's to write
        }
        let level = LevelData {
            spawn: [
                self.spawn[0].floor() as i32,
                self.spawn[1] as i32,
                self.spawn[2].floor() as i32,
            ],
            time: self.tick as i64,
            day_time: self.day_time,
            weather: self.weather.clone(),
            seed: self.seed,
            kind: self.kind,
            next_map_id: super::maps::next_id(),
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
            self.unload_mobs(x, z);
            if self.dirty.contains(&(x, z)) {
                self.save_chunk(x, z);
            }
            self.chunks.remove(&(x, z));
            self.block_entities.retain(|pos, _| pos.chunk() != (x, z));
            self.kept.remove(&(x, z));
            self.unreadable.remove(&(x, z));
            self.drop_scheduled_ticks(x, z);
            self.drop_fluid_ticks(x, z);
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

    /// A chest a structure placed comes filled from its loot table, once: saved, it
    /// keeps its items and no longer names the table.
    #[test]
    fn generated_chests_arrive_filled() {
        let dir = std::env::temp_dir().join(format!("levitas-loot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (mut w, _) = World::new(
            dir.clone(),
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let pos = BlockPos::new(5 * 16 + 3, -60, 5 * 16 + 4);
        let mut chunk = w.terrain.generate(5, 5);
        chunk.set(3, pos.y, 4, block::CHEST);
        let nbt = Compound::new()
            .with("id", "minecraft:chest")
            .with("x", pos.x)
            .with("y", pos.y)
            .with("z", pos.z)
            .with("LootTable", "minecraft:chests/buried_treasure")
            .with("LootTableSeed", 7i64);
        w.area_generated(vec![Generated {
            pos: (5, 5),
            chunk,
            block_entities: vec![nbt],
            entities: Vec::new(),
        }]);
        let heart = ItemInfo::id_of("minecraft:heart_of_the_sea").unwrap();
        let Some(BlockEntity::Chest(c)) = w.block_entities.get(&pos) else {
            panic!("no chest");
        };
        assert!(c.items.iter().any(|s| s.item == heart));
        let saved = w.block_entities[&pos].to_nbt(pos);
        assert!(saved.str("LootTable").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    /// What a saved chunk holds that Levitas does not handle (a shulker box's block
    /// entity, structure starts) is written back unchanged, until its block is gone.
    #[test]
    fn unknown_chunk_data_survives_a_save() {
        let dir = std::env::temp_dir().join(format!("levitas-keep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (mut w, _) = World::new(
            dir.clone(),
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let pos = BlockPos::new(1, -60, 1);
        w.set_block(pos, block::BLUE_SHULKER_BOX);
        w.save_chunk(0, 0);
        // Put an unknown block entity and structure data into the saved chunk.
        let mut nbt = w.storage.read(0, 0).unwrap().unwrap();
        let shulker = Compound::new()
            .with("id", "minecraft:shulker_box")
            .with("x", pos.x)
            .with("y", pos.y)
            .with("z", pos.z)
            .with("Lock", "secret");
        nbt.insert("block_entities", vec![Tag::Compound(shulker)]);
        nbt.insert("structures", Compound::new().with("marker", 1i32));
        nbt.insert("InhabitedTime", 1234i64);
        w.storage.write(0, 0, &nbt).unwrap();
        // Load it again and save it after a change elsewhere in the chunk.
        let chunk = w.load_chunk(0, 0).unwrap();
        w.chunks.insert((0, 0), chunk);
        w.set_block(BlockPos::new(5, -60, 5), block::STONE);
        w.save_chunk(0, 0);
        let saved = w.storage.read(0, 0).unwrap().unwrap();
        let entities = saved.list("block_entities").unwrap();
        assert!(entities
            .iter()
            .filter_map(Tag::as_compound)
            .any(|c| c.str("Lock") == Some("secret")));
        assert_eq!(saved.compound("structures").unwrap().int("marker"), Some(1));
        assert_eq!(saved.int("InhabitedTime"), Some(1234));
        // Once the box is broken its data goes too.
        w.set_block(pos, block::AIR);
        w.save_chunk(0, 0);
        let saved = w.storage.read(0, 0).unwrap().unwrap();
        assert!(!saved
            .list("block_entities")
            .unwrap()
            .iter()
            .filter_map(Tag::as_compound)
            .any(|c| c.str("Lock") == Some("secret")));
        // An unreadable chunk is never written over.
        w.unreadable.insert((0, 0));
        w.set_block(BlockPos::new(6, -60, 6), block::STONE);
        w.save_chunk(0, 0);
        let again = w.storage.read(0, 0).unwrap().unwrap();
        assert_eq!(again, saved);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The parts of a player file Levitas does not manage (the ender chest) stay, armour
    /// in the old inventory slots is worn, and the position counts in any dimension.
    #[test]
    fn player_files_keep_what_levitas_does_not_manage() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let profile = crate::connection::Profile {
            uuid: 5,
            name: "Keeper".to_owned(),
            properties: Vec::new(),
        };
        let mut p = Player::new(profile, tx, [0.0; 3]);
        let helmet = Compound::new()
            .with("id", "minecraft:iron_helmet")
            .with("count", 1i32)
            .with("Slot", 103i8);
        let shield = Compound::new()
            .with("id", "minecraft:shield")
            .with("count", 1i32)
            .with("Slot", -106i8);
        let saved = Compound::new()
            .with("Dimension", "minecraft:the_nether")
            .with(
                "Pos",
                vec![Tag::Double(30.5), Tag::Double(40.0), Tag::Double(-90.5)],
            )
            .with(
                "Inventory",
                vec![Tag::Compound(helmet), Tag::Compound(shield)],
            )
            .with(
                "EnderItems",
                vec![Tag::Compound(
                    Compound::new().with("id", "minecraft:diamond"),
                )],
            );
        p.load_nbt(&saved);
        assert_eq!(p.pos, [30.5, 40.0, -90.5]);
        let head = ItemInfo::id_of("minecraft:iron_helmet").unwrap();
        assert_eq!(p.inventory.slots[5].item, head);
        let shield = ItemInfo::id_of("minecraft:shield").unwrap();
        assert_eq!(p.inventory.slots[OFFHAND].item, shield);
        let out = p.to_nbt();
        assert_eq!(out.list("EnderItems").map(<[Tag]>::len), Some(1));
        // The armour is now saved the new way.
        assert!(out
            .compound("equipment")
            .unwrap()
            .compound("head")
            .is_some());
    }

    #[test]
    fn old_forms_and_damaged_files_load_safely() {
        // Dyed leather from 1.20.5-1.21.4 saved its colour as {rgb: N}.
        let boots = Compound::new()
            .with("id", "minecraft:leather_boots")
            .with("count", 1i32)
            .with(
                "components",
                Compound::new().with(
                    "minecraft:dyed_color",
                    Compound::new().with("rgb", 0x12_3456i32),
                ),
            );
        assert_eq!(Stack::from_nbt(&boots).color, Some(0x12_3456));
        // Amplifiers above 127 survive saving (the byte is unsigned).
        let strong = vec![super::super::effects::ActiveEffect {
            id: levitas_registry::mob_effect::STRENGTH,
            amplifier: 200,
            duration: 100,
        }];
        let tag = super::super::effects::effects_to_nbt(&strong);
        assert_eq!(
            super::super::effects::effects_from_nbt(Some(&tag))[0].amplifier,
            200
        );
        // A damaged JSON file is kept aside instead of being written over.
        let dir = std::env::temp_dir().join(format!("levitas-json-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stats.json");
        fs::write(&path, "{ broken").unwrap();
        assert!(read_json(&path).is_none());
        assert!(dir.join("stats.json.corrupt").is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_upgrade_that_would_hide_data_is_refused() {
        let dir = std::env::temp_dir().join(format!("levitas-clash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // A 1.21.8 region folder, and a 26.3 one made by an earlier run.
        fs::create_dir_all(dir.join("region")).unwrap();
        fs::write(dir.join("region").join("r.0.0.mca"), b"old").unwrap();
        let new = super::super::dimension::Dimension::Overworld
            .folder(&dir)
            .join("region");
        fs::create_dir_all(&new).unwrap();
        let problems = upgrade_layout(&dir);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            dir.join("region").join("r.0.0.mca").is_file(),
            "nothing is lost"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn level_data_in_a_new_folder() {
        let dir = std::env::temp_dir().join(format!("levitas-level-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(LevelData::load(&dir).is_none());
        let level = LevelData {
            spawn: [3, -60, -7],
            time: 12_345,
            day_time: 30_000,
            weather: crate::world::time::Weather {
                raining: true,
                rain_time: 777,
                ..Default::default()
            },
            seed: -42,
            kind: WorldKind::Noise,
            next_map_id: 7,
        };
        level.save(&dir).unwrap();
        // Saving twice keeps the previous copy as level.dat_old.
        level.save(&dir).unwrap();
        assert!(dir.join("level.dat_old").is_file());
        let loaded = LevelData::load(&dir).unwrap();
        assert_eq!((loaded.spawn, loaded.time), ([3, -60, -7], 12_345));
        assert_eq!(loaded.day_time, 30_000);
        assert!(loaded.weather.raining && !loaded.weather.thundering);
        assert_eq!(loaded.weather.rain_time, 777);
        assert_eq!((loaded.seed, loaded.kind), (-42, WorldKind::Noise));
        assert_eq!(loaded.next_map_id, 7);
        // Saved as vanilla 26.3 does: the last ID given out, in maps/last_id.dat.
        let last = saved_data(&dir, "maps/last_id").unwrap();
        assert_eq!(last.int("map"), Some(6));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn leaf_worlds_bring_their_nether_end_stats_and_advancements() {
        // Paper and Leaf: the Nether and the End in folders next to the world, and the
        // statistics and advancements of 1.21.8 inside it.
        let root = std::env::temp_dir().join(format!("levitas-leaf-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("world");
        for folder in [
            "world/region",
            "world/stats",
            "world/advancements",
            "world_nether/DIM-1/region",
            "world_the_end/DIM1/entities",
        ] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        fs::write(root.join("world_nether/DIM-1/region/r.0.0.mca"), b"nether").unwrap();
        fs::write(
            root.join("world_the_end/DIM1/entities/r.0.0.mca"),
            b"end mobs",
        )
        .unwrap();
        fs::write(dir.join("stats/alice.json"), b"{}").unwrap();
        fs::write(dir.join("advancements/alice.json"), b"{}").unwrap();

        assert!(upgrade_layout(&dir).is_empty());
        let world = dir.join("dimensions/minecraft");
        assert_eq!(
            fs::read(world.join("the_nether/region/r.0.0.mca")).unwrap(),
            b"nether"
        );
        assert_eq!(
            fs::read(world.join("the_end/entities/r.0.0.mca")).unwrap(),
            b"end mobs"
        );
        assert!(dir.join("players/stats/alice.json").is_file());
        assert!(dir.join("players/advancements/alice.json").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worlds_saved_by_1_21_8_move_into_the_26_3_layout() {
        let dir = std::env::temp_dir().join(format!("levitas-upgrade-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for folder in [
            "region",
            "entities",
            "DIM-1/region",
            "DIM1/region",
            "playerdata",
        ] {
            fs::create_dir_all(dir.join(folder)).unwrap();
        }
        fs::write(dir.join("region/r.0.0.mca"), b"overworld").unwrap();
        fs::write(dir.join("DIM-1/region/r.0.0.mca"), b"nether").unwrap();
        fs::write(dir.join("DIM1/levitas_dragon.dat"), b"fight").unwrap();
        fs::write(dir.join("playerdata/alice.dat"), b"alice").unwrap();
        // A 1.21.8 level.dat: rules as text, weather and spawn inline.
        let data = Compound::new()
            .with("DataVersion", 4440)
            .with("GameRules", Compound::new().with("keepInventory", "true"))
            .with("SpawnX", 5)
            .with("SpawnY", 70)
            .with("SpawnZ", -9)
            .with("DayTime", 1234i64)
            .with("raining", true)
            .with("rainTime", 50);
        let root = Compound::new().with("Data", data);
        fs::write(dir.join("level.dat"), levitas_nbt::to_gzip("", &root)).unwrap();

        assert!(upgrade_layout(&dir).is_empty());
        let world = dir.join("dimensions/minecraft");
        let read = |path: &str| fs::read(world.join(path)).unwrap();
        assert_eq!(read("overworld/region/r.0.0.mca"), b"overworld");
        assert!(world.join("overworld/entities").is_dir());
        assert_eq!(read("the_nether/region/r.0.0.mca"), b"nether");
        assert_eq!(read("the_end/levitas_dragon.dat"), b"fight");
        assert_eq!(
            fs::read(dir.join("players/data/alice.dat")).unwrap(),
            b"alice"
        );
        assert!(!dir.join("region").exists() && !dir.join("DIM-1").exists());

        let level = LevelData::load(&dir).unwrap();
        assert!(crate::rules::bool("keep_inventory"));
        assert_eq!((level.spawn, level.day_time), ([5, 70, -9], 1234));
        assert!(level.weather.raining);
        assert_eq!(level.weather.rain_time, 50);
        // Saved again, it is a 26.3 world that remembers it was a 1.21.8 one.
        level.save(&dir).unwrap();
        let (_, root) = levitas_nbt::from_gzip(&fs::read(dir.join("level.dat")).unwrap()).unwrap();
        let data = root.compound("Data").unwrap();
        assert_eq!(data.int("DataVersion"), Some(i64::from(DATA_VERSION)));
        assert_eq!(
            data.compound("spawn").unwrap().int_array("pos"),
            Some(&[5, 70, -9][..])
        );
        assert_eq!(
            saved_data(&dir, "game_rules")
                .unwrap()
                .int("minecraft:keep_inventory"),
            Some(1)
        );
        crate::rules::set("keep_inventory", "false").unwrap();
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
            properties: Vec::new(),
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
            properties: Vec::new(),
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
