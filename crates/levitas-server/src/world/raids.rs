//! Village raids and Bad Omen, following the Minecraft Wiki.
//!
//! A player who drinks an ominous bottle gains Bad Omen. Entering a village (near a
//! villager) clears Bad Omen and begins Raid Omen (30 seconds), warning the village with
//! ominous activation sounds. When Raid Omen expires, the raid begins with a sounding horn,
//! bringing waves of pillagers, vindicators, witches, evokers, and ravagers.
//!
//! Clearing all waves grants Hero of the Village, giving substantial discounts on trades.
//! If all villagers perish, the raid is lost and the raiders celebrate.

use std::collections::HashSet;

use levitas_protocol::{angle, BlockPos, Writer};
use levitas_registry::packet::play::clientbound;
use levitas_registry::{entity_type, mob_effect, Effect};

use super::dimension::Dimension;
use super::mobs::{Mob, MobKind};
use super::monsters::horizontal_distance;
use super::survival::Damage;
use super::*;
use crate::world::next_entity_id;

pub const BOSS_ADD: i32 = 0;
pub const BOSS_REMOVE: i32 = 1;
pub const BOSS_HEALTH: i32 = 2;
pub const BOSS_TITLE: i32 = 3;
pub const BOSS_STYLE: i32 = 4;

pub const BOSS_RED: i32 = 2;
pub const BOSS_GREEN: i32 = 3;

pub const RAID_BAR_FLAGS: u8 = 0;
pub const RAID_BAR_DIVISION: i32 = 0;

pub const RAID_RANGE: f64 = 96.0;
pub const RAID_LEAVE_RANGE: f64 = 112.0;
pub const VILLAGE_SEARCH_RANGE: f64 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidState {
    /// Countdown before the wave begins.
    Warmup { ticks: u32 },
    /// Wave is active, raiders are alive and fighting.
    Active,
    /// Intermission between waves (5 seconds = 100 ticks).
    Intermission { ticks: u32 },
    /// Victory! Boss bar turns green, challenge complete sound, hero of the village given.
    Victory { ticks: u32 },
    /// Defeat! All villagers were killed. Boss bar red, raiders celebrate.
    Defeat { ticks: u32 },
    /// Ended, will be removed.
    Over,
}

pub struct Raid {
    #[allow(dead_code)]
    pub id: u32,
    pub center: [f64; 3],
    pub boss_bar: u128,
    pub wave: u32,
    pub total_waves: u32,
    pub bad_omen_level: u32,
    pub state: RaidState,
    pub raiders: HashSet<i32>,
    pub initial_wave_raiders: usize,
    pub participating_players: HashSet<i32>,
    pub spawned_wave: bool,
}

#[derive(Debug, Clone)]
pub struct EvokerFang {
    pub id: i32,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub warmup: u32,
    pub age: u32,
    pub caster: Option<i32>,
}

pub fn boss_bar_add(bar: u128, title: &str, health: f32, color: i32) -> Writer {
    Writer::packet(clientbound::BOSS_EVENT)
        .uuid(bar)
        .varint(BOSS_ADD)
        .text(title)
        .f32(health)
        .varint(color)
        .varint(RAID_BAR_DIVISION)
        .u8(RAID_BAR_FLAGS)
}

pub fn boss_bar_remove(bar: u128) -> Writer {
    Writer::packet(clientbound::BOSS_EVENT)
        .uuid(bar)
        .varint(BOSS_REMOVE)
}

pub fn boss_bar_health(bar: u128, health: f32) -> Writer {
    Writer::packet(clientbound::BOSS_EVENT)
        .uuid(bar)
        .varint(BOSS_HEALTH)
        .f32(health)
}

pub fn boss_bar_title(bar: u128, title: &str) -> Writer {
    Writer::packet(clientbound::BOSS_EVENT)
        .uuid(bar)
        .varint(BOSS_TITLE)
        .text(title)
}

pub fn boss_bar_style(bar: u128, color: i32) -> Writer {
    Writer::packet(clientbound::BOSS_EVENT)
        .uuid(bar)
        .varint(BOSS_STYLE)
        .varint(color)
        .varint(RAID_BAR_DIVISION)
}

fn spawn_fang(id: i32, pos: [f64; 3], yaw: f32) -> Writer {
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::EVOKER_FANGS)
        .f64(pos[0])
        .f64(pos[1])
        .f64(pos[2])
        .lp_vec3([0.0, 0.0, 0.0])
        .u8(0)
        .u8(angle(yaw))
        .u8(angle(yaw))
        .varint(0)
}

pub fn wave_composition(wave: u32, bad_omen: u32) -> Vec<MobKind> {
    let mut mobs = Vec::new();
    match wave {
        1 => {
            for _ in 0..4 {
                mobs.push(MobKind::Pillager);
            }
            mobs.push(MobKind::Vindicator);
        }
        2 => {
            for _ in 0..3 {
                mobs.push(MobKind::Pillager);
            }
            for _ in 0..2 {
                mobs.push(MobKind::Vindicator);
            }
            mobs.push(MobKind::Witch);
        }
        3 => {
            for _ in 0..4 {
                mobs.push(MobKind::Pillager);
            }
            for _ in 0..2 {
                mobs.push(MobKind::Vindicator);
            }
            mobs.push(MobKind::Ravager);
        }
        4 => {
            for _ in 0..4 {
                mobs.push(MobKind::Pillager);
            }
            for _ in 0..3 {
                mobs.push(MobKind::Vindicator);
            }
            mobs.push(MobKind::Witch);
            mobs.push(MobKind::Ravager);
        }
        5 => {
            for _ in 0..4 {
                mobs.push(MobKind::Pillager);
            }
            for _ in 0..4 {
                mobs.push(MobKind::Vindicator);
            }
            mobs.push(MobKind::Evoker);
            mobs.push(MobKind::Ravager);
        }
        6 => {
            for _ in 0..4 {
                mobs.push(MobKind::Pillager);
            }
            for _ in 0..5 {
                mobs.push(MobKind::Vindicator);
            }
            for _ in 0..2 {
                mobs.push(MobKind::Witch);
            }
            mobs.push(MobKind::Evoker);
        }
        _ => {
            for _ in 0..4 {
                mobs.push(MobKind::Pillager);
            }
            for _ in 0..5 {
                mobs.push(MobKind::Vindicator);
            }
            for _ in 0..2 {
                mobs.push(MobKind::Witch);
            }
            for _ in 0..2 {
                mobs.push(MobKind::Evoker);
            }
            mobs.push(MobKind::Ravager);
        }
    }
    if bad_omen > 0 {
        mobs.push(MobKind::Vindicator);
    }
    mobs
}

impl World {
    fn find_ground_y(&mut self, x: f64, default_y: f64, z: f64) -> f64 {
        let bx = x.floor() as i32;
        let bz = z.floor() as i32;
        let start_y = (default_y as i32 + 10).clamp(MIN_Y, MAX_Y);
        for y in (MIN_Y..=start_y).rev() {
            let pos = BlockPos::new(bx, y, bz);
            if !is_air(self.block(pos)) {
                return f64::from(y + 1);
            }
        }
        default_y
    }

    /// Starts a raid at `center` with the given Bad Omen level.
    pub(super) fn start_raid(&mut self, center: [f64; 3], bad_omen_level: u32) -> u32 {
        let raid_id = self.next_raid_id;
        self.next_raid_id += 1;
        let boss_bar = (u128::from(self.rng.next_u64()) << 64) | u128::from(self.rng.next_u64());
        let total_waves = match crate::config::get().difficulty.as_str() {
            "easy" => 3,
            "hard" => 7,
            _ => 5,
        };
        let mut raid = Raid {
            id: raid_id,
            center,
            boss_bar,
            wave: 1,
            total_waves,
            bad_omen_level,
            state: RaidState::Warmup { ticks: 40 },
            raiders: HashSet::new(),
            initial_wave_raiders: 0,
            participating_players: HashSet::new(),
            spawned_wave: false,
        };

        let in_range_players: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive && horizontal_distance(p.pos, center) <= RAID_RANGE)
            .map(|(&pid, _)| pid)
            .collect();

        for pid in in_range_players {
            self.send(pid, boss_bar_add(boss_bar, "Raid", 1.0, BOSS_RED));
            raid.participating_players.insert(pid);
        }

        self.play_sound_at("minecraft:event.raid.horn", center, 64.0, 1.0, None);
        self.raids.insert(raid_id, raid);
        raid_id
    }

    /// Spawns the current wave of raiders for `raid_id`.
    pub(super) fn spawn_raid_wave(&mut self, raid_id: u32) {
        let (center, wave, bad_omen) = {
            let Some(r) = self.raids.get(&raid_id) else {
                return;
            };
            (r.center, r.wave, r.bad_omen_level)
        };
        let kinds = wave_composition(wave, bad_omen);
        let count = kinds.len();
        let mut new_raiders = HashSet::new();
        let base_angle = self.rng.f64() * std::f64::consts::TAU;

        for (i, kind) in kinds.into_iter().enumerate() {
            let angle = base_angle + (i as f64) * std::f64::consts::TAU / (count as f64);
            let dist = 32.0 + self.rng.f64() * 12.0;
            let rx = center[0] + angle.cos() * dist;
            let rz = center[2] + angle.sin() * dist;
            let floor_y = center[1];
            let ry = self.find_ground_y(rx, floor_y, rz);
            let at = [rx, ry, rz];

            let mut mob = Mob::new(kind, at, &mut self.rng);
            mob.raid_id = Some(raid_id);
            if wave == 1 && i == 0 && kind == MobKind::Pillager {
                mob.is_patrol_leader = true;
            }
            let mob_id = next_entity_id();
            self.mobs.insert(mob_id, mob);
            new_raiders.insert(mob_id);
        }

        let notify = if let Some(r) = self.raids.get_mut(&raid_id) {
            r.raiders = new_raiders;
            r.initial_wave_raiders = r.raiders.len();
            r.spawned_wave = true;
            r.state = RaidState::Active;

            let title = format!("Raid - Raiders remaining: {}", r.raiders.len());
            let players: Vec<i32> = r.participating_players.iter().copied().collect();
            Some((r.boss_bar, title, players))
        } else {
            None
        };

        if let Some((bar, title, players)) = notify {
            for pid in players {
                self.send(pid, boss_bar_title(bar, &title));
                self.send(pid, boss_bar_health(bar, 1.0));
            }
        }

        self.play_sound_at("minecraft:event.raid.horn", center, 64.0, 1.0, None);
    }

    /// Handles a raider being killed in a raid.
    pub(super) fn raider_killed(&mut self, raid_id: u32, mob_id: i32) {
        let notify = if let Some(r) = self.raids.get_mut(&raid_id) {
            r.raiders.remove(&mob_id);
            let remaining = r.raiders.len();
            let initial = r.initial_wave_raiders.max(1);
            let health = (remaining as f32) / (initial as f32);
            let title = format!("Raid - Raiders remaining: {remaining}");
            let players: Vec<i32> = r.participating_players.iter().copied().collect();
            Some((r.boss_bar, title, health, players))
        } else {
            None
        };

        if let Some((bar, title, health, players)) = notify {
            for pid in players {
                self.send(pid, boss_bar_title(bar, &title));
                self.send(pid, boss_bar_health(bar, health));
            }
        }
    }

    /// Removes boss bars when a player disconnects or leaves the dimension.
    pub(super) fn leave_raids(&mut self, player_id: i32) {
        let mut to_remove = Vec::new();
        for r in self.raids.values_mut() {
            if r.participating_players.remove(&player_id) {
                to_remove.push(r.boss_bar);
            }
        }
        for bar in to_remove {
            self.send(player_id, boss_bar_remove(bar));
        }
    }

    /// Called when Raid Omen effect on a player expires.
    pub(super) fn raid_omen_expired(&mut self, player_id: i32) {
        if self.dimension != Dimension::Overworld {
            return;
        }
        let Some(p) = self.players.get(&player_id) else {
            return;
        };
        if !p.alive || !p.survival() {
            return;
        }
        let pos = p.pos;
        let amp = p
            .effect(mob_effect::RAID_OMEN)
            .map(|e| e.amplifier)
            .unwrap_or(0);
        let nearest_villager = self
            .mobs
            .values()
            .filter(|m| m.alive() && m.kind == MobKind::Villager)
            .map(|m| (m.pos, horizontal_distance(pos, m.pos)))
            .filter(|&(_, d)| d <= VILLAGE_SEARCH_RANGE)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((villager_pos, _)) = nearest_villager {
            self.start_raid(villager_pos, amp as u32);
        }
    }

    /// Ticks raids, converting Bad Omen to Raid Omen near villagers, and advancing active raids.
    pub(super) fn tick_raids(&mut self) {
        if self.dimension != Dimension::Overworld {
            return;
        }

        // 1. Bad Omen -> Raid Omen when near a villager
        let mut to_raid_omen = Vec::new();
        for (&pid, p) in self.players.iter().filter(|(_, p)| p.alive && p.survival()) {
            if let Some(bo) = p.effect(mob_effect::BAD_OMEN) {
                let near_villager = self.mobs.values().any(|m| {
                    m.alive()
                        && m.kind == MobKind::Villager
                        && horizontal_distance(p.pos, m.pos) <= VILLAGE_SEARCH_RANGE
                });
                if near_villager {
                    to_raid_omen.push((pid, bo.amplifier, p.pos));
                }
            }
        }
        for (pid, amp, pos) in to_raid_omen {
            self.remove_effect(pid, mob_effect::BAD_OMEN);
            self.add_effect(
                pid,
                Effect {
                    id: mob_effect::RAID_OMEN,
                    duration: 600, // 30 seconds
                    amplifier: amp,
                },
            );
            self.play_sound_at(
                "minecraft:block.trial_spawner.ominous_activate",
                pos,
                1.0,
                1.0,
                None,
            );
            self.play_sound_at("minecraft:event.mob_effect.raid_omen", pos, 1.0, 1.0, None);
        }

        // 2. Advance active raids
        let raid_ids: Vec<u32> = self.raids.keys().copied().collect();
        for raid_id in raid_ids {
            self.tick_single_raid(raid_id);
        }
        self.raids.retain(|_, r| r.state != RaidState::Over);
    }

    fn tick_single_raid(&mut self, raid_id: u32) {
        let (center, boss_bar) = {
            let Some(r) = self.raids.get(&raid_id) else {
                return;
            };
            (r.center, r.boss_bar)
        };

        // Update participating players
        let mut entered = Vec::new();
        let mut left = Vec::new();
        if let Some(r) = self.raids.get_mut(&raid_id) {
            for (&pid, p) in &self.players {
                let dist = horizontal_distance(p.pos, center);
                let in_range = dist <= RAID_RANGE;
                let is_viewer = r.participating_players.contains(&pid);
                if in_range && !is_viewer {
                    entered.push(pid);
                } else if !in_range && is_viewer && dist > RAID_LEAVE_RANGE {
                    left.push(pid);
                }
            }
            for &pid in &entered {
                r.participating_players.insert(pid);
            }
            for &pid in &left {
                r.participating_players.remove(&pid);
            }
        }

        for pid in entered {
            let (title, color, health) = if let Some(r) = self.raids.get(&raid_id) {
                let title = match r.state {
                    RaidState::Active => format!("Raid - Raiders remaining: {}", r.raiders.len()),
                    RaidState::Victory { .. } => "Raid - Victory".to_string(),
                    RaidState::Defeat { .. } => "Raid - Defeat".to_string(),
                    _ => "Raid".to_string(),
                };
                let color = if matches!(r.state, RaidState::Victory { .. }) {
                    BOSS_GREEN
                } else {
                    BOSS_RED
                };
                let health = if r.initial_wave_raiders > 0 {
                    (r.raiders.len() as f32) / (r.initial_wave_raiders as f32)
                } else {
                    1.0
                };
                (title, color, health)
            } else {
                ("Raid".to_string(), BOSS_RED, 1.0)
            };
            self.send(pid, boss_bar_add(boss_bar, &title, health, color));
        }
        for pid in left {
            self.send(pid, boss_bar_remove(boss_bar));
        }

        let state = self.raids.get(&raid_id).map(|r| r.state);
        match state {
            Some(RaidState::Warmup { ticks }) => {
                if ticks == 0 {
                    self.spawn_raid_wave(raid_id);
                } else if let Some(r) = self.raids.get_mut(&raid_id) {
                    r.state = RaidState::Warmup { ticks: ticks - 1 };
                }
            }
            Some(RaidState::Active) => {
                if let Some(r) = self.raids.get_mut(&raid_id) {
                    r.raiders
                        .retain(|id| self.mobs.get(id).is_some_and(|m| m.alive()));
                }

                let villagers_alive = self.mobs.values().any(|m| {
                    m.alive()
                        && m.kind == MobKind::Villager
                        && horizontal_distance(m.pos, center) <= VILLAGE_SEARCH_RANGE
                });

                if !villagers_alive {
                    let (players, raiders) = if let Some(r) = self.raids.get_mut(&raid_id) {
                        r.state = RaidState::Defeat { ticks: 200 };
                        (
                            r.participating_players.iter().copied().collect::<Vec<_>>(),
                            r.raiders.iter().copied().collect::<Vec<_>>(),
                        )
                    } else {
                        (Vec::new(), Vec::new())
                    };
                    for pid in players {
                        self.send(pid, boss_bar_title(boss_bar, "Raid - Defeat"));
                    }
                    for id in raiders {
                        if let Some(mob) = self.mobs.get_mut(&id) {
                            mob.celebrating = true;
                        }
                    }
                    return;
                }

                let (raiders_empty, wave, total_waves, bad_omen, spawned) = {
                    let Some(r) = self.raids.get(&raid_id) else {
                        return;
                    };
                    (
                        r.raiders.is_empty(),
                        r.wave,
                        r.total_waves,
                        r.bad_omen_level,
                        r.spawned_wave,
                    )
                };

                if raiders_empty && spawned {
                    if wave < total_waves {
                        let players = if let Some(r) = self.raids.get_mut(&raid_id) {
                            r.state = RaidState::Intermission { ticks: 100 };
                            r.participating_players.iter().copied().collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        };
                        for pid in players {
                            self.send(pid, boss_bar_title(boss_bar, "Raid"));
                            self.send(pid, boss_bar_health(boss_bar, 0.0));
                        }
                    } else {
                        // Victory!
                        let players = if let Some(r) = self.raids.get_mut(&raid_id) {
                            r.state = RaidState::Victory { ticks: 200 };
                            r.participating_players.iter().copied().collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        };
                        for pid in players {
                            self.send(pid, boss_bar_title(boss_bar, "Raid - Victory"));
                            self.send(pid, boss_bar_style(boss_bar, BOSS_GREEN));
                            self.send(pid, boss_bar_health(boss_bar, 1.0));
                            self.add_effect(
                                pid,
                                Effect {
                                    id: mob_effect::HERO_OF_THE_VILLAGE,
                                    duration: 48_000,
                                    amplifier: bad_omen as u8,
                                },
                            );
                        }
                        self.play_sound_at(
                            "minecraft:ui.toast.challenge_complete",
                            center,
                            1.0,
                            1.0,
                            None,
                        );
                        for mob in self.mobs.values_mut() {
                            if mob.alive()
                                && mob.kind == MobKind::Villager
                                && horizontal_distance(mob.pos, center) <= VILLAGE_SEARCH_RANGE
                            {
                                mob.celebrating = true;
                            }
                        }
                    }
                }
            }
            Some(RaidState::Intermission { ticks }) => {
                if ticks == 0 {
                    if let Some(r) = self.raids.get_mut(&raid_id) {
                        r.wave += 1;
                    }
                    self.spawn_raid_wave(raid_id);
                } else if let Some(r) = self.raids.get_mut(&raid_id) {
                    r.state = RaidState::Intermission { ticks: ticks - 1 };
                }
            }
            Some(RaidState::Victory { ticks }) => {
                if ticks == 0 {
                    let players = self
                        .raids
                        .get(&raid_id)
                        .map(|r| r.participating_players.iter().copied().collect::<Vec<_>>())
                        .unwrap_or_default();
                    for pid in players {
                        self.send(pid, boss_bar_remove(boss_bar));
                    }
                    if let Some(r) = self.raids.get_mut(&raid_id) {
                        r.state = RaidState::Over;
                    }
                } else if let Some(r) = self.raids.get_mut(&raid_id) {
                    r.state = RaidState::Victory { ticks: ticks - 1 };
                }
            }
            Some(RaidState::Defeat { ticks }) => {
                if ticks == 0 {
                    let players = self
                        .raids
                        .get(&raid_id)
                        .map(|r| r.participating_players.iter().copied().collect::<Vec<_>>())
                        .unwrap_or_default();
                    for pid in players {
                        self.send(pid, boss_bar_remove(boss_bar));
                    }
                    if let Some(r) = self.raids.get_mut(&raid_id) {
                        r.state = RaidState::Over;
                    }
                } else if let Some(r) = self.raids.get_mut(&raid_id) {
                    r.state = RaidState::Defeat { ticks: ticks - 1 };
                }
            }
            _ => {}
        }
    }

    /// Casts a line of Evoker Fangs towards `to`.
    pub(super) fn cast_evoker_fangs(&mut self, caster_id: i32, from: [f64; 3], to: [f64; 3]) {
        let dx = to[0] - from[0];
        let dz = to[2] - from[2];
        let dist = dx.hypot(dz);
        if dist < 0.1 {
            return;
        }
        let dir_x = dx / dist;
        let dir_z = dz / dist;
        let yaw = f64::atan2(-dx, dz).to_degrees() as f32;

        let num_fangs = ((dist / 1.5).round() as usize).clamp(4, 10);
        for i in 1..=num_fangs {
            let fd = (i as f64) * 1.5;
            let fx = from[0] + dir_x * fd;
            let fz = from[2] + dir_z * fd;
            let fy = self.find_ground_y(fx, from[1], fz);
            let fang_id = next_entity_id();
            let warmup = 6 + (i as u32) * 2;
            let fang = EvokerFang {
                id: fang_id,
                pos: [fx, fy, fz],
                yaw,
                warmup,
                age: 0,
                caster: Some(caster_id),
            };
            self.broadcast(spawn_fang(fang_id, [fx, fy, fz], yaw), None);
            self.evoker_fangs.insert(fang_id, fang);
        }
    }

    /// Evoker fangs packets in flight, for a player who just arrived.
    pub(super) fn evoker_fangs_packets(&self) -> Vec<Writer> {
        self.evoker_fangs
            .values()
            .map(|f| spawn_fang(f.id, f.pos, f.yaw))
            .collect()
    }

    /// Ticks Evoker Fangs, delivering magic damage upon snap and expiring.
    pub(super) fn tick_evoker_fangs(&mut self) {
        let mut expired = Vec::new();
        let mut bites = Vec::new();

        for (&id, fang) in self.evoker_fangs.iter_mut() {
            fang.age += 1;
            if fang.age == fang.warmup {
                bites.push((id, fang.pos, fang.caster));
            }
            if fang.age >= fang.warmup + 16 {
                expired.push(id);
            }
        }

        for (_id, pos, caster) in bites {
            self.play_sound_at("minecraft:entity.evoker_fangs.attack", pos, 1.0, 1.0, None);
            let hit_players: Vec<i32> = self
                .players
                .iter()
                .filter(|(_, p)| {
                    p.alive
                        && p.survival()
                        && horizontal_distance(p.pos, pos) <= 1.5
                        && (p.pos[1] - pos[1]).abs() <= 1.5
                })
                .map(|(&pid, _)| pid)
                .collect();
            for pid in hit_players {
                self.damage_from(pid, 6.0, Damage::Magic, caster);
            }

            let mut hurt_mobs = Vec::new();
            for (&mid, mob) in self.mobs.iter() {
                if mob.alive() && !mob.kind.is_raider() && Some(mid) != caster {
                    let d = horizontal_distance(mob.pos, pos);
                    let dy = (mob.pos[1] - pos[1]).abs();
                    if d <= 1.5 && dy <= 1.5 {
                        hurt_mobs.push(mid);
                    }
                }
            }
            for mid in hurt_mobs {
                if let Some(mut mob) = self.mobs.remove(&mid) {
                    self.hurt_mob(mid, &mut mob, 6.0, "magic");
                    if mob.alive() {
                        self.mobs.insert(mid, mob);
                    }
                }
            }
        }

        for id in expired {
            self.evoker_fangs.remove(&id);
            self.broadcast(items::remove_entity(id), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;
    use tokio::sync::mpsc;

    fn test_world() -> World {
        let dir = std::env::temp_dir().join(format!("levitas-raid-test-{}", std::process::id()));
        let (w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(12345),
            },
        );
        w
    }

    fn add_test_player(w: &mut World, id: i32, pos: [f64; 3]) {
        let (tx, _rx): (mpsc::Sender<Frame>, _) = mpsc::channel(16);
        let profile = Profile {
            uuid: id as u128,
            name: format!("Player_{id}"),
            properties: Vec::new(),
        };
        let p = Player::new(profile, tx, pos);
        w.players.insert(id, p);
    }

    #[test]
    fn bad_omen_converts_to_raid_omen_near_villager() {
        let mut w = test_world();
        let pid = 100;
        add_test_player(&mut w, pid, [0.0, -60.0, 0.0]);
        w.add_effect(
            pid,
            Effect {
                id: mob_effect::BAD_OMEN,
                duration: 120_000,
                amplifier: 1,
            },
        );
        let villager = Mob::new(MobKind::Villager, [10.0, -60.0, 0.0], &mut w.rng);
        w.mobs.insert(200, villager);

        w.tick_raids();

        let p = &w.players[&pid];
        assert!(p.effect(mob_effect::BAD_OMEN).is_none());
        let raid_omen = p.effect(mob_effect::RAID_OMEN).expect("has raid omen");
        assert_eq!(raid_omen.duration, 600);
        assert_eq!(raid_omen.amplifier, 1);
    }

    #[test]
    fn raid_omen_expiry_starts_raid() {
        let mut w = test_world();
        let pid = 100;
        add_test_player(&mut w, pid, [0.0, -60.0, 0.0]);
        w.add_effect(
            pid,
            Effect {
                id: mob_effect::RAID_OMEN,
                duration: 1,
                amplifier: 0,
            },
        );
        let villager = Mob::new(MobKind::Villager, [5.0, -60.0, 0.0], &mut w.rng);
        w.mobs.insert(200, villager);

        w.tick_effects();

        assert_eq!(w.raids.len(), 1);
        let raid = w.raids.values().next().unwrap();
        assert_eq!(raid.wave, 1);
        assert_eq!(raid.bad_omen_level, 0);
        assert!(raid.id > 0);
    }

    #[test]
    fn raid_wave_spawns_and_clearing_grants_victory() {
        let mut w = test_world();
        let pid = 100;
        add_test_player(&mut w, pid, [0.0, -60.0, 0.0]);
        let villager = Mob::new(MobKind::Villager, [0.0, -60.0, 0.0], &mut w.rng);
        w.mobs.insert(200, villager);

        let raid_id = w.start_raid([0.0, -60.0, 0.0], 0);
        // Force total waves to 1 for quick test
        w.raids.get_mut(&raid_id).unwrap().total_waves = 1;

        // Advance warmup
        w.spawn_raid_wave(raid_id);

        let raid = &w.raids[&raid_id];
        assert_eq!(raid.state, RaidState::Active);
        assert!(!raid.raiders.is_empty());
        let raider_ids: Vec<i32> = raid.raiders.iter().copied().collect();

        // One of the pillagers must be the patrol leader
        let leader_count = raider_ids
            .iter()
            .filter(|&&id| w.mobs[&id].is_patrol_leader)
            .count();
        assert_eq!(leader_count, 1);

        // Kill all raiders in wave
        for rid in raider_ids {
            let mut mob = w.mobs.remove(&rid).unwrap();
            w.kill_mob(rid, &mut mob);
        }

        w.tick_raids();

        let raid = &w.raids[&raid_id];
        assert!(matches!(raid.state, RaidState::Victory { .. }));
        let p = &w.players[&pid];
        assert!(p.effect(mob_effect::HERO_OF_THE_VILLAGE).is_some());
    }

    #[test]
    fn evoker_fangs_snap_and_deal_magic_damage() {
        let mut w = test_world();
        let pid = 100;
        add_test_player(&mut w, pid, [3.0, -60.0, 0.0]);
        let start_health = w.players[&pid].health;

        w.cast_evoker_fangs(500, [0.0, -60.0, 0.0], [5.0, -60.0, 0.0]);
        assert!(!w.evoker_fangs.is_empty());

        // Fast-forward ticks to snap
        for _ in 0..15 {
            w.tick_evoker_fangs();
        }

        assert!(w.players[&pid].health < start_health);
    }

    #[test]
    fn patrol_leader_drops_ominous_bottle() {
        let mut w = test_world();
        let mut leader = Mob::new(MobKind::Pillager, [0.0, -60.0, 0.0], &mut w.rng);
        leader.is_patrol_leader = true;
        let id = next_entity_id();

        w.kill_mob(id, &mut leader);

        let dropped_bottle = w
            .items
            .values()
            .any(|item| item.stack().item == ItemInfo::id_of("minecraft:ominous_bottle").unwrap());
        assert!(dropped_bottle);
    }
}
