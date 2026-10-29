//! Monster spawners, as in dungeons and Nether fortresses, following the Minecraft Wiki.
//!
//! With a player within 16 blocks, a spawner counts down 10 to 40 seconds and then tries
//! four times to put its mob within 4 blocks sideways, unless six of them are already
//! close. Generated chunks do not say which mob a spawner holds, so Levitas picks as
//! the structures do: blazes in the Nether (fortresses), and in the overworld zombies,
//! skeletons or spiders, as dungeons have them.

use levitas_nbt::Compound;
use levitas_registry::block;

use super::dimension::Dimension;
use super::mobs::{Mob, MobKind};
use super::*;

/// Players this close keep a spawner going.
const PLAYER_RANGE: f64 = 16.0;
const SPAWN_RANGE: i32 = 4;
const SPAWN_COUNT: u32 = 4;
const MAX_NEARBY: usize = 6;
const MIN_DELAY: u32 = 200;
const MAX_DELAY: u32 = 800;

pub struct Spawner {
    pub mob: MobKind,
    /// Ticks until it next spawns.
    pub delay: u32,
    pub viewers: HashSet<i32>,
}

impl Spawner {
    pub fn new(mob: MobKind) -> Self {
        Spawner {
            mob,
            delay: MIN_DELAY,
            viewers: HashSet::new(),
        }
    }

    pub fn to_nbt(&self, nbt: &mut Compound) {
        nbt.insert("Delay", self.delay.min(i16::MAX as u32) as i16);
        nbt.insert("SpawnData", self.spawn_data());
    }

    pub fn read_nbt(nbt: &Compound) -> Option<Self> {
        let mob = nbt
            .compound("SpawnData")
            .and_then(|d| d.compound("entity"))
            .and_then(|e| e.str("id"))
            .and_then(MobKind::from_name)?;
        let mut spawner = Spawner::new(mob);
        spawner.delay = nbt.int("Delay").unwrap_or(0).max(0) as u32;
        Some(spawner)
    }

    fn spawn_data(&self) -> Compound {
        Compound::new().with("entity", Compound::new().with("id", self.mob.name()))
    }

    /// What clients need to show the little mob spinning inside.
    pub fn network_data(&self) -> Compound {
        Compound::new().with("SpawnData", self.spawn_data())
    }
}

/// The mob a freshly generated spawner holds.
pub fn generated_mob(dimension: Dimension, pos: BlockPos) -> MobKind {
    if dimension == Dimension::Nether {
        return MobKind::Blaze;
    }
    // Dungeons hold zombies half the time, else skeletons or spiders.
    let hash = (pos.x.wrapping_mul(73_856_093)
        ^ pos.y.wrapping_mul(19_349_663)
        ^ pos.z.wrapping_mul(83_492_791))
    .rem_euclid(4);
    match hash {
        0 | 1 => MobKind::Zombie,
        2 => MobKind::Skeleton,
        _ => MobKind::Spider,
    }
}

impl World {
    /// Gives the spawners in a freshly generated chunk their block entities.
    pub(super) fn find_spawners(&mut self, cx: i32, cz: i32) {
        let Some(chunk) = self.chunks.get(&(cx, cz)) else {
            return;
        };
        let mut found = Vec::new();
        for (s, section) in chunk.sections().iter().enumerate() {
            if !section.blocks().may_contain(block::SPAWNER) {
                continue;
            }
            for i in 0..4096 {
                if section.blocks().get(i) == block::SPAWNER {
                    let (x, y, z) = (i % 16, i / 256, (i / 16) % 16);
                    found.push(BlockPos::new(
                        cx * 16 + x as i32,
                        MIN_Y + (s * 16 + y) as i32,
                        cz * 16 + z as i32,
                    ));
                }
            }
        }
        for pos in found {
            let mob = generated_mob(self.dimension, pos);
            self.block_entities
                .entry(pos)
                .or_insert_with(|| BlockEntity::Spawner(Spawner::new(mob)));
        }
    }

    pub(super) fn tick_spawners(&mut self) {
        let ready: Vec<(BlockPos, MobKind)> = self
            .block_entities
            .iter_mut()
            .filter_map(|(&pos, e)| match e {
                BlockEntity::Spawner(s) => Some((pos, s)),
                _ => None,
            })
            .filter_map(|(pos, s)| {
                let centre = [pos.x, pos.y, pos.z].map(|v| f64::from(v) + 0.5);
                let near = self.players.values().any(|p| {
                    p.alive
                        && (0..3)
                            .map(|i| (p.pos[i] - centre[i]).powi(2))
                            .sum::<f64>()
                            .sqrt()
                            <= PLAYER_RANGE
                });
                if !near {
                    return None;
                }
                s.delay = s.delay.saturating_sub(1);
                (s.delay == 0).then_some((pos, s.mob))
            })
            .collect();
        for (pos, kind) in ready {
            self.spawn_from_spawner(pos, kind);
            let delay = MIN_DELAY + self.rng.below(MAX_DELAY - MIN_DELAY);
            if let Some(BlockEntity::Spawner(s)) = self.block_entities.get_mut(&pos) {
                s.delay = delay;
            }
        }
    }

    fn spawn_from_spawner(&mut self, pos: BlockPos, kind: MobKind) {
        let r = f64::from(SPAWN_RANGE * 2 + 1);
        let nearby = self
            .mobs
            .values()
            .filter(|m| {
                m.kind == kind
                    && (m.pos[0] - f64::from(pos.x)).abs() <= r
                    && (m.pos[1] - f64::from(pos.y)).abs() <= r
                    && (m.pos[2] - f64::from(pos.z)).abs() <= r
            })
            .count();
        if nearby >= MAX_NEARBY {
            return;
        }
        for _ in 0..SPAWN_COUNT {
            let offset =
                |rng: &mut Rng, range: i32| (rng.f64() - rng.f64()) * f64::from(range) + 0.5;
            let at = [
                f64::from(pos.x) + offset(&mut self.rng, SPAWN_RANGE),
                f64::from(pos.y + self.rng.below(3) as i32 - 1),
                f64::from(pos.z) + offset(&mut self.rng, SPAWN_RANGE),
            ];
            let feet = BlockPos::new(
                at[0].floor() as i32,
                at[1].floor() as i32,
                at[2].floor() as i32,
            );
            let ground = self.loaded_block(BlockPos::new(feet.x, feet.y - 1, feet.z));
            let stands = kind.flies()
                || ground.is_some_and(|g| crate::blocks::sturdy(g, crate::blocks::Dir::Up));
            if stands && self.mob_fits(kind, at) {
                let mob = Mob::new(kind, at, &mut self.rng);
                self.mobs.insert(next_entity_id(), mob);
                self.broadcast(
                    Writer::packet(clientbound::LEVEL_EVENT)
                        .i32(LEVEL_EVENT_SPAWNER_SPAWN)
                        .position(pos)
                        .i32(0)
                        .bool(false),
                    None,
                );
            }
        }
    }
}

/// The puff of flames a spawner makes when a mob comes out.
const LEVEL_EVENT_SPAWNER_SPAWN: i32 = 2004;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn spawners_found_in_new_chunks_spawn_when_players_are_near() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-spawners-{}-{}",
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
        let pos = BlockPos::new(8, -60, 8);
        w.set_block(pos, block::SPAWNER);
        w.find_spawners(0, 0);
        let Some(BlockEntity::Spawner(s)) = w.block_entities.get(&pos) else {
            panic!("no spawner found");
        };
        assert!(matches!(
            s.mob,
            MobKind::Zombie | MobKind::Skeleton | MobKind::Spider
        ));
        let kind = s.mob;

        for _ in 0..MIN_DELAY {
            w.tick_spawners();
        }
        assert!(w.mobs.is_empty(), "nobody near");

        let (out, _frames) = mpsc::channel(1000);
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
        };
        w.players.insert(
            next_entity_id(),
            Player::new(profile, out, [8.5, -60.0, 14.5]),
        );
        // Each try lands somewhere random, so give it a few rounds.
        for _ in 0..MAX_DELAY * 4 {
            w.tick_spawners();
        }
        assert!(w.mobs.values().any(|m| m.kind == kind));
        assert_eq!(generated_mob(Dimension::Nether, pos), MobKind::Blaze);
    }
}
