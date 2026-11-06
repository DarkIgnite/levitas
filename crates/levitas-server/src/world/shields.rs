//! Shields, following the Minecraft Wiki: held up (using the item), a shield blocks
//! attacks, arrows, fireballs and explosions from the front once it has been raised for
//! a quarter of a second. Blocking a hit of 3 or more wears it down; an axe blow
//! knocks it down for five seconds.

use super::*;

/// Ticks a shield must be up before it blocks.
const RAISE_TICKS: u64 = 5;
/// Ticks an axe keeps a shield down.
const DISABLED_TICKS: u64 = 100;
const ENTITY_EVENT_SHIELD_BLOCK: i8 = 29;
const ENTITY_EVENT_SHIELD_DISABLED: i8 = 30;
const ENTITY_EVENT_OFFHAND_BREAK: i8 = 48;
/// Living entity flags: whether a hand is in use, and which.
pub(super) const DATA_LIVING_FLAGS: u8 = 8;
pub(super) const SERIALIZER_BYTE: i32 = 0;
pub(super) const HAND_ACTIVE: u8 = 0x01;
pub(super) const OFFHAND_ACTIVE: u8 = 0x02;

/// How hard a shield pushes back a melee attacker it blocks.
const SHIELD_KNOCKBACK: f64 = 0.5;

fn is_shield(stack: Stack) -> bool {
    ItemInfo::get(stack.item).is_some_and(|i| i.name == "minecraft:shield")
}

impl World {
    /// Raises the shield in `hand`, if there is one. Returns whether there was.
    pub(super) fn raise_shield(&mut self, id: i32, hand: i32) -> bool {
        let tick = self.tick;
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        if !is_shield(p.inventory.slots[p.hand_slot(hand)]) {
            return false;
        }
        if p.alive && tick >= p.shield_ready {
            p.blocking = Some((hand, tick));
            let flags = HAND_ACTIVE | if hand == 1 { OFFHAND_ACTIVE } else { 0 };
            self.broadcast(living_flags(id, flags), Some(id));
        }
        true
    }

    /// Lowers a raised shield.
    pub(super) fn lower_shield(&mut self, id: i32) {
        if let Some(p) = self.players.get_mut(&id) {
            if p.blocking.take().is_some() {
                self.broadcast(living_flags(id, 0), Some(id));
            }
        }
    }

    /// Whether the player's shield takes a hit of `amount` coming from `from`. A
    /// blocked hit wears the shield; one from an axe (`axe`) knocks it down.
    pub(super) fn shield_blocks(
        &mut self,
        id: i32,
        amount: f32,
        from: [f64; 3],
        axe: bool,
    ) -> bool {
        let tick = self.tick;
        if !self.shield_faces(id, from) {
            return false;
        }
        let p = &self.players[&id];
        let (hand, _) = p.blocking.expect("checked by shield_faces");
        let slot = p.hand_slot(hand);
        let survival = p.survival();
        self.broadcast(entity_event(id, ENTITY_EVENT_SHIELD_BLOCK), None);
        if amount >= 3.0 && survival {
            let event = if hand == 1 {
                ENTITY_EVENT_OFFHAND_BREAK
            } else {
                ENTITY_EVENT_MAINHAND_BREAK
            };
            self.damage_item(id, slot, 1 + amount.floor() as i32, event);
        }
        if axe {
            self.lower_shield(id);
            let p = self.players.get_mut(&id).expect("player exists");
            p.shield_ready = tick + DISABLED_TICKS;
            self.broadcast(entity_event(id, ENTITY_EVENT_SHIELD_DISABLED), None);
            // Greys the shield out until it can be raised again.
            self.send(
                id,
                Writer::packet(clientbound::COOLDOWN)
                    .string("minecraft:shield")
                    .varint(DISABLED_TICKS as i32),
            );
        }
        true
    }

    /// Whether the player holds a raised shield towards `from` (raised long enough).
    pub(super) fn shield_faces(&self, id: i32, from: [f64; 3]) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let Some((hand, since)) = p.blocking else {
            return false;
        };
        let slot = p.hand_slot(hand);
        if self.tick < since + RAISE_TICKS || !is_shield(p.inventory.slots[slot]) {
            return false;
        }
        // Only hits from in front: the way to the source lies along the player's view.
        let yaw = f64::from(p.yaw).to_radians();
        let look = [-yaw.sin(), yaw.cos()];
        let to = [from[0] - p.pos[0], from[2] - p.pos[2]];
        to[0] * look[0] + to[1] * look[1] > 0.0
    }

    /// A melee attacker whose blow a shield stopped is pushed back.
    pub(super) fn shield_knockback(&mut self, id: i32, attacker: i32) {
        let Some(at) = self.players.get(&id).map(|p| p.pos) else {
            return;
        };
        let push = |from: [f64; 3]| {
            let d = [from[0] - at[0], from[2] - at[2]];
            let len = d[0].hypot(d[1]).max(1e-6);
            [
                d[0] / len * SHIELD_KNOCKBACK,
                0.2,
                d[1] / len * SHIELD_KNOCKBACK,
            ]
        };
        if let Some(m) = self.mobs.get_mut(&attacker) {
            let v = push(m.pos);
            m.push(v);
        } else if let Some(from) = self.players.get(&attacker).map(|p| p.pos) {
            if attacker != id {
                self.push_player(attacker, push(from));
            }
        }
    }

    /// Where an entity that caused damage is, and whether it swung an axe.
    pub(super) fn attacker(&self, source: i32) -> Option<([f64; 3], bool)> {
        if let Some(p) = self.players.get(&source) {
            let held = p.inventory.slots[p.hand_slot(0)];
            let axe = ItemInfo::get(held.item).is_some_and(|i| i.name.ends_with("_axe"));
            return Some((p.pos, axe));
        }
        self.mobs.get(&source).map(|m| (m.pos, false))
    }
}

pub(super) fn living_flags(id: i32, flags: u8) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_LIVING_FLAGS)
        .varint(SERIALIZER_BYTE)
        .u8(flags)
        .u8(0xFF)
}

#[cfg(test)]
mod tests {
    use super::survival::Damage;
    use super::*;
    use crate::connection::Profile;

    fn player(w: &mut World, name: &str, at: [f64; 3]) -> i32 {
        let (out, frames) = mpsc::channel(100_000);
        std::mem::forget(frames); // keep the channel open
        let id = next_entity_id();
        let profile = Profile {
            name: name.to_owned(),
            uuid: id as u128,
            properties: Vec::new(),
        };
        w.players.insert(id, Player::new(profile, out, at));
        id
    }

    fn item(name: &str) -> Stack {
        Stack::new(ItemInfo::id_of(&format!("minecraft:{name}")).unwrap(), 1)
    }

    #[test]
    fn raised_shields_block_hits_from_the_front() {
        let dir = std::env::temp_dir().join(format!("levitas-shield-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        // Alice looks south (+z), a shield in her offhand; Bob stands in front of her.
        let alice = player(&mut w, "Alice", [0.5, -60.0, 0.5]);
        let bob = player(&mut w, "Bob", [0.5, -60.0, 2.5]);
        w.players.get_mut(&alice).unwrap().inventory.slots[OFFHAND] = item("shield");
        let hit = |w: &mut World, by: i32| {
            w.players.get_mut(&alice).unwrap().hurt_ticks = 0;
            w.damage_from(alice, 4.0, Damage::Player(by), Some(by))
        };

        assert!(w.raise_shield(alice, 1));
        assert!(hit(&mut w, bob), "not up long enough yet");
        w.tick += RAISE_TICKS;
        let health = w.players[&alice].health;
        assert!(!hit(&mut w, bob), "blocked");
        assert_eq!(w.players[&alice].health, health);
        assert_eq!(w.players[&alice].inventory.slots[OFFHAND].damage, 5);

        // From behind the shield does nothing.
        let carol = player(&mut w, "Carol", [0.5, -60.0, -2.5]);
        assert!(hit(&mut w, carol));

        // An axe knocks the shield down for five seconds.
        let held = w.players[&bob].hand_slot(0);
        w.players.get_mut(&bob).unwrap().inventory.slots[held] = item("iron_axe");
        assert!(!hit(&mut w, bob));
        assert!(w.players[&alice].blocking.is_none());
        w.raise_shield(alice, 1);
        assert!(w.players[&alice].blocking.is_none(), "still knocked down");
        w.tick += DISABLED_TICKS;
        w.raise_shield(alice, 1);
        assert!(w.players[&alice].blocking.is_some());
    }

    #[test]
    fn blocked_blows_push_back_and_boosts_and_chorus_work() {
        let dir = std::env::temp_dir().join(format!("levitas-shield2-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let alice = player(&mut w, "Alice", [0.5, -60.0, 0.5]);
        w.players.get_mut(&alice).unwrap().inventory.slots[OFFHAND] = item("shield");
        assert!(w.raise_shield(alice, 1));
        w.tick += RAISE_TICKS;
        let zombie = next_entity_id();
        let m = super::super::mobs::Mob::new(
            super::super::mobs::MobKind::Zombie,
            [0.5, -60.0, 2.0],
            &mut w.rng,
        );
        w.mobs.insert(zombie, m);
        assert!(!w.damage_from(alice, 3.0, Damage::Mob("Zombie"), Some(zombie)));
        assert!(
            w.mobs[&zombie].knockback[1] > 0.0,
            "the zombie is pushed away"
        );

        // Health Boost II: 8 more health to fill.
        w.add_effect(
            alice,
            levitas_registry::Effect {
                id: levitas_registry::mob_effect::HEALTH_BOOST,
                duration: 200,
                amplifier: 1,
            },
        );
        assert_eq!(w.players[&alice].max_health(), 28.0);
        w.heal(alice, 100.0);
        assert_eq!(w.players[&alice].health, 28.0);

        // Chorus fruit lands the eater somewhere else, on their feet.
        let before = w.players[&alice].pos;
        w.teleport_randomly(alice, 16.0);
        let after = w.players[&alice].pos;
        assert!(before != after);
        assert!((after[0] - before[0]).abs() <= 8.0 && (after[2] - before[2]).abs() <= 8.0);
    }
}
