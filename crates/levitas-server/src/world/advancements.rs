//! Advancements, from the 26.3 data and the Minecraft Wiki's "Advancement" and protocol
//! pages: the tree shown on the advancements screen, criteria granted as players do
//! things, toasts and chat announcements, experience rewards, and progress saved in
//! `players/advancements/<uuid>.json` as vanilla saves it.
//!
//! Criteria conditions are matched for the common shapes: item predicates (items, tags,
//! counts), entity types, biomes and dimensions. A condition Levitas does not understand
//! is never met, so nothing is granted by mistake. Where an advancement sits on the
//! screen is worked out here: one column per level of the tree, one row per leaf.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use levitas_nbt::{Compound, Tag};
use levitas_registry::{TagRegistry, ADVANCEMENTS_JSON, DATA_VERSION};
use serde_json::Value;
use tracing::error;

use super::*;
use crate::inventory::write_template;

/// How often inventories and positions are checked against criteria.
const CHECK_TICKS: u64 = 10;

/// Something a player did that may meet criteria.
#[derive(Debug, Clone, Copy)]
pub enum Trigger {
    /// Checked now and then against the whole inventory.
    InventoryChanged,
    /// Checked now and then against where the player is.
    Location,
    /// The player killed an entity of this type.
    KilledEntity(i32),
    ConsumeItem(Stack),
    /// Animals the player fed had a baby of this type.
    BredAnimals(i32),
    /// The player tamed an animal of this type.
    TamedAnimal(i32),
    ChangedDimension {
        from: &'static str,
        to: &'static str,
    },
    EnchantedItem {
        item: Stack,
        levels: i32,
    },
    SleptInBed,
    VillagerTrade,
    CuredZombieVillager,
    HeroOfTheVillage,
    /// The player built a mob from blocks (an iron golem, ...).
    SummonedEntity(i32),
    /// A bucket the player filled, as it is now.
    FilledBucket(Stack),
}

impl Trigger {
    fn name(self) -> &'static str {
        match self {
            Trigger::InventoryChanged => "minecraft:inventory_changed",
            Trigger::Location => "minecraft:location",
            Trigger::KilledEntity(_) => "minecraft:player_killed_entity",
            Trigger::ConsumeItem(_) => "minecraft:consume_item",
            Trigger::BredAnimals(_) => "minecraft:bred_animals",
            Trigger::TamedAnimal(_) => "minecraft:tame_animal",
            Trigger::ChangedDimension { .. } => "minecraft:changed_dimension",
            Trigger::EnchantedItem { .. } => "minecraft:enchanted_item",
            Trigger::SleptInBed => "minecraft:slept_in_bed",
            Trigger::VillagerTrade => "minecraft:villager_trade",
            Trigger::CuredZombieVillager => "minecraft:cured_zombie_villager",
            Trigger::HeroOfTheVillage => "minecraft:hero_of_the_village",
            Trigger::SummonedEntity(_) => "minecraft:summoned_entity",
            Trigger::FilledBucket(_) => "minecraft:filled_bucket",
        }
    }
}

/// The frame drawn around an advancement, in protocol order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameType {
    Task = 0,
    Challenge = 1,
    Goal = 2,
}

struct Display {
    title: Value,
    description: Value,
    icon: Stack,
    frame: FrameType,
    background: Option<String>,
    show_toast: bool,
    announce: bool,
    hidden: bool,
    /// Where it sits on the advancements screen.
    x: f32,
    y: f32,
}

struct Criterion {
    name: String,
    trigger: String,
    conditions: Value,
}

pub struct Advancement {
    pub id: String,
    parent: Option<usize>,
    display: Option<Display>,
    criteria: Vec<Criterion>,
    /// Groups of criterion indices: one of each group must be met.
    requirements: Vec<Vec<usize>>,
    telemetry: bool,
    experience: i32,
}

/// Every advancement, with what links them.
pub struct Tree {
    pub list: Vec<Advancement>,
    by_id: HashMap<String, usize>,
    children: Vec<Vec<usize>>,
    /// Criteria (advancement, criterion) by trigger name.
    by_trigger: HashMap<String, Vec<(usize, usize)>>,
}

/// The advancements, loaded once.
pub fn tree() -> &'static Tree {
    static TREE: OnceLock<Tree> = OnceLock::new();
    TREE.get_or_init(|| Tree::load(ADVANCEMENTS_JSON))
}

fn icon(value: &Value) -> Stack {
    let item = value["id"]
        .as_str()
        .and_then(ItemInfo::id_of)
        .or_else(|| ItemInfo::id_of("minecraft:stone"))
        .unwrap_or(0);
    Stack::new(item, value["count"].as_i64().unwrap_or(1) as i32)
}

impl Tree {
    fn load(json: &str) -> Tree {
        let all: serde_json::Map<String, Value> =
            serde_json::from_str(json).expect("advancements are valid JSON");
        let mut list = Vec::new();
        let mut parents = Vec::new();
        for (id, a) in &all {
            let criteria: Vec<Criterion> = a["criteria"]
                .as_object()
                .map(|c| {
                    c.iter()
                        .map(|(name, c)| Criterion {
                            name: name.clone(),
                            trigger: c["trigger"].as_str().unwrap_or("").to_owned(),
                            conditions: c["conditions"].clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let index_of = |name: &str| criteria.iter().position(|c| c.name == name);
            // Without requirements every criterion is needed.
            let requirements = match a["requirements"].as_array() {
                Some(groups) => groups
                    .iter()
                    .map(|g| {
                        g.as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|n| index_of(n.as_str()?))
                            .collect()
                    })
                    .collect(),
                None => (0..criteria.len()).map(|i| vec![i]).collect(),
            };
            let display = a.get("display").filter(|d| d.is_object()).map(|d| Display {
                title: d["title"].clone(),
                description: d["description"].clone(),
                icon: icon(&d["icon"]),
                frame: match d["frame"].as_str() {
                    Some("challenge") => FrameType::Challenge,
                    Some("goal") => FrameType::Goal,
                    _ => FrameType::Task,
                },
                background: d["background"].as_str().map(str::to_owned),
                show_toast: d["show_toast"].as_bool().unwrap_or(true),
                announce: d["announce_to_chat"].as_bool().unwrap_or(true),
                hidden: d["hidden"].as_bool().unwrap_or(false),
                x: 0.0,
                y: 0.0,
            });
            parents.push(a["parent"].as_str().map(str::to_owned));
            list.push(Advancement {
                id: id.clone(),
                parent: None,
                display,
                criteria,
                requirements,
                telemetry: a["sends_telemetry_event"].as_bool().unwrap_or(false),
                experience: a["rewards"]["experience"].as_i64().unwrap_or(0) as i32,
            });
        }
        let by_id: HashMap<String, usize> = list
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id.clone(), i))
            .collect();
        let mut children = vec![Vec::new(); list.len()];
        for (i, parent) in parents.into_iter().enumerate() {
            list[i].parent = parent.and_then(|p| by_id.get(&p).copied());
            if let Some(p) = list[i].parent {
                children[p].push(i);
            }
        }
        // Children in a stable order: by name.
        for kids in &mut children {
            kids.sort_by(|&a, &b| list[a].id.cmp(&list[b].id));
        }
        let mut by_trigger: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
        for (a, adv) in list.iter().enumerate() {
            for (c, crit) in adv.criteria.iter().enumerate() {
                by_trigger
                    .entry(crit.trigger.clone())
                    .or_default()
                    .push((a, c));
            }
        }
        let mut tree = Tree {
            list,
            by_id,
            children,
            by_trigger,
        };
        let roots: Vec<usize> = (0..tree.list.len())
            .filter(|&i| tree.list[i].parent.is_none())
            .collect();
        for root in roots {
            let mut row = 0.0;
            tree.place(root, 0.0, &mut row);
        }
        tree
    }

    /// Lays out the subtree under `i`: each level one column further right, each leaf on
    /// a row of its own, and a parent level with its first child.
    fn place(&mut self, i: usize, column: f32, row: &mut f32) -> f32 {
        let kids = self.children[i].clone();
        let y = if kids.is_empty() {
            let y = *row;
            *row += 1.0;
            y
        } else {
            let mut first = None;
            for kid in kids {
                let y = self.place(kid, column + 1.0, row);
                first.get_or_insert(y);
            }
            first.unwrap_or(*row)
        };
        if let Some(d) = &mut self.list[i].display {
            d.x = column;
            d.y = y;
        }
        y
    }

    pub fn index(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }
}

/// A player's progress: when each criterion was met, in milliseconds since 1970.
#[derive(Debug, Default, Clone)]
pub struct Progress {
    met: HashMap<(usize, usize), i64>,
    /// Saved entries Levitas does not track (the recipe unlocks, datapack ones):
    /// written back unchanged.
    kept: serde_json::Map<String, Value>,
    /// The advancements this player's client has been sent.
    sent: HashSet<usize>,
}

impl Progress {
    fn has(&self, a: usize, c: usize) -> bool {
        self.met.contains_key(&(a, c))
    }

    pub fn is_done(&self, a: usize) -> bool {
        let adv = &tree().list[a];
        !adv.requirements.is_empty()
            && adv
                .requirements
                .iter()
                .all(|group| group.iter().any(|&c| self.has(a, c)))
    }

    /// Whether the advancements screen shows `a`: once it, its parent or its grandparent
    /// is done (a tab appears with its root); hidden ones only once done.
    fn visible(&self, a: usize) -> bool {
        let t = tree();
        let Some(display) = &t.list[a].display else {
            return false;
        };
        if self.is_done(a) {
            return true;
        }
        if display.hidden {
            return false;
        }
        let Some(parent) = t.list[a].parent else {
            return false;
        };
        self.is_done(parent) || t.list[parent].parent.is_some_and(|g| self.is_done(g))
    }

    /// The advancements to send that the client lacks: the visible ones, each after its
    /// ancestors (the client drops the whole tree when a parent is missing, as when a
    /// breeding advancement is done before its tab's root).
    fn to_send(&self) -> Vec<usize> {
        let t = tree();
        let mut out: Vec<usize> = Vec::new();
        for a in 0..t.list.len() {
            if self.sent.contains(&a) || !self.visible(a) {
                continue;
            }
            let mut chain = vec![a];
            let mut top = a;
            while let Some(parent) = t.list[top].parent {
                if self.sent.contains(&parent) {
                    break;
                }
                chain.push(parent);
                top = parent;
            }
            for x in chain.into_iter().rev() {
                if !out.contains(&x) {
                    out.push(x);
                }
            }
        }
        out
    }

    /// The vanilla saved form.
    pub fn to_json(&self) -> Value {
        let t = tree();
        let mut out = serde_json::Map::new();
        let mut ids: Vec<usize> = self.met.keys().map(|&(a, _)| a).collect();
        ids.sort_unstable();
        ids.dedup();
        for a in ids {
            let adv = &t.list[a];
            let mut criteria = serde_json::Map::new();
            for (c, crit) in adv.criteria.iter().enumerate() {
                if let Some(&at) = self.met.get(&(a, c)) {
                    criteria.insert(crit.name.clone(), Value::String(format_date(at)));
                }
            }
            let mut entry = serde_json::Map::new();
            entry.insert("criteria".into(), Value::Object(criteria));
            entry.insert("done".into(), Value::Bool(self.is_done(a)));
            out.insert(adv.id.clone(), Value::Object(entry));
        }
        for (id, entry) in &self.kept {
            out.entry(id.clone()).or_insert_with(|| entry.clone());
        }
        out.insert("DataVersion".into(), Value::from(DATA_VERSION));
        Value::Object(out)
    }

    pub fn from_json(json: &Value) -> Progress {
        let t = tree();
        let mut progress = Progress::default();
        let Some(all) = json.as_object() else {
            return progress;
        };
        for (id, entry) in all {
            let Some(a) = t.index(id) else {
                if id != "DataVersion" {
                    progress.kept.insert(id.clone(), entry.clone());
                }
                continue;
            };
            for (name, date) in entry["criteria"].as_object().into_iter().flatten() {
                if let Some(c) = t.list[a].criteria.iter().position(|c| &c.name == name) {
                    let at = date
                        .as_str()
                        .and_then(parse_date)
                        .unwrap_or_else(now_millis);
                    progress.met.insert((a, c), at);
                }
            }
        }
        progress
    }
}

fn now_millis() -> i64 {
    std::time::UNIX_EPOCH
        .elapsed()
        .map_or(0, |d| d.as_millis() as i64)
}

/// Days since 1970-01-01 to a calendar date (the proleptic Gregorian calendar).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// A calendar date to days since 1970-01-01.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A time as vanilla writes it in advancement files: `2026-11-01 12:30:00 +0000`.
fn format_date(millis: i64) -> String {
    let secs = millis.div_euclid(1000);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} +0000",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

fn parse_date(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once(' ')?;
    let (time, zone) = rest.split_once(' ').unwrap_or((rest, "+0000"));
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time.split(':').map(|p| p.parse::<i64>());
    let (h, min, s) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let offset = zone
        .trim_start_matches(['+', '-'])
        .parse::<i64>()
        .unwrap_or(0);
    let offset = sign * (offset / 100 * 3600 + offset % 100 * 60);
    let days = days_from_civil(y, m as u32, day as u32);
    Some(((days * 86_400 + h * 3600 + min * 60 + s) - offset) * 1000)
}

/// A JSON text component as the network's NBT form.
fn json_tag(value: &Value) -> Tag {
    match value {
        Value::Object(o) => {
            let mut c = Compound::new();
            for (k, v) in o {
                c.insert(k.as_str(), json_tag(v));
            }
            Tag::Compound(c)
        }
        Value::Array(list) => {
            // NBT lists hold one type; text lists are made compounds throughout.
            Tag::List(
                list.iter()
                    .map(|v| match json_tag(v) {
                        Tag::Compound(c) => Tag::Compound(c),
                        Tag::String(s) => Tag::Compound(Compound::new().with("text", s)),
                        other => Tag::Compound(Compound::new().with("", other)),
                    })
                    .collect(),
            )
        }
        Value::String(s) => Tag::String(s.clone()),
        Value::Bool(b) => Tag::Byte(i8::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Tag::Int(i as i32),
            None => Tag::Double(n.as_f64().unwrap_or(0.0)),
        },
        Value::Null => Tag::String(String::new()),
    }
}

fn text(packet: Writer, value: &Value) -> Writer {
    packet.bytes(&levitas_nbt::to_network(&json_tag(value)))
}

/// One advancement as the update packet carries it.
fn write_advancement(mut packet: Writer, a: usize) -> Writer {
    let t = tree();
    let adv = &t.list[a];
    packet = packet.string(&adv.id);
    packet = match adv.parent {
        Some(p) => packet.bool(true).string(&t.list[p].id),
        None => packet.bool(false),
    };
    packet = match &adv.display {
        None => packet.bool(false),
        Some(d) => {
            let mut packet = text(text(packet.bool(true), &d.title), &d.description);
            // The icon is an item stack template: item, count, components.
            packet = write_template(packet, d.icon).varint(d.frame as i32);
            let flags = i32::from(d.background.is_some())
                | i32::from(d.show_toast) << 1
                | i32::from(d.hidden) << 2;
            packet = packet.i32(flags);
            if let Some(background) = &d.background {
                packet = packet.string(background);
            }
            packet
        }
    };
    packet = packet.varint(adv.requirements.len() as i32);
    for group in &adv.requirements {
        packet = packet.varint(group.len() as i32);
        for &c in group {
            packet = packet.string(&adv.criteria[c].name);
        }
    }
    // Since 26.x the place on the screen ends every advancement, shown or not (seen in
    // a vanilla 26.3 server's packets; the wiki still has it inside the display).
    let (x, y) = adv.display.as_ref().map_or((0.0, 0.0), |d| (d.x, d.y));
    packet.bool(adv.telemetry).f32(x).f32(y)
}

fn write_progress(mut packet: Writer, progress: &Progress, a: usize) -> Writer {
    let adv = &tree().list[a];
    packet = packet.string(&adv.id).varint(adv.criteria.len() as i32);
    for (c, crit) in adv.criteria.iter().enumerate() {
        packet = packet.string(&crit.name);
        packet = match progress.met.get(&(a, c)) {
            Some(&at) => packet.bool(true).i64(at),
            None => packet.bool(false),
        };
    }
    packet
}

/// An advancements update: newly shown advancements, then the progress of `changed`.
fn update_packet(
    reset: bool,
    added: &[usize],
    progress: &Progress,
    changed: &[usize],
    show: bool,
) -> Writer {
    let mut packet = Writer::packet(clientbound::UPDATE_ADVANCEMENTS)
        .bool(reset)
        .varint(added.len() as i32);
    for &a in added {
        packet = write_advancement(packet, a);
    }
    packet = packet.varint(0).varint(changed.len() as i32); // nothing removed
    for &a in changed {
        packet = write_progress(packet, progress, a);
    }
    packet.bool(show)
}

/// Whether `id` names an entity type `entity` matches: an ID, a `#tag`, or a list.
fn matches_entity(spec: &Value, entity: i32) -> bool {
    match spec {
        Value::String(s) => match s.strip_prefix('#') {
            Some(tag) => TagRegistry::get("minecraft:entity_type")
                .is_some_and(|t| t.members(tag).contains(&i64::from(entity))),
            None => entity_type::NAMES.get(entity as usize) == Some(&s.as_str()),
        },
        Value::Array(list) => list.iter().any(|s| matches_entity(s, entity)),
        _ => false,
    }
}

/// Whether an item stack fits an item predicate: `items` (IDs, a tag or a list) and
/// `count`. Other parts are not understood, so they fail.
fn matches_item(predicate: &Value, stack: Stack) -> bool {
    if stack.is_empty() {
        return false;
    }
    let Some(p) = predicate.as_object() else {
        return false;
    };
    p.iter().all(|(key, value)| match key.as_str() {
        "items" => item_in(value, stack.item),
        "count" => in_range(value, i64::from(stack.count)),
        _ => false,
    })
}

fn item_in(spec: &Value, item: i32) -> bool {
    match spec {
        Value::String(s) => match s.strip_prefix('#') {
            Some(tag) => TagRegistry::get("minecraft:item")
                .is_some_and(|t| t.members(tag).contains(&i64::from(item))),
            None => ItemInfo::id_of(s) == Some(item),
        },
        Value::Array(list) => list.iter().any(|s| item_in(s, item)),
        _ => false,
    }
}

/// A number or a `{min, max}` range.
fn in_range(range: &Value, value: i64) -> bool {
    match range {
        Value::Number(n) => n.as_i64() == Some(value),
        Value::Object(o) => {
            o.get("min")
                .and_then(Value::as_i64)
                .is_none_or(|m| value >= m)
                && o.get("max")
                    .and_then(Value::as_i64)
                    .is_none_or(|m| value <= m)
        }
        _ => false,
    }
}

/// Where a player is, for location predicates.
struct Place {
    biome: Option<String>,
    dimension: &'static str,
}

/// An entity predicate: its type and, for the player, its location.
fn matches_entity_predicate(predicate: &Value, entity: i32, place: Option<&Place>) -> bool {
    let Some(p) = predicate.as_object() else {
        return false;
    };
    p.iter().all(|(key, value)| match key.as_str() {
        "minecraft:entity_type" | "type" => matches_entity(value, entity),
        "minecraft:location" | "location" => place.is_some_and(|at| matches_location(value, at)),
        _ => false,
    })
}

fn matches_location(predicate: &Value, at: &Place) -> bool {
    let Some(p) = predicate.as_object() else {
        return false;
    };
    p.iter().all(|(key, value)| match key.as_str() {
        "biomes" | "biome" => at
            .biome
            .as_deref()
            .is_some_and(|biome| biome_in(value, biome)),
        "dimension" => value.as_str() == Some(at.dimension),
        _ => false,
    })
}

fn biome_in(spec: &Value, biome: &str) -> bool {
    match spec {
        Value::String(s) => match s.strip_prefix('#') {
            Some(tag) => {
                let id =
                    SyncedRegistry::get("minecraft:worldgen/biome").and_then(|r| r.id_of(biome));
                TagRegistry::get("minecraft:worldgen/biome")
                    .zip(id)
                    .is_some_and(|(t, id)| t.members(tag).contains(&i64::from(id)))
            }
            None => s == biome,
        },
        Value::Array(list) => list.iter().any(|s| biome_in(s, biome)),
        _ => false,
    }
}

/// An entity condition: one `entity_properties` check, a list of them, or a bare
/// predicate. Other kinds of condition fail.
fn matches_entity_condition(condition: &Value, entity: i32, place: Option<&Place>) -> bool {
    match condition {
        Value::Array(list) => list
            .iter()
            .all(|c| matches_entity_condition(c, entity, place)),
        Value::Object(o) => match o.get("type").and_then(Value::as_str) {
            Some("minecraft:entity_properties") => {
                o.get("entity")
                    .and_then(Value::as_str)
                    .is_none_or(|e| e == "this")
                    && matches_entity_predicate(&o["predicate"], entity, place)
            }
            Some(_) if o.len() > 1 && !o.contains_key("predicate") => {
                // A bare predicate may name the entity `type`.
                matches_entity_predicate(condition, entity, place)
            }
            Some(_) => false,
            None => matches_entity_predicate(condition, entity, place),
        },
        _ => false,
    }
}

impl World {
    fn place_of(&self, id: i32) -> Option<Place> {
        let p = self.players.get(&id)?;
        let [x, y, z] = p.pos.map(|v| v.floor() as i32);
        Some(Place {
            biome: self.biome_at(x, y, z),
            dimension: self.dimension.name(),
        })
    }

    /// Whether a criterion's conditions are met by what just happened.
    fn criterion_met(&self, id: i32, conditions: &Value, trigger: Trigger) -> bool {
        let Some(conditions) = conditions.as_object() else {
            return true;
        };
        let player_type = entity_type::PLAYER;
        conditions
            .iter()
            .all(|(key, value)| match (key.as_str(), trigger) {
                ("player", _) => {
                    let place = self.place_of(id);
                    matches_entity_condition(value, player_type, place.as_ref())
                }
                ("entity", Trigger::KilledEntity(kind))
                | ("entity", Trigger::SummonedEntity(kind))
                | ("entity", Trigger::TamedAnimal(kind))
                | ("child" | "parent" | "partner", Trigger::BredAnimals(kind)) => {
                    matches_entity_condition(value, kind, None)
                }
                ("item", Trigger::ConsumeItem(stack))
                | ("item", Trigger::EnchantedItem { item: stack, .. })
                | ("item", Trigger::FilledBucket(stack)) => matches_item(value, stack),
                ("levels", Trigger::EnchantedItem { levels, .. }) => {
                    in_range(value, i64::from(levels))
                }
                ("items", Trigger::InventoryChanged) => {
                    let Some(p) = self.players.get(&id) else {
                        return false;
                    };
                    value.as_array().is_some_and(|predicates| {
                        predicates
                            .iter()
                            .all(|pr| p.inventory.slots.iter().any(|&s| matches_item(pr, s)))
                    })
                }
                ("from", Trigger::ChangedDimension { from, .. }) => value.as_str() == Some(from),
                ("to", Trigger::ChangedDimension { to, .. }) => value.as_str() == Some(to),
                _ => false,
            })
    }

    /// Grants whatever criteria `trigger` meets for player `id`, then tells them.
    pub(super) fn advance(&mut self, id: i32, trigger: Trigger) {
        let t = tree();
        let Some(candidates) = t.by_trigger.get(trigger.name()) else {
            return;
        };
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let open: Vec<(usize, usize)> = candidates
            .iter()
            .copied()
            .filter(|&(a, c)| !p.advancements.has(a, c))
            .collect();
        let met: Vec<(usize, usize)> = open
            .into_iter()
            .filter(|&(a, c)| self.criterion_met(id, &t.list[a].criteria[c].conditions, trigger))
            .collect();
        if !met.is_empty() {
            self.grant(id, &met);
        }
    }

    /// Marks criteria met, rewards and announces what that completes, and sends the
    /// changes (and anything they reveal) to the player.
    pub(super) fn grant(&mut self, id: i32, criteria: &[(usize, usize)]) {
        let t = tree();
        let now = now_millis();
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let was_done: HashSet<usize> = criteria
            .iter()
            .map(|&(a, _)| a)
            .filter(|&a| p.advancements.is_done(a))
            .collect();
        for &key in criteria {
            p.advancements.met.entry(key).or_insert(now);
        }
        let mut changed: Vec<usize> = criteria.iter().map(|&(a, _)| a).collect();
        changed.sort_unstable();
        changed.dedup();
        let completed: Vec<usize> = changed
            .iter()
            .copied()
            .filter(|&a| !was_done.contains(&a) && p.advancements.is_done(a))
            .collect();
        let name = p.profile.name.clone();
        for &a in &completed {
            let adv = &t.list[a];
            if adv.experience > 0 {
                self.give_xp(id, adv.experience);
            }
            if let Some(d) = adv.display.as_ref().filter(|d| d.announce) {
                if crate::rules::bool("show_advancement_messages") {
                    self.broadcast(announcement(&name, d), None);
                }
            }
        }
        self.sync_advancements(id, &changed, false);
    }

    /// Sends the player the advancements they can now see that they had not been sent,
    /// and the progress of `changed` (of everything visible when `reset`).
    pub(super) fn sync_advancements(&mut self, id: i32, changed: &[usize], reset: bool) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if reset {
            p.advancements.sent.clear();
        }
        let progress = &p.advancements;
        let added = progress.to_send();
        let mut update: Vec<usize> = changed
            .iter()
            .copied()
            .filter(|a| progress.sent.contains(a) || added.contains(a))
            .collect();
        for &a in &added {
            if !update.contains(&a) {
                update.push(a);
            }
        }
        if !reset && added.is_empty() && update.is_empty() {
            return;
        }
        // A vanilla 26.3 server sets the last flag on every update, the first included.
        let packet = update_packet(reset, &added, progress, &update, true);
        p.advancements.sent.extend(added);
        self.send(id, packet);
    }

    /// Checks inventories and positions against their criteria now and then.
    pub(super) fn tick_advancements(&mut self) {
        if !self.tick.is_multiple_of(CHECK_TICKS) {
            return;
        }
        let ids: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.advance(id, Trigger::InventoryChanged);
            self.advance(id, Trigger::Location);
        }
    }

    fn advancements_path(&self, uuid: u128) -> PathBuf {
        self.dir
            .join("players")
            .join("advancements")
            .join(format!("{}.json", storage::uuid_string(uuid)))
    }

    /// Reads a joining player's progress.
    pub(super) fn load_advancements(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let path = self.advancements_path(p.profile.uuid);
        let progress = super::storage::read_json(&path)
            .map(|json| Progress::from_json(&json))
            .unwrap_or_default();
        if let Some(p) = self.players.get_mut(&id) {
            p.advancements = progress;
        }
    }

    pub(super) fn save_advancements(&self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let path = self.advancements_path(p.profile.uuid);
        let json = p.advancements.to_json();
        let saved = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| super::storage::write_atomic(&path, json.to_string().as_bytes()));
        if let Err(e) = saved {
            error!("could not save advancements to {}: {e}", path.display());
        }
    }
}

/// "<player> has made the advancement [<title>]", as a chat message.
fn announcement(player: &str, d: &Display) -> Writer {
    let (key, color) = match d.frame {
        FrameType::Task => ("chat.type.advancement.task", "green"),
        FrameType::Goal => ("chat.type.advancement.goal", "green"),
        FrameType::Challenge => ("chat.type.advancement.challenge", "dark_purple"),
    };
    let message = serde_json::json!({
        "translate": key,
        "with": [
            {"text": player},
            {"translate": "chat.square_brackets", "with": [d.title], "color": color},
        ],
    });
    text(Writer::packet(clientbound::SYSTEM_CHAT), &message).bool(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32, mpsc::Receiver<Frame>) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-advancements-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut world, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        world.chunk(0, 0);
        let (out, frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        world
            .players
            .insert(id, Player::new(profile, out, [0.5, -60.0, 0.5]));
        (world, id, frames)
    }

    fn done(w: &World, id: i32, name: &str) -> bool {
        let a = tree().index(name).expect(name);
        w.players[&id].advancements.is_done(a)
    }

    #[test]
    fn saved_entries_levitas_does_not_track_are_kept() {
        let saved = serde_json::json!({
            "minecraft:recipes/misc/charcoal": {"criteria": {"has_log": "2026-01-01 10:00:00 +0000"}, "done": true},
            "DataVersion": 4440
        });
        let out = Progress::from_json(&saved).to_json();
        assert!(out.get("minecraft:recipes/misc/charcoal").is_some());
    }

    #[test]
    fn a_done_advancement_is_sent_after_its_undone_ancestors() {
        let t = tree();
        let child = t.index("minecraft:husbandry/breed_an_animal").unwrap();
        let root = t.index("minecraft:husbandry/root").unwrap();
        let mut progress = Progress::default();
        progress.met.insert((child, 0), 1);
        let send = progress.to_send();
        let (at_root, at_child) = (
            send.iter().position(|&a| a == root),
            send.iter().position(|&a| a == child),
        );
        assert!(at_root.is_some() && at_child.is_some(), "{send:?}");
        assert!(at_root < at_child, "the parent goes first");
        // Once sent, they are not sent again.
        progress.sent.extend(send);
        assert!(progress.to_send().is_empty());
    }

    #[test]
    fn the_tree_loads_with_parents_and_places() {
        let t = tree();
        assert!(t.list.len() > 100);
        let root = t.index("minecraft:story/root").unwrap();
        let stone = t.index("minecraft:story/mine_stone").unwrap();
        assert_eq!(t.list[stone].parent, Some(root));
        let (r, s) = (
            t.list[root].display.as_ref().unwrap(),
            t.list[stone].display.as_ref().unwrap(),
        );
        assert_eq!((r.x, s.x), (0.0, 1.0));
        // No two advancements of a tab share a spot.
        let mut spots = HashSet::new();
        for (i, a) in t.list.iter().enumerate() {
            let Some(d) = &a.display else { continue };
            let mut top = i;
            while let Some(p) = t.list[top].parent {
                top = p;
            }
            assert!(spots.insert((top, d.x as i32, d.y as i32)), "{}", a.id);
        }
    }

    #[test]
    fn crafting_a_table_starts_the_story_and_the_next_steps_appear() {
        let (mut w, id, mut frames) = world_with_player();
        w.sync_advancements(id, &[], true);
        let table = ItemInfo::id_of("minecraft:crafting_table").unwrap();
        w.players.get_mut(&id).unwrap().inventory.slots[HOTBAR] = Stack::new(table, 1);
        w.advance(id, Trigger::InventoryChanged);
        assert!(done(&w, id, "minecraft:story/root"));
        assert!(!done(&w, id, "minecraft:story/mine_stone"));
        // A cobblestone gets the next one.
        let cobble = ItemInfo::id_of("minecraft:cobblestone").unwrap();
        w.players.get_mut(&id).unwrap().inventory.slots[HOTBAR + 1] = Stack::new(cobble, 3);
        w.advance(id, Trigger::InventoryChanged);
        assert!(done(&w, id, "minecraft:story/mine_stone"));
        // The client got the tree, then updates.
        let mut updates = 0;
        while let Ok(frame) = frames.try_recv() {
            let body: &[u8] = &frame;
            let mut r = levitas_protocol::Reader::new(body);
            r.varint().unwrap();
            if r.varint().unwrap() == clientbound::UPDATE_ADVANCEMENTS {
                updates += 1;
            }
        }
        assert!(updates >= 3);
    }

    #[test]
    fn conditions_match_what_they_say_and_nothing_else() {
        let (mut w, id, _frames) = world_with_player();
        let blaze = entity_type::NAMES
            .iter()
            .position(|&n| n == "minecraft:blaze")
            .unwrap() as i32;
        let zombie = entity_type::ZOMBIE;
        w.advance(id, Trigger::KilledEntity(zombie));
        assert!(done(&w, id, "minecraft:adventure/kill_a_mob"));
        let kill_all = tree().index("minecraft:adventure/kill_all_mobs").unwrap();
        let has = |w: &World, name: &str| {
            let c = tree().list[kill_all]
                .criteria
                .iter()
                .position(|c| c.name == name)
                .unwrap();
            w.players[&id].advancements.has(kill_all, c)
        };
        assert!(has(&w, "minecraft:zombie") && !has(&w, "minecraft:blaze"));
        w.advance(id, Trigger::KilledEntity(blaze));
        assert!(has(&w, "minecraft:blaze"));
        // Changing dimension: only to where the criterion says.
        w.advance(
            id,
            Trigger::ChangedDimension {
                from: "minecraft:overworld",
                to: "minecraft:the_end",
            },
        );
        assert!(!done(&w, id, "minecraft:story/enter_the_nether"));
        w.advance(
            id,
            Trigger::ChangedDimension {
                from: "minecraft:overworld",
                to: "minecraft:the_nether",
            },
        );
        assert!(done(&w, id, "minecraft:story/enter_the_nether"));
        // The flat world is plains: one biome for Adventuring Time, not a fortress.
        w.advance(id, Trigger::Location);
        let time = tree()
            .index("minecraft:adventure/adventuring_time")
            .unwrap();
        assert!(w.players[&id]
            .advancements
            .met
            .keys()
            .any(|&(a, _)| a == time));
        assert!(!done(&w, id, "minecraft:nether/find_fortress"));
    }

    #[test]
    fn advancements_are_written_as_a_vanilla_26_3_server_writes_them() {
        // Taken apart the way a vanilla 26.3 server's packet was: the icon is an item
        // stack template, and the place on the screen comes after the telemetry flag.
        let t = tree();
        let a = t.index("minecraft:story/mine_stone").unwrap();
        let frame = write_advancement(Writer::packet(0), a).into_frame();
        let mut r = levitas_protocol::Reader::new(&frame);
        r.varint().unwrap(); // length
        r.varint().unwrap(); // packet ID
        assert_eq!(r.string(64).unwrap(), "minecraft:story/mine_stone");
        assert!(r.bool().unwrap());
        assert_eq!(r.string(64).unwrap(), "minecraft:story/root");
        assert!(r.bool().unwrap(), "shown");
        for _ in 0..2 {
            // Title and description: {translate: ...} compounds.
            assert_eq!(r.u8().unwrap(), 10);
            let mut depth = 1;
            while depth > 0 {
                match r.u8().unwrap() {
                    0 => depth -= 1,
                    8 => {
                        let n = r.u16().unwrap() as usize;
                        r.bytes(n).unwrap();
                        let n = r.u16().unwrap() as usize;
                        r.bytes(n).unwrap();
                    }
                    other => panic!("tag {other}"),
                }
            }
        }
        assert_eq!(
            r.varint().unwrap(),
            ItemInfo::id_of("minecraft:wooden_pickaxe").unwrap()
        );
        assert_eq!((r.varint().unwrap(), r.varint().unwrap()), (1, 0));
        r.varint().unwrap(); // components removed
        assert_eq!(r.varint().unwrap(), 0, "a task");
        assert_eq!(r.i32().unwrap(), 2, "a toast, no background");
        assert_eq!((r.varint().unwrap(), r.varint().unwrap()), (1, 1));
        assert_eq!(r.string(64).unwrap(), "get_stone");
        assert!(r.bool().unwrap(), "telemetry");
        assert_eq!(r.f32().unwrap(), 1.0, "one column right of the root");
        r.f32().unwrap();
        assert!(r.bytes(1).is_err());
    }

    #[test]
    fn progress_is_saved_as_vanilla_writes_it() {
        let (mut w, id, _frames) = world_with_player();
        w.advance(id, Trigger::SleptInBed);
        assert!(done(&w, id, "minecraft:adventure/sleep_in_bed"));
        w.save_advancements(id);
        let path = w.advancements_path(1);
        let saved: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let entry = &saved["minecraft:adventure/sleep_in_bed"];
        assert_eq!(entry["done"], Value::Bool(true));
        let date = entry["criteria"]["slept_in_bed"].as_str().unwrap();
        assert!(date.ends_with(" +0000") && date.len() == 25, "{date}");
        w.players.get_mut(&id).unwrap().advancements = Progress::default();
        w.load_advancements(id);
        assert!(done(&w, id, "minecraft:adventure/sleep_in_bed"));
        assert_eq!(format_date(0), "1970-01-01 00:00:00 +0000");
        assert_eq!(
            parse_date("2026-11-01 12:30:05 +0000"),
            Some(1_793_536_205_000)
        );
        let at = 1_793_536_205_000;
        assert_eq!(parse_date(&format_date(at)), Some(at));
    }
}
