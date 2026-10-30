//! Game rules and difficulty, shared by every dimension of the world.
//!
//! The rules are vanilla 26.3's, by their vanilla names, saved in
//! `data/minecraft/game_rules.dat` the way vanilla saves them, so a world's rules carry
//! over in both directions. Rules from a 1.21.8 `level.dat` are read under their new names.
//! Every rule is kept and saved; the ones Levitas enforces are listed in ROADMAP.md.
//! The difficulty comes from `server.properties` at start, as on a vanilla dedicated
//! server, and `/difficulty` changes it until the next restart.

#[cfg(not(test))]
use std::sync::{OnceLock, RwLock};

use levitas_nbt::Compound;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i32),
}

impl Value {
    fn text(self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
        }
    }

    /// The same kind of value, parsed from `text`.
    fn parse(self, text: &str) -> Option<Value> {
        match self {
            Value::Bool(_) => match text {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            Value::Int(_) => text.parse().ok().map(Value::Int),
        }
    }
}

use Value::{Bool, Int};

/// Every 26.3 game rule with its default, sorted by name. Saved and shown with the
/// `minecraft:` namespace, which commands may leave out.
const DEFAULTS: [(&str, Value); 59] = [
    ("advance_time", Bool(true)),
    ("advance_weather", Bool(true)),
    ("allow_entering_nether_using_portals", Bool(true)),
    ("block_drops", Bool(true)),
    ("block_explosion_drop_decay", Bool(true)),
    ("command_block_output", Bool(true)),
    ("command_blocks_work", Bool(true)),
    ("drowning_damage", Bool(true)),
    ("elytra_movement_check", Bool(true)),
    ("ender_pearls_vanish_on_death", Bool(true)),
    ("entity_drops", Bool(true)),
    ("fall_damage", Bool(true)),
    ("fire_damage", Bool(true)),
    ("fire_spread_radius_around_player", Int(128)),
    ("forgive_dead_players", Bool(true)),
    ("freeze_damage", Bool(true)),
    ("global_sound_events", Bool(true)),
    ("immediate_respawn", Bool(false)),
    ("keep_inventory", Bool(false)),
    ("lava_source_conversion", Bool(false)),
    ("limited_crafting", Bool(false)),
    ("locator_bar", Bool(true)),
    ("log_admin_commands", Bool(true)),
    ("max_block_modifications", Int(32768)),
    ("max_command_forks", Int(65536)),
    ("max_command_sequence_length", Int(65536)),
    ("max_entity_cramming", Int(24)),
    ("max_minecart_speed", Int(8)),
    ("max_snow_accumulation_height", Int(1)),
    ("mob_drops", Bool(true)),
    ("mob_explosion_drop_decay", Bool(true)),
    ("mob_griefing", Bool(true)),
    ("natural_health_regeneration", Bool(true)),
    ("player_movement_check", Bool(true)),
    ("players_nether_portal_creative_delay", Int(0)),
    ("players_nether_portal_default_delay", Int(80)),
    ("players_sleeping_percentage", Int(100)),
    ("projectiles_can_break_blocks", Bool(true)),
    ("pvp", Bool(true)),
    ("raids", Bool(true)),
    ("random_tick_speed", Int(3)),
    ("reduced_debug_info", Bool(false)),
    ("respawn_radius", Int(10)),
    ("send_command_feedback", Bool(true)),
    ("show_advancement_messages", Bool(true)),
    ("show_death_messages", Bool(true)),
    ("spawn_mobs", Bool(true)),
    ("spawn_monsters", Bool(true)),
    ("spawn_patrols", Bool(true)),
    ("spawn_phantoms", Bool(true)),
    ("spawn_wandering_traders", Bool(true)),
    ("spawn_wardens", Bool(true)),
    ("spawner_blocks_work", Bool(true)),
    ("spectators_generate_chunks", Bool(true)),
    ("spread_vines", Bool(true)),
    ("tnt_explodes", Bool(true)),
    ("tnt_explosion_drop_decay", Bool(false)),
    ("universal_anger", Bool(false)),
    ("water_source_conversion", Bool(true)),
];

/// 1.21.8 rule names and what they became, for worlds saved before 26.3. `true` marks
/// a rule whose meaning flipped (`disableRaids` became `raids`).
const RENAMED: [(&str, &str, bool); 48] = [
    ("announceAdvancements", "show_advancement_messages", false),
    (
        "blockExplosionDropDecay",
        "block_explosion_drop_decay",
        false,
    ),
    ("commandBlockOutput", "command_block_output", false),
    (
        "commandModificationBlockLimit",
        "max_block_modifications",
        false,
    ),
    ("disableElytraMovementCheck", "elytra_movement_check", true),
    ("disablePlayerMovementCheck", "player_movement_check", true),
    ("disableRaids", "raids", true),
    ("doDaylightCycle", "advance_time", false),
    ("doEntityDrops", "entity_drops", false),
    ("doImmediateRespawn", "immediate_respawn", false),
    ("doInsomnia", "spawn_phantoms", false),
    ("doLimitedCrafting", "limited_crafting", false),
    ("doMobLoot", "mob_drops", false),
    ("doMobSpawning", "spawn_mobs", false),
    ("doPatrolSpawning", "spawn_patrols", false),
    ("doTileDrops", "block_drops", false),
    ("doTraderSpawning", "spawn_wandering_traders", false),
    ("doVinesSpread", "spread_vines", false),
    ("doWardenSpawning", "spawn_wardens", false),
    ("doWeatherCycle", "advance_weather", false),
    ("drowningDamage", "drowning_damage", false),
    (
        "enderPearlsVanishOnDeath",
        "ender_pearls_vanish_on_death",
        false,
    ),
    ("fallDamage", "fall_damage", false),
    ("fireDamage", "fire_damage", false),
    ("forgiveDeadPlayers", "forgive_dead_players", false),
    ("freezeDamage", "freeze_damage", false),
    ("globalSoundEvents", "global_sound_events", false),
    ("keepInventory", "keep_inventory", false),
    ("lavaSourceConversion", "lava_source_conversion", false),
    ("logAdminCommands", "log_admin_commands", false),
    (
        "maxCommandChainLength",
        "max_command_sequence_length",
        false,
    ),
    ("maxCommandForkCount", "max_command_forks", false),
    ("maxEntityCramming", "max_entity_cramming", false),
    ("minecartMaxSpeed", "max_minecart_speed", false),
    ("mobExplosionDropDecay", "mob_explosion_drop_decay", false),
    ("mobGriefing", "mob_griefing", false),
    ("naturalRegeneration", "natural_health_regeneration", false),
    (
        "playersNetherPortalCreativeDelay",
        "players_nether_portal_creative_delay",
        false,
    ),
    (
        "playersNetherPortalDefaultDelay",
        "players_nether_portal_default_delay",
        false,
    ),
    (
        "playersSleepingPercentage",
        "players_sleeping_percentage",
        false,
    ),
    (
        "projectilesCanBreakBlocks",
        "projectiles_can_break_blocks",
        false,
    ),
    ("randomTickSpeed", "random_tick_speed", false),
    ("reducedDebugInfo", "reduced_debug_info", false),
    ("sendCommandFeedback", "send_command_feedback", false),
    ("showDeathMessages", "show_death_messages", false),
    (
        "snowAccumulationHeight",
        "max_snow_accumulation_height",
        false,
    ),
    ("spawnRadius", "respawn_radius", false),
    (
        "spectatorsGenerateChunks",
        "spectators_generate_chunks",
        false,
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Difficulty {
    Peaceful = 0,
    Easy = 1,
    Normal = 2,
    Hard = 3,
}

impl Difficulty {
    pub const ALL: [Difficulty; 4] = [
        Difficulty::Peaceful,
        Difficulty::Easy,
        Difficulty::Normal,
        Difficulty::Hard,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Difficulty::Peaceful => "peaceful",
            Difficulty::Easy => "easy",
            Difficulty::Normal => "normal",
            Difficulty::Hard => "hard",
        }
    }

    /// A difficulty by name or number, as `server.properties` and `/difficulty` take it.
    pub fn parse(text: &str) -> Option<Difficulty> {
        let text = text.trim();
        Difficulty::ALL
            .into_iter()
            .find(|d| d.name() == text || (*d as u8).to_string() == text)
    }
}

struct Rules {
    values: Vec<Value>,
    difficulty: Difficulty,
}

impl Rules {
    fn new() -> Self {
        Rules {
            values: DEFAULTS.iter().map(|&(_, v)| v).collect(),
            difficulty: Difficulty::parse(&crate::config::get().difficulty)
                .unwrap_or(Difficulty::Easy),
        }
    }
}

/// Reads the rules in force.
#[cfg(not(test))]
fn read<T>(f: impl FnOnce(&Rules) -> T) -> T {
    f(&global().read().expect("game rules"))
}

/// Changes the rules in force.
#[cfg(not(test))]
fn write<T>(f: impl FnOnce(&mut Rules) -> T) -> T {
    f(&mut global().write().expect("game rules"))
}

#[cfg(not(test))]
fn global() -> &'static RwLock<Rules> {
    static RULES: OnceLock<RwLock<Rules>> = OnceLock::new();
    RULES.get_or_init(|| RwLock::new(Rules::new()))
}

// Tests run side by side, so each test thread has rules of its own.
#[cfg(test)]
thread_local! {
    static RULES: std::cell::RefCell<Rules> = std::cell::RefCell::new(Rules::new());
}

#[cfg(test)]
fn read<T>(f: impl FnOnce(&Rules) -> T) -> T {
    RULES.with(|r| f(&r.borrow()))
}

#[cfg(test)]
fn write<T>(f: impl FnOnce(&mut Rules) -> T) -> T {
    RULES.with(|r| f(&mut r.borrow_mut()))
}

fn index(name: &str) -> Option<usize> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    DEFAULTS.binary_search_by_key(&name, |&(n, _)| n).ok()
}

/// A rule's name without its namespace, as Levitas looks it up.
pub fn short_name(name: &str) -> Option<&'static str> {
    index(name).map(|i| DEFAULTS[i].0)
}

fn value(name: &str) -> Value {
    let i = index(name).unwrap_or_else(|| panic!("no game rule {name}"));
    read(|r| r.values[i])
}

/// A true-or-false rule, such as `keep_inventory`.
pub fn bool(name: &str) -> bool {
    matches!(value(name), Bool(true))
}

/// A number rule, such as `random_tick_speed`.
pub fn int(name: &str) -> i32 {
    match value(name) {
        Int(n) => n,
        Bool(b) => i32::from(b),
    }
}

pub fn names() -> impl Iterator<Item = &'static str> {
    DEFAULTS.iter().map(|&(n, _)| n)
}

/// The rule's value as text, or `None` if there is no such rule.
pub fn get_text(name: &str) -> Option<String> {
    index(name).map(|_| value(name).text())
}

/// Sets a rule from text. Errors say why the value or rule was refused.
pub fn set(name: &str, text: &str) -> Result<(), String> {
    let i = index(name).ok_or_else(|| format!("Unknown game rule: {name}"))?;
    write(|r| {
        let new = r.values[i]
            .parse(text)
            .ok_or_else(|| format!("Invalid value for {name}: {text}"))?;
        r.values[i] = new;
        Ok(())
    })
}

pub fn difficulty() -> Difficulty {
    read(|r| r.difficulty)
}

pub fn set_difficulty(d: Difficulty) {
    write(|r| r.difficulty = d);
}

/// The rules as 26.3 saves them in `data/minecraft/game_rules.dat`: namespaced names,
/// true-or-false rules as bytes and number rules as ints.
pub fn to_nbt() -> Compound {
    read(|r| {
        let mut nbt = Compound::new();
        for (&(name, _), value) in DEFAULTS.iter().zip(&r.values) {
            let name = format!("minecraft:{name}");
            match *value {
                Bool(b) => nbt.insert(&name, b),
                Int(n) => nbt.insert(&name, n),
            }
        }
        nbt
    })
}

/// Takes the rules a world saved; others keep their defaults. Reads 26.3's typed rules
/// and the `GameRules` text values of 1.21.8's `level.dat`, under either set of names.
pub fn load_nbt(nbt: &Compound) {
    for (name, tag) in nbt.iter() {
        let (name, flip) = match RENAMED.iter().find(|&&(old, _, _)| old == name) {
            Some(&(_, new, flip)) => (new, flip),
            None => (name, false),
        };
        let Some(i) = index(name) else {
            // Rules this version does not have are dropped.
            continue;
        };
        let value = match (DEFAULTS[i].1, tag.as_str()) {
            (Bool(_), Some(text)) => text.parse().ok().map(Bool),
            (Int(_), Some(text)) => text.parse().ok().map(Int),
            (Bool(_), None) => tag.as_i64().map(|n| Bool(n != 0)),
            (Int(_), None) => tag.as_i64().map(|n| Int(n as i32)),
        };
        if let Some(value) = value {
            let value = match value {
                Bool(b) if flip => Bool(!b),
                v => v,
            };
            write(|r| r.values[i] = value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_sorted_for_lookup() {
        assert!(DEFAULTS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn rules_parse_check_and_save() {
        assert_eq!(get_text("random_tick_speed").as_deref(), Some("3"));
        assert_eq!(
            get_text("minecraft:random_tick_speed").as_deref(),
            Some("3")
        );
        assert!(set("block_drops", "maybe").is_err());
        assert!(set("noSuchRule", "true").is_err());
        assert!(set("max_snow_accumulation_height", "4").is_ok());
        assert_eq!(int("max_snow_accumulation_height"), 4);
        let nbt = to_nbt();
        assert_eq!(nbt.int("minecraft:max_snow_accumulation_height"), Some(4));
        set("max_snow_accumulation_height", "1").unwrap();
        load_nbt(&nbt);
        assert_eq!(int("max_snow_accumulation_height"), 4);
        set("max_snow_accumulation_height", "1").unwrap();
        assert_eq!(Difficulty::parse("hard"), Some(Difficulty::Hard));
        assert_eq!(Difficulty::parse("0"), Some(Difficulty::Peaceful));
    }

    #[test]
    fn rules_saved_by_1_21_8_carry_over() {
        let old = Compound::new()
            .with("keepInventory", "true")
            .with("disableRaids", "true")
            .with("randomTickSpeed", "5")
            .with("spawnChunkRadius", "2");
        load_nbt(&old);
        assert!(bool("keep_inventory"));
        assert!(!bool("raids"));
        assert_eq!(int("random_tick_speed"), 5);
        for (_, new, _) in RENAMED {
            assert!(index(new).is_some(), "{new}");
        }
    }

    #[test]
    fn every_26_3_rule_is_known() {
        let rules = levitas_registry::game_rule::NAMES;
        assert_eq!(rules.len(), DEFAULTS.len());
        for name in rules {
            assert!(index(name).is_some(), "{name}");
        }
    }
}
