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
