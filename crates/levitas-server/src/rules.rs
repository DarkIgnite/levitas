//! Game rules and difficulty, shared by every dimension of the world.
//!
//! The rules are vanilla's, by their vanilla names, saved in `level.dat` the way vanilla
//! saves them (each value as text), so a world's rules carry over in both directions.
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

/// Every 1.21.8 game rule with its default, sorted by name.
const DEFAULTS: [(&str, Value); 53] = [
    ("allowFireTicksAwayFromPlayer", Bool(false)),
    ("announceAdvancements", Bool(true)),
    ("blockExplosionDropDecay", Bool(true)),
    ("commandBlockOutput", Bool(true)),
    ("commandModificationBlockLimit", Int(32768)),
    ("disableElytraMovementCheck", Bool(false)),
    ("disablePlayerMovementCheck", Bool(false)),
    ("disableRaids", Bool(false)),
    ("doDaylightCycle", Bool(true)),
    ("doEntityDrops", Bool(true)),
    ("doFireTick", Bool(true)),
    ("doImmediateRespawn", Bool(false)),
    ("doInsomnia", Bool(true)),
    ("doLimitedCrafting", Bool(false)),
    ("doMobLoot", Bool(true)),
    ("doMobSpawning", Bool(true)),
    ("doPatrolSpawning", Bool(true)),
    ("doTileDrops", Bool(true)),
    ("doTraderSpawning", Bool(true)),
    ("doVinesSpread", Bool(true)),
    ("doWardenSpawning", Bool(true)),
    ("doWeatherCycle", Bool(true)),
    ("drowningDamage", Bool(true)),
    ("enderPearlsVanishOnDeath", Bool(true)),
    ("fallDamage", Bool(true)),
    ("fireDamage", Bool(true)),
    ("forgiveDeadPlayers", Bool(true)),
    ("freezeDamage", Bool(true)),
    ("globalSoundEvents", Bool(true)),
    ("keepInventory", Bool(false)),
    ("lavaSourceConversion", Bool(false)),
    ("logAdminCommands", Bool(true)),
    ("maxCommandChainLength", Int(65536)),
    ("maxCommandForkCount", Int(65536)),
    ("maxEntityCramming", Int(24)),
    ("minecartMaxSpeed", Int(8)),
    ("mobExplosionDropDecay", Bool(true)),
    ("mobGriefing", Bool(true)),
    ("naturalRegeneration", Bool(true)),
    ("playersNetherPortalCreativeDelay", Int(0)),
    ("playersNetherPortalDefaultDelay", Int(80)),
    ("playersSleepingPercentage", Int(100)),
    ("projectilesCanBreakBlocks", Bool(true)),
    ("randomTickSpeed", Int(3)),
    ("reducedDebugInfo", Bool(false)),
    ("sendCommandFeedback", Bool(true)),
    ("showDeathMessages", Bool(true)),
    ("snowAccumulationHeight", Int(1)),
    ("spawnChunkRadius", Int(2)),
    ("spawnRadius", Int(10)),
    ("spectatorsGenerateChunks", Bool(true)),
    ("tntExplosionDropDecay", Bool(false)),
    ("universalAnger", Bool(false)),
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
    DEFAULTS.binary_search_by_key(&name, |&(n, _)| n).ok()
}

fn value(name: &str) -> Value {
    let i = index(name).unwrap_or_else(|| panic!("no game rule {name}"));
    read(|r| r.values[i])
}

/// A true-or-false rule, such as `keepInventory`.
pub fn bool(name: &str) -> bool {
    matches!(value(name), Bool(true))
}

/// A number rule, such as `randomTickSpeed`.
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

/// The `GameRules` compound of `level.dat`.
pub fn to_nbt() -> Compound {
    read(|r| {
        let mut nbt = Compound::new();
        for (&(name, _), value) in DEFAULTS.iter().zip(&r.values) {
            nbt.insert(name, value.text());
        }
        nbt
    })
}

/// Takes the rules a world's `level.dat` lists; others keep their defaults.
pub fn load_nbt(nbt: &Compound) {
    for (name, tag) in nbt.iter() {
        if let Some(text) = tag.as_str() {
            // Rules this version does not have, or values it cannot read, are dropped.
            let _ = set(name, text);
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
        assert_eq!(get_text("randomTickSpeed").as_deref(), Some("3"));
        assert!(set("doTileDrops", "maybe").is_err());
        assert!(set("noSuchRule", "true").is_err());
        assert!(set("snowAccumulationHeight", "4").is_ok());
        assert_eq!(int("snowAccumulationHeight"), 4);
        let nbt = to_nbt();
        assert_eq!(nbt.str("snowAccumulationHeight"), Some("4"));
        set("snowAccumulationHeight", "1").unwrap();
        load_nbt(&nbt);
        assert_eq!(int("snowAccumulationHeight"), 4);
        set("snowAccumulationHeight", "1").unwrap();
        assert_eq!(Difficulty::parse("hard"), Some(Difficulty::Hard));
        assert_eq!(Difficulty::parse("0"), Some(Difficulty::Peaceful));
    }
}
