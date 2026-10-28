//! Note blocks, bells and daylight detectors, following the Minecraft Wiki.
//!
//! A note block plays its note when used (which also raises it a semitone, from 0 to
//! 24), hit, or powered; its instrument comes from the block under it. Clients play the
//! sound themselves from a block event. Bells ring when used or powered. Daylight
//! detectors give power by how much sky light reaches them and how high the sun is, or
//! the opposite when inverted; they check once a second.

use levitas_registry::{block_entity_type, STATE_BLOCK};

use super::*;
use crate::blocks::{self, offset, prop, with, with_bool, Dir};

const NOTES: u8 = 25;
const DAYLIGHT_PERIOD: u64 = 20;
const BELL_RING: u8 = 1;

/// The instrument a note block plays over `below`.
pub fn instrument(below: u16) -> &'static str {
    let n = blocks::name(below);
    let tag = |t: &str| block_has_tag(below, t);
    match n {
        "minecraft:gold_block" => return "bell",
        "minecraft:clay" => return "flute",
        "minecraft:packed_ice" => return "chime",
        "minecraft:bone_block" => return "xylophone",
        "minecraft:iron_block" => return "iron_xylophone",
        "minecraft:soul_sand" => return "cow_bell",
        "minecraft:pumpkin" => return "didgeridoo",
        "minecraft:emerald_block" => return "bit",
        "minecraft:hay_block" => return "banjo",
        "minecraft:glowstone" => return "pling",
        "minecraft:sea_lantern" | "minecraft:beacon" => return "hat",
        _ => {}
    }
    if tag("minecraft:wool") {
        "guitar"
    } else if tag("minecraft:sand") || n == "minecraft:gravel" || n.ends_with("concrete_powder") {
        "snare"
    } else if n.contains("glass") {
        "hat"
    } else if tag("minecraft:mineable/axe") {
        "bass"
    } else if tag("minecraft:mineable/pickaxe") {
        "basedrum"
    } else {
        "harp"
    }
}

impl World {
    fn block_event(&mut self, pos: BlockPos, state: u16, action: u8, param: u8) {
        self.broadcast(
            Writer::packet(clientbound::BLOCK_EVENT)
                .position(pos)
                .u8(action)
                .u8(param)
                .varint(i32::from(STATE_BLOCK[usize::from(state)])),
            None,
        );
    }

    /// Plays a note block, if nothing sits on top to muffle it.
    pub(super) fn play_note(&mut self, pos: BlockPos) {
        let state = self.block(pos);
        if blocks::name(state) != "minecraft:note_block" {
            return;
        }
        if !is_air(self.block(offset(pos, Dir::Up))) {
            return;
        }
        self.block_event(pos, state, 0, 0);
    }

    /// Using a note block raises its note and plays it.
    pub(super) fn tune_note_block(&mut self, pos: BlockPos, state: u16) {
        let note: u8 = prop(state, "note")
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let next = (note + 1) % NOTES;
        self.set_block(pos, with(state, "note", &next.to_string()));
        self.play_note(pos);
    }

    /// A note block keeps its instrument up to date and plays when power arrives.
    pub(super) fn react_note_block(&mut self, pos: BlockPos, state: u16) {
        let mut new = with(
            state,
            "instrument",
            instrument(self.block(offset(pos, Dir::Down))),
        );
        let powered = self.power_at(pos) > 0;
        let was = prop(state, "powered") == Some("true");
        new = with_bool(new, "powered", powered);
        if new != state {
            self.set_block(pos, new);
        }
        if powered && !was {
            self.play_note(pos);
        }
    }

    /// Rings a bell, swinging it away from `from`.
    pub(super) fn ring_bell(&mut self, pos: BlockPos, from: Dir) {
        let state = self.block(pos);
        self.block_event(pos, state, BELL_RING, from.face() as u8);
        self.play_sound("minecraft:block.bell.use", pos, None);
    }

    pub(super) fn react_bell(&mut self, pos: BlockPos, state: u16) {
        let powered = self.power_at(pos) > 0;
        if (prop(state, "powered") == Some("true")) == powered {
            return;
        }
        self.set_block(pos, with_bool(state, "powered", powered));
        if powered {
            let facing = blocks::facing(state).unwrap_or(Dir::North);
            self.ring_bell(pos, facing);
        }
    }

    /// Using a daylight detector turns it into a night detector and back.
    pub(super) fn invert_daylight_detector(&mut self, pos: BlockPos, state: u16) {
        let inverted = prop(state, "inverted") != Some("true");
        self.set_block(pos, with_bool(state, "inverted", inverted));
        self.update_daylight_detector(pos);
    }

    /// The sun's angle in radians: 0 at noon, π at midnight.
    fn sun_angle(&self) -> f64 {
        let d = ((self.day_time.rem_euclid(24_000) as f64) / 24_000.0 - 0.25).rem_euclid(1.0);
        let e = 0.5 - (d * std::f64::consts::PI).cos() / 2.0;
        (d * 2.0 + e) / 3.0 * std::f64::consts::TAU
    }

    fn update_daylight_detector(&mut self, pos: BlockPos) {
        let Some(state) = self.loaded_block(pos) else {
            return;
        };
        if blocks::name(state) != "minecraft:daylight_detector" {
            return;
        }
        let sky = self.light_at(pos).map_or(0, |(sky, _)| i32::from(sky));
        let mut power = sky - self.sky_darken();
        if prop(state, "inverted") == Some("true") {
            power = 15 - power;
        } else if power > 0 {
            // The sun low in the sky gives less.
            let mut angle = self.sun_angle();
            let towards = if angle < std::f64::consts::PI {
                0.0
            } else {
                std::f64::consts::TAU
            };
            angle += (towards - angle) * 0.2;
            power = (f64::from(power) * angle.cos()).round() as i32;
        }
        let power = power.clamp(0, 15);
        let new = with(state, "power", &power.to_string());
        if new != state {
            self.set_block(pos, new);
        }
    }

    /// Daylight detectors look at the sky once a second.
    pub(super) fn tick_daylight_detectors(&mut self) {
        if !self.tick.is_multiple_of(DAYLIGHT_PERIOD) {
            return;
        }
        let detectors: Vec<BlockPos> = self
            .block_entities
            .iter()
            .filter(|(_, e)| e.type_id() == block_entity_type::DAYLIGHT_DETECTOR)
            .map(|(&pos, _)| pos)
            .collect();
        for pos in detectors {
            self.update_daylight_detector(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::containers::BlockEntity;

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    #[test]
    fn instruments_follow_the_block_below() {
        assert_eq!(instrument(state("minecraft:oak_planks")), "bass");
        assert_eq!(instrument(state("minecraft:stone")), "basedrum");
        assert_eq!(instrument(state("minecraft:sand")), "snare");
        assert_eq!(instrument(state("minecraft:glass")), "hat");
        assert_eq!(instrument(state("minecraft:white_wool")), "guitar");
        assert_eq!(instrument(state("minecraft:gold_block")), "bell");
        assert_eq!(instrument(state("minecraft:dirt")), "harp");
    }

    #[test]
    fn daylight_detectors_give_power_at_noon_and_none_at_night() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-daylight-{}-{}",
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
        let pos = BlockPos::new(0, -60, 0);
        let detector = state("minecraft:daylight_detector");
        w.set_block(pos, detector);
        w.block_entities
            .insert(pos, BlockEntity::for_block(detector).unwrap());
        w.day_time = 6_000;
        w.update_daylight_detector(pos);
        assert_eq!(prop(w.block(pos), "power"), Some("15"));
        w.day_time = 18_000;
        w.update_daylight_detector(pos);
        assert_eq!(prop(w.block(pos), "power"), Some("0"));
        let current = w.block(pos);
        w.invert_daylight_detector(pos, current);
        assert!(prop(w.block(pos), "power").unwrap().parse::<u8>().unwrap() > 10);
    }

    #[test]
    fn note_blocks_take_their_instrument_and_power() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-notes-{}-{}",
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
        let pos = BlockPos::new(1, -59, 1);
        w.set_block(BlockPos::new(1, -60, 1), block::STONE);
        w.set_block(pos, state("minecraft:note_block"));
        let note = w.block(pos);
        w.tune_note_block(pos, note);
        assert_eq!(prop(w.block(pos), "note"), Some("1"));
        w.set_block(BlockPos::new(2, -59, 1), state("minecraft:redstone_block"));
        w.tick_redstone();
        let s = w.block(pos);
        assert_eq!(prop(s, "instrument"), Some("basedrum"));
        assert_eq!(prop(s, "powered"), Some("true"));
    }
}
