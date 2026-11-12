//! Bees, bee nests and beehives.
//!
//! Behaviour follows the Minecraft Wiki: a hive or nest holds up to three bees. By day,
//! when it is not raining, bees leave to find flowers (`minecraft:bee_attractive`)
//! within reach, hover at one to pollinate it, and fly home with nectar; in the hive a
//! bee with nectar raises its honey level (to 5) and stays two minutes, one without
//! half a minute. At night and in the rain they stay in or head home. On the way back
//! a bee with nectar helps crops below it grow. A full hive gives three honeycombs to
//! shears or a honey bottle to a glass bottle; without a campfire up to five blocks
//! below, its bees come out angry, as they do when the hive is broken. An angry bee
//! stings once (poison for 10 seconds on Normal, 18 on Hard) and dies some time after.
//! Nests in generated trees come with two or three bees.

use levitas_nbt::{Compound, Tag};

use super::containers::BlockEntity;
use super::mobs::{Mob, MobKind};
use super::*;
use crate::blocks::{self, offset, prop, with, Dir};
use crate::world::survival::Damage;

const CAPACITY: usize = 3;
/// Ticks a bee stays inside: two minutes with nectar, 30 seconds without.
const STAY_WITH_NECTAR: u32 = 2400;
const STAY_WITHOUT: u32 = 600;
/// Ticks of hovering at a flower to gather nectar.
const POLLINATE_TICKS: u32 = 200;
const FLOWER_RANGE: i32 = 5;
const HIVE_RANGE: i32 = 20;
const MAX_HONEY: u8 = 5;
/// How long a bee stays angry, and how far bees join in.
const ANGER_TICKS: u32 = 400;
const ALERT_RANGE: f64 = 16.0;

const DATA_BEE_FLAGS: u8 = 18;
const DATA_BEE_ANGER: u8 = 19;
const SERIALIZER_BYTE: i32 = 0;
const SERIALIZER_VARLONG: i32 = 2;
const FLAG_ANGRY: u8 = 0x02;
const FLAG_STUNG: u8 = 0x04;
const FLAG_NECTAR: u8 = 0x08;

/// A bee's own state.
#[derive(Debug, Clone, Default)]
pub struct Bee {
    pub hive: Option<BlockPos>,
    pub flower: Option<BlockPos>,
    pub nectar: bool,
    pub stung: bool,
    /// Ticks spent at the flower so far.
    pollinating: u32,
    /// Crops helped along on this trip.
    crops_grown: u8,
    /// Ticks left to live after stinging.
    dying_in: Option<u32>,
    /// Ticks before it may go back in (just came out).
    pub cannot_enter: u32,
    /// Whether it is on its way somewhere (a flower, home, its prey).
    pub heading: bool,
}

/// A hive or nest: the bees inside.
#[derive(Default)]
pub struct Beehive {
    pub bees: Vec<HiveBee>,
    pub viewers: HashSet<i32>,
}

/// A bee inside a hive: its saved form and how long it has been in.
pub struct HiveBee {
    pub nbt: Compound,
    pub ticks: u32,
    pub min_ticks: u32,
}

impl Beehive {
    pub fn read_nbt(nbt: &Compound) -> Self {
        let bees = nbt
            .list("bees")
            .into_iter()
            .flatten()
            .filter_map(Tag::as_compound)
            .filter_map(|b| {
                Some(HiveBee {
                    nbt: b.get("entity_data")?.as_compound()?.clone(),
                    ticks: b.int("ticks_in_hive").unwrap_or(0).max(0) as u32,
                    min_ticks: b.int("min_ticks_in_hive").unwrap_or(0).max(0) as u32,
                })
            })
            .take(CAPACITY)
            .collect();
        Beehive {
            bees,
            viewers: HashSet::new(),
        }
    }

    pub fn write_nbt(&self, nbt: &mut Compound) {
        let bees: Vec<Tag> = self
            .bees
            .iter()
            .map(|b| {
                Tag::Compound(
                    Compound::new()
                        .with("entity_data", b.nbt.clone())
                        .with("ticks_in_hive", b.ticks as i32)
                        .with("min_ticks_in_hive", b.min_ticks as i32),
                )
            })
            .collect();
        nbt.insert("bees", bees);
    }
}

/// Whether `state` is a bee nest or beehive.
pub fn is_hive(state: u16) -> bool {
    matches!(
        blocks::name(state),
        "minecraft:bee_nest" | "minecraft:beehive"
    )
}

fn honey(state: u16) -> u8 {
    prop(state, "honey_level")
        .and_then(|h| h.parse().ok())
        .unwrap_or(0)
}

fn centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// The entity data of a bee: angry, stung, with nectar.
pub fn bee_data(id: i32, mob: &Mob) -> Writer {
    let bee = mob.bee.as_deref().cloned().unwrap_or_default();
    let mut flags = 0;
    if mob.anger > 0 {
        flags |= FLAG_ANGRY;
    }
    if bee.stung {
        flags |= FLAG_STUNG;
    }
    if bee.nectar {
        flags |= FLAG_NECTAR;
    }
    let anger = if mob.anger > 0 {
        i64::from(mob.anger)
    } else {
        -1
    };
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_BEE_FLAGS)
        .varint(SERIALIZER_BYTE)
        .u8(flags)
        .u8(DATA_BEE_ANGER)
        .varint(SERIALIZER_VARLONG)
        .varlong(anger)
        .u8(0xFF)
}

/// Saves a bee's own tags.
pub fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let Some(bee) = &mob.bee else {
        return;
    };
    nbt.insert("HasNectar", bee.nectar);
    nbt.insert("HasStung", bee.stung);
    nbt.insert("CannotEnterHiveTicks", bee.cannot_enter as i32);
    if let Some(h) = bee.hive {
        nbt.insert("hive_pos", vec![h.x, h.y, h.z]);
    }
    if let Some(f) = bee.flower {
        nbt.insert("flower_pos", vec![f.x, f.y, f.z]);
    }
}

/// Reads a bee's own tags back.
pub fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let Some(bee) = mob.bee.as_mut() else {
        return;
    };
    let pos = |key: &str| match nbt.int_array(key) {
        Some(&[x, y, z]) => Some(BlockPos::new(x, y, z)),
        _ => None,
    };
    bee.hive = pos("hive_pos");
    bee.flower = pos("flower_pos");
    bee.nectar = nbt.int("HasNectar").unwrap_or(0) != 0;
    bee.stung = nbt.int("HasStung").unwrap_or(0) != 0;
    bee.cannot_enter = nbt.int("CannotEnterHiveTicks").unwrap_or(0).max(0) as u32;
}

impl World {
    fn is_daytime_for_bees(&self) -> bool {
        self.dimension.has_sky()
            && self.day_time.rem_euclid(24_000) < 12_000
            && !self.weather.is_raining()
    }

    /// A bee's turn. Returns whether it went into its hive (and so is gone).
    pub(super) fn tick_bee(&mut self, id: i32, mob: &mut Mob) -> bool {
        let Some(mut bee) = mob.bee.take() else {
            return false;
        };
        let gone = self.bee_turn(id, mob, &mut bee);
        mob.bee = Some(bee);
        gone
    }

    fn bee_turn(&mut self, id: i32, mob: &mut Mob, bee: &mut Bee) -> bool {
        bee.cannot_enter = bee.cannot_enter.saturating_sub(1);
        bee.heading = true;
        if let Some(left) = bee.dying_in.as_mut() {
            *left = left.saturating_sub(1);
            if *left == 0 {
                mob.health = 0.0;
                self.hurt_mob(id, mob, 1000.0, "minecraft:generic");
                return false;
            }
        }
        // Angry: after its prey, and a sting.
        if mob.anger > 0 {
            mob.anger -= 1;
            if mob.anger == 0 {
                mob.hunting = None;
                self.send_to_viewers(mob, bee_data(id, mob));
            }
            if let Some(pid) = mob.hunting {
                self.bee_attack(id, mob, bee, pid);
            }
            return false;
        }
        let home = bee
            .hive
            .filter(|&h| self.loaded_block(h).is_some_and(is_hive));
        if bee.hive.is_some()
            && home.is_none()
            && bee.hive.is_some_and(|h| self.loaded_block(h).is_some())
        {
            bee.hive = None; // the hive is gone
        }
        if bee.hive.is_none() && self.tick.is_multiple_of(20) {
            bee.hive = self.find_hive(mob.pos);
        }
        let wants_home = bee.nectar || !self.is_daytime_for_bees();
        if let (true, Some(hive)) = (wants_home, home) {
            let door = self.hive_door(hive);
            mob.fly_goal = Some(door);
            if bee.nectar {
                self.bee_helps_crops(mob, bee);
            }
            if distance(mob.pos, door) <= 1.5 && bee.cannot_enter == 0 {
                return self.enter_hive(id, mob, bee, hive);
            }
            return false;
        }
        // Out for nectar.
        let flower_ok = bee
            .flower
            .and_then(|f| self.loaded_block(f))
            .is_some_and(|s| block_has_tag(s, "minecraft:bee_attractive"));
        if !flower_ok {
            bee.flower = None;
            bee.pollinating = 0;
            bee.heading = false;
            if self.tick.is_multiple_of(10) {
                bee.flower = self.find_flower(mob.pos);
            }
            return false;
        }
        let flower = bee.flower.expect("checked");
        let spot = [
            f64::from(flower.x) + 0.5,
            f64::from(flower.y) + 0.6,
            f64::from(flower.z) + 0.5,
        ];
        mob.fly_goal = Some(spot);
        if distance(mob.pos, spot) <= 1.0 {
            bee.pollinating += 1;
            if bee.pollinating >= POLLINATE_TICKS {
                bee.pollinating = 0;
                bee.nectar = true;
                bee.crops_grown = 0;
                let data = bee_data(id, mob_with(mob, bee));
                self.send_to_viewers(mob, data);
                if bee.hive.is_none() {
                    bee.flower = None;
                }
            }
        }
        false
    }

    fn bee_attack(&mut self, id: i32, mob: &mut Mob, bee: &mut Bee, pid: i32) {
        if bee.stung {
            return;
        }
        let Some(p) = self.players.get(&pid).filter(|p| p.alive && p.survival()) else {
            mob.anger = 0;
            mob.hunting = None;
            return;
        };
        let body = [p.pos[0], p.pos[1] + 0.9, p.pos[2]];
        mob.fly_goal = Some(body);
        if distance(mob.pos, body) > 1.2 {
            return;
        }
        if self.damage_from(pid, 2.0, Damage::Mob("Bee"), Some(id)) {
            let seconds = match crate::rules::difficulty() {
                crate::rules::Difficulty::Normal => 10,
                crate::rules::Difficulty::Hard => 18,
                _ => 0,
            };
            if seconds > 0 {
                self.add_effect(
                    pid,
                    levitas_registry::Effect {
                        id: levitas_registry::mob_effect::POISON,
                        duration: seconds * 20,
                        amplifier: 0,
                    },
                );
            }
        }
        self.play_sound_at("minecraft:entity.bee.sting", mob.pos, 1.0, 1.0, None);
        bee.stung = true;
        bee.dying_in = Some(600 + self.rng.below(1200));
        mob.anger = 0;
        mob.hunting = None;
        let data = bee_data(id, mob_with(mob, bee));
        self.send_to_viewers(mob, data);
    }

    /// Bees with nectar flying over crops help them grow, up to ten times a trip.
    fn bee_helps_crops(&mut self, mob: &Mob, bee: &mut Bee) {
        if bee.crops_grown >= 10 || self.rng.below(30) != 0 {
            return;
        }
        for dy in 1..=2 {
            let pos = BlockPos::new(
                mob.pos[0].floor() as i32,
                mob.pos[1].floor() as i32 - dy,
                mob.pos[2].floor() as i32,
            );
            let state = self.block(pos);
            if !block_has_tag(state, "minecraft:bee_growables") {
                continue;
            }
            if let Some(max) = crate::blocks::plants::crop_max_age(state) {
                let age = crate::blocks::plants::age(state);
                if age < max {
                    self.set_block_updating(pos, crate::blocks::plants::with_age(state, age + 1));
                    bee.crops_grown += 1;
                }
                return;
            }
        }
    }

    /// Where bees go in and out: the air in front of the hive.
    fn hive_door(&self, hive: BlockPos) -> [f64; 3] {
        let facing = prop(self.loaded_block(hive).unwrap_or(0), "facing")
            .and_then(Dir::from_name)
            .unwrap_or(Dir::South);
        centre(offset(hive, facing))
    }

    fn find_hive(&self, at: [f64; 3]) -> Option<BlockPos> {
        self.block_entities
            .iter()
            .filter(|(pos, e)| {
                matches!(e, BlockEntity::Beehive(h) if h.bees.len() < CAPACITY)
                    && distance(centre(**pos), at) <= f64::from(HIVE_RANGE)
            })
            .map(|(pos, _)| *pos)
            .min_by(|a, b| distance(centre(*a), at).total_cmp(&distance(centre(*b), at)))
    }

    fn find_flower(&mut self, at: [f64; 3]) -> Option<BlockPos> {
        for _ in 0..8 {
            let pos = BlockPos::new(
                at[0].floor() as i32 + self.rng.below(FLOWER_RANGE as u32 * 2 + 1) as i32
                    - FLOWER_RANGE,
                at[1].floor() as i32 + self.rng.below(7) as i32 - 4,
                at[2].floor() as i32 + self.rng.below(FLOWER_RANGE as u32 * 2 + 1) as i32
                    - FLOWER_RANGE,
            );
            if self
                .loaded_block(pos)
                .is_some_and(|s| block_has_tag(s, "minecraft:bee_attractive"))
            {
                return Some(pos);
            }
        }
        None
    }

    fn enter_hive(&mut self, id: i32, mob: &mut Mob, bee: &mut Bee, hive: BlockPos) -> bool {
        let state = self.block(hive);
        let room = matches!(
            self.block_entities.get(&hive),
            Some(BlockEntity::Beehive(h)) if h.bees.len() < CAPACITY
        );
        if !room {
            bee.hive = None;
            return false;
        }
        let nectar = bee.nectar;
        bee.nectar = false;
        bee.flower = None;
        mob.bee = Some(Box::new(bee.clone()));
        let nbt = mob.to_nbt();
        mob.bee = None;
        if let Some(BlockEntity::Beehive(h)) = self.block_entities.get_mut(&hive) {
            h.bees.push(HiveBee {
                nbt,
                ticks: 0,
                min_ticks: if nectar {
                    STAY_WITH_NECTAR
                } else {
                    STAY_WITHOUT
                },
            });
        }
        if nectar {
            let level = honey(state);
            if level < MAX_HONEY {
                self.set_block(hive, with(state, "honey_level", &(level + 1).to_string()));
            }
        }
        self.dirty.insert(hive.chunk());
        self.send_to_viewers(mob, items::remove_entity(id));
        self.play_sound_at(
            "minecraft:block.beehive.enter",
            centre(hive),
            1.0,
            1.0,
            None,
        );
        true
    }

    /// Every second: bees whose time inside is up come out, by day and in fair weather.
    pub(super) fn tick_hives(&mut self) {
        if !self.tick.is_multiple_of(20) {
            return;
        }
        let day = self.is_daytime_for_bees();
        let hives: Vec<BlockPos> = self
            .block_entities
            .iter()
            .filter(|(_, e)| matches!(e, BlockEntity::Beehive(h) if !h.bees.is_empty()))
            .map(|(p, _)| *p)
            .collect();
        for pos in hives {
            let leaving: Vec<Compound> = match self.block_entities.get_mut(&pos) {
                Some(BlockEntity::Beehive(h)) => {
                    for b in &mut h.bees {
                        b.ticks += 20;
                    }
                    if !day {
                        continue;
                    }
                    let (out, stay): (Vec<HiveBee>, Vec<HiveBee>) = std::mem::take(&mut h.bees)
                        .into_iter()
                        .partition(|b| b.ticks >= b.min_ticks);
                    h.bees = stay;
                    out.into_iter().map(|b| b.nbt).collect()
                }
                _ => continue,
            };
            for nbt in leaving {
                self.release_bee(pos, &nbt, None);
            }
        }
    }

    /// A bee comes out of the hive at `pos`, angry at `angry_at` if given.
    fn release_bee(&mut self, pos: BlockPos, nbt: &Compound, angry_at: Option<i32>) {
        let Some(mut mob) = Mob::read(nbt, &mut self.rng) else {
            return;
        };
        mob.pos = self.hive_door(pos);
        mob.fly_goal = None;
        if let Some(bee) = mob.bee.as_mut() {
            bee.hive = Some(pos);
            bee.cannot_enter = 400;
        }
        if let Some(pid) = angry_at {
            mob.anger = ANGER_TICKS + self.rng.below(400);
            mob.hunting = Some(pid);
        }
        self.play_sound_at("minecraft:block.beehive.exit", mob.pos, 1.0, 1.0, None);
        self.mobs.insert(next_entity_id(), mob);
        self.dirty.insert(pos.chunk());
    }

    /// The hive at `pos` is broken: its bees come out, angry unless it was mined with
    /// silk touch.
    pub(super) fn hive_broken(&mut self, id: i32, pos: BlockPos, silk: bool) {
        let bees: Vec<Compound> = match self.block_entities.get_mut(&pos) {
            Some(BlockEntity::Beehive(h)) => h.bees.drain(..).map(|b| b.nbt).collect(),
            _ => return,
        };
        let angry = (!silk).then_some(id);
        for nbt in bees {
            self.release_bee(pos, &nbt, angry);
        }
        if !silk {
            self.anger_bees(centre(pos), id);
        }
    }

    /// Bees near `at` turn on player `id`.
    pub(super) fn anger_bees(&mut self, at: [f64; 3], id: i32) {
        if !self.players.get(&id).is_some_and(|p| p.survival()) {
            return;
        }
        let ids: Vec<i32> = self
            .mobs
            .iter()
            .filter(|(_, m)| {
                m.kind == MobKind::Bee && m.alive() && distance(m.pos, at) <= ALERT_RANGE
            })
            .map(|(&mid, _)| mid)
            .collect();
        for mid in ids {
            let anger = ANGER_TICKS + self.rng.below(400);
            if let Some(m) = self.mobs.get_mut(&mid) {
                if m.bee.as_ref().is_some_and(|b| b.stung) {
                    continue;
                }
                m.anger = anger;
                m.hunting = Some(id);
            }
            if let Some(m) = self.mobs.remove(&mid) {
                self.send_to_viewers(&m, bee_data(mid, &m));
                self.mobs.insert(mid, m);
            }
        }
    }

    /// Shears or a glass bottle on a full hive. Returns whether the hive took it.
    pub(super) fn use_hive(&mut self, id: i32, slot: usize, pos: BlockPos, state: u16) -> bool {
        if !is_hive(state) || honey(state) < MAX_HONEY {
            return false;
        }
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let held = ItemInfo::get(p.inventory.slots[slot].item).map_or("", |i| i.name);
        let survival = p.survival();
        match held {
            "minecraft:shears" => {
                if let Some(comb) = ItemInfo::id_of("minecraft:honeycomb") {
                    let at = [
                        f64::from(pos.x) + 0.5,
                        f64::from(pos.y) + 1.0,
                        f64::from(pos.z) + 0.5,
                    ];
                    self.scatter_item(at, Stack::new(comb, 3));
                }
                self.play_sound_at("minecraft:block.beehive.shear", centre(pos), 1.0, 1.0, None);
                if survival {
                    self.damage_item(id, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
                }
            }
            "minecraft:glass_bottle" => {
                self.play_sound_at("minecraft:item.bottle.fill", centre(pos), 1.0, 1.0, None);
                self.exchange_held(id, slot, "minecraft:honey_bottle");
            }
            _ => return false,
        }
        self.set_block_updating(pos, with(state, "honey_level", "0"));
        if !self.campfire_below(pos) {
            let bees: Vec<Compound> = match self.block_entities.get_mut(&pos) {
                Some(BlockEntity::Beehive(h)) => h.bees.drain(..).map(|b| b.nbt).collect(),
                _ => Vec::new(),
            };
            for nbt in bees {
                self.release_bee(pos, &nbt, Some(id));
            }
            self.anger_bees(centre(pos), id);
        }
        true
    }

    /// Whether a campfire up to five blocks under the hive calms its bees.
    fn campfire_below(&self, pos: BlockPos) -> bool {
        (1..=5).any(|dy| {
            self.loaded_block(BlockPos::new(pos.x, pos.y - dy, pos.z))
                .is_some_and(|s| blocks::name(s).ends_with("campfire"))
        })
    }

    /// Fresh chunks: nests the trees grew get two or three bees.
    pub(super) fn settle_nests(&mut self, chunk: (i32, i32)) {
        let Some(nest) = BlockInfo::by_name("minecraft:bee_nest") else {
            return;
        };
        let states: Vec<u16> = (nest.first_state..nest.first_state + 24).collect();
        let Some(c) = self.chunks.get(&chunk) else {
            return;
        };
        let mut found = Vec::new();
        for (s, section) in c.sections().iter().enumerate() {
            if section.is_empty() || !states.iter().any(|&st| section.blocks().may_contain(st)) {
                continue;
            }
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        if states.contains(&section.get(x, y, z)) {
                            found.push(BlockPos::new(
                                chunk.0 * 16 + x as i32,
                                MIN_Y + (s * 16 + y) as i32,
                                chunk.1 * 16 + z as i32,
                            ));
                        }
                    }
                }
            }
        }
        for pos in found {
            let count = 2 + self.rng.below(2) as usize;
            let mut hive = Beehive::default();
            for _ in 0..count {
                let mut bee = Mob::new(MobKind::Bee, centre(pos), &mut self.rng);
                if let Some(b) = bee.bee.as_mut() {
                    b.hive = Some(pos);
                }
                hive.bees.push(HiveBee {
                    nbt: bee.to_nbt(),
                    ticks: 0,
                    min_ticks: STAY_WITHOUT,
                });
            }
            self.block_entities
                .insert(pos, BlockEntity::Beehive(Box::new(hive)));
        }
    }
}

/// A view of `mob` with its bee state put back, for its entity data.
fn mob_with<'a>(mob: &'a mut Mob, bee: &Bee) -> &'a Mob {
    mob.bee = Some(Box::new(bee.clone()));
    mob
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-bees-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        for cx in -1..=1 {
            for cz in -1..=1 {
                w.chunk(cx, cz);
            }
        }
        w.day_time = 1000;
        w
    }

    fn nest(w: &mut World, pos: BlockPos) -> u16 {
        let state = BlockInfo::by_name("minecraft:bee_nest")
            .unwrap()
            .default_state;
        let state = with(state, "facing", "south");
        w.set_block(pos, state);
        state
    }

    #[test]
    fn generated_nests_get_bees_who_come_out_by_day() {
        let mut w = world();
        let pos = BlockPos::new(3, -58, 3);
        nest(&mut w, pos);
        w.settle_nests((0, 0));
        let inside = match w.block_entities.get(&pos) {
            Some(BlockEntity::Beehive(h)) => h.bees.len(),
            _ => 0,
        };
        assert!((2..=3).contains(&inside), "{inside}");
        // Saved and loaded with its bees.
        let nbt = w.block_entities[&pos].to_nbt(pos);
        let (_, back) = BlockEntity::from_nbt(&nbt).unwrap();
        assert!(matches!(back, BlockEntity::Beehive(h) if h.bees.len() == inside));
        let before = w.mobs.len();
        for _ in 0..40 {
            w.tick += 1;
            if let Some(BlockEntity::Beehive(h)) = w.block_entities.get_mut(&pos) {
                for b in &mut h.bees {
                    b.ticks = b.min_ticks;
                }
            }
            w.tick_hives();
        }
        assert_eq!(w.mobs.len(), before + inside, "out for the day");
        let bee = w.mobs.values().find(|m| m.kind == MobKind::Bee).unwrap();
        assert_eq!(bee.bee.as_ref().unwrap().hive, Some(pos));
    }

    #[test]
    fn bees_bring_nectar_home_and_make_honey() {
        let mut w = world();
        let pos = BlockPos::new(3, -58, 3);
        let state = nest(&mut w, pos);
        w.block_entities
            .insert(pos, BlockEntity::Beehive(Box::default()));
        let poppy = BlockInfo::by_name("minecraft:poppy").unwrap().default_state;
        let flower = BlockPos::new(6, -60, 3);
        w.set_block(flower, poppy);
        let id = next_entity_id();
        let mut mob = Mob::new(MobKind::Bee, [6.5, -59.4, 3.5], &mut w.rng);
        {
            let b = mob.bee.as_mut().unwrap();
            b.hive = Some(pos);
            b.flower = Some(flower);
        }
        for _ in 0..POLLINATE_TICKS {
            w.tick_bee(id, &mut mob);
        }
        assert!(mob.bee.as_ref().unwrap().nectar, "gathered nectar");
        // Back at the door it goes in, and the honey level rises.
        mob.pos = w.hive_door(pos);
        assert!(w.tick_bee(id, &mut mob));
        assert_eq!(honey(w.block(pos)), 1);
        assert!(
            matches!(w.block_entities.get(&pos), Some(BlockEntity::Beehive(h)) if h.bees.len() == 1)
        );
        let _ = state;
    }

    #[test]
    fn full_hives_give_honeycomb_and_anger_bees_without_a_campfire() {
        let mut w = world();
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 5,
            properties: Vec::new(),
        };
        let mut p = Player::new(profile, out, [3.5, -60.0, 5.5]);
        p.inventory.slots[36] = Stack::new(ItemInfo::id_of("minecraft:shears").unwrap(), 1);
        w.players.insert(alice, p);
        let pos = BlockPos::new(3, -58, 3);
        let state = nest(&mut w, pos);
        w.set_block(pos, with(state, "honey_level", "5"));
        let mut hive = Beehive::default();
        let bee = Mob::new(MobKind::Bee, centre(pos), &mut w.rng);
        hive.bees.push(HiveBee {
            nbt: bee.to_nbt(),
            ticks: 0,
            min_ticks: 600,
        });
        w.block_entities
            .insert(pos, BlockEntity::Beehive(Box::new(hive)));
        let full = w.block(pos);
        assert!(w.use_hive(alice, 36, pos, full));
        assert_eq!(honey(w.block(pos)), 0);
        let comb = ItemInfo::id_of("minecraft:honeycomb").unwrap();
        assert!(w.items.values().any(|i| i.stack().item == comb));
        let angry = w.mobs.values().find(|m| m.kind == MobKind::Bee).unwrap();
        assert!(angry.anger > 0 && angry.hunting == Some(alice));
    }
}
