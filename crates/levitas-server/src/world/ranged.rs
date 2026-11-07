//! Crossbows and tridents, as the Minecraft Wiki describes them.
//!
//! A crossbow is loaded by holding use for 1.25 seconds (a quarter second less a level
//! of Quick Charge) and letting go: it takes an arrow and keeps it (the
//! `minecraft:charged_projectiles` component), three with Multishot. Using it again
//! shoots at 3.15 blocks a tick, the extra Multishot arrows ten degrees to each side;
//! Piercing sends an arrow through that many more mobs.
//!
//! A trident is thrown after holding use for half a second, at 2.5 blocks a tick, and
//! deals 8 damage (Impaling adds 2.5 a level against water mobs); it then drops away.
//! Loyalty brings it back to its thrower, Channeling calls lightning onto what it hits
//! in a thunderstorm, and Riptide flings the player instead of throwing, in water or
//! rain.

use super::arrows::{Arrow, Hit, Shot};
use super::*;
use crate::inventory::Charged;
use crate::world::survival::Damage;

/// Ticks to load a crossbow, less this many a level of Quick Charge.
const CROSSBOW_CHARGE: u64 = 25;
const QUICK_CHARGE_STEP: u64 = 5;
const CROSSBOW_SPEED: f64 = 3.15;
/// Ticks of holding before a trident can be thrown.
const TRIDENT_CHARGE: u64 = 10;
const TRIDENT_SPEED: f64 = 2.5;
const TRIDENT_DAMAGE: f32 = 8.0;

fn named(stack: Stack, name: &str) -> bool {
    ItemInfo::get(stack.item).is_some_and(|i| i.name == name)
}

/// Entity data for a thrown trident: its Loyalty level and whether it glints.
pub(super) fn trident_data(id: i32, trident: Stack) -> Writer {
    const DATA_LOYALTY: u8 = 11;
    const DATA_GLINT: u8 = 12;
    const SERIALIZER_BYTE: i32 = 0;
    const SERIALIZER_BOOLEAN: i32 = 8;
    let loyalty = trident.enchants.level_of("minecraft:loyalty").min(127) as u8;
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_LOYALTY)
        .varint(SERIALIZER_BYTE)
        .u8(loyalty)
        .u8(DATA_GLINT)
        .varint(SERIALIZER_BOOLEAN)
        .bool(!trident.enchants.is_empty())
        .u8(0xFF)
}

/// Turns a direction about the vertical axis by `degrees`.
fn turn(dir: [f64; 3], degrees: f64) -> [f64; 3] {
    let (s, c) = degrees.to_radians().sin_cos();
    [dir[0] * c - dir[2] * s, dir[1], dir[0] * s + dir[2] * c]
}

impl World {
    /// Using a crossbow (loading or shooting it) or a trident (starting the throw).
    /// Returns whether the item was one of them.
    pub(super) fn start_ranged(&mut self, id: i32, hand: i32) -> bool {
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        let slot = p.hand_slot(hand);
        let held = p.inventory.slots[slot];
        if named(held, "minecraft:crossbow") {
            if held.charged.is_some() {
                self.fire_crossbow(id, hand);
            } else if p.alive && (!p.survival() || super::arrows::find_arrows(p).is_some()) {
                p.drawing = Some((hand, tick));
                let at = p.pos;
                self.play_sound_at("minecraft:item.crossbow.loading_start", at, 1.0, 1.0, None);
            }
            return true;
        }
        if named(held, "minecraft:trident") {
            let worn_out =
                ItemInfo::get(held.item).is_some_and(|i| held.damage >= i.max_damage - 1);
            if p.alive && !worn_out {
                p.drawing = Some((hand, tick));
            }
            return true;
        }
        false
    }

    /// Letting go of use: a crossbow held long enough is loaded, a trident thrown (or
    /// ridden, with Riptide). Returns whether it was one of them.
    pub(super) fn release_ranged(&mut self, id: i32) -> bool {
        let tick = self.tick;
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let Some((hand, start)) = p.drawing else {
            return false;
        };
        let held = p.inventory.slots[p.hand_slot(hand)];
        let held_for = tick.saturating_sub(start);
        if named(held, "minecraft:crossbow") {
            self.players.get_mut(&id).expect("checked").drawing = None;
            let quick = u64::from(held.enchants.level_of("minecraft:quick_charge"));
            let needed = CROSSBOW_CHARGE.saturating_sub(QUICK_CHARGE_STEP * quick);
            if held_for >= needed && held.charged.is_none() {
                self.load_crossbow(id, hand);
            }
            return true;
        }
        if named(held, "minecraft:trident") {
            self.players.get_mut(&id).expect("checked").drawing = None;
            if held_for >= TRIDENT_CHARGE {
                self.release_trident(id, hand);
            }
            return true;
        }
        false
    }

    fn load_crossbow(&mut self, id: i32, hand: i32) {
        let p = self.players.get_mut(&id).expect("player exists");
        let slot = p.hand_slot(hand);
        let crossbow = p.inventory.slots[slot];
        let rocket = p.inventory.slots[OFFHAND].is_named("minecraft:firework_rocket");
        let arrow = if rocket {
            Some(OFFHAND)
        } else {
            super::arrows::find_arrows(p)
        };
        let ammo = match arrow {
            Some(s) => p.inventory.slots[s],
            None if !p.survival() => Stack::new(ItemInfo::id_of("minecraft:arrow").unwrap_or(0), 1),
            None => return,
        };
        if p.survival() {
            if let Some(s) = arrow {
                p.inventory.take(s, 1);
                let update = p.inventory.set_slot_packet(s);
                self.send(id, update);
            }
        }
        let multishot = crossbow.enchants.level_of("minecraft:multishot") > 0;
        let p = self.players.get_mut(&id).expect("player exists");
        p.inventory.slots[slot].charged = Some(Charged {
            item: ammo.item,
            count: if multishot { 3 } else { 1 },
            potion: ammo.potion,
            firework: ammo.firework,
        });
        let update = p.inventory.set_slot_packet(slot);
        let at = p.pos;
        self.send(id, update);
        self.play_sound_at("minecraft:item.crossbow.loading_end", at, 1.0, 1.0, None);
    }

    /// Shoots a loaded crossbow.
    pub(super) fn fire_crossbow(&mut self, id: i32, hand: i32) {
        let p = &self.players[&id];
        let slot = p.hand_slot(hand);
        let crossbow = p.inventory.slots[slot];
        let Some(loaded) = crossbow.charged else {
            return;
        };
        let survival = p.survival();
        let (eye, look, _) = p.eye_ray();
        let pierce = crossbow.enchants.level_of("minecraft:piercing").min(255) as u8;
        let tipped = ItemInfo::get(loaded.item).is_some_and(|i| i.name == "minecraft:tipped_arrow");
        let rocket =
            ItemInfo::get(loaded.item).is_some_and(|i| i.name == "minecraft:firework_rocket");
        for (i, angle) in [0.0, -10.0, 10.0]
            .into_iter()
            .take(usize::from(loaded.count))
            .enumerate()
        {
            if rocket {
                let mut stack = Stack::new(loaded.item, 1);
                stack.firework = loaded.firework;
                let dir = turn(look, angle);
                self.launch_rocket(stack, eye, dir.map(|v| v * 1.6), Some(id), None, true);
                continue;
            }
            self.shoot_arrow(
                id,
                true,
                Shot {
                    from: eye,
                    dir: turn(look, angle),
                    speed: CROSSBOW_SPEED,
                    inaccuracy: 1.0,
                    base_damage: 2.0,
                    // The extra Multishot arrows cannot be picked up.
                    pickup: survival && i == 0,
                    potion: loaded.potion.filter(|_| tipped),
                    pierce,
                    ..Default::default()
                },
            );
        }
        let p = self.players.get_mut(&id).expect("player exists");
        p.inventory.slots[slot].charged = None;
        let update = p.inventory.set_slot_packet(slot);
        let at = p.pos;
        self.send(id, update);
        self.play_sound_at("minecraft:item.crossbow.shoot", at, 1.0, 1.0, None);
        self.stat(id, levitas_registry::stat_type::USED, crossbow.item, 1);
        if survival {
            let wear = if loaded.count > 1 { 3 } else { 1 };
            let event = if hand == 0 { 47 } else { 48 };
            self.damage_item(id, slot, wear, event);
        }
    }

    fn release_trident(&mut self, id: i32, hand: i32) {
        let p = &self.players[&id];
        let slot = p.hand_slot(hand);
        let trident = p.inventory.slots[slot];
        let survival = p.survival();
        let (eye, look, _) = p.eye_ray();
        let at = p.pos;
        let riptide = trident.enchants.level_of("minecraft:riptide");
        let event = if hand == 0 { 47 } else { 48 };
        if riptide > 0 {
            // Only in water or in the rain.
            let feet = BlockPos::new(
                at[0].floor() as i32,
                at[1].floor() as i32,
                at[2].floor() as i32,
            );
            let wet = self.in_water(at) || self.raining_at(feet);
            if !wet {
                return;
            }
            let force = 3.0 * (1.0 + f64::from(riptide)) / 4.0;
            self.push_player(id, look.map(|v| v * force));
            let sound = format!("minecraft:item.trident.riptide_{}", riptide.clamp(1, 3));
            self.play_sound_at(&sound, at, 1.0, 1.0, None);
            if survival {
                self.damage_item(id, slot, 1, event);
            }
            self.stat(id, levitas_registry::stat_type::USED, trident.item, 1);
            return;
        }
        let mut thrown = trident;
        if survival {
            if let Some(info) = ItemInfo::get(trident.item) {
                thrown.damage = (thrown.damage + 1).min(info.max_damage);
            }
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.slots[slot] = Stack::EMPTY;
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        self.stat(id, levitas_registry::stat_type::USED, trident.item, 1);
        self.shoot_arrow(
            id,
            true,
            Shot {
                from: eye,
                dir: look,
                speed: TRIDENT_SPEED,
                inaccuracy: 1.0,
                base_damage: 0.0,
                pickup: survival,
                trident: Some(thrown),
                ..Default::default()
            },
        );
    }

    /// A thrown trident hits a player or mob: 8 damage (Impaling adds against water
    /// mobs), lightning with Channeling in a thunderstorm, and it drops away.
    pub(super) fn trident_hits(&mut self, arrow: &mut Arrow, hit: Hit) {
        let Some(trident) = arrow.trident else {
            return;
        };
        let impaling = trident.enchants.level_of("minecraft:impaling") as f32;
        let horizontal = arrow.vel[0].hypot(arrow.vel[2]).max(1e-9);
        let away = [arrow.vel[0] / horizontal, arrow.vel[2] / horizontal];
        let target_pos = match hit {
            Hit::Player(pid) => {
                self.damage_from(
                    pid,
                    TRIDENT_DAMAGE,
                    Damage::Mob("Trident"),
                    Some(arrow.shooter),
                );
                self.players.get(&pid).map(|p| p.pos)
            }
            Hit::Mob(mid) => {
                let aquatic = self.mobs.get(&mid).is_some_and(|m| m.kind.swims());
                let damage = TRIDENT_DAMAGE + if aquatic { 2.5 * impaling } else { 0.0 };
                let by = arrow.by_player.then_some(arrow.shooter);
                self.hit_mob(mid, damage, by, away, "minecraft:trident");
                self.mobs.get(&mid).map(|m| m.pos)
            }
            _ => None,
        };
        self.play_sound_at("minecraft:item.trident.hit", arrow.pos, 1.0, 1.0, None);
        if let Some(at) = target_pos {
            let open = BlockPos::new(
                at[0].floor() as i32,
                at[1].floor() as i32,
                at[2].floor() as i32,
            );
            if trident.enchants.level_of("minecraft:channeling") > 0
                && self.weather.is_thundering()
                && self.raining_at(open)
            {
                self.strike_lightning(at);
                self.play_sound_at("minecraft:item.trident.thunder", at, 5.0, 1.0, None);
            }
        }
        arrow.dealt = true;
        arrow.vel = [
            arrow.vel[0] * -0.01,
            arrow.vel[1] * -0.1,
            arrow.vel[2] * -0.01,
        ];
    }

    /// A loyal trident that hit something or landed flies back to its thrower, through
    /// blocks, and goes back into their inventory. `None` when it is not coming back;
    /// otherwise whether it has arrived (and is gone).
    pub(super) fn trident_returns(&mut self, id: i32, arrow: &mut Arrow) -> Option<bool> {
        let trident = arrow.trident?;
        let loyalty = trident.enchants.level_of("minecraft:loyalty");
        let landed = arrow.stuck.is_some_and(|t| t >= 2);
        if loyalty == 0 || !(arrow.dealt || landed || arrow.returning) {
            return None;
        }
        let owner = self.players.get(&arrow.shooter).filter(|p| p.alive)?;
        let eye = [owner.pos[0], owner.pos[1] + 1.62, owner.pos[2]];
        let survival = owner.survival();
        if !arrow.returning {
            arrow.returning = true;
            arrow.stuck = None;
            self.play_sound_at("minecraft:item.trident.return", arrow.pos, 10.0, 1.0, None);
            self.broadcast(
                Writer::packet(clientbound::SET_ENTITY_DATA)
                    .varint(id)
                    .u8(8)
                    .varint(0)
                    .u8(0x02) // no clip
                    .u8(0xFF),
                None,
            );
        }
        let to = [0, 1, 2].map(|i| eye[i] - arrow.pos[i]);
        let distance = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
        if distance < 1.5 {
            let pid = arrow.shooter;
            if arrow.pickup && survival {
                let p = self.players.get_mut(&pid).expect("checked");
                let (changed, left) = p.inventory.add(trident);
                let updates: Vec<Writer> = changed
                    .into_iter()
                    .map(|s| p.inventory.set_slot_packet(s))
                    .collect();
                for u in updates {
                    self.send(pid, u);
                }
                if left > 0 {
                    self.scatter_item(eye, trident);
                }
            }
            self.broadcast(
                Writer::packet(clientbound::TAKE_ITEM_ENTITY)
                    .varint(id)
                    .varint(pid)
                    .varint(1),
                None,
            );
            self.broadcast(super::items::remove_entity(id), None);
            return Some(true);
        }
        let pull = 0.05 * f64::from(loyalty);
        arrow.vel = [0, 1, 2].map(|i| arrow.vel[i] * 0.95 + to[i] / distance * pull);
        arrow.pos = [0, 1, 2].map(|i| arrow.pos[i] + arrow.vel[i]);
        self.broadcast(
            Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                .varint(id)
                .varint(0)
                .f64(arrow.pos[0])
                .f64(arrow.pos[1])
                .f64(arrow.pos[2])
                .f32(0.0)
                .f32(0.0)
                .bool(false),
            None,
        );
        Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::super::mobs::{Mob, MobKind};
    use super::*;
    use crate::connection::Profile;

    fn world_with_player(at: [f64; 3]) -> (World, i32, tokio::sync::mpsc::Receiver<Frame>) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-ranged-{}-{}",
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
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Archer".to_owned(),
            uuid: 5,
            properties: Vec::new(),
        };
        world.players.insert(id, Player::new(profile, out, at));
        (world, id, frames)
    }

    fn give(w: &mut World, id: i32, slot: usize, name: &str, count: i32) {
        let item = ItemInfo::id_of(name).unwrap();
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = Stack::new(item, count);
    }

    #[test]
    fn crossbows_load_and_shoot_three_with_multishot() {
        let (mut w, id, _frames) = world_with_player([2.5, -60.0, 2.5]);
        let slot = w.players[&id].hand_slot(0);
        give(&mut w, id, slot, "minecraft:crossbow", 1);
        w.players.get_mut(&id).unwrap().inventory.slots[slot]
            .enchants
            .set(
                crate::enchanting::enchantment_id("minecraft:multishot").unwrap(),
                1,
            );
        give(&mut w, id, OFFHAND, "minecraft:arrow", 5);
        assert!(w.start_ranged(id, 0));
        // Let go too soon: nothing loads.
        w.tick += 10;
        assert!(w.release_ranged(id));
        assert!(w.players[&id].inventory.slots[slot].charged.is_none());
        assert!(w.start_ranged(id, 0));
        w.tick += CROSSBOW_CHARGE;
        assert!(w.release_ranged(id));
        let loaded = w.players[&id].inventory.slots[slot]
            .charged
            .expect("loaded");
        assert_eq!(loaded.count, 3);
        assert_eq!(
            w.players[&id].inventory.slots[OFFHAND].count, 4,
            "one arrow for three"
        );
        // Saved and read back with its load.
        let s = w.players[&id].inventory.slots[slot];
        assert_eq!(Stack::from_nbt(&s.to_nbt()), s);
        let before = w.arrows.len();
        assert!(w.start_ranged(id, 0));
        assert_eq!(w.arrows.len(), before + 3);
        let s = w.players[&id].inventory.slots[slot];
        assert!(s.charged.is_none());
        assert_eq!(s.damage, 3);
    }

    #[test]
    fn thrown_tridents_hurt_and_loyal_ones_come_back() {
        let (mut w, id, _frames) = world_with_player([2.5, -60.0, 2.5]);
        let slot = w.players[&id].hand_slot(0);
        give(&mut w, id, slot, "minecraft:trident", 1);
        w.players.get_mut(&id).unwrap().inventory.slots[slot]
            .enchants
            .set(
                crate::enchanting::enchantment_id("minecraft:loyalty").unwrap(),
                3,
            );
        // A cow straight ahead (looking south, +z).
        let cow = next_entity_id();
        let mob = Mob::new(MobKind::Cow, [2.5, -60.0, 6.5], &mut w.rng);
        w.mobs.insert(cow, mob);
        w.players.get_mut(&id).unwrap().pitch = 15.0;
        assert!(w.start_ranged(id, 0));
        w.tick += TRIDENT_CHARGE;
        assert!(w.release_ranged(id));
        assert!(w.players[&id].inventory.slots[slot].is_empty(), "thrown");
        let full = MobKind::Cow.max_health();
        for _ in 0..200 {
            w.tick += 1;
            w.tick_arrows();
            if w.arrows.is_empty() {
                break;
            }
        }
        assert!(
            w.mobs[&cow].health <= full - TRIDENT_DAMAGE,
            "the cow was hit"
        );
        assert!(w.arrows.is_empty(), "the trident came back");
        let back = w.players[&id]
            .inventory
            .slots
            .iter()
            .any(|s| ItemInfo::get(s.item).is_some_and(|i| i.name == "minecraft:trident"));
        assert!(back);
    }
}
