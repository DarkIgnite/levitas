//! Piglins, piglin brutes and hoglins.
//!
//! Behaviour follows the Minecraft Wiki: piglins attack players who wear no gold armour
//! and leave the others be, unless angered by a hit, by a player opening or breaking
//! something they guard (`minecraft:guarded_by_piglins`) within 16 blocks, or by a
//! neighbour being hit. Given a gold ingot (by hand, or picked up from the ground), an
//! adult piglin admires it for six seconds and then throws out a barter from
//! `gameplay/piglin_bartering`. Half of them carry crossbows. Outside the Nether
//! piglins and brutes turn into zombified piglins, and hoglins into zoglins, after 15
//! seconds.

use super::mobs::{Mob, MobKind};
use super::*;

/// How long a piglin admires gold before it barters.
const ADMIRE_TICKS: u32 = 120;
/// How long a piglin, brute or hoglin lasts outside the Nether.
const ZOMBIFY_TICKS: u32 = 300;
/// How far piglins notice a player meddling with what they guard.
const GUARD_RANGE: f64 = 16.0;
/// How long a piglin stays angry.
const ANGER_MIN: u32 = 400;
const ANGER_SPREAD: u32 = 380;
/// Variant of a piglin that carries a crossbow (others carry a golden sword).
pub const CROSSBOW: u8 = 1;

/// Whether it is one of the piglins.
pub fn is_piglin(kind: MobKind) -> bool {
    matches!(kind, MobKind::Piglin | MobKind::PiglinBrute)
}

/// What a piglin holds: a crossbow or a golden sword; a brute's golden axe.
pub fn weapon(mob: &Mob) -> Option<&'static str> {
    match mob.kind {
        MobKind::Piglin if mob.variant == CROSSBOW => Some("minecraft:crossbow"),
        MobKind::Piglin => Some("minecraft:golden_sword"),
        MobKind::PiglinBrute => Some("minecraft:golden_axe"),
        _ => None,
    }
}

/// Whether a player wears at least one piece of gold armour.
pub fn wears_gold(p: &Player) -> bool {
    (5..=8).any(|slot| {
        ItemInfo::get(p.inventory.slots[slot].item).is_some_and(|i| {
            levitas_registry::item_has_tag(i.name, "minecraft:piglin_safe_armor")
                || i.name.starts_with("minecraft:golden_")
        })
    })
}

/// The item shown in a piglin's off hand while it admires gold.
fn offhand_packet(id: i32, item: Option<i32>) -> Writer {
    let packet = Writer::packet(clientbound::SET_EQUIPMENT).varint(id).u8(1);
    crate::inventory::write_stack(packet, item.map_or(Stack::EMPTY, |i| Stack::new(i, 1)))
}

impl World {
    /// Whether a piglin leaves player `pid` alone: one wearing gold, unless it is angry.
    pub(super) fn piglin_spares(&self, mob: &Mob, pid: i32) -> bool {
        mob.kind == MobKind::Piglin
            && mob.anger == 0
            && self.players.get(&pid).is_some_and(wears_gold)
    }

    /// A player hands a piglin a gold ingot. Returns whether it took it.
    pub(super) fn give_piglin_gold(&mut self, id: i32, target: i32, slot: usize) -> bool {
        let Some(mob) = self.mobs.get(&target) else {
            return false;
        };
        if mob.kind != MobKind::Piglin || mob.age < 0 || mob.admiring.is_some() || mob.anger > 0 {
            return false;
        }
        let Some(gold) = ItemInfo::id_of("minecraft:gold_ingot") else {
            return false;
        };
        if self.players.get(&id).is_some_and(|p| p.survival()) {
            let p = self.players.get_mut(&id).expect("checked");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        self.start_admiring(target, gold);
        true
    }

    fn start_admiring(&mut self, target: i32, item: i32) {
        let Some(mut mob) = self.mobs.remove(&target) else {
            return;
        };
        mob.admiring = Some((ADMIRE_TICKS, item));
        mob.hunting = None;
        mob.target = None;
        mob.path.clear();
        self.send_to_viewers(&mob, offhand_packet(target, Some(item)));
        self.play_sound_at(
            "minecraft:entity.piglin.admiring_item",
            mob.pos,
            1.0,
            1.0,
            None,
        );
        self.mobs.insert(target, mob);
    }

    /// A piglin's own turn, before it hunts: admiring and bartering, picking up gold,
    /// cooling down. Returns whether it is busy admiring (and so neither moves nor
    /// fights).
    pub(super) fn tick_piglin(&mut self, id: i32, mob: &mut Mob) -> bool {
        if let Some((ticks, item)) = mob.admiring.as_mut() {
            *ticks = ticks.saturating_sub(1);
            mob.target = None;
            if *ticks > 0 {
                return true;
            }
            let item = *item;
            mob.admiring = None;
            self.send_to_viewers(mob, offhand_packet(id, None));
            if ItemInfo::get(item).is_some_and(|i| i.name == "minecraft:gold_ingot") {
                self.barter(mob);
            }
            return false;
        }
        if mob.kind != MobKind::Piglin || mob.age < 0 || mob.anger > 0 {
            return false;
        }
        // Gold on the ground nearby is picked up and admired.
        let gold = ItemInfo::id_of("minecraft:gold_ingot");
        let found = self.items.iter().find_map(|(&iid, item)| {
            let close = (0..3).all(|i| (item.pos()[i] - mob.pos[i]).abs() <= 1.5);
            (close && Some(item.stack_ref().item) == gold).then_some(iid)
        });
        if let (Some(iid), Some(gold)) = (found, gold) {
            self.take_one_item(iid, id);
            mob.admiring = Some((ADMIRE_TICKS, gold));
            mob.hunting = None;
            self.send_to_viewers(mob, offhand_packet(id, Some(gold)));
            self.play_sound_at(
                "minecraft:entity.piglin.admiring_item",
                mob.pos,
                1.0,
                1.0,
                None,
            );
            return true;
        }
        false
    }

    /// The barter: items from the bartering table, thrown towards the nearest player.
    fn barter(&mut self, mob: &Mob) {
        let mut ctx = crate::loot::LootContext {
            state: 0,
            tool: 0,
            rng: &mut self.rng,
            enchants: crate::enchanting::Enchantments::NONE,
            entity: Default::default(),
        };
        let items = self.loot.gameplay("minecraft:piglin_bartering", &mut ctx);
        let toward = self
            .players
            .values()
            .filter(|p| p.alive)
            .map(|p| p.pos)
            .min_by(|a, b| {
                let d = |q: &[f64; 3]| (q[0] - mob.pos[0]).hypot(q[2] - mob.pos[2]);
                d(a).total_cmp(&d(b))
            });
        let from = [mob.pos[0], mob.pos[1] + 1.0, mob.pos[2]];
        for stack in items {
            let vel = match toward {
                Some(t) => {
                    let (dx, dz) = (t[0] - from[0], t[2] - from[2]);
                    let len = dx.hypot(dz).max(1e-6);
                    [dx / len * 0.3, 0.2, dz / len * 0.3]
                }
                None => [0.0, 0.2, 0.0],
            };
            self.spawn_item_moving(stack, from, vel);
        }
    }

    /// A player hit a piglin: every piglin and brute within 16 blocks turns on them.
    pub(super) fn piglins_angered_at(&mut self, at: [f64; 3], attacker: i32) {
        let ids: Vec<i32> = self
            .mobs
            .iter()
            .filter(|(_, m)| {
                is_piglin(m.kind)
                    && m.alive()
                    && (m.pos[0] - at[0]).hypot(m.pos[2] - at[2]) <= GUARD_RANGE
            })
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let anger = ANGER_MIN + self.rng.below(ANGER_SPREAD);
            if let Some(mut m) = self.mobs.remove(&id) {
                m.anger = anger;
                m.hunting = Some(attacker);
                if m.admiring.take().is_some() {
                    self.send_to_viewers(&m, offhand_packet(id, None));
                }
                self.mobs.insert(id, m);
            }
        }
    }

    /// A player opened or broke a block piglins guard (chests, gold blocks, gold ore):
    /// piglins around turn on them.
    pub(super) fn piglins_guard(&mut self, id: i32, pos: BlockPos, state: u16) {
        if !block_has_tag(state, "minecraft:guarded_by_piglins") {
            return;
        }
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let near = self.mobs.values().any(|m| {
            m.kind == MobKind::Piglin
                && m.alive()
                && (0..3)
                    .map(|i| (m.pos[i] - at[i]).powi(2))
                    .sum::<f64>()
                    .sqrt()
                    <= GUARD_RANGE
        });
        if near && self.players.get(&id).is_some_and(|p| p.survival()) {
            self.piglins_angered_at(at, id);
        }
    }

    /// Outside the Nether, piglins, brutes and hoglins shake for 15 seconds and turn
    /// into their zombified kin. Returns the new mob once it turned.
    pub(super) fn zombify(&mut self, id: i32, mob: &mut Mob) {
        let turns_into = match mob.kind {
            MobKind::Piglin | MobKind::PiglinBrute => MobKind::ZombifiedPiglin,
            MobKind::Hoglin => MobKind::Zoglin,
            _ => return,
        };
        if self.dimension == super::dimension::Dimension::Nether {
            mob.converting = None;
            return;
        }
        let left = mob.converting.get_or_insert(ZOMBIFY_TICKS);
        *left = left.saturating_sub(1);
        if *left > 0 {
            return;
        }
        let mut new = Mob::new(turns_into, mob.pos, &mut self.rng);
        new.uuid = mob.uuid;
        new.name = mob.name;
        new.yaw = mob.yaw;
        new.head_yaw = mob.head_yaw;
        new.age = mob.age.min(0);
        new.viewers = std::mem::take(&mut mob.viewers);
        self.send_to_viewers(&new, super::items::remove_entity(id));
        for p in new.spawn_packets(id) {
            self.send_to_viewers(&new, p);
        }
        let sound = format!(
            "minecraft:entity.{}.converted_to_zombified",
            mob.kind.sound()
        );
        self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
        *mob = new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-piglin-{}-{}",
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
        for cx in -1..=1 {
            for cz in -1..=1 {
                w.chunk(cx, cz);
            }
        }
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 9,
            properties: Vec::new(),
        };
        let mut p = Player::new(profile, out, [2.5, -60.0, 2.5]);
        p.inventory.slots[36] = Stack::new(ItemInfo::id_of("minecraft:gold_ingot").unwrap(), 4);
        w.players.insert(alice, p);
        (w, alice)
    }

    #[test]
    fn piglins_spare_gold_wearers_unless_angered() {
        let (mut w, alice) = world_with_player();
        let piglin = Mob::new(MobKind::Piglin, [4.5, -60.0, 2.5], &mut w.rng);
        assert!(!w.piglin_spares(&piglin, alice), "no gold, no mercy");
        let helmet = ItemInfo::id_of("minecraft:golden_helmet").unwrap();
        w.players.get_mut(&alice).unwrap().inventory.slots[5] = Stack::new(helmet, 1);
        assert!(w.piglin_spares(&piglin, alice));
        let id = next_entity_id();
        w.mobs.insert(id, piglin);
        w.piglins_angered_at([4.5, -60.0, 2.5], alice);
        assert!(!w.piglin_spares(&w.mobs[&id], alice), "angry now");
        assert_eq!(w.mobs[&id].hunting, Some(alice));
        // A brute never spares anyone.
        let brute = Mob::new(MobKind::PiglinBrute, [4.5, -60.0, 2.5], &mut w.rng);
        assert!(!w.piglin_spares(&brute, alice));
    }

    #[test]
    fn piglins_barter_gold_after_admiring_it() {
        let (mut w, alice) = world_with_player();
        let id = next_entity_id();
        let mut piglin = Mob::new(MobKind::Piglin, [4.5, -60.0, 2.5], &mut w.rng);
        piglin.age = 0;
        w.mobs.insert(id, piglin);
        assert!(w.give_piglin_gold(alice, id, 36));
        assert_eq!(w.players[&alice].inventory.slots[36].count, 3);
        assert!(w.mobs[&id].admiring.is_some());
        let before = w.items.len();
        for _ in 0..ADMIRE_TICKS {
            let mut m = w.mobs.remove(&id).unwrap();
            w.tick_piglin(id, &mut m);
            w.mobs.insert(id, m);
        }
        assert!(w.mobs[&id].admiring.is_none());
        assert!(w.items.len() > before, "a barter was thrown out");
    }

    #[test]
    fn piglins_and_hoglins_zombify_outside_the_nether() {
        let (mut w, _) = world_with_player();
        for (kind, into) in [
            (MobKind::Piglin, MobKind::ZombifiedPiglin),
            (MobKind::PiglinBrute, MobKind::ZombifiedPiglin),
            (MobKind::Hoglin, MobKind::Zoglin),
        ] {
            let mut mob = Mob::new(kind, [4.5, -60.0, 2.5], &mut w.rng);
            let uuid = mob.uuid;
            for _ in 0..ZOMBIFY_TICKS - 1 {
                w.zombify(1, &mut mob);
            }
            assert_eq!(mob.kind, kind, "still shaking");
            w.zombify(1, &mut mob);
            assert_eq!(mob.kind, into);
            assert_eq!(mob.uuid, uuid);
        }
    }
}
