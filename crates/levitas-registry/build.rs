//! Generates Rust tables from the data-generator output in `generated/<version>/`.
//! Run `python tools/datagen.py` once to produce it.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

const VERSION: &str = "26.3";

/// Data-driven registries the client expects during the configuration phase.
const SYNCED_REGISTRIES: &[&str] = &[
    "banner_pattern",
    "block_transformer",
    "cat_sound_variant",
    "cat_variant",
    "chat_type",
    "chicken_sound_variant",
    "chicken_variant",
    "cow_sound_variant",
    "cow_variant",
    "damage_type",
    "decorated_pot_pattern",
    "dialog",
    "dimension_type",
    "enchantment",
    "frog_variant",
    "instrument",
    "jukebox_song",
    "painting_variant",
    "pig_sound_variant",
    "pig_variant",
    "sulfur_cube_archetype",
    "test_environment",
    "test_instance",
    "timeline",
    "trim_material",
    "trim_pattern",
    "wolf_sound_variant",
    "wolf_variant",
    "world_clock",
    "worldgen/biome",
    "worldgen/block_state_provider",
    "zombie_nautilus_variant",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let data = manifest.join("../../generated").join(VERSION);
    println!("cargo:rerun-if-changed={}", data.display());
    if !data.join("version.json").is_file() {
        panic!(
            "missing {}: run `python tools/datagen.py` first (needs Java 21+)",
            data.display()
        );
    }

    let mut out = String::new();
    let version = read_json(&data.join("version.json"));
    writeln!(
        out,
        "/// Data version of 1.21.8 worlds, stored in chunks, level.dat and player data.\n\
         pub const DATA_VERSION: i32 = {};\n",
        version["world_version"].as_i64().unwrap()
    )
    .unwrap();
    packets(&data, &mut out);
    let block_defaults = blocks(&data, &mut out);
    let mut by_name: Vec<(&String, u64)> = block_defaults.iter().map(|(n, &d)| (n, d)).collect();
    by_name.sort();
    let by_name: Vec<String> = by_name
        .iter()
        .map(|(name, default)| format!("({name:?}, {default})"))
        .collect();
    writeln!(
        out,
        "/// Block names with their default state, sorted by name.\n\
         pub static BLOCKS_BY_NAME: &[(&str, u16)] = &[{}];\n",
        by_name.join(", ")
    )
    .unwrap();
    items_and_entities(&data, &block_defaults, &mut out);
    let synced = synced_registries(&data, &mut out);
    tags(&data, &synced, &mut out);

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    embed_json_dir(
        &data,
        &out_dir,
        "loot_table/blocks",
        "BLOCK_LOOT_JSON",
        "Block loot tables as JSON: an object mapping block names to loot tables.",
        &mut out,
    );
    embed_json_dir(
        &data,
        &out_dir,
        "loot_table/entities",
        "ENTITY_LOOT_JSON",
        "Entity loot tables as JSON: an object mapping `minecraft:<entity>` to loot tables.",
        &mut out,
    );
    embed_json_dir(
        &data,
        &out_dir,
        "loot_table/chests",
        "CHEST_LOOT_JSON",
        "Chest loot tables as JSON: an object mapping `minecraft:<path under chests/>` to loot tables.",
        &mut out,
    );
    embed_json_dir(
        &data,
        &out_dir,
        "predicate",
        "PREDICATES_JSON",
        "Named loot predicates as JSON (`minecraft:tool/can_silk_touch`, ...).",
        &mut out,
    );
    embed_json_dir(
        &data,
        &out_dir,
        "painting_variant",
        "PAINTING_VARIANTS_JSON",
        "Painting variants as JSON: their size and picture.",
        &mut out,
    );
    embed_json_dir(
        &data,
        &out_dir,
        "worldgen/biome",
        "BIOMES_JSON",
        "Biome definitions as JSON (spawners, spawn probabilities, climate, ...).",
        &mut out,
    );
    embed_json_dir(
        &data,
        &out_dir,
        "enchantment",
        "ENCHANTMENTS_JSON",
        "Enchantment definitions as JSON: costs, weights, supported items, effects.",
        &mut out,
    );
    embed_json_dir(
        &data,
        &out_dir,
        "recipe",
        "RECIPES_JSON",
        "Recipes as JSON: an object mapping recipe names to recipes.",
        &mut out,
    );
    fs::write(out_dir.join("generated.rs"), out).unwrap();
}

/// Items that place a block of another name: seeds, crops and a few others.
const PLACED_UNDER_ANOTHER_NAME: &[(&str, &str)] = &[
    ("minecraft:wheat_seeds", "minecraft:wheat"),
    ("minecraft:beetroot_seeds", "minecraft:beetroots"),
    ("minecraft:carrot", "minecraft:carrots"),
    ("minecraft:potato", "minecraft:potatoes"),
    ("minecraft:melon_seeds", "minecraft:melon_stem"),
    ("minecraft:pumpkin_seeds", "minecraft:pumpkin_stem"),
    ("minecraft:torchflower_seeds", "minecraft:torchflower_crop"),
    ("minecraft:pitcher_pod", "minecraft:pitcher_crop"),
    ("minecraft:sweet_berries", "minecraft:sweet_berry_bush"),
    ("minecraft:cocoa_beans", "minecraft:cocoa"),
    ("minecraft:glow_berries", "minecraft:cave_vines"),
    ("minecraft:redstone", "minecraft:redstone_wire"),
    ("minecraft:string", "minecraft:tripwire"),
];

/// For a block minecraft-data has no row for yet (it trails the game: 26.1's tables serve
/// 26.3), the older blocks to take its hardness, light and tool needs from, and its
/// collision shapes from (one with the very same properties): `(material, shape)`.
fn stand_in(name: &str) -> (String, String) {
    let short = name.strip_prefix("minecraft:").unwrap_or(name);
    // The poplar wood set is built like oak's.
    if short.contains("poplar") {
        let oak = if short.ends_with("_poplar_leaves") {
            "oak_leaves".to_owned()
        } else {
            short.replace("poplar", "oak")
        };
        return (oak.clone(), oak);
    }
    for (suffix, shape) in [
        ("_stairs", "oak_stairs"),
        ("_slab", "oak_slab"),
        ("_wall", "cobblestone_wall"),
    ] {
        if let Some(base) = short.strip_suffix(suffix) {
            // Wool and concrete stairs and slabs are as soft or hard as their block;
            // the sulfur and cinnabar families are stone-like.
            let material = if base.ends_with("_wool") || base.ends_with("_concrete") {
                base.to_owned()
            } else {
                "stone".to_owned()
            };
            return (material, shape.to_owned());
        }
    }
    let same = |b: &str| (b.to_owned(), b.to_owned());
    match short {
        "sulfur_spike" => same("pointed_dripstone"),
        "red_shrub" => same("dead_bush"),
        "shelf_mushroom" => same("brown_mushroom"),
        "straw_bed" => same("white_bed"),
        _ => same("stone"),
    }
}

/// Every item's default components, by item name. 26.x reports them one file per item
/// under `reports/minecraft/components/item/`; older versions in one `items.json`.
fn item_components(data: &Path) -> Value {
    let one_file = data.join("reports/items.json");
    if one_file.exists() {
        return read_json(&one_file);
    }
    let dir = data.join("reports/minecraft/components/item");
    let mut all = serde_json::Map::new();
    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            let name = path.file_stem().unwrap().to_string_lossy();
            all.insert(format!("minecraft:{name}"), read_json(&path));
        }
    }
    Value::Object(all)
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `minecraft:level_chunk_with_light` -> `LEVEL_CHUNK_WITH_LIGHT`
fn const_name(id: &str) -> String {
    let name = id.strip_prefix("minecraft:").unwrap_or(id).to_uppercase();
    let name = name.replace(['/', '.', '-', ':'], "_");
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
    // Hardness, resistance and light come from minecraft-data (see tools/datagen.py).
    let extra: HashMap<String, Value> = read_json(&data.join("minecraft-data/blocks.json"))
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                format!("minecraft:{}", b["name"].as_str().unwrap()),
                b.clone(),
            )
        })
        .collect();
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
        let e = extra
            .get(name)
            .or_else(|| extra.get(&format!("minecraft:{}", stand_in(name).0)))
            .unwrap_or_else(|| panic!("no hardness for {name}, and no stand-in"));
        rows[block_id as usize] = format!(
            "BlockInfo {{ name: {name:?}, first_state: {first}, default_state: {default}, \
             properties: &[{}], hardness: {:?}, resistance: {:?}, light_emission: {}, \
             light_filter: {}, requires_tool: {} }},",
            props.join(", "),
            e["hardness"].as_f64().unwrap_or(-1.0) as f32,
            e["resistance"].as_f64().unwrap_or(0.0) as f32,
            e["emitLight"].as_u64().unwrap_or(0),
            e["filterLight"].as_u64().unwrap_or(0),
            // Blocks listing harvest tools only drop with a correct tool.
            e.get("harvestTools")
                .and_then(Value::as_object)
                .is_some_and(|t| !t.is_empty()),
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
    shapes(data, &blocks, state_count, out);
    defaults
}

/// Collision boxes of every block state, and which faces of each shape are fully covered.
fn shapes(data: &Path, blocks: &Value, state_count: usize, out: &mut String) {
    let collision = read_json(&data.join("minecraft-data/blockCollisionShapes.json"));
    let mut state_shape = vec![0u64; state_count];
    for (name, block) in blocks.as_object().unwrap() {
        let short = name.strip_prefix("minecraft:").unwrap();
        let states = block["states"].as_array().unwrap();
        let first = states
            .iter()
            .map(|s| s["id"].as_u64().unwrap())
            .min()
            .unwrap();
        for state in states {
            let id = state["id"].as_u64().unwrap();
            let source = if collision["blocks"].get(short).is_some() {
                short.to_owned()
            } else {
                stand_in(name).1
            };
            state_shape[id as usize] = match &collision["blocks"][&source] {
                Value::Array(per_state) => per_state
                    .get((id - first) as usize)
                    .or(per_state.first())
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| panic!("no shape for {name}")),
                one => one
                    .as_u64()
                    .unwrap_or_else(|| panic!("no shape for {name} (stand-in {source})")),
            };
        }
    }
    let count = collision["shapes"].as_object().unwrap().len();
    let mut shapes = Vec::with_capacity(count);
    let mut sturdy = Vec::with_capacity(count);
    for i in 0..count {
        let boxes: Vec<[f32; 6]> = collision["shapes"][i.to_string()]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                let v: Vec<f32> = b
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_f64().unwrap() as f32)
                    .collect();
                [v[0], v[1], v[2], v[3], v[4], v[5]]
            })
            .collect();
        sturdy.push(sturdy_faces(&boxes));
        shapes.push(format!("&{boxes:?}"));
    }
    writeln!(
        out,
        "/// Collision shapes: lists of boxes `[min_x, min_y, min_z, max_x, max_y, max_z]`.\n\
         pub static SHAPES: &[&[[f32; 6]]] = &[{}];\n\
         /// Shape index of every block state.\npub static STATE_SHAPE: &[u16] = &{state_shape:?};\n\
         /// Per shape, one bit per face (down, up, north, south, west, east) that the shape\n\
         /// covers completely, so things can be attached to it.\n\
         pub static SHAPE_STURDY: &[u8] = &{sturdy:?};\n",
        shapes.join(", ")
    )
    .unwrap();
}

/// Which faces of the unit cube a set of boxes covers completely, sampled on the 1/16 grid.
fn sturdy_faces(boxes: &[[f32; 6]]) -> u8 {
    // Per face (in protocol order): the axis it is perpendicular to, whether it is the
    // maximum side, and the two axes spanning it.
    const FACES: [(usize, bool, usize, usize); 6] = [
        (1, false, 0, 2),
        (1, true, 0, 2),
        (2, false, 0, 1),
        (2, true, 0, 1),
        (0, false, 2, 1),
        (0, true, 2, 1),
    ];
    let mut mask = 0;
    for (bit, &(axis, max_side, u, v)) in FACES.iter().enumerate() {
        let covered = (0..16).all(|i| {
            (0..16).all(|j| {
                let (pu, pv) = ((i as f32 + 0.5) / 16.0, (j as f32 + 0.5) / 16.0);
                boxes.iter().any(|b| {
                    let on_face = if max_side {
                        b[axis + 3] >= 1.0
                    } else {
                        b[axis] <= 0.0
                    };
                    on_face && b[u] <= pu && pu <= b[u + 3] && b[v] <= pv && pv <= b[v + 3]
                })
            })
        });
        if covered {
            mask |= 1 << bit;
        }
    }
    mask
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

    let mut sounds: Vec<(String, u64)> = entries("minecraft:sound_event");
    sounds.sort();
    let sounds: Vec<String> = sounds
        .iter()
        .map(|(n, id)| format!("({n:?}, {id})"))
        .collect();
    writeln!(
        out,
        "/// Sound event names with their IDs, sorted by name.\npub static SOUND_EVENTS: &[(&str, i32)] = &[{}];\n",
        sounds.join(", ")
    )
    .unwrap();

    for (module, registry, doc) in [
        ("entity_type", "minecraft:entity_type", "Entity type IDs."),
        (
            "menu",
            "minecraft:menu",
            "Menu (container screen) type IDs.",
        ),
        (
            "block_entity_type",
            "minecraft:block_entity_type",
            "Block entity type IDs.",
        ),
        (
            "component",
            "minecraft:data_component_type",
            "Item data component type IDs.",
        ),
        ("particle", "minecraft:particle_type", "Particle type IDs."),
        ("mob_effect", "minecraft:mob_effect", "Status effect IDs."),
        ("potion", "minecraft:potion", "Potion IDs."),
        (
            "villager_profession",
            "minecraft:villager_profession",
            "Villager profession IDs.",
        ),
        (
            "villager_type",
            "minecraft:villager_type",
            "Villager type IDs.",
        ),
        (
            "command_argument",
            "minecraft:command_argument_type",
            "Command argument parser IDs (`BRIGADIER_INTEGER`, `ITEM_STACK`, ...).",
        ),
    ] {
        writeln!(out, "/// {doc}\npub mod {module} {{").unwrap();
        let list = entries(registry);
        for (name, id) in &list {
            writeln!(out, "pub const {}: i32 = {id};", const_name(name)).unwrap();
        }
        let names: Vec<String> = list.iter().map(|(n, _)| format!("{n:?}")).collect();
        writeln!(
            out,
            "/// Every name, at the index of its ID.
pub static NAMES: &[&str] = &[{}];",
            names.join(", ")
        )
        .unwrap();
        out.push_str("}\n\n");
    }

    let effect_ids: HashMap<String, u64> = entries("minecraft:mob_effect").into_iter().collect();
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

    let components = item_components(data);
    out.push_str(
        "/// Every item, indexed by item registry ID.\npub static ITEMS: &[ItemInfo] = &[\n",
    );
    for (name, _) in entries("minecraft:item") {
        let c = &components[&name]["components"];
        let max_stack = c["minecraft:max_stack_size"].as_u64().unwrap_or(64);
        // Block items share their block's name, except the few below that place a
        // differently named block; for other items the block is air (0).
        let places = PLACED_UNDER_ANOTHER_NAME
            .iter()
            .find(|(item, _)| *item == name)
            .map_or(name.as_str(), |(_, block)| block);
        let block = block_defaults.get(places).copied().unwrap_or(0);
        let food = match c.get("minecraft:food") {
            Some(f) => format!(
                "Some(Food {{ nutrition: {}, saturation: {:?}, can_always_eat: {} }})",
                f["nutrition"].as_i64().unwrap(),
                f["saturation"].as_f64().unwrap() as f32,
                f["can_always_eat"].as_bool().unwrap_or(false)
            ),
            None => "None".to_owned(),
        };
        let consumable = match c.get("minecraft:consumable") {
            Some(cons) => {
                let ticks = (cons["consume_seconds"].as_f64().unwrap_or(1.6) * 20.0).round() as u32;
                let mut effects = Vec::new();
                for e in cons["on_consume_effects"].as_array().into_iter().flatten() {
                    let effect_list = |v: &Value| -> Vec<String> {
                        v.as_array()
                            .into_iter()
                            .flatten()
                            .map(|x| {
                                format!(
                                    "Effect {{ id: {}, duration: {}, amplifier: {} }}",
                                    effect_ids[x["id"].as_str().unwrap()],
                                    x["duration"].as_i64().unwrap_or(1),
                                    x["amplifier"].as_u64().unwrap_or(0)
                                )
                            })
                            .collect()
                    };
                    match e["type"].as_str().unwrap() {
                        "minecraft:apply_effects" => effects.push(format!(
                            "OnConsume::Apply {{ probability: {:?}f32, effects: &[{}] }}",
                            e["probability"].as_f64().unwrap_or(1.0) as f32,
                            effect_list(&e["effects"]).join(", ")
                        )),
                        "minecraft:clear_all_effects" => effects.push("OnConsume::ClearAll".into()),
                        "minecraft:teleport_randomly" => {
                            effects.push("OnConsume::TeleportRandomly".into())
                        }
                        "minecraft:remove_effects" => {
                            let names: Vec<&str> = match &e["effects"] {
                                Value::String(s) => vec![s.as_str()],
                                Value::Array(l) => l.iter().filter_map(|v| v.as_str()).collect(),
                                _ => Vec::new(),
                            };
                            let ids: Vec<String> = names
                                .iter()
                                .filter_map(|n| effect_ids.get(*n))
                                .map(|id| id.to_string())
                                .collect();
                            effects.push(format!("OnConsume::Remove(&[{}])", ids.join(", ")));
                        }
                        _ => {}
                    }
                }
                let remainder = c["minecraft:use_remainder"]["id"].as_str();
                format!(
                    "Some(Consumable {{ ticks: {ticks}, effects: &[{}], remainder: {remainder:?} }})",
                    effects.join(", ")
                )
            }
            None => "None".to_owned(),
        };
        let enchantability = c["minecraft:enchantable"]["value"].as_u64().unwrap_or(0);
        let repair_with = c["minecraft:repairable"]["items"].as_str();
        let tool = match c.get("minecraft:tool") {
            Some(tool) => {
                let rules: Vec<String> = tool["rules"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|rule| {
                        let blocks = resolve_blocks(&rule["blocks"]);
                        let correct = rule["correct_for_drops"].as_bool();
                        let speed = rule["speed"].as_f64().map(|v| v as f32);
                        format!(
                            "ToolRule {{ blocks: &{blocks:?}, correct_for_drops: {correct:?}, \
                             speed: {speed:?} }}"
                        )
                    })
                    .collect();
                format!(
                    "Some(Tool {{ rules: &[{}], default_speed: {:?}, damage_per_block: {} }})",
                    rules.join(", "),
                    tool["default_mining_speed"].as_f64().unwrap_or(1.0) as f32,
                    tool["damage_per_block"].as_i64().unwrap_or(1),
                )
            }
            None => "None".to_owned(),
        };
        let max_damage = c["minecraft:max_damage"].as_i64().unwrap_or(0);
        let equip_slot = c["minecraft:equippable"]["slot"].as_str();
        let trim_material = c["minecraft:provides_trim_material"].as_str();
        // Main-hand attack modifiers: a player's base damage is 1 and speed 4.
        let modifier_in = |kind: &str, mainhand: bool| {
            c["minecraft:attribute_modifiers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["type"] == kind && (m["slot"] == "mainhand") == mainhand)
                .filter_map(|m| m["amount"].as_f64())
                .sum::<f64>()
        };
        let modifier = |kind: &str| modifier_in(kind, true);
        // Armor counts while worn, in any slot but the hand.
        let armor = modifier_in("minecraft:armor", false) as f32;
        let toughness = modifier_in("minecraft:armor_toughness", false) as f32;
        // Rounded to f32 first so the generated literals carry no false precision.
        let attack_damage = (1.0 + modifier("minecraft:attack_damage")) as f32;
        let attack_speed = (4.0 + modifier("minecraft:attack_speed")) as f32;
        writeln!(
            out,
            "ItemInfo {{ name: {name:?}, max_stack: {max_stack}, block: {block}, food: {food}, \
             consumable: {consumable}, enchantability: {enchantability}, repair_with: {repair_with:?}, \
             tool: {tool}, max_damage: {max_damage}, equip_slot: {equip_slot:?}, \
             attack_damage: {attack_damage:?}f32, attack_speed: {attack_speed:?}f32, \
             armor: {armor:?}f32, toughness: {toughness:?}f32, \
             trim_material: {trim_material:?} }},"
        )
        .unwrap();
    }
    out.push_str("];\n\n");
}

/// Collects every JSON file under `data/minecraft/<dir>` into one JSON object keyed by
/// entry name, and exposes it as the string constant `name`.
fn embed_json_dir(data: &Path, out_dir: &Path, dir: &str, name: &str, doc: &str, out: &mut String) {
    let dir = data.join("data/minecraft").join(dir);
    let mut entries = serde_json::Map::new();
    let mut names = Vec::new();
    collect_entries(&dir, &dir, &mut names);
    names.sort();
    for entry in names {
        let path = dir.join(format!(
            "{}.json",
            entry.strip_prefix("minecraft:").unwrap()
        ));
        entries.insert(entry, read_json(&path));
    }
    let file = format!("{}.json", name.to_lowercase());
    let json = serde_json::to_string(&Value::Object(entries)).unwrap();
    fs::write(out_dir.join(&file), json).unwrap();
    writeln!(
        out,
        "/// {doc}\npub static {name}: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/{file}\"));\n"
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
