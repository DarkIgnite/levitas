//! Item components Levitas does not handle (from other servers, plugins or newer data),
//! kept so that they are saved back as they were read.
//!
//! Stacks are `Copy`, so the leftover components are interned here and a stack holds a
//! handle (`Stack::extra`). They are only written to disk: clients are never sent what
//! the server cannot encode.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use levitas_nbt::Compound;

/// The components `Stack` reads and writes itself.
pub const KNOWN: &[&str] = &[
    "minecraft:damage",
    "minecraft:enchantments",
    "minecraft:stored_enchantments",
    "minecraft:repair_cost",
    "minecraft:custom_name",
    "minecraft:potion_contents",
    "minecraft:trim",
    "minecraft:dyed_color",
    "minecraft:lodestone_tracker",
    "minecraft:map_id",
    "minecraft:map_post_processing",
    "minecraft:map_decorations",
    "minecraft:suspicious_stew_effects",
    "minecraft:ominous_bottle_amplifier",
    "minecraft:instrument",
    "minecraft:charged_projectiles",
    "minecraft:pot_decorations",
    "minecraft:writable_book_content",
    "minecraft:written_book_content",
    "minecraft:banner_patterns",
    "minecraft:base_color",
    "minecraft:firework_explosion",
    "minecraft:fireworks",
];

struct Store {
    all: Vec<Compound>,
    index: HashMap<Vec<u8>, u32>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(Store {
            all: Vec::new(),
            index: HashMap::new(),
        })
    })
}

/// The components of `components` that Levitas does not handle, interned (`None` if
/// there are none, or the store is full).
pub fn unknown_of(components: &Compound) -> Option<u32> {
    let mut rest = Compound::new();
    for (key, tag) in components.iter() {
        if !KNOWN.contains(&key) {
            rest.insert(key, tag.clone());
        }
    }
    if rest.is_empty() {
        return None;
    }
    let key = levitas_nbt::to_bytes("", &rest);
    let mut s = store().lock().expect("the component store");
    if let Some(&id) = s.index.get(&key) {
        return Some(id);
    }
    if s.all.len() >= 1 << 16 {
        return None;
    }
    let id = s.all.len() as u32;
    s.all.push(rest);
    s.index.insert(key, id);
    Some(id)
}

/// Adds the kept components back into a stack's saved components, under those the
/// stack wrote itself.
pub fn merge_into(handle: u32, components: &mut Compound) {
    let s = store().lock().expect("the component store");
    let Some(kept) = s.all.get(handle as usize) else {
        return;
    };
    for (key, tag) in kept.iter() {
        if components.get(key).is_none() {
            components.insert(key, tag.clone());
        }
    }
}
