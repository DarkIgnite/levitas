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

    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("generated.rs");
    fs::write(dest, out).unwrap();
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

/// Writes default block state IDs and returns them by block name.
fn blocks(data: &Path, out: &mut String) -> HashMap<String, u64> {
    let blocks = read_json(&data.join("reports/blocks.json"));
    let block_ids = &read_json(&data.join("reports/registries.json"))["minecraft:block"]["entries"];
    let mut defaults = HashMap::new();
    let mut state_count = 0;
    let mut state_block = Vec::new();
    out.push_str("/// Default block state ID of every block.\npub mod block {\n");
    for (name, block) in blocks.as_object().unwrap() {
        let states = block["states"].as_array().unwrap();
        state_count += states.len();
        let block_id = block_ids[name]["protocol_id"].as_u64().unwrap();
        for state in states {
            let id = state["id"].as_u64().unwrap() as usize;
            if state_block.len() <= id {
                state_block.resize(id + 1, 0);
            }
            state_block[id] = block_id;
        }
        let default = states
            .iter()
            .find(|s| s["default"].as_bool() == Some(true))
            .unwrap();
        let id = default["id"].as_u64().unwrap();
        writeln!(out, "pub const {}: u16 = {id};", const_name(name)).unwrap();
        defaults.insert(name.clone(), id);
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
    defaults
}

/// Entity type IDs, and for every item the default state of the block it places.
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

    // Block items share their block's name; for other items the block is air (0).
    let items = entries("minecraft:item");
    let item_blocks: Vec<u64> = items
        .iter()
        .map(|(name, _)| block_defaults.get(name).copied().unwrap_or(0))
        .collect();
    writeln!(
        out,
        "/// Default block state placed by each item, indexed by item ID; 0 (air) if the \
         item is not a block.\npub static ITEM_BLOCK: &[u16] = &{item_blocks:?};\n"
    )
    .unwrap();
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
