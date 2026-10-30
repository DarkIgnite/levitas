//! The dragon fight in the End, following the Minecraft Wiki, simplified.
//!
//! When players first come to the End, end crystals appear on the obsidian pillars,
//! the exit portal's bedrock frame is built at the island's centre, and the ender
//! dragon (200 health) takes off. It circles the island, now and then dives at a
//! player, and now and then lands on the portal to rest, when its head is in reach.
//! Crystals within 32 blocks heal it; breaking one blows it up and hurts the dragon if
//! it was healing from it. Hits to the head count in full, elsewhere a quarter plus
//! one. When it dies it rises and bursts into experience (12,000 the first time), the
//! exit portal opens, and the dragon egg sits on top. The fight is kept in
//! `levitas_dragon.dat` in the End's folder.

use std::collections::HashMap;
use std::fs;

use levitas_nbt::{Compound, Tag};
use levitas_registry::{block, entity_type};

use super::dimension::Dimension;
use super::survival::Damage;
use super::*;

const MAX_HEALTH: f32 = 200.0;
const DRAGON_PARTS: i32 = 8;
const CRYSTAL_HEAL_RANGE: f64 = 32.0;
const CRYSTAL_POWER: f32 = 6.0;
/// How far around the centre to look for the pillars' bedrock tops.
const PILLAR_SEARCH: i32 = 50;
const CIRCLE_RADIUS: f64 = 60.0;
const CIRCLE_POINTS: usize = 12;
const DEATH_TICKS: u32 = 200;
const FIRST_KILL_XP: i32 = 12_000;
const LATER_KILL_XP: i32 = 500;
const PERCH_TICKS: u32 = 160;
const CHARGE_TICKS: u32 = 100;
const CONTACT_DAMAGE: f32 = 10.0;
const FIGHT_FILE: &str = "levitas_dragon.dat";
/// Entity data: a living entity's health, and a crystal's beam target.
const DATA_HEALTH: u8 = 9;
const DATA_BEAM: u8 = 8;
const SERIALIZER_FLOAT: i32 = 3;
const SERIALIZER_OPTIONAL_POS: i32 = 11;
const BOSS_ADD: i32 = 0;
const BOSS_REMOVE: i32 = 1;
const BOSS_HEALTH: i32 = 2;
const BOSS_PINK: i32 = 0;
/// Boss music and the End's fog.
const BOSS_FLAGS: u8 = 0x02 | 0x04;
const LEVEL_EVENT_DRAGON_DIES: i32 = 1028;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Circling { waypoint: usize },
    Charging { target: i32, ticks: u32 },
    Landing,
    Perched { ticks: u32 },
    Dying { ticks: u32 },
}

pub struct Dragon {
    id: i32,
    pos: [f64; 3],
    vel: [f64; 3],
    yaw: f32,
    pitch: f32,
    health: f32,
    phase: Phase,
    /// Ticks until the next hit on a player counts.
    contact_cooldown: u32,
}

pub struct Crystal {
    pos: [f64; 3],
    /// Whether its beam is on the dragon.
    healing: bool,
}

/// The state of the End's dragon fight.
pub struct Fight {
    dragon: Option<Dragon>,
    crystals: HashMap<i32, Crystal>,
    /// The exit portal's centre, at the level of its portal blocks.
    podium: BlockPos,
    killed: bool,
    boss_bar: u128,
}

fn bedrock_ring(x: i32, z: i32) -> f64 {
    f64::from(x * x + z * z).sqrt()
}

impl World {
    /// Starts (or brings back) the dragon fight once players are in the End.
    pub(super) fn tick_dragon_fight(&mut self) {
        if self.dimension != Dimension::End {
            return;
        }
        if self.fight.is_none() {
            if self.players.is_empty() {
                return;
            }
            self.start_fight();
        }
        self.tick_dragon();
    }

    fn start_fight(&mut self) {
        let saved = self.load_fight();
        let podium = match saved.as_ref().and_then(|s| s.podium) {
            Some(p) => p,
            None => {
                let y = (MIN_Y..=MAX_Y)
                    .rev()
                    .find(|&y| !is_air(self.block(BlockPos::new(0, y, 0))))
                    .unwrap_or(60)
                    + 1;
                BlockPos::new(0, y, 0)
            }
        };
        let killed = saved.as_ref().is_some_and(|s| s.killed);
        let crystal_spots = match saved.as_ref() {
            Some(s) => s.crystals.clone(),
            None => self.pillar_tops(),
        };
        let mut crystals = HashMap::new();
        if !killed {
            for pos in crystal_spots {
                crystals.insert(
                    next_entity_id(),
                    Crystal {
                        pos,
                        healing: false,
                    },
                );
            }
        }
        let mut fight = Fight {
            dragon: None,
            crystals,
            podium,
            killed,
            boss_bar: (u128::from(self.rng.next_u64()) << 64) | u128::from(self.rng.next_u64()),
        };
        self.build_podium(podium, killed, false);
        if !killed {
            let id = next_entity_ids(1 + DRAGON_PARTS);
            let health = saved.and_then(|s| s.health).unwrap_or(MAX_HEALTH);
            fight.dragon = Some(Dragon {
                id,
                pos: [0.5, f64::from(podium.y) + 30.0, CIRCLE_RADIUS],
                vel: [0.0; 3],
                yaw: 0.0,
                pitch: 0.0,
                health,
                phase: Phase::Circling { waypoint: 0 },
                contact_cooldown: 0,
            });
        }
        self.fight = Some(fight);
        for packet in self.dragon_packets() {
            self.broadcast(packet, None);
        }
    }

    /// The tops of the obsidian pillars: bedrock high above the island.
    fn pillar_tops(&mut self) -> Vec<[f64; 3]> {
        let mut tops = Vec::new();
        for x in -PILLAR_SEARCH..=PILLAR_SEARCH {
            for z in -PILLAR_SEARCH..=PILLAR_SEARCH {
                if bedrock_ring(x, z) < 20.0 {
                    continue; // the portal at the centre
                }
                let top = (MIN_Y..=MAX_Y).rev().find(|&y| {
                    let s = self.block(BlockPos::new(x, y, z));
                    !is_air(s) && !crate::blocks::fire::is_fire(s)
                });
                if let Some(y) = top {
                    if self.block(BlockPos::new(x, y, z)) == block::BEDROCK {
                        tops.push([f64::from(x) + 0.5, f64::from(y) + 1.0, f64::from(z) + 0.5]);
                    }
                }
            }
        }
        tops
    }

    /// The exit portal: a bedrock basin with a pillar in the middle, open once the
    /// dragon is dead, with the egg on top the first time.
    fn build_podium(&mut self, centre: BlockPos, open: bool, egg: bool) {
        let portal = BlockInfo::by_name("minecraft:end_portal")
            .expect("End portals exist")
            .default_state;
        for x in -4..=4 {
            for z in -4..=4 {
                let d = bedrock_ring(x, z);
                if d > 3.5 {
                    continue;
                }
                let at = |dy: i32| BlockPos::new(centre.x + x, centre.y + dy, centre.z + z);
                self.set_block(at(-1), block::BEDROCK);
                let middle = x == 0 && z == 0;
                let level = if middle || d > 2.5 {
                    block::BEDROCK
                } else if open {
                    portal
                } else {
                    block::AIR
                };
                self.set_block(at(0), level);
                for dy in 1..=3 {
                    self.set_block(at(dy), if middle { block::BEDROCK } else { block::AIR });
                }
            }
        }
        if egg {
            self.set_block(
                BlockPos::new(centre.x, centre.y + 4, centre.z),
                block::DRAGON_EGG,
            );
        }
    }

    /// The dragon, its boss bar and the crystals, for players in the End.
    pub(super) fn dragon_packets(&self) -> Vec<Writer> {
        let Some(fight) = &self.fight else {
            return Vec::new();
        };
        let mut packets = Vec::new();
        for (&id, c) in &fight.crystals {
            packets.push(spawn(id, entity_type::END_CRYSTAL, c.pos, 0.0, 0.0));
        }
        if let Some(d) = &fight.dragon {
            packets.push(spawn(
                d.id,
                entity_type::ENDER_DRAGON,
                d.pos,
                d.yaw,
                d.pitch,
            ));
            packets.push(health_data(d.id, d.health));
            packets.push(
                Writer::packet(clientbound::BOSS_EVENT)
                    .uuid(fight.boss_bar)
                    .varint(BOSS_ADD)
                    .text("Ender Dragon")
                    .f32(d.health / MAX_HEALTH)
                    .varint(BOSS_PINK)
                    .varint(0) // one bar
                    .u8(BOSS_FLAGS),
            );
        }
        packets
    }

    /// Takes the boss bar off a player leaving the End.
    pub(super) fn leave_fight(&mut self, id: i32) {
        let Some(bar) = self.fight.as_ref().map(|f| f.boss_bar) else {
            return;
        };
        self.send(
            id,
            Writer::packet(clientbound::BOSS_EVENT)
                .uuid(bar)
                .varint(BOSS_REMOVE),
        );
    }

    fn tick_dragon(&mut self) {
        let Some(fight) = self.fight.as_mut() else {
            return;
        };
        let Some(mut dragon) = fight.dragon.take() else {
            return;
        };
        let podium = fight.podium;
        dragon.contact_cooldown = dragon.contact_cooldown.saturating_sub(1);
        let players: Vec<(i32, [f64; 3])> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive && p.survival())
            .map(|(&id, p)| (id, p.pos))
            .collect();
        let centre_top = [0.5, f64::from(podium.y) + 5.0, 0.5];

        // Where to fly, and how fast.
        let (goal, speed) = match dragon.phase {
            Phase::Circling { waypoint } => {
                let angle = waypoint as f64 / CIRCLE_POINTS as f64 * std::f64::consts::TAU;
                let goal = [
                    angle.cos() * CIRCLE_RADIUS,
                    f64::from(podium.y) + 20.0 + (waypoint % 3) as f64 * 5.0,
                    angle.sin() * CIRCLE_RADIUS,
                ];
                (goal, 0.6)
            }
            Phase::Charging { target, .. } => {
                let goal = players
                    .iter()
                    .find(|(id, _)| *id == target)
                    .map_or(centre_top, |(_, p)| [p[0], p[1] + 1.0, p[2]]);
                (goal, 1.0)
            }
            Phase::Landing => (centre_top, 0.4),
            Phase::Perched { .. } => (centre_top, 0.0),
            Phase::Dying { .. } => ([dragon.pos[0], dragon.pos[1] + 10.0, dragon.pos[2]], 0.1),
        };
        let to = [0, 1, 2].map(|i| goal[i] - dragon.pos[i]);
        let distance = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
        if speed > 0.0 && distance > 1e-6 {
            for (v, t) in dragon.vel.iter_mut().zip(to) {
                *v = *v * 0.9 + t / distance * speed * 0.15;
            }
        } else {
            dragon.vel = [0.0; 3];
        }
        for (p, v) in dragon.pos.iter_mut().zip(dragon.vel) {
            *p += v;
        }
        let horizontal = dragon.vel[0].hypot(dragon.vel[2]);
        if horizontal > 1e-3 {
            // The dragon's yaw runs opposite to other entities'.
            dragon.yaw = (-dragon.vel[0].atan2(dragon.vel[2])).to_degrees() as f32 + 180.0;
            dragon.pitch = (-dragon.vel[1].atan2(horizontal)).to_degrees() as f32;
        }

        // What comes next.
        dragon.phase = match dragon.phase {
            Phase::Circling { waypoint } => {
                if distance < 10.0 {
                    let next = (waypoint + 1) % CIRCLE_POINTS;
                    let crystals_left = fight_crystals(&self.fight);
                    let land_odds = if crystals_left == 0 { 3 } else { 8 };
                    if !players.is_empty() && self.rng.below(land_odds) == 0 {
                        Phase::Landing
                    } else if let Some(&(target, _)) =
                        players.get(self.rng.below(players.len().max(1) as u32) as usize)
                    {
                        if self.rng.below(4) == 0 {
                            Phase::Charging {
                                target,
                                ticks: CHARGE_TICKS,
                            }
                        } else {
                            Phase::Circling { waypoint: next }
                        }
                    } else {
                        Phase::Circling { waypoint: next }
                    }
                } else {
                    Phase::Circling { waypoint }
                }
            }
            Phase::Charging { target, ticks } => {
                if ticks == 0 || distance < 2.0 {
                    Phase::Circling { waypoint: 0 }
                } else {
                    Phase::Charging {
                        target,
                        ticks: ticks - 1,
                    }
                }
            }
            Phase::Landing if distance < 2.0 => Phase::Perched { ticks: PERCH_TICKS },
            Phase::Perched { ticks: 0 } => Phase::Circling { waypoint: 0 },
            Phase::Perched { ticks } => Phase::Perched { ticks: ticks - 1 },
            other => other,
        };

        // Running into players.
        let charging = matches!(dragon.phase, Phase::Charging { .. });
        if dragon.contact_cooldown == 0 && !matches!(dragon.phase, Phase::Dying { .. }) {
            let hit: Vec<i32> = players
                .iter()
                .filter(|(_, p)| {
                    (p[0] - dragon.pos[0]).abs() < 4.0
                        && (p[2] - dragon.pos[2]).abs() < 4.0
                        && (p[1] - dragon.pos[1]).abs() < 4.0
                })
                .map(|&(id, _)| id)
                .collect();
            for id in hit {
                let p = self.players[&id].pos;
                let away = [p[0] - dragon.pos[0], p[2] - dragon.pos[2]];
                let length = away[0].hypot(away[1]).max(1e-6);
                self.push_player(id, [away[0] / length * 2.0, 1.0, away[1] / length * 2.0]);
                if charging {
                    self.damage_from(
                        id,
                        CONTACT_DAMAGE,
                        Damage::Mob("Ender Dragon"),
                        Some(dragon.id),
                    );
                }
                dragon.contact_cooldown = 20;
            }
        }

        // Crystals heal it.
        if !matches!(dragon.phase, Phase::Dying { .. }) {
            self.heal_from_crystals(&mut dragon);
        }

        if let Phase::Dying { ticks } = dragon.phase {
            if ticks >= DEATH_TICKS {
                self.dragon_died(dragon);
                return;
            }
            dragon.phase = Phase::Dying { ticks: ticks + 1 };
        }

        self.broadcast(
            Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                .varint(dragon.id)
                .varint(0) // a linear path
                .f64(dragon.pos[0])
                .f64(dragon.pos[1])
                .f64(dragon.pos[2])
                .f32(dragon.yaw)
                .f32(dragon.pitch)
                .bool(false),
            None,
        );
        if let Some(fight) = self.fight.as_mut() {
            fight.dragon = Some(dragon);
        }
    }

    fn heal_from_crystals(&mut self, dragon: &mut Dragon) {
        let Some(fight) = self.fight.as_mut() else {
            return;
        };
        let nearest = fight
            .crystals
            .iter()
            .map(|(&id, c)| {
                let d: f64 = (0..3).map(|i| (c.pos[i] - dragon.pos[i]).powi(2)).sum();
                (id, d.sqrt())
            })
            .filter(|&(_, d)| d <= CRYSTAL_HEAL_RANGE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id);
        let mut beams = Vec::new();
        for (&id, c) in fight.crystals.iter_mut() {
            let healing = Some(id) == nearest;
            if healing != c.healing {
                c.healing = healing;
                beams.push((id, healing));
            }
        }
        if nearest.is_some() && self.tick.is_multiple_of(10) && dragon.health < MAX_HEALTH {
            dragon.health = (dragon.health + 1.0).min(MAX_HEALTH);
            self.dragon_health_changed(dragon);
        }
        let target = BlockPos::new(
            dragon.pos[0].floor() as i32,
            dragon.pos[1].floor() as i32 - 2,
            dragon.pos[2].floor() as i32,
        );
        for (id, on) in beams {
            let packet = Writer::packet(clientbound::SET_ENTITY_DATA)
                .varint(id)
                .u8(DATA_BEAM)
                .varint(SERIALIZER_OPTIONAL_POS)
                .bool(on);
            let packet = if on { packet.position(target) } else { packet };
            self.broadcast(packet.u8(0xFF), None);
        }
    }

    fn dragon_health_changed(&mut self, dragon: &Dragon) {
        let Some(bar) = self.fight.as_ref().map(|f| f.boss_bar) else {
            return;
        };
        self.broadcast(health_data(dragon.id, dragon.health), None);
        self.broadcast(
            Writer::packet(clientbound::BOSS_EVENT)
                .uuid(bar)
                .varint(BOSS_HEALTH)
                .f32(dragon.health / MAX_HEALTH),
            None,
        );
    }

    /// A player hit the dragon (or one of its parts) or a crystal. Returns whether
    /// `target` was one of those.
    pub(super) fn attack_dragon_or_crystal(&mut self, player: i32, target: i32) -> bool {
        let Some(fight) = self.fight.as_ref() else {
            return false;
        };
        if fight.crystals.contains_key(&target) {
            self.destroy_crystal(target);
            return true;
        }
        let Some(dragon) = fight.dragon.as_ref() else {
            return false;
        };
        let part = target - dragon.id;
        if !(0..=DRAGON_PARTS).contains(&part) {
            return false;
        }
        let Some(p) = self.players.get(&player) else {
            return true;
        };
        let item = ItemInfo::get(p.inventory.slots[p.hand_slot(0)].item);
        let (damage, speed) = item.map_or((1.0, 4.0), |i| (i.attack_damage, i.attack_speed));
        let cooldown = 20.0 / f64::from(speed);
        let charge = ((self.tick.saturating_sub(p.attack_reset) as f64 + 0.5) / cooldown).min(1.0);
        let damage = damage * (0.2 + (charge * charge) as f32 * 0.8);
        if let Some(p) = self.players.get_mut(&player) {
            p.attack_reset = self.tick;
        }
        // The head (the first part) takes hits in full.
        let head = part <= 1;
        self.hurt_dragon(damage, head);
        true
    }

    /// Hurts the dragon: in full on the head, a quarter plus one elsewhere.
    fn hurt_dragon(&mut self, amount: f32, head: bool) {
        let Some(mut dragon) = self.fight.as_mut().and_then(|f| f.dragon.take()) else {
            return;
        };
        if !matches!(dragon.phase, Phase::Dying { .. }) {
            let dealt = if head { amount } else { amount / 4.0 + 1.0 };
            dragon.health = (dragon.health - dealt).max(0.0);
            self.play_sound_at(
                "minecraft:entity.ender_dragon.hurt",
                dragon.pos,
                5.0,
                1.0,
                None,
            );
            if dragon.health <= 0.0 {
                dragon.phase = Phase::Dying { ticks: 0 };
                self.play_sound_at(
                    "minecraft:entity.ender_dragon.death",
                    dragon.pos,
                    5.0,
                    1.0,
                    None,
                );
            } else if matches!(dragon.phase, Phase::Perched { .. }) && self.rng.below(4) == 0 {
                // Hurt enough while resting, it takes off.
                dragon.phase = Phase::Circling { waypoint: 0 };
            }
            self.dragon_health_changed(&dragon);
        }
        if let Some(fight) = self.fight.as_mut() {
            fight.dragon = Some(dragon);
        }
    }

    /// An arrow (or anything else in flight) at `at`: hits the dragon or breaks a
    /// crystal there. Returns whether it hit something.
    pub(super) fn dragon_or_crystal_at(&mut self, at: [f64; 3], damage: f32) -> bool {
        let Some(fight) = self.fight.as_ref() else {
            return false;
        };
        let crystal = fight.crystals.iter().find(|(_, c)| {
            (at[0] - c.pos[0]).abs() <= 1.0
                && (at[2] - c.pos[2]).abs() <= 1.0
                && (0.0..=2.0).contains(&(at[1] - c.pos[1]))
        });
        if let Some((&id, _)) = crystal {
            self.destroy_crystal(id);
            return true;
        }
        let hits_dragon = fight.dragon.as_ref().is_some_and(|d| {
            (at[0] - d.pos[0]).abs() <= 4.0
                && (at[2] - d.pos[2]).abs() <= 4.0
                && (-1.0..=4.0).contains(&(at[1] - d.pos[1]))
        });
        if hits_dragon {
            self.hurt_dragon(damage, false);
        }
        hits_dragon
    }

    fn destroy_crystal(&mut self, id: i32) {
        let Some(crystal) = self.fight.as_mut().and_then(|f| f.crystals.remove(&id)) else {
            return;
        };
        self.broadcast(items::remove_entity(id), None);
        self.explode(crystal.pos, CRYSTAL_POWER, "End Crystal");
        // The dragon feels the loss of the crystal healing it.
        if crystal.healing {
            self.hurt_dragon(10.0, true);
        }
    }

    fn dragon_died(&mut self, dragon: Dragon) {
        let first = self.fight.as_ref().is_some_and(|f| !f.killed);
        let (podium, bar) = match self.fight.as_mut() {
            Some(f) => {
                f.killed = true;
                (f.podium, f.boss_bar)
            }
            None => return,
        };
        self.broadcast(items::remove_entity(dragon.id), None);
        self.broadcast(
            Writer::packet(clientbound::BOSS_EVENT)
                .uuid(bar)
                .varint(BOSS_REMOVE),
            None,
        );
        self.broadcast(
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(LEVEL_EVENT_DRAGON_DIES)
                .position(podium)
                .i32(0)
                .bool(true),
            None,
        );
        let xp = if first { FIRST_KILL_XP } else { LATER_KILL_XP };
        self.spawn_xp([0.5, f64::from(podium.y) + 5.0, 0.5], xp);
        self.build_podium(podium, true, first);
        self.save_fight();
    }

    fn load_fight(&self) -> Option<SavedFight> {
        let bytes = fs::read(self.dimension.folder(&self.dir).join(FIGHT_FILE)).ok()?;
        let (_, nbt) = levitas_nbt::from_gzip(&bytes).ok()?;
        let pos = |c: &Compound| -> Option<[f64; 3]> {
            Some([c.float("x")?, c.float("y")?, c.float("z")?])
        };
        Some(SavedFight {
            killed: nbt.int("DragonKilled").unwrap_or(0) != 0,
            health: nbt.float("Health").map(|h| h as f32),
            podium: nbt.int_array("Podium").and_then(|p| match p {
                &[x, y, z] => Some(BlockPos::new(x, y, z)),
                _ => None,
            }),
            crystals: nbt
                .list("Crystals")
                .unwrap_or_default()
                .iter()
                .filter_map(|t| pos(t.as_compound()?))
                .collect(),
        })
    }

    /// Saves the fight: whether the dragon died, its health, and the crystals left.
    pub(super) fn save_fight(&self) {
        let Some(fight) = &self.fight else {
            return;
        };
        let crystals: Vec<Tag> = fight
            .crystals
            .values()
            .map(|c| {
                Tag::Compound(
                    Compound::new()
                        .with("x", c.pos[0])
                        .with("y", c.pos[1])
                        .with("z", c.pos[2]),
                )
            })
            .collect();
        let mut nbt = Compound::new()
            .with("DragonKilled", fight.killed)
            .with(
                "Podium",
                vec![fight.podium.x, fight.podium.y, fight.podium.z],
            )
            .with("Crystals", crystals);
        if let Some(d) = &fight.dragon {
            nbt.insert("Health", d.health);
        }
        let folder = self.dimension.folder(&self.dir);
        let result = fs::create_dir_all(&folder)
            .and_then(|()| fs::write(folder.join(FIGHT_FILE), levitas_nbt::to_gzip("", &nbt)));
        if let Err(e) = result {
            tracing::error!("could not save the dragon fight: {e}");
        }
    }
}

struct SavedFight {
    killed: bool,
    health: Option<f32>,
    podium: Option<BlockPos>,
    crystals: Vec<[f64; 3]>,
}

fn fight_crystals(fight: &Option<Fight>) -> usize {
    fight.as_ref().map_or(0, |f| f.crystals.len())
}

fn spawn(id: i32, kind: i32, pos: [f64; 3], yaw: f32, pitch: f32) -> Writer {
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(kind)
        .f64(pos[0])
        .f64(pos[1])
        .f64(pos[2])
        .lp_vec3([0.0, 0.0, 0.0])
        .u8(angle(pitch))
        .u8(angle(yaw))
        .u8(angle(yaw))
        .varint(0)
}

fn health_data(id: i32, health: f32) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_HEALTH)
        .varint(SERIALIZER_FLOAT)
        .f32(health)
        .u8(0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn the_dragon_fight_from_crystals_to_the_open_exit_portal() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-dragon-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut end, _) = World::new_in(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(1),
            },
            Dimension::End,
        );
        let (out, _frames) = mpsc::channel(1_000_000);
        let player = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        end.players
            .insert(player, Player::new(profile, out, [100.5, 49.0, 0.5]));
        end.tick_dragon_fight();
        let fight = end.fight.as_ref().expect("the fight began");
        let crystals = fight.crystals.len();
        assert!((8..=10).contains(&crystals), "{crystals} crystals");
        let dragon_id = fight.dragon.as_ref().unwrap().id;
        assert_eq!(fight.dragon.as_ref().unwrap().health, MAX_HEALTH);
        let podium = fight.podium;

        // Hitting a crystal blows it up.
        let crystal = *fight.crystals.keys().next().unwrap();
        assert!(end.attack_dragon_or_crystal(player, crystal));
        assert_eq!(end.fight.as_ref().unwrap().crystals.len(), crystals - 1);

        // Bring the dragon down, and wait out its death.
        end.hurt_dragon(1000.0, true);
        assert!(
            end.attack_dragon_or_crystal(player, dragon_id + 1),
            "its head is part of it"
        );
        for _ in 0..=DEATH_TICKS {
            end.tick_dragon_fight();
        }
        let fight = end.fight.as_ref().unwrap();
        assert!(fight.killed);
        assert!(fight.dragon.is_none());
        assert_eq!(
            crate::blocks::name(end.block(BlockPos::new(1, podium.y, 1))),
            "minecraft:end_portal"
        );
        assert_eq!(
            end.block(BlockPos::new(0, podium.y + 4, 0)),
            block::DRAGON_EGG
        );
        assert!(!end.orbs.is_empty(), "experience burst out");
    }
}
