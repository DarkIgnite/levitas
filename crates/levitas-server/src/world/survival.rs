//! Survival rules: health, hunger, damage, death and respawning, and which blocks break
//! instantly or need the right tool to drop anything.

use levitas_registry::BlockInfo;

use super::*;

pub const MAX_HEALTH: f32 = 20.0;
/// Eating takes 1.6 seconds.
const EAT_TICKS: u64 = 32;
const ENTITY_EVENT_DEATH: i8 = 3;
const ENTITY_EVENT_USE_ITEM_COMPLETE: i8 = 9;
/// Below this height players take void damage.
const VOID_Y: f64 = (MIN_Y - 64) as f64;

/// Blocks with zero hardness, which break as soon as a survival player starts digging.
/// Hardness is not part of the data generator output, so the list is kept by hand.
const INSTANT_BREAK: &[&str] = &[
    "minecraft:short_grass",
    "minecraft:tall_grass",
    "minecraft:fern",
    "minecraft:large_fern",
    "minecraft:dead_bush",
    "minecraft:bush",
    "minecraft:short_dry_grass",
    "minecraft:tall_dry_grass",
    "minecraft:firefly_bush",
    "minecraft:seagrass",
    "minecraft:tall_seagrass",
    "minecraft:kelp",
    "minecraft:kelp_plant",
    "minecraft:sea_pickle",
    "minecraft:dandelion",
    "minecraft:poppy",
    "minecraft:blue_orchid",
    "minecraft:allium",
    "minecraft:azure_bluet",
    "minecraft:red_tulip",
    "minecraft:orange_tulip",
    "minecraft:white_tulip",
    "minecraft:pink_tulip",
    "minecraft:oxeye_daisy",
    "minecraft:cornflower",
    "minecraft:lily_of_the_valley",
    "minecraft:wither_rose",
    "minecraft:torchflower",
    "minecraft:closed_eyeblossom",
    "minecraft:open_eyeblossom",
    "minecraft:sunflower",
    "minecraft:lilac",
    "minecraft:rose_bush",
    "minecraft:peony",
    "minecraft:pitcher_plant",
    "minecraft:pink_petals",
    "minecraft:wildflowers",
    "minecraft:spore_blossom",
    "minecraft:oak_sapling",
    "minecraft:spruce_sapling",
    "minecraft:birch_sapling",
    "minecraft:jungle_sapling",
    "minecraft:acacia_sapling",
    "minecraft:dark_oak_sapling",
    "minecraft:cherry_sapling",
    "minecraft:pale_oak_sapling",
    "minecraft:mangrove_propagule",
    "minecraft:brown_mushroom",
    "minecraft:red_mushroom",
    "minecraft:crimson_fungus",
    "minecraft:warped_fungus",
    "minecraft:crimson_roots",
    "minecraft:warped_roots",
    "minecraft:nether_sprouts",
    "minecraft:weeping_vines",
    "minecraft:weeping_vines_plant",
    "minecraft:twisting_vines",
    "minecraft:twisting_vines_plant",
    "minecraft:hanging_roots",
    "minecraft:small_dripleaf",
    "minecraft:sugar_cane",
    "minecraft:lily_pad",
    "minecraft:torch",
    "minecraft:wall_torch",
    "minecraft:soul_torch",
    "minecraft:soul_wall_torch",
    "minecraft:redstone_torch",
    "minecraft:redstone_wall_torch",
    "minecraft:redstone_wire",
    "minecraft:repeater",
    "minecraft:comparator",
    "minecraft:tripwire",
    "minecraft:tripwire_hook",
    "minecraft:wheat",
    "minecraft:carrots",
    "minecraft:potatoes",
    "minecraft:beetroots",
    "minecraft:melon_stem",
    "minecraft:attached_melon_stem",
    "minecraft:pumpkin_stem",
    "minecraft:attached_pumpkin_stem",
    "minecraft:nether_wart",
    "minecraft:sweet_berry_bush",
    "minecraft:torchflower_crop",
    "minecraft:pitcher_crop",
    "minecraft:flower_pot",
    "minecraft:tnt",
    "minecraft:slime_block",
    "minecraft:honey_block",
    "minecraft:scaffolding",
    "minecraft:frogspawn",
    "minecraft:end_rod",
    "minecraft:decorated_pot",
    "minecraft:fire",
    "minecraft:soul_fire",
];

/// Pickaxe-mineable blocks that still drop when broken by hand.
const NO_TOOL_NEEDED: &[&str] = &[
    "minecraft:rail",
    "minecraft:powered_rail",
    "minecraft:detector_rail",
    "minecraft:activator_rail",
    "minecraft:pointed_dripstone",
    "minecraft:piston",
    "minecraft:sticky_piston",
    "minecraft:piston_head",
    "minecraft:conduit",
];

pub fn breaks_instantly(state: u16) -> bool {
    let name = BlockInfo::of(state).name;
    INSTANT_BREAK.contains(&name) || name.starts_with("minecraft:potted_")
}

/// Whether breaking `state` while holding `tool` drops anything. Vanilla flags each
/// block that needs the correct tool; that flag is not in the generated data, so this
/// treats pickaxe blocks (with a few exceptions), snow and cobwebs as needing one.
pub fn drops_with_tool(state: u16, tool: i32) -> bool {
    let name = BlockInfo::of(state).name;
    let needs_tool = (block_has_tag(state, "minecraft:mineable/pickaxe")
        && !NO_TOOL_NEEDED.contains(&name))
        || matches!(
            name,
            "minecraft:snow" | "minecraft:snow_block" | "minecraft:cobweb"
        );
    !needs_tool || ItemInfo::get(tool).and_then(|t| t.correct_for_drops(state)) == Some(true)
}

#[derive(Debug, Clone, Copy)]
pub enum Damage {
    Fall,
    Void,
    Starve,
    Kill,
}

impl Damage {
    fn type_name(self) -> &'static str {
        match self {
            Damage::Fall => "minecraft:fall",
            Damage::Void => "minecraft:out_of_world",
            Damage::Starve => "minecraft:starve",
            Damage::Kill => "minecraft:generic_kill",
        }
    }

    fn death_message(self, name: &str) -> String {
        match self {
            Damage::Fall => format!("{name} hit the ground too hard"),
            Damage::Void => format!("{name} fell out of the world"),
            Damage::Starve => format!("{name} starved to death"),
            Damage::Kill => format!("{name} was killed"),
        }
    }

    /// Whether this damage also hurts creative players.
    fn bypasses_creative(self) -> bool {
        matches!(self, Damage::Void | Damage::Kill)
    }
}

pub fn health_packet(p: &Player) -> Writer {
    Writer::packet(clientbound::SET_HEALTH)
        .f32(p.health)
        .varint(p.food)
        .f32(p.saturation)
}

impl World {
    pub(super) fn damage(&mut self, id: i32, amount: f32, kind: Damage) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if !p.alive || (!p.survival() && !kind.bypasses_creative()) {
            return;
        }
        p.health = (p.health - amount).max(0.0);
        p.exhaustion += 0.1;
        let dead = p.health <= 0.0;
        let health = health_packet(p);
        p.health_dirty = false;

        let damage_type = SyncedRegistry::get("minecraft:damage_type")
            .and_then(|r| r.id_of(kind.type_name()))
            .unwrap_or(0);
        self.broadcast(
            Writer::packet(clientbound::DAMAGE_EVENT)
                .varint(id)
                .varint(damage_type)
                .varint(0) // no cause entity
                .varint(0) // no direct entity
                .bool(false), // no source position
            None,
        );
        self.send(id, health);
        if dead {
            self.die(id, kind);
        }
    }

    fn die(&mut self, id: i32, kind: Damage) {
        self.close_container(id);
        let p = self.players.get_mut(&id).expect("player exists");
        p.alive = false;
        p.eating = None;
        p.fall_distance = 0.0;
        let message = kind.death_message(&p.profile.name);
        let pos = p.pos;
        let dropped = p.inventory.drain();
        let content = p.inventory.content_packet();

        self.send(id, content);
        self.send(
            id,
            Writer::packet(clientbound::PLAYER_COMBAT_KILL)
                .varint(id)
                .text(&message),
        );
        self.broadcast(entity_event(id, ENTITY_EVENT_DEATH), Some(id));
        self.broadcast(system_chat(&message), None);
        info!("{message}");
        for stack in dropped {
            self.scatter_item(pos, stack);
        }
    }

    pub(super) fn respawn(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if p.alive {
            return;
        }
        p.alive = true;
        p.health = MAX_HEALTH;
        p.food = 20;
        p.saturation = 5.0;
        p.exhaustion = 0.0;
        p.food_timer = 0;
        p.pos = SPAWN;
        p.sent_pos = SPAWN;
        p.yaw = 0.0;
        p.pitch = 0.0;
        p.moved = false;
        p.center = (0, 0);
        // The client throws away its chunks and entities on respawn.
        p.loaded.clear();
        let mode = p.mode;

        let respawn = spawn_info(Writer::packet(clientbound::RESPAWN), mode).u8(0); // keep no data
        self.send(id, respawn);
        self.send(
            id,
            Writer::packet(clientbound::GAME_EVENT)
                .u8(GAME_EVENT_START_WAITING_FOR_CHUNKS)
                .f32(0.0),
        );
        self.update_view(id);
        self.send(id, position_packet(SPAWN));
        self.send_status(id);
        self.send_entities_to(id);

        self.broadcast(
            Writer::packet(clientbound::REMOVE_ENTITIES)
                .varint(1)
                .varint(id),
            Some(id),
        );
        let spawn = spawn_player(id, &self.players[&id]);
        for packet in spawn {
            self.broadcast(packet, Some(id));
        }
    }

    pub(super) fn use_item(&mut self, id: i32, hand: i32, sequence: i32) {
        self.ack(id, sequence);
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let stack = p.inventory.slots[p.hand_slot(hand)];
        let Some(food) = ItemInfo::get(stack.item).and_then(|i| i.food.as_ref()) else {
            return;
        };
        if p.alive && !stack.is_empty() && (p.food < 20 || food.can_always_eat) {
            p.eating = Some((hand, tick));
        }
    }

    fn finish_eating(&mut self, id: i32, hand: i32) {
        let p = self.players.get_mut(&id).expect("player exists");
        let slot = p.hand_slot(hand);
        let stack = p.inventory.slots[slot];
        let Some(food) = ItemInfo::get(stack.item).and_then(|i| i.food.as_ref()) else {
            return;
        };
        if stack.is_empty() {
            return;
        }
        p.food = (p.food + food.nutrition).min(20);
        p.saturation = (p.saturation + food.saturation).min(p.food as f32);
        p.health_dirty = true;
        let update = p.survival().then(|| {
            p.inventory.take(slot, 1);
            p.inventory.set_slot_packet(slot)
        });
        if let Some(update) = update {
            self.send(id, update);
        }
        self.send(id, entity_event(id, ENTITY_EVENT_USE_ITEM_COMPLETE));
    }

    /// Hunger, natural regeneration, starvation, void damage and eating, once per tick.
    pub(super) fn tick_survival(&mut self) {
        let tick = self.tick;
        let mut damage = Vec::new();
        let mut eaten = Vec::new();
        for (&id, p) in self.players.iter_mut().filter(|(_, p)| p.alive) {
            if p.pos[1] < VOID_Y && tick % 10 == 0 {
                damage.push((id, 4.0, Damage::Void));
            }
            if let Some((hand, start)) = p.eating {
                if tick - start >= EAT_TICKS {
                    p.eating = None;
                    eaten.push((id, hand));
                }
            }
            if !p.survival() {
                continue;
            }
            if p.exhaustion > 4.0 {
                p.exhaustion -= 4.0;
                if p.saturation > 0.0 {
                    p.saturation = (p.saturation - 1.0).max(0.0);
                } else {
                    p.food = (p.food - 1).max(0);
                }
                p.health_dirty = true;
            }
            p.food_timer += 1;
            let hurt = p.health < MAX_HEALTH;
            if hurt && p.food >= 20 && p.saturation > 0.0 {
                // Fast regeneration from saturation.
                if p.food_timer >= 10 {
                    let spent = p.saturation.min(6.0);
                    p.health = (p.health + spent / 6.0).min(MAX_HEALTH);
                    p.exhaustion += spent;
                    p.food_timer = 0;
                    p.health_dirty = true;
                }
            } else if hurt && p.food >= 18 {
                if p.food_timer >= 80 {
                    p.health = (p.health + 1.0).min(MAX_HEALTH);
                    p.exhaustion += 6.0;
                    p.food_timer = 0;
                    p.health_dirty = true;
                }
            } else if p.food <= 0 {
                // Normal difficulty: starving stops at half a heart.
                if p.food_timer >= 80 {
                    if p.health > 1.0 {
                        damage.push((id, 1.0, Damage::Starve));
                    }
                    p.food_timer = 0;
                }
            } else {
                p.food_timer = 0;
            }
        }
        for (id, hand) in eaten {
            self.finish_eating(id, hand);
        }
        for (id, amount, kind) in damage {
            self.damage(id, amount, kind);
        }
        let dirty: Vec<(i32, Writer)> = self
            .players
            .iter_mut()
            .filter(|(_, p)| p.health_dirty)
            .map(|(&id, p)| {
                p.health_dirty = false;
                (id, health_packet(p))
            })
            .collect();
        for (id, packet) in dirty {
            self.send(id, packet);
        }
    }
}
