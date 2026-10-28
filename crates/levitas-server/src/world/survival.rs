//! Survival rules: health, hunger, damage, death and respawning, and which blocks break
//! instantly or need the right tool to drop anything.

use levitas_registry::{mob_effect, BlockInfo, OnConsume};

use super::*;

pub const MAX_HEALTH: f32 = 20.0;
/// Ticks of invulnerability after a hit.
const HURT_TICKS: u32 = 10;
/// Eating takes 1.6 seconds.
const EAT_TICKS: u64 = 32;
const ENTITY_EVENT_DEATH: i8 = 3;
const ENTITY_EVENT_USE_ITEM_COMPLETE: i8 = 9;
/// Below this height players take void damage.
const VOID_Y: f64 = (MIN_Y - 64) as f64;

/// How much of `state` a survival player breaks per tick with `tool` in hand (vanilla's
/// destroy progress): 1 or more breaks it at once, 0 means it cannot be broken.
#[cfg(test)]
pub fn break_progress(state: u16, tool: i32) -> f32 {
    break_progress_with(state, tool, 0)
}

/// [`break_progress`] with an efficiency level on the tool, which adds level squared
/// plus one to the speed of a tool that suits the block.
pub fn break_progress_with(state: u16, tool: i32, efficiency: u32) -> f32 {
    let info = BlockInfo::of(state);
    if info.hardness < 0.0 {
        return 0.0;
    }
    if info.hardness == 0.0 {
        return 1.0;
    }
    let mut speed = ItemInfo::get(tool).map_or(1.0, |i| i.mining_speed(state));
    if efficiency > 0 && speed > 1.0 {
        speed += (efficiency * efficiency + 1) as f32;
    }
    let divisor = if harvestable(state, tool) {
        30.0
    } else {
        100.0
    };
    speed / info.hardness / divisor
}

/// Whether breaking `state` while holding `tool` drops anything: blocks that require a
/// tool need a correct one.
pub fn harvestable(state: u16, tool: i32) -> bool {
    !BlockInfo::of(state).requires_tool
        || ItemInfo::get(tool).and_then(|t| t.correct_for_drops(state)) == Some(true)
}

#[derive(Debug, Clone, Copy)]
pub enum Damage {
    Fall,
    Void,
    Starve,
    Kill,
    FallingAnvil,
    #[allow(dead_code)]
    FallingBlock,
    FallingStalactite,
    /// Hit by a mob, named as in death messages ("Zombie").
    Mob(&'static str),
    /// Caught in an explosion caused by what is named.
    Explosion(&'static str),
    /// Shot by an arrow from what is named.
    Arrow(&'static str),
    Drown,
    Lava,
    InFire,
    OnFire,
    HotFloor,
    /// Hit by another player (their entity id).
    Player(i32),
    /// Standing on a lit campfire.
    Campfire,
    /// Poison and instant damage.
    Magic,
    Wither,
}

impl Damage {
    fn type_name(self) -> &'static str {
        match self {
            Damage::Fall => "minecraft:fall",
            Damage::Void => "minecraft:out_of_world",
            Damage::Starve => "minecraft:starve",
            Damage::Kill => "minecraft:generic_kill",
            Damage::FallingAnvil => "minecraft:falling_anvil",
            Damage::FallingBlock => "minecraft:falling_block",
            Damage::FallingStalactite => "minecraft:falling_stalactite",
            Damage::Mob(_) => "minecraft:mob_attack",
            Damage::Explosion(_) => "minecraft:explosion",
            Damage::Arrow(_) => "minecraft:arrow",
            Damage::Drown => "minecraft:drown",
            Damage::Lava => "minecraft:lava",
            Damage::InFire => "minecraft:in_fire",
            Damage::OnFire => "minecraft:on_fire",
            Damage::HotFloor => "minecraft:hot_floor",
            Damage::Player(_) => "minecraft:player_attack",
            Damage::Campfire => "minecraft:campfire",
            Damage::Magic => "minecraft:magic",
            Damage::Wither => "minecraft:wither",
        }
    }

    fn death_message(self, name: &str) -> String {
        match self {
            Damage::Fall => format!("{name} hit the ground too hard"),
            Damage::Void => format!("{name} fell out of the world"),
            Damage::Starve => format!("{name} starved to death"),
            Damage::Kill => format!("{name} was killed"),
            Damage::FallingAnvil => format!("{name} was squashed by a falling anvil"),
            Damage::FallingBlock => format!("{name} was squashed by a falling block"),
            Damage::FallingStalactite => format!("{name} was skewered by a falling stalactite"),
            Damage::Mob(mob) => format!("{name} was slain by {mob}"),
            Damage::Explosion(by) => format!("{name} was blown up by {by}"),
            Damage::Arrow(by) => format!("{name} was shot by {by}"),
            Damage::Drown => format!("{name} drowned"),
            Damage::Lava => format!("{name} tried to swim in lava"),
            Damage::InFire => format!("{name} went up in flames"),
            Damage::OnFire => format!("{name} burned to death"),
            Damage::HotFloor => format!("{name} discovered the floor was lava"),
            Damage::Player(_) => format!("{name} was slain by a player"),
            Damage::Campfire => format!("{name} went up in flames"),
            Damage::Magic => format!("{name} was killed by magic"),
            Damage::Wither => format!("{name} withered away"),
        }
    }

    /// Whether worn armor softens this damage.
    fn reduced_by_armor(self) -> bool {
        // The data generator's `bypasses_armor` tag leaves these to armor.
        matches!(
            self,
            Damage::Mob(_)
                | Damage::Player(_)
                | Damage::Explosion(_)
                | Damage::Arrow(_)
                | Damage::Lava
                | Damage::InFire
                | Damage::HotFloor
                | Damage::Campfire
        )
    }

    /// Whether fire resistance keeps this damage away.
    fn is_fire(self) -> bool {
        matches!(
            self,
            Damage::Lava | Damage::InFire | Damage::OnFire | Damage::HotFloor | Damage::Campfire
        )
    }

    /// Whether this damage also hurts creative players.
    fn bypasses_creative(self) -> bool {
        matches!(self, Damage::Void | Damage::Kill)
    }
}

/// Damage left after armor: each effective armor point takes off 4%, and hard hits
/// lower the effective points unless the armor is tough.
pub fn after_armor(amount: f32, armor: f32, toughness: f32) -> f32 {
    let effective = (armor / 5.0)
        .max(armor - amount / (2.0 + toughness / 4.0))
        .min(20.0);
    amount * (1.0 - effective / 25.0)
}

#[cfg(test)]
mod armor_tests {
    use super::after_armor;

    #[test]
    fn armor_softens_hits() {
        assert_eq!(after_armor(5.0, 0.0, 0.0), 5.0);
        // Full diamond (20 armor, 8 toughness) against a zombie's 3.
        let hit = after_armor(3.0, 20.0, 8.0);
        assert!((hit - 0.69).abs() < 0.01, "{hit}");
        // Big hits get through more, unless the armor is tough.
        assert!(after_armor(30.0, 20.0, 0.0) > after_armor(30.0, 20.0, 12.0));
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
        self.damage_from(id, amount, kind, None);
    }

    /// Hurts a player; `source` is the entity that caused it, if any. Returns whether
    /// the hit got through.
    pub(super) fn damage_from(
        &mut self,
        id: i32,
        amount: f32,
        kind: Damage,
        source: Option<i32>,
    ) -> bool {
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        if !p.alive || (!p.survival() && !kind.bypasses_creative()) {
            return false;
        }
        if kind.is_fire() && p.effect(mob_effect::FIRE_RESISTANCE).is_some() {
            return false;
        }
        // Just after a hit, only a harder one gets through, and only by the difference.
        let amount = if p.hurt_ticks > 0 && !kind.bypasses_creative() {
            let extra = amount - p.last_damage;
            if extra <= 0.0 {
                return false;
            }
            p.last_damage = amount;
            extra
        } else {
            p.last_damage = amount;
            p.hurt_ticks = HURT_TICKS;
            amount
        };
        let amount = if kind.reduced_by_armor() {
            self.through_armor(id, amount)
        } else {
            amount
        };
        let amount = self.through_protection(id, amount, kind);
        let p = self.players.get_mut(&id).expect("player exists");
        // Resistance takes off a fifth per level; absorption hearts go first.
        let amount = if kind.bypasses_creative() {
            amount
        } else {
            let level = p.effect_level(mob_effect::RESISTANCE) as f32;
            amount * (1.0 - 0.2 * level).max(0.0)
        };
        let absorbed = amount.min(p.absorption);
        p.absorption -= absorbed;
        let amount = amount - absorbed;
        p.health = (p.health - amount).max(0.0);
        p.exhaustion += 0.1;
        let dead = p.health <= 0.0;
        let health = health_packet(p);
        p.health_dirty = false;

        let damage_type = SyncedRegistry::get("minecraft:damage_type")
            .and_then(|r| r.id_of(kind.type_name()))
            .unwrap_or(0);
        let cause = source.map_or(0, |s| s + 1);
        self.broadcast(
            Writer::packet(clientbound::DAMAGE_EVENT)
                .varint(id)
                .varint(damage_type)
                .varint(cause)
                .varint(cause)
                .bool(false), // no source position
            None,
        );
        self.send(id, health);
        if absorbed > 0.0 {
            self.send_absorption(id);
        }
        if dead {
            self.die(id, kind);
        }
        true
    }

    /// What is left of `amount` after the player's armor, which wears down a little.
    /// Follows the wiki's formula: each armor point takes 4% off, less against hard
    /// hits unless the armor is tough.
    fn through_armor(&mut self, id: i32, amount: f32) -> f32 {
        const ARMOR_SLOTS: [usize; 4] = [5, 6, 7, 8];
        let p = &self.players[&id];
        let (armor, toughness) = ARMOR_SLOTS
            .iter()
            .filter_map(|&slot| ItemInfo::get(p.inventory.slots[slot].item))
            .fold((0.0, 0.0), |(a, t), i| (a + i.armor, t + i.toughness));
        if armor <= 0.0 {
            return amount;
        }
        let wear = ((amount / 4.0).floor() as i32).max(1);
        for slot in ARMOR_SLOTS {
            let worn = ItemInfo::get(self.players[&id].inventory.slots[slot].item)
                .is_some_and(|i| i.armor > 0.0);
            if worn {
                // Break events 49 to 52 are the head, chest, legs and feet slots.
                let event = ENTITY_EVENT_HEAD_BREAK + (slot - 5) as i8;
                self.damage_item(id, slot, wear, event);
            }
        }
        after_armor(amount, armor, toughness)
    }

    /// What is left of `amount` after protection enchantments on the armor: each point
    /// of protection takes 4% off, up to 80%. Protection counts once per level, the
    /// specialised kinds twice (fire, blast, projectile) and feather falling three times.
    fn through_protection(&self, id: i32, amount: f32, kind: Damage) -> f32 {
        if matches!(kind, Damage::Void | Damage::Kill | Damage::Starve) {
            return amount;
        }
        let p = &self.players[&id];
        let mut points = 0;
        for slot in 5..9 {
            let e = p.inventory.slots[slot].enchants;
            points += e.level_of("minecraft:protection");
            points += match kind {
                k if k.is_fire() => 2 * e.level_of("minecraft:fire_protection"),
                Damage::Explosion(_) => 2 * e.level_of("minecraft:blast_protection"),
                Damage::Arrow(_) => 2 * e.level_of("minecraft:projectile_protection"),
                Damage::Fall => 3 * e.level_of("minecraft:feather_falling"),
                _ => 0,
            };
        }
        amount * (1.0 - points.min(20) as f32 / 25.0)
    }

    fn die(&mut self, id: i32, kind: Damage) {
        self.wake_up(id);
        self.clear_effects(id);
        self.drop_xp_on_death(id);
        self.close_container(id);
        let p = self.players.get_mut(&id).expect("player exists");
        p.alive = false;
        p.eating = None;
        p.fall_distance = 0.0;
        let mut message = kind.death_message(&p.profile.name);
        if let Damage::Player(killer) = kind {
            if let Some(k) = self.players.get(&killer) {
                message = format!(
                    "{} was slain by {}",
                    self.players[&id].profile.name, k.profile.name
                );
            }
        }
        let p = self.players.get_mut(&id).expect("player exists");
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
        self.announce(system_chat(&message));
        info!("{message}");
        for stack in dropped {
            self.scatter_item(pos, stack);
        }
    }

    pub(super) fn respawn(&mut self, id: i32) {
        if self.players.get(&id).is_none_or(|p| p.alive) {
            return;
        }
        // Players respawn in the overworld, wherever they died.
        if self.dimension != super::dimension::Dimension::Overworld
            && self.hand_over(
                id,
                super::dimension::Dimension::Overworld,
                None,
                false,
                true,
            )
        {
            return;
        }
        let spawn = self.respawn_point(id);
        let p = self.players.get_mut(&id).expect("player exists");
        p.alive = true;
        p.health = MAX_HEALTH;
        p.food = 20;
        p.saturation = 5.0;
        p.exhaustion = 0.0;
        p.food_timer = 0;
        p.air = 300;
        p.fire_ticks = 0;
        p.pos = spawn;
        p.sent_pos = spawn;
        p.yaw = 0.0;
        p.pitch = 0.0;
        p.moved = false;
        p.center = (
            (spawn[0] / 16.0).floor() as i32,
            (spawn[2] / 16.0).floor() as i32,
        );
        // The client throws away its chunks and entities on respawn.
        p.loaded.clear();
        let mode = p.mode;

        let respawn = spawn_info(Writer::packet(clientbound::RESPAWN), mode, self.dimension).u8(0); // keep no data
        self.send(id, respawn);
        self.send(
            id,
            Writer::packet(clientbound::GAME_EVENT)
                .u8(GAME_EVENT_START_WAITING_FOR_CHUNKS)
                .f32(0.0),
        );
        self.update_view(id);
        self.send(id, position_packet(spawn, 0.0, 0.0));
        self.send_status(id);
        self.send_time_and_weather(id);
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
        if self.use_bucket(id, hand) || self.start_drawing(id, hand) {
            return;
        }
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let stack = p.inventory.slots[p.hand_slot(hand)];
        let Some(info) = ItemInfo::get(stack.item).filter(|i| i.consumable.is_some()) else {
            return;
        };
        // Food needs an appetite unless it can always be eaten; drinks never do.
        let hungry = info
            .food
            .as_ref()
            .is_none_or(|f| p.food < 20 || f.can_always_eat || !p.survival());
        if p.alive && !stack.is_empty() && hungry {
            p.eating = Some((hand, tick));
        }
    }

    fn finish_eating(&mut self, id: i32, hand: i32) {
        let p = self.players.get_mut(&id).expect("player exists");
        let slot = p.hand_slot(hand);
        let stack = p.inventory.slots[slot];
        let Some(info) = ItemInfo::get(stack.item) else {
            return;
        };
        let Some(consumable) = info.consumable.as_ref() else {
            return;
        };
        if stack.is_empty() {
            return;
        }
        if let Some(food) = &info.food {
            p.food = (p.food + food.nutrition).min(20);
            p.saturation = (p.saturation + food.saturation).min(p.food as f32);
            p.health_dirty = true;
        }
        if p.survival() {
            p.inventory.take(slot, 1);
            // A bowl, bucket or bottle is left behind: in the hand if it is now empty.
            if let Some(left) = consumable.remainder.and_then(ItemInfo::id_of) {
                let left = Stack::new(left, 1);
                if p.inventory.slots[slot].is_empty() {
                    p.inventory.slots[slot] = left;
                } else {
                    let (changed, over) = p.inventory.add(left);
                    let updates: Vec<Writer> = changed
                        .into_iter()
                        .map(|s| p.inventory.set_slot_packet(s))
                        .collect();
                    let pos = p.pos;
                    for update in updates {
                        self.send(id, update);
                    }
                    if over > 0 {
                        self.scatter_item(pos, Stack::new(left.item, over));
                    }
                }
            }
            let p = self.players.get_mut(&id).expect("player exists");
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        self.send(id, entity_event(id, ENTITY_EVENT_USE_ITEM_COMPLETE));
        for effect in consumable.effects {
            match effect {
                OnConsume::Apply {
                    probability,
                    effects,
                } => {
                    if self.rng.f64() < f64::from(*probability) {
                        for &e in *effects {
                            self.add_effect(id, e);
                        }
                    }
                }
                OnConsume::ClearAll => {
                    self.clear_effects(id);
                }
                OnConsume::Remove(ids) => {
                    for &e in *ids {
                        self.remove_effect(id, e);
                    }
                }
                OnConsume::TeleportRandomly => {}
            }
        }
    }

    /// Hunger, natural regeneration, starvation, void damage and eating, once per tick.
    pub(super) fn tick_survival(&mut self) {
        let tick = self.tick;
        let mut damage = Vec::new();
        let mut eaten = Vec::new();
        for (&id, p) in self.players.iter_mut().filter(|(_, p)| p.alive) {
            if p.pos[1] < VOID_Y && tick.is_multiple_of(10) {
                damage.push((id, 4.0, Damage::Void));
            }
            if let Some((hand, start)) = p.eating {
                let held = p.inventory.slots[p.hand_slot(hand)].item;
                let ticks = ItemInfo::get(held)
                    .and_then(|i| i.consumable.as_ref())
                    .map_or(EAT_TICKS, |c| u64::from(c.ticks));
                if tick - start >= ticks {
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

#[cfg(test)]
mod tests {
    use super::*;
    use levitas_registry::block;

    /// Ticks to break `state` with `tool`, as vanilla counts them.
    fn ticks(state: u16, tool: &str) -> u32 {
        let tool = ItemInfo::id_of(tool).unwrap_or(0);
        (1.0 / break_progress(state, tool)).ceil() as u32
    }

    #[test]
    fn mining_times_match_vanilla() {
        // Stone: 7.5 s by hand, 1.15 s with a wooden pickaxe, 0.3 s with diamond.
        assert_eq!(ticks(block::STONE, "minecraft:air"), 150);
        assert_eq!(ticks(block::STONE, "minecraft:wooden_pickaxe"), 23);
        assert_eq!(ticks(block::STONE, "minecraft:diamond_pickaxe"), 6);
        // Dirt: 0.75 s by hand, 0.4 s with a wooden shovel.
        assert_eq!(ticks(block::DIRT, "minecraft:air"), 15);
        assert_eq!(ticks(block::DIRT, "minecraft:wooden_shovel"), 8);
        // Logs need no tool; obsidian needs diamond to drop.
        assert!(harvestable(block::OAK_LOG, 0));
        assert!(!harvestable(
            block::OBSIDIAN,
            ItemInfo::id_of("minecraft:iron_pickaxe").unwrap()
        ));
        assert!(harvestable(
            block::OBSIDIAN,
            ItemInfo::id_of("minecraft:diamond_pickaxe").unwrap()
        ));
        // Flowers break at once, bedrock never.
        assert!(break_progress(block::POPPY, 0) >= 1.0);
        assert_eq!(break_progress(block::BEDROCK, 0), 0.0);
    }
}
