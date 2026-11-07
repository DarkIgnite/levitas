//! Firework stars and rockets, as the Minecraft Wiki describes them.
//!
//! A star is one explosion (shape, colours, fade colours, trail, twinkle); a rocket
//! has a flight duration (1 to 3, its gunpowder) and the explosions of its stars.
//! Stacks are `Copy`, so the contents are interned here and a stack keeps a handle
//! (`Stack::firework`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use levitas_nbt::{Compound, Tag};
use levitas_protocol::Writer;
use levitas_registry::ItemInfo;

/// Explosion shapes, in their protocol order.
pub const SHAPES: [&str; 5] = ["small_ball", "large_ball", "star", "creeper", "burst"];

/// The colour each dye gives a firework (white, orange, ... black).
pub const DYE_COLORS: [i32; 16] = [
    0xF0F0F0, 0xEB8844, 0xC354CD, 0x6689D3, 0xDECF2A, 0x41CD34, 0xD88198, 0x434343, 0xABABAB,
    0x287697, 0x7B2FBE, 0x253192, 0x51301A, 0x3B511A, 0xB3312C, 0x1E1B1B,
];

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Explosion {
    pub shape: u8,
    pub colors: Vec<i32>,
    pub fade: Vec<i32>,
    pub trail: bool,
    pub twinkle: bool,
}

/// A star (one explosion, no flight) or a rocket.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Firework {
    pub flight: u8,
    pub explosions: Vec<Explosion>,
}

struct Store {
    all: Vec<Firework>,
    index: HashMap<Firework, u32>,
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

pub fn intern(f: Firework) -> Option<u32> {
    let mut s = store().lock().expect("the firework store");
    if let Some(&id) = s.index.get(&f) {
        return Some(id);
    }
    if s.all.len() >= 1 << 16 {
        return None;
    }
    let id = s.all.len() as u32;
    s.all.push(f.clone());
    s.index.insert(f, id);
    Some(id)
}

pub fn get(handle: Option<u32>) -> Firework {
    handle
        .and_then(|h| {
            store()
                .lock()
                .expect("the firework store")
                .all
                .get(h as usize)
                .cloned()
        })
        .unwrap_or_default()
}

/// The firework colour of a dye item.
pub fn dye(item: i32) -> Option<i32> {
    let name = ItemInfo::get(item)?
        .name
        .strip_prefix("minecraft:")?
        .strip_suffix("_dye")?;
    crate::banners::color_index(name).map(|c| DYE_COLORS[usize::from(c)])
}

fn explosion_nbt(e: &Explosion) -> Compound {
    Compound::new()
        .with("shape", SHAPES[usize::from(e.shape).min(4)])
        .with("colors", e.colors.clone())
        .with("fade_colors", e.fade.clone())
        .with("has_trail", e.trail)
        .with("has_twinkle", e.twinkle)
}

fn explosion_from(c: &Compound) -> Explosion {
    let ints = |k: &str| c.int_array(k).map(<[i32]>::to_vec).unwrap_or_default();
    Explosion {
        shape: c
            .str("shape")
            .and_then(|s| SHAPES.iter().position(|&x| x == s))
            .unwrap_or(0) as u8,
        colors: ints("colors"),
        fade: ints("fade_colors"),
        trail: c.int("has_trail").unwrap_or(0) != 0,
        twinkle: c.int("has_twinkle").unwrap_or(0) != 0,
    }
}

/// A star's `minecraft:firework_explosion`.
pub fn star_nbt(f: &Firework) -> Compound {
    f.explosions.first().map(explosion_nbt).unwrap_or_default()
}

/// A rocket's `minecraft:fireworks`.
pub fn rocket_nbt(f: &Firework) -> Compound {
    Compound::new()
        .with("flight_duration", f.flight as i8)
        .with(
            "explosions",
            f.explosions
                .iter()
                .map(|e| Tag::Compound(explosion_nbt(e)))
                .collect::<Vec<_>>(),
        )
}

pub fn star_from(c: &Compound) -> Option<u32> {
    intern(Firework {
        flight: 0,
        explosions: vec![explosion_from(c)],
    })
}

pub fn rocket_from(c: &Compound) -> Option<u32> {
    intern(Firework {
        flight: c.int("flight_duration").unwrap_or(1).clamp(0, 127) as u8,
        explosions: c
            .list("explosions")
            .unwrap_or_default()
            .iter()
            .filter_map(Tag::as_compound)
            .map(explosion_from)
            .collect(),
    })
}

fn write_explosion(mut packet: Writer, e: &Explosion) -> Writer {
    packet = packet
        .varint(i32::from(e.shape))
        .varint(e.colors.len() as i32);
    for &c in &e.colors {
        packet = packet.i32(c);
    }
    packet = packet.varint(e.fade.len() as i32);
    for &c in &e.fade {
        packet = packet.i32(c);
    }
    packet.bool(e.trail).bool(e.twinkle)
}

pub fn write_star(packet: Writer, f: &Firework) -> Writer {
    write_explosion(packet, &f.explosions.first().cloned().unwrap_or_default())
}

pub fn write_rocket(mut packet: Writer, f: &Firework) -> Writer {
    packet = packet
        .varint(i32::from(f.flight))
        .varint(f.explosions.len() as i32);
    for e in &f.explosions {
        packet = write_explosion(packet, e);
    }
    packet
}
