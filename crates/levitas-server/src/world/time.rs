//! The day/night cycle, weather and sleeping.
//!
//! Behaviour follows the Minecraft Wiki: a day is 24000 ticks, players may sleep from
//! tick 12542 to 23459 of the day or during a thunderstorm, and once every player is
//! asleep for 100 ticks the night is skipped and the weather clears. Rain and thunder
//! come and go on random timers; their strength fades in and out over 100 ticks.

use super::*;
use crate::blocks::{self, prop, with_bool};

pub const DAY_TICKS: i64 = 24_000;
/// Time of day sleeping becomes possible, and stops being possible.
const SLEEP_FROM: i64 = 12_542;
const SLEEP_UNTIL: i64 = 23_459;
/// Ticks every player must have slept before the night is skipped.
const SLEEP_TICKS: u32 = 100;
/// How often the time is sent to players (the client advances it in between).
const TIME_SYNC_TICKS: u64 = 20;

const GAME_EVENT_NO_RESPAWN_BLOCK: u8 = 0;
const GAME_EVENT_START_RAINING: u8 = 1;
const GAME_EVENT_STOP_RAINING: u8 = 2;
const GAME_EVENT_RAIN_LEVEL: u8 = 7;
const GAME_EVENT_THUNDER_LEVEL: u8 = 8;

/// Entity data: the pose (index 6, serializer 21) and the bed being slept in (index 14,
/// an optional block position, serializer 11).
const DATA_POSE: u8 = 6;
const SERIALIZER_POSE: i32 = 20;
const DATA_SLEEPING_POS: u8 = 14;
const SERIALIZER_OPTIONAL_POS: i32 = 11;
const POSE_STANDING: i32 = 0;
const POSE_SLEEPING: i32 = 2;
/// Animation shown when a player gets out of bed.
const ANIMATION_WAKE_UP: u8 = 2;
/// Height of a bed's top surface.
const BED_HEIGHT: f64 = 0.5625;

/// Rain and thunder, as level.dat stores them, plus how strong they are right now.
#[derive(Debug, Clone, Default)]
pub struct Weather {
    pub raining: bool,
    pub thundering: bool,
    /// Ticks until `raining` flips; 0 picks a new duration.
    pub rain_time: i32,
    pub thunder_time: i32,
    /// Ticks of guaranteed clear weather left (set by `/weather clear`).
    pub clear_time: i32,
    pub rain_level: f32,
    pub thunder_level: f32,
}

impl Weather {
    /// Whether it rains noticeably (what the client shows as rain).
    pub fn is_raining(&self) -> bool {
        self.rain_level > 0.2
    }

    pub fn is_thundering(&self) -> bool {
        self.rain_level * self.thunder_level > 0.9
    }
}

fn uniform(rng: &mut Rng, from: i32, to: i32) -> i32 {
    from + rng.below((to - from + 1) as u32) as i32
}

impl World {
    pub(super) fn time_packet(&self) -> Writer {
        // The world's age, then its clocks: the time each shows and how fast the
        // client should run it on between updates.
        let clocks = SyncedRegistry::get("minecraft:world_clock").map_or(&[][..], |r| r.entries);
        let rate = if crate::rules::bool("doDaylightCycle") {
            1.0
        } else {
            0.0
        };
        let mut packet = Writer::packet(clientbound::SET_TIME)
            .i64(self.tick as i64)
            .varint(clocks.len() as i32);
        for id in 0..clocks.len() {
            packet = packet
                .varint(id as i32)
                .varlong(self.day_time)
                .f32(0.0)
                .f32(rate);
        }
        packet
    }

    /// Time and weather for a player who just joined or respawned.
    pub(super) fn send_time_and_weather(&mut self, id: i32) {
        self.send(id, self.time_packet());
        if self.dimension.has_sky() && self.weather.is_raining() {
            self.send(id, game_event(GAME_EVENT_START_RAINING, 0.0));
            self.send(
                id,
                game_event(GAME_EVENT_RAIN_LEVEL, self.weather.rain_level),
            );
            self.send(
                id,
                game_event(GAME_EVENT_THUNDER_LEVEL, self.weather.thunder_level),
            );
        }
    }

    pub(super) fn tick_time(&mut self) {
        if crate::rules::bool("doDaylightCycle") {
            self.day_time += 1;
        }
        if crate::rules::bool("doWeatherCycle") {
            self.tick_weather();
        }
        self.tick_sleep();
        if self.tick.is_multiple_of(TIME_SYNC_TICKS) {
            self.broadcast(self.time_packet(), None);
        }
    }

    fn tick_weather(&mut self) {
        let w = &mut self.weather;
        let rng = &mut self.rng;
        if w.clear_time > 0 {
            w.clear_time -= 1;
            w.thunder_time = if w.thundering { 0 } else { 1 };
            w.rain_time = if w.raining { 0 } else { 1 };
            w.thundering = false;
            w.raining = false;
        } else {
            if w.thunder_time > 0 {
                w.thunder_time -= 1;
                if w.thunder_time == 0 {
                    w.thundering = !w.thundering;
                }
            } else if w.thundering {
                w.thunder_time = uniform(rng, 3_600, 15_600);
            } else {
                w.thunder_time = uniform(rng, 12_000, 180_000);
            }
            if w.rain_time > 0 {
                w.rain_time -= 1;
                if w.rain_time == 0 {
                    w.raining = !w.raining;
                }
            } else if w.raining {
                w.rain_time = uniform(rng, 12_000, 24_000);
            } else {
                w.rain_time = uniform(rng, 12_000, 180_000);
            }
        }

        let was_raining = w.is_raining();
        let (old_rain, old_thunder) = (w.rain_level, w.thunder_level);
        let step = |level: f32, on: bool| {
            let target = if on { 1.0 } else { 0.0 };
            (level + (target - level).clamp(-0.01, 0.01)).clamp(0.0, 1.0)
        };
        w.thunder_level = step(w.thunder_level, w.thundering);
        w.rain_level = step(w.rain_level, w.raining);
        let (rain, thunder) = (w.rain_level, w.thunder_level);
        let now_raining = w.is_raining();

        if was_raining != now_raining {
            let event = if now_raining {
                GAME_EVENT_START_RAINING
            } else {
                GAME_EVENT_STOP_RAINING
            };
            self.broadcast(game_event(event, 0.0), None);
        }
        if rain != old_rain || was_raining != now_raining {
            self.broadcast(game_event(GAME_EVENT_RAIN_LEVEL, rain), None);
        }
        if thunder != old_thunder || was_raining != now_raining {
            self.broadcast(game_event(GAME_EVENT_THUNDER_LEVEL, thunder), None);
        }
    }

    /// Sets the weather like `/weather`: clear, rain or thunder for `duration` ticks.
    pub(super) fn set_weather(&mut self, kind: &str, duration: Option<i32>) -> bool {
        let w = &mut self.weather;
        match kind {
            "clear" => {
                w.clear_time = duration.unwrap_or_else(|| uniform(&mut self.rng, 12_000, 180_000));
                w.rain_time = 0;
                w.thunder_time = 0;
                w.raining = false;
                w.thundering = false;
            }
            "rain" => {
                let time = duration.unwrap_or_else(|| uniform(&mut self.rng, 12_000, 24_000));
                w.clear_time = 0;
                w.rain_time = time;
                w.thunder_time = time;
                w.raining = true;
                w.thundering = false;
            }
            "thunder" => {
                let time = duration.unwrap_or_else(|| uniform(&mut self.rng, 3_600, 15_600));
                w.clear_time = 0;
                w.rain_time = time;
                w.thunder_time = time;
                w.raining = true;
                w.thundering = true;
            }
            _ => return false,
        }
        true
    }

    /// Whether players may sleep now: at night, or in a thunderstorm.
    fn can_sleep_now(&self) -> bool {
        let time = self.day_time.rem_euclid(DAY_TICKS);
        (SLEEP_FROM..=SLEEP_UNTIL).contains(&time) || self.weather.is_thundering()
    }

    /// A player uses a bed: it becomes their respawn point, and at night they lie down.
    /// Returns whether the click was handled.
    pub(super) fn use_bed(&mut self, id: i32, pos: BlockPos, state: u16) -> bool {
        if !blocks::is_bed(state) {
            return false;
        }
        // The player lies with their head on the head part.
        let head = if prop(state, "part") == Some("foot") {
            match blocks::other_part(state, pos) {
                Some(head) => head,
                None => return true,
            }
        } else {
            pos
        };
        let head_state = self.block(head);
        if !blocks::is_bed(head_state) || self.players.get(&id).is_none_or(|p| !p.alive) {
            return true;
        }
        // Beds anywhere but the overworld blow up, setting fires.
        if !self.dimension.beds_work() {
            let foot = blocks::other_part(head_state, head);
            self.set_block(head, block::AIR);
            if let Some(foot) = foot {
                self.set_block(foot, block::AIR);
            }
            let centre = [head.x, head.y, head.z].map(|v| f64::from(v) + 0.5);
            self.explode_with_fire(centre, 5.0, "[Intentional Game Design]");
            return true;
        }
        if self.players[&id].sleeping.is_some() {
            return true;
        }
        let p = self.players.get_mut(&id).expect("player exists");
        let new_spawn = p.respawn != Some(head);
        p.respawn = Some(head);
        if new_spawn {
            self.send(id, action_bar("Respawn point set"));
        }
        if !self.can_sleep_now() {
            self.send(
                id,
                action_bar("You can sleep only at night or during thunderstorms"),
            );
            return true;
        }
        if self.monsters_near(head) {
            self.send(
                id,
                action_bar("You may not rest now; there are monsters nearby"),
            );
            return true;
        }
        if prop(head_state, "occupied") == Some("true") {
            self.send(id, action_bar("This bed is occupied"));
            return true;
        }
        self.set_bed_occupied(head, true);
        let p = self.players.get_mut(&id).expect("player exists");
        p.sleeping = Some(head);
        p.sleep_ticks = 0;
        p.pos = [
            f64::from(head.x) + 0.5,
            f64::from(head.y) + BED_HEIGHT,
            f64::from(head.z) + 0.5,
        ];
        p.moved = true;
        self.broadcast(sleep_data(id, Some(head)), None);
        true
    }

    fn set_bed_occupied(&mut self, head: BlockPos, occupied: bool) {
        let state = self.block(head);
        if !blocks::is_bed(state) {
            return;
        }
        self.set_block(head, with_bool(state, "occupied", occupied));
        if let Some(foot) = blocks::other_part(state, head) {
            let foot_state = self.block(foot);
            if blocks::is_bed(foot_state) {
                self.set_block(foot, with_bool(foot_state, "occupied", occupied));
            }
        }
    }

    /// Gets a player out of bed, standing on top of it.
    pub(super) fn wake_up(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let Some(head) = p.sleeping.take() else {
            return;
        };
        p.sleep_ticks = 0;
        let stand = [
            f64::from(head.x) + 0.5,
            f64::from(head.y) + BED_HEIGHT,
            f64::from(head.z) + 0.5,
        ];
        p.pos = stand;
        p.moved = true;
        let (yaw, pitch) = (p.yaw, p.pitch);
        self.set_bed_occupied(head, false);
        self.broadcast(sleep_data(id, None), None);
        self.broadcast(
            Writer::packet(clientbound::ANIMATE)
                .varint(id)
                .u8(ANIMATION_WAKE_UP),
            None,
        );
        self.send(id, position_packet(stand, yaw, pitch));
    }

    /// Counts sleeping time, and skips the night once everyone has slept long enough.
    fn tick_sleep(&mut self) {
        let mut anyone = false;
        let (mut players, mut rested) = (0, 0);
        for p in self.players.values_mut().filter(|p| p.alive) {
            players += 1;
            if p.sleeping.is_some() {
                anyone = true;
                p.sleep_ticks = p.sleep_ticks.saturating_add(1);
                if p.sleep_ticks >= SLEEP_TICKS {
                    rested += 1;
                }
            }
        }
        // `playersSleepingPercentage` of the players must have slept, at least one.
        let percentage = crate::rules::int("playersSleepingPercentage").max(0);
        let needed = ((players * percentage + 99) / 100).max(1);
        let everyone_rested = percentage <= 100 && rested >= needed;
        if anyone && !self.can_sleep_now() {
            // Morning came on its own: everyone gets up.
            let sleepers: Vec<i32> = self
                .players
                .iter()
                .filter(|(_, p)| p.sleeping.is_some())
                .map(|(&id, _)| id)
                .collect();
            for id in sleepers {
                self.wake_up(id);
            }
            return;
        }
        if !anyone || !everyone_rested {
            return;
        }
        self.day_time += DAY_TICKS - self.day_time.rem_euclid(DAY_TICKS);
        let w = &mut self.weather;
        w.rain_time = 0;
        w.thunder_time = 0;
        w.raining = false;
        w.thundering = false;
        self.broadcast(self.time_packet(), None);
        let sleepers: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.sleeping.is_some())
            .map(|(&id, _)| id)
            .collect();
        for id in sleepers {
            self.wake_up(id);
        }
    }

    /// Where a player respawns: on their bed if it is still there, otherwise at the
    /// world spawn (and they are told their bed is missing).
    pub(super) fn respawn_point(&mut self, id: i32) -> [f64; 3] {
        let Some(bed) = self.players.get(&id).and_then(|p| p.respawn) else {
            return self.spawn;
        };
        if blocks::is_bed(self.block(bed)) {
            return [
                f64::from(bed.x) + 0.5,
                f64::from(bed.y) + BED_HEIGHT,
                f64::from(bed.z) + 0.5,
            ];
        }
        if let Some(p) = self.players.get_mut(&id) {
            p.respawn = None;
        }
        self.send(id, game_event(GAME_EVENT_NO_RESPAWN_BLOCK, 0.0));
        self.spawn
    }
}

fn game_event(event: u8, value: f32) -> Writer {
    Writer::packet(clientbound::GAME_EVENT).u8(event).f32(value)
}

fn action_bar(text: &str) -> Writer {
    Writer::packet(clientbound::SYSTEM_CHAT)
        .text(text)
        .bool(true)
}

/// Entity data putting a player into a bed at `bed`, or back on their feet.
fn sleep_data(id: i32, bed: Option<BlockPos>) -> Writer {
    let pose = if bed.is_some() {
        POSE_SLEEPING
    } else {
        POSE_STANDING
    };
    let packet = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_POSE)
        .varint(SERIALIZER_POSE)
        .varint(pose)
        .u8(DATA_SLEEPING_POS)
        .varint(SERIALIZER_OPTIONAL_POS);
    match bed {
        Some(pos) => packet.bool(true).position(pos),
        None => packet.bool(false),
    }
    .u8(0xFF)
}
