//! Redstone rules that depend only on block states: which blocks give off power and in
//! which directions, which blocks conduct it, and how redstone dust connects.
//!
//! Follows the Minecraft Wiki. Power comes as weak power, which only redstone
//! components pick up, and strong power, which also passes through a solid block to
//! dust next to it. Directions here point from the block giving power to the block
//! receiving it.

use levitas_protocol::BlockPos;
use levitas_registry::{block_has_tag, is_full_cube, BlockInfo};

use super::{name, offset, prop, with, Dir};

pub const MAX_POWER: u8 = 15;

pub fn is_wire(state: u16) -> bool {
    name(state) == "minecraft:redstone_wire"
}

pub fn wire_power(state: u16) -> u8 {
    prop(state, "power")
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}

fn is_on(state: u16, property: &str) -> bool {
    prop(state, property) == Some("true")
}

fn is_torch(state: u16) -> bool {
    matches!(
        name(state),
        "minecraft:redstone_torch" | "minecraft:redstone_wall_torch"
    )
}

pub fn is_repeater(state: u16) -> bool {
    name(state) == "minecraft:repeater"
}

pub fn is_comparator(state: u16) -> bool {
    name(state) == "minecraft:comparator"
}

pub fn is_observer(state: u16) -> bool {
    name(state) == "minecraft:observer"
}

/// Repeaters and comparators: they take power at the back and give it at the front.
pub fn is_diode(state: u16) -> bool {
    is_repeater(state) || is_comparator(state)
}

fn is_pressure_plate(state: u16) -> bool {
    name(state).ends_with("_pressure_plate")
}

/// The side a lever, button or torch hangs on, pointing from it to its support.
pub fn attached_to(state: u16) -> Option<Dir> {
    let n = name(state);
    if n == "minecraft:redstone_torch" {
        return Some(Dir::Down);
    }
    if n == "minecraft:redstone_wall_torch" {
        return super::facing(state).map(Dir::opposite);
    }
    match prop(state, "face")? {
        "floor" => Some(Dir::Down),
        "ceiling" => Some(Dir::Up),
        _ => super::facing(state).map(Dir::opposite),
    }
}

/// Whether a block gives off redstone power of its own.
pub fn is_source(state: u16) -> bool {
    let n = name(state);
    n == "minecraft:redstone_block"
        || n == "minecraft:lever"
        || block_has_tag(state, "minecraft:buttons")
        || is_pressure_plate(state)
        || is_torch(state)
        || is_diode(state)
        || is_observer(state)
        || is_wire(state)
}

/// Whether a block carries strong power to the dust around it: solid, opaque, and not
/// itself a source.
pub fn is_conductor(state: u16) -> bool {
    is_full_cube(state) && BlockInfo::of(state).light_filter == 15 && !is_source(state)
}

/// The output side of a repeater or comparator: they face their input.
pub fn diode_output(state: u16) -> Option<Dir> {
    super::facing(state).map(Dir::opposite)
}

/// Weak power `state` gives the block in direction `to`.
pub fn weak(state: u16, to: Dir) -> u8 {
    let n = name(state);
    if n == "minecraft:redstone_block" {
        return MAX_POWER;
    }
    if n == "minecraft:lever" || block_has_tag(state, "minecraft:buttons") {
        return if is_on(state, "powered") {
            MAX_POWER
        } else {
            0
        };
    }
    if is_pressure_plate(state) {
        return plate_power(state);
    }
    if is_torch(state) {
        let lit = is_on(state, "lit");
        return if lit && Some(to) != attached_to(state) {
            MAX_POWER
        } else {
            0
        };
    }
    if is_repeater(state) {
        return if is_on(state, "powered") && Some(to) == diode_output(state) {
            MAX_POWER
        } else {
            0
        };
    }
    if is_observer(state) {
        // An observer faces what it watches and pulses out of its back.
        return if is_on(state, "powered") && super::facing(state).map(Dir::opposite) == Some(to) {
            MAX_POWER
        } else {
            0
        };
    }
    if is_wire(state) {
        return match to {
            Dir::Up => 0,
            Dir::Down => wire_power(state),
            side => {
                if prop(state, side.name()).is_some_and(|c| c != "none") {
                    wire_power(state)
                } else {
                    0
                }
            }
        };
    }
    0
}

/// Strong power `state` gives the block in direction `to`, which passes on to dust.
pub fn strong(state: u16, to: Dir) -> u8 {
    let n = name(state);
    if n == "minecraft:lever" || block_has_tag(state, "minecraft:buttons") {
        return if Some(to) == attached_to(state) {
            weak(state, to)
        } else {
            0
        };
    }
    if is_pressure_plate(state) {
        return if to == Dir::Down {
            plate_power(state)
        } else {
            0
        };
    }
    if is_torch(state) {
        return if to == Dir::Up { weak(state, to) } else { 0 };
    }
    if is_repeater(state) || is_wire(state) || is_observer(state) {
        return weak(state, to);
    }
    0
}

fn plate_power(state: u16) -> u8 {
    match prop(state, "power") {
        Some(p) => p.parse().unwrap_or(0),
        None => {
            if is_on(state, "powered") {
                MAX_POWER
            } else {
                0
            }
        }
    }
}

/// Whether dust running towards `side` points at the block `state` there: other dust
/// and power sources, but repeaters only along their axis.
fn dust_points_at(state: u16, side: Dir) -> bool {
    if is_diode(state) {
        return super::facing(state).is_some_and(|f| f == side || f == side.opposite());
    }
    if is_observer(state) {
        return super::facing(state) == Some(side);
    }
    is_source(state)
        || matches!(
            name(state),
            "minecraft:redstone_lamp"
                | "minecraft:observer"
                | "minecraft:target"
                | "minecraft:comparator"
                | "minecraft:daylight_detector"
        )
}

/// The dust a piece of dust at `pos` links up with: beside it, and up or down a step.
pub fn wire_links(pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> Vec<BlockPos> {
    let above_open = !is_conductor(get(offset(pos, Dir::Up)));
    let mut links = Vec::new();
    for side in Dir::HORIZONTAL {
        let beside = offset(pos, side);
        let state = get(beside);
        if is_wire(state) {
            links.push(beside);
        } else if is_conductor(state) {
            let up = offset(beside, Dir::Up);
            if above_open && is_wire(get(up)) {
                links.push(up);
            }
        } else {
            let down = offset(beside, Dir::Down);
            if is_wire(get(down)) {
                links.push(down);
            }
        }
    }
    links
}

/// The dust's state with its side connections worked out: `up` where it climbs a
/// block, `side` where it runs into something, `none` elsewhere. Dust pointing only one
/// way also reaches out the other way, as a line.
pub fn wire_shape(state: u16, pos: BlockPos, get: &mut impl FnMut(BlockPos) -> u16) -> u16 {
    let above_open = !is_conductor(get(offset(pos, Dir::Up)));
    let mut sides = [("none", Dir::North); 4];
    for (i, side) in Dir::HORIZONTAL.into_iter().enumerate() {
        let beside = offset(pos, side);
        let next = get(beside);
        let connection = if is_conductor(next) {
            if above_open && is_wire(get(offset(beside, Dir::Up))) {
                "up"
            } else {
                "none"
            }
        } else if is_wire(next)
            || dust_points_at(next, side)
            || is_wire(get(offset(beside, Dir::Down)))
        {
            "side"
        } else {
            "none"
        };
        sides[i] = (connection, side);
    }
    let connected: Vec<Dir> = sides
        .iter()
        .filter(|(c, _)| *c != "none")
        .map(|&(_, d)| d)
        .collect();
    if let [only] = connected[..] {
        if let Some(entry) = sides.iter_mut().find(|(_, d)| *d == only.opposite()) {
            entry.0 = "side";
        }
    }
    sides.iter().fold(state, |s, &(connection, side)| {
        with(s, side.name(), connection)
    })
}

/// Whether redstone logic cares about this block: it gives, carries or uses power.
pub fn is_component(state: u16) -> bool {
    is_source(state) || reacts(state)
}

/// Whether a block changes when powered.
pub fn reacts(state: u16) -> bool {
    let n = name(state);
    super::is_door(state)
        || block_has_tag(state, "minecraft:trapdoors")
        || block_has_tag(state, "minecraft:fence_gates")
        || n == "minecraft:redstone_lamp"
        || n == "minecraft:tnt"
        || is_torch(state)
        || is_diode(state)
}

/// The repeater's delay in game ticks (one to four redstone ticks).
pub fn repeater_delay(state: u16) -> u64 {
    prop(state, "delay")
        .and_then(|d| d.parse::<u64>().ok())
        .unwrap_or(1)
        * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    #[test]
    fn sources_and_conductors() {
        assert!(is_conductor(state("minecraft:stone")));
        assert!(!is_conductor(state("minecraft:glass")));
        assert!(!is_conductor(state("minecraft:redstone_block")));
        assert_eq!(weak(state("minecraft:redstone_block"), Dir::North), 15);
        assert_eq!(strong(state("minecraft:redstone_block"), Dir::North), 0);
        // A standing torch powers everything but its support, and strongly only up.
        let torch = state("minecraft:redstone_torch");
        assert_eq!(weak(torch, Dir::North), 15);
        assert_eq!(weak(torch, Dir::Down), 0);
        assert_eq!(strong(torch, Dir::Up), 15);
        assert_eq!(strong(torch, Dir::North), 0);
    }

    #[test]
    fn repeaters_send_power_away_from_their_input() {
        let repeater = with(state("minecraft:repeater"), "facing", "north");
        let on = with(repeater, "powered", "true");
        assert_eq!(weak(on, Dir::South), 15);
        assert_eq!(weak(on, Dir::North), 0);
        assert_eq!(strong(on, Dir::South), 15);
    }
}
