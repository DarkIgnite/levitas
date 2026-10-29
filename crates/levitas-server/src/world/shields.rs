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
const DATA_LIVING_FLAGS: u8 = 8;
const SERIALIZER_BYTE: i32 = 0;
const HAND_ACTIVE: u8 = 0x01;
const OFFHAND_ACTIVE: u8 = 0x02;

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
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let Some((hand, since)) = p.blocking else {
            return false;
        };
        let slot = p.hand_slot(hand);
        if tick < since + RAISE_TICKS || !is_shield(p.inventory.slots[slot]) {
            return false;
        }
        // Only hits from in front: the way to the source lies along the player's view.
        let yaw = f64::from(p.yaw).to_radians();
        let look = [-yaw.sin(), yaw.cos()];
        let to = [from[0] - p.pos[0], from[2] - p.pos[2]];
        if to[0] * look[0] + to[1] * look[1] <= 0.0 {
            return false;
        }
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

fn living_flags(id: i32, flags: u8) -> Writer {
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
}
