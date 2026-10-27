//! Mobs: for now the passive farm animals (cows, pigs, sheep and chickens).
//!
//! Behaviour follows the Minecraft Wiki: animals wander about, panic and run for a few
//! seconds when hurt, chickens flutter down slowly and lay eggs, sheep can be sheared and
//! regrow their wool by eating grass, cows can be milked. Animals appear when a chunk is
//! first generated, in groups chosen from the biome's spawn list, and are saved in the
//! vanilla `entities/` region files.

use std::collections::{HashMap, HashSet};

use levitas_nbt::{Compound, Tag};
use levitas_registry::{collision_boxes, entity_type, SyncedRegistry, BIOMES_JSON, DATA_VERSION};
use serde_json::Value;
use tracing::error;

use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::loot::EntityProps;

const GRAVITY: f64 = 0.08;
const AIR_DRAG: f64 = 0.98;
const JUMP: f64 = 0.42;
/// Ticks a mob cannot be hurt again (unless by a stronger hit) after being hurt.
const HURT_TICKS: u32 = 10;
/// Ticks a dead mob lies on its side before it disappears.
const DEATH_TICKS: u32 = 20;
/// Ticks an animal runs about after being hurt.
const PANIC_TICKS: u32 = 100;
const KNOCKBACK: f64 = 0.4;
/// How far players can hit or use a mob from (eyes to the mob's centre).
const REACH: f64 = 6.0;
/// Chance per chunk generation attempt of an animal group, unless the biome says.
const DEFAULT_CREATURE_PROBABILITY: f64 = 0.1;

const DATA_SHEEP_WOOL: u8 = 17;
const SERIALIZER_BYTE: i32 = 0;
const SHEARED: u8 = 0x10;
const ENTITY_EVENT_MOB_DEATH: i8 = 3;
const ENTITY_EVENT_EAT_GRASS: i8 = 10;

/// Wool colours by their dye number, as sheep store them.
const COLORS: [&str; 16] = [
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MobKind {
    Cow,
    Pig,
    Sheep,
    Chicken,
}

impl MobKind {
    pub fn from_name(name: &str) -> Option<MobKind> {
        match name {
            "minecraft:cow" => Some(MobKind::Cow),
            "minecraft:pig" => Some(MobKind::Pig),
            "minecraft:sheep" => Some(MobKind::Sheep),
            "minecraft:chicken" => Some(MobKind::Chicken),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MobKind::Cow => "minecraft:cow",
            MobKind::Pig => "minecraft:pig",
            MobKind::Sheep => "minecraft:sheep",
            MobKind::Chicken => "minecraft:chicken",
        }
    }

    fn entity_type(self) -> i32 {
        match self {
            MobKind::Cow => entity_type::COW,
            MobKind::Pig => entity_type::PIG,
            MobKind::Sheep => entity_type::SHEEP,
            MobKind::Chicken => entity_type::CHICKEN,
        }
    }

    pub fn max_health(self) -> f32 {
        match self {
            MobKind::Cow | MobKind::Pig => 10.0,
            MobKind::Sheep => 8.0,
            MobKind::Chicken => 4.0,
        }
    }

    /// Width and height of the hitbox.
    fn size(self) -> (f64, f64) {
        match self {
            MobKind::Cow => (0.9, 1.4),
            MobKind::Pig => (0.9, 0.9),
            MobKind::Sheep => (0.9, 1.3),
            MobKind::Chicken => (0.4, 0.7),
        }
    }

    /// Walking speed in blocks per tick.
    fn speed(self) -> f64 {
        match self {
            MobKind::Chicken => 0.08,
            _ => 0.1,
        }
    }

    /// The sound family, as in `minecraft:entity.<family>.hurt`.
    fn sound(self) -> &'static str {
        match self {
            MobKind::Cow => "cow",
            MobKind::Pig => "pig",
            MobKind::Sheep => "sheep",
            MobKind::Chicken => "chicken",
        }
    }
}

pub struct Mob {
    pub kind: MobKind,
    uuid: u128,
    pub pos: [f64; 3],
    vel: [f64; 3],
    yaw: f32,
    head_yaw: f32,
    on_ground: bool,
    pub health: f32,
    fall_distance: f64,
    hurt_ticks: u32,
    last_damage: f32,
    dying: Option<u32>,
    panic: u32,
    /// Where the mob is walking to, and for how many more ticks it tries.
    target: Option<([f64; 2], u32)>,
    blocked: bool,
    ambient: i32,
    /// Sheep: wool colour (0-15) and whether sheared.
    pub color: u8,
    pub sheared: bool,
    /// Chicken: ticks until the next egg.
    egg_ticks: u32,
    /// What players were last told.
    sent_pos: [f64; 3],
    sent_angles: (u8, u8),
    /// Players who can see this mob.
    viewers: HashSet<i32>,
}

impl Mob {
    pub fn new(kind: MobKind, pos: [f64; 3], rng: &mut Rng) -> Mob {
        Mob {
            kind,
            uuid: (u128::from(rng.next_u64()) << 64) | u128::from(rng.next_u64()),
            pos,
            vel: [0.0; 3],
            yaw: rng.f64() as f32 * 360.0,
            head_yaw: 0.0,
            on_ground: false,
            health: kind.max_health(),
            fall_distance: 0.0,
            hurt_ticks: 0,
            last_damage: 0.0,
            dying: None,
            panic: 0,
            target: None,
            blocked: false,
            ambient: 0,
            color: if kind == MobKind::Sheep {
                random_wool(rng)
            } else {
                0
            },
            sheared: false,
            egg_ticks: 6000 + rng.below(6000),
            sent_pos: pos,
            sent_angles: (0, 0),
            viewers: HashSet::new(),
        }
    }

    pub fn chunk(&self) -> (i32, i32) {
        (
            (self.pos[0] / 16.0).floor() as i32,
            (self.pos[2] / 16.0).floor() as i32,
        )
    }

    fn spawn_packets(&self, id: i32) -> Vec<Writer> {
        let mut packets = vec![Writer::packet(clientbound::ADD_ENTITY)
            .varint(id)
            .uuid(self.uuid)
            .varint(self.kind.entity_type())
            .f64(self.pos[0])
            .f64(self.pos[1])
            .f64(self.pos[2])
            .u8(0)
            .u8(angle(self.yaw))
            .u8(angle(self.head_yaw))
            .varint(0)
            .i16(0)
            .i16(0)
            .i16(0)];
        if self.kind == MobKind::Sheep {
            packets.push(self.wool_data(id));
        }
        packets
    }

    fn wool_data(&self, id: i32) -> Writer {
        let wool = self.color | if self.sheared { SHEARED } else { 0 };
        Writer::packet(clientbound::SET_ENTITY_DATA)
            .varint(id)
            .u8(DATA_SHEEP_WOOL)
            .varint(SERIALIZER_BYTE)
            .u8(wool)
            .u8(0xFF)
    }

    fn loot_props(&self) -> EntityProps {
        EntityProps {
            on_fire: false,
            sheep_color: (self.kind == MobKind::Sheep).then(|| COLORS[usize::from(self.color)]),
            sheared: self.sheared,
        }
    }

    /// The mob as vanilla saves it in `entities/` region files.
    fn to_nbt(&self) -> Compound {
        let uuid = [
            (self.uuid >> 96) as i32,
            (self.uuid >> 64) as i32,
            (self.uuid >> 32) as i32,
            self.uuid as i32,
        ];
        let mut nbt = Compound::new()
            .with("id", self.kind.name())
            .with("UUID", uuid.to_vec())
            .with(
                "Pos",
                self.pos.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with(
                "Motion",
                self.vel.iter().map(|&v| Tag::Double(v)).collect::<Vec<_>>(),
            )
            .with("Rotation", vec![Tag::Float(self.yaw), Tag::Float(0.0)])
            .with("Health", self.health)
            .with("OnGround", self.on_ground)
            .with("FallDistance", self.fall_distance)
            .with("Age", 0);
        match self.kind {
            MobKind::Sheep => {
                nbt.insert("Color", self.color as i8);
                nbt.insert("Sheared", self.sheared);
            }
            MobKind::Chicken => nbt.insert("EggLayTime", self.egg_ticks as i32),
            _ => {}
        }
        nbt
    }

    fn from_nbt(nbt: &Compound, rng: &mut Rng) -> Option<Mob> {
        let kind = MobKind::from_name(nbt.str("id")?)?;
        let pos: Vec<f64> = nbt.list("Pos")?.iter().filter_map(Tag::as_f64).collect();
        let &[x, y, z] = &pos[..] else {
            return None;
        };
        let mut mob = Mob::new(kind, [x, y, z], rng);
        if let Some(&[a, b, c, d]) = nbt.int_array("UUID") {
            mob.uuid = (u128::from(a as u32) << 96)
                | (u128::from(b as u32) << 64)
                | (u128::from(c as u32) << 32)
                | u128::from(d as u32);
        }
        if let Some(Tag::Float(yaw)) = nbt.list("Rotation").and_then(|r| r.first()) {
            mob.yaw = *yaw;
        }
        mob.health = nbt.float("Health").map_or(kind.max_health(), |h| h as f32);
        if mob.health <= 0.0 {
            return None;
        }
        mob.on_ground = nbt.int("OnGround").unwrap_or(1) != 0;
        if kind == MobKind::Sheep {
            mob.color = nbt.int("Color").unwrap_or(0).clamp(0, 15) as u8;
            mob.sheared = nbt.int("Sheared").unwrap_or(0) != 0;
        }
        if let Some(t) = nbt.int("EggLayTime") {
            mob.egg_ticks = t.max(0) as u32;
        }
        mob.sent_pos = mob.pos;
        Some(mob)
    }
}

/// A natural wool colour, with vanilla's odds: mostly white, sometimes grey, black or
/// brown, and rarely pink.
fn random_wool(rng: &mut Rng) -> u8 {
    let roll = rng.below(100_000);
    match roll {
        0..=4_999 => 15,       // black 5%
        5_000..=9_999 => 7,    // gray 5%
        10_000..=14_999 => 8,  // light gray 5%
        15_000..=17_999 => 12, // brown 3%
        18_000..=18_163 => 6,  // pink 0.164%
        _ => 0,                // white
    }
}

/// One animal spawner from a biome's spawn list.
pub struct Spawner {
    kind: MobKind,
    weight: u32,
    min: u32,
    max: u32,
}

/// The animals each biome spawns when its chunks are generated.
pub struct BiomeSpawns {
    by_biome: HashMap<String, (f64, Vec<Spawner>)>,
}

impl BiomeSpawns {
    pub fn load() -> Self {
        let biomes: serde_json::Map<String, Value> =
            serde_json::from_str(BIOMES_JSON).expect("biomes are valid JSON");
        let by_biome = biomes
            .into_iter()
            .map(|(name, biome)| {
                let probability = biome["creature_spawn_probability"]
                    .as_f64()
                    .unwrap_or(DEFAULT_CREATURE_PROBABILITY);
                let spawners = biome["spawners"]["creature"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| {
                        Some(Spawner {
                            kind: MobKind::from_name(s["type"].as_str()?)?,
                            weight: s["weight"].as_u64()? as u32,
                            min: s["minCount"].as_u64()? as u32,
                            max: s["maxCount"].as_u64()? as u32,
                        })
                    })
                    .collect();
                (name, (probability, spawners))
            })
            .collect();
        BiomeSpawns { by_biome }
    }
}

impl World {
    /// Puts groups of animals into a freshly generated chunk, as vanilla does once per
    /// chunk: while a roll stays under the biome's creature probability, a group of one
    /// kind from the biome's list spawns around a random spot.
    pub(super) fn spawn_animals_in_chunk(&mut self, cx: i32, cz: i32) {
        // The biome where animals would stand, not the caves below it.
        let (x, z) = (cx * 16 + 8, cz * 16 + 8);
        let surface = self.surface_y(x, z).unwrap_or(64);
        let Some(biome) = self.biome_at(x, surface, z) else {
            return;
        };
        let Some((probability, spawners)) = self.biome_spawns.by_biome.get(&biome) else {
            return;
        };
        let total: u32 = spawners.iter().map(|s| s.weight).sum();
        if total == 0 {
            return;
        }
        let probability = *probability;
        let mut groups = Vec::new();
        while self.rng.f64() < probability && groups.len() < 4 {
            let mut pick = self.rng.below(total);
            let spawner = spawners
                .iter()
                .find(|s| {
                    if pick < s.weight {
                        true
                    } else {
                        pick -= s.weight;
                        false
                    }
                })
                .expect("weights add up");
            let size = spawner.min + self.rng.below(spawner.max - spawner.min + 1);
            groups.push((spawner.kind, size));
        }
        for (kind, size) in groups {
            let mut x = cx * 16 + self.rng.below(16) as i32;
            let mut z = cz * 16 + self.rng.below(16) as i32;
            for _ in 0..size {
                // Each animal of the group a few blocks from the last one, inside the chunk.
                x = (x + self.rng.below(11) as i32 - 5).clamp(cx * 16, cx * 16 + 15);
                z = (z + self.rng.below(11) as i32 - 5).clamp(cz * 16, cz * 16 + 15);
                if let Some(y) = self.animal_spawn_height(x, z) {
                    let pos = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
                    let mob = Mob::new(kind, pos, &mut self.rng);
                    self.mobs.insert(next_entity_id(), mob);
                }
            }
        }
    }

    /// The y of the highest block in a column that is not air or leaves.
    fn surface_y(&self, x: i32, z: i32) -> Option<i32> {
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
        (MIN_Y..MAX_Y).rev().find(|&y| {
            let state = chunk.get(lx, y, lz);
            !is_air(state) && !block_has_tag(state, "minecraft:leaves")
        })
    }

    /// The y an animal can stand at in column (`x`, `z`): on top of grass (or another
    /// block animals spawn on), with two blocks of room above.
    fn animal_spawn_height(&self, x: i32, z: i32) -> Option<i32> {
        let ground = self.surface_y(x, z)?;
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
        let below = chunk.get(lx, ground, lz);
        let room = (1..=2).all(|dy| collision_boxes(chunk.get(lx, ground + dy, lz)).is_empty());
        (block_has_tag(below, "minecraft:animals_spawnable_on") && room).then_some(ground + 1)
    }

    /// The biome at a block, like `minecraft:plains`.
    fn biome_at(&self, x: i32, y: i32, z: i32) -> Option<String> {
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        let section = chunk.sections().get(((y - MIN_Y) / 16) as usize)?;
        let index =
            ((((y - MIN_Y) & 15) >> 2) * 16 + ((z & 15) >> 2) * 4 + ((x & 15) >> 2)) as usize;
        let biome = section.biomes().get(index);
        let registry = SyncedRegistry::get("minecraft:worldgen/biome")?;
        registry
            .entries
            .get(usize::from(biome))
            .map(|s| (*s).to_owned())
    }

    pub(super) fn tick_mobs(&mut self) {
        let ids: Vec<i32> = self.mobs.keys().copied().collect();
        for id in ids {
            let Some(mut mob) = self.mobs.remove(&id) else {
                continue;
            };
            // Mobs in chunks that are not loaded stand still.
            if !self.chunks.contains_key(&mob.chunk()) {
                self.mobs.insert(id, mob);
                continue;
            }
            if let Some(ticks) = mob.dying.as_mut() {
                *ticks += 1;
                if *ticks >= DEATH_TICKS {
                    self.send_to_viewers(&mob, items::remove_entity(id));
                    continue;
                }
                self.mobs.insert(id, mob);
                continue;
            }
            self.think(&mut mob);
            self.step(&mut mob);
            self.animal_chores(id, &mut mob);
            self.mobs.insert(id, mob);
        }
        self.track_mobs();
    }

    /// Picks where to walk and how to face.
    fn think(&mut self, mob: &mut Mob) {
        mob.hurt_ticks = mob.hurt_ticks.saturating_sub(1);
        let panicking = mob.panic > 0;
        mob.panic = mob.panic.saturating_sub(1);
        if let Some((_, ticks)) = mob.target.as_mut() {
            *ticks = ticks.saturating_sub(1);
        }
        let arrived = mob.target.is_some_and(|(t, ticks)| {
            ticks == 0 || (t[0] - mob.pos[0]).hypot(t[1] - mob.pos[2]) < 1.0
        });
        if arrived {
            mob.target = None;
        }
        // Wandering starts now and then; panic keeps picking new spots nearby.
        let wants_target = if panicking {
            mob.target.is_none()
        } else {
            mob.target.is_none() && self.rng.below(120) == 0
        };
        if wants_target {
            let reach = if panicking { 5.0 } else { 10.0 };
            let dx = (self.rng.f64() * 2.0 - 1.0) * reach;
            let dz = (self.rng.f64() * 2.0 - 1.0) * reach;
            mob.target = Some(([mob.pos[0] + dx, mob.pos[2] + dz], 200));
        }

        // Look at a nearby player now and then.
        let eyes = [
            mob.pos[0],
            mob.pos[1] + mob.kind.size().1 * 0.85,
            mob.pos[2],
        ];
        let watched = self
            .players
            .values()
            .filter(|p| p.alive)
            .map(|p| p.pos)
            .find(|p| (p[0] - eyes[0]).hypot(p[2] - eyes[2]) < 6.0);
        mob.head_yaw = match watched {
            Some(p) if !panicking => yaw_towards(eyes, p),
            _ => mob.yaw,
        };
    }

    /// Moves the mob one tick: walking, jumping, swimming, gravity and collisions.
    fn step(&mut self, mob: &mut Mob) {
        let in_water = self
            .loaded_state(feet(mob.pos))
            .and_then(fluid_at)
            .is_some_and(|f| f.fluid == Fluid::Water);
        if let Some((target, _)) = mob.target {
            let (dx, dz) = (target[0] - mob.pos[0], target[1] - mob.pos[2]);
            let distance = dx.hypot(dz).max(1e-6);
            let speed = mob.kind.speed() * if mob.panic > 0 { 2.0 } else { 1.0 };
            if mob.on_ground || in_water {
                mob.vel[0] = dx / distance * speed;
                mob.vel[2] = dz / distance * speed;
            }
            mob.yaw = yaw_towards(mob.pos, [target[0], mob.pos[1], target[1]]);
            if mob.blocked && (mob.on_ground || in_water) {
                mob.vel[1] = JUMP;
            }
        } else if mob.on_ground {
            mob.vel[0] *= 0.5;
            mob.vel[2] *= 0.5;
        }

        if in_water {
            // Animals swim up and float at the surface.
            mob.vel[1] = (mob.vel[1] * 0.8 + 0.04).min(0.1);
        } else {
            mob.vel[1] = (mob.vel[1] - GRAVITY) * AIR_DRAG;
            if mob.kind == MobKind::Chicken && mob.vel[1] < 0.0 {
                mob.vel[1] *= 0.6; // flapping
            }
        }

        let (width, height) = mob.kind.size();
        let half = width / 2.0;
        let mut pos = mob.pos;
        // Vertical first, then each horizontal axis, stopping at blocks.
        let new_y = pos[1] + mob.vel[1];
        let falling = mob.vel[1] < 0.0;
        match self.first_hit(pos, half, height, 1, new_y) {
            Some(stop) => {
                pos[1] = stop;
                mob.on_ground = falling;
                mob.vel[1] = 0.0;
            }
            None => {
                pos[1] = new_y;
                mob.on_ground = false;
            }
        }
        mob.blocked = false;
        for axis in [0, 2] {
            let moved = pos[axis] + mob.vel[axis];
            if self.first_hit(pos, half, height, axis, moved).is_some() {
                mob.blocked = true;
                mob.vel[axis] = 0.0;
            } else {
                pos[axis] = moved;
            }
        }

        if mob.on_ground || in_water {
            let fallen = std::mem::take(&mut mob.fall_distance);
            if !in_water && fallen > 3.0 && mob.kind != MobKind::Chicken {
                mob.health -= (fallen - 3.0).ceil() as f32;
            }
        } else if pos[1] < mob.pos[1] {
            mob.fall_distance += mob.pos[1] - pos[1];
        }
        mob.pos = pos;
        if pos[1] < f64::from(MIN_Y - 64) {
            mob.health = 0.0;
        }
    }

    /// Where movement along `axis` towards `to` is stopped by a block, if it is: the
    /// coordinate the mob ends at instead. Blocks in unloaded chunks count as solid.
    fn first_hit(
        &self,
        pos: [f64; 3],
        half: f64,
        height: f64,
        axis: usize,
        to: f64,
    ) -> Option<f64> {
        let mut min = [pos[0] - half, pos[1], pos[2] - half];
        let mut max = [pos[0] + half, pos[1] + height, pos[2] + half];
        let delta = to - pos[axis];
        min[axis] += delta;
        max[axis] += delta;
        let eps = 1e-7;
        let mut stop: Option<f64> = None;
        for bx in (min[0] + eps).floor() as i32..=(max[0] - eps).floor() as i32 {
            for by in (min[1] + eps).floor() as i32 - 1..=(max[1] - eps).floor() as i32 {
                for bz in (min[2] + eps).floor() as i32..=(max[2] - eps).floor() as i32 {
                    let block = BlockPos::new(bx, by, bz);
                    let boxes: &[[f32; 6]] = match self.loaded_state(block) {
                        Some(state) => collision_boxes(state),
                        None if (MIN_Y..=MAX_Y).contains(&by) => &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]],
                        None => &[],
                    };
                    for b in boxes {
                        let lo = [
                            f64::from(bx) + f64::from(b[0]),
                            f64::from(by) + f64::from(b[1]),
                            f64::from(bz) + f64::from(b[2]),
                        ];
                        let hi = [
                            f64::from(bx) + f64::from(b[3]),
                            f64::from(by) + f64::from(b[4]),
                            f64::from(bz) + f64::from(b[5]),
                        ];
                        let overlaps = (0..3).all(|i| min[i] < hi[i] - eps && max[i] > lo[i] + eps);
                        if !overlaps {
                            continue;
                        }
                        // Stop against the face the mob moves into.
                        let end = if delta > 0.0 {
                            lo[axis] - (max[axis] - min[axis]) + if axis == 1 { 0.0 } else { half }
                        } else if axis == 1 {
                            hi[1]
                        } else {
                            hi[axis] + half
                        };
                        stop = Some(match stop {
                            Some(s) if delta > 0.0 => s.min(end),
                            Some(s) => s.max(end),
                            None => end,
                        });
                    }
                }
            }
        }
        stop
    }

    fn loaded_state(&self, pos: BlockPos) -> Option<u16> {
        if !(MIN_Y..=MAX_Y).contains(&pos.y) {
            return Some(block::AIR);
        }
        let chunk = self.chunks.get(&pos.chunk())?;
        Some(chunk.get((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize))
    }

    /// Sounds, eggs and grass eating; and dying from a fall.
    fn animal_chores(&mut self, id: i32, mob: &mut Mob) {
        if mob.health <= 0.0 {
            self.kill_mob(id, mob);
            return;
        }
        mob.ambient += 1;
        if self.rng.below(1000) < mob.ambient.max(0) as u32 {
            mob.ambient = -80;
            let sound = format!("minecraft:entity.{}.ambient", mob.kind.sound());
            self.mob_sound(mob, &sound);
        }
        match mob.kind {
            MobKind::Chicken => {
                mob.egg_ticks = mob.egg_ticks.saturating_sub(1);
                if mob.egg_ticks == 0 {
                    mob.egg_ticks = 6000 + self.rng.below(6000);
                    if let Some(egg) = ItemInfo::id_of("minecraft:egg") {
                        self.scatter_item(mob.pos, Stack::new(egg, 1));
                    }
                    self.mob_sound(mob, "minecraft:entity.chicken.egg");
                }
            }
            MobKind::Sheep if mob.sheared && mob.on_ground && self.rng.below(1000) == 0 => {
                let ground = BlockPos::new(
                    mob.pos[0].floor() as i32,
                    mob.pos[1].floor() as i32 - 1,
                    mob.pos[2].floor() as i32,
                );
                if self.loaded_state(ground) == Some(block::GRASS_BLOCK) {
                    self.set_block_updating(ground, block::DIRT);
                    mob.sheared = false;
                    self.send_to_viewers(mob, entity_event(id, ENTITY_EVENT_EAT_GRASS));
                    self.send_to_viewers(mob, mob.wool_data(id));
                }
            }
            _ => {}
        }
    }

    fn mob_sound(&mut self, mob: &Mob, name: &str) {
        let pitch = 0.8 + self.rng.f64() as f32 * 0.4;
        self.play_sound_at(name, mob.pos, 1.0, pitch, None);
    }

    fn send_to_viewers(&mut self, mob: &Mob, packet: Writer) {
        let frame = frame(packet);
        for id in &mob.viewers {
            if let Some(p) = self.players.get_mut(id) {
                p.deliver(&frame);
            }
        }
    }

    /// Shows mobs to players who have their chunk loaded, hides them from players who
    /// do not, and sends everyone watching the moves of the last tick.
    fn track_mobs(&mut self) {
        let ids: Vec<i32> = self.mobs.keys().copied().collect();
        for id in ids {
            let mut mob = self.mobs.remove(&id).expect("listed");
            let chunk = mob.chunk();
            let watching: HashSet<i32> = self
                .players
                .iter()
                .filter(|(_, p)| p.loaded.contains(&chunk))
                .map(|(&pid, _)| pid)
                .collect();
            for &pid in watching.difference(&mob.viewers) {
                for packet in mob.spawn_packets(id) {
                    self.send(pid, packet);
                }
            }
            for &pid in mob.viewers.difference(&watching) {
                self.send(pid, items::remove_entity(id));
            }
            mob.viewers = watching;

            let delta = [0, 1, 2].map(|i| ((mob.pos[i] - mob.sent_pos[i]) * 4096.0).round());
            let angles = (angle(mob.yaw), angle(mob.head_yaw));
            let moved = delta.iter().any(|&d| d != 0.0);
            if moved {
                let packet = if delta.iter().all(|d| d.abs() <= f64::from(i16::MAX)) {
                    for (sent, d) in mob.sent_pos.iter_mut().zip(delta) {
                        *sent += d / 4096.0;
                    }
                    Writer::packet(clientbound::MOVE_ENTITY_POS_ROT)
                        .varint(id)
                        .i16(delta[0] as i16)
                        .i16(delta[1] as i16)
                        .i16(delta[2] as i16)
                        .u8(angles.0)
                        .u8(0)
                        .bool(mob.on_ground)
                } else {
                    mob.sent_pos = mob.pos;
                    Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                        .varint(id)
                        .f64(mob.pos[0])
                        .f64(mob.pos[1])
                        .f64(mob.pos[2])
                        .f64(0.0)
                        .f64(0.0)
                        .f64(0.0)
                        .f32(mob.yaw)
                        .f32(0.0)
                        .bool(mob.on_ground)
                };
                self.send_to_viewers(&mob, packet);
            } else if angles.0 != mob.sent_angles.0 {
                self.send_to_viewers(
                    &mob,
                    Writer::packet(clientbound::MOVE_ENTITY_ROT)
                        .varint(id)
                        .u8(angles.0)
                        .u8(0)
                        .bool(mob.on_ground),
                );
            }
            if angles.1 != mob.sent_angles.1 {
                self.send_to_viewers(
                    &mob,
                    Writer::packet(clientbound::ROTATE_HEAD)
                        .varint(id)
                        .u8(angles.1),
                );
            }
            mob.sent_angles = angles;
            self.mobs.insert(id, mob);
        }
    }

    /// A player hits a mob with what they hold. Damage follows the held item and how far
    /// the attack has recharged; the mob is knocked back and panics.
    pub(super) fn attack_mob(&mut self, attacker: i32, target: i32) {
        let Some(p) = self.players.get(&attacker) else {
            return;
        };
        let Some(mob) = self.mobs.get(&target) else {
            return;
        };
        if !p.alive || mob.dying.is_some() || !within_reach(p, mob) {
            return;
        }
        let slot = p.hand_slot(0);
        let item = ItemInfo::get(p.inventory.slots[slot].item);
        let (damage, speed) = item.map_or((1.0, 4.0), |i| (i.attack_damage, i.attack_speed));
        let cooldown = 20.0 / f64::from(speed);
        let charge = ((self.tick.saturating_sub(p.attack_reset) as f64 + 0.5) / cooldown).min(1.0);
        let mut damage = damage * (0.2 + (charge * charge) as f32 * 0.8);
        let critical = charge > 0.9 && p.fall_distance > 0.0 && !p.on_ground;
        if critical {
            damage *= 1.5;
        }
        let yaw = f64::from(p.yaw).to_radians();
        let is_weapon = item.is_some_and(|i| {
            i.name.ends_with("_sword") || i.name.ends_with("_axe") || i.name.ends_with("_trident")
        });

        let p = self.players.get_mut(&attacker).expect("player exists");
        p.attack_reset = self.tick;
        p.exhaustion += 0.1;

        let mut mob = self.mobs.remove(&target).expect("checked");
        // A mob still recovering from a hit only takes what a harder hit adds.
        let dealt = if mob.hurt_ticks > 0 {
            damage - mob.last_damage
        } else {
            damage
        };
        if dealt <= 0.0 {
            self.mobs.insert(target, mob);
            return;
        }
        if mob.hurt_ticks == 0 {
            mob.last_damage = damage;
        }
        mob.hurt_ticks = HURT_TICKS;
        mob.health -= dealt;
        mob.panic = PANIC_TICKS;
        mob.target = None;
        // Knocked away from the attacker.
        let (kx, kz) = (-yaw.sin(), yaw.cos());
        mob.vel[0] = mob.vel[0] / 2.0 + kx * KNOCKBACK;
        mob.vel[2] = mob.vel[2] / 2.0 + kz * KNOCKBACK;
        if mob.on_ground {
            mob.vel[1] = (mob.vel[1] / 2.0 + KNOCKBACK).min(KNOCKBACK);
        }
        let damage_type = SyncedRegistry::get("minecraft:damage_type")
            .and_then(|r| r.id_of("minecraft:player_attack"))
            .unwrap_or(0);
        self.send_to_viewers(
            &mob,
            Writer::packet(clientbound::DAMAGE_EVENT)
                .varint(target)
                .varint(damage_type)
                .varint(attacker + 1)
                .varint(attacker + 1)
                .bool(false),
        );
        if critical {
            self.send_to_viewers(
                &mob,
                Writer::packet(clientbound::ANIMATE).varint(target).u8(4),
            );
        }
        if is_weapon {
            self.damage_item(attacker, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
        }
        if mob.health <= 0.0 {
            self.kill_mob(target, &mut mob);
        } else {
            let sound = format!("minecraft:entity.{}.hurt", mob.kind.sound());
            self.mob_sound(&mob, &sound);
        }
        self.mobs.insert(target, mob);
    }

    fn kill_mob(&mut self, id: i32, mob: &mut Mob) {
        if mob.dying.is_some() {
            return;
        }
        mob.dying = Some(0);
        mob.health = 0.0;
        self.send_to_viewers(mob, entity_event(id, ENTITY_EVENT_MOB_DEATH));
        let sound = format!("minecraft:entity.{}.death", mob.kind.sound());
        self.mob_sound(mob, &sound);
        let drops = self.loot.entity_drops(
            mob.kind.name(),
            &mut LootContext {
                state: block::AIR,
                tool: 0,
                rng: &mut self.rng,
                entity: mob.loot_props(),
            },
        );
        for (item, count) in drops {
            self.scatter_item(mob.pos, Stack::new(item, count));
        }
    }

    /// A player uses an item on a mob: shears on a sheep, a bucket on a cow.
    pub(super) fn use_on_mob(&mut self, id: i32, target: i32, hand: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let Some(mob) = self.mobs.get(&target) else {
            return;
        };
        if !p.alive || mob.dying.is_some() || !within_reach(p, mob) {
            return;
        }
        let slot = p.hand_slot(hand);
        let held = ItemInfo::get(p.inventory.slots[slot].item).map_or("", |i| i.name);
        match (mob.kind, held) {
            (MobKind::Sheep, "minecraft:shears") if !mob.sheared => {
                let mut mob = self.mobs.remove(&target).expect("checked");
                mob.sheared = true;
                let wool = format!("minecraft:{}_wool", COLORS[usize::from(mob.color)]);
                let count = 1 + self.rng.below(3) as i32;
                if let Some(item) = ItemInfo::id_of(&wool) {
                    self.scatter_item(mob.pos, Stack::new(item, count));
                }
                self.mob_sound(&mob, "minecraft:entity.sheep.shear");
                self.send_to_viewers(&mob, mob.wool_data(target));
                self.mobs.insert(target, mob);
                if self.players[&id].survival() {
                    self.damage_item(id, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
                }
            }
            (MobKind::Cow, "minecraft:bucket") => {
                let pos = mob.pos;
                self.play_sound_at("minecraft:entity.cow.milk", pos, 1.0, 1.0, None);
                self.exchange_held(id, slot, "minecraft:milk_bucket");
            }
            _ => {}
        }
    }

    /// Mobs in a chunk being saved or unloaded, in the vanilla entity chunk form.
    fn mob_chunk_nbt(&self, cx: i32, cz: i32) -> Compound {
        let entities: Vec<Tag> = self
            .mobs
            .values()
            .filter(|m| m.dying.is_none() && m.chunk() == (cx, cz))
            .map(|m| Tag::Compound(m.to_nbt()))
            .collect();
        Compound::new()
            .with("DataVersion", DATA_VERSION)
            .with("Position", vec![cx, cz])
            .with("Entities", entities)
    }

    /// Saves the mobs of every loaded chunk that has (or had) any.
    pub(super) fn save_mobs(&mut self) {
        let now: HashSet<(i32, i32)> = self.mobs.values().map(Mob::chunk).collect();
        let chunks: Vec<(i32, i32)> = now
            .union(&self.mob_chunks)
            .copied()
            .filter(|c| self.chunks.contains_key(c))
            .collect();
        for (x, z) in chunks {
            self.save_mob_chunk(x, z);
        }
        self.mob_chunks = now;
    }

    fn save_mob_chunk(&mut self, x: i32, z: i32) {
        let nbt = self.mob_chunk_nbt(x, z);
        if let Err(e) = self.entity_storage.write(x, z, &nbt) {
            error!("could not save the mobs of chunk {x},{z}: {e}");
        }
    }

    /// Saves and removes the mobs of a chunk that is being unloaded.
    pub(super) fn unload_mobs(&mut self, x: i32, z: i32) {
        let had_mobs = self.mob_chunks.remove(&(x, z));
        let ids: Vec<i32> = self
            .mobs
            .iter()
            .filter(|(_, m)| m.chunk() == (x, z))
            .map(|(&id, _)| id)
            .collect();
        if ids.is_empty() && !had_mobs {
            return;
        }
        self.save_mob_chunk(x, z);
        for id in ids {
            let mob = self.mobs.remove(&id).expect("listed");
            self.send_to_viewers(&mob, items::remove_entity(id));
        }
    }

    /// Brings back the mobs saved with a chunk that was just loaded.
    pub(super) fn load_mobs(&mut self, x: i32, z: i32) {
        let nbt = match self.entity_storage.read(x, z) {
            Ok(Some(nbt)) => nbt,
            Ok(None) => return,
            Err(e) => {
                error!("the mobs of chunk {x},{z} are unreadable: {e}");
                return;
            }
        };
        for entity in nbt.list("Entities").unwrap_or_default() {
            if let Some(mob) = entity
                .as_compound()
                .and_then(|e| Mob::from_nbt(e, &mut self.rng))
            {
                self.mob_chunks.insert(mob.chunk());
                self.mobs.insert(next_entity_id(), mob);
            }
        }
    }

    /// Summons a mob next to a player (`/summon`).
    pub(super) fn summon(&mut self, id: i32, name: &str) -> String {
        let full = if name.contains(':') {
            name.to_owned()
        } else {
            format!("minecraft:{name}")
        };
        let Some(kind) = MobKind::from_name(&full) else {
            return format!("Cannot summon {full} yet: cow, pig, sheep and chicken work");
        };
        let Some(p) = self.players.get(&id) else {
            return String::new();
        };
        let pos = p.pos;
        let mob = Mob::new(kind, pos, &mut self.rng);
        self.mobs.insert(next_entity_id(), mob);
        format!("Summoned new {}", kind.sound())
    }
}

fn feet(pos: [f64; 3]) -> BlockPos {
    BlockPos::new(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    )
}

/// The yaw (degrees, Minecraft's convention) that faces from `from` towards `to`.
fn yaw_towards(from: [f64; 3], to: [f64; 3]) -> f32 {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    (-dx.atan2(dz)).to_degrees() as f32
}

fn within_reach(p: &Player, mob: &Mob) -> bool {
    let (eye, _, _) = p.eye_ray();
    let height = mob.kind.size().1;
    let centre = [mob.pos[0], mob.pos[1] + height / 2.0, mob.pos[2]];
    let d: f64 = (0..3).map(|i| (eye[i] - centre[i]).powi(2)).sum();
    d.sqrt() <= REACH
}
