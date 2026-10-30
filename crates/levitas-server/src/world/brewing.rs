//! Brewing stands, following the Minecraft Wiki: blaze powder fuels 20 brews; with an
//! ingredient that changes at least one of the three bottles, a brew takes 20 seconds,
//! then turns the bottles and uses the ingredient up. Splash potions thrown at players
//! give them the potion's effects, weaker the farther away they land.

use std::collections::HashSet;

use levitas_nbt::Compound;
use levitas_registry::{block_entity_type, mob_effect, Effect};

use super::*;
use crate::potions::{self, BREW_TICKS, FUEL_USES};

pub const BOTTLES: std::ops::Range<usize> = 0..3;
pub const INGREDIENT: usize = 3;
pub const FUEL: usize = 4;
pub const SLOTS: usize = 5;

/// Reach of a splash potion: players within 4 blocks get its effects.
const SPLASH_RADIUS: f64 = 4.0;
const LEVEL_EVENT_SPLASH_POTION: i32 = 2002;
const LEVEL_EVENT_INSTANT_SPLASH: i32 = 2007;
/// The particle colour of a splash, when the potion has none of its own to show.
const SPLASH_COLOR: i32 = 0x38_5D_C6;

#[derive(Default)]
pub struct Brewer {
    pub items: [Stack; SLOTS],
    /// Ticks left on the brew under way, or 0.
    pub brew_time: i32,
    /// Brews the fuel already burnt still covers.
    pub fuel: i32,
    /// The ingredient the brew under way started with.
    ingredient: i32,
    pub viewers: HashSet<i32>,
}

impl Brewer {
    fn results(&self) -> [Option<Stack>; 3] {
        let ingredient = self.items[INGREDIENT];
        [0, 1, 2].map(|i| potions::brew(ingredient, self.items[i]))
    }

    fn can_brew(&self) -> bool {
        self.results().iter().any(Option::is_some)
    }

    pub fn write_nbt(&self, nbt: &mut Compound) {
        nbt.insert("BrewTime", self.brew_time.clamp(0, BREW_TICKS) as i16);
        nbt.insert("Fuel", self.fuel.clamp(0, FUEL_USES) as i8);
    }

    pub fn read_nbt(&mut self, nbt: &Compound) {
        self.brew_time = nbt.int("BrewTime").unwrap_or(0) as i32;
        self.fuel = nbt.int("Fuel").unwrap_or(0) as i32;
        self.ingredient = self.items[INGREDIENT].item;
    }

    pub fn type_id(&self) -> i32 {
        block_entity_type::BREWING_STAND
    }
}

fn is_blaze_powder(stack: Stack) -> bool {
    ItemInfo::get(stack.item).is_some_and(|i| i.name == "minecraft:blaze_powder")
}

/// The window properties: brew time left and fuel.
pub fn brewing_data(window: i32, b: &Brewer) -> Vec<Writer> {
    [b.brew_time, b.fuel]
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

impl World {
    pub(super) fn tick_brewing(&mut self) {
        let mut updates = Vec::new();
        let mut brewed = Vec::new();
        for (&pos, entity) in self.block_entities.iter_mut() {
            let BlockEntity::Brewing(b) = entity else {
                continue;
            };
            let mut changed = Vec::new();
            let mut data = false;
            if b.fuel <= 0 && is_blaze_powder(b.items[FUEL]) {
                b.fuel = FUEL_USES;
                b.items[FUEL] = b.items[FUEL].with_count(b.items[FUEL].count - 1);
                changed.push(FUEL);
                data = true;
            }
            let can_brew = b.can_brew();
            if b.brew_time > 0 {
                b.brew_time -= 1;
                data = true;
                if b.brew_time == 0 && can_brew {
                    for (slot, result) in b.results().into_iter().enumerate() {
                        if let Some(result) = result {
                            b.items[slot] = result;
                        }
                    }
                    let ingredient = b.items[INGREDIENT];
                    b.items[INGREDIENT] = ingredient.with_count(ingredient.count - 1);
                    changed.extend([0, 1, 2, INGREDIENT]);
                    brewed.push(pos);
                } else if !can_brew || b.items[INGREDIENT].item != b.ingredient {
                    // The ingredient was taken out or swapped: the brew stops.
                    b.brew_time = 0;
                }
            } else if can_brew && b.fuel > 0 {
                b.fuel -= 1;
                b.brew_time = BREW_TICKS;
                b.ingredient = b.items[INGREDIENT].item;
                data = true;
            }
            if data || !changed.is_empty() {
                updates.push((pos, changed));
            }
        }
        for (pos, changed) in updates {
            self.dirty.insert(pos.chunk());
            if !changed.is_empty() {
                self.notify_viewers(pos, &changed, None);
                self.redstone_dirty.push(pos);
            }
            self.send_brewing_data(pos);
            self.show_bottles(pos);
        }
        for pos in brewed {
            self.play_sound("minecraft:block.brewing_stand.brew", pos, None);
        }
    }

    /// Sends the brew progress and fuel to everyone with the stand open.
    pub(super) fn send_brewing_data(&mut self, pos: BlockPos) {
        let Some(BlockEntity::Brewing(b)) = self.block_entities.get(&pos) else {
            return;
        };
        let viewers: Vec<i32> = b.viewers.iter().copied().collect();
        for viewer in viewers {
            let window = self
                .players
                .get(&viewer)
                .and_then(|p| p.open.as_ref())
                .map(|o| o.window);
            let (Some(window), Some(BlockEntity::Brewing(b))) =
                (window, self.block_entities.get(&pos))
            else {
                continue;
            };
            for packet in brewing_data(window, b) {
                self.send(viewer, packet);
            }
        }
    }

    /// The stand's block shows which bottle slots hold something.
    pub(super) fn show_bottles(&mut self, pos: BlockPos) {
        let Some(BlockEntity::Brewing(b)) = self.block_entities.get(&pos) else {
            return;
        };
        let full = [0, 1, 2].map(|i| !b.items[i].is_empty());
        let state = self.block(pos);
        let info = BlockInfo::of(state);
        let mut new = state;
        for (i, full) in full.into_iter().enumerate() {
            let value = if full { "true" } else { "false" };
            if let Some(s) = info.with(new, &format!("has_bottle_{i}"), value) {
                new = s;
            }
        }
        if new != state {
            self.set_block(pos, new);
        }
    }

    /// A splash potion breaks at `at`: players and mobs near get its effects, those nearer more
    /// of them, and one it hit directly all of them.
    pub(super) fn splash(
        &mut self,
        at: [f64; 3],
        potion: Option<u8>,
        direct_player: Option<i32>,
        direct_mob: Option<i32>,
    ) {
        let effects = potion.map(potions::effects).unwrap_or_default();
        let instant = effects.iter().any(|e| potions::is_instant(e.id));
        let event = if instant {
            LEVEL_EVENT_INSTANT_SPLASH
        } else {
            LEVEL_EVENT_SPLASH_POTION
        };
        let pos = BlockPos::new(
            at[0].floor() as i32,
            at[1].floor() as i32,
            at[2].floor() as i32,
        );
        let color = potion.map_or(SPLASH_COLOR, potions::color);
        self.broadcast(
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(event)
                .position(pos)
                .i32(color)
                .bool(false),
            None,
        );
        let near: Vec<(i32, f64)> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive)
            .filter_map(|(&id, p)| {
                let d = [p.pos[0] - at[0], p.pos[1] + 0.9 - at[1], p.pos[2] - at[2]];
                let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let strength = if Some(id) == direct_player {
                    1.0
                } else {
                    1.0 - distance / SPLASH_RADIUS
                };
                (strength > 0.0).then_some((id, strength))
            })
            .collect();
        for (id, strength) in near {
            for effect in &effects {
                // Healing restores 4 health per level, harming deals 6, both scaled.
                let level = f32::from(1u8 << effect.amplifier.min(5)) * strength as f32;
                if effect.id == mob_effect::INSTANT_HEALTH {
                    self.heal(id, (4.0 * level).round());
                    continue;
                }
                if effect.id == mob_effect::INSTANT_DAMAGE {
                    self.damage(id, (6.0 * level).round(), survival::Damage::Magic);
                    continue;
                }
                let duration = (f64::from(effect.duration) * strength + 0.5) as i32;
                if duration > 20 {
                    self.add_effect(
                        id,
                        Effect {
                            duration,
                            ..*effect
                        },
                    );
                }
            }
        }
        let near_mobs: Vec<(i32, f64)> = self
            .mobs
            .iter()
            .filter(|(_, m)| m.alive())
            .filter_map(|(&id, m)| {
                let (_, height) = m.size();
                let d = [
                    m.pos[0] - at[0],
                    m.pos[1] + height * 0.5 - at[1],
                    m.pos[2] - at[2],
                ];
                let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let strength = if Some(id) == direct_mob {
                    1.0
                } else {
                    1.0 - distance / SPLASH_RADIUS
                };
                (strength > 0.0).then_some((id, strength))
            })
            .collect();
        for (id, strength) in near_mobs {
            let is_undead = self.mobs.get(&id).is_some_and(|m| m.kind.is_undead());
            for effect in &effects {
                let level = f32::from(1u8 << effect.amplifier.min(5)) * strength as f32;
                if effect.id == mob_effect::INSTANT_HEALTH {
                    if is_undead {
                        self.hit_mob(
                            id,
                            (6.0 * level).round(),
                            None,
                            [0.0, 0.0],
                            "minecraft:magic",
                        );
                    } else {
                        self.heal_mob(id, (4.0 * level).round());
                    }
                    continue;
                }
                if effect.id == mob_effect::INSTANT_DAMAGE {
                    if is_undead {
                        self.heal_mob(id, (4.0 * level).round());
                    } else {
                        self.hit_mob(
                            id,
                            (6.0 * level).round(),
                            None,
                            [0.0, 0.0],
                            "minecraft:magic",
                        );
                    }
                    continue;
                }
                let duration = (f64::from(effect.duration) * strength + 0.5) as i32;
                if duration > 20 {
                    self.add_mob_effect(
                        id,
                        Effect {
                            duration,
                            ..*effect
                        },
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;

    #[test]
    fn a_stand_brews_awkward_potions_from_water() {
        let dir = std::env::temp_dir().join(format!("levitas-brew-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let pos = BlockPos::new(1, -60, 1);
        let stand = BlockInfo::by_name("minecraft:brewing_stand")
            .unwrap()
            .default_state;
        w.set_block(pos, stand);
        let mut b = Brewer::default();
        let water = potions::stack("minecraft:potion", "water");
        b.items[0] = water;
        b.items[2] = water;
        b.items[INGREDIENT] = Stack::new(ItemInfo::id_of("minecraft:nether_wart").unwrap(), 2);
        b.items[FUEL] = Stack::new(ItemInfo::id_of("minecraft:blaze_powder").unwrap(), 1);
        w.block_entities
            .insert(pos, BlockEntity::Brewing(Box::new(b)));

        w.tick_brewing();
        // The bottles show on the stand.
        assert_eq!(blocks::prop(w.block(pos), "has_bottle_0"), Some("true"));
        assert_eq!(blocks::prop(w.block(pos), "has_bottle_1"), Some("false"));
        for _ in 0..BREW_TICKS {
            w.tick_brewing();
        }
        let Some(BlockEntity::Brewing(b)) = w.block_entities.get(&pos) else {
            panic!("no stand");
        };
        let awkward = potions::stack("minecraft:potion", "awkward");
        assert_eq!((b.items[0], b.items[2]), (awkward, awkward));
        assert!(b.items[1].is_empty());
        assert_eq!(b.items[INGREDIENT].count, 1);
        assert!(b.items[FUEL].is_empty());
        assert_eq!(b.fuel, FUEL_USES - 1);
        assert_eq!(b.brew_time, 0);
    }
}

#[cfg(test)]
mod splash_tests {
    use super::mobs::Mob;
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn splash_potions_reach_players_nearby_weaker_with_distance() {
        let dir = std::env::temp_dir().join(format!("levitas-splash-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let add = |w: &mut World, x: f64| {
            let (out, frames) = mpsc::channel(100_000);
            std::mem::forget(frames);
            let id = next_entity_id();
            let profile = Profile {
                name: format!("P{id}"),
                uuid: id as u128,
                properties: Vec::new(),
            };
            w.players
                .insert(id, Player::new(profile, out, [x, -60.0, 0.5]));
            id
        };
        let near = add(&mut w, 0.5);
        let farther = add(&mut w, 2.5);
        let away = add(&mut w, 10.5);
        let swift = potions::id("swiftness");
        w.splash([0.5, -59.1, 0.5], swift, None, None);
        let speed =
            |w: &World, id: i32| w.players[&id].effect(mob_effect::SPEED).map(|e| e.duration);
        let (a, b) = (speed(&w, near).unwrap(), speed(&w, farther).unwrap());
        assert!(a > b && a <= 3600, "{a} {b}");
        assert_eq!(speed(&w, away), None);
    }

    #[test]
    fn splash_potions_reach_mobs_and_undead_react_inversely_to_healing_and_harming() {
        let dir = std::env::temp_dir().join(format!("levitas-splash-mobs-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let mut rng = Rng::new(42);
        let cow = next_entity_id();
        let mut cow_mob = Mob::new(super::mobs::MobKind::Cow, [0.5, -60.0, 0.5], &mut rng);
        cow_mob.health = 5.0; // max is 10.0
        w.mobs.insert(cow, cow_mob);

        let zombie = next_entity_id();
        let mut zombie_mob = Mob::new(super::mobs::MobKind::Zombie, [1.0, -60.0, 0.5], &mut rng);
        zombie_mob.health = 20.0;
        w.mobs.insert(zombie, zombie_mob);

        // Instant health heals the cow and hurts the zombie.
        let healing = potions::id("healing");
        w.splash([0.5, -60.0, 0.5], healing, None, Some(cow));
        assert!(w.mobs[&cow].health > 5.0);
        assert!(w.mobs[&zombie].health < 20.0);

        // Instant damage hurts the cow and heals the zombie.
        let zombie_hp_before = w.mobs[&zombie].health;
        let cow_hp_before = w.mobs[&cow].health;
        let harming = potions::id("harming");
        w.splash([1.0, -60.0, 0.5], harming, None, Some(zombie));
        assert!(w.mobs[&zombie].health > zombie_hp_before);
        assert!(w.mobs[&cow].health < cow_hp_before);

        // Slowness gives effects to mobs within reach.
        let slowness = potions::id("slowness");
        w.splash([0.5, -60.0, 0.5], slowness, None, None);
        assert!(w.mobs[&cow].effect(mob_effect::SLOWNESS).is_some());
        assert!(w.mobs[&zombie].effect(mob_effect::SLOWNESS).is_some());
    }
}
