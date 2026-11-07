//! Banner patterns, as banners, shields and the loom keep them.
//!
//! Stacks are small `Copy` values, so a banner's layers live here, interned like
//! custom names: a stack holds the handle of its layers (`Stack::patterns`). Each layer
//! is a pattern from the `minecraft:banner_pattern` registry and a dye colour.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use levitas_nbt::{Compound, Tag};
use levitas_protocol::Writer;
use levitas_registry::{ItemInfo, SyncedRegistry};
use serde_json::Value;

use crate::inventory::Stack;

/// The dye colours in their protocol order.
pub const COLORS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];
/// Most layers the loom puts on a banner.
pub const MAX_LAYERS: usize = 6;
const CAPACITY: usize = 1 << 16;

/// A banner's layers: pattern (registry ID) and colour, bottom first.
pub type Layers = Vec<(u16, u8)>;

struct Store {
    all: Vec<Layers>,
    index: HashMap<Layers, u32>,
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

/// The handle of these layers (`None` for no layers, or when the store is full).
pub fn intern(layers: Layers) -> Option<u32> {
    if layers.is_empty() {
        return None;
    }
    let mut s = store().lock().expect("the pattern store");
    if let Some(&id) = s.index.get(&layers) {
        return Some(id);
    }
    if s.all.len() >= CAPACITY {
        return None;
    }
    let id = s.all.len() as u32;
    s.all.push(layers.clone());
    s.index.insert(layers, id);
    Some(id)
}

pub fn layers(handle: Option<u32>) -> Layers {
    handle
        .and_then(|h| {
            store()
                .lock()
                .expect("the pattern store")
                .all
                .get(h as usize)
                .cloned()
        })
        .unwrap_or_default()
}

fn registry() -> Option<&'static SyncedRegistry> {
    SyncedRegistry::get("minecraft:banner_pattern")
}

pub fn color_index(name: &str) -> Option<u8> {
    COLORS.iter().position(|&c| c == name).map(|i| i as u8)
}

/// The saved form: `[{pattern: "minecraft:creeper", color: "red"}, ...]`.
pub fn to_nbt(handle: Option<u32>) -> Vec<Tag> {
    let Some(reg) = registry() else {
        return Vec::new();
    };
    layers(handle)
        .into_iter()
        .filter_map(|(p, c)| {
            Some(Tag::Compound(
                Compound::new()
                    .with("pattern", *reg.entries.get(usize::from(p))?)
                    .with("color", COLORS[usize::from(c) % 16]),
            ))
        })
        .collect()
}

pub fn from_nbt(list: &[Tag]) -> Option<u32> {
    let reg = registry()?;
    let layers: Layers = list
        .iter()
        .filter_map(Tag::as_compound)
        .filter_map(|l| {
            let pattern = reg.id_of(l.str("pattern")?)? as u16;
            let color = color_index(l.str("color").unwrap_or("white"))?;
            Some((pattern, color))
        })
        .collect();
    intern(layers)
}

/// The component on the network: each layer's pattern (a registry holder) and colour.
pub fn write(mut packet: Writer, handle: Option<u32>) -> Writer {
    let layers = layers(handle);
    packet = packet.varint(layers.len() as i32);
    for (p, c) in layers {
        packet = packet.varint(i32::from(p) + 1).varint(i32::from(c));
    }
    packet
}

/// The colour of a banner item or block (`red_banner`, `red_wall_banner`).
pub fn banner_color(name: &str) -> Option<u8> {
    let n = name.strip_prefix("minecraft:")?;
    let c = n
        .strip_suffix("_wall_banner")
        .or_else(|| n.strip_suffix("_banner"))?;
    color_index(c)
}

/// The colour of a dye item.
pub fn dye_color(item: i32) -> Option<u8> {
    let n = ItemInfo::get(item)?.name.strip_prefix("minecraft:")?;
    color_index(n.strip_suffix("_dye")?)
}

/// The banner pattern tags, in their files' order (the loom lists them that way).
fn pattern_tags() -> &'static HashMap<String, Vec<String>> {
    static TAGS: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    TAGS.get_or_init(|| {
        let all: serde_json::Map<String, Value> =
            serde_json::from_str(levitas_registry::BANNER_PATTERN_TAGS_JSON).unwrap_or_default();
        all.into_iter()
            .map(|(name, t)| {
                let values = t["values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                (name, values)
            })
            .collect()
    })
}

fn tag_members(tag: &str, out: &mut Vec<String>) {
    for v in pattern_tags().get(tag).into_iter().flatten() {
        match v.strip_prefix('#') {
            Some(inner) => tag_members(inner, out),
            None => out.push(v.clone()),
        }
    }
}

/// The patterns the loom offers: with no pattern item those needing none, with one
/// the patterns it provides (`flower_banner_pattern` gives `pattern_item/flower`).
pub fn selectable(pattern_item: Stack) -> Vec<u16> {
    let tag = if pattern_item.is_empty() {
        "minecraft:no_item_required".to_owned()
    } else {
        let Some(name) = ItemInfo::get(pattern_item.item).map(|i| i.name) else {
            return Vec::new();
        };
        let Some(kind) = name
            .strip_prefix("minecraft:")
            .and_then(|n| n.strip_suffix("_banner_pattern"))
        else {
            return Vec::new();
        };
        format!("minecraft:pattern_item/{kind}")
    };
    let mut names = Vec::new();
    tag_members(&tag, &mut names);
    let Some(reg) = registry() else {
        return Vec::new();
    };
    names
        .iter()
        .filter_map(|n| reg.id_of(n).map(|i| i as u16))
        .collect()
}

/// The loom's result: the banner with one more layer of the chosen pattern in the
/// dye's colour, or nothing.
pub fn loom_result(banner: Stack, dye: Stack, pattern_item: Stack, choice: Option<usize>) -> Stack {
    let is_banner = ItemInfo::get(banner.item).is_some_and(|i| banner_color(i.name).is_some());
    let (Some(choice), true, Some(color)) = (choice, is_banner, dye_color(dye.item)) else {
        return Stack::EMPTY;
    };
    let Some(&pattern) = selectable(pattern_item).get(choice) else {
        return Stack::EMPTY;
    };
    let mut l = layers(banner.patterns);
    if l.len() >= MAX_LAYERS {
        return Stack::EMPTY;
    }
    l.push((pattern, color));
    Stack {
        count: 1,
        patterns: intern(l),
        ..banner
    }
}
