//! Breeding: feeding animals, love mode, babies growing up, and animals following
//! players who hold their food.
//!
//! Behaviour follows the Minecraft Wiki: an adult fed its food enters love mode for 30
//! seconds; two of the same kind in love find each other and make a baby, which takes
//! 20 minutes to grow up (feeding it speeds that up by a tenth of what is left); the
//! parents cannot breed again for 5 minutes, and breeding drops 1 to 7 experience.
//! Animals within 10 blocks follow a player holding their food.

use super::mobs::{Mob, MobKind};
use super::*;

/// Ticks of love mode after feeding.
const LOVE_TICKS: u32 = 600;
/// A baby's age when born: it grows up when it reaches 0.
pub const BABY_AGE: i32 = -24_000;
/// Ticks before parents can breed again.
const BREED_COOLDOWN: i32 = 6000;
/// How far animals look for a partner, and how close they must get.
const PARTNER_RANGE: f64 = 8.0;
const MATING_DISTANCE: f64 = 2.5;
/// How far animals notice food in a player's hand, and how close they follow.
const TEMPT_RANGE: f64 = 10.0;
const TEMPT_STOP: f64 = 2.5;
const ENTITY_EVENT_LOVE_HEARTS: i8 = 18;
const DATA_BABY: u8 = 16;
const SERIALIZER_BOOLEAN: i32 = 8;

/// Whether `kind` eats (and breeds with) `item`.
pub fn is_food(kind: MobKind, item: &str) -> bool {
    match kind {
        MobKind::Cow | MobKind::Sheep => item == "minecraft:wheat",
        MobKind::Pig => matches!(
            item,
            "minecraft:carrot" | "minecraft:potato" | "minecraft:beetroot"
        ),
        MobKind::Chicken => matches!(
            item,
            "minecraft:wheat_seeds"
                | "minecraft:melon_seeds"
                | "minecraft:pumpkin_seeds"
                | "minecraft:beetroot_seeds"
                | "minecraft:torchflower_seeds"
                | "minecraft:pitcher_pod"
        ),
        _ => false,
    }
}

/// Entity data telling clients whether a mob is a baby.
pub fn baby_data(id: i32, baby: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_BABY)
        .varint(SERIALIZER_BOOLEAN)
        .bool(baby)
        .u8(0xFF)
}

impl World {
    /// A player feeds an animal its food: an adult falls in love, a baby grows up a bit.
    /// Returns whether the food was eaten.
    pub(super) fn feed_animal(&mut self, player: i32, target: i32, slot: usize) -> bool {
        let Some(mut mob) = self.mobs.remove(&target) else {
            return false;
        };
        let eaten = if mob.age < 0 {
            mob.age += -mob.age / 10;
            true
        } else if mob.age == 0 && mob.love == 0 {
            mob.love = LOVE_TICKS;
            self.send_to_viewers(&mob, entity_event(target, ENTITY_EVENT_LOVE_HEARTS));
            true
        } else {
            false
        };
        if eaten {
            let sound = format!("minecraft:entity.{}.ambient", mob.kind.sound());
            self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
        }
        self.mobs.insert(target, mob);
        if eaten && self.players.get(&player).is_some_and(|p| p.survival()) {
            let p = self.players.get_mut(&player).expect("player exists");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(player, update);
        }
        eaten
    }

    /// Growing up, cooling down after breeding, and looking for a partner while in love.
    pub(super) fn tick_breeding(&mut self, id: i32, mob: &mut Mob) {
        if mob.age < 0 {
            mob.age += 1;
            if mob.age == 0 {
                self.send_to_viewers(mob, baby_data(id, false));
            }
        } else if mob.age > 0 {
            mob.age -= 1;
        }
        if mob.love == 0 {
            return;
        }
        mob.love -= 1;
        if mob.love.is_multiple_of(10) {
            self.send_to_viewers(mob, entity_event(id, ENTITY_EVENT_LOVE_HEARTS));
        }
        let partner = self
            .mobs
            .iter()
            .filter(|(_, m)| m.kind == mob.kind && m.love > 0 && m.age == 0 && m.alive())
            .map(|(&pid, m)| (pid, distance(m.pos, mob.pos)))
            .filter(|&(_, d)| d <= PARTNER_RANGE)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((pid, d)) = partner else {
            return;
        };
        let partner_pos = self.mobs[&pid].pos;
        if d > MATING_DISTANCE {
            mob.target = Some(([partner_pos[0], partner_pos[2]], 20));
            return;
        }
        // A baby between the two parents, who then rest.
        let at = [
            (mob.pos[0] + partner_pos[0]) / 2.0,
            mob.pos[1].max(partner_pos[1]),
            (mob.pos[2] + partner_pos[2]) / 2.0,
        ];
        let mut baby = Mob::new(mob.kind, at, &mut self.rng);
        baby.age = BABY_AGE;
        if mob.kind == MobKind::Sheep {
            baby.color = if self.rng.below(2) == 0 {
                mob.color
            } else {
                self.mobs[&pid].color
            };
        }
        self.mobs.insert(next_entity_id(), baby);
        mob.love = 0;
        mob.age = BREED_COOLDOWN;
        mob.target = None;
        if let Some(partner) = self.mobs.get_mut(&pid) {
            partner.love = 0;
            partner.age = BREED_COOLDOWN;
            partner.target = None;
        }
        let xp = 1 + self.rng.below(7) as i32;
        self.spawn_xp(at, xp);
    }

    /// Where the nearest player holding this animal's food stands, if one is close.
    pub(super) fn tempting_player(&self, mob: &Mob) -> Option<[f64; 3]> {
        self.players
            .values()
            .filter(|p| p.alive)
            .filter(|p| {
                [p.hand_slot(0), p.hand_slot(1)].iter().any(|&slot| {
                    ItemInfo::get(p.inventory.slots[slot].item)
                        .is_some_and(|i| is_food(mob.kind, i.name))
                })
            })
            .map(|p| p.pos)
            .filter(|&pos| distance(pos, mob.pos) <= TEMPT_RANGE)
            .min_by(|a, b| distance(*a, mob.pos).total_cmp(&distance(*b, mob.pos)))
    }

    /// Steers a tempted animal towards the player, stopping a little short.
    pub(super) fn follow_temptation(&self, mob: &mut Mob) -> bool {
        let Some(pos) = self.tempting_player(mob) else {
            return false;
        };
        mob.target = (distance(pos, mob.pos) > TEMPT_STOP).then_some(([pos[0], pos[2]], 20));
        true
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}
