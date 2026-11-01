//! Statistics, as the Minecraft Wiki's "Statistics" and protocol pages describe them:
//! counts per category (blocks mined, items crafted, used, broken, picked up and dropped,
//! entities killed and killed by) and the custom ones (play time, distances, deaths, ...).
//! The client asks for them when its statistics screen opens; they are saved in
//! `players/stats/<uuid>.json` as vanilla saves them.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use levitas_registry::{custom_stat, stat_type, BLOCKS, DATA_VERSION};
use serde_json::Value;
use tracing::error;

use super::*;

/// A player's statistics: values by (category, statistic) IDs.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    values: HashMap<(i32, i32), i32>,
}

impl Stats {
    pub fn add(&mut self, category: i32, stat: i32, amount: i32) {
        if amount <= 0 {
            return;
        }
        let value = self.values.entry((category, stat)).or_insert(0);
        *value = value.saturating_add(amount);
    }

    pub fn custom(&mut self, stat: i32, amount: i32) {
        self.add(stat_type::CUSTOM, stat, amount);
    }

    pub fn set(&mut self, category: i32, stat: i32, value: i32) {
        self.values.insert((category, stat), value);
    }

    #[cfg(test)]
    pub fn get(&self, category: i32, stat: i32) -> i32 {
        self.values.get(&(category, stat)).copied().unwrap_or(0)
    }

    /// The statistics packet: every value.
    pub fn packet(&self) -> Writer {
        let mut packet = Writer::packet(clientbound::AWARD_STATS).varint(self.values.len() as i32);
        for (&(category, stat), &value) in &self.values {
            packet = packet.varint(category).varint(stat).varint(value);
        }
        packet
    }

    /// The vanilla saved form: `{stats: {category: {statistic: value}}, DataVersion}`.
    pub fn to_json(&self) -> Value {
        let mut stats: serde_json::Map<String, Value> = serde_json::Map::new();
        for (&(category, stat), &value) in &self.values {
            let (Some(c), Some(s)) = (
                stat_type::NAMES.get(category as usize),
                name(category, stat),
            ) else {
                continue;
            };
            stats
                .entry(c.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()))
                .as_object_mut()
                .expect("an object")
                .insert(s.to_owned(), Value::from(value));
        }
        serde_json::json!({ "stats": stats, "DataVersion": DATA_VERSION })
    }

    pub fn from_json(json: &Value) -> Stats {
        let mut out = Stats::default();
        for (category, values) in json["stats"].as_object().into_iter().flatten() {
            let Some(c) = stat_type::NAMES.iter().position(|&n| n == category) else {
                continue;
            };
            for (stat, value) in values.as_object().into_iter().flatten() {
                if let (Some(s), Some(v)) = (id(c as i32, stat), value.as_i64()) {
                    out.set(c as i32, s, v.clamp(0, i64::from(i32::MAX)) as i32);
                }
            }
        }
        out
    }
}

/// The name of statistic `stat` in `category`: a block, item, entity type or custom one.
fn name(category: i32, stat: i32) -> Option<&'static str> {
    match category {
        stat_type::MINED => BLOCKS.get(stat as usize).map(|b| b.name),
        stat_type::KILLED | stat_type::KILLED_BY => entity_type::NAMES.get(stat as usize).copied(),
        stat_type::CUSTOM => custom_stat::NAMES.get(stat as usize).copied(),
        _ => ItemInfo::get(stat).map(|i| i.name),
    }
}

fn id(category: i32, name: &str) -> Option<i32> {
    let index = |names: &[&str]| names.iter().position(|&n| n == name).map(|i| i as i32);
    match category {
        stat_type::MINED => BLOCKS.iter().position(|b| b.name == name).map(|i| i as i32),
        stat_type::KILLED | stat_type::KILLED_BY => index(entity_type::NAMES),
        stat_type::CUSTOM => index(custom_stat::NAMES),
        _ => ItemInfo::id_of(name),
    }
}

/// The custom statistic for opening a block's screen, if there is one.
pub fn opened(block: &str) -> Option<i32> {
    let short = block.strip_prefix("minecraft:")?;
    Some(match short {
        "chest" | "trapped_chest" => custom_stat::OPEN_CHEST,
        "barrel" => custom_stat::OPEN_BARREL,
        "ender_chest" => custom_stat::OPEN_ENDERCHEST,
        "crafting_table" => custom_stat::INTERACT_WITH_CRAFTING_TABLE,
        "furnace" => custom_stat::INTERACT_WITH_FURNACE,
        "blast_furnace" => custom_stat::INTERACT_WITH_BLAST_FURNACE,
        "smoker" => custom_stat::INTERACT_WITH_SMOKER,
        "anvil" | "chipped_anvil" | "damaged_anvil" => custom_stat::INTERACT_WITH_ANVIL,
        "grindstone" => custom_stat::INTERACT_WITH_GRINDSTONE,
        "stonecutter" => custom_stat::INTERACT_WITH_STONECUTTER,
        "smithing_table" => custom_stat::INTERACT_WITH_SMITHING_TABLE,
        "cartography_table" => custom_stat::INTERACT_WITH_CARTOGRAPHY_TABLE,
        "loom" => custom_stat::INTERACT_WITH_LOOM,
        "brewing_stand" => custom_stat::INTERACT_WITH_BREWINGSTAND,
        "beacon" => custom_stat::INTERACT_WITH_BEACON,
        "lectern" => custom_stat::INTERACT_WITH_LECTERN,
        "hopper" => custom_stat::INSPECT_HOPPER,
        "dropper" => custom_stat::INSPECT_DROPPER,
        "dispenser" => custom_stat::INSPECT_DISPENSER,
        _ if short.ends_with("shulker_box") => custom_stat::OPEN_SHULKER_BOX,
        _ => return None,
    })
}

/// Centimetres, as distance statistics count them.
fn cm(blocks: f64) -> i32 {
    (blocks * 100.0).round() as i32
}

/// How a player moved, for the distance statistics.
pub struct Movement {
    pub horizontal: f64,
    pub vertical: f64,
    pub on_ground: bool,
    pub was_on_ground: bool,
    pub in_water: bool,
    pub climbing: bool,
    pub flying: bool,
}

impl Stats {
    /// Counts one movement: the distance by how it was covered, and jumps.
    pub fn moved(&mut self, m: &Movement, sneaking: bool, sprinting: bool) {
        let total = m.horizontal.hypot(m.vertical);
        if m.in_water {
            self.custom(custom_stat::SWIM_ONE_CM, cm(total));
        } else if m.climbing && m.vertical > 0.0 {
            self.custom(custom_stat::CLIMB_ONE_CM, cm(m.vertical));
        } else if m.on_ground {
            let stat = if sneaking {
                custom_stat::CROUCH_ONE_CM
            } else if sprinting {
                custom_stat::SPRINT_ONE_CM
            } else {
                custom_stat::WALK_ONE_CM
            };
            self.custom(stat, cm(m.horizontal));
        } else if m.flying {
            self.custom(custom_stat::FLY_ONE_CM, cm(total));
        } else {
            self.custom(custom_stat::FLY_ONE_CM, cm(m.horizontal));
            if m.vertical < 0.0 {
                self.custom(custom_stat::FALL_ONE_CM, cm(-m.vertical));
            }
        }
        if m.was_on_ground && !m.on_ground && m.vertical > 0.0 && !m.in_water && !m.climbing {
            self.custom(custom_stat::JUMP, 1);
        }
    }
}

impl World {
    /// Adds to one of a player's statistics.
    pub(super) fn stat(&mut self, id: i32, category: i32, stat: i32, amount: i32) {
        if let Some(p) = self.players.get_mut(&id) {
            p.stats.add(category, stat, amount);
        }
    }

    /// The time statistics, once a tick for everyone here.
    pub(super) fn tick_stats(&mut self) {
        for p in self.players.values_mut() {
            p.stats.custom(custom_stat::PLAY_TIME, 1);
            p.stats.custom(custom_stat::TOTAL_WORLD_TIME, 1);
            if p.alive {
                p.stats.custom(custom_stat::TIME_SINCE_DEATH, 1);
            }
            if p.sleeping.is_none() {
                p.stats.custom(custom_stat::TIME_SINCE_REST, 1);
            }
            if p.sneaking {
                p.stats.custom(custom_stat::SNEAK_TIME, 1);
            }
        }
    }

    /// Sends a player their statistics, as their statistics screen asks.
    pub(super) fn send_stats(&mut self, id: i32) {
        if let Some(packet) = self.players.get(&id).map(|p| p.stats.packet()) {
            self.send(id, packet);
        }
    }

    fn stats_path(&self, uuid: u128) -> PathBuf {
        self.dir
            .join("players")
            .join("stats")
            .join(format!("{}.json", storage::uuid_string(uuid)))
    }

    pub(super) fn load_stats(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let stats = fs::read_to_string(self.stats_path(p.profile.uuid))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|json| Stats::from_json(&json))
            .unwrap_or_default();
        if let Some(p) = self.players.get_mut(&id) {
            p.stats = stats;
        }
    }

    pub(super) fn save_stats(&self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let path = self.stats_path(p.profile.uuid);
        let saved = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, p.stats.to_json().to_string()));
        if let Err(e) = saved {
            error!("could not save statistics to {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_are_saved_as_vanilla_writes_them() {
        let mut stats = Stats::default();
        let stone = BLOCKS
            .iter()
            .position(|b| b.name == "minecraft:stone")
            .unwrap() as i32;
        stats.add(stat_type::MINED, stone, 5);
        stats.custom(custom_stat::PLAY_TIME, 1200);
        let json = stats.to_json();
        assert_eq!(json["stats"]["minecraft:mined"]["minecraft:stone"], 5);
        assert_eq!(
            json["stats"]["minecraft:custom"]["minecraft:play_time"],
            1200
        );
        let back = Stats::from_json(&json);
        assert_eq!(back.get(stat_type::MINED, stone), 5);
        assert_eq!(back.get(stat_type::CUSTOM, custom_stat::PLAY_TIME), 1200);
    }

    #[test]
    fn mining_counts_and_the_screen_gets_the_numbers() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-stats-{}-{}",
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
        let (out, mut frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = crate::connection::Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, out, [0.5, -60.0, 0.5]));
        let pos = BlockPos::new(2, -61, 2);
        let grass = w.block(pos);
        w.break_block(id, pos, grass);
        let grass_block = i32::from(levitas_registry::STATE_BLOCK[usize::from(grass)]);
        assert_eq!(w.players[&id].stats.get(stat_type::MINED, grass_block), 1);
        while frames.try_recv().is_ok() {}
        w.send_stats(id);
        let frame = frames.try_recv().expect("the statistics");
        let mut r = levitas_protocol::Reader::new(&frame);
        r.varint().unwrap(); // length
        assert_eq!(r.varint().unwrap(), clientbound::AWARD_STATS);
        let n = r.varint().unwrap();
        let entries: Vec<(i32, i32, i32)> = (0..n)
            .map(|_| {
                (
                    r.varint().unwrap(),
                    r.varint().unwrap(),
                    r.varint().unwrap(),
                )
            })
            .collect();
        assert!(entries.contains(&(stat_type::MINED, grass_block, 1)));
        assert!(r.bytes(1).is_err());
        // Saved and read back on the next login.
        w.save_stats(id);
        w.players.get_mut(&id).unwrap().stats = Stats::default();
        w.load_stats(id);
        assert_eq!(w.players[&id].stats.get(stat_type::MINED, grass_block), 1);
    }

    #[test]
    fn movements_count_by_how_they_were_made() {
        let mut stats = Stats::default();
        let walk = Movement {
            horizontal: 0.2,
            vertical: 0.0,
            on_ground: true,
            was_on_ground: true,
            in_water: false,
            climbing: false,
            flying: false,
        };
        stats.moved(&walk, false, false);
        stats.moved(&walk, false, true);
        stats.moved(&walk, true, false);
        let jump = Movement {
            vertical: 0.42,
            on_ground: false,
            ..walk
        };
        stats.moved(&jump, false, false);
        let custom = |s: &Stats, stat| s.get(stat_type::CUSTOM, stat);
        assert_eq!(custom(&stats, custom_stat::WALK_ONE_CM), 20);
        assert_eq!(custom(&stats, custom_stat::SPRINT_ONE_CM), 20);
        assert_eq!(custom(&stats, custom_stat::CROUCH_ONE_CM), 20);
        assert_eq!(custom(&stats, custom_stat::JUMP), 1);
        assert_eq!(
            opened("minecraft:red_shulker_box"),
            Some(custom_stat::OPEN_SHULKER_BOX)
        );
        assert_eq!(opened("minecraft:stone"), None);
    }
}
