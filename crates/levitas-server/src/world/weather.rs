//! What the weather does to the world, as the Minecraft Wiki describes it. Every tick
//! each loaded chunk has a 1 in 16 chance of a precipitation tick on one random column:
//! in a cold biome the water on top freezes and falling snow lays a snow layer, and
//! rain or snow fills a cauldron there now and then. During a thunderstorm each chunk
//! has a 1 in 100,000 chance a tick of being struck by lightning, which goes to the
//! nearest lightning rod within 128 blocks, or else to a living thing standing near the
//! spot. Lightning sets fire, hurts and ignites what is near and changes some mobs.

use std::collections::HashMap;
use std::sync::OnceLock;

use levitas_registry::{collision_boxes, entity_type, BIOMES_JSON};
use serde_json::Value;

use super::mobs::{Mob, MobKind};
use super::survival::Damage;
use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::blocks::{self, offset, prop, with, Dir};

/// One chunk in this many gets a precipitation tick each game tick.
const PRECIPITATION_CHANCE: u32 = 16;
/// One chunk in this many is struck by lightning each tick of a thunderstorm.
const LIGHTNING_CHANCE: u32 = 100_000;
/// How far a lightning rod draws lightning, in blocks.
const ROD_REACH: i32 = 128;
/// How long a struck rod gives off redstone power, in ticks.
const ROD_POWER_TICKS: u64 = 8;
/// How long a bolt is shown before the client is told to remove it.
const BOLT_TICKS: u64 = 10;
/// Lightning damage, and how long it sets things on fire, in ticks.
const LIGHTNING_DAMAGE: f32 = 5.0;
const LIGHTNING_FIRE_TICKS: i32 = 160;
/// A biome is cold enough for snow and ice below this temperature.
const SNOW_TEMPERATURE: f64 = 0.15;
/// Sound categories as the protocol numbers them.
const SOUND_CATEGORY_WEATHER: i32 = 3;

/// What falls from the sky in a biome: whether anything does, and how warm it is.
fn climate(biome: &str) -> (bool, f64) {
    static CLIMATES: OnceLock<HashMap<String, (bool, f64)>> = OnceLock::new();
    let all = CLIMATES.get_or_init(|| {
        let biomes: serde_json::Map<String, Value> =
            serde_json::from_str(BIOMES_JSON).expect("biomes are valid JSON");
        biomes
            .into_iter()
            .map(|(name, b)| {
                let name = name.trim_start_matches("minecraft:").to_owned();
                let wet = b["has_precipitation"].as_bool().unwrap_or(true);
                let temperature = b["temperature"].as_f64().unwrap_or(0.8);
                (name, (wet, temperature))
            })
            .collect()
    });
    all.get(biome.trim_start_matches("minecraft:"))
        .copied()
        .unwrap_or((true, 0.8))
}

/// What falls at a spot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Precipitation {
    None,
    Rain,
    Snow,
}

/// The biome's temperature at a height: it falls by 0.00125 a block above y 80.
fn temperature_at(base: f64, y: i32) -> f64 {
    if y > 80 {
        base - f64::from(y - 80) * 0.00125
    } else {
        base
    }
}

/// Whether a block stops falling things (and rain) for the motion-blocking heightmap.
fn blocks_motion(state: u16) -> bool {
    !collision_boxes(state).is_empty() || fluid_at(state).is_some()
}

impl World {
    /// What would fall at `pos` (rain or snow), ignoring whether it is raining.
    pub(super) fn precipitation_kind(&self, pos: BlockPos) -> Precipitation {
        let Some(biome) = self.biome_at(pos.x, pos.y, pos.z) else {
            return Precipitation::Rain;
        };
        let (wet, base) = climate(&biome);
        if !wet {
            Precipitation::None
        } else if temperature_at(base, pos.y) < SNOW_TEMPERATURE {
            Precipitation::Snow
        } else {
            Precipitation::Rain
        }
    }

    /// The y just above the highest block in a column that stops motion or holds fluid.
    fn rain_height(&self, x: i32, z: i32) -> Option<i32> {
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
        let top = (MIN_Y..MAX_Y)
            .rev()
            .find(|&y| blocks_motion(chunk.get(lx, y, lz)))
            .unwrap_or(MIN_Y - 1);
        Some(top + 1)
    }

    /// Whether it is raining (not snowing) on `pos`, which must be open to the sky.
    fn raining_at(&self, pos: BlockPos) -> bool {
        self.weather.is_raining()
            && self.rain_height(pos.x, pos.z).is_some_and(|y| y <= pos.y)
            && self.precipitation_kind(pos) == Precipitation::Rain
    }

    /// Precipitation ticks and lightning for every loaded chunk.
    pub(super) fn tick_weather_effects(&mut self) {
        if !self.dimension.has_sky() {
            return;
        }
        let thundering = self.weather.is_thundering();
        let mut columns = Vec::new();
        let mut strikes = Vec::new();
        for &(cx, cz) in self.chunks.keys() {
            if self.rng.below(PRECIPITATION_CHANCE) == 0 {
                let r = self.rng.below(256) as i32;
                columns.push((cx * 16 + (r & 15), cz * 16 + (r >> 4)));
            }
            if thundering && self.rng.below(LIGHTNING_CHANCE) == 0 {
                let r = self.rng.below(256) as i32;
                strikes.push((cx * 16 + (r & 15), cz * 16 + (r >> 4)));
            }
        }
        for (x, z) in columns {
            self.precipitation_tick(x, z);
        }
        for (x, z) in strikes {
            self.thunderstorm_strike(x, z);
        }
        self.tick_bolts();
    }

    /// Freezes water, lays snow and fills cauldrons at the top of column (`x`, `z`).
    fn precipitation_tick(&mut self, x: i32, z: i32) {
        let Some(above) = self.rain_height(x, z) else {
            return;
        };
        let top = BlockPos::new(x, above - 1, z);
        let air = BlockPos::new(x, above, z);
        let kind = self.precipitation_kind(top);
        let cold = kind == Precipitation::Snow;
        let dark = |w: &World, p: BlockPos| w.light_at(p).is_none_or(|(_, block)| block < 10);
        // Still water on top freezes in the cold, rain or not.
        let water = self.block(top);
        if cold
            && dark(self, top)
            && fluid_at(water).is_some_and(|f| f.fluid == Fluid::Water && f.source)
            && blocks::name(water) == "minecraft:water"
        {
            if let Some(ice) = BlockInfo::by_name("minecraft:ice") {
                self.set_block_updating(top, ice.default_state);
            }
            return;
        }
        if !self.weather.is_raining() || kind == Precipitation::None {
            return;
        }
        // Snow settles, a layer at a time up to the game rule's height.
        if cold && dark(self, air) {
            let here = self.block(air);
            let max = crate::rules::int("max_snow_accumulation_height").clamp(0, 8);
            if is_air(here) && max > 0 {
                if let Some(snow) = BlockInfo::by_name("minecraft:snow") {
                    let layer = snow.default_state;
                    if blocks::can_survive(layer, air, &mut |q| self.block(q)) {
                        self.set_block_updating(air, layer);
                    }
                }
            } else if blocks::name(here) == "minecraft:snow" {
                let layers: i32 = prop(here, "layers")
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(1);
                if layers < max {
                    self.set_block_updating(air, with(here, "layers", &(layers + 1).to_string()));
                }
            }
        }
        // A cauldron catches rain (5% of the time) or snow (10%).
        let cauldron = self.block(top);
        let filled = match (blocks::name(cauldron), kind) {
            ("minecraft:cauldron", Precipitation::Rain) if self.rng.f64() < 0.05 => {
                BlockInfo::by_name("minecraft:water_cauldron").map(|b| b.default_state)
            }
            ("minecraft:cauldron", Precipitation::Snow) if self.rng.f64() < 0.1 => {
                BlockInfo::by_name("minecraft:powder_snow_cauldron").map(|b| b.default_state)
            }
            ("minecraft:water_cauldron", Precipitation::Rain) if self.rng.f64() < 0.05 => {
                fuller(cauldron)
            }
            ("minecraft:powder_snow_cauldron", Precipitation::Snow) if self.rng.f64() < 0.1 => {
                fuller(cauldron)
            }
            _ => None,
        };
        if let Some(state) = filled {
            self.set_block_updating(top, state);
        }
    }

    /// A random strike in a thunderstorm, at column (`x`, `z`) or what draws it there.
    fn thunderstorm_strike(&mut self, x: i32, z: i32) {
        let Some(y) = self.rain_height(x, z) else {
            return;
        };
        let spot = BlockPos::new(x, y, z);
        let target = self.lightning_target(spot);
        if !self.raining_at(target) {
            return;
        }
        self.strike_lightning([
            f64::from(target.x) + 0.5,
            f64::from(target.y),
            f64::from(target.z) + 0.5,
        ]);
    }

    /// Where lightning aimed at `spot` lands: on the nearest lightning rod within 128
    /// blocks that has nothing above it, else on a living thing in the open within
    /// three blocks of the column, else on the spot.
    fn lightning_target(&mut self, spot: BlockPos) -> BlockPos {
        if let Some(rod) = self.nearest_rod(spot) {
            return BlockPos::new(rod.x, rod.y + 1, rod.z);
        }
        let near = |p: [f64; 3]| {
            (p[0] - f64::from(spot.x) - 0.5).abs() <= 3.5
                && (p[2] - f64::from(spot.z) - 0.5).abs() <= 3.5
                && p[1] >= f64::from(spot.y) - 3.0
        };
        let mut living: Vec<[f64; 3]> = self
            .players
            .values()
            .filter(|p| p.alive && near(p.pos))
            .map(|p| p.pos)
            .chain(
                self.mobs
                    .values()
                    .filter(|m| m.alive() && near(m.pos))
                    .map(|m| m.pos),
            )
            .collect();
        living.retain(|p| {
            let at = BlockPos::new(
                p[0].floor() as i32,
                p[1].floor() as i32,
                p[2].floor() as i32,
            );
            self.rain_height(at.x, at.z).is_some_and(|y| y <= at.y)
        });
        if living.is_empty() {
            return spot;
        }
        let p = living[self.rng.below(living.len() as u32) as usize];
        BlockPos::new(
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        )
    }

    /// The nearest lightning rod within reach of `spot` that is the top of its column.
    fn nearest_rod(&self, spot: BlockPos) -> Option<BlockPos> {
        let rods: Vec<u16> = rod_states();
        let reach = ROD_REACH / 16 + 1;
        let (scx, scz) = (spot.x.div_euclid(16), spot.z.div_euclid(16));
        let mut best: Option<(i64, BlockPos)> = None;
        for cx in scx - reach..=scx + reach {
            for cz in scz - reach..=scz + reach {
                let Some(chunk) = self.chunks.get(&(cx, cz)) else {
                    continue;
                };
                for (s, section) in chunk.sections().iter().enumerate() {
                    if !rods.iter().any(|&r| section.blocks().may_contain(r)) {
                        continue;
                    }
                    for i in 0..4096 {
                        let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
                        if !rods.contains(&section.get(x, y, z)) {
                            continue;
                        }
                        let pos = BlockPos::new(
                            cx * 16 + x as i32,
                            MIN_Y + (s * 16 + y) as i32,
                            cz * 16 + z as i32,
                        );
                        let (dx, dy, dz) = (pos.x - spot.x, pos.y - spot.y, pos.z - spot.z);
                        if dx.abs() > ROD_REACH || dz.abs() > ROD_REACH {
                            continue;
                        }
                        if self.rain_height(pos.x, pos.z) != Some(pos.y + 1) {
                            continue;
                        }
                        let d = i64::from(dx).pow(2) + i64::from(dy).pow(2) + i64::from(dz).pow(2);
                        if best.is_none_or(|(b, _)| d < b) {
                            best = Some((d, pos));
                        }
                    }
                }
            }
        }
        best.map(|(_, pos)| pos)
    }

    /// A lightning bolt at `at`: shown and heard, setting fires, hurting and changing
    /// what it hits, powering the rod it lands on.
    pub(super) fn strike_lightning(&mut self, at: [f64; 3]) {
        let id = next_entity_id();
        let uuid = (u128::from(self.rng.next_u64()) << 64) | u128::from(self.rng.next_u64());
        self.broadcast(
            Writer::packet(clientbound::ADD_ENTITY)
                .varint(id)
                .uuid(uuid)
                .varint(entity_type::LIGHTNING_BOLT)
                .f64(at[0])
                .f64(at[1])
                .f64(at[2])
                .lp_vec3([0.0, 0.0, 0.0])
                .u8(0)
                .u8(0)
                .u8(0)
                .varint(0),
            None,
        );
        self.bolts.push((id, self.tick + BOLT_TICKS));
        let thunder_pitch = 0.8 + self.rng.f64() as f32 * 0.2;
        let impact_pitch = 0.5 + self.rng.f64() as f32 * 0.2;
        self.weather_sound(
            "minecraft:entity.lightning_bolt.thunder",
            at,
            10_000.0,
            thunder_pitch,
        );
        self.weather_sound(
            "minecraft:entity.lightning_bolt.impact",
            at,
            2.0,
            impact_pitch,
        );

        let pos = BlockPos::new(
            at[0].floor() as i32,
            at[1].floor() as i32,
            at[2].floor() as i32,
        );
        let below = offset(pos, Dir::Down);
        let hit = self.block(below);
        if block_has_tag(hit, "minecraft:lightning_rods") {
            self.power_rod(below, hit);
        }
        // Fire where it lands and a few spots around, on Normal and Hard.
        let fiery = matches!(
            crate::rules::difficulty(),
            crate::rules::Difficulty::Normal | crate::rules::Difficulty::Hard
        ) && crate::rules::int("fire_spread_radius_around_player") != 0;
        if fiery {
            let mut spots = vec![pos];
            for _ in 0..4 {
                spots.push(BlockPos::new(
                    pos.x + self.rng.below(3) as i32 - 1,
                    pos.y + self.rng.below(3) as i32 - 1,
                    pos.z + self.rng.below(3) as i32 - 1,
                ));
            }
            for spot in spots {
                if !is_air(self.block(spot)) {
                    continue;
                }
                if let Some(fire) = crate::blocks::fire::fire_for(spot, &mut |q| self.block(q)) {
                    self.light_fire(spot, fire);
                }
            }
        }
        self.lightning_hits(at);
    }

    /// What lightning does to everything within three blocks of where it struck.
    fn lightning_hits(&mut self, at: [f64; 3]) {
        let inside = |p: [f64; 3]| {
            (p[0] - at[0]).abs() <= 3.0
                && (p[2] - at[2]).abs() <= 3.0
                && p[1] >= at[1] - 3.0
                && p[1] <= at[1] + 9.0
        };
        let players: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive && inside(p.pos))
            .map(|(&id, _)| id)
            .collect();
        for id in players {
            self.damage_from(id, LIGHTNING_DAMAGE, Damage::Lightning, None);
            self.ignite_player(id, LIGHTNING_FIRE_TICKS);
        }
        let mobs: Vec<i32> = self
            .mobs
            .iter()
            .filter(|(_, m)| m.alive() && inside(m.pos))
            .map(|(&id, _)| id)
            .collect();
        for id in mobs {
            let kind = self.mobs[&id].kind;
            match kind {
                // Pigs become zombified piglins and villagers witches, unhurt.
                MobKind::Pig => {
                    if !matches!(
                        crate::rules::difficulty(),
                        crate::rules::Difficulty::Peaceful
                    ) {
                        self.transform_mob(id, MobKind::ZombifiedPiglin);
                        continue;
                    }
                }
                MobKind::Villager => {
                    if !matches!(
                        crate::rules::difficulty(),
                        crate::rules::Difficulty::Peaceful
                    ) {
                        self.transform_mob(id, MobKind::Witch);
                        continue;
                    }
                }
                // Creepers become charged.
                MobKind::Creeper => {
                    if let Some(m) = self.mobs.get_mut(&id) {
                        m.powered = true;
                    }
                    if let Some(m) = self.mobs.remove(&id) {
                        self.send_to_viewers(&m, super::monsters::creeper_powered(id, true));
                        self.mobs.insert(id, m);
                    }
                }
                _ => {}
            }
            self.ignite_mob(id, LIGHTNING_FIRE_TICKS);
            self.hit_mob(
                id,
                LIGHTNING_DAMAGE,
                None,
                [0.0, 0.0],
                "minecraft:lightning_bolt",
            );
        }
        // Items on the ground burn up.
        let burnt: Vec<i32> = self
            .items
            .iter()
            .filter(|(_, item)| inside(item.pos()))
            .map(|(&id, _)| id)
            .collect();
        for id in burnt {
            self.items.remove(&id);
            self.broadcast(super::items::remove_entity(id), None);
        }
    }

    /// Turns mob `id` into a mob of `kind` in its place, keeping who it was.
    fn transform_mob(&mut self, id: i32, kind: MobKind) {
        let Some(old) = self.mobs.remove(&id) else {
            return;
        };
        let mut new = Mob::new(kind, old.pos, &mut self.rng);
        new.uuid = old.uuid;
        new.name = old.name;
        new.yaw = old.yaw;
        new.head_yaw = old.head_yaw;
        new.viewers = old.viewers;
        self.send_to_viewers(&new, super::items::remove_entity(id));
        for p in new.spawn_packets(id) {
            self.send_to_viewers(&new, p);
        }
        self.mobs.insert(id, new);
    }

    /// A struck lightning rod gives off power for a moment, and loses its patina.
    fn power_rod(&mut self, pos: BlockPos, state: u16) {
        let name = blocks::name(state);
        let fresh = if name.starts_with("minecraft:waxed_") {
            state
        } else {
            let mut s = state;
            if let Some(rod) = BlockInfo::by_name("minecraft:lightning_rod") {
                let mut base = rod.default_state;
                for p in ["facing", "waterlogged"] {
                    if let Some(v) = prop(state, p) {
                        base = with(base, p, v);
                    }
                }
                s = base;
            }
            s
        };
        let powered = with(fresh, "powered", "true");
        self.set_block_updating(pos, powered);
        self.schedule(pos, powered, ROD_POWER_TICKS);
    }

    /// Removes the bolts that have been shown long enough.
    fn tick_bolts(&mut self) {
        let now = self.tick;
        let (done, left): (Vec<_>, Vec<_>) = self.bolts.drain(..).partition(|&(_, end)| end <= now);
        self.bolts = left;
        for (id, _) in done {
            self.broadcast(super::items::remove_entity(id), None);
        }
    }

    /// A sound in the weather category, heard as far as its volume carries.
    fn weather_sound(&mut self, name: &str, at: [f64; 3], volume: f32, pitch: f32) {
        let Some(sound) = levitas_registry::sound_id(name) else {
            return;
        };
        let seed = self.rng.next_u64() as i64;
        let fixed = |v: f64| (v * 8.0) as i32;
        self.broadcast(
            Writer::packet(clientbound::SOUND)
                .varint(sound + 1)
                .varint(SOUND_CATEGORY_WEATHER)
                .i32(fixed(at[0]))
                .i32(fixed(at[1]))
                .i32(fixed(at[2]))
                .f32(volume)
                .f32(pitch)
                .i64(seed),
            None,
        );
    }
}

/// A cauldron one level fuller, if it is not full.
fn fuller(state: u16) -> Option<u16> {
    let level: i32 = prop(state, "level").and_then(|l| l.parse().ok())?;
    (level < 3).then(|| with(state, "level", &(level + 1).to_string()))
}

/// The lightning rods, as the `minecraft:lightning_rods` tag lists them.
const ROD_NAMES: &[&str] = &[
    "minecraft:lightning_rod",
    "minecraft:exposed_lightning_rod",
    "minecraft:weathered_lightning_rod",
    "minecraft:oxidized_lightning_rod",
    "minecraft:waxed_lightning_rod",
    "minecraft:waxed_exposed_lightning_rod",
    "minecraft:waxed_weathered_lightning_rod",
    "minecraft:waxed_oxidized_lightning_rod",
];

/// Every state of every lightning rod.
fn rod_states() -> Vec<u16> {
    static STATES: OnceLock<Vec<u16>> = OnceLock::new();
    STATES
        .get_or_init(|| {
            ROD_NAMES
                .iter()
                .filter_map(|name| BlockInfo::by_name(name))
                .flat_map(|b| {
                    let count: usize = b.properties.iter().map(|(_, v)| v.len()).product();
                    (0..count as u16).map(move |i| b.first_state + i)
                })
                .collect()
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-weather-{}-{}",
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
        world
    }

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    fn rain(w: &mut World) {
        w.set_weather("rain", Some(1000));
        w.weather.rain_level = 1.0;
    }

    #[test]
    fn biomes_decide_between_rain_and_snow() {
        assert!(!climate("minecraft:desert").0);
        assert!(climate("minecraft:snowy_plains").1 < SNOW_TEMPERATURE);
        // High up even a temperate biome is cold.
        let (_, windswept) = climate("minecraft:windswept_hills");
        assert!(temperature_at(windswept, 80) >= SNOW_TEMPERATURE);
        assert!(temperature_at(windswept, 200) < SNOW_TEMPERATURE);
    }

    #[test]
    fn rain_fills_a_cauldron_in_the_open() {
        let mut w = flat_world();
        rain(&mut w);
        let top = w.rain_height(3, 3).unwrap();
        let at = BlockPos::new(3, top, 3);
        w.set_block(at, state("minecraft:cauldron"));
        for _ in 0..2000 {
            w.precipitation_tick(3, 3);
        }
        let filled = w.block(at);
        assert_eq!(blocks::name(filled), "minecraft:water_cauldron");
        assert_eq!(prop(filled, "level"), Some("3"));
    }

    #[test]
    fn lightning_charges_creepers_turns_pigs_and_powers_rods() {
        let mut w = flat_world();
        let top = w.rain_height(8, 8).unwrap();
        let at = [8.5, f64::from(top), 8.5];
        let creeper = next_entity_id();
        w.mobs
            .insert(creeper, Mob::new(MobKind::Creeper, at, &mut w.rng));
        let pig = next_entity_id();
        w.mobs
            .insert(pig, Mob::new(MobKind::Pig, [9.5, at[1], 8.5], &mut w.rng));
        w.strike_lightning(at);
        assert!(w.mobs[&creeper].powered);
        assert_eq!(w.mobs[&pig].kind, MobKind::ZombifiedPiglin);
        assert!(w.mobs[&creeper].to_nbt().int("powered").is_some());

        // A rod in reach draws the strike and gives off power for a moment.
        let rod = BlockPos::new(20, top, 20);
        w.set_block(rod, state("minecraft:oxidized_lightning_rod"));
        let spot = BlockPos::new(2, top, 2);
        assert_eq!(w.lightning_target(spot), BlockPos::new(20, top + 1, 20));
        w.strike_lightning([20.5, f64::from(top + 1), 20.5]);
        let struck = w.block(rod);
        assert_eq!(blocks::name(struck), "minecraft:lightning_rod");
        assert_eq!(prop(struck, "powered"), Some("true"));
        assert_eq!(
            crate::blocks::redstone::weak(struck, Dir::North),
            crate::blocks::redstone::MAX_POWER
        );
        // The bolt goes away again.
        assert!(!w.bolts.is_empty());
        w.tick += BOLT_TICKS;
        w.tick_bolts();
        assert!(w.bolts.is_empty());
    }
}
