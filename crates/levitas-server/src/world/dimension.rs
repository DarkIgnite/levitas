//! Dimensions. Each runs as its own world task with its own chunks, entities and
//! storage; players move between them through portals. They share the world folder's
//! `level.dat` (written by the overworld) and `playerdata/`.

use std::path::{Path, PathBuf};

use levitas_registry::SyncedRegistry;
use levitas_world::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dimension {
    Overworld,
    Nether,
}

impl Dimension {
    pub const ALL: [Dimension; 2] = [Dimension::Overworld, Dimension::Nether];

    /// The dimension's name, which is also its dimension type's.
    pub fn name(self) -> &'static str {
        match self {
            Dimension::Overworld => "minecraft:overworld",
            Dimension::Nether => "minecraft:the_nether",
        }
    }

    pub fn from_name(name: &str) -> Option<Dimension> {
        let full = if name.contains(':') {
            name.to_owned()
        } else {
            format!("minecraft:{name}")
        };
        Dimension::ALL.into_iter().find(|d| d.name() == full)
    }

    /// The dimension type's network ID.
    pub fn type_id(self) -> i32 {
        SyncedRegistry::get("minecraft:dimension_type")
            .and_then(|r| r.id_of(self.name()))
            .expect("vanilla dimension types")
    }

    /// The sections clients see and the disk keeps.
    pub fn span(self) -> Span {
        match self {
            Dimension::Overworld => Span::OVERWORLD,
            Dimension::Nether => Span::NETHER,
        }
    }

    /// Whether the dimension has a sky (and so sky light, days and weather).
    pub fn has_sky(self) -> bool {
        self == Dimension::Overworld
    }

    /// Whether water boils away and lava runs far and fast (the Nether).
    pub fn ultrawarm(self) -> bool {
        self == Dimension::Nether
    }

    /// Whether beds work; elsewhere they blow up.
    pub fn beds_work(self) -> bool {
        self == Dimension::Overworld
    }

    /// Where the dimension keeps its regions, inside the world folder.
    pub fn folder(self, world: &Path) -> PathBuf {
        match self {
            Dimension::Overworld => world.to_path_buf(),
            Dimension::Nether => world.join("DIM-1"),
        }
    }

    pub fn sea_level(self) -> i32 {
        match self {
            Dimension::Overworld => 63,
            Dimension::Nether => 32,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::connection::Profile;
    use crate::world::*;

    #[test]
    fn beds_blow_up_and_lava_runs_far_in_the_nether() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-nether-rules-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut nether, _) = World::new_in(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
            Dimension::Nether,
        );
        use crate::blocks::fluid::Fluid;
        assert_eq!(nether.drop_off(Fluid::Lava), 1);

        nether.chunk(0, 0);
        // A bed on a little stone floor high up in the Nether's open air.
        let foot = BlockPos::new(5, 100, 5);
        for x in 3..8 {
            for z in 3..8 {
                nether.set_block(BlockPos::new(x, 99, z), block::STONE);
                for y in 100..104 {
                    nether.set_block(BlockPos::new(x, y, z), block::AIR);
                }
            }
        }
        let bed = BlockInfo::by_name("minecraft:red_bed")
            .unwrap()
            .default_state;
        let foot_state =
            crate::blocks::with(crate::blocks::with(bed, "part", "foot"), "facing", "south");
        nether.set_block(foot, foot_state);
        nether.set_block(
            BlockPos::new(5, 100, 6),
            crate::blocks::with(foot_state, "part", "head"),
        );
        let (out, _frames) = mpsc::channel(1000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
        };
        nether
            .players
            .insert(id, Player::new(profile, out, [5.5, 100.0, 3.5]));
        assert!(nether.use_bed(id, foot, foot_state));
        assert!(is_air(nether.block(foot)), "the bed is gone");
        assert!(nether.players[&id].health < 20.0, "and it hurt");
    }
}
