//! Beacons, following the Minecraft Wiki: built atop pyramids of mineral blocks
//! (iron, gold, diamond, emerald, netherite), powered by feeding an ingot/gem, and pulsing
//! status effects (Speed, Haste, Resistance, Jump Boost, Strength, Regeneration) to
//! players within range every 4 seconds (80 ticks).

use std::collections::HashSet;

use levitas_nbt::{Compound, Tag};
use levitas_registry::packet::play::clientbound;
use levitas_registry::{block, block_entity_type, block_has_tag, mob_effect, BlockInfo, Effect};

use super::*;
use crate::world::containers::View;

pub const BEACON_PULSE_TICKS: u8 = 80;

#[derive(Debug, Clone, Default)]
pub struct Beacon {
    pub levels: i32,
    pub primary: Option<i32>,
    pub secondary: Option<i32>,
    pub custom_name: Option<&'static str>,
    pub lock: Option<String>,
    pub viewers: HashSet<i32>,
    pub tick_timer: u8,
}

impl Beacon {
    pub fn write_nbt(&self, nbt: &mut Compound) {
        nbt.insert("Levels", self.levels);
        if let Some(primary) = self.primary {
            nbt.insert("Primary", primary);
            if let Some(&name) = mob_effect::NAMES.get(primary as usize) {
                nbt.insert("primary_effect", name);
            }
        }
        if let Some(secondary) = self.secondary {
            nbt.insert("Secondary", secondary);
            if let Some(&name) = mob_effect::NAMES.get(secondary as usize) {
                nbt.insert("secondary_effect", name);
            }
        }
        if let Some(name) = self.custom_name {
            nbt.insert("CustomName", name);
        }
        if let Some(ref lock) = self.lock {
            nbt.insert("Lock", lock.as_str());
        }
    }

    pub fn read_nbt(&mut self, nbt: &Compound) {
        self.levels = nbt.int("Levels").unwrap_or(0) as i32;
        self.primary = nbt
            .str("primary_effect")
            .and_then(|name| mob_effect::NAMES.iter().position(|&n| n == name))
            .map(|i| i as i32)
            .or_else(|| nbt.int("Primary").map(|p| p as i32));
        self.secondary = nbt
            .str("secondary_effect")
            .and_then(|name| mob_effect::NAMES.iter().position(|&n| n == name))
            .map(|i| i as i32)
            .or_else(|| nbt.int("Secondary").map(|s| s as i32));
        self.custom_name = nbt.str("CustomName").and_then(crate::names::intern);
        self.lock = nbt.str("Lock").map(ToOwned::to_owned);
    }

    pub fn network_data(&self) -> Compound {
        let mut nbt = Compound::default();
        nbt.insert("Levels", self.levels);
        if let Some(primary) = self.primary {
            if let Some(&name) = mob_effect::NAMES.get(primary as usize) {
                nbt.insert("primary_effect", name);
            }
        }
        if let Some(secondary) = self.secondary {
            if let Some(&name) = mob_effect::NAMES.get(secondary as usize) {
                nbt.insert("secondary_effect", name);
            }
        }
        if let Some(name) = self.custom_name {
            nbt.insert("CustomName", name);
        }
        nbt
    }

    pub fn type_id(&self) -> i32 {
        block_entity_type::BEACON
    }
}

/// The beacon window properties: pyramid levels (0..4), primary effect ID, and secondary effect ID.
pub fn beacon_data(window: i32, b: &Beacon) -> Vec<Writer> {
    let primary = b.primary.unwrap_or(-1);
    let secondary = b.secondary.unwrap_or(-1);
    [b.levels, primary, secondary]
        .into_iter()
        .enumerate()
        .map(|(property, value)| {
            Writer::packet(clientbound::CONTAINER_SET_DATA)
                .varint(window)
                .i16(property as i16)
                .i16(value as i16)
        })
        .collect()
}

/// Checks whether a beacon has clear sky and calculates its pyramid levels (0 to 4).
pub fn check_pyramid(world: &World, pos: BlockPos) -> (i32, bool) {
    // 1. Check sky clearance: every block above must not block light completely.
    for y in (pos.y + 1)..320 {
        let state = world
            .loaded_block(BlockPos::new(pos.x, y, pos.z))
            .unwrap_or(block::AIR);
        let info = BlockInfo::of(state);
        if info.light_filter >= 15 {
            return (0, false);
        }
    }

    // 2. Check pyramid layers: 1 to 4 layers beneath the beacon.
    let mut levels = 0;
    for l in 1..=4 {
        let dy = pos.y - l;
        if dy < -64 {
            break;
        }
        let mut layer_intact = true;
        for dx in -l..=l {
            for dz in -l..=l {
                let p = BlockPos::new(pos.x + dx, dy, pos.z + dz);
                let state = world.loaded_block(p).unwrap_or(block::AIR);
                if !block_has_tag(state, "minecraft:beacon_base_blocks") {
                    layer_intact = false;
                    break;
                }
            }
            if !layer_intact {
                break;
            }
        }
        if layer_intact {
            levels = l;
        } else {
            break;
        }
    }
    (levels, true)
}

pub fn is_valid_primary(level: i32, effect: i32) -> bool {
    match effect {
        mob_effect::SPEED | mob_effect::HASTE if level >= 1 => true,
        mob_effect::RESISTANCE | mob_effect::JUMP_BOOST if level >= 2 => true,
        mob_effect::STRENGTH if level >= 3 => true,
        _ => false,
    }
}

pub fn is_valid_secondary(level: i32, primary: Option<i32>, secondary: i32) -> bool {
    if level < 4 {
        return false;
    }
    secondary == mob_effect::REGENERATION || Some(secondary) == primary
}

impl World {
    /// Pulses beacon effects to players within range every 80 ticks (4 seconds).
    pub(super) fn tick_beacons(&mut self) {
        let beacon_positions: Vec<BlockPos> = self
            .block_entities
            .iter()
            .filter_map(|(&pos, entity)| match entity {
                BlockEntity::Beacon(_) => Some(pos),
                _ => None,
            })
            .collect();

        for pos in beacon_positions {
            let mut should_pulse = false;
            let mut level_changed = false;
            let mut became_active = false;
            let mut became_inactive = false;

            if let Some(BlockEntity::Beacon(b)) = self.block_entities.get_mut(&pos) {
                b.tick_timer = (b.tick_timer + 1) % BEACON_PULSE_TICKS;
                if b.tick_timer == 0 {
                    should_pulse = true;
                }
            }

            if should_pulse {
                let (new_levels, _clear_sky) = check_pyramid(self, pos);
                let mut notify_viewers = false;
                if let Some(BlockEntity::Beacon(b)) = self.block_entities.get_mut(&pos) {
                    if new_levels != b.levels {
                        level_changed = true;
                        became_active = b.levels == 0 && new_levels > 0;
                        became_inactive = b.levels > 0 && new_levels == 0;
                        b.levels = new_levels;
                        notify_viewers = true;
                    }
                }
                if level_changed {
                    self.send_beacon_update(pos);
                    self.dirty.insert(pos.chunk());
                }
                if notify_viewers {
                    self.send_beacon_data(pos);
                }
                if became_active {
                    self.play_sound("minecraft:block.beacon.activate", pos, None);
                } else if became_inactive {
                    self.play_sound("minecraft:block.beacon.deactivate", pos, None);
                }

                // If active and primary effect is chosen, apply effect to nearby players.
                let mut pulse_effect: Option<(i32, u8, i32, f64, Option<i32>)> = None;
                if let Some(BlockEntity::Beacon(b)) = self.block_entities.get(&pos) {
                    if let (true, Some(primary)) = (b.levels > 0, b.primary) {
                        let primary_amp = if b.levels >= 4 && b.secondary == Some(primary) {
                            1
                        } else {
                            0
                        };
                        let duration = (9 + b.levels * 2) * 20;
                        let range = (b.levels * 10 + 10) as f64;
                        let secondary_regen =
                            if b.levels >= 4 && b.secondary == Some(mob_effect::REGENERATION) {
                                Some(mob_effect::REGENERATION)
                            } else {
                                None
                            };
                        pulse_effect =
                            Some((primary, primary_amp, duration, range, secondary_regen));
                    }
                }

                if let Some((primary, primary_amp, duration, range, secondary_regen)) = pulse_effect
                {
                    let near_players: Vec<i32> = self
                        .players
                        .iter()
                        .filter(|(_, p)| p.alive)
                        .filter_map(|(&id, p)| {
                            let dx = (p.pos[0] - (pos.x as f64 + 0.5)).abs();
                            let dz = (p.pos[2] - (pos.z as f64 + 0.5)).abs();
                            let y = p.pos[1];
                            if dx <= range
                                && dz <= range
                                && y >= (pos.y as f64 - range)
                                && y <= 320.0
                            {
                                Some(id)
                            } else {
                                None
                            }
                        })
                        .collect();

                    for pid in near_players {
                        self.add_effect(
                            pid,
                            Effect {
                                id: primary,
                                amplifier: primary_amp,
                                duration,
                            },
                        );
                        if let Some(regen) = secondary_regen {
                            self.add_effect(
                                pid,
                                Effect {
                                    id: regen,
                                    amplifier: 0,
                                    duration,
                                },
                            );
                        }
                    }
                    self.play_sound("minecraft:block.beacon.ambient", pos, None);
                }
            }
        }
    }

    /// Broadcasts the updated beacon block entity data packet to show/hide the beam.
    pub(super) fn send_beacon_update(&mut self, pos: BlockPos) {
        let Some(BlockEntity::Beacon(b)) = self.block_entities.get(&pos) else {
            return;
        };
        let data = levitas_nbt::to_network(&Tag::Compound(b.network_data()));
        self.broadcast(
            Writer::packet(clientbound::BLOCK_ENTITY_DATA)
                .position(pos)
                .varint(block_entity_type::BEACON)
                .bytes(&data),
            None,
        );
    }

    /// Sends the updated beacon levels and powers to anyone with the beacon window open.
    pub(super) fn send_beacon_data(&mut self, pos: BlockPos) {
        let Some(BlockEntity::Beacon(b)) = self.block_entities.get(&pos) else {
            return;
        };
        let viewers: Vec<i32> = b.viewers.iter().copied().collect();
        for viewer in viewers {
            let window = self
                .players
                .get(&viewer)
                .and_then(|p| p.open.as_ref())
                .map(|o| o.window);
            let (Some(window), Some(BlockEntity::Beacon(b))) =
                (window, self.block_entities.get(&pos))
            else {
                continue;
            };
            for packet in beacon_data(window, b) {
                self.send(viewer, packet);
            }
        }
    }

    /// Handles a player clicking "Done" in a beacon interface.
    pub(super) fn set_beacon(&mut self, id: i32, primary: Option<i32>, secondary: Option<i32>) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let Some(open) = p.open.as_mut() else {
            return;
        };
        let View::Beacon(pos) = open.view else {
            return;
        };
        let payment = open.grid[0];
        if !crate::click::is_beacon_payment(&payment) {
            return;
        }

        let Some(BlockEntity::Beacon(b)) = self.block_entities.get(&pos) else {
            return;
        };
        let levels = b.levels;

        if let Some(p) = primary {
            if !is_valid_primary(levels, p) {
                return;
            }
        }
        if let Some(s) = secondary {
            if !is_valid_secondary(levels, primary, s) {
                return;
            }
        }

        // Consume 1 payment item from transient slot 0.
        open.grid[0] = payment.with_count(payment.count - 1);
        let window = open.window;
        let updated_slot = open.grid[0];

        // Update beacon data.
        if let Some(BlockEntity::Beacon(b)) = self.block_entities.get_mut(&pos) {
            b.primary = primary;
            b.secondary = secondary;
        }

        self.dirty.insert(pos.chunk());
        self.play_sound("minecraft:block.beacon.power_select", pos, None);
        self.send_beacon_data(pos);
        self.send_beacon_update(pos);

        // Tell the player the payment slot has been consumed.
        if let Some(p) = self.players.get_mut(&id) {
            let packet = p.inventory.window_slot_packet(window, 0, updated_slot);
            self.send(id, packet);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn beacon_pyramid_and_sky_clearance() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-beacon-pyramid-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut world, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        world.chunk(0, 0);

        let pos = BlockPos::new(0, -40, 0);
        let beacon = BlockInfo::by_name("minecraft:beacon")
            .unwrap()
            .default_state;
        world.set_block(pos, beacon);
        world
            .block_entities
            .insert(pos, BlockEntity::for_block(beacon).unwrap());

        // Level 0 initially.
        let (level, clear) = check_pyramid(&world, pos);
        assert_eq!(level, 0);
        assert!(clear);

        // Build layer 1 (3x3 iron blocks at y = -41).
        let iron = BlockInfo::by_name("minecraft:iron_block")
            .unwrap()
            .default_state;
        for dx in -1..=1 {
            for dz in -1..=1 {
                world.set_block(BlockPos::new(dx, -41, dz), iron);
            }
        }
        let (level, clear) = check_pyramid(&world, pos);
        assert_eq!(level, 1);
        assert!(clear);

        // Build layer 2 (5x5 gold blocks at y = -42).
        let gold = BlockInfo::by_name("minecraft:gold_block")
            .unwrap()
            .default_state;
        for dx in -2..=2 {
            for dz in -2..=2 {
                world.set_block(BlockPos::new(dx, -42, dz), gold);
            }
        }
        let (level, clear) = check_pyramid(&world, pos);
        assert_eq!(level, 2);
        assert!(clear);

        // Obstruct sky with stone above beacon.
        let stone = BlockInfo::by_name("minecraft:stone").unwrap().default_state;
        world.set_block(BlockPos::new(0, -30, 0), stone);
        let (level, clear) = check_pyramid(&world, pos);
        assert_eq!(level, 0);
        assert!(!clear);

        // Replace stone with glass (does not obstruct light).
        let glass = BlockInfo::by_name("minecraft:glass").unwrap().default_state;
        world.set_block(BlockPos::new(0, -30, 0), glass);
        let (level, clear) = check_pyramid(&world, pos);
        assert_eq!(level, 2);
        assert!(clear);
    }

    #[test]
    fn beacon_nbt_and_network_round_trip() {
        let b = Beacon {
            levels: 4,
            primary: Some(mob_effect::SPEED),
            secondary: Some(mob_effect::REGENERATION),
            custom_name: Some("Super Beacon"),
            lock: Some("SecretKey".to_owned()),
            viewers: HashSet::new(),
            tick_timer: 0,
        };

        let mut nbt = Compound::default();
        b.write_nbt(&mut nbt);
        assert_eq!(nbt.int("Levels"), Some(4));
        assert_eq!(nbt.str("primary_effect"), Some("minecraft:speed"));
        assert_eq!(nbt.str("secondary_effect"), Some("minecraft:regeneration"));
        assert_eq!(nbt.str("CustomName"), Some("Super Beacon"));
        assert_eq!(nbt.str("Lock"), Some("SecretKey"));

        let mut restored = Beacon::default();
        restored.read_nbt(&nbt);
        assert_eq!(restored.levels, 4);
        assert_eq!(restored.primary, Some(mob_effect::SPEED));
        assert_eq!(restored.secondary, Some(mob_effect::REGENERATION));
        assert_eq!(restored.custom_name, Some("Super Beacon"));
        assert_eq!(restored.lock.as_deref(), Some("SecretKey"));

        let net = restored.network_data();
        assert_eq!(net.int("Levels"), Some(4));
        assert_eq!(net.str("primary_effect"), Some("minecraft:speed"));
        assert_eq!(net.str("secondary_effect"), Some("minecraft:regeneration"));
    }

    #[test]
    fn beacon_effect_selection_and_pulses_effects() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-beacon-pulse-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut world, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        world.chunk(0, 0);

        let (out1, _frames1) = mpsc::channel(100_000);
        let id1 = next_entity_id();
        let p1 = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        world
            .players
            .insert(id1, Player::new(p1, out1, [5.0, -40.0, 5.0]));

        let (out2, _frames2) = mpsc::channel(100_000);
        let id2 = next_entity_id();
        let p2 = Profile {
            name: "Bob".to_owned(),
            uuid: 2,
            properties: Vec::new(),
        };
        world
            .players
            .insert(id2, Player::new(p2, out2, [100.0, -40.0, 100.0]));

        let pos = BlockPos::new(0, -40, 0);
        let beacon_state = BlockInfo::by_name("minecraft:beacon")
            .unwrap()
            .default_state;
        world.set_block(pos, beacon_state);
        world.block_entities.insert(
            pos,
            BlockEntity::Beacon(Box::new(Beacon {
                levels: 4,
                primary: None,
                secondary: None,
                custom_name: None,
                lock: None,
                viewers: HashSet::new(),
                tick_timer: 0,
            })),
        );

        // Build 4 layers under the beacon so check_pyramid keeps it at level 4.
        let iron = BlockInfo::by_name("minecraft:iron_block")
            .unwrap()
            .default_state;
        for l in 1..=4 {
            for dx in -l..=l {
                for dz in -l..=l {
                    world.set_block(BlockPos::new(dx, -40 - l, dz), iron);
                }
            }
        }

        // Alice opens beacon.
        assert!(world.open_container(id1, pos, beacon_state));

        // Put an emerald in slot 0.
        let emerald_id = ItemInfo::id_of("minecraft:emerald").unwrap();
        world
            .players
            .get_mut(&id1)
            .unwrap()
            .open
            .as_mut()
            .unwrap()
            .grid[0] = Stack::new(emerald_id, 1);

        // Select Speed as primary and Speed as secondary (Speed II).
        world.set_beacon(id1, Some(mob_effect::SPEED), Some(mob_effect::SPEED));

        // Payment consumed!
        let open_slot = world.players[&id1].open.as_ref().unwrap().grid[0];
        assert_eq!(open_slot.count, 0);

        // Beacon now has Speed configured.
        if let Some(BlockEntity::Beacon(b)) = world.block_entities.get(&pos) {
            assert_eq!(b.primary, Some(mob_effect::SPEED));
            assert_eq!(b.secondary, Some(mob_effect::SPEED));
        } else {
            panic!("beacon entity missing");
        }

        // Tick beacon to pulse effects.
        if let Some(BlockEntity::Beacon(b)) = world.block_entities.get_mut(&pos) {
            b.tick_timer = BEACON_PULSE_TICKS - 1;
        }
        world.tick_beacons();

        // Alice (within 50 blocks) receives Speed II (amplifier 1).
        let alice_speed = world.players[&id1].effect(mob_effect::SPEED);
        assert!(alice_speed.is_some());
        assert_eq!(alice_speed.unwrap().amplifier, 1);

        // Bob (at 100 blocks, beyond range) has no speed effect.
        assert!(world.players[&id2].effect(mob_effect::SPEED).is_none());
    }
}
