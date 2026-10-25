//! Keeping sky light and block light right as blocks change.
//!
//! Light spreads from sources one block at a time, losing at least one level per step and
//! more through blocks that filter light (water, leaves). Sky light also falls straight
//! down without loss through fully clear blocks. A change runs in two passes, both
//! breadth-first: first light that may have come through the changed block is removed,
//! then light is spread again from every source next to the cleared region and from the
//! new block itself.

use std::collections::{HashMap, VecDeque};

use levitas_registry::BlockInfo;

use crate::chunk::{Chunk, MAX_Y, MIN_Y};

const DIRECTIONS: [(i32, i32, i32); 6] = [
    (0, -1, 0),
    (0, 1, 0),
    (-1, 0, 0),
    (1, 0, 0),
    (0, 0, -1),
    (0, 0, 1),
];

/// Light a block gives off in `state`. Blocks that switch on and off (furnaces,
/// redstone lamps, candles) give off light only while lit.
pub fn emission(state: u16) -> u8 {
    let info = BlockInfo::of(state);
    match info.property(state, "lit") {
        Some("false") => 0,
        Some("true") => match info.name {
            "minecraft:furnace" | "minecraft:smoker" | "minecraft:blast_furnace" => 13,
            "minecraft:redstone_lamp" => 15,
            "minecraft:redstone_ore" | "minecraft:deepslate_redstone_ore" => 9,
            name if name.ends_with("candle") => {
                let candles = info.property(state, "candles").and_then(|c| c.parse().ok());
                3 * candles.unwrap_or(1u8)
            }
            name if name.ends_with("candle_cake") => 3,
            _ => info.light_emission,
        },
        _ => info.light_emission,
    }
}

/// How much light a block in `state` takes away as light passes into it.
pub fn filter(state: u16) -> u8 {
    BlockInfo::of(state).light_filter
}

/// Which light: from the sky, or from blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sky,
    Block,
}

/// Queues reused between updates, and the sections whose light changed.
#[derive(Default)]
pub struct LightEngine {
    remove: VecDeque<([i32; 3], u8)>,
    spread: VecDeque<([i32; 3], u8)>,
    /// Chunks with changed light: bit `i` of each mask is light section `i` (the
    /// protocol's numbering, starting one section below the world), for sky and block.
    changed: HashMap<(i32, i32), (u64, u64)>,
}

impl LightEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates the light around `pos` after its block changed from `old` to `new`.
    /// Chunks that are not loaded are left alone.
    pub fn block_changed(
        &mut self,
        chunks: &mut HashMap<(i32, i32), Chunk>,
        pos: [i32; 3],
        old: u16,
        new: u16,
    ) {
        if emission(old) == emission(new) && filter(old) == filter(new) {
            return;
        }
        for kind in [Kind::Sky, Kind::Block] {
            self.relight(chunks, pos, kind, new);
        }
    }

    /// Sections changed since the last call: `(chunk, sky mask, block mask)`.
    pub fn take_changes(&mut self) -> Vec<((i32, i32), u64, u64)> {
        self.changed
            .drain()
            .map(|(pos, (sky, block))| (pos, sky, block))
            .collect()
    }

    fn relight(
        &mut self,
        chunks: &mut HashMap<(i32, i32), Chunk>,
        pos: [i32; 3],
        kind: Kind,
        state: u16,
    ) {
        let mut world = Access {
            chunks,
            changed: &mut self.changed,
            kind,
        };
        // Take away whatever light was here and everything that depended on it.
        if let Some(level) = world.light(pos).filter(|&l| l > 0) {
            world.set(pos, 0);
            self.remove.push_back((pos, level));
        }
        while let Some((at, level)) = self.remove.pop_front() {
            for (dx, dy, dz) in DIRECTIONS {
                let next = [at[0] + dx, at[1] + dy, at[2] + dz];
                let Some(current) = world.light(next).filter(|&l| l > 0) else {
                    continue;
                };
                let straight_down = kind == Kind::Sky && dy == -1 && level == 15;
                if current < level || (straight_down && current == 15) {
                    world.set(next, 0);
                    self.remove.push_back((next, current));
                    if kind == Kind::Block {
                        let own = world.state(next).map_or(0, emission);
                        if own > 0 {
                            world.set(next, own);
                            self.spread.push_back((next, own));
                        }
                    }
                } else {
                    // Lit from elsewhere: it will spread back into the cleared region.
                    self.spread.push_back((next, current));
                }
            }
        }
        // Light flows back in from the neighbors, and from the block itself.
        for (dx, dy, dz) in DIRECTIONS {
            let next = [pos[0] + dx, pos[1] + dy, pos[2] + dz];
            if let Some(level) = world.light(next).filter(|&l| l > 0) {
                self.spread.push_back((next, level));
            }
        }
        if kind == Kind::Block {
            let own = emission(state);
            if own > world.light(pos).unwrap_or(0) {
                world.set(pos, own);
                self.spread.push_back((pos, own));
            }
        }
        while let Some((at, level)) = self.spread.pop_front() {
            for (dx, dy, dz) in DIRECTIONS {
                let next = [at[0] + dx, at[1] + dy, at[2] + dz];
                let Some(state) = world.state(next) else {
                    continue;
                };
                let filter = filter(state);
                let new = if kind == Kind::Sky && dy == -1 && level == 15 && filter == 0 {
                    15
                } else {
                    level.saturating_sub(filter.max(1))
                };
                if new > world.light(next).unwrap_or(15) {
                    world.set(next, new);
                    if new > 1 {
                        self.spread.push_back((next, new));
                    }
                }
            }
        }
    }
}

/// One kind of light across loaded chunks.
struct Access<'a> {
    chunks: &'a mut HashMap<(i32, i32), Chunk>,
    changed: &'a mut HashMap<(i32, i32), (u64, u64)>,
    kind: Kind,
}

impl Access<'_> {
    fn locate(pos: [i32; 3]) -> Option<((i32, i32), usize, [usize; 3])> {
        if !(MIN_Y..=MAX_Y).contains(&pos[1]) {
            return None;
        }
        let section = ((pos[1] - MIN_Y) >> 4) as usize + 1;
        let local = [
            (pos[0] & 15) as usize,
            (pos[1] & 15) as usize,
            (pos[2] & 15) as usize,
        ];
        Some(((pos[0] >> 4, pos[2] >> 4), section, local))
    }

    fn state(&self, pos: [i32; 3]) -> Option<u16> {
        let (chunk, _, [x, _, z]) = Self::locate(pos)?;
        Some(self.chunks.get(&chunk)?.get(x, pos[1], z))
    }

    /// Light at `pos`; `None` outside the world or in a chunk that is not loaded.
    fn light(&self, pos: [i32; 3]) -> Option<u8> {
        let (chunk, section, [x, y, z]) = Self::locate(pos)?;
        let light = &self.chunks.get(&chunk)?.light;
        let layers = match self.kind {
            Kind::Sky => &light.sky,
            Kind::Block => &light.block,
        };
        Some(layers[section].get(x, y, z))
    }

    fn set(&mut self, pos: [i32; 3], level: u8) {
        let Some((chunk_pos, section, [x, y, z])) = Self::locate(pos) else {
            return;
        };
        let Some(chunk) = self.chunks.get_mut(&chunk_pos) else {
            return;
        };
        let layers = match self.kind {
            Kind::Sky => &mut chunk.light.sky,
            Kind::Block => &mut chunk.light.block,
        };
        layers[section].set(x, y, z, level);
        let masks = self.changed.entry(chunk_pos).or_default();
        match self.kind {
            Kind::Sky => masks.0 |= 1 << section,
            Kind::Block => masks.1 |= 1 << section,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light::{ChunkLight, LightLayer, LIGHT_SECTIONS};
    use crate::paletted::Kind as Palette;
    use levitas_registry::block;

    /// A 3x3 chunk world of stone up to y = 63 with daylight above it, lit as the
    /// generator would.
    fn stone_world() -> HashMap<(i32, i32), Chunk> {
        let mut chunks = HashMap::new();
        for cx in -1..=1 {
            for cz in -1..=1 {
                let mut chunk = Chunk::empty(0, Palette::biomes(64));
                for x in 0..16 {
                    for z in 0..16 {
                        for y in MIN_Y..64 {
                            chunk.set(x, y, z, block::STONE);
                        }
                    }
                }
                let mut light = ChunkLight::full_sky();
                for section in 0..LIGHT_SECTIONS {
                    // Sections 0..=8 hold y < 64.
                    if section <= 8 {
                        light.sky[section] = LightLayer::Uniform(0);
                    }
                }
                chunk.light = light;
                chunks.insert((cx, cz), chunk);
            }
        }
        chunks
    }

    fn set(
        engine: &mut LightEngine,
        chunks: &mut HashMap<(i32, i32), Chunk>,
        pos: [i32; 3],
        state: u16,
    ) {
        let (cx, cz) = (pos[0] >> 4, pos[2] >> 4);
        let old = chunks.get_mut(&(cx, cz)).unwrap().set(
            (pos[0] & 15) as usize,
            pos[1],
            (pos[2] & 15) as usize,
            state,
        );
        engine.block_changed(chunks, pos, old, state);
    }

    fn light(chunks: &HashMap<(i32, i32), Chunk>, kind: Kind, pos: [i32; 3]) -> u8 {
        let mut changed = HashMap::new();
        let mut chunks = chunks.clone();
        Access {
            chunks: &mut chunks,
            changed: &mut changed,
            kind,
        }
        .light(pos)
        .unwrap()
    }

    #[test]
    fn torches_light_a_cave_and_take_it_back() {
        let (mut chunks, mut engine) = (stone_world(), LightEngine::new());
        // A tunnel along x at y = 10, crossing a chunk border.
        for x in -8..8 {
            set(&mut engine, &mut chunks, [x, 10, 0], block::AIR);
        }
        assert_eq!(light(&chunks, Kind::Block, [0, 10, 0]), 0);
        set(&mut engine, &mut chunks, [-3, 10, 0], block::TORCH);
        assert_eq!(light(&chunks, Kind::Block, [-3, 10, 0]), 14);
        assert_eq!(light(&chunks, Kind::Block, [2, 10, 0]), 9);
        // Light does not go into the stone.
        assert_eq!(light(&chunks, Kind::Block, [2, 11, 0]), 0);
        assert!(engine
            .take_changes()
            .iter()
            .any(|(pos, _, block)| *pos == (0, 0) && *block != 0));

        set(&mut engine, &mut chunks, [-3, 10, 0], block::AIR);
        assert_eq!(light(&chunks, Kind::Block, [2, 10, 0]), 0);
        assert_eq!(light(&chunks, Kind::Block, [-3, 10, 0]), 0);
    }

    #[test]
    fn digging_down_lets_the_sky_in() {
        let (mut chunks, mut engine) = (stone_world(), LightEngine::new());
        for y in (50..64).rev() {
            set(&mut engine, &mut chunks, [5, y, 5], block::AIR);
        }
        // Straight down a clear shaft, daylight does not fade.
        assert_eq!(light(&chunks, Kind::Sky, [5, 50, 5]), 15);
        // A side pocket at the bottom gets a little less.
        set(&mut engine, &mut chunks, [6, 50, 5], block::AIR);
        assert_eq!(light(&chunks, Kind::Sky, [6, 50, 5]), 14);

        // Covering the shaft takes the daylight away again.
        set(&mut engine, &mut chunks, [5, 63, 5], block::STONE);
        assert_eq!(light(&chunks, Kind::Sky, [5, 50, 5]), 0);
        assert_eq!(light(&chunks, Kind::Sky, [6, 50, 5]), 0);
    }

    #[test]
    fn lit_state_decides_emission() {
        let furnace = BlockInfo::by_name("minecraft:furnace").unwrap();
        let lit = furnace.state_with([("lit", "true")]);
        assert_eq!(emission(lit), 13);
        assert_eq!(emission(furnace.state_with([("lit", "false")])), 0);
        assert_eq!(emission(block::TORCH), 14);
    }
}
