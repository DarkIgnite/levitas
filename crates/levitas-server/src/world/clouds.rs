//! Area effect clouds, created by lingering potions and dragon's breath, following the
//! Minecraft Wiki.
//!
//! A cloud holds potion effects or dragon's breath, lingers in an area for up to 30
//! seconds, applies its effects to players and mobs entering its boundary every second
//! (20-tick reapplication delay), and shrinks by 0.5 blocks per application until it
//! dissipates. Players can scoop dragon's breath into an empty glass bottle.
//!
//! Portions of entity metadata and particle layout adapted from Pumpkin
//! (`crates/pumpkin-core/src/entity/area_effect_cloud.rs`), Copyright the Pumpkin contributors, GPL-3.0.

use std::collections::HashMap;

use levitas_protocol::Writer;
use levitas_registry::packet::play::clientbound;
use levitas_registry::{entity_type, mob_effect, particle, Effect};

use super::survival::Damage;
use super::*;
use crate::potions;

/// Default initial radius of a lingering potion or dragon breath cloud.
pub const DEFAULT_RADIUS: f32 = 3.0;
/// Default lifetime in ticks (30 seconds).
pub const DEFAULT_DURATION: i32 = 600;
/// Ticks to wait before the cloud becomes active.
pub const DEFAULT_WAIT_TIME: i32 = 20;
/// Ticks between reapplications to the same entity.
pub const REAPPLICATION_DELAY: i32 = 20;
/// Radius shrink on each application to an entity.
pub const RADIUS_ON_USE: f32 = -0.5;

pub struct AreaEffectCloud {
    pub pos: [f64; 3],
    pub radius: f32,
    pub duration: i32,
    pub age: i32,
    pub wait_time: i32,
    pub reapplication_delay: i32,
    pub radius_on_use: f32,
    pub radius_per_tick: f32,
    pub potion: Option<u8>,
    pub is_dragon_breath: bool,
    pub color: i32,
    #[allow(dead_code)]
    pub owner: Option<i32>,
    pub reapplication_map: HashMap<i32, i32>,
    pub last_sent_radius: f32,
}

impl AreaEffectCloud {
    pub fn new_lingering(pos: [f64; 3], potion: Option<u8>, owner: Option<i32>) -> Self {
        let color = potion.map_or(potions::SPLASH_COLOR, potions::color);
        Self {
            pos,
            radius: DEFAULT_RADIUS,
            duration: DEFAULT_DURATION,
            age: 0,
            wait_time: DEFAULT_WAIT_TIME,
            reapplication_delay: REAPPLICATION_DELAY,
            radius_on_use: RADIUS_ON_USE,
            radius_per_tick: -DEFAULT_RADIUS / DEFAULT_DURATION as f32,
            potion,
            is_dragon_breath: false,
            color,
            owner,
            reapplication_map: HashMap::new(),
            last_sent_radius: DEFAULT_RADIUS,
        }
    }

    pub fn new_dragon_breath(pos: [f64; 3]) -> Self {
        Self {
            pos,
            radius: DEFAULT_RADIUS,
            duration: DEFAULT_DURATION,
            age: 0,
            wait_time: DEFAULT_WAIT_TIME,
            reapplication_delay: REAPPLICATION_DELAY,
            radius_on_use: RADIUS_ON_USE,
            radius_per_tick: -DEFAULT_RADIUS / DEFAULT_DURATION as f32,
            potion: None,
            is_dragon_breath: true,
            color: 0xCC00CC,
            owner: None,
            reapplication_map: HashMap::new(),
            last_sent_radius: DEFAULT_RADIUS,
        }
    }
}

pub fn spawn_packet(id: i32, cloud: &AreaEffectCloud) -> Writer {
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::AREA_EFFECT_CLOUD)
        .f64(cloud.pos[0])
        .f64(cloud.pos[1])
        .f64(cloud.pos[2])
        .lp_vec3([0.0, 0.0, 0.0])
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(0)
}

pub fn data_packet(id: i32, cloud: &AreaEffectCloud) -> Writer {
    const DATA_RADIUS: u8 = 8;
    const DATA_WAITING: u8 = 9;
    const DATA_PARTICLE: u8 = 10;
    const SERIALIZER_FLOAT: i32 = 3;
    const SERIALIZER_BOOLEAN: i32 = 8;
    const SERIALIZER_PARTICLE: i32 = 16;

    let is_waiting = cloud.wait_time > 0;
    let mut packet = Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_RADIUS)
        .varint(SERIALIZER_FLOAT)
        .f32(cloud.radius)
        .u8(DATA_WAITING)
        .varint(SERIALIZER_BOOLEAN)
        .bool(is_waiting)
        .u8(DATA_PARTICLE)
        .varint(SERIALIZER_PARTICLE);

    if cloud.is_dragon_breath {
        packet = packet.varint(particle::DRAGON_BREATH);
    } else {
        let argb = (0xFFi32 << 24) | (cloud.color & 0x00FF_FFFF);
        packet = packet
            .varint(particle::ENTITY_EFFECT)
            .bytes(&argb.to_be_bytes());
    }

    packet.u8(0xFF)
}

pub fn update_packet(id: i32, radius: f32, waiting: bool) -> Writer {
    const DATA_RADIUS: u8 = 8;
    const DATA_WAITING: u8 = 9;
    const SERIALIZER_FLOAT: i32 = 3;
    const SERIALIZER_BOOLEAN: i32 = 8;

    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_RADIUS)
        .varint(SERIALIZER_FLOAT)
        .f32(radius)
        .u8(DATA_WAITING)
        .varint(SERIALIZER_BOOLEAN)
        .bool(waiting)
        .u8(0xFF)
}

impl World {
    pub(super) fn spawn_lingering_cloud(
        &mut self,
        pos: [f64; 3],
        potion: Option<u8>,
        owner: Option<i32>,
    ) -> i32 {
        let cloud = AreaEffectCloud::new_lingering(pos, potion, owner);
        let id = next_entity_id();
        self.broadcast(spawn_packet(id, &cloud), None);
        self.broadcast(data_packet(id, &cloud), None);
        self.area_effect_clouds.insert(id, cloud);
        id
    }

    pub(super) fn spawn_dragon_breath_cloud(&mut self, pos: [f64; 3]) -> i32 {
        let cloud = AreaEffectCloud::new_dragon_breath(pos);
        let id = next_entity_id();
        self.broadcast(spawn_packet(id, &cloud), None);
        self.broadcast(data_packet(id, &cloud), None);
        self.area_effect_clouds.insert(id, cloud);
        id
    }

    pub(super) fn area_effect_cloud_packets(&self) -> Vec<Writer> {
        self.area_effect_clouds
            .iter()
            .flat_map(|(&id, c)| [spawn_packet(id, c), data_packet(id, c)])
            .collect()
    }

    pub(super) fn tick_area_effect_clouds(&mut self) {
        let ids: Vec<i32> = self.area_effect_clouds.keys().copied().collect();
        for id in ids {
            let Some(mut cloud) = self.area_effect_clouds.remove(&id) else {
                continue;
            };
            cloud.age += 1;
            if cloud.wait_time > 0 {
                cloud.wait_time -= 1;
                if cloud.wait_time == 0 {
                    self.broadcast(update_packet(id, cloud.radius, false), None);
                }
            }

            cloud.radius += cloud.radius_per_tick;

            if cloud.age >= cloud.duration || cloud.radius <= 0.0 {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }

            if (cloud.radius - cloud.last_sent_radius).abs() >= 0.1 {
                cloud.last_sent_radius = cloud.radius;
                self.broadcast(update_packet(id, cloud.radius, cloud.wait_time > 0), None);
            }

            // Waiting clouds don't apply effects yet.
            if cloud.wait_time > 0 {
                self.area_effect_clouds.insert(id, cloud);
                continue;
            }

            // Decrement reapplication cooldowns.
            cloud.reapplication_map.retain(|_, ticks| {
                *ticks -= 1;
                *ticks > 0
            });

            let radius_sq = f64::from(cloud.radius * cloud.radius);
            let mut affected = false;

            // Check alive players.
            let candidate_players: Vec<i32> = self
                .players
                .iter()
                .filter(|(&p_id, p)| {
                    if !p.alive || cloud.reapplication_map.contains_key(&p_id) {
                        return false;
                    }
                    let dx = p.pos[0] - cloud.pos[0];
                    let dy = p.pos[1] - cloud.pos[1];
                    let dz = p.pos[2] - cloud.pos[2];
                    dx * dx + dz * dz <= radius_sq && (-1.0..=2.0).contains(&dy)
                })
                .map(|(&p_id, _)| p_id)
                .collect();

            for p_id in candidate_players {
                if cloud.is_dragon_breath {
                    self.damage(p_id, 3.0, Damage::Magic);
                } else if let Some(pot) = cloud.potion {
                    let effects = potions::effects(pot);
                    for effect in &effects {
                        let level = f32::from(1u8 << effect.amplifier.min(5));
                        if effect.id == mob_effect::INSTANT_HEALTH {
                            self.heal(p_id, (2.0 * level).round());
                        } else if effect.id == mob_effect::INSTANT_DAMAGE {
                            self.damage(p_id, (3.0 * level).round(), Damage::Magic);
                        } else {
                            let duration = (effect.duration / 4).max(20);
                            self.add_effect(
                                p_id,
                                Effect {
                                    duration,
                                    ..*effect
                                },
                            );
                        }
                    }
                }
                cloud
                    .reapplication_map
                    .insert(p_id, cloud.reapplication_delay);
                cloud.radius += cloud.radius_on_use;
                affected = true;
                if cloud.radius <= 0.0 {
                    break;
                }
            }

            if cloud.radius > 0.0 {
                // Check alive mobs.
                let candidate_mobs: Vec<(i32, bool)> = self
                    .mobs
                    .iter()
                    .filter(|(&m_id, m)| {
                        if !m.alive() || cloud.reapplication_map.contains_key(&m_id) {
                            return false;
                        }
                        let dx = m.pos[0] - cloud.pos[0];
                        let dy = m.pos[1] - cloud.pos[1];
                        let dz = m.pos[2] - cloud.pos[2];
                        dx * dx + dz * dz <= radius_sq && (-1.0..=2.0).contains(&dy)
                    })
                    .map(|(&m_id, m)| (m_id, m.kind.is_undead()))
                    .collect();

                for (m_id, is_undead) in candidate_mobs {
                    if cloud.is_dragon_breath {
                        self.hit_mob(m_id, 3.0, None, [0.0, 0.0], "minecraft:magic");
                    } else if let Some(pot) = cloud.potion {
                        let effects = potions::effects(pot);
                        for effect in &effects {
                            let level = f32::from(1u8 << effect.amplifier.min(5));
                            if effect.id == mob_effect::INSTANT_HEALTH {
                                if is_undead {
                                    self.hit_mob(
                                        m_id,
                                        (3.0 * level).round(),
                                        None,
                                        [0.0, 0.0],
                                        "minecraft:magic",
                                    );
                                } else {
                                    self.heal_mob(m_id, (2.0 * level).round());
                                }
                            } else if effect.id == mob_effect::INSTANT_DAMAGE {
                                if is_undead {
                                    self.heal_mob(m_id, (2.0 * level).round());
                                } else {
                                    self.hit_mob(
                                        m_id,
                                        (3.0 * level).round(),
                                        None,
                                        [0.0, 0.0],
                                        "minecraft:magic",
                                    );
                                }
                            } else {
                                let duration = (effect.duration / 4).max(20);
                                self.add_mob_effect(
                                    m_id,
                                    Effect {
                                        duration,
                                        ..*effect
                                    },
                                );
                            }
                        }
                    }
                    cloud
                        .reapplication_map
                        .insert(m_id, cloud.reapplication_delay);
                    cloud.radius += cloud.radius_on_use;
                    affected = true;
                    if cloud.radius <= 0.0 {
                        break;
                    }
                }
            }

            if cloud.radius <= 0.0 {
                self.broadcast(items::remove_entity(id), None);
                continue;
            }

            if affected {
                cloud.last_sent_radius = cloud.radius;
                self.broadcast(update_packet(id, cloud.radius, false), None);
            }

            self.area_effect_clouds.insert(id, cloud);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_packets_serialize_correctly() {
        let cloud =
            AreaEffectCloud::new_lingering([10.5, 64.0, -20.5], potions::id("swiftness"), None);
        let s_pkt = spawn_packet(42, &cloud).into_frame();
        assert!(!s_pkt.is_empty());

        let d_pkt = data_packet(42, &cloud).into_frame();
        assert!(!d_pkt.is_empty());

        let u_pkt = update_packet(42, 2.5, false).into_frame();
        assert!(!u_pkt.is_empty());
    }

    #[test]
    fn lingering_cloud_waits_then_pulses_scaled_effects_and_shrinks() {
        let dir = std::env::temp_dir().join(format!("levitas-cloud-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let (out, _inbox) = tokio::sync::mpsc::channel(16);
        let player_id = 1;
        let profile = Profile {
            uuid: player_id as u128,
            name: "Tester".into(),
            properties: Vec::new(),
        };
        w.players
            .insert(player_id, Player::new(profile, out, [0.0, -60.0, 0.0]));
        w.players.get_mut(&player_id).unwrap().health = 10.0;

        let potion_id = potions::id("healing");
        let cloud_id = w.spawn_lingering_cloud([0.0, -60.0, 0.0], potion_id, None);
        assert!(w.area_effect_clouds.contains_key(&cloud_id));

        // Advance 19 ticks: still in wait_time (grace period)
        for _ in 0..19 {
            w.tick_area_effect_clouds();
        }
        assert_eq!(w.players[&player_id].health, 10.0);
        let cloud = w.area_effect_clouds.get(&cloud_id).unwrap();
        assert_eq!(cloud.wait_time, 1);

        // 20th tick: wait_time expires and effect is applied!
        w.tick_area_effect_clouds();
        assert_eq!(w.players[&player_id].health, 12.0);
        let cloud = w.area_effect_clouds.get(&cloud_id).unwrap();
        assert_eq!(cloud.wait_time, 0);
        // Cloud radius shrank by 0.5 on use (from ~2.9 minus tick decay)
        assert!(cloud.radius <= 2.5 && cloud.radius >= 2.3);

        // Advancing 10 ticks should not re-apply due to reapplication delay
        for _ in 0..10 {
            w.tick_area_effect_clouds();
        }
        assert_eq!(w.players[&player_id].health, 12.0);

        // Advancing another 10 ticks (total 20 ticks): reapplication cooldown expires and heals again
        for _ in 0..10 {
            w.tick_area_effect_clouds();
        }
        assert_eq!(w.players[&player_id].health, 14.0);
    }

    #[test]
    fn dragon_breath_cloud_deals_magic_damage() {
        let dir = std::env::temp_dir().join(format!("levitas-cloud-dragon-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let (out, _inbox) = tokio::sync::mpsc::channel(16);
        let player_id = 2;
        let profile = Profile {
            uuid: player_id as u128,
            name: "DragonTarget".into(),
            properties: Vec::new(),
        };
        w.players
            .insert(player_id, Player::new(profile, out, [5.0, -60.0, 5.0]));
        w.players.get_mut(&player_id).unwrap().health = 20.0;

        let cloud_id = w.spawn_dragon_breath_cloud([5.0, -60.0, 5.0]);
        // Advance 19 ticks (still waiting)
        for _ in 0..19 {
            w.tick_area_effect_clouds();
        }
        assert_eq!(w.players[&player_id].health, 20.0);

        // 20th tick: wait expires and applies 3.0 magic damage
        w.tick_area_effect_clouds();
        assert_eq!(w.players[&player_id].health, 17.0);
        assert!(w.area_effect_clouds[&cloud_id].radius <= 2.5);
    }

    #[test]
    fn dragon_breath_can_be_bottled_with_glass_bottle() {
        let dir = std::env::temp_dir().join(format!("levitas-cloud-bottle-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let (out, _inbox) = tokio::sync::mpsc::channel(16);
        let player_id = 3;
        let profile = Profile {
            uuid: player_id as u128,
            name: "Bottler".into(),
            properties: Vec::new(),
        };
        w.players
            .insert(player_id, Player::new(profile, out, [0.0, -60.0, 0.0]));
        let bottle_id = ItemInfo::id_of("minecraft:glass_bottle").unwrap();
        w.players.get_mut(&player_id).unwrap().inventory.slots[36] = Stack::new(bottle_id, 3);

        let cloud_id = w.spawn_dragon_breath_cloud([0.5, -60.0, 0.5]);
        assert_eq!(w.area_effect_clouds[&cloud_id].radius, 3.0);

        // Player uses glass bottle on the dragon breath cloud
        let used = w.fill_bottle(player_id, 0);
        assert!(used);

        // Cloud radius shrank by 0.5
        assert_eq!(w.area_effect_clouds[&cloud_id].radius, 2.5);

        // Player has 2 glass bottles and 1 dragon's breath
        let dragon_breath_id = ItemInfo::id_of("minecraft:dragon_breath").unwrap();
        let p = &w.players[&player_id];
        assert_eq!(p.inventory.slots[36], Stack::new(bottle_id, 2));
        assert!(p
            .inventory
            .slots
            .iter()
            .any(|s| s.item == dragon_breath_id && s.count == 1));
    }
}
