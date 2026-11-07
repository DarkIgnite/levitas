//! A villager's day, as the Minecraft Wiki describes it. Adults follow a schedule: idle
//! from dawn, at work from 2000 (going to their workstation), meeting at the village
//! bell from 9000, idle again from 11000 and asleep in their bed from 12000; babies
//! play instead of working. Villagers claim a bed (their home) and remember the bell
//! (their meeting point), pick up food and crops, and farmers harvest and replant
//! ripe crops near their composter. Two adults with 12 food points each and a free bed
//! about make a baby.

use super::mobs::{Mob, MobKind};
use super::villagers::{Profession, Villager};
use super::*;
use crate::blocks::plants::{age, crop_max_age, with_age};
use crate::blocks::{self, prop};
use crate::loot::LootContext;

/// What a villager is up to at a time of day.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Activity {
    Idle,
    Work,
    Meet,
    Rest,
}

/// The vanilla schedules: `villager_default` for adults, `villager_baby` for babies
/// (whose play is idling here).
pub(super) fn activity(time: i64, baby: bool) -> Activity {
    let t = time.rem_euclid(24_000);
    if !(10..12_000).contains(&t) {
        Activity::Rest
    } else if baby {
        Activity::Idle
    } else if (2_000..9_000).contains(&t) {
        Activity::Work
    } else if (9_000..11_000).contains(&t) {
        Activity::Meet
    } else {
        Activity::Idle
    }
}

/// How far a villager looks for a bed and a bell, and how often.
const POI_REACH: i32 = 16;
const POI_REACH_UP: i32 = 4;
const POI_CHECK_TICKS: u64 = 100;
/// How close to its bed a villager lies down, and to its workstation it stops.
const BED_REACH: f64 = 1.5;
const WORK_REACH: f64 = 2.0;
/// Meeting villagers gather within this distance of the bell.
const MEET_REACH: f64 = 4.0;
/// How far a farmer looks for ripe crops, around itself.
const FARM_REACH: i32 = 8;
/// Food points needed to breed, and how long being willing lasts.
pub(super) const FOOD_TO_BREED: i32 = 12;
const WILLING_TICKS: u32 = 600;
/// Slots in a villager's inventory.
pub(super) const VILLAGER_SLOTS: usize = 8;
/// A villager holding more food points than this shares with one that has too few.
const SHARE_ABOVE: i32 = 24;

/// Food points an item is worth to a villager.
pub(super) fn food_points(item: i32) -> i32 {
    match ItemInfo::get(item).map_or("", |i| i.name) {
        "minecraft:bread" => 4,
        "minecraft:carrot" | "minecraft:potato" | "minecraft:beetroot" => 1,
        _ => 0,
    }
}

/// What villagers pick up off the ground.
fn wanted(item: i32) -> bool {
    matches!(
        ItemInfo::get(item).map_or("", |i| i.name),
        "minecraft:bread"
            | "minecraft:potato"
            | "minecraft:carrot"
            | "minecraft:wheat"
            | "minecraft:wheat_seeds"
            | "minecraft:beetroot"
            | "minecraft:beetroot_seeds"
            | "minecraft:torchflower_seeds"
            | "minecraft:pitcher_pod"
    )
}

/// The item that replants a crop.
fn seed_of(crop: &str) -> Option<&'static str> {
    Some(match crop {
        "minecraft:wheat" => "minecraft:wheat_seeds",
        "minecraft:carrots" => "minecraft:carrot",
        "minecraft:potatoes" => "minecraft:potato",
        "minecraft:beetroots" => "minecraft:beetroot_seeds",
        _ => return None,
    })
}

fn feet(pos: [f64; 3]) -> BlockPos {
    BlockPos::new(
        pos[0].floor() as i32,
        (pos[1] + 0.01).floor() as i32,
        pos[2].floor() as i32,
    )
}

fn centre_distance(pos: [f64; 3], block: BlockPos) -> f64 {
    let dy = f64::from(block.y) - pos[1];
    (f64::from(block.x) + 0.5 - pos[0])
        .hypot(f64::from(block.z) + 0.5 - pos[2])
        .hypot(dy.min(0.0))
}

impl Villager {
    /// Food points in the inventory.
    pub fn food(&self) -> i32 {
        self.inventory
            .iter()
            .map(|s| food_points(s.item) * s.count)
            .sum()
    }

    /// Puts a stack in the inventory; returns what did not fit.
    pub fn stow(&mut self, mut stack: Stack) -> Stack {
        for slot in &mut self.inventory {
            if stack.is_empty() {
                break;
            }
            if slot.same_kind(&stack) && slot.count < slot.max_stack() {
                let moved = (slot.max_stack() - slot.count).min(stack.count);
                *slot = slot.with_count(slot.count + moved);
                stack = stack.with_count(stack.count - moved);
            }
        }
        if !stack.is_empty() && self.inventory.len() < VILLAGER_SLOTS {
            self.inventory.push(stack);
            return Stack::EMPTY;
        }
        stack
    }

    /// Takes one of `item` out of the inventory, if there is one.
    pub fn take_one(&mut self, item: i32) -> bool {
        let Some(i) = self
            .inventory
            .iter()
            .position(|s| s.item == item && s.count > 0)
        else {
            return false;
        };
        let s = self.inventory[i];
        if s.count <= 1 {
            self.inventory.remove(i);
        } else {
            self.inventory[i] = s.with_count(s.count - 1);
        }
        true
    }

    /// Eats `points` food points' worth, the cheapest food first. Returns whether it had
    /// enough.
    pub fn eat(&mut self, points: i32) -> bool {
        if self.food() < points {
            return false;
        }
        let mut left = points;
        while left > 0 {
            let Some(i) = self
                .inventory
                .iter()
                .enumerate()
                .filter(|(_, s)| food_points(s.item) > 0)
                .min_by_key(|(_, s)| food_points(s.item))
                .map(|(i, _)| i)
            else {
                break;
            };
            let s = self.inventory[i];
            left -= food_points(s.item);
            if s.count <= 1 {
                self.inventory.remove(i);
            } else {
                self.inventory[i] = s.with_count(s.count - 1);
            }
        }
        true
    }
}

impl World {
    /// A villager's day: sleeping, going to work, to the bell and to bed, picking up
    /// food, farming and wanting to breed. Runs after its wandering is decided, so where
    /// it needs to go wins.
    pub(super) fn villager_life(&mut self, id: i32, mob: &mut Mob) {
        let Some(v) = mob.villager.as_ref() else {
            return;
        };
        if v.wandering || mob.kind != MobKind::Villager {
            return;
        }
        let mut act = activity(self.day_time, mob.age < 0);
        // A bell rang: hide at home (asleep only if it is night anyway).
        let hiding = self.bell_alarm_near(mob.pos);
        if hiding {
            act = Activity::Rest;
        }
        let tick = self.tick + u64::from(id.unsigned_abs());

        // Asleep: stays put until morning, or until something wakes it.
        if let Some(bed) = v.sleeping {
            let bed_state = self.block(bed);
            let bed_ok = blocks::is_bed(bed_state) && prop(bed_state, "part") == Some("head");
            if act == Activity::Rest && bed_ok && mob.hurt_ticks == 0 && mob.panic_ticks == 0 {
                mob.target = None;
                mob.path.clear();
                mob.vel = [0.0, mob.vel[1].min(0.0), 0.0];
                return;
            }
            self.wake_villager(id, mob);
        }

        if tick.is_multiple_of(POI_CHECK_TICKS) {
            self.find_village_places(mob);
        }
        if tick.is_multiple_of(10) {
            self.villager_pickup(id, mob);
        }
        if mob.panic_ticks > 0 || mob.panic > 0 || mob.love > 0 {
            return;
        }
        let v = mob.villager.as_ref().expect("checked above");
        match act {
            Activity::Rest => {
                if let Some(home) = v.home {
                    let night = activity(self.day_time, mob.age < 0) == Activity::Rest;
                    if centre_distance(mob.pos, home) <= BED_REACH {
                        if night {
                            self.sleep_villager(id, mob, home);
                        } else {
                            mob.target = None;
                            mob.path.clear();
                        }
                    } else {
                        self.follow_path(mob, home);
                    }
                }
            }
            Activity::Work => {
                let farmer = v.profession == Profession::Farmer;
                if let Some(site) = v.job_site {
                    if farmer && tick.is_multiple_of(20) && self.farm(mob) {
                        return;
                    }
                    if farmer && mob.target.is_some() && mob.path.last().is_some_and(|p| *p != site)
                    {
                        // On its way to a crop.
                        return;
                    }
                    if centre_distance(mob.pos, site) > WORK_REACH {
                        self.follow_path(mob, site);
                    } else {
                        mob.target = None;
                        mob.path.clear();
                    }
                }
            }
            Activity::Meet => {
                if let Some(bell) = v.meeting {
                    if centre_distance(mob.pos, bell) > MEET_REACH {
                        self.follow_path(mob, bell);
                    } else if tick.is_multiple_of(40) {
                        self.share_food(id, mob);
                    }
                }
                if tick.is_multiple_of(POI_CHECK_TICKS) {
                    self.maybe_willing(mob);
                }
            }
            Activity::Idle => {
                if tick.is_multiple_of(POI_CHECK_TICKS) {
                    self.maybe_willing(mob);
                }
            }
        }
    }

    /// Checks the villager's bed and bell are still there, and looks for them if not:
    /// the nearest bed no other villager has, and the nearest bell.
    fn find_village_places(&mut self, mob: &mut Mob) {
        let pos = mob.pos;
        let Some(v) = mob.villager.as_mut() else {
            return;
        };
        if let Some(home) = v.home {
            let s = self.loaded_state(home).unwrap_or(block::AIR);
            if !(blocks::is_bed(s) && prop(s, "part") == Some("head")) {
                v.home = None;
            }
        }
        if let Some(bell) = v.meeting {
            if self
                .loaded_state(bell)
                .is_some_and(|s| blocks::name(s) != "minecraft:bell")
            {
                v.meeting = None;
            }
        }
        if v.home.is_some() && v.meeting.is_some() {
            return;
        }
        let claimed: HashSet<BlockPos> = self.claimed_beds();
        let (beds, bell) = self.scan_village(feet(pos));
        let Some(v) = mob.villager.as_mut() else {
            return;
        };
        if v.home.is_none() {
            v.home = beds.into_iter().find(|b| !claimed.contains(b));
        }
        if v.meeting.is_none() {
            v.meeting = bell;
        }
    }

    /// Beds other villagers call home.
    fn claimed_beds(&self) -> HashSet<BlockPos> {
        self.mobs
            .values()
            .filter_map(|m| m.villager.as_ref()?.home)
            .collect()
    }

    /// The bed heads around `at`, nearest first, and the nearest bell.
    fn scan_village(&self, at: BlockPos) -> (Vec<BlockPos>, Option<BlockPos>) {
        let mut beds: Vec<(i32, BlockPos)> = Vec::new();
        let mut bell: Option<(i32, BlockPos)> = None;
        for dy in -POI_REACH_UP..=POI_REACH_UP {
            for dx in -POI_REACH..=POI_REACH {
                for dz in -POI_REACH..=POI_REACH {
                    let q = BlockPos::new(at.x + dx, at.y + dy, at.z + dz);
                    let Some(s) = self.loaded_state(q) else {
                        continue;
                    };
                    let d = dx * dx + dy * dy + dz * dz;
                    if blocks::is_bed(s)
                        && prop(s, "part") == Some("head")
                        && !blocks::is_straw_bed(s)
                    {
                        beds.push((d, q));
                    } else if blocks::name(s) == "minecraft:bell" && bell.is_none_or(|(b, _)| d < b)
                    {
                        bell = Some((d, q));
                    }
                }
            }
        }
        beds.sort_by_key(|&(d, _)| d);
        (
            beds.into_iter().map(|(_, b)| b).collect(),
            bell.map(|(_, b)| b),
        )
    }

    /// The villager lies down in its bed, if no player is in it.
    fn sleep_villager(&mut self, id: i32, mob: &mut Mob, head: BlockPos) {
        let state = self.block(head);
        if !blocks::is_bed(state) {
            return;
        }
        if self.players.values().any(|p| p.sleeping == Some(head)) {
            return;
        }
        self.set_bed_occupied(head, true);
        mob.pos = [
            f64::from(head.x) + 0.5,
            f64::from(head.y) + 0.6875,
            f64::from(head.z) + 0.5,
        ];
        mob.target = None;
        mob.path.clear();
        if let Some(v) = mob.villager.as_mut() {
            v.sleeping = Some(head);
        }
        self.send_to_viewers(mob, super::time::sleep_data(id, Some(head)));
    }

    /// The villager gets up and stands beside its bed.
    pub(super) fn wake_villager(&mut self, id: i32, mob: &mut Mob) {
        let Some(bed) = mob.villager.as_mut().and_then(|v| v.sleeping.take()) else {
            return;
        };
        if blocks::is_bed(self.block(bed)) {
            self.set_bed_occupied(bed, false);
        }
        mob.pos[1] = f64::from(bed.y) + 0.5625;
        self.send_to_viewers(mob, super::time::sleep_data(id, None));
    }

    /// Picks up wanted items it touches, into its inventory.
    fn villager_pickup(&mut self, id: i32, mob: &mut Mob) {
        let near: Vec<i32> = self
            .items
            .iter()
            .filter(|(_, item)| {
                let p = item.pos();
                (p[0] - mob.pos[0]).abs() <= 1.3
                    && (p[2] - mob.pos[2]).abs() <= 1.3
                    && (-0.5..=2.5).contains(&(p[1] - mob.pos[1]))
                    && wanted(item.stack_ref().item)
                    && item.can_be_picked_up()
            })
            .map(|(&iid, _)| iid)
            .collect();
        for iid in near {
            let Some(v) = mob.villager.as_mut() else {
                return;
            };
            let stack = self.items[&iid].stack_ref();
            let left = v.stow(stack);
            let picked = stack.count - left.count;
            if picked == 0 {
                continue;
            }
            self.broadcast(
                Writer::packet(clientbound::TAKE_ITEM_ENTITY)
                    .varint(iid)
                    .varint(id)
                    .varint(picked),
                None,
            );
            if left.is_empty() {
                self.items.remove(&iid);
                self.broadcast(super::items::remove_entity(iid), None);
            } else {
                self.set_item_stack(iid, left);
            }
        }
    }

    /// A farmer harvests the nearest ripe crop around it (walking there first) and
    /// replants it. Returns whether it is busy with a crop.
    fn farm(&mut self, mob: &mut Mob) -> bool {
        let at = feet(mob.pos);
        let mut best: Option<(i32, BlockPos)> = None;
        for dy in -1..=1 {
            for dx in -FARM_REACH..=FARM_REACH {
                for dz in -FARM_REACH..=FARM_REACH {
                    let q = BlockPos::new(at.x + dx, at.y + dy, at.z + dz);
                    let Some(s) = self.loaded_state(q) else {
                        continue;
                    };
                    let ripe = crop_max_age(s).is_some_and(|max| age(s) >= max)
                        && seed_of(blocks::name(s)).is_some();
                    let d = dx * dx + dy * dy + dz * dz;
                    if ripe && best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, q));
                    }
                }
            }
        }
        let Some((_, crop)) = best else {
            return false;
        };
        if centre_distance(mob.pos, crop) > 1.5 {
            self.follow_path(mob, crop);
            return true;
        }
        let state = self.block(crop);
        let drops = self.loot.block_drops(&mut LootContext {
            state,
            tool: 0,
            rng: &mut self.rng,
            enchants: Default::default(),
            entity: Default::default(),
        });
        self.broadcast(
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(2001)
                .position(crop)
                .i32(i32::from(state))
                .bool(false),
            None,
        );
        self.set_block_updating(crop, block::AIR);
        let Some(v) = mob.villager.as_mut() else {
            return true;
        };
        let mut spill = Vec::new();
        for (item, count) in drops {
            let left = v.stow(Stack::new(item, count));
            if !left.is_empty() {
                spill.push(left);
            }
        }
        // Replant from what it carries.
        let seed = seed_of(blocks::name(state)).and_then(ItemInfo::id_of);
        if let Some(seed) = seed {
            if v.take_one(seed) {
                self.set_block_updating(crop, with_age(BlockInfo::of(state).default_state, 0));
            }
        }
        for s in spill {
            self.drop_at_block(crop, s);
        }
        mob.target = None;
        mob.path.clear();
        true
    }

    /// A villager with plenty of food throws some to a hungry neighbour nearby.
    fn share_food(&mut self, id: i32, mob: &mut Mob) {
        let Some(v) = mob.villager.as_mut() else {
            return;
        };
        if v.food() <= SHARE_ABOVE {
            return;
        }
        let hungry = self
            .mobs
            .iter()
            .filter(|(&other, m)| {
                other != id
                    && m.kind == MobKind::Villager
                    && m.alive()
                    && (m.pos[0] - mob.pos[0]).hypot(m.pos[2] - mob.pos[2]) <= MEET_REACH * 2.0
                    && m.villager
                        .as_ref()
                        .is_some_and(|ov| ov.food() < FOOD_TO_BREED)
            })
            .map(|(&other, _)| other)
            .next();
        let Some(other) = hungry else {
            return;
        };
        // Half of the richest food stack goes.
        let Some(i) = v
            .inventory
            .iter()
            .enumerate()
            .filter(|(_, s)| food_points(s.item) > 0)
            .max_by_key(|(_, s)| food_points(s.item) * s.count)
            .map(|(i, _)| i)
        else {
            return;
        };
        let s = v.inventory[i];
        let given = (s.count / 2).max(1);
        if s.count - given <= 0 {
            v.inventory.remove(i);
        } else {
            v.inventory[i] = s.with_count(s.count - given);
        }
        let target = self.mobs[&other].pos;
        let from = [mob.pos[0], mob.pos[1] + 1.3, mob.pos[2]];
        let d = [target[0] - from[0], target[2] - from[2]];
        let len = d[0].hypot(d[1]).max(0.1);
        self.spawn_item_moving(
            s.with_count(given),
            from,
            [d[0] / len * 0.3, 0.2, d[1] / len * 0.3],
        );
    }

    /// An adult with enough food and a free bed about becomes willing to breed.
    fn maybe_willing(&mut self, mob: &mut Mob) {
        let Some(v) = mob.villager.as_ref() else {
            return;
        };
        if mob.age != 0 || mob.love > 0 || v.food() < FOOD_TO_BREED || v.sleeping.is_some() {
            return;
        }
        let claimed = self.claimed_beds();
        let (beds, _) = self.scan_village(feet(mob.pos));
        if beds.iter().all(|b| claimed.contains(b)) {
            return;
        }
        mob.love = WILLING_TICKS;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{offset, Dir};

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-village-{}-{}",
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
        w.chunk(0, 0);
        w
    }

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    #[test]
    fn the_day_follows_the_vanilla_schedule() {
        assert_eq!(activity(1_000, false), Activity::Idle);
        assert_eq!(activity(3_000, false), Activity::Work);
        assert_eq!(activity(3_000, true), Activity::Idle);
        assert_eq!(activity(10_000, false), Activity::Meet);
        assert_eq!(activity(11_500, false), Activity::Idle);
        assert_eq!(activity(13_000, false), Activity::Rest);
        assert_eq!(activity(5, true), Activity::Rest);
    }

    #[test]
    fn villagers_claim_a_bed_and_sleep_in_it_at_night() {
        let mut w = world();
        let y = -60;
        let head = BlockPos::new(6, y, 4);
        let foot = BlockPos::new(6, y, 5);
        let bed = state("minecraft:red_bed");
        w.set_block(
            head,
            blocks::with(blocks::with(bed, "part", "head"), "facing", "north"),
        );
        w.set_block(
            foot,
            blocks::with(blocks::with(bed, "part", "foot"), "facing", "north"),
        );
        let id = next_entity_id();
        let mut mob = Mob::new(MobKind::Villager, [2.5, f64::from(y), 2.5], &mut w.rng);
        w.find_village_places(&mut mob);
        assert_eq!(mob.villager.as_ref().unwrap().home, Some(head));

        w.day_time = 13_000;
        mob.pos = [6.5, f64::from(y), 3.5];
        w.villager_life(id, &mut mob);
        assert_eq!(mob.villager.as_ref().unwrap().sleeping, Some(head));
        assert_eq!(prop(w.block(head), "occupied"), Some("true"));
        // Morning wakes it.
        w.day_time = 24_000 + 100;
        w.villager_life(id, &mut mob);
        assert_eq!(mob.villager.as_ref().unwrap().sleeping, None);
        assert_eq!(prop(w.block(head), "occupied"), Some("false"));
    }

    #[test]
    fn farmers_harvest_and_replant_and_food_counts() {
        let mut w = world();
        let y = -60;
        let soil = BlockPos::new(3, y - 1, 3);
        w.set_block(soil, state("minecraft:farmland"));
        let crop = offset(soil, Dir::Up);
        let wheat = state("minecraft:wheat");
        w.set_block(crop, with_age(wheat, crop_max_age(wheat).unwrap()));
        let mut mob = Mob::new(MobKind::Villager, [3.5, f64::from(y), 3.9], &mut w.rng);
        let v = mob.villager.as_mut().unwrap();
        v.profession = Profession::Farmer;
        assert!(w.farm(&mut mob));
        let v = mob.villager.as_ref().unwrap();
        let wheat_item = ItemInfo::id_of("minecraft:wheat").unwrap();
        assert!(v.inventory.iter().any(|s| s.item == wheat_item));
        // Replanted with a seed from what it harvested.
        assert_eq!(blocks::name(w.block(crop)), "minecraft:wheat");
        assert_eq!(age(w.block(crop)), 0);

        let mut v = Villager::new("minecraft:plains");
        let bread = ItemInfo::id_of("minecraft:bread").unwrap();
        v.stow(Stack::new(bread, 3));
        assert_eq!(v.food(), 12);
        assert!(v.eat(FOOD_TO_BREED));
        assert_eq!(v.food(), 0);
    }

    #[test]
    fn fed_villagers_with_a_free_bed_have_a_baby() {
        let mut w = world();
        let y = -60;
        let bed = state("minecraft:white_bed");
        w.set_block(BlockPos::new(8, y, 8), blocks::with(bed, "part", "head"));
        let bread = ItemInfo::id_of("minecraft:bread").unwrap();
        let mut ids = Vec::new();
        for x in [4.5, 5.5] {
            let mut m = Mob::new(MobKind::Villager, [x, f64::from(y), 4.5], &mut w.rng);
            m.villager.as_mut().unwrap().stow(Stack::new(bread, 3));
            w.maybe_willing(&mut m);
            assert!(m.love > 0, "fed and with a bed about, it is willing");
            let id = next_entity_id();
            w.mobs.insert(id, m);
            ids.push(id);
        }
        let mut m = w.mobs.remove(&ids[0]).unwrap();
        w.tick_breeding(ids[0], &mut m);
        w.mobs.insert(ids[0], m);
        let babies: Vec<&Mob> = w
            .mobs
            .values()
            .filter(|m| m.kind == MobKind::Villager && m.age < 0)
            .collect();
        assert_eq!(babies.len(), 1);
        assert!(babies[0].villager.is_some());
        // Both parents ate their food.
        for id in ids {
            assert_eq!(w.mobs[&id].villager.as_ref().unwrap().food(), 0);
        }
    }

    #[test]
    fn bells_ring_from_their_sides_and_send_villagers_home() {
        let mut w = world();
        let pos = BlockPos::new(5, -60, 5);
        // A floor bell facing north swings along north-south.
        let bell = blocks::with(
            blocks::with(state("minecraft:bell"), "attachment", "floor"),
            "facing",
            "north",
        );
        w.set_block(pos, bell);
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let id = next_entity_id();
        let profile = crate::connection::Profile {
            name: "Ringer".to_owned(),
            uuid: 11,
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, out, [5.5, -60.0, 3.5]));
        assert!(
            !w.use_bell(id, pos, bell, Dir::East, 0.5),
            "not from the side it does not swing to"
        );
        assert!(
            !w.use_bell(id, pos, bell, Dir::North, 0.9),
            "not on its beam"
        );
        let pillager = next_entity_id();
        w.mobs.insert(
            pillager,
            Mob::new(MobKind::Pillager, [20.5, -60.0, 5.5], &mut w.rng),
        );
        assert!(w.use_bell(id, pos, bell, Dir::North, 0.5));
        assert!(w.bell_alarm_near([10.0, -60.0, 10.0]));
        assert!(!w.bell_alarm_near([60.0, -60.0, 60.0]));
        assert!(w.mobs[&pillager]
            .effect(levitas_registry::mob_effect::GLOWING)
            .is_some());
    }
}
