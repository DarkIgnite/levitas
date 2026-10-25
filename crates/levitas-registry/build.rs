//! Generates Rust tables from the data-generator output in `generated/<version>/`.
//! Run `python tools/datagen.py` once to produce it.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

const VERSION: &str = "1.21.8";

/// Data-driven registries the client expects during the configuration phase.
const SYNCED_REGISTRIES: &[&str] = &[
    "banner_pattern",
    "cat_variant",
    "chat_type",
    "chicken_variant",
    "cow_variant",
    "damage_type",
    "dialog",
    "dimension_type",
    "enchantment",
    "frog_variant",
    "instrument",
    "jukebox_song",
    "painting_variant",
    "pig_variant",
    "test_environment",
    "test_instance",
    "trim_material",
    "trim_pattern",
    "wolf_sound_variant",
    "wolf_variant",
    "worldgen/biome",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let data = manifest.join("../../generated").join(VERSION);
    println!("cargo:rerun-if-changed={}", data.display());
    if !data.join("reports").is_dir() {
        panic!(
            "missing {}: run `python tools/datagen.py` first (needs Java 21+)",
            data.display()
        );
    }

    let mut out = String::new();
    packets(&data, &mut out);
    let block_defaults = blocks(&data, &mut out);
    items_and_entities(&data, &block_defaults, &mut out);
    let synced = synced_registries(&data, &mut out);
    tags(&data, &synced, &mut out);

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    block_loot(&data, &out_dir, &mut out);
    fs::write(out_dir.join("generated.rs"), out).unwrap();
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `minecraft:level_chunk_with_light` -> `LEVEL_CHUNK_WITH_LIGHT`
fn const_name(id: &str) -> String {
    let name = id.strip_prefix("minecraft:").unwrap_or(id).to_uppercase();
    let name = name.replace(['/', '.', '-'], "_");
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{name}")
    } else {
        name
    }
}

fn packets(data: &Path, out: &mut String) {
    let packets = read_json(&data.join("reports/packets.json"));
    out.push_str("/// Packet IDs per protocol state and direction.\npub mod packet {\n");
    for (state, directions) in packets.as_object().unwrap() {
        writeln!(out, "pub mod {state} {{").unwrap();
        for (direction, list) in directions.as_object().unwrap() {
            writeln!(out, "pub mod {direction} {{").unwrap();
            for (name, info) in list.as_object().unwrap() {
                let id = info["protocol_id"].as_i64().unwrap();
                writeln!(out, "pub const {}: i32 = {id};", const_name(name)).unwrap();
            }
            out.push_str("}\n");
        }
        out.push_str("}\n");
    }
    out.push_str("}\n\n");
}

/// Writes block constants and the block and block state tables. Returns each block's
/// default state ID by name.
fn blocks(data: &Path, out: &mut String) -> HashMap<String, u64> {
    let blocks = read_json(&data.join("reports/blocks.json"));
    let block_ids = &read_json(&data.join("reports/registries.json"))["minecraft:block"]["entries"];
    let mut defaults = HashMap::new();
    let mut state_count = 0;
    let mut state_block = Vec::new();
    let mut rows = vec![String::new(); block_ids.as_object().unwrap().len()];
    out.push_str("/// Default block state ID of every block.\npub mod block {\n");
    for (name, block) in blocks.as_object().unwrap() {
        let states = block["states"].as_array().unwrap();
        state_count += states.len();
        let block_id = block_ids[name]["protocol_id"].as_u64().unwrap();
        let first = states
            .iter()
            .map(|s| s["id"].as_u64().unwrap())
            .min()
            .unwrap();
        let properties: Vec<(String, Vec<String>)> = block
            .get("properties")
            .and_then(Value::as_object)
            .map(|props| {
                props
                    .iter()
                    .map(|(k, v)| {
                        let values = v.as_array().unwrap();
                        let values = values.iter().map(|x| x.as_str().unwrap().to_owned());
                        (k.clone(), values.collect())
                    })
                    .collect()
            })
            .unwrap_or_default();
        for state in states {
            let id = state["id"].as_u64().unwrap();
            // State IDs count through property values with the last property fastest;
            // BlockInfo::property relies on it.
            let mut index = 0;
            for (prop, values) in &properties {
                let value = state["properties"][prop].as_str().unwrap();
                index = index * values.len() + values.iter().position(|v| v == value).unwrap();
            }
            assert_eq!(id - first, index as u64, "state order of {name}");
            let id = id as usize;
            if state_block.len() <= id {
                state_block.resize(id + 1, 0);
            }
            state_block[id] = block_id;
        }
        let default = states
            .iter()
            .find(|s| s["default"].as_bool() == Some(true))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        writeln!(out, "pub const {}: u16 = {default};", const_name(name)).unwrap();
        defaults.insert(name.clone(), default);
        let props: Vec<String> = properties
            .iter()
            .map(|(k, values)| format!("({k:?}, &{values:?})"))
            .collect();
        rows[block_id as usize] = format!(
            "BlockInfo {{ name: {name:?}, first_state: {first}, default_state: {default}, \
             properties: &[{}] }},",
            props.join(", ")
        );
    }
    out.push_str("}\n\n");
    writeln!(
        out,
        "/// Number of block states; state IDs are `0..BLOCK_STATE_COUNT`.\n\
         pub const BLOCK_STATE_COUNT: usize = {state_count};\n"
    )
    .unwrap();
    writeln!(
        out,
        "/// Block registry ID of each block state, indexed by state ID.\n\
         pub static STATE_BLOCK: &[u16] = &{state_block:?};\n"
    )
    .unwrap();
    writeln!(
        out,
        "/// Every block, indexed by block registry ID.\npub static BLOCKS: &[BlockInfo] = &[\n{}\n];\n",
        rows.join("\n")
    )
    .unwrap();
    defaults
}

/// Entity type IDs and the item table.
fn items_and_entities(data: &Path, block_defaults: &HashMap<String, u64>, out: &mut String) {
    let registries = read_json(&data.join("reports/registries.json"));
    let entries = |name: &str| -> Vec<(String, u64)> {
        let mut list: Vec<_> = registries[name]["entries"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(entry, info)| (entry.clone(), info["protocol_id"].as_u64().unwrap()))
            .collect();
        list.sort_by_key(|(_, id)| *id);
        list
    };

    out.push_str("/// Entity type IDs.\npub mod entity_type {\n");
    for (name, id) in entries("minecraft:entity_type") {
        writeln!(out, "pub const {}: i32 = {id};", const_name(&name)).unwrap();
    }
    out.push_str("}\n\n");

    let block_ids: HashMap<String, i64> = entries("minecraft:block")
        .into_iter()
        .map(|(name, id)| (name, id as i64))
        .collect();
    let block_tags = data.join("data/minecraft/tags/block");
    let resolve_blocks = |spec: &Value| -> Vec<i64> {
        let mut ids = Vec::new();
        let names: Vec<&str> = match spec {
            Value::String(s) => vec![s.as_str()],
            Value::Array(list) => list.iter().map(|v| v.as_str().unwrap()).collect(),
            _ => panic!("unexpected tool rule blocks {spec}"),
        };
        for name in names {
            match name.strip_prefix('#') {
                Some(tag) => resolve_tag(&block_tags, tag, &block_ids, &mut ids, &mut Vec::new()),
                None => ids.extend(block_ids.get(name)),
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    };

    let components = read_json(&data.join("reports/items.json"));
    out.push_str(
        "/// Every item, indexed by item registry ID.\npub static ITEMS: &[ItemInfo] = &[\n",
    );
    for (name, _) in entries("minecraft:item") {
        let c = &components[&name]["components"];
        let max_stack = c["minecraft:max_stack_size"].as_u64().unwrap_or(64);
        // Block items share their block's name; for other items the block is air (0).
        let block = block_defaults.get(&name).copied().unwrap_or(0);
        let food = match c.get("minecraft:food") {
            Some(f) => format!(
                "Some(Food {{ nutrition: {}, saturation: {:?}, can_always_eat: {} }})",
                f["nutrition"].as_i64().unwrap(),
                f["saturation"].as_f64().unwrap() as f32,
                f["can_always_eat"].as_bool().unwrap_or(false)
            ),
            None => "None".to_owned(),
        };
        let mut rules = Vec::new();
        if let Some(tool) = c.get("minecraft:tool") {
            for rule in tool["rules"].as_array().unwrap() {
                let blocks = resolve_blocks(&rule["blocks"]);
                let correct = rule["correct_for_drops"].as_bool();
                rules.push(format!(
                    "ToolRule {{ blocks: &{blocks:?}, correct_for_drops: {correct:?} }}"
                ));
            }
        }
        writeln!(
            out,
            "ItemInfo {{ name: {name:?}, max_stack: {max_stack}, block: {block}, food: {food}, \
             tool_rules: &[{}] }},",
            rules.join(", ")
        )
        .unwrap();
    }
    out.push_str("];\n\n");
}

/// Collects every block loot table into one JSON object keyed by block name.
fn block_loot(data: &Path, out_dir: &Path, out: &mut String) {
    let dir = data.join("data/minecraft/loot_table/blocks");
    let mut tables = serde_json::Map::new();
    let mut names = Vec::new();
    collect_entries(&dir, &dir, &mut names);
    names.sort();
    for name in names {
        let path = dir.join(format!("{}.json", name.strip_prefix("minecraft:").unwrap()));
        tables.insert(name, read_json(&path));
    }
    let json = serde_json::to_string(&Value::Object(tables)).unwrap();
    fs::write(out_dir.join("block_loot.json"), json).unwrap();
    out.push_str(
        "/// Block loot tables as JSON: an object mapping block names to loot tables.\n\
         pub static BLOCK_LOOT_JSON: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/block_loot.json\"));\n\n",
    );
}

/// Collects entry names of each synced registry, sorted so their IDs are stable.
fn synced_registries(data: &Path, out: &mut String) -> BTreeMap<String, Vec<String>> {
    let mut registries = BTreeMap::new();
    for &registry in SYNCED_REGISTRIES {
        let dir = data.join("data/minecraft").join(registry);
        let mut entries = Vec::new();
        collect_entries(&dir, &dir, &mut entries);
        entries.sort();
        registries.insert(format!("minecraft:{registry}"), entries);
    }

    out.push_str(
        "/// Data-driven registries sent during configuration. An entry's network ID is its index.\n\
         pub static SYNCED_REGISTRIES: &[SyncedRegistry] = &[\n",
    );
    for (name, entries) in &registries {
        writeln!(
            out,
            "SyncedRegistry {{ name: {name:?}, entries: &{entries:?} }},"
        )
        .unwrap();
    }
    out.push_str("];\n\n");
    registries
}

fn collect_entries(base: &Path, dir: &Path, entries: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_entries(base, &path, entries);
        } else if path.extension().is_some_and(|e| e == "json") {
            let rel = path.strip_prefix(base).unwrap().with_extension("");
            let rel = rel.to_string_lossy().replace('\\', "/");
            entries.push(format!("minecraft:{rel}"));
        }
    }
}

/// Resolves every tag to numeric IDs, for registries whose IDs the client knows.
fn tags(data: &Path, synced: &BTreeMap<String, Vec<String>>, out: &mut String) {
    let mut ids: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for (name, entries) in synced {
        let map = entries.iter().cloned().zip(0..).collect();
        ids.insert(name.clone(), map);
    }
    let registries = read_json(&data.join("reports/registries.json"));
    for (name, registry) in registries.as_object().unwrap() {
        let map = registry["entries"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(entry, info)| (entry.clone(), info["protocol_id"].as_i64().unwrap()))
            .collect();
        ids.insert(name.clone(), map);
    }

    let tags_dir = data.join("data/minecraft/tags");
    let mut by_registry: BTreeMap<String, BTreeMap<String, Vec<i64>>> = BTreeMap::new();
    for (registry, entry_ids) in &ids {
        let dir = tags_dir.join(registry.strip_prefix("minecraft:").unwrap());
        if !dir.is_dir() {
            continue;
        }
        let mut names = Vec::new();
        collect_entries(&dir, &dir, &mut names);
        let mut resolved = BTreeMap::new();
        for name in names {
            let mut values = Vec::new();
            resolve_tag(&dir, &name, entry_ids, &mut values, &mut Vec::new());
            values.sort_unstable();
            values.dedup();
            resolved.insert(name, values);
        }
        by_registry.insert(registry.clone(), resolved);
    }

    out.push_str("/// Tags sent during configuration, resolved to network IDs.\npub static TAGS: &[TagRegistry] = &[\n");
    for (registry, tags) in &by_registry {
        writeln!(out, "TagRegistry {{ registry: {registry:?}, tags: &[").unwrap();
        for (name, values) in tags {
            writeln!(out, "({name:?}, &{values:?}),").unwrap();
        }
        out.push_str("] },\n");
    }
    out.push_str("];\n");
}

fn resolve_tag(
    dir: &Path,
    name: &str,
    ids: &HashMap<String, i64>,
    values: &mut Vec<i64>,
    stack: &mut Vec<String>,
) {
    if stack.iter().any(|s| s == name) {
        return;
    }
    let path = dir.join(format!("{}.json", name.strip_prefix("minecraft:").unwrap()));
    if !path.exists() {
        return;
    }
    stack.push(name.to_owned());
    for value in read_json(&path)["values"].as_array().unwrap() {
        // Entries are either "id" or {"id": ..., "required": false}.
        let id = value.as_str().or_else(|| value["id"].as_str()).unwrap();
        if let Some(tag) = id.strip_prefix('#') {
            resolve_tag(dir, tag, ids, values, stack);
        } else if let Some(&n) = ids.get(id) {
            values.push(n);
        }
    }
    stack.pop();
}
