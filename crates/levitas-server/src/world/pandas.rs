//! Pandas: genes and personalities, sneezing, eating, rolling, and their temper.
//!
//! Behaviour follows the Minecraft Wiki (Panda):
//! - Each panda has a main and a hidden gene, each rolled as a mutated gene is: normal
//!   5/16, aggressive, lazy, worried and playful 1/16 each, weak 5/16, brown 2/16.
//!   Normal, aggressive, lazy, worried and playful are dominant: a main gene of one of
//!   them is the personality. Weak and brown are recessive: they show only when both
//!   genes are the same, otherwise the panda is normal. A baby gets one gene from each
//!   parent at random, and each of its genes mutates one time in 32.
//! - Five pandas in a hundred spawn as babies. Weak pandas have half the health.
//! - Lazy pandas lie on their backs now and then; playful ones (and babies) roll over;
//!   worried ones keep away from players and most monsters, and keep still, hiding,
//!   in thunderstorms.
//! - Hurt by a player, an adult panda hits back once; an aggressive one keeps at it
//!   until the player is dead or out of range, does not panic, and aggressive pandas
//!   nearby join in when another panda is hurt. Babies panic. On Peaceful they are
//!   passive.
//! - Baby pandas sneeze one tick in 6000 (weak babies one in 500); a sneeze makes adult
//!   pandas within 10 blocks jump, and drops a slimeball one time in 700.
//! - Adult pandas that are not worried pick up bamboo or cake lying nearby and sit to
//!   eat it.
//! - In Java Edition, pandas fed bamboo fall in love, but breed only with a bamboo plant
//!   (not a shoot) within a 7x7x3 area; otherwise they shake their heads. If only one
//!   of the two has bamboo by it, the other shakes its head but a baby is still born.
//!
//! Entity data (26.3, after Ageable Mob's baby 16 and age locked 17; Animal adds none):
//! breed timer 18, sneeze timer 19, eat timer 20, main gene 21, hidden gene 22 (bytes),
//! flags 23 (sneezing, rolling, sitting, on back).
//! Saved as vanilla: `MainGene` and `HiddenGene`, by name.

use std::cell::Cell;

use levitas_nbt::Compound;

use super::mobs::{Mob, MobKind};
use super::*;
use crate::world::survival::Damage;

const DATA_BREED_TIMER: u8 = 18;
const DATA_SNEEZE_TIMER: u8 = 19;
const DATA_EAT_TIMER: u8 = 20;
const DATA_MAIN_GENE: u8 = 21;
const DATA_HIDDEN_GENE: u8 = 22;
const DATA_FLAGS: u8 = 23;
const SERIALIZER_BYTE: i32 = 0;
const SERIALIZER_INT: i32 = 1;
const FLAG_SNEEZING: u8 = 0x02;
const FLAG_ROLLING: u8 = 0x04;
const FLAG_SITTING: u8 = 0x08;
const FLAG_ON_BACK: u8 = 0x10;
/// Equipment slot of the main hand, where a panda holds what it eats.
const SLOT_MAINHAND: u8 = 0;

/// Ticks of head shaking when it cannot breed; the sound plays at these two points.
const BREED_SHAKE: u8 = 32;
const BREED_SOUNDS: [u8; 2] = [29, 14];
/// A panda in love without bamboo shakes its head at most this often.
const SHAKE_COOLDOWN: u32 = 600;
/// How often a panda in love looks again for bamboo growing about.
const BAMBOO_CHECK: u32 = 20;
/// How often a panda looks about for food and for things to be scared of.
const SCAN_TICKS: u8 = 10;
/// A sneeze: the nose tickles at 1 and it sneezes at 21.
const SNEEZE_AT: u8 = 21;
const SNEEZE_CHANCE: u32 = 6000;
const WEAK_SNEEZE_CHANCE: u32 = 500;
const SLIMEBALL_CHANCE: u32 = 700;
const SNEEZE_JUMP_RANGE: f64 = 10.0;
const JUMP_SPEED: f64 = 0.42;
/// How long it sits eating, and how long before it looks for food again.
const EAT_TICKS: u32 = 200;
const EAT_COOLDOWN: u32 = 400;
/// How far it notices food lying about, and how close it must be to take it.
const FOOD_RANGE: f64 = 6.0;
const FOOD_REACH: f64 = 1.5;
/// Lazy pandas: chance per tick of lying down, and for how long.
const LAZY_CHANCE: u32 = 800;
const ON_BACK_TICKS: u32 = 200;
/// Rolling: chance per tick for playful pandas and for babies, and for how long.
const PLAYFUL_ROLL_CHANCE: u32 = 400;
const BABY_ROLL_CHANCE: u32 = 1200;
const ROLL_TICKS: u32 = 32;
/// Worried pandas keep this far from players and monsters.
const WORRY_RANGE: f64 = 8.0;
/// A panda after its attacker gives up beyond this distance.
const CHASE_RANGE: f64 = 16.0;
const ATTACK_REACH: f64 = 2.0;
const ATTACK_COOLDOWN: u32 = 20;
/// Aggressive pandas this close join a fight when another panda is hurt.
const HELP_RANGE: f64 = 10.0;
const BABY_AGE: i32 = -24_000;
/// Breeding sets the parents resting for 6000 ticks: a baby born next to pandas that
/// rested this long or longer is theirs.
const JUST_BRED: i32 = 5_990;
const PARENT_RANGE: f64 = 4.0;
const BABY_CHANCE: u32 = 20;
const MUTATION_CHANCE: u32 = 32;
pub const WEAK_HEALTH: f32 = 10.0;
/// Monsters worried pandas are not scared of.
const NOT_SCARY: [&str; 11] = [
    "minecraft:ghast",
    "minecraft:hoglin",
    "minecraft:magma_cube",
    "minecraft:phantom",
    "minecraft:shulker",
    "minecraft:slime",
    "minecraft:skeleton_horse",
    "minecraft:zombie_horse",
    "minecraft:zombie_nautilus",
    "minecraft:camel_husk",
    "minecraft:ender_dragon",
];

/// A panda gene, in the order the client numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gene {
    Normal,
    Lazy,
    Worried,
    Playful,
    Brown,
    Weak,
    Aggressive,
}

const GENES: [Gene; 7] = [
    Gene::Normal,
    Gene::Lazy,
    Gene::Worried,
    Gene::Playful,
    Gene::Brown,
    Gene::Weak,
    Gene::Aggressive,
];

impl Gene {
    pub fn name(self) -> &'static str {
        match self {
            Gene::Normal => "normal",
            Gene::Lazy => "lazy",
            Gene::Worried => "worried",
            Gene::Playful => "playful",
            Gene::Brown => "brown",
            Gene::Weak => "weak",
            Gene::Aggressive => "aggressive",
        }
    }

    pub fn from_name(name: &str) -> Option<Gene> {
        GENES.into_iter().find(|g| g.name() == name)
    }

    fn id(self) -> u8 {
        self as u8
    }

    fn recessive(self) -> bool {
        matches!(self, Gene::Weak | Gene::Brown)
    }

    /// A gene rolled as a mutation (and as a spawning panda's genes) is.
    pub fn roll(rng: &mut Rng) -> Gene {
        match rng.below(16) {
            0..=4 => Gene::Normal,
            5 => Gene::Aggressive,
            6 => Gene::Lazy,
            7 => Gene::Worried,
            8 => Gene::Playful,
            9..=13 => Gene::Weak,
            _ => Gene::Brown,
        }
    }
}

/// The personality two genes show.
pub fn personality(main: Gene, hidden: Gene) -> Gene {
    if !main.recessive() || main == hidden {
        main
    } else {
        Gene::Normal
    }
}

/// A baby's genes: one from each parent's pair, in random order, each mutating one
/// time in 32.
pub fn inherit(a: (Gene, Gene), b: (Gene, Gene), rng: &mut Rng) -> (Gene, Gene) {
    let pick = |pair: (Gene, Gene), rng: &mut Rng| {
        if rng.below(2) == 0 {
            pair.0
        } else {
            pair.1
        }
    };
    let from_a = pick(a, rng);
    let from_b = pick(b, rng);
    let (mut main, mut hidden) = if rng.below(2) == 0 {
        (from_a, from_b)
    } else {
        (from_b, from_a)
    };
    if rng.below(MUTATION_CHANCE) == 0 {
        main = Gene::roll(rng);
    }
    if rng.below(MUTATION_CHANCE) == 0 {
        hidden = Gene::roll(rng);
    }
    (main, hidden)
}

/// What viewers were last shown of a panda.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shown {
    flags: u8,
    shaking: bool,
    eating: Option<i32>,
}

/// A panda's own state, kept in `Mob::kin`.
#[derive(Debug)]
pub struct Panda {
    pub main: Gene,
    pub hidden: Gene,
    /// Ticks of head shaking left (it could not breed).
    breed_timer: u8,
    /// Ticks into a sneeze, or 0.
    sneeze: u8,
    /// Ticks spent eating, or 0, and the item in its paws.
    eat: u32,
    eating: Option<i32>,
    eat_cooldown: u32,
    /// Ticks until it next looks about, and what it saw then: food lying near (entity,
    /// item, position) and the nearest threat, for a worried panda.
    scan: u8,
    food: Option<(i32, i32, [f64; 3])>,
    threat: Option<[f64; 3]>,
    /// In love: ticks until it looks for bamboo again and what it found, and ticks
    /// before it may shake its head again.
    bamboo_check: u32,
    bamboo_ok: bool,
    shake_cooldown: u32,
    /// Ticks left lying on its back, and of rolling over.
    on_back: u32,
    roll: u32,
    /// In love with no bamboo growing nearby: it shakes its head on its next turn.
    refused: Cell<bool>,
    sent: Option<Shown>,
}

impl Panda {
    pub fn new(main: Gene, hidden: Gene) -> Panda {
        Panda {
            main,
            hidden,
            breed_timer: 0,
            sneeze: 0,
            eat: 0,
            eating: None,
            eat_cooldown: 0,
            scan: 0,
            food: None,
            threat: None,
            bamboo_check: 0,
            bamboo_ok: false,
            shake_cooldown: 0,
            on_back: 0,
            roll: 0,
            refused: Cell::new(false),
            sent: None,
        }
    }

    pub fn personality(&self) -> Gene {
        personality(self.main, self.hidden)
    }

    fn flags(&self) -> u8 {
        let mut flags = 0;
        if self.sneeze > 0 {
            flags |= FLAG_SNEEZING;
        }
        if self.roll > 0 {
            flags |= FLAG_ROLLING;
        }
        if self.eat > 0 {
            flags |= FLAG_SITTING;
        }
        if self.on_back > 0 {
            flags |= FLAG_ON_BACK;
        }
        flags
    }

    fn shown(&self) -> Shown {
        Shown {
            flags: self.flags(),
            shaking: self.breed_timer > 0,
            eating: self.eating,
        }
    }

    /// Gets up: stops lying, rolling and eating.
    fn stand_up(&mut self) {
        self.on_back = 0;
        self.roll = 0;
        self.eat = 0;
        self.eating = None;
    }
}

/// A panda's state, if it has any yet.
pub fn panda(mob: &Mob) -> Option<&Panda> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Panda>())
}

fn panda_mut(mob: &mut Mob) -> Option<&mut Panda> {
    mob.kin.as_mut().and_then(|k| k.downcast_mut::<Panda>())
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn panda_data(id: i32, p: &Panda) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_BREED_TIMER)
        .varint(SERIALIZER_INT)
        .varint(i32::from(p.breed_timer))
        .u8(DATA_SNEEZE_TIMER)
        .varint(SERIALIZER_INT)
        .varint(i32::from(p.sneeze))
        .u8(DATA_EAT_TIMER)
        .varint(SERIALIZER_INT)
        .varint(p.eat as i32)
        .u8(DATA_MAIN_GENE)
        .varint(SERIALIZER_BYTE)
        .u8(p.main.id())
        .u8(DATA_HIDDEN_GENE)
        .varint(SERIALIZER_BYTE)
        .u8(p.hidden.id())
        .u8(DATA_FLAGS)
        .varint(SERIALIZER_BYTE)
        .u8(p.flags())
        .u8(0xFF)
}

fn held_packet(id: i32, item: Option<i32>) -> Writer {
    let packet = Writer::packet(clientbound::SET_EQUIPMENT)
        .varint(id)
        .u8(SLOT_MAINHAND);
    crate::inventory::write_stack(packet, item.map_or(Stack::EMPTY, |i| Stack::new(i, 1)))
}

/// Gives a fresh panda its state; weak pandas have half the health.
fn give_genes(mob: &mut Mob, main: Gene, hidden: Gene) {
    let p = Panda::new(main, hidden);
    if p.personality() == Gene::Weak {
        mob.health = mob.health.min(WEAK_HEALTH);
    }
    mob.kin = Some(Box::new(p));
}

pub(super) fn spawned(w: &mut World, mob: &mut Mob, _biome: &str) {
    let main = Gene::roll(&mut w.rng);
    let hidden = Gene::roll(&mut w.rng);
    give_genes(mob, main, hidden);
    if w.rng.below(BABY_CHANCE) == 0 {
        mob.age = BABY_AGE;
    }
}

/// The state of a panda that has none yet: a baby just born next to its parents takes
/// a gene from each, any other panda rolls its own.
fn fresh(w: &mut World, mob: &mut Mob) {
    let parents: Vec<(Gene, Gene)> = if mob.age <= BABY_AGE + 20 {
        let mut near: Vec<(f64, (Gene, Gene))> = w
            .mobs
            .values()
            .filter(|m| m.kind == MobKind::Panda && m.age >= JUST_BRED && m.alive())
            .filter_map(|m| {
                let d = distance(m.pos, mob.pos);
                let p = panda(m)?;
                (d <= PARENT_RANGE).then_some((d, (p.main, p.hidden)))
            })
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        near.into_iter().take(2).map(|(_, g)| g).collect()
    } else {
        Vec::new()
    };
    let (main, hidden) = match parents[..] {
        [a, b] => inherit(a, b, &mut w.rng),
        _ => (Gene::roll(&mut w.rng), Gene::roll(&mut w.rng)),
    };
    give_genes(mob, main, hidden);
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if panda(mob).is_none() {
        fresh(w, mob);
    }
    let Some(mut p) = mob.kin.take().and_then(|k| k.downcast::<Panda>().ok()) else {
        return false;
    };
    let busy = tick_panda(w, id, mob, &mut p);
    let now = p.shown();
    if p.sent != Some(now) {
        if p.sent.map(|s| s.eating) != Some(now.eating) {
            w.send_to_viewers(mob, held_packet(id, now.eating));
        }
        w.send_to_viewers(mob, panda_data(id, &p));
        p.sent = Some(now);
    }
    mob.kin = Some(p);
    busy
}

/// A panda's turn. Returns whether it keeps still.
fn tick_panda(w: &mut World, id: i32, mob: &mut Mob, p: &mut Panda) -> bool {
    let kind = p.personality();
    let adult = mob.age >= 0;
    mob.attack_cooldown = mob.attack_cooldown.saturating_sub(1);
    p.eat_cooldown = p.eat_cooldown.saturating_sub(1);
    p.shake_cooldown = p.shake_cooldown.saturating_sub(1);
    p.bamboo_check = p.bamboo_check.saturating_sub(1);
    // Looking about is not cheap: only every few ticks.
    let scan = p.scan == 0;
    if scan {
        p.scan = SCAN_TICKS;
    } else {
        p.scan -= 1;
    }

    // Shaking its head: in love with no bamboo about.
    if p.refused.take() && p.breed_timer == 0 {
        p.breed_timer = BREED_SHAKE;
    }
    if p.breed_timer > 0 {
        if BREED_SOUNDS.contains(&p.breed_timer) {
            w.play_sound_at("minecraft:entity.panda.cant_breed", mob.pos, 1.0, 1.0, None);
        }
        p.breed_timer -= 1;
    }

    // Sneezing babies.
    if p.sneeze > 0 {
        p.sneeze += 1;
        if p.sneeze >= SNEEZE_AT {
            p.sneeze = 0;
            sneeze(w, mob);
        }
    } else if !adult {
        let chance = if kind == Gene::Weak {
            WEAK_SNEEZE_CHANCE
        } else {
            SNEEZE_CHANCE
        };
        if w.rng.below(chance) == 0 {
            p.sneeze = 1;
            w.play_sound_at("minecraft:entity.panda.pre_sneeze", mob.pos, 1.0, 1.0, None);
        }
    }

    // After whoever hurt it.
    if let Some(pid) = mob.hunting {
        if chase(w, id, mob, kind, pid) {
            p.stand_up();
            return false;
        }
        mob.hunting = None;
    }

    if kind == Gene::Worried {
        if w.weather.is_thundering() {
            // Hiding its face until the storm is over.
            p.stand_up();
            mob.target = None;
            mob.path.clear();
            if w.rng.below(200) == 0 {
                w.play_sound_at(
                    "minecraft:entity.panda.worried_ambient",
                    mob.pos,
                    1.0,
                    1.0,
                    None,
                );
            }
            return true;
        }
        if scan {
            p.threat = scary_near(w, mob);
        }
        if let Some(threat) = p.threat {
            p.stand_up();
            let (dx, dz) = (mob.pos[0] - threat[0], mob.pos[2] - threat[2]);
            let len = dx.hypot(dz).max(1e-6);
            mob.target = Some((
                [
                    mob.pos[0] + dx / len * WORRY_RANGE,
                    mob.pos[2] + dz / len * WORRY_RANGE,
                ],
                40,
            ));
            return false;
        }
    }

    // Sitting with something to eat.
    if p.eat > 0 {
        p.eat += 1;
        mob.target = None;
        mob.path.clear();
        if p.eat.is_multiple_of(20) {
            let volume = [0.5, 1.0, 1.5][w.rng.below(3) as usize];
            w.play_sound_at("minecraft:entity.panda.eat", mob.pos, volume, 1.0, None);
        }
        if p.eat >= EAT_TICKS {
            p.eat = 0;
            p.eating = None;
            p.eat_cooldown = EAT_COOLDOWN;
        }
        return true;
    }
    if adult && kind != Gene::Worried && p.eat_cooldown == 0 && mob.love == 0 && mob.panic == 0 {
        if scan {
            p.food = food_near(w, mob.pos);
        }
        // Someone else may have taken it since it last looked.
        if p.food
            .is_some_and(|(iid, _, _)| !w.items.get(&iid).is_some_and(|i| i.can_be_picked_up()))
        {
            p.food = None;
        }
        if let Some((iid, item, at)) = p.food {
            if distance(at, mob.pos) <= FOOD_REACH {
                p.food = None;
                p.stand_up();
                w.take_one_item(iid, id);
                p.eat = 1;
                p.eating = Some(item);
                w.play_sound_at("minecraft:entity.panda.eat", mob.pos, 1.0, 1.0, None);
                mob.target = None;
                mob.path.clear();
                return true;
            }
            if p.on_back == 0 {
                mob.target = Some(([at[0], at[2]], 40));
                return false;
            }
        }
    }

    // Lazy pandas lie on their backs.
    if p.on_back > 0 {
        p.on_back -= 1;
        mob.target = None;
        mob.path.clear();
        return true;
    }
    if adult && kind == Gene::Lazy && mob.panic == 0 && w.rng.below(LAZY_CHANCE) == 0 {
        p.on_back = ON_BACK_TICKS + w.rng.below(ON_BACK_TICKS);
        mob.target = None;
        mob.path.clear();
        return true;
    }

    // Playful pandas and babies roll over, tumbling forwards.
    if p.roll > 0 {
        p.roll -= 1;
        let yaw = f64::from(mob.yaw).to_radians();
        mob.target = Some((
            [mob.pos[0] - yaw.sin() * 2.0, mob.pos[2] + yaw.cos() * 2.0],
            5,
        ));
        return false;
    }
    let roll_chance = if kind == Gene::Playful {
        Some(PLAYFUL_ROLL_CHANCE)
    } else if !adult {
        Some(BABY_ROLL_CHANCE)
    } else {
        None
    };
    if let Some(chance) = roll_chance {
        if mob.panic == 0 && w.rng.below(chance) == 0 {
            p.roll = ROLL_TICKS;
            if mob.vel[1].abs() < 0.1 {
                mob.vel[1] = JUMP_SPEED * 0.5;
            }
        }
    }
    false
}

/// A baby's sneeze: adult pandas nearby jump, and now and then out comes a slimeball.
fn sneeze(w: &mut World, mob: &Mob) {
    w.play_sound_at("minecraft:entity.panda.sneeze", mob.pos, 1.0, 1.0, None);
    if mob.age < 0 && w.rng.below(SLIMEBALL_CHANCE) == 0 {
        if let Some(slime) = ItemInfo::id_of("minecraft:slime_ball") {
            w.scatter_item(mob.pos, Stack::new(slime, 1));
        }
    }
    for m in w.mobs.values_mut() {
        if m.kind == MobKind::Panda
            && m.age >= 0
            && m.alive()
            && distance(m.pos, mob.pos) <= SNEEZE_JUMP_RANGE
            && m.vel[1].abs() < 0.1
        {
            m.vel[1] = JUMP_SPEED;
        }
    }
}

/// Goes after player `pid` and bites when close. Returns whether it is still at it.
fn chase(w: &mut World, id: i32, mob: &mut Mob, kind: Gene, pid: i32) -> bool {
    let difficulty = crate::rules::difficulty();
    if difficulty == crate::rules::Difficulty::Peaceful || mob.age < 0 {
        return false;
    }
    let Some(at) = w
        .players
        .get(&pid)
        .filter(|pl| pl.alive && pl.survival())
        .map(|pl| pl.pos)
    else {
        return false;
    };
    if distance(at, mob.pos) > CHASE_RANGE {
        return false;
    }
    // Hurt, it would panic; it fights instead.
    mob.panic = 0;
    mob.target = Some(([at[0], at[2]], 20));
    let close = (at[0] - mob.pos[0]).hypot(at[2] - mob.pos[2]) <= ATTACK_REACH
        && (at[1] - mob.pos[1]).abs() < 2.0;
    if !close || mob.attack_cooldown > 0 {
        return true;
    }
    mob.attack_cooldown = ATTACK_COOLDOWN;
    let damage = attack_damage(kind, difficulty);
    w.damage_from(pid, damage, Damage::Mob("Panda"), Some(id));
    w.play_sound_at("minecraft:entity.panda.bite", mob.pos, 1.0, 1.0, None);
    // Only an aggressive panda keeps fighting after its first bite.
    kind == Gene::Aggressive
}

/// A panda's bite: aggressive ones bite far harder.
pub fn attack_damage(kind: Gene, difficulty: crate::rules::Difficulty) -> f32 {
    use crate::rules::Difficulty;
    match (kind == Gene::Aggressive, difficulty) {
        (true, Difficulty::Easy) => 4.0,
        (true, Difficulty::Hard) => 9.0,
        (true, _) => 6.0,
        (false, Difficulty::Hard) => 3.0,
        (false, _) => 2.0,
    }
}

/// The nearest player or monster a worried panda keeps away from.
fn scary_near(w: &World, mob: &Mob) -> Option<[f64; 3]> {
    let players = w
        .players
        .values()
        .filter(|pl| pl.alive && pl.survival())
        .map(|pl| pl.pos);
    let monsters = w
        .mobs
        .values()
        .filter(|m| m.alive() && m.kind.is_monster() && !NOT_SCARY.contains(&m.kind.name()))
        .map(|m| m.pos);
    players
        .chain(monsters)
        .filter(|&at| distance(at, mob.pos) <= WORRY_RANGE)
        .min_by(|a, b| distance(*a, mob.pos).total_cmp(&distance(*b, mob.pos)))
}

/// The nearest bamboo or cake lying on the ground: its entity, item and position.
fn food_near(w: &World, from: [f64; 3]) -> Option<(i32, i32, [f64; 3])> {
    let bamboo = ItemInfo::id_of("minecraft:bamboo");
    let cake = ItemInfo::id_of("minecraft:cake");
    w.items
        .iter()
        .filter(|(_, item)| item.can_be_picked_up())
        .filter_map(|(&iid, item)| {
            let stack = item.stack_ref();
            let food = Some(stack.item) == bamboo || Some(stack.item) == cake;
            let at = item.pos();
            let near = (at[0] - from[0]).hypot(at[2] - from[2]) <= FOOD_RANGE
                && (at[1] - from[1]).abs() <= 2.0;
            (food && near && stack.count > 0).then_some((iid, stack.item, at))
        })
        .min_by(|a, b| distance(a.2, from).total_cmp(&distance(b.2, from)))
}

pub(super) fn use_on(
    _w: &mut World,
    _player: i32,
    _target: i32,
    _slot: usize,
    _held: &str,
) -> bool {
    // Feeding bamboo is breeding's (`breeding.rs`), with `may_love` below.
    false
}

pub(super) fn hurt(w: &mut World, _id: i32, mob: &mut Mob, attacker: Option<i32>) {
    if let Some(p) = panda_mut(mob) {
        p.stand_up();
    }
    let Some(pid) = attacker else {
        return;
    };
    if crate::rules::difficulty() == crate::rules::Difficulty::Peaceful {
        return;
    }
    if mob.age >= 0 && mob.health > 0.0 {
        mob.hunting = Some(pid);
    }
    // Aggressive pandas nearby take its side, unless it died of the one hit (Java).
    if mob.health <= 0.0 {
        return;
    }
    for m in w.mobs.values_mut() {
        if m.kind != MobKind::Panda
            || m.age < 0
            || !m.alive()
            || m.hunting.is_some()
            || distance(m.pos, mob.pos) > HELP_RANGE
        {
            continue;
        }
        if panda(m).is_some_and(|p| p.personality() == Gene::Aggressive) {
            m.hunting = Some(pid);
        }
    }
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let Some(p) = panda(mob) else {
        return Vec::new();
    };
    let mut packets = vec![panda_data(id, p)];
    if p.eating.is_some() {
        packets.push(held_packet(id, p.eating));
    }
    packets
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    if let Some(p) = panda(mob) {
        nbt.insert("MainGene", p.main.name());
        nbt.insert("HiddenGene", p.hidden.name());
    }
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let main = nbt.str("MainGene").and_then(Gene::from_name);
    let hidden = nbt.str("HiddenGene").and_then(Gene::from_name);
    if main.is_some() || hidden.is_some() {
        mob.kin = Some(Box::new(Panda::new(
            main.unwrap_or(Gene::Normal),
            hidden.unwrap_or(Gene::Normal),
        )));
    }
}

/// Fed bamboo, a panda always falls in love (Java Edition); whether it breeds is
/// `may_mate`'s.
pub(super) fn may_love(_w: &World, _mob: &Mob) -> bool {
    true
}

/// Whether a bamboo plant (not a shoot) grows within the 7x7x3 area around `pos`.
fn bamboo_near(w: &World, pos: [f64; 3]) -> bool {
    let x = pos[0].floor() as i32;
    let y = pos[1].floor() as i32;
    let z = pos[2].floor() as i32;
    (-3..=3).any(|dx| {
        (-1..=1).any(|dy| {
            (-3..=3).any(|dz| {
                w.loaded_block(BlockPos::new(x + dx, y + dy, z + dz))
                    .is_some_and(|s| crate::blocks::name(s) == "minecraft:bamboo")
            })
        })
    })
}

/// Java Edition: a panda in love goes looking for a partner only with a bamboo plant
/// within a 7x7x3 area around it. Without one it shakes its head now and then, and
/// stays in love; a partner that has bamboo by it may still come to it.
// Called by `kin::may_mate` from `tick_breeding` once that hook is wired up.
pub(super) fn may_mate(w: &World, mob: &mut Mob) -> bool {
    let pos = mob.pos;
    let Some(p) = panda_mut(mob) else {
        return true;
    };
    if p.bamboo_check == 0 {
        p.bamboo_ok = bamboo_near(w, pos);
        p.bamboo_check = BAMBOO_CHECK;
    }
    if !p.bamboo_ok && p.shake_cooldown == 0 {
        p.refused.set(true);
        p.shake_cooldown = SHAKE_COOLDOWN;
    }
    p.bamboo_ok
}

/// A panda's maximum health, when its genes change it: weak pandas have half.
// Called by `Mob::max_health` once that is wired up.
pub fn max_health(mob: &Mob) -> Option<f32> {
    panda(mob)
        .is_some_and(|p| p.personality() == Gene::Weak)
        .then_some(WEAK_HEALTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-pandas-{}-{}",
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
            uuid: 11,
            properties: Vec::new(),
        };
        w.players
            .insert(alice, Player::new(profile, out, [20.5, -60.0, 20.5]));
        (w, alice)
    }

    fn panda_with(w: &mut World, main: Gene, hidden: Gene, at: [f64; 3]) -> Mob {
        let mut mob = Mob::new(MobKind::Panda, at, &mut w.rng);
        give_genes(&mut mob, main, hidden);
        mob
    }

    fn state(mob: &Mob) -> &Panda {
        panda(mob).expect("has genes")
    }

    #[test]
    fn recessive_genes_show_only_in_pairs() {
        assert_eq!(personality(Gene::Lazy, Gene::Weak), Gene::Lazy);
        assert_eq!(personality(Gene::Weak, Gene::Lazy), Gene::Normal);
        assert_eq!(personality(Gene::Weak, Gene::Weak), Gene::Weak);
        assert_eq!(personality(Gene::Brown, Gene::Weak), Gene::Normal);
        assert_eq!(personality(Gene::Brown, Gene::Brown), Gene::Brown);
        for g in GENES {
            assert_eq!(Gene::from_name(g.name()), Some(g));
        }
    }

    #[test]
    fn gene_odds_follow_the_wiki() {
        let mut rng = Rng::new(7);
        let n = 160_000;
        let mut counts = [0u32; 7];
        for _ in 0..n {
            counts[Gene::roll(&mut rng) as usize] += 1;
        }
        let share = |g: Gene| f64::from(counts[g as usize]) / f64::from(n) * 16.0;
        assert!((share(Gene::Normal) - 5.0).abs() < 0.2);
        assert!((share(Gene::Weak) - 5.0).abs() < 0.2);
        assert!((share(Gene::Brown) - 2.0).abs() < 0.2);
        assert!((share(Gene::Lazy) - 1.0).abs() < 0.2);
        // Personalities: brown is the rarest, 1/64.
        let mut brown = 0;
        for _ in 0..n {
            let (a, b) = (Gene::roll(&mut rng), Gene::roll(&mut rng));
            brown += u32::from(personality(a, b) == Gene::Brown);
        }
        let brown = f64::from(brown) / f64::from(n);
        assert!((brown - 1.0 / 64.0).abs() < 0.004, "{brown}");
    }

    #[test]
    fn babies_take_a_gene_from_each_parent() {
        let mut rng = Rng::new(3);
        let a = (Gene::Brown, Gene::Brown);
        let b = (Gene::Lazy, Gene::Lazy);
        let mut same = 0;
        for _ in 0..1000 {
            let (main, hidden) = inherit(a, b, &mut rng);
            if [main, hidden].contains(&Gene::Brown) && [main, hidden].contains(&Gene::Lazy) {
                same += 1;
            }
        }
        // Only mutations (1 in 32 per gene) give anything else.
        assert!(same > 900, "{same}");
    }

    #[test]
    fn a_baby_born_beside_its_parents_inherits() {
        let (mut w, _) = world_with_player();
        for at in [[2.5, -60.0, 2.5], [3.5, -60.0, 2.5]] {
            let mut parent = panda_with(&mut w, Gene::Weak, Gene::Weak, at);
            parent.age = 6000;
            w.mobs.insert(next_entity_id(), parent);
        }
        let mut baby = Mob::new(MobKind::Panda, [3.0, -60.0, 3.0], &mut w.rng);
        baby.age = BABY_AGE;
        fresh(&mut w, &mut baby);
        let p = state(&baby);
        // Weak from both, unless a gene mutated.
        assert!(p.main == Gene::Weak || p.hidden == Gene::Weak);
    }

    #[test]
    fn weak_pandas_have_half_health_and_genes_are_saved() {
        let (mut w, _) = world_with_player();
        let mob = panda_with(&mut w, Gene::Weak, Gene::Weak, [3.5, -60.0, 3.5]);
        assert_eq!(mob.health, WEAK_HEALTH);
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        assert_eq!(nbt.str("MainGene"), Some("weak"));
        let mut back = Mob::new(MobKind::Panda, [3.5, -60.0, 3.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        assert_eq!(state(&back).personality(), Gene::Weak);
        assert!(!spawn_extras(1, &back).is_empty());
    }

    #[test]
    fn baby_sneezes_make_adults_jump() {
        let (mut w, _) = world_with_player();
        let adult = next_entity_id();
        let grown = panda_with(&mut w, Gene::Normal, Gene::Normal, [4.5, -60.0, 3.5]);
        w.mobs.insert(adult, grown);
        let mut baby = panda_with(&mut w, Gene::Weak, Gene::Weak, [3.5, -60.0, 3.5]);
        baby.age = BABY_AGE;
        let mut sneezed = false;
        for _ in 0..20_000 {
            w.mobs.get_mut(&adult).unwrap().vel[1] = 0.0;
            tick(&mut w, 1, &mut baby);
            if state(&baby).sneeze == 0 && w.mobs[&adult].vel[1] == JUMP_SPEED {
                sneezed = true;
                break;
            }
        }
        assert!(sneezed);
    }

    #[test]
    fn lazy_pandas_lie_on_their_backs() {
        let (mut w, _) = world_with_player();
        let mut mob = panda_with(&mut w, Gene::Lazy, Gene::Normal, [3.5, -60.0, 3.5]);
        let mut lay = false;
        for _ in 0..20_000 {
            if tick(&mut w, 1, &mut mob) && state(&mob).flags() & FLAG_ON_BACK != 0 {
                lay = true;
                break;
            }
        }
        assert!(lay);
    }

    #[test]
    fn playful_pandas_roll() {
        let (mut w, _) = world_with_player();
        let mut mob = panda_with(&mut w, Gene::Playful, Gene::Normal, [3.5, -60.0, 3.5]);
        let mut rolled = false;
        for _ in 0..20_000 {
            tick(&mut w, 1, &mut mob);
            if state(&mob).flags() & FLAG_ROLLING != 0 {
                rolled = true;
                break;
            }
        }
        assert!(rolled);
    }

    #[test]
    fn worried_pandas_flee_players_and_hide_in_storms() {
        let (mut w, _) = world_with_player();
        let mut mob = panda_with(&mut w, Gene::Worried, Gene::Normal, [17.5, -60.0, 20.5]);
        assert!(!tick(&mut w, 1, &mut mob));
        let (goal, _) = mob.target.expect("fleeing");
        assert!(goal[0] < 17.5, "away from the player at x 20.5");
        w.weather.rain_level = 1.0;
        w.weather.thunder_level = 1.0;
        assert!(tick(&mut w, 1, &mut mob), "keeps still in the storm");
        assert!(mob.target.is_none());
    }

    #[test]
    fn pandas_hit_back_and_aggressive_ones_keep_at_it() {
        let (mut w, alice) = world_with_player();
        let helper = next_entity_id();
        let angry = panda_with(&mut w, Gene::Aggressive, Gene::Normal, [21.5, -60.0, 22.5]);
        w.mobs.insert(helper, angry);
        let mut calm = panda_with(&mut w, Gene::Normal, Gene::Normal, [21.5, -60.0, 20.5]);
        hurt(&mut w, 1, &mut calm, Some(alice));
        assert_eq!(calm.hunting, Some(alice));
        assert_eq!(w.mobs[&helper].hunting, Some(alice), "joins in");
        // A normal panda bites once and calms down.
        let before = w.players[&alice].health;
        tick(&mut w, 1, &mut calm);
        assert!(w.players[&alice].health < before);
        assert_eq!(calm.hunting, None);
        // An aggressive one stays after its prey.
        let mut fierce = w.mobs.remove(&helper).unwrap();
        tick(&mut w, helper, &mut fierce);
        assert_eq!(fierce.hunting, Some(alice));
        assert_eq!(fierce.panic, 0);
        // Babies only panic.
        let mut baby = panda_with(&mut w, Gene::Aggressive, Gene::Normal, [21.5, -60.0, 20.5]);
        baby.age = BABY_AGE;
        hurt(&mut w, 2, &mut baby, Some(alice));
        assert_eq!(baby.hunting, None);
    }

    #[test]
    fn pandas_sit_and_eat_bamboo_lying_close() {
        let (mut w, _) = world_with_player();
        let bamboo = ItemInfo::id_of("minecraft:bamboo").unwrap();
        w.spawn_dropped(Stack::new(bamboo, 1), [3.5, -60.0, 3.5], [0.0; 3]);
        for _ in 0..40 {
            w.tick_items();
        }
        let mut mob = panda_with(&mut w, Gene::Normal, Gene::Normal, [3.5, -60.0, 4.0]);
        assert!(tick(&mut w, 1, &mut mob));
        let p = state(&mob);
        assert_eq!(p.eating, Some(bamboo));
        assert_ne!(p.flags() & FLAG_SITTING, 0);
        assert!(w.items.is_empty(), "picked up");
        for _ in 0..EAT_TICKS {
            tick(&mut w, 1, &mut mob);
        }
        assert_eq!(state(&mob).eating, None, "eaten");
    }

    #[test]
    fn breeding_needs_bamboo_growing_nearby() {
        let (mut w, _) = world_with_player();
        let mut mob = panda_with(&mut w, Gene::Normal, Gene::Normal, [3.5, -60.0, 3.5]);
        assert!(may_love(&w, &mob), "fed bamboo, it falls in love anyway");
        assert!(!may_mate(&w, &mut mob));
        tick(&mut w, 1, &mut mob);
        assert!(state(&mob).breed_timer > 0, "shakes its head");
        // No second head shake straight away.
        for _ in 0..BREED_SHAKE {
            tick(&mut w, 1, &mut mob);
        }
        assert!(!may_mate(&w, &mut mob));
        tick(&mut w, 1, &mut mob);
        assert_eq!(state(&mob).breed_timer, 0);
        let plant = BlockInfo::by_name("minecraft:bamboo")
            .unwrap()
            .default_state;
        w.set_block(BlockPos::new(5, -60, 1), plant);
        for _ in 0..BAMBOO_CHECK {
            tick(&mut w, 1, &mut mob);
        }
        assert!(may_mate(&w, &mut mob));
    }

    #[test]
    fn entity_data_follows_ageable_mob() {
        // Ageable Mob ends at 17 (age locked) in 26.3; Animal adds nothing.
        assert_eq!(DATA_BREED_TIMER, 18);
        assert_eq!(DATA_FLAGS, 23);
    }

    #[test]
    fn weak_pandas_have_ten_max_health() {
        let (mut w, _) = world_with_player();
        let weak = panda_with(&mut w, Gene::Weak, Gene::Weak, [3.5, -60.0, 3.5]);
        assert_eq!(max_health(&weak), Some(WEAK_HEALTH));
        let normal = panda_with(&mut w, Gene::Weak, Gene::Normal, [3.5, -60.0, 3.5]);
        assert_eq!(max_health(&normal), None);
    }

    #[test]
    fn a_panda_killed_in_one_hit_gets_no_help() {
        let (mut w, alice) = world_with_player();
        let helper = next_entity_id();
        let angry = panda_with(&mut w, Gene::Aggressive, Gene::Normal, [21.5, -60.0, 22.5]);
        w.mobs.insert(helper, angry);
        let mut dead = panda_with(&mut w, Gene::Normal, Gene::Normal, [21.5, -60.0, 20.5]);
        dead.health = 0.0;
        hurt(&mut w, 1, &mut dead, Some(alice));
        assert_eq!(w.mobs[&helper].hunting, None);
    }
}
