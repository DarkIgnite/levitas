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
    /// Hit by a fireball from what is named.
    Fireball(&'static str),
    /// Poison and instant damage.
    Magic,
    Wither,
    Lightning,
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
            Damage::Fireball(_) => "minecraft:fireball",
            Damage::Magic => "minecraft:magic",
            Damage::Wither => "minecraft:wither",
            Damage::Lightning => "minecraft:lightning_bolt",
        }
    }

    /// Whether a shield can stop this kind of damage.
    fn blockable(self) -> bool {
        matches!(
            self,
            Damage::Mob(_)
                | Damage::Player(_)
                | Damage::Arrow(_)
                | Damage::Fireball(_)
                | Damage::Explosion(_)
        )
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
            Damage::Fireball(by) => format!("{name} was fireballed by {by}"),
            Damage::Magic => format!("{name} was killed by magic"),
            Damage::Wither => format!("{name} withered away"),
            Damage::Lightning => format!("{name} was struck by lightning"),
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
                | Damage::Fireball(_)
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

use crate::rules::Difficulty;

/// How long after a mob's hit a death still counts as killed by it (killed_by).
const KILL_CREDIT_TICKS: u64 = 100;

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
        let rule = match kind {
            Damage::Fall => "fall_damage",
            Damage::Drown => "drowning_damage",
            _ if kind.is_fire() || matches!(kind, Damage::OnFire) => "fire_damage",
            _ => "",
        };
        if !rule.is_empty() && !crate::rules::bool(rule) {
            return false;
        }
        if let Some(attacker) = source.filter(|s| self.mobs.contains_key(s)) {
            self.rally_wolves(id, attacker);
        }
        // Monsters hit harder or softer with the difficulty.
        let by_mob = source.is_some_and(|s| self.mobs.contains_key(&s))
            || matches!(kind, Damage::Explosion("Creeper"));
        let amount = if by_mob {
            match crate::rules::difficulty() {
                Difficulty::Peaceful => return false,
                Difficulty::Easy => (amount / 2.0 + 1.0).min(amount),
                Difficulty::Normal => amount,
                Difficulty::Hard => amount * 1.5,
            }
        } else {
            amount
        };
        // A raised shield takes hits from mobs, players and their projectiles.
        if let Some((from, axe)) = source.and_then(|s| self.attacker(s)) {
            let axe = axe && matches!(kind, Damage::Player(_));
            if kind.blockable() && self.shield_blocks(id, amount, from, axe) {
                if let (Some(attacker), Damage::Mob(_) | Damage::Player(_)) = (source, kind) {
                    self.shield_knockback(id, attacker);
                }
                return false;
            }
        }
        let now = self.tick;
        let hurt_by = source
            .and_then(|s| self.mobs.get(&s))
            .map(|m| m.kind.entity_type());
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
        p.stats.custom(
            levitas_registry::custom_stat::DAMAGE_TAKEN,
            (amount * 10.0).round() as i32,
        );
        if let Some(kind) = hurt_by {
            p.hurt_by = Some((kind, now));
        }
        p.stats.custom(
            levitas_registry::custom_stat::DAMAGE_ABSORBED,
            (absorbed * 10.0).round() as i32,
        );
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
        // Thorns hurts whoever struck in melee.
        if let Some(attacker) =
            source.filter(|_| matches!(kind, Damage::Mob(_) | Damage::Player(_)))
        {
            self.thorns(id, attacker);
        }
        if dead {
            self.die(id, kind);
        }
        true
    }

    /// Each armour piece with thorns may strike back: a 15% chance a level, 1 to 4
    /// damage (the level less 10 above level 10).
    fn thorns(&mut self, id: i32, attacker: i32) {
        let levels: Vec<u32> = [5, 6, 7, 8]
            .iter()
            .map(|&slot| {
                self.players[&id].inventory.slots[slot]
                    .enchants
                    .level_of("minecraft:thorns")
            })
            .filter(|&l| l > 0)
            .collect();
        for level in levels {
            if self.rng.f64() >= 0.15 * f64::from(level) {
                continue;
            }
            let damage = if level > 10 {
                (level - 10) as f32
            } else {
                1.0 + self.rng.below(4) as f32
            };
            if self.mobs.contains_key(&attacker) {
                self.hit_mob(attacker, damage, Some(id), [0.0, 0.0], "minecraft:thorns");
            } else if attacker != id && self.players.contains_key(&attacker) {
                self.damage_from(attacker, damage, Damage::Magic, None);
            }
        }
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
        let now = self.tick;
        self.get_off(id);
        self.wake_up(id);
        self.clear_effects(id);
        let keep = crate::rules::bool("keep_inventory");
        if !keep {
            self.drop_xp_on_death(id);
        }
        self.close_container(id);
        self.lower_shield(id);
        self.stop_spyglass(id);
        let p = self.players.get_mut(&id).expect("player exists");
        p.alive = false;
        p.eating = None;
        p.fall_distance = 0.0;
        p.stats.custom(levitas_registry::custom_stat::DEATHS, 1);
        p.stats.set(
            levitas_registry::stat_type::CUSTOM,
            levitas_registry::custom_stat::TIME_SINCE_DEATH,
            0,
        );
        // Killed by a mob, or by another player: both sides count it.
        let killer_kind = match kind {
            Damage::Player(_) => Some(entity_type::PLAYER),
            // Only a mob that hit them in the last five seconds killed them.
            _ => p
                .hurt_by
                .take()
                .filter(|&(_, at)| now.saturating_sub(at) <= KILL_CREDIT_TICKS)
                .map(|(kind, _)| kind),
        };
        if let Some(kind) = killer_kind {
            p.stats.add(levitas_registry::stat_type::KILLED_BY, kind, 1);
        }
        if let Damage::Player(killer) = kind {
            self.stat(
                killer,
                levitas_registry::stat_type::KILLED,
                entity_type::PLAYER,
                1,
            );
            self.stat(
                killer,
                levitas_registry::stat_type::CUSTOM,
                levitas_registry::custom_stat::PLAYER_KILLS,
                1,
            );
        }
        let p = self.players.get_mut(&id).expect("player exists");
        let pos = p.pos;
        let death_pos = BlockPos::new(
            pos[0].floor() as i32,
            pos[1].floor() as i32,
            pos[2].floor() as i32,
        );
        let death_dim = match self.dimension {
            super::dimension::Dimension::Nether => "minecraft:the_nether",
            super::dimension::Dimension::End => "minecraft:the_end",
            super::dimension::Dimension::Overworld => "minecraft:overworld",
        };
        p.last_death = Some((death_dim, death_pos));
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
        let dropped = if keep {
            Vec::new()
        } else {
            // Curse of vanishing: gone instead of dropped.
            p.inventory
                .drain()
                .into_iter()
                .filter(|s| s.enchants.level_of("minecraft:vanishing_curse") == 0)
                .collect()
        };
        let content = p.inventory.content_packet();

        self.send(id, content);
        self.send(
            id,
            Writer::packet(clientbound::PLAYER_COMBAT_KILL)
                .varint(id)
                .text(&message),
        );
        self.broadcast(entity_event(id, ENTITY_EVENT_DEATH), Some(id));
        if crate::rules::bool("show_death_messages") {
            self.announce(system_chat(&message));
        }
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
                super::travel::Via::Respawn,
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

        let death = p.last_death;
        let respawn = spawn_info(
            Writer::packet(clientbound::RESPAWN),
            mode,
            self.dimension,
            death,
        )
        .u8(0); // keep no data
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
        // A respawned client starts its attributes afresh.
        if let Some(p) = self.players.get_mut(&id) {
            p.sent_attributes = None;
        }
        let spawn = spawn_player(id, &self.players[&id]);
        for packet in spawn {
            self.broadcast(packet, Some(id));
        }
        self.send(id, super::player_data(id, &self.players[&id]));
    }

    /// Chorus fruit: the eater goes to a random spot within `diameter` blocks across
    /// where they can stand (ground under two free blocks), trying 16 times.
    pub(super) fn teleport_randomly(&mut self, id: i32, diameter: f64) {
        let Some(from) = self.players.get(&id).map(|p| p.pos) else {
            return;
        };
        for _ in 0..16 {
            let x = from[0] + (self.rng.f64() - 0.5) * diameter;
            let z = from[2] + (self.rng.f64() - 0.5) * diameter;
            let y_top = (from[1] + (self.rng.f64() - 0.5) * diameter).floor() as i32;
            let (bx, bz) = (x.floor() as i32, z.floor() as i32);
            let free = |w: &mut World, y: i32| {
                let s = w.block(BlockPos::new(bx, y, bz));
                levitas_registry::collision_boxes(s).is_empty()
                    && crate::blocks::fluid::fluid_at(s).is_none()
            };
            let mut y = y_top.clamp(MIN_Y + 1, MAX_Y - 2);
            // Down to the ground.
            while y > MIN_Y + 1 && free(self, y - 1) {
                y -= 1;
            }
            if y <= MIN_Y + 1 || !free(self, y) || !free(self, y + 1) {
                continue;
            }
            let to = [x, f64::from(y), z];
            self.play_sound_at("minecraft:item.chorus_fruit.teleport", from, 1.0, 1.0, None);
            self.teleport(id, to);
            self.play_sound_at("minecraft:item.chorus_fruit.teleport", to, 1.0, 1.0, None);
            if let Some(p) = self.players.get_mut(&id) {
                p.fall_distance = 0.0;
            }
            self.send(
                id,
                Writer::packet(clientbound::COOLDOWN)
                    .string("minecraft:chorus_fruit")
                    .varint(20),
            );
            return;
        }
    }

    pub(super) fn use_spyglass(&mut self, id: i32, hand: i32) -> bool {
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        let stack = p.inventory.slots[p.hand_slot(hand)];
        if ItemInfo::get(stack.item).is_none_or(|i| i.name != "minecraft:spyglass") {
            return false;
        }
        if p.alive {
            p.using_spyglass = Some(hand);
            let at = p.pos;
            let flags = super::shields::HAND_ACTIVE
                | if hand == 1 {
                    super::shields::OFFHAND_ACTIVE
                } else {
                    0
                };
            self.broadcast(super::shields::living_flags(id, flags), Some(id));
            self.play_sound_at("minecraft:item.spyglass.use", at, 1.0, 1.0, None);
        }
        true
    }

    pub(super) fn stop_spyglass(&mut self, id: i32) {
        if let Some(p) = self.players.get_mut(&id) {
            if p.using_spyglass.take().is_some() {
                let at = p.pos;
                self.broadcast(super::shields::living_flags(id, 0), Some(id));
                self.play_sound_at("minecraft:item.spyglass.stop_using", at, 1.0, 1.0, None);
            }
        }
    }

    pub(super) fn open_empty_map(&mut self, id: i32, hand: i32) -> bool {
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        let slot = p.hand_slot(hand);
        let stack = p.inventory.slots[slot];
        if ItemInfo::get(stack.item).is_none_or(|i| i.name != "minecraft:map") {
            return false;
        }
        let Some(filled_id) = ItemInfo::id_of("minecraft:filled_map") else {
            return false;
        };
        let at = p.pos;
        let map_id = self.create_map(at[0].floor() as i32, at[2].floor() as i32, 0);
        let p = self.players.get_mut(&id).expect("player exists");
        let mut filled = Stack::new(filled_id, 1);
        filled.map_id = Some(map_id);
        let survival = p.survival();
        if stack.count == 1 {
            p.inventory.slots[slot] = filled;
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        } else if survival {
            p.inventory.take(slot, 1);
            let (changed, over) = p.inventory.add(filled);
            let mut updates: Vec<Writer> = changed
                .into_iter()
                .map(|s| p.inventory.set_slot_packet(s))
                .collect();
            updates.push(p.inventory.set_slot_packet(slot));
            for update in updates {
                self.send(id, update);
            }
            if over > 0 {
                self.scatter_item(at, filled.with_count(over));
            }
        } else {
            let (changed, over) = p.inventory.add(filled);
            let updates: Vec<Writer> = changed
                .into_iter()
                .map(|s| p.inventory.set_slot_packet(s))
                .collect();
            for update in updates {
                self.send(id, update);
            }
            if over > 0 {
                self.scatter_item(at, filled.with_count(over));
            }
        }
        self.play_sound_at(
            "minecraft:ui.cartography_table.take_result",
            at,
            1.0,
            1.0,
            None,
        );
        true
    }

    pub(super) fn use_item(&mut self, id: i32, hand: i32, sequence: i32) {
        self.ack(id, sequence);
        if self.use_bucket(id, hand)
            || self.place_boat(id, hand)
            || self.fill_bottle(id, hand)
            || self.start_ranged(id, hand)
            || self.open_book(id, hand)
            || self.start_drawing(id, hand)
            || self.throw_held(id, hand)
            || self.use_fishing_rod(id, hand)
            || self.throw_eye(id, hand)
            || self.use_spyglass(id, hand)
            || self.open_empty_map(id, hand)
        {
            return;
        }
        if self.raise_shield(id, hand) {
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
        let eaten = stack.with_count(1);
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
        for effect in stack
            .potion
            .map(crate::potions::effects)
            .unwrap_or_default()
        {
            self.add_effect(id, effect);
        }
        let mut chorus = false;
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
                OnConsume::TeleportRandomly => chorus = true,
            }
        }
        if chorus {
            self.teleport_randomly(id, 16.0);
        }
        if info.name == "minecraft:ominous_bottle" {
            let pos = self.players.get(&id).map(|p| p.pos).unwrap_or_default();
            self.play_sound_at("minecraft:item.ominous_bottle.dispose", pos, 1.0, 1.0, None);
            self.add_effect(
                id,
                levitas_registry::Effect {
                    id: levitas_registry::mob_effect::BAD_OMEN,
                    duration: 120_000,
                    amplifier: stack.ominous.unwrap_or(0),
                },
            );
        }
        // A suspicious stew gives its effect.
        if let Some((effect, duration)) = stack.stew {
            self.add_effect(
                id,
                levitas_registry::Effect {
                    id: i32::from(effect),
                    duration,
                    amplifier: 0,
                },
            );
        }
        self.stat(id, levitas_registry::stat_type::USED, eaten.item, 1);
        self.advance(id, super::advancements::Trigger::ConsumeItem(eaten));
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
            let difficulty = crate::rules::difficulty();
            let regenerates = crate::rules::bool("natural_health_regeneration");
            if difficulty == Difficulty::Peaceful {
                // No hunger in peaceful: health and food come back on their own.
                if regenerates && p.health < p.max_health() && tick.is_multiple_of(20) {
                    p.health = (p.health + 1.0).min(p.max_health());
                    p.health_dirty = true;
                }
                if p.food < 20 && tick.is_multiple_of(10) {
                    p.food += 1;
                    p.health_dirty = true;
                }
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
            let max = p.max_health();
            let hurt = p.health < max;
            // Health beyond the most allowed (a Health Boost that ran out) goes.
            if p.health > max {
                p.health = max;
                p.health_dirty = true;
            }
            if hurt && regenerates && p.food >= 20 && p.saturation > 0.0 {
                // Fast regeneration from saturation.
                if p.food_timer >= 10 {
                    let spent = p.saturation.min(6.0);
                    p.health = (p.health + spent / 6.0).min(max);
                    p.exhaustion += spent;
                    p.food_timer = 0;
                    p.health_dirty = true;
                }
            } else if hurt && regenerates && p.food >= 18 {
                if p.food_timer >= 80 {
                    p.health = (p.health + 1.0).min(max);
                    p.exhaustion += 6.0;
                    p.food_timer = 0;
                    p.health_dirty = true;
                }
            } else if p.food <= 0 {
                // Starving stops at five hearts on Easy and half a heart on Normal,
                // and kills on Hard.
                let floor = match difficulty {
                    Difficulty::Easy => 10.0,
                    Difficulty::Normal => 1.0,
                    _ => 0.0,
                };
                if p.food_timer >= 80 {
                    if p.health > floor {
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

    #[test]
    fn spyglass_use_and_stop() {
        let dir = std::env::temp_dir().join(format!("levitas-spyglass-{}", next_entity_id()));
        let (mut world, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let (out, _frames) = mpsc::channel(100);
        let id = next_entity_id();
        let profile = crate::connection::Profile {
            name: "Bob".to_owned(),
            uuid: 2,
            properties: Vec::new(),
        };
        let mut player = Player::new(profile, out, [0.0, 64.0, 0.0]);
        let spyglass = ItemInfo::id_of("minecraft:spyglass").unwrap();
        player.inventory.slots[crate::inventory::HOTBAR] = Stack::new(spyglass, 1);
        world.players.insert(id, player);

        assert!(world.use_spyglass(id, 0));
        assert_eq!(world.players[&id].using_spyglass, Some(0));

        world.stop_spyglass(id);
        assert_eq!(world.players[&id].using_spyglass, None);
    }

    #[test]
    fn death_tracks_last_death_location() {
        let dir = std::env::temp_dir().join(format!("levitas-death-{}", next_entity_id()));
        let (mut world, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let (out, _frames) = mpsc::channel(100);
        let id = next_entity_id();
        let profile = crate::connection::Profile {
            name: "Charlie".to_owned(),
            uuid: 3,
            properties: Vec::new(),
        };
        let player = Player::new(profile, out, [12.7, 65.2, -34.8]);
        world.players.insert(id, player);

        world.die(id, Damage::Kill);
        let p = &world.players[&id];
        assert!(!p.alive);
        let (dim, pos) = p.last_death.expect("last death should be recorded");
        assert_eq!(dim, "minecraft:overworld");
        assert_eq!(pos, BlockPos::new(12, 65, -35));
    }

    #[test]
    fn empty_map_opens_into_filled_map() {
        let dir = std::env::temp_dir().join(format!("levitas-map-{}", next_entity_id()));
        let (mut world, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let (out, _frames) = mpsc::channel(100);
        let id = next_entity_id();
        let profile = crate::connection::Profile {
            name: "Dave".to_owned(),
            uuid: 4,
            properties: Vec::new(),
        };
        let mut player = Player::new(profile, out, [0.0, 64.0, 0.0]);
        let empty_map = ItemInfo::id_of("minecraft:map").unwrap();
        let filled_map = ItemInfo::id_of("minecraft:filled_map").unwrap();
        player.inventory.slots[crate::inventory::HOTBAR] = Stack::new(empty_map, 1);
        world.players.insert(id, player);

        assert!(world.open_empty_map(id, 0));
        let held = world.players[&id].inventory.slots[crate::inventory::HOTBAR];
        assert_eq!(held.item, filled_map);
        // A blank map of the grid square the player stands in.
        let map = world.map_data(held.map_id.unwrap()).unwrap();
        assert_eq!((map.center, map.scale), ([0, 0], 0));
    }
}

#[cfg(test)]
mod curse_tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with(id: i32) -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-curses-{}-{}",
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
        let (tx, _rx) = tokio::sync::mpsc::channel(4096);
        let profile = Profile {
            uuid: 5,
            name: "Cursed".to_owned(),
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, tx, [0.5, -60.0, 0.5]));
        w
    }

    fn cursed(item: &str, curse: &str) -> Stack {
        let mut s = Stack::new(ItemInfo::id_of(item).unwrap(), 1);
        s.enchants
            .set(crate::enchanting::enchantment_id(curse).unwrap(), 1);
        s
    }

    #[test]
    fn vanishing_items_are_gone_on_death_and_bound_armour_stays_on() {
        let id = 1;
        let mut w = world_with(id);
        let p = w.players.get_mut(&id).unwrap();
        p.inventory.slots[10] = cursed("minecraft:diamond_sword", "minecraft:vanishing_curse");
        p.inventory.slots[11] = Stack::new(ItemInfo::id_of("minecraft:dirt").unwrap(), 3);
        p.inventory.slots[5] = cursed("minecraft:iron_helmet", "minecraft:binding_curse");
        // Taking the bound helmet off is refused in survival.
        w.container_click(id, 0, 5, 0, 0, Vec::new(), Stack::EMPTY);
        assert!(!w.players[&id].inventory.slots[5].is_empty());
        // Dying drops the dirt but not the vanishing sword.
        let before = w.items.len();
        w.die(id, Damage::Kill);
        let sword = ItemInfo::id_of("minecraft:diamond_sword").unwrap();
        let dropped: Vec<i32> = w.items.values().map(|i| i.stack().item).collect();
        assert!(w.items.len() > before);
        assert!(!dropped.contains(&sword));
    }

    #[test]
    fn thorns_strikes_back_at_a_mob() {
        let id = 1;
        let mut w = world_with(id);
        let mut chest = cursed("minecraft:iron_chestplate", "minecraft:thorns");
        // Thorns X: a 150% chance, so it always strikes.
        chest.enchants.set(
            crate::enchanting::enchantment_id("minecraft:thorns").unwrap(),
            10,
        );
        w.players.get_mut(&id).unwrap().inventory.slots[6] = chest;
        let zid = next_entity_id();
        w.mobs.insert(
            zid,
            super::super::mobs::Mob::new(
                super::super::mobs::MobKind::Zombie,
                [1.5, -60.0, 0.5],
                &mut w.rng,
            ),
        );
        let before = w.mobs[&zid].health;
        w.damage_from(id, 2.0, Damage::Mob("Zombie"), Some(zid));
        assert!(w.mobs[&zid].health < before);
    }
}
