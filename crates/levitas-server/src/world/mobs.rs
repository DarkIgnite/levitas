//! Mobs: the farm animals (cows, pigs, sheep, chickens) and the first monsters
//! (zombies, spiders, creepers; their hunting lives in [`super::monsters`]).
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
use crate::enchanting::Enchantments;
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
const DATA_SLIME_SIZE: u8 = 16;
const SERIALIZER_INT: i32 = 1;
/// How long an enderman stays angry after being looked at or hit.
pub const ENDERMAN_ANGER: u32 = 600;
/// How far a hit zombified piglin's anger spreads to others.
const PIGLIN_ALERT_RANGE: f64 = 20.0;
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
    Zombie,
    Spider,
    Creeper,
    Skeleton,
    ZombifiedPiglin,
    MagmaCube,
    Ghast,
    Blaze,
    Enderman,
    Villager,
    IronGolem,
}

impl MobKind {
    pub fn from_name(name: &str) -> Option<MobKind> {
        match name {
            "minecraft:cow" => Some(MobKind::Cow),
            "minecraft:pig" => Some(MobKind::Pig),
            "minecraft:sheep" => Some(MobKind::Sheep),
            "minecraft:chicken" => Some(MobKind::Chicken),
            "minecraft:zombie" => Some(MobKind::Zombie),
            "minecraft:spider" => Some(MobKind::Spider),
            "minecraft:creeper" => Some(MobKind::Creeper),
            "minecraft:skeleton" => Some(MobKind::Skeleton),
            "minecraft:zombified_piglin" => Some(MobKind::ZombifiedPiglin),
            "minecraft:magma_cube" => Some(MobKind::MagmaCube),
            "minecraft:ghast" => Some(MobKind::Ghast),
            "minecraft:blaze" => Some(MobKind::Blaze),
            "minecraft:enderman" => Some(MobKind::Enderman),
            "minecraft:villager" => Some(MobKind::Villager),
            "minecraft:iron_golem" => Some(MobKind::IronGolem),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MobKind::Cow => "minecraft:cow",
            MobKind::Pig => "minecraft:pig",
            MobKind::Sheep => "minecraft:sheep",
            MobKind::Chicken => "minecraft:chicken",
            MobKind::Zombie => "minecraft:zombie",
            MobKind::Spider => "minecraft:spider",
            MobKind::Creeper => "minecraft:creeper",
            MobKind::Skeleton => "minecraft:skeleton",
            MobKind::ZombifiedPiglin => "minecraft:zombified_piglin",
            MobKind::MagmaCube => "minecraft:magma_cube",
            MobKind::Ghast => "minecraft:ghast",
            MobKind::Blaze => "minecraft:blaze",
            MobKind::Enderman => "minecraft:enderman",
            MobKind::Villager => "minecraft:villager",
            MobKind::IronGolem => "minecraft:iron_golem",
        }
    }

    /// The name in death messages.
    pub fn display(self) -> &'static str {
        match self {
            MobKind::Cow => "Cow",
            MobKind::Pig => "Pig",
            MobKind::Sheep => "Sheep",
            MobKind::Chicken => "Chicken",
            MobKind::Zombie => "Zombie",
            MobKind::Spider => "Spider",
            MobKind::Creeper => "Creeper",
            MobKind::Skeleton => "Skeleton",
            MobKind::ZombifiedPiglin => "Zombified Piglin",
            MobKind::MagmaCube => "Magma Cube",
            MobKind::Ghast => "Ghast",
            MobKind::Blaze => "Blaze",
            MobKind::Enderman => "Enderman",
            MobKind::Villager => "Villager",
            MobKind::IronGolem => "Iron Golem",
        }
    }

    pub fn is_monster(self) -> bool {
        matches!(
            self,
            MobKind::Zombie
                | MobKind::Spider
                | MobKind::Creeper
                | MobKind::Skeleton
                | MobKind::ZombifiedPiglin
                | MobKind::MagmaCube
                | MobKind::Ghast
                | MobKind::Blaze
                | MobKind::Enderman
        )
    }

    /// Whether fire and lava leave it unhurt.
    pub fn fire_immune(self) -> bool {
        matches!(
            self,
            MobKind::ZombifiedPiglin | MobKind::MagmaCube | MobKind::Ghast | MobKind::Blaze
        )
    }

    /// Whether it flies instead of walking.
    pub fn flies(self) -> bool {
        matches!(self, MobKind::Ghast | MobKind::Blaze)
    }

    /// Melee damage on Normal difficulty.
    pub fn attack_damage(self) -> f32 {
        match self {
            MobKind::Zombie => 3.0,
            MobKind::Spider => 2.0,
            // With its golden sword.
            MobKind::ZombifiedPiglin => 8.0,
            MobKind::Blaze => 6.0,
            MobKind::Enderman => 7.0,
            _ => 0.0,
        }
    }

    /// How far away a monster notices players.
    pub fn follow_range(self) -> f64 {
        match self {
            MobKind::Zombie | MobKind::ZombifiedPiglin => 35.0,
            MobKind::Ghast | MobKind::Enderman => 64.0,
            MobKind::Blaze => 48.0,
            _ => 16.0,
        }
    }

    pub fn burns_in_daylight(self) -> bool {
        matches!(self, MobKind::Zombie | MobKind::Skeleton)
    }

    fn entity_type(self) -> i32 {
        match self {
            MobKind::Cow => entity_type::COW,
            MobKind::Pig => entity_type::PIG,
            MobKind::Sheep => entity_type::SHEEP,
            MobKind::Chicken => entity_type::CHICKEN,
            MobKind::Zombie => entity_type::ZOMBIE,
            MobKind::Spider => entity_type::SPIDER,
            MobKind::Creeper => entity_type::CREEPER,
            MobKind::Skeleton => entity_type::SKELETON,
            MobKind::ZombifiedPiglin => entity_type::ZOMBIFIED_PIGLIN,
            MobKind::MagmaCube => entity_type::MAGMA_CUBE,
            MobKind::Ghast => entity_type::GHAST,
            MobKind::Blaze => entity_type::BLAZE,
            MobKind::Enderman => entity_type::ENDERMAN,
            MobKind::Villager => entity_type::VILLAGER,
            MobKind::IronGolem => entity_type::IRON_GOLEM,
        }
    }

    pub fn max_health(self) -> f32 {
        match self {
            MobKind::Cow | MobKind::Pig => 10.0,
            MobKind::Sheep => 8.0,
            MobKind::Chicken => 4.0,
            MobKind::Zombie | MobKind::Creeper | MobKind::Skeleton => 20.0,
            MobKind::ZombifiedPiglin => 20.0,
            MobKind::Spider => 16.0,
            // A magma cube's health is its size squared; this is the smallest's.
            MobKind::MagmaCube => 1.0,
            MobKind::Ghast => 10.0,
            MobKind::Blaze => 20.0,
            MobKind::Enderman => 40.0,
            MobKind::Villager => 20.0,
            MobKind::IronGolem => 100.0,
        }
    }

    /// Width and height of the hitbox.
    pub fn size(self) -> (f64, f64) {
        match self {
            MobKind::Cow => (0.9, 1.4),
            MobKind::Pig => (0.9, 0.9),
            MobKind::Sheep => (0.9, 1.3),
            MobKind::Chicken => (0.4, 0.7),
            MobKind::Zombie => (0.6, 1.95),
            MobKind::Spider => (1.4, 0.9),
            MobKind::Creeper => (0.6, 1.7),
            MobKind::Skeleton => (0.6, 1.99),
            MobKind::ZombifiedPiglin => (0.6, 1.95),
            MobKind::MagmaCube => (0.52, 0.52),
            MobKind::Ghast => (4.0, 4.0),
            MobKind::Blaze => (0.6, 1.8),
            MobKind::Enderman => (0.6, 2.9),
            MobKind::Villager => (0.6, 1.95),
            MobKind::IronGolem => (1.4, 2.7),
        }
    }

    /// Walking speed in blocks per tick.
    fn speed(self) -> f64 {
        match self {
            MobKind::Chicken => 0.08,
            MobKind::Zombie => 0.12,
            MobKind::Spider => 0.15,
            MobKind::Creeper => 0.11,
            MobKind::Skeleton | MobKind::ZombifiedPiglin => 0.12,
            // Magma cubes only move by hopping.
            MobKind::MagmaCube => 0.0,
            MobKind::Ghast => 0.08,
            MobKind::Blaze => 0.06,
            MobKind::Enderman => 0.15,
            _ => 0.1,
        }
    }

    /// The sound family, as in `minecraft:entity.<family>.hurt`.
    pub fn sound(self) -> &'static str {
        match self {
            MobKind::Cow => "cow",
            MobKind::Pig => "pig",
            MobKind::Sheep => "sheep",
            MobKind::Chicken => "chicken",
            MobKind::Zombie => "zombie",
            MobKind::Spider => "spider",
            MobKind::Creeper => "creeper",
            MobKind::Skeleton => "skeleton",
            MobKind::ZombifiedPiglin => "zombified_piglin",
            MobKind::MagmaCube => "magma_cube",
            MobKind::Ghast => "ghast",
            MobKind::Blaze => "blaze",
            MobKind::Enderman => "enderman",
            MobKind::Villager => "villager",
            MobKind::IronGolem => "iron_golem",
        }
    }
}

pub struct Mob {
    pub kind: MobKind,
    uuid: u128,
    pub pos: [f64; 3],
    vel: [f64; 3],
    /// Sideways speed from knockback and explosions, fading with friction.
    knockback: [f64; 2],
    pub yaw: f32,
    pub head_yaw: f32,
    on_ground: bool,
    pub health: f32,
    fall_distance: f64,
    hurt_ticks: u32,
    last_damage: f32,
    dying: Option<u32>,
    panic: u32,
    /// Where the mob is walking to, and for how many more ticks it tries.
    pub target: Option<([f64; 2], u32)>,
    /// Monsters: the player being hunted, ticks until the next attack, ticks left on
    /// fire, and how long a creeper has been hissing.
    pub hunting: Option<i32>,
    pub attack_cooldown: u32,
    pub fire_ticks: u32,
    pub fuse: Option<u32>,
    /// Whether a player dealt the last hit (some loot needs it).
    killed_by_player: bool,
    /// The weapon enchantments of the player who last hit it, for looting.
    looting: Enchantments,
    /// Negative while a baby (it grows up at 0), positive while resting after breeding.
    pub age: i32,
    /// Ticks of love mode left.
    pub love: u32,
    /// The spots still to walk through towards a goal, and ticks until the path is
    /// worked out again.
    pub path: Vec<BlockPos>,
    pub path_ticks: u32,
    blocked: bool,
    ambient: i32,
    /// Sheep: wool colour (0-15) and whether sheared.
    pub color: u8,
    pub sheared: bool,
    /// Chicken: ticks until the next egg.
    egg_ticks: u32,
    /// Zombified piglins: ticks left of anger.
    pub anger: u32,
    /// Magma cubes: size (1, 2 or 4) and ticks until the next hop.
    pub slime_size: u8,
    jump_cooldown: u32,
    /// Ghasts and blazes: where they are flying to, how their attack is coming on, and
    /// how many fireballs of a blaze's volley are still to come.
    pub fly_goal: Option<[f64; 3]>,
    pub charge: i32,
    pub volley: u8,
    /// What players were last told.
    sent_pos: [f64; 3],
    sent_angles: (u8, u8),
    /// Players who can see this mob.
    viewers: HashSet<i32>,
    /// A villager's profession, trades and workstation.
    pub villager: Option<Box<super::villagers::Villager>>,
    /// Iron golems: the monster being fought, and whether a player built it.
    pub prey: Option<i32>,
    pub player_created: bool,
    /// A name given by a name tag.
    pub name: Option<&'static str>,
}

impl Mob {
    pub fn new(kind: MobKind, pos: [f64; 3], rng: &mut Rng) -> Mob {
        let slime_size: u8 = if kind == MobKind::MagmaCube {
            1 << rng.below(3)
        } else {
            1
        };
        let health = if kind == MobKind::MagmaCube {
            f32::from(slime_size * slime_size)
        } else {
            kind.max_health()
        };
        Mob {
            kind,
            uuid: (u128::from(rng.next_u64()) << 64) | u128::from(rng.next_u64()),
            pos,
            vel: [0.0; 3],
            knockback: [0.0; 2],
            yaw: rng.f64() as f32 * 360.0,
            head_yaw: 0.0,
            on_ground: false,
            health,
            fall_distance: 0.0,
            hurt_ticks: 0,
            last_damage: 0.0,
            dying: None,
            panic: 0,
            target: None,
            hunting: None,
            attack_cooldown: 0,
            fire_ticks: 0,
            fuse: None,
            killed_by_player: false,
            looting: Enchantments::NONE,
            age: 0,
            love: 0,
            path: Vec::new(),
            path_ticks: 0,
            blocked: false,
            ambient: 0,
            color: if kind == MobKind::Sheep {
                random_wool(rng)
            } else {
                0
            },
            sheared: false,
            egg_ticks: 6000 + rng.below(6000),
            anger: 0,
            slime_size,
            jump_cooldown: 0,
            fly_goal: None,
            charge: 0,
            volley: 0,
            sent_pos: pos,
            sent_angles: (0, 0),
            viewers: HashSet::new(),
            villager: (kind == MobKind::Villager)
                .then(|| Box::new(super::villagers::Villager::new("minecraft:plains"))),
            prey: None,
            player_created: false,
            name: None,
        }
    }

    /// Hitbox width and height: babies are half size, magma cubes grow with theirs.
    pub fn size(&self) -> (f64, f64) {
        let (width, height) = self.kind.size();
        if self.kind == MobKind::MagmaCube {
            let s = f64::from(self.slime_size);
            return (width * s, height * s);
        }
        if self.age < 0 {
            (width / 2.0, height / 2.0)
        } else {
            (width, height)
        }
    }

    /// Melee damage: a magma cube's grows with its size.
    pub fn attack_damage(&self) -> f32 {
        match (self.kind, self.slime_size) {
            (MobKind::MagmaCube, 1) => 3.0,
            (MobKind::MagmaCube, 2) => 4.0,
            (MobKind::MagmaCube, _) => 6.0,
            (kind, _) => kind.attack_damage(),
        }
    }

    pub fn alive(&self) -> bool {
        self.dying.is_none()
    }

    /// Adds to the mob's velocity (knockback).
    /// Pushes the mob (knockback, explosions): sideways on top of its walking, which
    /// does not cancel it, and up through its own velocity.
    pub fn push(&mut self, by: [f64; 3]) {
        self.knockback[0] += by[0];
        self.knockback[1] += by[2];
        self.vel[1] += by[1];
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
        if let Some(v) = &self.villager {
            packets.push(v.data_packet(id));
        }
        if self.kind == MobKind::IronGolem {
            packets.push(super::golems::golem_data(id, self.player_created));
        }
        if let Some(name) = self.name {
            packets.push(name_data(id, name));
        }
        if self.age < 0 {
            packets.push(super::breeding::baby_data(id, true));
        }
        if self.kind == MobKind::MagmaCube {
            packets.push(
                Writer::packet(clientbound::SET_ENTITY_DATA)
                    .varint(id)
                    .u8(DATA_SLIME_SIZE)
                    .varint(SERIALIZER_INT)
                    .varint(i32::from(self.slime_size))
                    .u8(0xFF),
            );
        }
        if self.kind == MobKind::ZombifiedPiglin {
            if let Some(sword) = ItemInfo::id_of("minecraft:golden_sword") {
                let packet = Writer::packet(clientbound::SET_EQUIPMENT).varint(id).u8(0);
                packets.push(crate::inventory::write_stack(packet, Stack::new(sword, 1)));
            }
        }
        if self.kind == MobKind::Skeleton {
            // Skeletons hold a bow.
            if let Some(bow) = ItemInfo::id_of("minecraft:bow") {
                let packet = Writer::packet(clientbound::SET_EQUIPMENT).varint(id).u8(0); // main hand, the last entry
                packets.push(crate::inventory::write_stack(packet, Stack::new(bow, 1)));
            }
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
            killed_by_player: self.killed_by_player,
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
            .with("Age", self.age)
            .with("InLove", self.love as i32);
        match self.kind {
            MobKind::Sheep => {
                nbt.insert("Color", self.color as i8);
                nbt.insert("Sheared", self.sheared);
            }
            MobKind::Chicken => nbt.insert("EggLayTime", self.egg_ticks as i32),
            MobKind::MagmaCube => nbt.insert("Size", i32::from(self.slime_size) - 1),
            MobKind::ZombifiedPiglin => nbt.insert("AngerTime", self.anger as i32),
            _ => {}
        }
        if let Some(v) = &self.villager {
            v.write_nbt(&mut nbt);
        }
        if self.kind == MobKind::IronGolem {
            nbt.insert("PlayerCreated", self.player_created);
        }
        if let Some(name) = self.name {
            nbt.insert("CustomName", name);
        }
        nbt
    }

    /// A mob from its saved form, or a structure's.
    pub(super) fn read(nbt: &Compound, rng: &mut Rng) -> Option<Mob> {
        Mob::from_nbt(nbt, rng)
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
        if let Some(size) = nbt.int("Size").filter(|_| kind == MobKind::MagmaCube) {
            mob.slime_size = (size.clamp(0, 3) as u8 + 1).next_power_of_two().min(4);
        }
        mob.anger = nbt.int("AngerTime").unwrap_or(0).max(0) as u32;
        mob.health = nbt.float("Health").map_or(kind.max_health(), |h| h as f32);
        if mob.health <= 0.0 {
            return None;
        }
        mob.on_ground = nbt.int("OnGround").unwrap_or(1) != 0;
        if kind == MobKind::Sheep {
            mob.color = nbt.int("Color").unwrap_or(0).clamp(0, 15) as u8;
            mob.sheared = nbt.int("Sheared").unwrap_or(0) != 0;
        }
        mob.age = nbt.int("Age").unwrap_or(0) as i32;
        mob.love = nbt.int("InLove").unwrap_or(0).max(0) as u32;
        if let Some(t) = nbt.int("EggLayTime") {
            mob.egg_ticks = t.max(0) as u32;
        }
        if kind == MobKind::Villager {
            mob.villager = Some(Box::new(super::villagers::Villager::read_nbt(nbt)));
        }
        mob.player_created = nbt.int("PlayerCreated").unwrap_or(0) != 0;
        mob.name = nbt
            .get("CustomName")
            .map(crate::names::plain_text)
            .filter(|n| !n.is_empty())
            .and_then(|n| crate::names::intern(&n));
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

/// What each biome spawns: animals once when its chunks are generated, monsters in
/// the dark.
pub struct BiomeSpawns {
    by_biome: HashMap<String, SpawnList>,
}

struct SpawnList {
    creature_probability: f64,
    creatures: Vec<Spawner>,
    monsters: Vec<Spawner>,
}

/// Mobs `/summon` knows.
pub const SUMMONABLE: [&str; 15] = [
    "cow",
    "pig",
    "sheep",
    "chicken",
    "zombie",
    "spider",
    "creeper",
    "skeleton",
    "zombified_piglin",
    "magma_cube",
    "ghast",
    "blaze",
    "enderman",
    "villager",
    "iron_golem",
];

fn spawners(list: &Value) -> Vec<Spawner> {
    list.as_array()
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
        .collect()
}

/// A weighted pick from a spawn list, with a group size.
fn pick(list: &[Spawner], rng: &mut Rng) -> Option<(MobKind, u32)> {
    let total: u32 = list.iter().map(|s| s.weight).sum();
    if total == 0 {
        return None;
    }
    let mut roll = rng.below(total);
    let spawner = list.iter().find(|s| {
        if roll < s.weight {
            true
        } else {
            roll -= s.weight;
            false
        }
    })?;
    let size = spawner.min + rng.below(spawner.max.saturating_sub(spawner.min) + 1);
    Some((spawner.kind, size.min(4)))
}

impl BiomeSpawns {
    pub fn load() -> Self {
        let biomes: serde_json::Map<String, Value> =
            serde_json::from_str(BIOMES_JSON).expect("biomes are valid JSON");
        let by_biome = biomes
            .into_iter()
            .map(|(name, biome)| {
                let list = SpawnList {
                    creature_probability: biome["creature_spawn_probability"]
                        .as_f64()
                        .unwrap_or(DEFAULT_CREATURE_PROBABILITY),
                    creatures: spawners(&biome["spawners"]["creature"]),
                    monsters: spawners(&biome["spawners"]["monster"]),
                };
                (name, list)
            })
            .collect();
        BiomeSpawns { by_biome }
    }

    /// A monster group for a biome, among the monsters Levitas has.
    pub fn pick_monster(&self, biome: &str, rng: &mut Rng) -> Option<(MobKind, u32)> {
        pick(&self.by_biome.get(biome)?.monsters, rng)
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
        let Some(list) = self.biome_spawns.by_biome.get(&biome) else {
            return;
        };
        let probability = list.creature_probability;
        let mut groups = Vec::new();
        while self.rng.f64() < probability && groups.len() < 4 {
            let list = &self.biome_spawns.by_biome[&biome];
            match pick(&list.creatures, &mut self.rng) {
                Some(group) => groups.push(group),
                None => break,
            }
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
                    if !self.mob_fits(kind, pos) {
                        continue;
                    }
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
    pub(super) fn biome_at(&self, x: i32, y: i32, z: i32) -> Option<String> {
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
            // Peaceful leaves no monsters about.
            let peaceful = crate::rules::difficulty() == crate::rules::Difficulty::Peaceful;
            if self.despawns(&mob) || (peaceful && mob.kind.is_monster()) {
                self.send_to_viewers(&mob, items::remove_entity(id));
                continue;
            }
            self.think(&mut mob);
            if mob.kind == MobKind::Villager {
                self.tick_villager(id, &mut mob);
                if mob.villager.as_ref().is_some_and(|v| v.customer.is_some()) {
                    mob.target = None;
                    mob.path.clear();
                }
            }
            if mob.kind == MobKind::IronGolem {
                self.tick_golem(id, &mut mob);
            } else if !mob.kind.is_monster() {
                self.tick_breeding(id, &mut mob);
            }
            if mob.kind.is_monster() && self.hunt(id, &mut mob) {
                continue; // exploded
            }
            if mob.kind == MobKind::MagmaCube {
                self.hop(&mut mob);
            }
            if mob.kind.flies() {
                self.fly_mob(&mut mob);
            } else {
                self.step(&mut mob);
            }
            self.burn_in_daylight(id, &mut mob);
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
        // Food in a player's hand beats wandering; love sends them to their partner.
        if !panicking && !mob.kind.is_monster() && mob.love == 0 && self.follow_temptation(mob) {
            return;
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
        let eyes = [mob.pos[0], mob.pos[1] + mob.size().1 * 0.85, mob.pos[2]];
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

    /// An enderman vanishes and turns up somewhere within `range` of `around`: on solid
    /// ground with room for it and out of water. Returns whether it found a spot.
    pub(super) fn teleport_enderman(
        &mut self,
        mob: &mut Mob,
        around: [f64; 3],
        range: f64,
    ) -> bool {
        for _ in 0..16 {
            let r = |rng: &mut Rng| (rng.f64() - 0.5) * 2.0 * range;
            let x = (around[0] + r(&mut self.rng)).floor() as i32;
            let z = (around[2] + r(&mut self.rng)).floor() as i32;
            let mut y = (around[1] + r(&mut self.rng) / 2.0).floor() as i32;
            // Down to the ground.
            while y > MIN_Y
                && self
                    .loaded_state(BlockPos::new(x, y - 1, z))
                    .is_some_and(is_air)
            {
                y -= 1;
            }
            let ground = self.loaded_state(BlockPos::new(x, y - 1, z));
            let feet = self.loaded_state(BlockPos::new(x, y, z));
            let solid = ground.is_some_and(|g| crate::blocks::sturdy(g, crate::blocks::Dir::Up));
            let wet = feet.and_then(fluid_at).is_some();
            let at = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
            if solid && !wet && self.mob_fits(MobKind::Enderman, at) {
                let from = mob.pos;
                mob.pos = at;
                mob.path.clear();
                mob.target = None;
                self.play_sound_at("minecraft:entity.enderman.teleport", from, 1.0, 1.0, None);
                self.play_sound_at("minecraft:entity.enderman.teleport", at, 1.0, 1.0, None);
                return true;
            }
        }
        false
    }

    /// Magma cubes move only by hopping: towards their prey, or anywhere, now and then.
    fn hop(&mut self, mob: &mut Mob) {
        mob.target = None;
        mob.jump_cooldown = mob.jump_cooldown.saturating_sub(1);
        if !mob.on_ground || mob.jump_cooldown > 0 {
            return;
        }
        let prey = mob
            .hunting
            .and_then(|pid| self.players.get(&pid))
            .map(|p| p.pos);
        let yaw = match prey {
            Some(p) => f64::from(yaw_towards(mob.pos, p)),
            None => self.rng.f64() * 360.0,
        }
        .to_radians();
        let s = f64::from(mob.slime_size);
        mob.yaw = yaw.to_degrees() as f32;
        mob.vel[1] = 0.42 + 0.1 * s;
        let push = 0.1 + 0.05 * s;
        mob.knockback = [-yaw.sin() * push, yaw.cos() * push];
        // Angry cubes hop three times as often.
        let wait = 10 + self.rng.below(20);
        mob.jump_cooldown = if prey.is_some() { wait / 3 } else { wait };
        self.mob_sound(mob, "minecraft:entity.magma_cube.jump");
    }

    /// Flying mobs drift towards their goal, picking a new one now and then; with prey
    /// they keep near it (a blaze a little above, a ghast at a distance).
    fn fly_mob(&mut self, mob: &mut Mob) {
        mob.target = None;
        let (wander, speed) = match mob.kind {
            MobKind::Ghast => (16.0, mob.kind.speed()),
            _ => (6.0, mob.kind.speed()),
        };
        let prey = mob
            .hunting
            .and_then(|pid| self.players.get(&pid))
            .map(|p| p.pos);
        let reached = mob
            .fly_goal
            .is_none_or(|g| (0..3).map(|i| (g[i] - mob.pos[i]).powi(2)).sum::<f64>() < 1.0);
        if let (MobKind::Blaze, Some(p)) = (mob.kind, prey) {
            mob.fly_goal = Some([p[0], p[1] + 1.0 + self.rng.f64() * 2.0, p[2]]);
        } else if reached || self.rng.below(100) == 0 {
            let r = |rng: &mut Rng| (rng.f64() * 2.0 - 1.0) * wander;
            mob.fly_goal = Some([
                mob.pos[0] + r(&mut self.rng),
                mob.pos[1] + r(&mut self.rng) / 2.0,
                mob.pos[2] + r(&mut self.rng),
            ]);
        }
        let goal = mob.fly_goal.unwrap_or(mob.pos);
        let d = [0, 1, 2].map(|i| goal[i] - mob.pos[i]);
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let move_by = if length < 0.5 {
            [0.0; 3]
        } else {
            d.map(|v| v / length * speed)
        };
        if prey.is_none() && length >= 0.5 {
            mob.yaw = yaw_towards(mob.pos, goal);
        }
        let (width, height) = mob.size();
        let half = width / 2.0;
        let mut pos = mob.pos;
        for axis in 0..3 {
            let push = match axis {
                0 => mob.knockback[0],
                2 => mob.knockback[1],
                _ => 0.0,
            };
            let delta = self.clip_move(pos, half, height, axis, move_by[axis] + push);
            if (delta - (move_by[axis] + push)).abs() > 1e-9 {
                mob.fly_goal = None; // something in the way: pick another goal
            }
            pos[axis] += delta;
        }
        for k in &mut mob.knockback {
            *k *= 0.91;
        }
        mob.pos = pos;
        mob.on_ground = false;
        mob.fall_distance = 0.0;
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
            let mut speed = mob.kind.speed() * if mob.panic > 0 { 2.0 } else { 1.0 };
            // A mob knocked into the air has little say in where it goes.
            if !mob.on_ground && mob.knockback.iter().any(|k| k.abs() > 0.05) {
                speed *= 0.2;
            }
            // Mobs keep heading for their target in the air too (over a step they
            // jumped, or up a wall a spider climbs).
            mob.vel[0] = dx / distance * speed;
            mob.vel[2] = dz / distance * speed;
            mob.yaw = yaw_towards(mob.pos, [target[0], mob.pos[1], target[1]]);
            if mob.blocked && mob.kind == MobKind::Spider {
                mob.vel[1] = 0.2; // climbing
            } else if mob.blocked && (mob.on_ground || in_water) {
                mob.vel[1] = JUMP;
            }
        } else if mob.on_ground {
            mob.vel[0] *= 0.5;
            mob.vel[2] *= 0.5;
        }

        let (width, height) = mob.size();
        let half = width / 2.0;
        let mut pos = mob.pos;
        // Vertical first, then each horizontal axis, each as far as blocks allow.
        let dy = self.clip_move(pos, half, height, 1, mob.vel[1]);
        pos[1] += dy;
        let stopped = (dy - mob.vel[1]).abs() > 1e-9;
        mob.on_ground = stopped && mob.vel[1] < 0.0;
        if stopped {
            mob.vel[1] = 0.0;
        }
        mob.blocked = false;
        for (axis, push) in [(0, 0), (2, 1)] {
            let wanted = mob.vel[axis] + mob.knockback[push];
            let d = self.clip_move(pos, half, height, axis, wanted);
            pos[axis] += d;
            if (d - wanted).abs() > 1e-9 {
                mob.blocked = true;
                mob.vel[axis] = 0.0;
                mob.knockback[push] = 0.0;
            }
        }
        // Knockback fades like a slide: quickly on the ground, slowly in the air.
        let friction = if mob.on_ground { 0.546 } else { 0.91 };
        for k in &mut mob.knockback {
            *k *= friction;
            if k.abs() < 0.003 {
                *k = 0.0;
            }
        }
        // Like vanilla, the mob moves first and gravity then slows the next move, so a
        // jump's full 0.42 counts and lifts it over a block.
        if in_water {
            // Swimming up to float at the surface.
            mob.vel[1] = (mob.vel[1] * 0.8 + 0.04).min(0.1);
        } else {
            mob.vel[1] = (mob.vel[1] - GRAVITY) * AIR_DRAG;
            if mob.kind == MobKind::Chicken && mob.vel[1] < 0.0 {
                mob.vel[1] *= 0.6; // flapping
            }
        }

        if mob.on_ground || in_water {
            let fallen = std::mem::take(&mut mob.fall_distance);
            if !in_water
                && fallen > 3.0
                && !matches!(
                    mob.kind,
                    MobKind::Chicken | MobKind::MagmaCube | MobKind::IronGolem
                )
            {
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

    /// How far a mob of half-width `half` and `height` standing at `pos` can move by
    /// `delta` along `axis` before a block stops it. Only blocks ahead count: blocks the
    /// mob already overlaps (a spider in a gap narrower than itself) never push it.
    /// Blocks in chunks that are not loaded count as solid.
    pub(super) fn clip_move(
        &self,
        pos: [f64; 3],
        half: f64,
        height: f64,
        axis: usize,
        delta: f64,
    ) -> f64 {
        if delta == 0.0 {
            return 0.0;
        }
        let eps = 1e-7;
        let min = [pos[0] - half, pos[1], pos[2] - half];
        let max = [pos[0] + half, pos[1] + height, pos[2] + half];
        // The region swept by the move.
        let mut lo_region = min;
        let mut hi_region = max;
        if delta > 0.0 {
            hi_region[axis] += delta;
        } else {
            lo_region[axis] += delta;
        }
        let mut allowed = delta;
        let range = |i: usize, below: i32| {
            (lo_region[i] + eps).floor() as i32 - below..=(hi_region[i] - eps).floor() as i32
        };
        for bx in range(0, 0) {
            // One block lower too: fences and walls stick up above their block.
            for by in range(1, 1) {
                for bz in range(2, 0) {
                    let block = BlockPos::new(bx, by, bz);
                    let boxes: &[[f32; 6]] = match self.loaded_state(block) {
                        Some(state) => collision_boxes(state),
                        None if (MIN_Y..=MAX_Y).contains(&by) => &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]],
                        None => &[],
                    };
                    let origin = [f64::from(bx), f64::from(by), f64::from(bz)];
                    for b in boxes {
                        let lo = [0, 1, 2].map(|i| origin[i] + f64::from(b[i]));
                        let hi = [0, 1, 2].map(|i| origin[i] + f64::from(b[i + 3]));
                        // It must be beside the mob on the other two axes...
                        let beside = (0..3)
                            .filter(|&i| i != axis)
                            .all(|i| min[i] < hi[i] - eps && max[i] > lo[i] + eps);
                        if !beside {
                            continue;
                        }
                        // ...and ahead of it on this one.
                        if delta > 0.0 && lo[axis] >= max[axis] - eps {
                            allowed = allowed.min(lo[axis] - max[axis]);
                        } else if delta < 0.0 && hi[axis] <= min[axis] + eps {
                            allowed = allowed.max(hi[axis] - min[axis]);
                        }
                    }
                }
            }
        }
        if delta > 0.0 {
            allowed.max(0.0)
        } else {
            allowed.min(0.0)
        }
    }

    /// Whether a mob of `kind` at `pos` would be inside a block.
    pub(super) fn mob_fits(&self, kind: MobKind, pos: [f64; 3]) -> bool {
        let (width, height) = kind.size();
        let half = width / 2.0;
        let eps = 1e-7;
        let min = [pos[0] - half, pos[1], pos[2] - half];
        let max = [pos[0] + half, pos[1] + height, pos[2] + half];
        for bx in (min[0] + eps).floor() as i32..=(max[0] - eps).floor() as i32 {
            for by in (min[1] + eps).floor() as i32 - 1..=(max[1] - eps).floor() as i32 {
                for bz in (min[2] + eps).floor() as i32..=(max[2] - eps).floor() as i32 {
                    let Some(state) = self.loaded_state(BlockPos::new(bx, by, bz)) else {
                        return false;
                    };
                    let origin = [f64::from(bx), f64::from(by), f64::from(bz)];
                    let inside = collision_boxes(state).iter().any(|b| {
                        (0..3).all(|i| {
                            min[i] < origin[i] + f64::from(b[i + 3]) - eps
                                && max[i] > origin[i] + f64::from(b[i]) + eps
                        })
                    });
                    if inside {
                        return false;
                    }
                }
            }
        }
        true
    }

    pub(super) fn loaded_state(&self, pos: BlockPos) -> Option<u16> {
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

    pub(super) fn send_to_viewers(&mut self, mob: &Mob, packet: Writer) {
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

    /// Hurts a mob: it flinches, is knocked back along `away` (a horizontal unit
    /// vector), and panics or, for a monster hit by a player, hunts them. A mob still
    /// recovering from a hit only takes what a harder hit adds. Returns whether the hit
    /// did anything.
    /// Whether a mob belongs to an entity type tag (`minecraft:undead`, ...).
    pub(super) fn mob_tagged(&self, id: i32, tag: &str) -> bool {
        let Some(mob) = self.mobs.get(&id) else {
            return false;
        };
        let name = mob.kind.name();
        let Some(kind) = entity_type::NAMES.iter().position(|n| *n == name) else {
            return false;
        };
        levitas_registry::TagRegistry::get("minecraft:entity_type")
            .is_some_and(|t| t.members(tag).contains(&(kind as i64)))
    }

    pub(super) fn hit_mob(
        &mut self,
        target: i32,
        damage: f32,
        by_player: Option<i32>,
        away: [f64; 2],
        damage_type: &str,
    ) -> bool {
        let Some(mut mob) = self.mobs.remove(&target) else {
            return false;
        };
        // Endermen dodge what is thrown or shot at them.
        if mob.kind == MobKind::Enderman
            && matches!(
                damage_type,
                "minecraft:arrow" | "minecraft:thrown" | "minecraft:fireball"
            )
        {
            let at = mob.pos;
            self.teleport_enderman(&mut mob, at, 32.0);
            self.mobs.insert(target, mob);
            return false;
        }
        let dealt = if mob.hurt_ticks > 0 {
            damage - mob.last_damage
        } else {
            damage
        };
        if dealt <= 0.0 || mob.dying.is_some() {
            self.mobs.insert(target, mob);
            return false;
        }
        if mob.hurt_ticks == 0 {
            mob.last_damage = damage;
        }
        mob.hurt_ticks = HURT_TICKS;
        mob.health -= dealt;
        let anger_piglins = by_player.filter(|_| mob.kind == MobKind::ZombifiedPiglin);
        if let (Some(attacker), MobKind::Enderman) = (by_player, mob.kind) {
            mob.anger = mob.anger.max(ENDERMAN_ANGER);
            mob.hunting = Some(attacker);
        }
        if let Some(attacker) = by_player {
            mob.killed_by_player = true;
            mob.looting = self
                .players
                .get(&attacker)
                .map_or(Enchantments::NONE, |p| p.inventory.held(p.held).enchants);
            if mob.kind.is_monster() {
                mob.hunting = Some(attacker);
            }
        }
        if mob.kind == MobKind::IronGolem {
            // Golems fight back instead of fleeing, and do not get knocked back.
            if let Some(attacker) = by_player {
                World::golem_hit_by(&mut mob, attacker);
            }
        } else {
            if !mob.kind.is_monster() {
                mob.panic = PANIC_TICKS;
                mob.target = None;
            }
            mob.knockback[0] = mob.knockback[0] / 2.0 + away[0] * KNOCKBACK;
            mob.knockback[1] = mob.knockback[1] / 2.0 + away[1] * KNOCKBACK;
            if mob.on_ground {
                mob.vel[1] = (mob.vel[1] / 2.0 + KNOCKBACK).min(KNOCKBACK);
            }
        }
        let kind = SyncedRegistry::get("minecraft:damage_type")
            .and_then(|r| r.id_of(damage_type))
            .unwrap_or(0);
        let cause = by_player.map_or(0, |p| p + 1);
        self.send_to_viewers(
            &mob,
            Writer::packet(clientbound::DAMAGE_EVENT)
                .varint(target)
                .varint(kind)
                .varint(cause)
                .varint(cause)
                .bool(false),
        );
        if mob.health <= 0.0 {
            self.kill_mob(target, &mut mob);
        } else {
            let sound = format!("minecraft:entity.{}.hurt", mob.kind.sound());
            self.mob_sound(&mob, &sound);
        }
        let at = mob.pos;
        self.mobs.insert(target, mob);
        if let Some(attacker) = anger_piglins {
            self.anger_piglins(at, attacker);
        }
        true
    }

    /// Hitting a zombified piglin angers every one around it at the attacker, for 20
    /// to 39 seconds.
    fn anger_piglins(&mut self, at: [f64; 3], attacker: i32) {
        let ids: Vec<i32> = self
            .mobs
            .iter()
            .filter(|(_, m)| {
                m.kind == MobKind::ZombifiedPiglin
                    && m.alive()
                    && (m.pos[0] - at[0]).hypot(m.pos[2] - at[2]) <= PIGLIN_ALERT_RANGE
            })
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let anger = 400 + self.rng.below(380);
            if let Some(m) = self.mobs.get_mut(&id) {
                m.anger = anger;
                m.hunting = Some(attacker);
            }
        }
    }

    fn kill_mob(&mut self, id: i32, mob: &mut Mob) {
        if mob.dying.is_some() {
            return;
        }
        mob.dying = Some(0);
        mob.health = 0.0;
        self.send_to_viewers(mob, entity_event(id, ENTITY_EVENT_MOB_DEATH));
        // A big magma cube breaks into two to four smaller ones.
        if mob.kind == MobKind::MagmaCube && mob.slime_size > 1 {
            let size = mob.slime_size / 2;
            for _ in 0..2 + self.rng.below(3) {
                let offset = |rng: &mut Rng| (rng.f64() - 0.5) * f64::from(size) * 0.5;
                let at = [
                    mob.pos[0] + offset(&mut self.rng),
                    mob.pos[1] + 0.5,
                    mob.pos[2] + offset(&mut self.rng),
                ];
                let mut small = Mob::new(MobKind::MagmaCube, at, &mut self.rng);
                small.slime_size = size;
                small.health = f32::from(size * size);
                self.mobs.insert(next_entity_id(), small);
            }
        }
        let sound = format!("minecraft:entity.{}.death", mob.kind.sound());
        self.mob_sound(mob, &sound);
        if mob.age < 0 {
            return; // babies drop nothing
        }
        let drops = self.loot.entity_drops(
            mob.kind.name(),
            &mut LootContext {
                state: block::AIR,
                tool: 0,
                rng: &mut self.rng,
                enchants: mob.looting,
                entity: mob.loot_props(),
            },
        );
        for (item, count) in drops {
            self.scatter_item(mob.pos, Stack::new(item, count));
        }
        if mob.killed_by_player && crate::rules::bool("doMobLoot") {
            self.mob_xp(mob.pos, mob.kind.is_monster());
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
        if held == "minecraft:name_tag" {
            self.name_mob(id, target, slot);
            return;
        }
        if mob.kind == MobKind::Villager {
            self.open_trading(id, target);
            return;
        }
        if mob.kind == MobKind::IronGolem {
            if held == "minecraft:iron_ingot" {
                self.repair_golem(id, target, slot);
            }
            return;
        }
        if super::breeding::is_food(mob.kind, held) {
            self.feed_animal(id, target, slot);
            return;
        }
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

    /// Mobs and dropped items in a chunk being saved or unloaded, in the vanilla entity
    /// chunk form.
    fn mob_chunk_nbt(&self, cx: i32, cz: i32) -> Compound {
        let mut entities: Vec<Tag> = self
            .mobs
            .values()
            .filter(|m| m.dying.is_none() && m.chunk() == (cx, cz))
            .map(|m| Tag::Compound(m.to_nbt()))
            .collect();
        entities.extend(
            self.items
                .values()
                .filter(|i| i.chunk() == (cx, cz))
                .map(|i| Tag::Compound(i.to_nbt())),
        );
        entities.extend(
            self.boats
                .values()
                .filter(|b| b.chunk() == (cx, cz))
                .map(|b| Tag::Compound(b.to_nbt())),
        );
        entities.extend(
            self.carts
                .values()
                .filter(|c| c.chunk() == (cx, cz))
                .map(|c| Tag::Compound(c.to_nbt())),
        );
        Compound::new()
            .with("DataVersion", DATA_VERSION)
            .with("Position", vec![cx, cz])
            .with("Entities", entities)
    }

    /// Saves the mobs and items of every loaded chunk that has (or had) any.
    pub(super) fn save_mobs(&mut self) {
        let now: HashSet<(i32, i32)> = self
            .mobs
            .values()
            .map(Mob::chunk)
            .chain(self.items.values().map(items::ItemEntity::chunk))
            .chain(self.boats.values().map(super::boats::Boat::chunk))
            .chain(self.carts.values().map(super::minecarts::Cart::chunk))
            .collect();
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
        let item_ids: Vec<i32> = self
            .items
            .iter()
            .filter(|(_, i)| i.chunk() == (x, z))
            .map(|(&id, _)| id)
            .collect();
        // Boats with a rider go along with them.
        let boat_ids: Vec<i32> = self
            .boats
            .iter()
            .filter(|(_, b)| b.chunk() == (x, z) && b.rider.is_none())
            .map(|(&id, _)| id)
            .collect();
        let cart_ids: Vec<i32> = self
            .carts
            .iter()
            .filter(|(_, c)| c.chunk() == (x, z) && c.rider.is_none())
            .map(|(&id, _)| id)
            .collect();
        if ids.is_empty()
            && item_ids.is_empty()
            && boat_ids.is_empty()
            && cart_ids.is_empty()
            && !had_mobs
        {
            return;
        }
        self.save_mob_chunk(x, z);
        for id in ids {
            let mob = self.mobs.remove(&id).expect("listed");
            self.send_to_viewers(&mob, items::remove_entity(id));
        }
        for id in item_ids {
            self.items.remove(&id);
            self.broadcast(items::remove_entity(id), None);
        }
        for id in boat_ids {
            self.boats.remove(&id);
            self.broadcast(items::remove_entity(id), None);
        }
        for id in cart_ids {
            self.carts.remove(&id);
            self.broadcast(items::remove_entity(id), None);
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
            let Some(entity) = entity.as_compound() else {
                continue;
            };
            if entity.str("id") == Some("minecraft:item") {
                if let Some(item) = items::ItemEntity::from_nbt(entity) {
                    self.mob_chunks.insert(item.chunk());
                    let id = next_entity_id();
                    for packet in items::spawn_packets(id, &item) {
                        self.broadcast(packet, None);
                    }
                    self.items.insert(id, item);
                }
                continue;
            }
            if let Some(cart) = super::minecarts::Cart::from_nbt(entity, &mut self.rng) {
                self.mob_chunks.insert(cart.chunk());
                let id = next_entity_id();
                for packet in super::minecarts::spawn_packets(id, &cart) {
                    self.broadcast(packet, None);
                }
                self.carts.insert(id, cart);
                continue;
            }
            if let Some(boat) = super::boats::Boat::from_nbt(entity, &mut self.rng) {
                self.mob_chunks.insert(boat.chunk());
                let id = next_entity_id();
                for packet in super::boats::spawn_packets(id, &boat) {
                    self.broadcast(packet, None);
                }
                self.boats.insert(id, boat);
                continue;
            }
            if let Some(mob) = Mob::from_nbt(entity, &mut self.rng) {
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
            return format!("Cannot summon {full} yet: {}", SUMMONABLE.join(", "));
        };
        let Some(p) = self.players.get(&id) else {
            return String::new();
        };
        let pos = p.pos;
        let mob = if kind == MobKind::Villager {
            let biome = self
                .biome_at(pos[0] as i32, pos[1] as i32, pos[2] as i32)
                .unwrap_or_default();
            super::villagers::summoned(pos, &biome, &mut self.rng)
        } else {
            Mob::new(kind, pos, &mut self.rng)
        };
        self.mobs.insert(next_entity_id(), mob);
        format!("Summoned new {}", kind.display())
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
/// Entity data showing a mob's custom name: index 2, an optional text component.
fn name_data(id: i32, name: &str) -> Writer {
    const DATA_CUSTOM_NAME: u8 = 2;
    const SERIALIZER_OPTIONAL_COMPONENT: i32 = 6;
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_CUSTOM_NAME)
        .varint(SERIALIZER_OPTIONAL_COMPONENT)
        .bool(true)
        .text(name)
        .u8(0xFF)
}

impl World {
    /// A named name tag used on a mob gives it the name, and is used up.
    fn name_mob(&mut self, id: i32, target: i32, slot: usize) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let tag = p.inventory.slots[slot];
        let survival = p.survival();
        let Some(name) = tag.name else {
            return; // an unnamed tag does nothing
        };
        let Some(mob) = self.mobs.get_mut(&target) else {
            return;
        };
        mob.name = Some(name);
        let mob = self.mobs.remove(&target).expect("checked above");
        self.send_to_viewers(&mob, name_data(target, name));
        self.mobs.insert(target, mob);
        if survival {
            let p = self.players.get_mut(&id).expect("checked above");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
    }
}

pub(super) fn yaw_towards(from: [f64; 3], to: [f64; 3]) -> f32 {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    (-dx.atan2(dz)).to_degrees() as f32
}

fn within_reach(p: &Player, mob: &Mob) -> bool {
    let (eye, _, _) = p.eye_ray();
    let height = mob.size().1;
    let centre = [mob.pos[0], mob.pos[1] + height / 2.0, mob.pos[2]];
    let d: f64 = (0..3).map(|i| (eye[i] - centre[i]).powi(2)).sum();
    d.sqrt() <= REACH
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat world (grass on top at y = -61, standing height -60) with the chunks
    /// around the origin loaded.
    fn flat_world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-mobs-{}-{}",
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
        for cx in -1..=1 {
            for cz in -1..=1 {
                world.chunk(cx, cz);
            }
        }
        world
    }

    fn wall(world: &mut World, x: i32, heights: std::ops::Range<i32>) {
        for z in -3..=3 {
            for y in heights.clone() {
                world.set_block(BlockPos::new(x, y, z), block::STONE);
            }
        }
    }

    #[test]
    fn wide_mobs_stay_on_the_floor_of_a_narrow_gap() {
        let mut world = flat_world();
        // A 1-wide gap between two 2-high walls; a spider is 1.4 wide.
        wall(&mut world, 0, -60..-58);
        wall(&mut world, 2, -60..-58);
        let mut spider = Mob::new(MobKind::Spider, [1.5, -60.0, 0.5], &mut world.rng);
        for _ in 0..40 {
            world.step(&mut spider);
            assert!(spider.pos[1] < -59.9, "lifted to y {}", spider.pos[1]);
        }
    }

    #[test]
    fn spiders_climb_walls() {
        let mut world = flat_world();
        // A wall 4 blocks high between the spider and where it wants to go.
        wall(&mut world, 3, -60..-56);
        let mut spider = Mob::new(MobKind::Spider, [1.5, -60.0, 0.5], &mut world.rng);
        let mut highest = spider.pos[1];
        for _ in 0..200 {
            spider.target = Some(([8.5, 0.5], 20));
            world.step(&mut spider);
            highest = highest.max(spider.pos[1]);
        }
        assert!(highest >= -56.0, "climbed only to y {highest}");
        assert!(spider.pos[0] > 4.0, "still at x {}", spider.pos[0]);
    }

    #[test]
    fn walkers_hop_up_one_block_steps() {
        let mut world = flat_world();
        wall(&mut world, 3, -60..-59);
        let mut cow = Mob::new(MobKind::Cow, [1.5, -60.0, 0.5], &mut world.rng);
        for _ in 0..100 {
            cow.target = Some(([8.5, 0.5], 20));
            world.step(&mut cow);
        }
        assert!(
            cow.pos[0] > 4.0,
            "a cow should hop a 1-block step, at x {}",
            cow.pos[0]
        );
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;

    fn flat_world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-path-{}-{}",
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
    fn zombies_walk_around_walls_too_high_to_jump() {
        let mut world = flat_world();
        // A wall three blocks high and seven wide between the zombie and its goal.
        for z in -3..=3 {
            for y in -60..-57 {
                world.set_block(BlockPos::new(3, y, z), block::STONE);
            }
        }
        let goal = BlockPos::new(6, -60, 0);
        let path = world
            .find_path(BlockPos::new(1, -60, 0), goal, 2)
            .expect("a way around");
        assert_eq!(path.last(), Some(&goal));
        assert!(path.iter().all(|p| p.x != 3 || p.z.abs() > 3), "{path:?}");

        let mut zombie = Mob::new(MobKind::Zombie, [1.5, -60.0, 0.5], &mut world.rng);
        for _ in 0..400 {
            world.follow_path(&mut zombie, goal);
            world.step(&mut zombie);
        }
        assert!(zombie.pos[0] > 5.0, "stuck at {:?}", zombie.pos);
    }

    #[test]
    fn knockback_carries_a_walking_mob_back() {
        let mut world = flat_world();
        // A zombie walking towards +x is hit back towards -x, once softly and once with
        // knockback II.
        let distance = |strength: f64, world: &mut World| {
            let mut zombie = Mob::new(MobKind::Zombie, [0.5, -60.0, 0.5], &mut world.rng);
            zombie.on_ground = true;
            zombie.target = Some(([20.0, 0.5], 200));
            zombie.knockback = [-strength, 0.0];
            zombie.vel[1] = KNOCKBACK;
            let lowest = (0..20)
                .map(|_| {
                    world.step(&mut zombie);
                    zombie.pos[0]
                })
                .fold(f64::MAX, f64::min);
            0.5 - lowest
        };
        let plain = distance(0.4, &mut world);
        let strong = distance(1.4, &mut world);
        assert!(plain > 0.5, "a plain hit still pushes: {plain}");
        assert!(
            strong > plain * 2.0,
            "knockback II pushes further: {strong} vs {plain}"
        );
    }

    #[test]
    fn paths_climb_steps_and_drop_down() {
        let mut world = flat_world();
        // A one-block step up, then a two-block drop into a pit.
        for z in -3..=3 {
            world.set_block(BlockPos::new(3, -60, z), block::STONE);
            world.set_block(BlockPos::new(4, -61, z), block::AIR);
            world.set_block(BlockPos::new(4, -62, z), block::AIR);
        }
        let goal = BlockPos::new(4, -62, 0);
        let path = world
            .find_path(BlockPos::new(1, -60, 0), goal, 2)
            .expect("a way");
        assert!(path.contains(&BlockPos::new(3, -59, 0)), "{path:?}");
        assert_eq!(path.last(), Some(&goal));
    }

    #[test]
    fn hitting_a_zombified_piglin_angers_its_group() {
        let mut world = flat_world();
        let player = next_entity_id();
        let profile = crate::connection::Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        let (out, _frames) = mpsc::channel(1000);
        world
            .players
            .insert(player, Player::new(profile, out, [0.5, -60.0, 0.5]));
        let near = next_entity_id();
        let far = next_entity_id();
        let hit = next_entity_id();
        for (id, x) in [(hit, 3.0), (near, 8.0), (far, 60.0)] {
            let m = Mob::new(MobKind::ZombifiedPiglin, [x, -60.0, 0.5], &mut world.rng);
            world.mobs.insert(id, m);
        }
        // Left alone, they leave the player alone.
        let mut m = world.mobs.remove(&hit).unwrap();
        world.hunt(hit, &mut m);
        assert_eq!(m.hunting, None);
        world.mobs.insert(hit, m);

        world.hit_mob(
            hit,
            1.0,
            Some(player),
            [1.0, 0.0],
            "minecraft:player_attack",
        );
        assert_eq!(world.mobs[&hit].hunting, Some(player));
        assert_eq!(world.mobs[&near].hunting, Some(player));
        assert_eq!(world.mobs[&far].hunting, None);
    }

    #[test]
    fn big_magma_cubes_split_into_smaller_ones() {
        let mut world = flat_world();
        let mut cube = Mob::new(MobKind::MagmaCube, [0.5, -60.0, 0.5], &mut world.rng);
        cube.slime_size = 4;
        cube.health = 16.0;
        assert_eq!(cube.size(), (2.08, 2.08));
        assert_eq!(cube.attack_damage(), 6.0);
        let id = next_entity_id();
        world.kill_mob(id, &mut cube);
        let small: Vec<&Mob> = world
            .mobs
            .values()
            .filter(|m| m.kind == MobKind::MagmaCube)
            .collect();
        assert!((2..=4).contains(&small.len()), "{}", small.len());
        assert!(small.iter().all(|m| m.slime_size == 2 && m.health == 4.0));
    }

    #[test]
    fn endermen_turn_on_players_who_stare_and_dodge_arrows() {
        let mut world = flat_world();
        let player = next_entity_id();
        let profile = crate::connection::Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        let (out, _frames) = mpsc::channel(1000);
        let mut alice = Player::new(profile, out, [0.5, -60.0, 0.5]);
        alice.yaw = 0.0; // facing south, towards +z
        alice.pitch = -6.0;
        world.players.insert(player, alice);
        let id = next_entity_id();
        let mut enderman = Mob::new(MobKind::Enderman, [0.5, -60.0, 10.5], &mut world.rng);
        world.hunt(id, &mut enderman);
        assert_eq!(enderman.hunting, Some(player), "stared at");

        let before = enderman.pos;
        world.mobs.insert(id, enderman);
        assert!(!world.hit_mob(id, 5.0, Some(player), [0.0, 1.0], "minecraft:arrow"));
        let after = &world.mobs[&id];
        assert_eq!(after.health, 40.0, "unhurt");
        assert_ne!(after.pos, before, "gone elsewhere");
    }
}
