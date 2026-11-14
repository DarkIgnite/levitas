//! Everyday goals of walking mobs: wandering, panicking, getting out of water, doors.
//!
//! Behaviour follows the Minecraft Wiki (Mob AI, Villager, Zombie, Door, Difficulty,
//! Game rule):
//! - Wandering mobs now and then pick a random spot up to 10 blocks away (and up to 3
//!   up or down; a spot inside a block moves up to the air above it) and walk there
//!   along a path (`pathfinding.rs`), not in a straight line. They only pick spots they
//!   can reach and that cost nothing to stand on, so they keep out of water, lava,
//!   fire, berry bushes, powder snow, magma and cactus, and never step off a drop of
//!   more than 3 blocks.
//! - A panicking mob runs to spots up to 5 blocks away, along paths and away from what
//!   hurt or scared it; with nowhere to go it just runs off that way.
//! - Mobs that cannot swim float in water ("bounce on the surface, preferring land") and
//!   head for the nearest land they find. Animals do not avoid the sun: the wiki lists
//!   no such goal for them.
//! - Villagers open the wooden and copper doors they path through and close them once
//!   past, unless someone is in the doorway; they cannot open iron doors, trapdoors or
//!   fence gates. Other mobs treat closed doors as walls.
//! - Up to 10% of zombies (10% times the clamped regional difficulty when they first
//!   act; zombie villagers and husks too) path through closed wooden and copper doors,
//!   and on Hard, if mob griefing is on, break them down: about 10 seconds of hitting
//!   (`entity.zombie.attack_wooden_door`, level event 1019) with the crack growing
//!   (Set Block Destroy Stage), then the door breaks (level event 1021 and the break
//!   particles, 2001) and drops nothing. Saved as vanilla: `CanBreakDoors`.
//!
//! Paths are only worked out when a new spot is picked (and every second while
//! hunting, `follow_path`), so wandering costs next to nothing between picks.

use levitas_nbt::Compound;

use super::mobs::{Mob, MobKind};
use super::pathfinding::{advance_path, closed_mob_door, mob_feet, PathRules};
use super::*;
use crate::blocks::{self, prop, with_bool};

/// How far a wandering mob looks for a spot, sideways and up or down.
const WANDER_REACH: i32 = 10;
const WANDER_HEIGHT: i32 = 3;
/// How far a panicking mob runs before picking another spot.
const PANIC_REACH: f64 = 5.0;
/// One tick in this many a mob with nothing to do starts wandering.
const WANDER_ONE_IN: u32 = 120;
/// Spots tried per pick, and when looking for land from the water.
const TRIES: usize = 3;
const LAND_TRIES: usize = 8;
/// Ticks between looks for land while floating.
const LAND_WAIT: u8 = 20;
/// Ticks a mob is given per spot of its path, on top of a little slack.
const TICKS_PER_SPOT: u32 = 20;
const SLACK_TICKS: u32 = 60;
/// A mob farther than this from the next spot of its path was pushed off it.
const OFF_PATH: f64 = 3.0;
/// How close a door must be to be opened or broken, and how far past it a villager
/// closes it again (and gives up on it).
const DOOR_REACH: f64 = 1.5;
const CLOSE_AFTER: f64 = 2.0;
const FORGET_DOOR: f64 = 8.0;
/// Ticks of hitting a door before it breaks: "approximately 10 seconds".
pub(super) const BREAK_TICKS: u32 = 200;
/// One tick in this many a zombie hitting a door makes it heard.
const KNOCK_ONE_IN: u32 = 20;
/// Level events.
const ATTACK_WOODEN_DOOR: i32 = 1019;
const BREAK_WOODEN_DOOR: i32 = 1021;
const BLOCK_BREAK: i32 = 2001;
/// Chance of a zombie being able to break doors, at full clamped regional difficulty.
const BREAK_DOORS_CHANCE: f64 = 0.1;

/// A mob's own walking goals.
#[derive(Debug, Default)]
pub struct Ai {
    /// The spots still to walk through to where it wanders or flees, and ticks left to
    /// get there.
    path: Vec<BlockPos>,
    ticks: u32,
    /// The spot it last aimed for along that path: a different target means another
    /// goal took over.
    aim: Option<[f64; 2]>,
    /// The way away from what last hurt or scared it (a horizontal unit vector).
    pub(super) flee: Option<[f64; 2]>,
    /// Ticks until a mob floating in water looks for land again.
    land_wait: u8,
    /// Zombies: whether it can break doors, once decided.
    pub(super) break_doors: Option<bool>,
    /// The door being broken, and for how many ticks it has been hit.
    breaking: Option<(BlockPos, u32)>,
    /// Villagers: doors it opened, to close behind it.
    opened: Vec<BlockPos>,
}

impl Ai {
    /// Whether it is on its way somewhere it picked itself.
    pub(super) fn walking(&self) -> bool {
        !self.path.is_empty()
    }

    /// Something hurt or scared it from the other side of `away` (a horizontal
    /// direction): it drops where it was going and, once `fresh`, runs off that way.
    /// A direction too short to tell (harming clouds, conduits) leaves it running
    /// about at random.
    pub(super) fn scared(&mut self, away: [f64; 2], fresh: bool) {
        let length = away[0].hypot(away[1]);
        self.flee = (length > 0.1).then(|| [away[0] / length, away[1] / length]);
        if fresh {
            self.path.clear();
            self.aim = None;
        }
    }
}

/// Mobs that can break doors.
fn breaks_doors(kind: MobKind) -> bool {
    matches!(
        kind,
        MobKind::Zombie | MobKind::Husk | MobKind::ZombieVillager
    )
}

/// The regional difficulty (0 to 6.75) for `difficulty`, the total day time in ticks,
/// a chunk's inhabited time in ticks and the moon phase (0 for a full moon), as the
/// wiki's Difficulty page works it out.
pub(super) fn regional_difficulty(
    difficulty: crate::rules::Difficulty,
    day_time: i64,
    inhabited: i64,
    moon_phase: i64,
) -> f64 {
    use crate::rules::Difficulty;
    if difficulty == Difficulty::Peaceful {
        return 0.0;
    }
    let day = if day_time > 63 * 24_000 {
        0.25
    } else if day_time < 3 * 24_000 {
        0.0
    } else {
        (day_time - 72_000) as f64 / 5_760_000.0
    };
    let mut chunk = if inhabited > 3_600_000 {
        1.0
    } else {
        inhabited as f64 / 3_600_000.0
    };
    if matches!(difficulty, Difficulty::Easy | Difficulty::Normal) {
        chunk *= 0.75;
    }
    let moon = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75][moon_phase.rem_euclid(8) as usize];
    chunk += f64::min(moon * 0.25, day);
    if difficulty == Difficulty::Easy {
        chunk *= 0.5;
    }
    let regional = 0.75 + day + chunk;
    match difficulty {
        Difficulty::Normal => regional * 2.0,
        Difficulty::Hard => regional * 3.0,
        _ => regional,
    }
}

/// The clamped regional difficulty (0 to 1).
pub(super) fn clamped(regional: f64) -> f64 {
    ((regional - 2.0) / 2.0).clamp(0.0, 1.0)
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    if let Some(can) = mob.ai.break_doors {
        nbt.insert("CanBreakDoors", can);
    }
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    if breaks_doors(mob.kind) {
        mob.ai.break_doors = nbt.int("CanBreakDoors").map(|b| b != 0);
    }
}

fn centre(pos: BlockPos) -> [f64; 2] {
    [f64::from(pos.x) + 0.5, f64::from(pos.z) + 0.5]
}

fn distance_to(mob: &Mob, pos: BlockPos) -> f64 {
    let c = centre(pos);
    (c[0] - mob.pos[0]).hypot(c[1] - mob.pos[2])
}

impl World {
    /// A mob's walk for this tick, while nothing more pressing (food, a partner, prey,
    /// a villager's day) steers it: it keeps to the path it picked, and now and then
    /// picks a new spot. Runs before those other goals, which win by steering after it.
    pub(super) fn wander(&mut self, mob: &mut Mob, panicking: bool) {
        if mob.kind.flies()
            || mob.vehicle.is_some()
            || ((mob.kind.swims() || super::critters::amphibious(mob.kind))
                && self.in_water(mob.pos))
        {
            mob.ai.path.clear();
            return;
        }
        // Hunting or chasing prey steers along its own path.
        if mob.hunting.is_some() || mob.prey.is_some() {
            mob.ai.path.clear();
        }
        let panicking = panicking || mob.panic_ticks > 0;
        if !panicking {
            mob.ai.flee = None;
        }
        let rules = PathRules::of(mob);
        let wet = rules.water != 0
            && (self.in_water(mob.pos)
                || self.in_water([mob.pos[0], mob.pos[1] - 0.3, mob.pos[2]]));
        mob.ai.land_wait = mob.ai.land_wait.saturating_sub(1);

        if mob.ai.walking() {
            if mob.target.is_some_and(|(t, _)| Some(t) != mob.ai.aim) {
                // Another goal (food, a partner, a module of its own) sent it elsewhere.
                mob.ai.path.clear();
                mob.ai.aim = None;
                return;
            }
            mob.ai.ticks = mob.ai.ticks.saturating_sub(1);
            advance_path(mob.pos, &mut mob.ai.path);
            match mob.ai.path.first().copied() {
                Some(next) if mob.ai.ticks > 0 && distance_to(mob, next) <= OFF_PATH => {
                    mob.target = Some((centre(next), 20));
                    mob.ai.aim = Some(centre(next));
                    return;
                }
                // Arrived, too late, or pushed off the way.
                _ => mob.ai.path.clear(),
            }
        }

        let wants = if panicking {
            mob.target.is_none()
        } else if wet {
            mob.ai.land_wait == 0
        } else {
            mob.target.is_none()
                && mob.hunting.is_none()
                && mob.prey.is_none()
                && self.rng.below(WANDER_ONE_IN) == 0
        };
        if !wants {
            return;
        }
        match self.pick_spot(mob, rules, panicking, wet) {
            Some(path) => {
                mob.ai.ticks = (path.len() as u32 * TICKS_PER_SPOT + SLACK_TICKS).min(400);
                mob.target = Some((centre(path[0]), 20));
                mob.ai.aim = Some(centre(path[0]));
                mob.ai.path = path;
            }
            None if panicking => {
                // Nowhere to run to along a path: run off anyway, away if it can.
                let (dx, dz) = match mob.ai.flee {
                    Some(away) => (away[0] * PANIC_REACH, away[1] * PANIC_REACH),
                    None => (
                        (self.rng.f64() * 2.0 - 1.0) * PANIC_REACH,
                        (self.rng.f64() * 2.0 - 1.0) * PANIC_REACH,
                    ),
                };
                mob.target = Some(([mob.pos[0] + dx, mob.pos[2] + dz], 40));
            }
            None => {
                if wet {
                    mob.ai.land_wait = LAND_WAIT;
                }
            }
        }
    }

    /// A spot for a mob to walk to and the path there: anywhere harmless nearby, away
    /// from danger when panicking, or out of the water.
    fn pick_spot(
        &mut self,
        mob: &Mob,
        rules: PathRules,
        panicking: bool,
        wet: bool,
    ) -> Option<Vec<BlockPos>> {
        let height = mob.size().1.ceil() as i32;
        let from = mob_feet(mob);
        let tries = if wet { LAND_TRIES } else { TRIES };
        for _ in 0..tries {
            let (dx, dz) = match (panicking, mob.ai.flee) {
                (true, Some(away)) => {
                    // Ahead, away from the danger, give or take a little to the side.
                    let ahead = 2.0 + self.rng.f64() * (PANIC_REACH - 2.0);
                    let side = (self.rng.f64() * 2.0 - 1.0) * 2.0;
                    (
                        away[0] * ahead - away[1] * side,
                        away[1] * ahead + away[0] * side,
                    )
                }
                (true, None) => {
                    let r = |rng: &mut crate::rng::Rng| (rng.f64() * 2.0 - 1.0) * PANIC_REACH;
                    (r(&mut self.rng), r(&mut self.rng))
                }
                _ => {
                    let span = (WANDER_REACH * 2 + 1) as u32;
                    (
                        f64::from(self.rng.below(span) as i32 - WANDER_REACH),
                        f64::from(self.rng.below(span) as i32 - WANDER_REACH),
                    )
                }
            };
            let dy = self.rng.below((WANDER_HEIGHT * 2 + 1) as u32) as i32 - WANDER_HEIGHT;
            let x = (mob.pos[0] + dx).floor() as i32;
            let z = (mob.pos[2] + dz).floor() as i32;
            let Some(spot) = self.settle(x, from.y + dy, z, height, rules, wet) else {
                continue;
            };
            if spot == from {
                continue;
            }
            let Some(path) = self.find_path_for(from, spot, height, rules) else {
                continue;
            };
            if path.last() == Some(&spot) {
                return Some(path);
            }
        }
        None
    }

    /// The spot in column (`x`, `z`) near `y` a mob would walk to: moved up out of
    /// blocks and down to the ground. Only harmless spots count, and only dry ones when
    /// it is looking for land.
    fn settle(
        &self,
        x: i32,
        y: i32,
        z: i32,
        height: i32,
        rules: PathRules,
        wet: bool,
    ) -> Option<BlockPos> {
        let fits = |y: i32| {
            (0..height).all(|dy| {
                self.loaded_state(BlockPos::new(x, y + dy, z))
                    .is_some_and(|s| levitas_registry::collision_boxes(s).is_empty())
            })
        };
        let mut y = y;
        let mut raised = 0;
        while !fits(y) {
            y += 1;
            raised += 1;
            if raised > WANDER_HEIGHT + 1 {
                return None;
            }
        }
        for _ in 0..=WANDER_HEIGHT * 2 {
            let pos = BlockPos::new(x, y, z);
            if let Some(cost) = self.spot_cost(pos, height, rules) {
                let dry = !self
                    .loaded_state(pos)
                    .is_some_and(|s| crate::blocks::fluid::fluid_at(s).is_some());
                return if wet {
                    dry.then_some(pos)
                } else {
                    (cost == 0).then_some(pos)
                };
            }
            y -= 1;
            if !fits(y) {
                return None;
            }
        }
        None
    }

    /// Doors on a mob's way: villagers open and close them, zombies that can break them
    /// do so on Hard. Runs after the mob decided where to go, before it moves.
    pub(super) fn mob_doors(&mut self, id: i32, mob: &mut Mob) {
        if mob.kind == MobKind::Villager {
            self.villager_doors(mob);
        } else if breaks_doors(mob.kind) {
            if mob.ai.break_doors.is_none() {
                let inhabited = self
                    .kept
                    .get(&mob.chunk())
                    .map_or(0, |k| k.inhabited_time());
                let regional = regional_difficulty(
                    crate::rules::difficulty(),
                    self.day_time,
                    inhabited,
                    self.day_time.div_euclid(24_000),
                );
                mob.ai.break_doors = Some(self.rng.f64() < BREAK_DOORS_CHANCE * clamped(regional));
            }
            let hard = crate::rules::difficulty() == crate::rules::Difficulty::Hard
                && crate::rules::bool("mob_griefing");
            self.break_doors(id, mob, hard);
        }
    }

    /// The closed door (its lower half) on the next spots of a mob's way, if it is
    /// close enough to reach.
    fn door_ahead(&self, mob: &Mob) -> Option<BlockPos> {
        let feet = mob_feet(mob);
        // Only the path it is steering along: a hunting or villager-life path, else
        // its wander path.
        let path = if mob.path.is_empty() {
            &mob.ai.path
        } else {
            &mob.path
        };
        path.iter()
            .take(2)
            .flat_map(|&p| [p, BlockPos::new(p.x, p.y + 1, p.z)])
            .find(|&p| {
                (p.y - feet.y).abs() <= 1
                    && distance_to(mob, p) < DOOR_REACH
                    && self
                        .loaded_state(p)
                        .is_some_and(|s| closed_mob_door(s) && prop(s, "half") == Some("lower"))
            })
    }

    fn villager_doors(&mut self, mob: &mut Mob) {
        if let Some(door) = self.door_ahead(mob) {
            let state = self.block(door);
            self.set_block_updating(door, with_bool(state, "open", true));
            self.play_sound(&super::building::toggle_sound(state, true), door, None);
            if !mob.ai.opened.contains(&door) {
                mob.ai.opened.push(door);
            }
        }
        if mob.ai.opened.is_empty() || !self.tick.is_multiple_of(5) {
            return;
        }
        let mut opened = std::mem::take(&mut mob.ai.opened);
        opened.retain(|&door| {
            let far = distance_to(mob, door);
            if far > FORGET_DOOR {
                return false;
            }
            let still_on_way = mob
                .path
                .iter()
                .take(2)
                .any(|p| p.x == door.x && p.z == door.z);
            if far < CLOSE_AFTER || still_on_way {
                return true;
            }
            let state = self.block(door);
            if !blocks::is_door(state) || prop(state, "open") != Some("true") {
                return false;
            }
            let c = [
                f64::from(door.x) + 0.5,
                f64::from(door.y),
                f64::from(door.z) + 0.5,
            ];
            let in_doorway = |p: [f64; 3]| {
                (p[0] - c[0]).abs() < 0.9 && (p[2] - c[2]).abs() < 0.9 && (p[1] - c[1]).abs() < 2.0
            };
            let blocked = self.mobs.values().any(|m| in_doorway(m.pos))
                || self.players.values().any(|p| p.alive && in_doorway(p.pos));
            if blocked {
                return true;
            }
            self.set_block_updating(door, with_bool(state, "open", false));
            self.play_sound(&super::building::toggle_sound(state, false), door, None);
            false
        });
        mob.ai.opened = opened;
    }

    /// A zombie that can break doors hitting the one in its way; `hard` is whether
    /// that ever succeeds (Hard, with mob griefing on).
    pub(super) fn break_doors(&mut self, id: i32, mob: &mut Mob, hard: bool) {
        let door = if hard && mob.ai.break_doors == Some(true) {
            self.door_ahead(mob)
        } else {
            None
        };
        let previous = mob.ai.breaking.take();
        if let Some((old, _)) = previous.filter(|&(old, _)| Some(old) != door) {
            self.send_destroy_stage(id, old, None);
        }
        let Some(door) = door else {
            return;
        };
        let ticks = previous
            .filter(|&(old, _)| old == door)
            .map_or(0, |(_, t)| t + 1);
        // It stands and hits the door.
        mob.target = None;
        mob.yaw = super::mobs::yaw_towards(
            mob.pos,
            [f64::from(door.x) + 0.5, mob.pos[1], f64::from(door.z) + 0.5],
        );
        if self.rng.below(KNOCK_ONE_IN) == 0 {
            self.level_event(ATTACK_WOODEN_DOOR, door, 0);
        }
        let stage = |t: u32| (t * 10 / BREAK_TICKS) as u8;
        if ticks == 0 || stage(ticks) != stage(ticks - 1) {
            self.send_destroy_stage(id, door, Some(stage(ticks).min(9)));
        }
        if ticks < BREAK_TICKS {
            mob.ai.breaking = Some((door, ticks));
            return;
        }
        let state = self.block(door);
        self.send_destroy_stage(id, door, None);
        self.level_event(BREAK_WOODEN_DOOR, door, 0);
        self.level_event(BLOCK_BREAK, door, i32::from(state));
        self.set_block(BlockPos::new(door.x, door.y + 1, door.z), block::AIR);
        self.set_block_updating(door, block::AIR);
        mob.path.clear();
    }

    /// A mob that dies or goes away while hitting a door takes its crack away.
    pub(super) fn stop_breaking(&mut self, id: i32, mob: &mut Mob) {
        if let Some((door, _)) = mob.ai.breaking.take() {
            self.send_destroy_stage(id, door, None);
        }
    }

    /// The crack on a block being broken by mob `id`; `None` takes it away.
    fn send_destroy_stage(&mut self, id: i32, pos: BlockPos, stage: Option<u8>) {
        self.broadcast(
            Writer::packet(clientbound::BLOCK_DESTRUCTION)
                .varint(id)
                .position(pos)
                .u8(stage.unwrap_or(u8::MAX)),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Difficulty;

    fn flat_world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-ai-{}-{}",
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
        for cx in -2..=2 {
            for cz in -2..=2 {
                w.chunk(cx, cz);
            }
        }
        w
    }

    fn fill(w: &mut World, from: (i32, i32, i32), to: (i32, i32, i32), state: u16) {
        for x in from.0..=to.0 {
            for y in from.1..=to.1 {
                for z in from.2..=to.2 {
                    w.set_block(BlockPos::new(x, y, z), state);
                }
            }
        }
    }

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).expect(name).default_state
    }

    /// A closed door's two halves at `pos`.
    fn place_door(w: &mut World, pos: BlockPos, name: &str) {
        let lower = BlockInfo::by_name(name).expect(name).default_state;
        let lower = blocks::with(blocks::with(lower, "half", "lower"), "facing", "east");
        let lower = with_bool(lower, "open", false);
        w.set_block(pos, lower);
        w.set_block(
            BlockPos::new(pos.x, pos.y + 1, pos.z),
            blocks::with(lower, "half", "upper"),
        );
    }

    /// A wall at x = 3 too long to walk round, three blocks high, with a door in the
    /// middle.
    fn wall_with_door(w: &mut World, door: &str) -> BlockPos {
        fill(w, (3, -60, -30), (3, -58, 30), block::STONE);
        let pos = BlockPos::new(3, -60, 0);
        place_door(w, pos, door);
        pos
    }

    /// Runs a mob's turns as the shared mob code does, without the rest of the world.
    fn run(w: &mut World, id: i32, mob: &mut Mob, ticks: u32) {
        for _ in 0..ticks {
            w.tick += 1;
            w.think(mob);
            w.mob_doors(id, mob);
            w.step(mob);
        }
    }

    #[test]
    fn paths_keep_out_of_fire_and_berry_bushes() {
        let mut w = flat_world();
        // A row of fire with a gap at z = 4, and berry bushes with a gap at z = -4.
        for z in -6..=6 {
            if z != 4 {
                w.set_block(BlockPos::new(3, -60, z), state("minecraft:fire"));
            }
            if z != -4 {
                w.set_block(
                    BlockPos::new(6, -60, z),
                    state("minecraft:sweet_berry_bush"),
                );
            }
        }
        let cow = PathRules::of_kind(MobKind::Cow);
        let goal = BlockPos::new(9, -60, 0);
        let path = w
            .find_path_for(BlockPos::new(0, -60, 0), goal, 2, cow)
            .expect("a way");
        assert_eq!(path.last(), Some(&goal));
        let hurts = |p: &BlockPos| {
            (p.x == 3 && p.z != 4 && p.z.abs() <= 6) || (p.x == 6 && p.z != -4 && p.z.abs() <= 6)
        };
        assert!(!path.iter().any(hurts), "{path:?}");
        // Lava is never walked through, not even by monsters.
        fill(&mut w, (12, -61, -6), (12, -61, 6), state("minecraft:lava"));
        let zombie = PathRules::of_kind(MobKind::Zombie);
        let far = BlockPos::new(14, -60, 0);
        let path = w.find_path_for(BlockPos::new(10, -60, 0), far, 2, zombie);
        assert!(path.is_none_or(|p| p.iter().all(|s| s.x != 12 || s.z.abs() > 6)));
    }

    #[test]
    fn water_costs_land_animals_but_not_fish() {
        let w = flat_world();
        let mut w = w;
        w.set_block(BlockPos::new(0, -61, 0), state("minecraft:water"));
        w.set_block(BlockPos::new(0, -60, 0), state("minecraft:water"));
        let spot = BlockPos::new(0, -60, 0);
        let cow = PathRules::of_kind(MobKind::Cow);
        assert_eq!(w.spot_cost(spot, 2, cow), Some(80));
        assert_eq!(
            w.spot_cost(BlockPos::new(1, -60, 0), 2, cow),
            Some(80),
            "the border"
        );
        assert_eq!(w.spot_cost(BlockPos::new(5, -60, 0), 2, cow), Some(0));
        let chicken = PathRules::of_kind(MobKind::Chicken);
        assert_eq!(w.spot_cost(spot, 1, chicken), Some(0));
        let sniffer = PathRules::of_kind(MobKind::Sniffer);
        assert_eq!(w.spot_cost(spot, 2, sniffer), None);
    }

    #[test]
    fn nothing_stands_on_fences_and_drops_stop_at_three() {
        let mut w = flat_world();
        w.set_block(BlockPos::new(0, -60, 0), state("minecraft:oak_fence"));
        let cow = PathRules::of_kind(MobKind::Cow);
        assert_eq!(w.spot_cost(BlockPos::new(0, -59, 0), 2, cow), None);
        // A tower five blocks up: no way down from it.
        fill(&mut w, (0, -60, 5), (0, -56, 5), block::STONE);
        let top = BlockPos::new(0, -55, 5);
        let path = w.find_path_for(top, BlockPos::new(2, -60, 5), 2, cow);
        assert!(path.is_none(), "{path:?}");
    }

    #[test]
    fn wandering_follows_paths_to_reachable_dry_spots() {
        let mut w = flat_world();
        // A pond on one side.
        fill(
            &mut w,
            (-8, -62, -8),
            (-2, -61, 8),
            state("minecraft:water"),
        );
        for _ in 0..30 {
            let mut cow = Mob::new(MobKind::Cow, [3.5, -60.0, 0.5], &mut w.rng);
            let rules = PathRules::of(&cow);
            let Some(path) = w.pick_spot(&cow, rules, false, false) else {
                continue;
            };
            let end = *path.last().unwrap();
            // Never in the water or next to it.
            assert_eq!(w.spot_cost(end, 2, rules), Some(0), "{end:?}");
            cow.ai.path = path;
            assert!(cow.ai.walking());
        }
        // A wandering cow walks round a wall rather than into it.
        fill(&mut w, (6, -60, -3), (6, -58, 3), block::STONE);
        let mut cow = Mob::new(MobKind::Cow, [4.5, -60.0, 0.5], &mut w.rng);
        let goal = BlockPos::new(8, -60, 0);
        let rules = PathRules::of(&cow);
        cow.ai.path = w.find_path_for(mob_feet(&cow), goal, 2, rules).unwrap();
        cow.ai.ticks = 400;
        let id = next_entity_id();
        for _ in 0..300 {
            if !cow.ai.walking() {
                break;
            }
            w.tick += 1;
            w.think(&mut cow);
            w.mob_doors(id, &mut cow);
            w.step(&mut cow);
        }
        assert!(
            (cow.pos[0] - 8.5).hypot(cow.pos[2] - 0.5) < 1.5,
            "got to {:?}",
            cow.pos
        );
    }

    #[test]
    fn panicking_animals_run_away_from_what_hurt_them() {
        let mut w = flat_world();
        let id = next_entity_id();
        let mut pig = Mob::new(MobKind::Pig, [0.5, -60.0, 0.5], &mut w.rng);
        pig.panic = 100;
        pig.ai.flee = Some([1.0, 0.0]);
        run(&mut w, id, &mut pig, 80);
        assert!(pig.pos[0] > 4.0, "ran to {:?}", pig.pos);
    }

    #[test]
    fn shy_rabbits_run_away_from_players() {
        let mut w = flat_world();
        let (out, _frames) = tokio::sync::mpsc::channel(1_000);
        let profile = Profile {
            name: "Indy".to_owned(),
            uuid: 7,
            properties: Vec::new(),
        };
        w.players.insert(
            next_entity_id(),
            Player::new(profile, out, [-2.5, -60.0, 0.5]),
        );
        let id = next_entity_id();
        let mut rabbit = Mob::new(MobKind::Rabbit, [0.5, -60.0, 0.5], &mut w.rng);
        // Walking a wander path towards the player when it notices them.
        rabbit.ai.path = vec![BlockPos::new(-1, -60, 0), BlockPos::new(-6, -60, 0)];
        rabbit.ai.ticks = 400;
        run(&mut w, id, &mut rabbit, 30);
        assert!(rabbit.pos[0] > 3.0, "ran to {:?}", rabbit.pos);
    }

    #[test]
    fn panic_without_a_direction_still_runs() {
        let mut w = flat_world();
        let id = next_entity_id();
        let mut pig = Mob::new(MobKind::Pig, [0.5, -60.0, 0.5], &mut w.rng);
        pig.panic = 100;
        pig.ai.scared([0.0, 0.0], true);
        assert_eq!(pig.ai.flee, None);
        run(&mut w, id, &mut pig, 60);
        assert!(
            (pig.pos[0] - 0.5).hypot(pig.pos[2] - 0.5) > 1.5,
            "stayed at {:?}",
            pig.pos
        );
    }

    #[test]
    fn other_goals_take_over_from_a_wander_path() {
        let mut w = flat_world();
        let id = next_entity_id();
        let mut cow = Mob::new(MobKind::Cow, [0.5, -60.0, 0.5], &mut w.rng);
        cow.ai.path = w
            .find_path_for(
                mob_feet(&cow),
                BlockPos::new(8, -60, 0),
                2,
                PathRules::of(&cow),
            )
            .unwrap();
        cow.ai.ticks = 400;
        run(&mut w, id, &mut cow, 5);
        assert!(cow.ai.walking());
        // Another module sends it the other way.
        cow.target = Some(([-8.0, 0.5], 200));
        run(&mut w, id, &mut cow, 1);
        assert!(!cow.ai.walking());
        assert_eq!(cow.target.map(|t| t.0), Some([-8.0, 0.5]));
    }

    #[test]
    fn land_animals_in_water_head_for_land() {
        let mut w = flat_world();
        // A pond two deep, 9 by 9.
        fill(&mut w, (-4, -62, -4), (4, -61, 4), state("minecraft:water"));
        let id = next_entity_id();
        let mut sheep = Mob::new(MobKind::Sheep, [0.5, -61.0, 0.5], &mut w.rng);
        run(&mut w, id, &mut sheep, 400);
        let wet =
            w.in_water(sheep.pos) || w.in_water([sheep.pos[0], sheep.pos[1] - 0.3, sheep.pos[2]]);
        assert!(!wet, "still in the water at {:?}", sheep.pos);
    }

    #[test]
    fn villagers_open_wooden_doors_and_close_them_behind() {
        let mut w = flat_world();
        let door = wall_with_door(&mut w, "minecraft:oak_door");
        let id = next_entity_id();
        let mut villager = Mob::new(MobKind::Villager, [0.5, -60.0, 0.5], &mut w.rng);
        let goal = BlockPos::new(7, -60, 0);
        let mut opened = false;
        for _ in 0..400 {
            w.tick += 1;
            w.follow_path(&mut villager, goal);
            w.mob_doors(id, &mut villager);
            w.step(&mut villager);
            opened |= prop(w.block(door), "open") == Some("true");
        }
        assert!(opened, "it opened the door");
        assert!(villager.pos[0] > 6.0, "got through: {:?}", villager.pos);
        assert_eq!(prop(w.block(door), "open"), Some("false"), "and closed it");
    }

    #[test]
    fn iron_doors_and_gates_stop_villagers_and_closed_doors_stop_animals() {
        let mut w = flat_world();
        let villager = PathRules::of_kind(MobKind::Villager);
        let cow = PathRules::of_kind(MobKind::Cow);
        let goal = BlockPos::new(6, -60, 0);
        let from = BlockPos::new(0, -60, 0);
        let reaches = |w: &World, rules| {
            w.find_path_for(from, goal, 2, rules)
                .is_some_and(|p| p.last() == Some(&goal))
        };
        // Wall it in completely, so that the door is the only way.
        fill(&mut w, (3, -60, -30), (3, -57, 30), block::STONE);
        let door = BlockPos::new(3, -60, 0);
        place_door(&mut w, door, "minecraft:iron_door");
        assert!(!reaches(&w, villager));
        place_door(&mut w, door, "minecraft:copper_door");
        assert!(reaches(&w, villager));
        assert!(!reaches(&w, cow));
        let open = with_bool(w.block(door), "open", true);
        w.set_block(door, open);
        let upper = with_bool(w.block(BlockPos::new(3, -59, 0)), "open", true);
        w.set_block(BlockPos::new(3, -59, 0), upper);
        assert!(reaches(&w, cow), "an open door lets anyone through");
        // Fence gates are walls to villagers.
        w.set_block(BlockPos::new(3, -59, 0), block::STONE);
        w.set_block(door, state("minecraft:oak_fence_gate"));
        assert!(!reaches(&w, villager));
    }

    #[test]
    fn zombies_that_can_break_doors_break_them_on_hard_only() {
        let mut w = flat_world();
        let door = wall_with_door(&mut w, "minecraft:oak_door");
        let goal = BlockPos::new(7, -60, 0);
        let id = next_entity_id();

        // One that cannot break doors stays behind it.
        let mut plain = Mob::new(MobKind::Zombie, [0.5, -60.0, 0.5], &mut w.rng);
        plain.ai.break_doors = Some(false);
        for _ in 0..300 {
            w.follow_path(&mut plain, goal);
            w.break_doors(id, &mut plain, true);
            w.step(&mut plain);
        }
        assert!(blocks::is_door(w.block(door)));
        assert!(plain.pos[0] < 3.0);

        // One that can, but not on Hard, hits it in vain.
        let mut breaker = Mob::new(MobKind::Zombie, [0.5, -60.0, 0.5], &mut w.rng);
        breaker.ai.break_doors = Some(true);
        for _ in 0..300 {
            w.follow_path(&mut breaker, goal);
            w.break_doors(id, &mut breaker, false);
            w.step(&mut breaker);
        }
        assert!(blocks::is_door(w.block(door)));

        // On Hard it takes about ten seconds.
        let mut ticks = 0;
        while blocks::is_door(w.block(door)) && ticks < 1000 {
            w.follow_path(&mut breaker, goal);
            w.break_doors(id, &mut breaker, true);
            w.step(&mut breaker);
            ticks += 1;
        }
        assert!(is_air(w.block(door)), "broken after {ticks} ticks");
        assert!(is_air(w.block(BlockPos::new(3, -59, 0))));
        assert!(ticks >= BREAK_TICKS, "{ticks}");
        for _ in 0..200 {
            w.follow_path(&mut breaker, goal);
            w.break_doors(id, &mut breaker, true);
            w.step(&mut breaker);
        }
        assert!(breaker.pos[0] > 5.0, "went through: {:?}", breaker.pos);
    }

    #[test]
    fn regional_difficulty_follows_the_wiki() {
        // Hard on day 0, new moon (phase 4): 2.25, clamped to 0.125.
        let r = regional_difficulty(Difficulty::Hard, 0, 0, 4);
        assert!((r - 2.25).abs() < 1e-9);
        assert!((clamped(r) - 0.125).abs() < 1e-9);
        // Easy never gets above zero clamped; Normal maxes out at 4.
        assert_eq!(
            clamped(regional_difficulty(
                Difficulty::Easy,
                100 * 24_000,
                4_000_000,
                0
            )),
            0.0
        );
        let normal = regional_difficulty(Difficulty::Normal, 100 * 24_000, 4_000_000, 0);
        assert!((normal - 4.0).abs() < 1e-9, "{normal}");
        assert_eq!(regional_difficulty(Difficulty::Peaceful, 1, 1, 1), 0.0);
    }

    #[test]
    fn can_break_doors_is_saved() {
        let mut w = flat_world();
        let mut zombie = Mob::new(MobKind::Zombie, [0.5, -60.0, 0.5], &mut w.rng);
        let mut nbt = Compound::new();
        write_nbt(&zombie, &mut nbt);
        assert!(nbt.get("CanBreakDoors").is_none(), "not decided yet");
        zombie.ai.break_doors = Some(true);
        write_nbt(&zombie, &mut nbt);
        let mut loaded = Mob::new(MobKind::Zombie, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut loaded, &nbt);
        assert_eq!(loaded.ai.break_doors, Some(true));
        let mut cow = Mob::new(MobKind::Cow, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut cow, &nbt);
        assert_eq!(cow.ai.break_doors, None);
    }
}
