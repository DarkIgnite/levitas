//! Water and lava as block states: sources, flowing levels and falling columns, and
//! which blocks fluids may flow into.
//!
//! Behaviour follows the Minecraft Wiki's descriptions of water and lava: a source has
//! level 0, flowing fluid levels 1 to 7 (weaker as the number grows), and fluid falling
//! down a column level 8. Water spreads 7 blocks from a source and looks 4 blocks ahead
//! for a way down; lava in the Overworld spreads 3 blocks and looks 2 ahead.

use levitas_registry::{block, collision_boxes, BlockInfo};

use super::{name, prop};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fluid {
    Water,
    Lava,
}

impl Fluid {
    /// Game ticks between two flow steps.
    pub fn delay(self) -> u64 {
        match self {
            Fluid::Water => 5,
            Fluid::Lava => 30,
        }
    }

    /// How much weaker fluid gets per block sideways.
    pub fn drop_off(self) -> u8 {
        match self {
            Fluid::Water => 1,
            Fluid::Lava => 2,
        }
    }

    /// How far sideways fluid looks for a way down.
    pub fn slope_distance(self) -> i32 {
        match self {
            Fluid::Water => 4,
            Fluid::Lava => 2,
        }
    }

    pub fn block(self) -> u16 {
        match self {
            Fluid::Water => block::WATER,
            Fluid::Lava => block::LAVA,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Fluid::Water => "minecraft:water",
            Fluid::Lava => "minecraft:lava",
        }
    }

    pub fn from_name(name: &str) -> Option<Fluid> {
        match name {
            "minecraft:water" | "minecraft:flowing_water" => Some(Fluid::Water),
            "minecraft:lava" | "minecraft:flowing_lava" => Some(Fluid::Lava),
            _ => None,
        }
    }
}

/// The fluid in a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidState {
    pub fluid: Fluid,
    pub source: bool,
    /// 8 for sources and falling fluid, 1 to 7 for fluid flowing sideways.
    pub amount: u8,
    pub falling: bool,
}

impl FluidState {
    pub fn source(fluid: Fluid) -> Self {
        FluidState {
            fluid,
            source: true,
            amount: 8,
            falling: false,
        }
    }

    pub fn flowing(fluid: Fluid, amount: u8) -> Self {
        FluidState {
            fluid,
            source: false,
            amount,
            falling: false,
        }
    }

    pub fn falling(fluid: Fluid) -> Self {
        FluidState {
            fluid,
            source: false,
            amount: 8,
            falling: true,
        }
    }

    /// The water or lava block holding this fluid.
    pub fn block(self) -> u16 {
        let level = if self.source {
            0
        } else if self.falling {
            8
        } else {
            8 - self.amount.clamp(1, 7)
        };
        let fluid = self.fluid.block();
        BlockInfo::of(fluid)
            .with(fluid, "level", &level.to_string())
            .unwrap_or(fluid)
    }
}

/// The fluid in a block, if any: water and lava blocks, waterlogged blocks, and the
/// underwater plants that are always waterlogged.
pub fn fluid_at(state: u16) -> Option<FluidState> {
    let block = name(state);
    if let Some(fluid) = Fluid::from_name(block) {
        let level: u8 = prop(state, "level")
            .and_then(|l| l.parse().ok())
            .unwrap_or(0);
        return Some(match level {
            0 => FluidState::source(fluid),
            1..=7 => FluidState::flowing(fluid, 8 - level),
            _ => FluidState::falling(fluid),
        });
    }
    let always_wet = matches!(
        block,
        "minecraft:bubble_column"
            | "minecraft:kelp"
            | "minecraft:kelp_plant"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
    );
    (always_wet || prop(state, "waterlogged") == Some("true"))
        .then(|| FluidState::source(Fluid::Water))
}

/// Whether flowing fluid may wash this block away and take its place: blocks without
/// collision, except those that keep fluid out (doors, signs, ladders, sugar cane,
/// portals) and waterloggable blocks, which only a source can fill.
pub fn can_hold_fluid(state: u16) -> bool {
    if fluid_at(state).is_some() {
        return false;
    }
    let block = name(state);
    let keeps_out = block.ends_with("_door")
        || block.ends_with("_sign")
        || matches!(
            block,
            "minecraft:ladder"
                | "minecraft:sugar_cane"
                | "minecraft:nether_portal"
                | "minecraft:end_portal"
                | "minecraft:end_gateway"
                | "minecraft:structure_void"
        );
    !keeps_out && prop(state, "waterlogged").is_none() && collision_boxes(state).is_empty()
}

/// Whether `state` can be waterlogged and is not yet.
pub fn can_waterlog(state: u16) -> bool {
    prop(state, "waterlogged") == Some("false")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_round_trip() {
        for fluid in [Fluid::Water, Fluid::Lava] {
            for state in [
                FluidState::source(fluid),
                FluidState::falling(fluid),
                FluidState::flowing(fluid, 1),
                FluidState::flowing(fluid, 7),
            ] {
                assert_eq!(fluid_at(state.block()), Some(state));
            }
        }
        assert_eq!(
            fluid_at(block::WATER),
            Some(FluidState::source(Fluid::Water))
        );
        assert_eq!(fluid_at(block::STONE), None);
    }

    #[test]
    fn waterlogged_blocks_hold_a_source() {
        let slab = BlockInfo::by_name("minecraft:oak_slab").unwrap();
        let wet = slab.state_with([("waterlogged", "true")]);
        assert_eq!(fluid_at(wet), Some(FluidState::source(Fluid::Water)));
        assert!(can_waterlog(slab.default_state));
        assert!(!can_hold_fluid(slab.default_state));
    }

    #[test]
    fn fluids_wash_away_plants_but_not_walls() {
        assert!(can_hold_fluid(block::AIR));
        assert!(can_hold_fluid(block::SHORT_GRASS));
        assert!(can_hold_fluid(block::TORCH));
        assert!(!can_hold_fluid(block::STONE));
        assert!(!can_hold_fluid(block::LADDER));
        assert!(!can_hold_fluid(block::WATER));
    }
}
