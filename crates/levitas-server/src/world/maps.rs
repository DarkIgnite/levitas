//! Filled maps, as the Minecraft Wiki's "Map" and "Map item format" pages describe them.
//!
//! A map covers a square of 128 by 128 pixels, each 2^scale blocks wide, aligned to a
//! grid of such squares. While a player holds it in the dimension it was made in, the
//! terrain within 128 blocks of them (64 in the Nether) is drawn onto it: each pixel is
//! the colour of the highest block that maps can see, shaded by its height against the
//! pixel to the north, and water is shaded by its depth. Maps are saved as
//! `data/minecraft/map_<id>.dat`, their IDs counted in `idcounts.dat` for all dimensions.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};

use levitas_nbt::{Compound, Tag};
use levitas_registry::{map_decoration, DATA_VERSION};
use tracing::error;

use super::*;
use crate::inventory::OFFHAND;

/// Pixels along each side of a map.
pub const SIZE: i32 = 128;
/// The widest a map can be zoomed out.
pub const MAX_SCALE: u8 = 4;
/// How far from a player a map they hold is drawn, in blocks; half that in the Nether.
const DRAW_RADIUS: i32 = 128;
/// The pixel columns are drawn in this many turns, one per tick, to spread the work.
const COLUMN_TURNS: i32 = 8;
/// How often the player markers on held maps are refreshed.
const ICON_TICKS: u64 = 5;
/// The base colour of water on maps.
const WATER: u8 = 12;

/// The next free map ID, shared by every dimension (vanilla's `idcounts.dat`).
static NEXT_ID: AtomicI32 = AtomicI32::new(0);

/// Takes a new map ID.
pub fn allocate_id() -> i32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// The next map ID that will be given out, as `idcounts.dat` saves it.
pub fn next_id() -> i32 {
    NEXT_ID.load(Ordering::Relaxed)
}

/// Makes sure IDs already given out in this world are not given again.
pub fn reserve_ids(next: i32) {
    NEXT_ID.fetch_max(next, Ordering::Relaxed);
}

/// What one map shows.
#[derive(Debug, Clone)]
pub struct MapData {
    pub dimension: &'static str,
    /// The block at the middle of the map.
    pub center: [i32; 2],
    pub scale: u8,
    pub locked: bool,
    /// `SIZE * SIZE` map colours, row by row from the north-west corner.
    pub colors: Vec<u8>,
    /// Pixels changed since they were last sent: west, north, east, south (inclusive).
    changed: Option<[i32; 4]>,
    /// Whether the map changed since it was last saved.
    unsaved: bool,
}

impl MapData {
    /// A blank map of `scale` whose square on the grid holds block (`x`, `z`).
    pub fn new(dimension: &'static str, x: i32, z: i32, scale: u8) -> Self {
        let width = SIZE << scale;
        let center = |v: i32| (v + 64).div_euclid(width) * width + width / 2 - 64;
        MapData {
            dimension,
            center: [center(x), center(z)],
            scale,
            locked: false,
            colors: vec![0; (SIZE * SIZE) as usize],
            changed: None,
            unsaved: true,
        }
    }

    /// Blocks per pixel.
    fn step(&self) -> i32 {
        1 << self.scale
    }

    /// The block column a pixel shows (its north-west block).
    fn block_of(&self, px: i32, pz: i32) -> (i32, i32) {
        let step = self.step();
        let west = self.center[0] - SIZE / 2 * step;
        let north = self.center[1] - SIZE / 2 * step;
        (west + px * step, north + pz * step)
    }

    fn set(&mut self, px: i32, pz: i32, color: u8) {
        let i = (pz * SIZE + px) as usize;
        if self.colors[i] == color {
            return;
        }
        self.colors[i] = color;
        self.unsaved = true;
        self.changed = Some(match self.changed {
            None => [px, pz, px, pz],
            Some([w, n, e, s]) => [w.min(px), n.min(pz), e.max(px), s.max(pz)],
        });
    }

    fn to_nbt(&self) -> Compound {
        Compound::new()
            .with("scale", self.scale as i8)
            .with("dimension", self.dimension)
            .with("trackingPosition", true)
            .with("unlimitedTracking", false)
            .with("locked", self.locked)
            .with("xCenter", self.center[0])
            .with("zCenter", self.center[1])
            .with("banners", Vec::<Tag>::new())
            .with("frames", Vec::<Tag>::new())
            .with(
                "colors",
                self.colors.iter().map(|&c| c as i8).collect::<Vec<i8>>(),
            )
    }

    fn from_nbt(nbt: &Compound) -> Option<Self> {
        let mut colors: Vec<u8> = nbt
            .byte_array("colors")
            .map(|c| c.iter().map(|&b| b as u8).collect())
            .unwrap_or_default();
        colors.resize((SIZE * SIZE) as usize, 0);
        Some(MapData {
            dimension: match nbt.str("dimension")? {
                "minecraft:the_nether" => "minecraft:the_nether",
                "minecraft:the_end" => "minecraft:the_end",
                _ => "minecraft:overworld",
            },
            center: [nbt.int("xCenter")? as i32, nbt.int("zCenter")? as i32],
            scale: (nbt.int("scale").unwrap_or(0) as u8).min(MAX_SCALE),
            locked: nbt.int("locked").unwrap_or(0) != 0,
            colors,
            changed: None,
            unsaved: false,
        })
    }
}

fn map_file(dir: &Path, id: i32) -> PathBuf {
    dir.join("data")
        .join("minecraft")
        .join(format!("map_{id}.dat"))
}

/// A player's marker on a map: its icon type, position in half pixels from the middle,
/// and direction in sixteenths of a turn. `None` if they are off the map.
fn marker(map: &MapData, pos: [f64; 3], yaw: f32) -> Option<(i8, i8, u8)> {
    let step = f64::from(map.step());
    let x = (pos[0] - f64::from(map.center[0])) / step * 2.0;
    let z = (pos[2] - f64::from(map.center[1])) / step * 2.0;
    let edge = f64::from(SIZE);
    if x < -edge || x >= edge || z < -edge || z >= edge {
        return None;
    }
    let direction = ((f64::from(yaw) / 22.5).round() as i32).rem_euclid(16) as u8;
    Some((x.floor() as i8, z.floor() as i8, direction))
}

/// A map data packet: the markers, then the changed rectangle of pixels if any.
fn map_packet(id: i32, map: &MapData, icons: &[(i8, i8, u8)], area: Option<[i32; 4]>) -> Writer {
    let mut packet = Writer::packet(clientbound::MAP_ITEM_DATA)
        .varint(id)
        .i8(map.scale as i8)
        .bool(map.locked)
        .bool(true)
        .varint(icons.len() as i32);
    for &(x, z, direction) in icons {
        packet = packet
            .varint(map_decoration::PLAYER)
            .i8(x)
            .i8(z)
            .i8(direction as i8)
            .bool(false); // no name
    }
    match area {
        None => packet.u8(0),
        Some([west, north, east, south]) => {
            let (columns, rows) = (east - west + 1, south - north + 1);
            let mut data = Vec::with_capacity((columns * rows) as usize);
            for pz in north..=south {
                for px in west..=east {
                    data.push(map.colors[(pz * SIZE + px) as usize]);
                }
            }
            packet
                .u8(columns as u8)
                .u8(rows as u8)
                .u8(west as u8)
                .u8(north as u8)
                .varint(data.len() as i32)
                .bytes(&data)
        }
    }
}

/// The shade a pixel takes from water `depth` blocks deep: the three shades, with
/// checkered steps between them (`odd` picks the checker square).
fn water_shade(depth: i32, odd: bool) -> u8 {
    match depth {
        ..=2 => 2,
        3..=4 => 2 - u8::from(odd),
        5..=6 => 1,
        7..=9 => 1 - u8::from(odd),
        _ => 0,
    }
}

impl World {
    /// A map's data, loaded from disk the first time it is needed here.
    pub(super) fn map_data(&mut self, id: i32) -> Option<&mut MapData> {
        if !self.maps.contains_key(&id) {
            let bytes = fs::read(map_file(&self.dir, id)).ok()?;
            let map = match levitas_nbt::from_gzip(&bytes) {
                Ok((_, root)) => MapData::from_nbt(root.compound("data")?)?,
                Err(e) => {
                    error!("map {id} is unreadable: {e}");
                    return None;
                }
            };
            self.maps.insert(id, map);
        }
        self.maps.get_mut(&id)
    }

    /// Starts a new map of `scale` around block (`x`, `z`) here, returning its ID.
    pub(super) fn create_map(&mut self, x: i32, z: i32, scale: u8) -> i32 {
        let id = allocate_id();
        let map = MapData::new(self.dimension.name(), x, z, scale.min(MAX_SCALE));
        self.maps.insert(id, map);
        id
    }

    /// A copy of map `id` under a new ID, locked so it never changes again. `None` if
    /// there is no such map.
    pub(super) fn lock_map(&mut self, id: i32) -> Option<i32> {
        let mut copy = self.map_data(id)?.clone();
        copy.locked = true;
        copy.unsaved = true;
        let new = allocate_id();
        self.maps.insert(new, copy);
        Some(new)
    }

    /// A blank map one step further zoomed out than `id`, covering its area.
    pub(super) fn zoom_map(&mut self, id: i32) -> Option<i32> {
        let map = self.map_data(id)?;
        if map.locked || map.scale >= MAX_SCALE {
            return None;
        }
        let (dimension, [x, z], scale) = (map.dimension, map.center, map.scale + 1);
        let new = allocate_id();
        self.maps.insert(new, MapData::new(dimension, x, z, scale));
        Some(new)
    }

    /// Turns the maps a cartography table marked (locked or zoomed out) in a player's
    /// inventory and cursor into the new maps they stand for. The caller sends the
    /// player their window again.
    pub(super) fn finish_map_posts(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let pending: Vec<(Option<usize>, Stack)> = (0..p.inventory.slots.len())
            .map(|slot| (Some(slot), p.inventory.slots[slot]))
            .chain(std::iter::once((None, p.inventory.carried)))
            .filter(|(_, s)| s.map_post.is_some() && s.map_id.is_some())
            .collect();
        for (slot, mut stack) in pending {
            let old = stack.map_id.expect("filtered");
            let new = match stack.map_post {
                Some(crate::inventory::MAP_LOCK) => self.lock_map(old),
                _ => self.zoom_map(old),
            };
            stack.map_post = None;
            if let Some(new) = new {
                stack.map_id = Some(new);
            }
            let p = self.players.get_mut(&id).expect("player exists");
            match slot {
                Some(slot) => p.inventory.slots[slot] = stack,
                None => p.inventory.carried = stack,
            }
        }
    }

    /// Writes the maps that changed since they were last saved.
    pub(super) fn save_maps(&mut self) {
        let folder = self.dir.join("data").join("minecraft");
        for (&id, map) in &mut self.maps {
            if !map.unsaved {
                continue;
            }
            let root = Compound::new()
                .with("data", map.to_nbt())
                .with("DataVersion", DATA_VERSION);
            let saved = fs::create_dir_all(&folder)
                .and_then(|()| fs::write(map_file(&self.dir, id), levitas_nbt::to_gzip("", &root)));
            match saved {
                Ok(()) => map.unsaved = false,
                Err(e) => error!("could not save map {id}: {e}"),
            }
        }
    }

    /// The colour and height a map gives block column (`x`, `z`): the highest block maps
    /// can see, with how deep the water is if that is water. `None` where the chunk is
    /// not loaded.
    fn map_column(&self, x: i32, z: i32) -> Option<(u8, i32, i32)> {
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
        // Skip the empty sections at the top.
        let sections = chunk.sections();
        let top = sections.iter().rposition(|s| !s.is_empty())?;
        let mut y = MIN_Y + (top as i32 + 1) * 16 - 1;
        while y >= MIN_Y {
            let color = BlockInfo::of(chunk.get(lx, y, lz)).map_color;
            if color == WATER {
                let surface = y;
                while y > MIN_Y && BlockInfo::of(chunk.get(lx, y - 1, lz)).map_color == WATER {
                    y -= 1;
                }
                return Some((WATER, surface, surface - y + 1));
            }
            if color != 0 {
                return Some((color, y, 0));
            }
            y -= 1;
        }
        Some((0, MIN_Y, 0))
    }

    /// Draws one turn's worth of a map's pixels within reach of a player at `pos`.
    fn draw_map(&mut self, id: i32, pos: [f64; 3]) {
        let nether = self.dimension == Dimension::Nether;
        let Some(map) = self.maps.get(&id) else {
            return;
        };
        let step = map.step();
        let radius = (if nether { DRAW_RADIUS / 2 } else { DRAW_RADIUS }) / step;
        let (west, north) = map.block_of(0, 0);
        let px = (pos[0].floor() as i32 - west).div_euclid(step);
        let pz = (pos[2].floor() as i32 - north).div_euclid(step);
        let turn = (self.tick % COLUMN_TURNS as u64) as i32;
        let mut pixels = Vec::new();
        for column in (px - radius).max(0)..(px + radius + 1).min(SIZE) {
            if column.rem_euclid(COLUMN_TURNS) != turn {
                continue;
            }
            let (x, _) = map.block_of(column, 0);
            let first = (pz - radius).max(0);
            let last = (pz + radius).min(SIZE - 1);
            // The pixel to the north decides the first one's shade.
            let (_, z) = map.block_of(column, first);
            let mut north_height = self.map_column(x, z - step).map(|c| c.1);
            for row in first..=last {
                let (dx, dz) = (column - px, row - pz);
                let (_, z) = map.block_of(column, row);
                let here = self.map_column(x, z);
                if dx * dx + dz * dz <= radius * radius {
                    if let Some((color, height, depth)) = here {
                        let shade = if color == 0 {
                            0
                        } else if color == WATER {
                            water_shade(depth, (column + row) % 2 == 1)
                        } else {
                            match north_height.map(|n| height.cmp(&n)) {
                                Some(std::cmp::Ordering::Greater) => 2,
                                Some(std::cmp::Ordering::Less) => 0,
                                _ => 1,
                            }
                        };
                        pixels.push((column, row, color * 4 + shade));
                    }
                }
                north_height = here.map(|c| c.1);
            }
        }
        let Some(map) = self.maps.get_mut(&id) else {
            return;
        };
        for (column, row, color) in pixels {
            map.set(column, row, color);
        }
    }

    /// Draws the maps players hold and sends them what changed and where everyone is.
    pub(super) fn tick_maps(&mut self) {
        let here = self.dimension.name();
        let mut held: HashMap<i32, Vec<i32>> = HashMap::new();
        for (&pid, p) in &self.players {
            if !p.alive {
                continue;
            }
            for slot in [HOTBAR + p.held, OFFHAND] {
                if let Some(id) = p.inventory.slots[slot].map_id {
                    let holders = held.entry(id).or_default();
                    if !holders.contains(&pid) {
                        holders.push(pid);
                    }
                }
            }
        }
        for (id, holders) in held {
            let Some(map) = self.map_data(id) else {
                continue;
            };
            let drawable = map.dimension == here && !map.locked;
            if drawable {
                for &pid in &holders {
                    let pos = self.players[&pid].pos;
                    self.draw_map(id, pos);
                }
            }
            let Some(map) = self.maps.get_mut(&id) else {
                continue;
            };
            let icons: Vec<(i8, i8, u8)> = if map.dimension == here {
                holders
                    .iter()
                    .filter_map(|pid| {
                        let p = &self.players[pid];
                        marker(map, p.pos, p.yaw)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let changed = map.changed.take();
            let refresh = self.tick.is_multiple_of(ICON_TICKS);
            let mut packets = Vec::new();
            for &pid in &holders {
                let p = self.players.get_mut(&pid).expect("holder exists");
                let packet = if p.known_maps.insert(id) {
                    // The first look at a map gets all of it.
                    Some(map_packet(
                        id,
                        map,
                        &icons,
                        Some([0, 0, SIZE - 1, SIZE - 1]),
                    ))
                } else if changed.is_some() || refresh {
                    Some(map_packet(id, map, &icons, changed))
                } else {
                    None
                };
                if let Some(packet) = packet {
                    packets.push((pid, packet));
                }
            }
            for (pid, packet) in packets {
                self.send(pid, packet);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn flat_world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-maps-{}-{}",
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
        for cx in -2..=2 {
            for cz in -2..=2 {
                world.chunk(cx, cz);
            }
        }
        world
    }

    #[test]
    fn maps_sit_on_a_grid() {
        // A zoomed-out level 3 map made at the origin covers -64 to 959 (the wiki's
        // example); a level 0 one covers -64 to 63.
        assert_eq!(
            MapData::new("minecraft:overworld", 0, 0, 3).center,
            [448, 448]
        );
        assert_eq!(MapData::new("minecraft:overworld", 0, 0, 0).center, [0, 0]);
        assert_eq!(
            MapData::new("minecraft:overworld", 70, -70, 0).center,
            [128, -128]
        );
    }

    #[test]
    fn held_maps_draw_the_ground_and_follow_the_player() {
        let mut w = flat_world();
        let (out, mut frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, out, [0.5, -60.0, 0.5]));
        let map_id = w.create_map(0, 0, 0);
        let filled = ItemInfo::id_of("minecraft:filled_map").unwrap();
        let mut stack = Stack::new(filled, 1);
        stack.map_id = Some(map_id);
        w.players.get_mut(&id).unwrap().inventory.slots[HOTBAR] = stack;
        // A stone block one higher than the grass around it, north of the player.
        w.set_block(BlockPos::new(0, -60, -3), block::STONE);
        for _ in 0..COLUMN_TURNS {
            w.tick_maps();
            w.tick += 1;
        }
        let map = &w.maps[&map_id];
        let pixel = |x: i32, z: i32| map.colors[((z + 64) * SIZE + x + 64) as usize];
        // Grass (base colour 1) at its normal shade, the raised stone (11) brighter,
        // and the grass south of it darker.
        assert_eq!(pixel(5, 5), 4 + 1);
        assert_eq!(pixel(0, -3), 11 * 4 + 2);
        assert_eq!(pixel(0, -2), 4);
        // Out of reach stays blank.
        assert_eq!(map.colors.len(), (SIZE * SIZE) as usize);
        // The player was sent the whole map once, with their marker in the middle.
        let mut got_map = false;
        while let Ok(frame) = frames.try_recv() {
            let body: &[u8] = &frame;
            let mut r = levitas_protocol::Reader::new(body);
            r.varint().unwrap();
            if r.varint().unwrap() != clientbound::MAP_ITEM_DATA || got_map {
                continue;
            }
            got_map = true;
            // 26.3's layout: ID, scale, locked, optional icons, then the patch.
            assert_eq!(r.varint().unwrap(), map_id);
            assert_eq!((r.u8().unwrap(), r.bool().unwrap()), (0, false));
            assert!(r.bool().unwrap(), "icons present");
            assert_eq!(r.varint().unwrap(), 1, "the holder's marker");
            assert_eq!(r.varint().unwrap(), map_decoration::PLAYER);
            let (x, z) = (r.u8().unwrap() as i8, r.u8().unwrap() as i8);
            assert_eq!((x, z), (1, 1), "half a block from the middle");
            r.u8().unwrap(); // direction
            assert!(!r.bool().unwrap(), "no name");
            let patch = [
                r.u8().unwrap(),
                r.u8().unwrap(),
                r.u8().unwrap(),
                r.u8().unwrap(),
            ];
            assert_eq!(patch, [128, 128, 0, 0], "the whole map");
            assert_eq!(r.varint().unwrap(), SIZE * SIZE);
            assert_eq!(r.bytes((SIZE * SIZE) as usize).unwrap().len(), 16384);
            assert!(r.bytes(1).is_err(), "nothing after the colours");
        }
        assert!(got_map);
        // Saved and loaded again, it shows the same.
        w.save_maps();
        let colors = w.maps[&map_id].colors.clone();
        w.maps.clear();
        assert_eq!(w.map_data(map_id).unwrap().colors, colors);
    }

    #[test]
    fn locking_and_zooming_make_new_maps() {
        let mut w = flat_world();
        let original = w.create_map(10, 10, 0);
        let locked = w.lock_map(original).unwrap();
        assert_ne!(locked, original);
        assert!(w.maps[&locked].locked);
        assert!(w.zoom_map(locked).is_none(), "locked maps do not zoom");
        let zoomed = w.zoom_map(original).unwrap();
        assert_eq!(w.maps[&zoomed].scale, 1);
        assert_eq!(w.maps[&zoomed].center, [64, 64]);
        assert!(water_shade(1, false) > water_shade(12, false));
    }
}
