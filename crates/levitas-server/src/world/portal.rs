//! Nether portals in the world: lighting them, travelling through them, and finding or
//! building the portal on the other side, following the Minecraft Wiki.
//!
//! A player standing in a portal for 4 seconds (at once in creative) goes to the other
//! dimension, at coordinates divided by 8 going into the Nether and multiplied by 8
//! coming back. There they come out of the nearest known portal within 16 blocks
//! (Nether) or 128 blocks (overworld) of that point, or a new one built close by.
//! Each dimension keeps a list of its portals in `levitas_portals.dat`.

use std::fs;

use levitas_nbt::{Compound, Tag};

use super::dimension::Dimension;
use super::*;
use crate::blocks::portal::{find_opening, Opening};
use crate::blocks::{self, Dir};

/// Ticks a survival player stands in a portal before it takes them.
const PORTAL_DELAY: u32 = 80;
/// Ticks after arriving before a portal can take a player again.
const ARRIVAL_COOLDOWN: u32 = 20;
const LEVEL_EVENT_TRAVEL: i32 = 1032;
const PORTALS_FILE: &str = "levitas_portals.dat";

/// A known portal: the lowest corner of its opening and whether it runs along x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownPortal {
    corner: BlockPos,
    along_x: bool,
}

impl KnownPortal {
    /// Where a player comes out: the middle of the opening's first two blocks.
    fn exit(self) -> [f64; 3] {
        let (dx, dz) = if self.along_x { (1.0, 0.5) } else { (0.5, 1.0) };
        [
            f64::from(self.corner.x) + dx,
            f64::from(self.corner.y),
            f64::from(self.corner.z) + dz,
        ]
    }
}

impl World {
    /// Fire in a frame opening becomes a portal. Returns whether it did.
    pub(super) fn try_light_portal(&mut self, pos: BlockPos) -> bool {
        let Some(opening) = find_opening(pos, &mut |q| self.block(q)) else {
            return false;
        };
        self.fill_portal(&opening);
        true
    }

    fn fill_portal(&mut self, opening: &Opening) {
        let portal = opening.portal_state();
        let blocks: Vec<BlockPos> = opening.blocks().collect();
        for &p in &blocks {
            self.set_block(p, portal);
        }
        self.remember_portal(KnownPortal {
            corner: opening.corner,
            along_x: opening.along == Dir::East,
        });
    }

    fn remember_portal(&mut self, portal: KnownPortal) {
        if !self.portals.contains(&portal) {
            self.portals.push(portal);
        }
    }

    /// Players standing in portals count up, and go through when it is time.
    pub(super) fn tick_portals(&mut self) {
        let mut leaving = Vec::new();
        for (&id, p) in self.players.iter_mut().filter(|(_, p)| p.alive) {
            p.portal_cooldown = p.portal_cooldown.saturating_sub(1);
            let cells = [p.pos[1], p.pos[1] + 1.0].map(|y| {
                BlockPos::new(
                    p.pos[0].floor() as i32,
                    y.floor() as i32,
                    p.pos[2].floor() as i32,
                )
            });
            let inside = cells.iter().any(|&c| {
                self.chunks
                    .get(&c.chunk())
                    .filter(|_| (MIN_Y..=MAX_Y).contains(&c.y))
                    .is_some_and(|chunk| {
                        blocks::name(chunk.get((c.x & 15) as usize, c.y, (c.z & 15) as usize))
                            == "minecraft:nether_portal"
                    })
            });
            if !inside {
                p.portal_ticks = 0;
                continue;
            }
            p.portal_ticks += 1;
            let delay = if p.survival() { PORTAL_DELAY } else { 1 };
            if p.portal_ticks >= delay && p.portal_cooldown == 0 {
                p.portal_ticks = 0;
                leaving.push(id);
            }
        }
        for id in leaving {
            self.go_through_portal(id);
        }
    }

    fn go_through_portal(&mut self, id: i32) {
        let pos = self.players[&id].pos;
        let (to, scale) = match self.dimension {
            Dimension::Overworld => (Dimension::Nether, 1.0 / 8.0),
            Dimension::Nether => (Dimension::Overworld, 8.0),
            Dimension::End => return,
        };
        let target = [pos[0] * scale, pos[1], pos[2] * scale];
        self.hand_over_through_portal(id, to, target);
    }

    /// Where a player coming through a portal near `near` comes out: a known portal
    /// close enough, or a new one.
    pub(super) fn portal_exit(&mut self, near: [f64; 3]) -> [f64; 3] {
        let radius = match self.dimension {
            Dimension::Nether => 16,
            _ => 128,
        };
        let (nx, nz) = (near[0].floor() as i32, near[2].floor() as i32);
        let mut candidates: Vec<KnownPortal> = self
            .portals
            .iter()
            .copied()
            .filter(|p| (p.corner.x - nx).abs() <= radius && (p.corner.z - nz).abs() <= radius)
            .collect();
        candidates.sort_by_key(|p| {
            let (dx, dz) = (i64::from(p.corner.x - nx), i64::from(p.corner.z - nz));
            dx * dx + dz * dz
        });
        for portal in candidates {
            // Portals broken since are forgotten.
            if blocks::name(self.block(portal.corner)) == "minecraft:nether_portal" {
                return portal.exit();
            }
            self.portals.retain(|p| *p != portal);
        }
        self.build_portal(near).exit()
    }

    /// The highest and lowest a new portal's frame may stand here.
    fn portal_heights(&self, near_y: i32) -> (i32, i32) {
        match self.dimension {
            Dimension::Nether => (32, 120),
            Dimension::Overworld | Dimension::End => {
                ((near_y - 40).max(MIN_Y + 1), (near_y + 40).min(MAX_Y - 5))
            }
        }
    }

    /// Builds a portal along x close to `near`: on open ground with room for its 4 x 5
    /// frame if there is any within 16 blocks, otherwise on an obsidian ledge carved
    /// out at `near`.
    fn build_portal(&mut self, near: [f64; 3]) -> KnownPortal {
        let (nx, ny, nz) = (
            near[0].floor() as i32,
            near[1].floor() as i32,
            near[2].floor() as i32,
        );
        let (low, high) = self.portal_heights(ny);
        let replaceable = |s: u16| is_air(s) || block_has_tag(s, "minecraft:replaceable");
        let mut spot = None;
        'search: for r in 0..=16i32 {
            for dx in -r..=r {
                for dz in -r..=r {
                    if dx.abs() != r && dz.abs() != r {
                        continue;
                    }
                    let (x, z) = (nx + dx, nz + dz);
                    for y in (low..=high).rev() {
                        let fits = (0..4).all(|i| {
                            !replaceable(self.block(BlockPos::new(x + i, y - 1, z)))
                                && (0..5).all(|j| {
                                    replaceable(self.block(BlockPos::new(x + i, y + j, z)))
                                })
                        }) && (1..3).all(|i| {
                            (1..3).all(|j| {
                                replaceable(self.block(BlockPos::new(x + i, y + j, z + 1)))
                            })
                        });
                        if fits {
                            spot = Some(BlockPos::new(x, y, z));
                            break 'search;
                        }
                    }
                }
            }
        }
        let obsidian = BlockInfo::by_name("minecraft:obsidian")
            .expect("obsidian exists")
            .default_state;
        let base = spot.unwrap_or_else(|| {
            // No room: carve some out, with a ledge to stand on either side.
            let y = ny.clamp(low, high);
            let base = BlockPos::new(nx, y, nz);
            for i in -1..5 {
                for dz in -1..=1 {
                    self.set_block(BlockPos::new(nx + i, y - 1, nz + dz), obsidian);
                    for j in 0..5 {
                        self.set_block(BlockPos::new(nx + i, y + j, nz + dz), block::AIR);
                    }
                }
            }
            base
        });
        // The frame, then the portal in it.
        for i in 0..4 {
            for j in 0..5 {
                let edge = i == 0 || i == 3 || j == 0 || j == 4;
                let at = BlockPos::new(base.x + i, base.y + j, base.z);
                if edge {
                    self.set_block(at, obsidian);
                }
            }
        }
        let opening = Opening {
            corner: BlockPos::new(base.x + 1, base.y + 1, base.z),
            along: Dir::East,
            width: 2,
            height: 3,
        };
        self.fill_portal(&opening);
        for p in opening.blocks() {
            self.update_neighbors(p);
        }
        KnownPortal {
            corner: opening.corner,
            along_x: true,
        }
    }

    pub(super) fn after_portal_arrival(&mut self, id: i32) {
        if let Some(p) = self.players.get_mut(&id) {
            p.portal_cooldown = ARRIVAL_COOLDOWN;
            p.portal_ticks = 0;
        }
        self.send(
            id,
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(LEVEL_EVENT_TRAVEL)
                .position(BlockPos::new(0, 0, 0))
                .i32(0)
                .bool(false),
        );
    }

    /// Reads this dimension's list of portals.
    pub(super) fn load_portals(folder: &std::path::Path) -> Vec<KnownPortal> {
        let Ok(bytes) = fs::read(folder.join(PORTALS_FILE)) else {
            return Vec::new();
        };
        let Ok((_, nbt)) = levitas_nbt::from_gzip(&bytes) else {
            return Vec::new();
        };
        nbt.list("Portals")
            .unwrap_or_default()
            .iter()
            .filter_map(|t| {
                let c = t.as_compound()?;
                Some(KnownPortal {
                    corner: BlockPos::new(
                        c.int("x")? as i32,
                        c.int("y")? as i32,
                        c.int("z")? as i32,
                    ),
                    along_x: c.int("AlongX").unwrap_or(1) != 0,
                })
            })
            .collect()
    }

    pub(super) fn save_portals(&self) {
        let list: Vec<Tag> = self
            .portals
            .iter()
            .map(|p| {
                Tag::Compound(
                    Compound::new()
                        .with("x", p.corner.x)
                        .with("y", p.corner.y)
                        .with("z", p.corner.z)
                        .with("AlongX", p.along_x),
                )
            })
            .collect();
        let folder = self.dimension.folder(&self.dir);
        let nbt = Compound::new().with("Portals", list);
        let result = fs::create_dir_all(&folder)
            .and_then(|()| fs::write(folder.join(PORTALS_FILE), levitas_nbt::to_gzip("", &nbt)));
        if let Err(e) = result {
            tracing::error!("could not save the portal list: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "levitas-portal-{}-{}",
            std::process::id(),
            next_entity_id()
        ))
    }

    fn flat() -> NewWorld {
        NewWorld {
            kind: WorldKind::Flat,
            seed: Some(0),
        }
    }

    /// A 4 x 5 obsidian frame along x, its bottom row at (x, y, z).
    fn build_frame(w: &mut World, x: i32, y: i32, z: i32) {
        let obsidian = BlockInfo::by_name("minecraft:obsidian")
            .unwrap()
            .default_state;
        for i in 0..4 {
            for j in 0..5 {
                if i == 0 || i == 3 || j == 0 || j == 4 {
                    w.set_block(BlockPos::new(x + i, y + j, z), obsidian);
                }
            }
        }
    }

    #[test]
    fn fire_in_a_frame_lights_it_and_breaking_the_frame_puts_it_out() {
        let (mut w, _) = World::new(temp_dir(), flat());
        w.chunk(0, 0);
        build_frame(&mut w, 2, -60, 2);
        let inside = BlockPos::new(3, -59, 2);
        let fire = crate::blocks::fire::fire_for(inside, &mut |q| w.block(q)).unwrap();
        w.light_fire(inside, fire);
        assert_eq!(blocks::name(w.block(inside)), "minecraft:nether_portal");
        assert_eq!(w.portals.len(), 1);

        w.set_block(BlockPos::new(2, -58, 2), block::AIR);
        w.update_neighbors(BlockPos::new(2, -58, 2));
        assert!(is_air(w.block(inside)), "the portal went out");
    }

    #[tokio::test]
    async fn standing_in_a_portal_takes_players_to_a_portal_in_the_nether() {
        let dir = temp_dir();
        let (mut overworld, _) = World::new(dir.clone(), flat());
        let (mut nether, _) = World::new_in(dir, flat(), Dimension::Nether);
        let (tx, mut rx) = mpsc::channel(16);
        overworld.links.insert(Dimension::Nether, tx);
        overworld.chunk(0, 0);
        build_frame(&mut overworld, 2, -60, 2);
        overworld.try_light_portal(BlockPos::new(3, -59, 2));

        let (out, _frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        overworld
            .players
            .insert(id, Player::new(profile, out, [4.0, -59.0, 2.5]));
        for _ in 0..PORTAL_DELAY {
            assert!(overworld.players.contains_key(&id), "not yet");
            overworld.tick_portals();
        }
        assert!(!overworld.players.contains_key(&id), "gone through");
        let Some(Event::Arrive(arrival)) = rx.recv().await else {
            panic!("no arrival");
        };
        nether.arrive(*arrival);
        let pos = nether.players[&id].pos;
        // From (4, 2.5) the Nether side is near (0.5, 0.3).
        assert!(pos[0].abs() < 20.0 && pos[2].abs() < 20.0, "{pos:?}");
        let feet = BlockPos::new(
            pos[0].floor() as i32,
            pos[1].floor() as i32,
            pos[2].floor() as i32,
        );
        assert_eq!(blocks::name(nether.block(feet)), "minecraft:nether_portal");
        assert_eq!(nether.portals.len(), 1);

        // Coming through again finds the same portal rather than building another.
        let again = nether.portal_exit([1.0, 70.0, 1.0]);
        assert_eq!(again, pos);
        assert_eq!(nether.portals.len(), 1);
    }

    #[test]
    fn exits_are_inside_the_opening() {
        let p = KnownPortal {
            corner: BlockPos::new(10, 64, -3),
            along_x: true,
        };
        assert_eq!(p.exit(), [11.0, 64.0, -2.5]);
    }
}
