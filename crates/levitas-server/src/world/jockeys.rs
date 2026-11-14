//! Mobs riding mobs, and the spawn variants that come with them: spider jockeys,
//! chicken jockeys, baby zombies, zombie reinforcements, armed drowned and skeleton
//! trap horses.
//!
//! Behaviour follows the Minecraft Wiki (Riding, Spider Jockey, Chicken Jockey, Zombie,
//! Drowned, Skeleton Horse, Difficulty):
//! - A mob can ride another mob. Players see it through the passengers packet; the rider
//!   stays where its mount is, and in Java Edition the rider steers: the mount walks
//!   where the rider wants to go. A rider whose mount dies, or a mount whose rider dies,
//!   is left on its own. Riders are saved inside their mount, in its `Passengers` list.
//! - A spider has a 1% chance to spawn with a skeleton riding it (only skeletons, in
//!   Java Edition, even in the Nether).
//! - Zombies, husks, drowned, zombie villagers and zombified piglins are babies in 5% of
//!   spawns: half size, half again as fast, and they never grow up. A baby has a 5%
//!   chance to take a chicken from within a 10x6x10 box around it (5x3x5 for a
//!   zombified piglin) and ride it, and failing that a further 5% chance to spawn on a
//!   new chicken. Such a chicken lays no eggs, despawns like a monster and is worth 10
//!   experience (`IsChickenJockey`). A baby zombie is worth 12 experience, not 5.
//! - A hurt zombie (of any kind but zombified piglins) calls the zombies of its kind
//!   around it onto its attacker. On Hard, while monsters may spawn, it may also call
//!   a reinforcement: its chance starts between 0 and 10% (0 for zombified piglins) and
//!   drops by 5% each time. Up to 50 spots are tried 7 to 40 blocks away on each axis
//!   (or level on one) that a monster of its kind could spawn at, with no player within
//!   7 blocks; the reinforcement hunts the same target, with a 5% lower chance of its
//!   own.
//! - Drowned spawn holding a trident (6.25%) or a fishing rod (3.75%), and with a
//!   nautilus shell in the off hand (3%). One with a trident throws it at its target
//!   every 1.5 seconds within 20 blocks. Each spawned item drops with a chance of 8.5%
//!   (1% more a level of Looting) when a player kills it.
//! - In a thunderstorm, a natural lightning strike (not one drawn by a lightning rod)
//!   leaves a skeleton trap horse with a chance of one hundredth of the regional
//!   difficulty (0.75-1.5% on Easy, 1.5-4% on Normal, 2.25-6.75% on Hard). That
//!   strike neither burns nor hurts. The trap despawns after 15 minutes; a player who
//!   comes within 10 blocks sets it off: a harmless lightning strike, and it turns into
//!   four skeleton horsemen, skeletons with an enchanted iron helmet and an enchanted
//!   bow riding skeleton horses. They do not despawn. A helmet keeps a skeleton or
//!   zombie from burning in the sun. Saved as vanilla: `SkeletonTrap`,
//!   `SkeletonTrapTime`.
//!
//! The state lives in `Mob::jockey`, made only for the mobs that need it.

use levitas_nbt::{Compound, Tag};
use levitas_registry::collision_boxes;

use super::arrows::Shot;
use super::mobs::{Mob, MobKind};
use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::blocks::{sturdy, Dir};
use crate::rules::Difficulty;

/// Equipment slots, as the equipment packet numbers them and `equipment` names them.
const MAIN_HAND: usize = 0;
const OFF_HAND: usize = 1;
const HEAD: usize = 5;
const SLOT_NAMES: [&str; 6] = ["mainhand", "offhand", "feet", "legs", "chest", "head"];

/// One spider in this many spawns with a skeleton on its back.
const SPIDER_JOCKEY_ONE_IN: u32 = 100;
/// One zombie in this many spawns as a baby.
const BABY_ONE_IN: u32 = 20;
/// Chances of a baby taking a chicken nearby, or else spawning on a new one.
const NEARBY_CHICKEN: f64 = 0.05;
const NEW_CHICKEN: f64 = 0.05;
/// A baby zombie walks this much faster.
const BABY_SPEED: f64 = 1.5;
/// Experience: a baby zombie gives 12 instead of 5, a chicken jockey's chicken 10
/// instead of 1 to 3.
const BABY_EXTRA_XP: i32 = 7;
const CHICKEN_JOCKEY_XP: i32 = 8;

/// Reinforcements: the most a zombie starts with, what each call costs, how many spots
/// are tried and how far away.
const REINFORCE_MAX: f64 = 0.1;
const REINFORCE_STEP: f64 = 0.05;
const REINFORCE_TRIES: u32 = 50;
const REINFORCE_NEAR: i32 = 7;
const REINFORCE_FAR: i32 = 40;
/// How far a hurt zombie calls its kind: its follow range around, 10 blocks up and down.
const CALL_HEIGHT: f64 = 10.0;

/// Drowned weapons: the odds of a trident, a fishing rod (out of the same roll) and a
/// nautilus shell; the throw's pace, reach and speed.
const TRIDENT_CHANCE: f64 = 0.0625;
const FISHING_ROD_CHANCE: f64 = 0.0375;
const SHELL_CHANCE: f64 = 0.03;
const THROW_TICKS: u32 = 30;
const THROW_RANGE: f64 = 20.0;
const THROW_SPEED: f64 = 1.6;
/// Spawned equipment drops with this chance (and 1% more a level of Looting).
const EQUIPMENT_DROP: f64 = 0.085;

/// Skeleton traps: how close a player sets one off, how long one waits, and how many
/// horsemen it becomes.
const TRAP_REACH: f64 = 10.0;
const TRAP_TICKS: u32 = 18_000;
const HORSEMEN: usize = 4;
/// Ticks a lightning bolt is shown.
const BOLT_TICKS: u64 = 10;

/// What a mob keeps for this module.
#[derive(Default)]
pub struct Jockey {
    /// The mobs riding it, the first one steering.
    pub passengers: Vec<i32>,
    /// The mob it rides (also its `Mob::vehicle`).
    pub mount: Option<i32>,
    /// What it holds and wears, by equipment slot.
    pub equipment: [Stack; 6],
    /// A chicken that came with a zombie rider (`IsChickenJockey`).
    pub chicken_jockey: bool,
    /// A skeleton trap horse: ticks it has been waiting.
    pub trap: Option<u32>,
    /// A zombie's chance of calling a reinforcement when hurt, once rolled.
    pub reinforce: Option<f64>,
    /// Ticks until a drowned can throw its trident again.
    pub throw_cooldown: u32,
}

fn state(mob: &mut Mob) -> &mut Jockey {
    mob.jockey.get_or_insert_with(Box::default)
}

/// The zombies: the mobs that can be babies, ride chickens and call reinforcements.
pub(super) fn zombie_kin(kind: MobKind) -> bool {
    matches!(
        kind,
        MobKind::Zombie
            | MobKind::Husk
            | MobKind::Drowned
            | MobKind::ZombieVillager
            | MobKind::ZombifiedPiglin
    )
}

fn item(name: &str) -> Option<Stack> {
    ItemInfo::id_of(name).map(|i| Stack::new(i, 1))
}

fn holds(mob: &Mob, slot: usize, name: &str) -> bool {
    mob.jockey.as_ref().is_some_and(|j| {
        !j.equipment[slot].is_empty()
            && ItemInfo::get(j.equipment[slot].item).is_some_and(|i| i.name == name)
    })
}

/// Whether something on its head keeps it from burning in the sun.
pub(super) fn wears_helmet(mob: &Mob) -> bool {
    mob.jockey
        .as_ref()
        .is_some_and(|j| !j.equipment[HEAD].is_empty())
}

/// How much faster than its kind it walks: babies of the zombies are quicker.
pub(super) fn speed_factor(mob: &Mob) -> f64 {
    if mob.age < 0 && zombie_kin(mob.kind) {
        BABY_SPEED
    } else {
        1.0
    }
}

/// A chicken that carries (or carried) a zombie: no eggs, and it despawns.
pub(super) fn chicken_jockey(mob: &Mob) -> bool {
    mob.jockey.as_ref().is_some_and(|j| j.chicken_jockey)
}

/// The mob it rides, if any.
pub(super) fn mount_of(mob: &Mob) -> Option<i32> {
    mob.jockey.as_ref().and_then(|j| j.mount)
}

/// Where a rider sits on the mob `mount`, if that is a living mob.
pub(super) fn seat(w: &World, mount: i32) -> Option<[f64; 3]> {
    w.mobs.get(&mount).filter(|m| m.alive()).map(|m| m.pos)
}

/// Whether the mob is saved inside its mount rather than on its own.
pub(super) fn carried(w: &World, mob: &Mob) -> bool {
    mount_of(mob).is_some_and(|m| w.mobs.get(&m).is_some_and(|m| m.alive()))
}

/// The chunk a mob is saved and unloaded with: its lowest mount's.
pub(super) fn save_chunk(w: &World, mob: &Mob) -> (i32, i32) {
    let mut at = mob;
    for _ in 0..8 {
        match mount_of(at).and_then(|m| w.mobs.get(&m)) {
            Some(m) if m.alive() => at = m,
            _ => break,
        }
    }
    at.chunk()
}

/// Who rides `mount`.
pub(super) fn passengers_packet(mount: i32, riders: &[i32]) -> Writer {
    let mut packet = Writer::packet(clientbound::SET_PASSENGERS)
        .varint(mount)
        .varint(riders.len() as i32);
    for &r in riders {
        packet = packet.varint(r);
    }
    packet
}

/// One equipment slot.
fn equipment_packet(id: i32, slot: usize, stack: Stack) -> Writer {
    let packet = Writer::packet(clientbound::SET_EQUIPMENT)
        .varint(id)
        .u8(slot as u8);
    crate::inventory::write_stack(packet, stack)
}

/// The packets showing a new viewer what the mob holds and who rides whom.
pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let Some(j) = &mob.jockey else {
        return Vec::new();
    };
    let mut out: Vec<Writer> = j
        .equipment
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.is_empty())
        .map(|(slot, &s)| equipment_packet(id, slot, s))
        .collect();
    if !j.passengers.is_empty() {
        out.push(passengers_packet(id, &j.passengers));
    }
    // The rider may come into view after its mount.
    if let Some(m) = j.mount {
        out.push(passengers_packet(m, &[id]));
    }
    out
}

/// Puts the mob `rider` on the mob `mount`, both in the world.
pub(super) fn link(w: &mut World, rider: i32, mount: i32) {
    if rider == mount || !w.mobs.contains_key(&rider) {
        return;
    }
    let Some(m) = w.mobs.get_mut(&mount) else {
        return;
    };
    let pos = m.pos;
    let j = state(m);
    if !j.passengers.contains(&rider) {
        j.passengers.push(rider);
    }
    let riders = j.passengers.clone();
    let r = w.mobs.get_mut(&rider).expect("checked");
    r.pos = pos;
    r.vehicle = Some(mount);
    state(r).mount = Some(mount);
    let packet = passengers_packet(mount, &riders);
    let m = w.mobs.remove(&mount).expect("checked");
    w.send_to_viewers(&m, packet);
    w.mobs.insert(mount, m);
}

/// The regional difficulty at `pos` (0 to 6.75), from the difficulty, the days the
/// world has run and the moon; Levitas does not count the time chunks are inhabited.
pub(super) fn regional_difficulty(w: &World, difficulty: Difficulty) -> f64 {
    if difficulty == Difficulty::Peaceful {
        return 0.0;
    }
    let total = w.day_time.max(0) as f64;
    let day_factor = if total > 63.0 * 24_000.0 {
        0.25
    } else if total < 72_000.0 {
        0.0
    } else {
        (total - 72_000.0) / 5_760_000.0
    };
    // The moon's size: full on day 0, new on day 4.
    const MOON: [f64; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];
    let phase = MOON[(w.day_time.max(0) / 24_000 % 8) as usize];
    let mut chunk_factor = (phase * 0.25).min(day_factor);
    if difficulty == Difficulty::Easy {
        chunk_factor *= 0.5;
    }
    let regional = 0.75 + day_factor + chunk_factor;
    match difficulty {
        Difficulty::Normal => regional * 2.0,
        Difficulty::Hard => regional * 3.0,
        _ => regional,
    }
}

/// A mob spawned by the world (natural spawning, spawners, reinforcements): babies,
/// jockeys and weapons are rolled, and it is added. Returns its entity ID.
pub(super) fn spawn_natural(w: &mut World, mut mob: Mob) -> i32 {
    let kind = mob.kind;
    if zombie_kin(kind) && w.rng.below(BABY_ONE_IN) == 0 {
        mob.age = -1;
    }
    if kind == MobKind::Drowned {
        arm_drowned(w, &mut mob);
    }
    let baby = mob.age < 0;
    let pos = mob.pos;
    let id = next_entity_id();
    w.mobs.insert(id, mob);
    if kind == MobKind::Spider && w.rng.below(SPIDER_JOCKEY_ONE_IN) == 0 {
        let skeleton = Mob::new(MobKind::Skeleton, pos, &mut w.rng);
        let rider = next_entity_id();
        w.mobs.insert(rider, skeleton);
        link(w, rider, id);
    }
    if baby && zombie_kin(kind) {
        chicken_for(w, id, pos, kind);
    }
    id
}

/// A drowned's spawned weapon and shell.
fn arm_drowned(w: &mut World, mob: &mut Mob) {
    let roll = w.rng.f64();
    let main = if roll < TRIDENT_CHANCE {
        item("minecraft:trident")
    } else if roll < TRIDENT_CHANCE + FISHING_ROD_CHANCE {
        item("minecraft:fishing_rod")
    } else {
        None
    };
    let shell = (w.rng.f64() < SHELL_CHANCE)
        .then(|| item("minecraft:nautilus_shell"))
        .flatten();
    if main.is_none() && shell.is_none() {
        return;
    }
    let j = state(mob);
    if let Some(s) = main {
        j.equipment[MAIN_HAND] = s;
    }
    if let Some(s) = shell {
        j.equipment[OFF_HAND] = s;
    }
}

/// A baby zombie may take a chicken nearby, or come riding a new one.
fn chicken_for(w: &mut World, id: i32, pos: [f64; 3], kind: MobKind) {
    let (half, height) = if kind == MobKind::ZombifiedPiglin {
        (2.5, 1.5)
    } else {
        (5.0, 3.0)
    };
    if w.rng.f64() < NEARBY_CHICKEN {
        let near = w
            .mobs
            .iter()
            .filter(|(_, m)| {
                m.kind == MobKind::Chicken
                    && m.alive()
                    && m.age >= 0
                    && m.vehicle.is_none()
                    && m.jockey.as_ref().is_none_or(|j| j.passengers.is_empty())
                    && (m.pos[0] - pos[0]).abs() <= half
                    && (m.pos[2] - pos[2]).abs() <= half
                    && (m.pos[1] - pos[1]).abs() <= height
            })
            .map(|(&c, _)| c)
            .min();
        if let Some(chicken) = near {
            if let Some(c) = w.mobs.get_mut(&chicken) {
                state(c).chicken_jockey = true;
            }
            link(w, id, chicken);
        }
    } else if w.rng.f64() < NEW_CHICKEN {
        let mut chicken = Mob::new(MobKind::Chicken, pos, &mut w.rng);
        if let Some(rider) = w.mobs.get(&id) {
            chicken.yaw = rider.yaw;
        }
        state(&mut chicken).chicken_jockey = true;
        let c = next_entity_id();
        w.mobs.insert(c, chicken);
        link(w, id, c);
    }
}

/// The mob's own turn: a trap waiting for players, riders and mounts keeping track of
/// each other (the mount walking where its rider steers), and a drowned throwing its
/// trident. Returns whether the mob is gone.
pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if mob.jockey.is_none() {
        return false;
    }
    if let Some(t) = mob.jockey.as_mut().and_then(|j| j.trap.as_mut()) {
        *t += 1;
        if *t >= TRAP_TICKS {
            w.send_to_viewers(mob, items::remove_entity(id));
            return true;
        }
        let near = w.players.values().any(|p| {
            p.alive
                && (0..3).map(|i| (p.pos[i] - mob.pos[i]).powi(2)).sum::<f64>()
                    <= TRAP_REACH * TRAP_REACH
        });
        if near {
            spring_trap(w, id, mob);
        }
    }
    keep_seats(w, id, mob);
    if holds(mob, MAIN_HAND, "minecraft:trident") {
        throw_trident(w, id, mob);
    }
    false
}

/// A rider checks it still has its mount; a mount drops riders that are gone, and
/// goes where its first rider steers.
fn keep_seats(w: &mut World, id: i32, mob: &mut Mob) {
    let Some(j) = mob.jockey.as_mut() else {
        return;
    };
    if let Some(m) = j.mount {
        let held = w.mobs.get(&m).is_some_and(|mm| {
            mm.alive()
                && mm
                    .jockey
                    .as_ref()
                    .is_some_and(|mj| mj.passengers.contains(&id))
        });
        if !held {
            j.mount = None;
            if mob.vehicle == Some(m) {
                mob.vehicle = None;
            }
        }
    }
    if j.passengers.is_empty() {
        return;
    }
    let before = j.passengers.len();
    j.passengers.retain(|p| {
        w.mobs
            .get(p)
            .is_some_and(|r| r.alive() && r.jockey.as_ref().is_some_and(|rj| rj.mount == Some(id)))
    });
    let changed = j.passengers.len() != before;
    let first = j.passengers.first().copied();
    if changed {
        let packet = passengers_packet(id, &j.passengers);
        w.send_to_viewers(mob, packet);
    }
    // In Java Edition the rider steers.
    if let Some(rider) = first.and_then(|r| w.mobs.get(&r)) {
        mob.target = rider.target;
        mob.path.clone_from(&rider.path);
        if rider.hunting.is_some() || rider.target.is_some() {
            mob.yaw = rider.yaw;
        }
    }
}

/// A drowned holding a trident throws it at what it hunts.
fn throw_trident(w: &mut World, id: i32, mob: &mut Mob) {
    let j = state(mob);
    j.throw_cooldown = j.throw_cooldown.saturating_sub(1);
    if j.throw_cooldown > 0 {
        return;
    }
    let Some(target) = mob.hunting.and_then(|p| w.players.get(&p)).map(|p| p.pos) else {
        return;
    };
    let eye = [mob.pos[0], mob.pos[1] + mob.size().1 * 0.85, mob.pos[2]];
    let chest = [target[0], target[1] + 0.6, target[2]];
    let (dx, dy, dz) = (chest[0] - eye[0], chest[1] - eye[1], chest[2] - eye[2]);
    let distance = (dx * dx + dy * dy + dz * dz).sqrt();
    // Close up it strikes instead.
    if !(2.0..=THROW_RANGE).contains(&distance)
        || !w.line_of_sight(eye, [target[0], target[1] + 1.62, target[2]])
    {
        return;
    }
    let trident = mob.jockey.as_ref().expect("made above").equipment[MAIN_HAND];
    let lift = dx.hypot(dz) * 0.2;
    let inaccuracy = f64::from(14 - 4 * crate::rules::difficulty() as i32).max(2.0);
    w.shoot_arrow(
        id,
        false,
        Shot {
            from: eye,
            dir: [dx, dy + lift, dz],
            speed: THROW_SPEED,
            inaccuracy,
            base_damage: 0.0,
            pickup: false,
            trident: Some(trident),
            ..Default::default()
        },
    );
    w.play_sound_at("minecraft:entity.drowned.shoot", mob.pos, 1.0, 1.0, None);
    state(mob).throw_cooldown = THROW_TICKS;
}

/// The mob died: its riders and mount part from it, spawned equipment may drop, and
/// jockeys give their extra experience.
pub(super) fn killed(w: &mut World, id: i32, mob: &mut Mob, by_player: bool, looting: u32) {
    let Some(j) = mob.jockey.as_mut() else {
        if by_player && mob.age < 0 && zombie_kin(mob.kind) && crate::rules::bool("mob_drops") {
            w.spawn_xp(mob.pos, BABY_EXTRA_XP);
        }
        return;
    };
    let riders = std::mem::take(&mut j.passengers);
    let mount = j.mount.take();
    let equipment = j.equipment;
    let chicken = j.chicken_jockey;
    if mob.vehicle.is_some() && mob.vehicle == mount {
        mob.vehicle = None;
    }
    for r in &riders {
        if let Some(rider) = w.mobs.get_mut(r) {
            if let Some(rj) = rider.jockey.as_mut() {
                rj.mount = None;
            }
            if rider.vehicle == Some(id) {
                rider.vehicle = None;
            }
        }
    }
    if !riders.is_empty() {
        w.send_to_viewers(mob, passengers_packet(id, &[]));
    }
    if let Some(m) = mount {
        if let Some(mut mm) = w.mobs.remove(&m) {
            if let Some(mj) = mm.jockey.as_mut() {
                mj.passengers.retain(|&p| p != id);
                let packet = passengers_packet(m, &mj.passengers);
                w.send_to_viewers(&mm, packet);
            }
            w.mobs.insert(m, mm);
        }
    }
    if !by_player {
        return;
    }
    let chance = EQUIPMENT_DROP + 0.01 * f64::from(looting);
    for stack in equipment {
        if !stack.is_empty() && w.rng.f64() < chance {
            w.scatter_item(mob.pos, stack);
        }
    }
    if crate::rules::bool("mob_drops") {
        let extra = if chicken {
            CHICKEN_JOCKEY_XP
        } else if mob.age < 0 && zombie_kin(mob.kind) {
            BABY_EXTRA_XP
        } else {
            0
        };
        if extra > 0 {
            w.spawn_xp(mob.pos, extra);
        }
    }
}

/// A zombie was hurt, by player `attacker` if any: it calls its kind onto whoever it
/// fights, and on Hard maybe a reinforcement.
pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, attacker: Option<i32>) {
    if !zombie_kin(mob.kind) || mob.health <= 0.0 {
        return;
    }
    let target = attacker.or(mob.hunting);
    let Some(target) = target.filter(|t| w.players.get(t).is_some_and(|p| p.alive)) else {
        return;
    };
    if let Some(attacker) = attacker.filter(|_| mob.kind != MobKind::ZombifiedPiglin) {
        let range = mob.kind.follow_range();
        for (&other, m) in w.mobs.iter_mut() {
            if other != id
                && m.kind == mob.kind
                && m.alive()
                && m.hunting.is_none()
                && (m.pos[0] - mob.pos[0]).abs() <= range
                && (m.pos[2] - mob.pos[2]).abs() <= range
                && (m.pos[1] - mob.pos[1]).abs() <= CALL_HEIGHT
            {
                m.hunting = Some(attacker);
            }
        }
    }
    let difficulty = crate::rules::difficulty();
    if difficulty != Difficulty::Hard
        || !crate::rules::bool("spawn_mobs")
        || !crate::rules::bool("spawn_monsters")
    {
        return;
    }
    reinforce(w, mob, target);
}

/// A zombie's own chance of calling help, rolled once.
fn reinforce_chance(w: &mut World, mob: &mut Mob) -> f64 {
    let base = if mob.kind == MobKind::ZombifiedPiglin {
        0.0
    } else {
        w.rng.f64() * REINFORCE_MAX
    };
    *state(mob).reinforce.get_or_insert(base)
}

/// Tries to call a reinforcement for `mob` against player `target`. Returns its ID.
pub(super) fn reinforce(w: &mut World, mob: &mut Mob, target: i32) -> Option<i32> {
    let chance = reinforce_chance(w, mob);
    if w.rng.f64() >= chance {
        return None;
    }
    let base = mob.pos.map(|v| v.floor() as i32);
    for _ in 0..REINFORCE_TRIES {
        let mut offset = || {
            let sign = w.rng.below(3) as i32 - 1;
            sign * (REINFORCE_NEAR
                + w.rng.below((REINFORCE_FAR - REINFORCE_NEAR + 1) as u32) as i32)
        };
        let (dx, dy, dz) = (offset(), offset(), offset());
        let pos = BlockPos::new(base[0] + dx, base[1] + dy, base[2] + dz);
        if !reinforcement_fits(w, mob.kind, pos) {
            continue;
        }
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let mut help = Mob::new(mob.kind, at, &mut w.rng);
        let own = if mob.kind == MobKind::ZombifiedPiglin {
            0.0
        } else {
            w.rng.f64() * REINFORCE_MAX
        };
        state(&mut help).reinforce = Some((own - REINFORCE_STEP).max(0.0));
        help.hunting = Some(target);
        let new = spawn_natural(w, help);
        let j = state(mob);
        j.reinforce = Some((chance - REINFORCE_STEP).max(0.0));
        return Some(new);
    }
    None
}

/// Whether a reinforcement of `kind` may appear at `pos`.
fn reinforcement_fits(w: &World, kind: MobKind, pos: BlockPos) -> bool {
    let centre = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ];
    let near_player = w.players.values().any(|p| {
        (0..3).map(|i| (p.pos[i] - centre[i]).powi(2)).sum::<f64>()
            < f64::from(REINFORCE_NEAR * REINFORCE_NEAR)
    });
    if near_player {
        return false;
    }
    let at = |dy: i32| w.loaded_state(BlockPos::new(pos.x, pos.y + dy, pos.z));
    let (Some(below), Some(feet), Some(head)) = (at(-1), at(0), at(1)) else {
        return false;
    };
    if !sturdy(below, Dir::Up) || below == block::BEDROCK {
        return false;
    }
    if kind == MobKind::ZombifiedPiglin
        && crate::blocks::name(below) == "minecraft:nether_wart_block"
    {
        return false;
    }
    let water = |s: u16| fluid_at(s).is_some_and(|f| f.fluid == Fluid::Water);
    if kind == MobKind::Drowned {
        if !water(feet) {
            return false;
        }
    } else if !collision_boxes(feet).is_empty()
        || fluid_at(feet).is_some()
        || !collision_boxes(head).is_empty()
    {
        return false;
    }
    // Dark as for natural spawning: no block light and a dim sky in the Overworld, up to
    // 7 in the Nether, 11 for zombified piglins.
    let Some((sky, light)) = w.light_at(pos) else {
        return false;
    };
    let dark = if kind == MobKind::ZombifiedPiglin {
        w.light_level(pos) <= 11
    } else if w.dimension.has_sky() {
        light == 0 && i32::from(sky) - w.sky_darken() <= 7
    } else {
        w.light_level(pos) <= 7
    };
    dark && w.sky_allows(kind, pos) && w.mob_fits(kind, centre)
}

/// A natural strike in a thunderstorm at `at` may leave a skeleton trap horse instead
/// of burning and hurting. Returns whether it did (and the strike has been shown).
pub(super) fn lightning_trap(w: &mut World, at: BlockPos) -> bool {
    let difficulty = crate::rules::difficulty();
    if difficulty == Difficulty::Peaceful {
        return false;
    }
    // Lightning drawn by a rod never leaves one.
    let below = w.loaded_state(BlockPos::new(at.x, at.y - 1, at.z));
    if below.is_some_and(|s| block_has_tag(s, "minecraft:lightning_rods")) {
        return false;
    }
    let chance = regional_difficulty(w, difficulty) * 0.01;
    if w.rng.f64() >= chance {
        return false;
    }
    let pos = [
        f64::from(at.x) + 0.5,
        f64::from(at.y),
        f64::from(at.z) + 0.5,
    ];
    spawn_trap(w, pos);
    true
}

/// A skeleton trap horse at `pos`, shown with a harmless strike.
pub(super) fn spawn_trap(w: &mut World, pos: [f64; 3]) -> i32 {
    harmless_bolt(w, pos);
    let mut horse = Mob::new(MobKind::SkeletonHorse, pos, &mut w.rng);
    horse.age = 0;
    state(&mut horse).trap = Some(0);
    let id = next_entity_id();
    w.mobs.insert(id, horse);
    id
}

/// A lightning bolt that is only seen and heard.
fn harmless_bolt(w: &mut World, at: [f64; 3]) {
    let id = next_entity_id();
    let uuid = (u128::from(w.rng.next_u64()) << 64) | u128::from(w.rng.next_u64());
    w.broadcast(
        Writer::packet(clientbound::ADD_ENTITY)
            .varint(id)
            .uuid(uuid)
            .varint(entity_type::LIGHTNING_BOLT)
            .f64(at[0])
            .f64(at[1])
            .f64(at[2])
            .lp_vec3([0.0, 0.0, 0.0])
            .u8(0)
            .u8(0)
            .u8(0)
            .varint(0),
        None,
    );
    w.bolts.push((id, w.tick + BOLT_TICKS));
    let pitch = 0.8 + w.rng.f64() as f32 * 0.2;
    w.weather_sound(
        "minecraft:entity.lightning_bolt.thunder",
        at,
        10_000.0,
        pitch,
    );
    let pitch = 0.5 + w.rng.f64() as f32 * 0.2;
    w.weather_sound("minecraft:entity.lightning_bolt.impact", at, 2.0, pitch);
}

/// A player came near a trap: lightning, and it becomes four skeleton horsemen.
fn spring_trap(w: &mut World, id: i32, mob: &mut Mob) {
    state(mob).trap = None;
    mob.persistent = true;
    harmless_bolt(w, mob.pos);
    let pos = mob.pos;
    // The trap horse is out of the map for its turn: it is seated from both sides here.
    let first = horseman(w, pos);
    if let Some(r) = w.mobs.get_mut(&first) {
        r.vehicle = Some(id);
        state(r).mount = Some(id);
    }
    state(mob).passengers.push(first);
    let riders = state(mob).passengers.clone();
    w.send_to_viewers(mob, passengers_packet(id, &riders));
    for _ in 1..HORSEMEN {
        let spread = |rng: &mut Rng| (rng.f64() - rng.f64()) * 3.0;
        let at = [
            pos[0] + spread(&mut w.rng),
            pos[1],
            pos[2] + spread(&mut w.rng),
        ];
        let mut horse = Mob::new(MobKind::SkeletonHorse, at, &mut w.rng);
        horse.age = 0;
        horse.persistent = true;
        let h = next_entity_id();
        w.mobs.insert(h, horse);
        let rider = horseman(w, at);
        link(w, rider, h);
    }
}

/// A skeleton with an enchanted iron helmet and an enchanted bow, added at `at`.
fn horseman(w: &mut World, at: [f64; 3]) -> i32 {
    let mut skeleton = Mob::new(MobKind::Skeleton, at, &mut w.rng);
    skeleton.persistent = true;
    let level = 5 + w.rng.below(18);
    for (slot, name) in [
        (HEAD, "minecraft:iron_helmet"),
        (MAIN_HAND, "minecraft:bow"),
    ] {
        if let Some(mut stack) = item(name) {
            for (e, l) in crate::enchanting::select(&mut w.rng, stack.item, level) {
                stack.enchants.set(e, l);
            }
            state(&mut skeleton).equipment[slot] = stack;
        }
    }
    let id = next_entity_id();
    w.mobs.insert(id, skeleton);
    id
}

/// The mobs whose held and worn items this module shows and saves: the zombies and
/// the skeletons (other modules keep the items of the mobs that use theirs).
fn armed(kind: MobKind) -> bool {
    zombie_kin(kind) || kind.family() == MobKind::Skeleton || kind == MobKind::WitherSkeleton
}

/// Saved tags: `IsBaby` for the zombies, `IsChickenJockey`, `SkeletonTrap` and
/// `SkeletonTrapTime`, and spawned `equipment`.
pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    if zombie_kin(mob.kind) {
        nbt.insert("IsBaby", mob.age < 0);
    }
    if mob.kind == MobKind::Chicken {
        nbt.insert("IsChickenJockey", chicken_jockey(mob));
    }
    if mob.kind == MobKind::SkeletonHorse {
        let trap = mob.jockey.as_ref().and_then(|j| j.trap);
        nbt.insert("SkeletonTrap", trap.is_some());
        nbt.insert("SkeletonTrapTime", trap.unwrap_or(0) as i32);
    }
    let Some(j) = &mob.jockey else {
        return;
    };
    if armed(mob.kind) && j.equipment.iter().any(|s| !s.is_empty()) {
        // Into what other modules saved there already.
        let mut equipment = nbt.compound("equipment").cloned().unwrap_or_default();
        for (slot, stack) in j.equipment.iter().enumerate() {
            if !stack.is_empty() {
                equipment.insert(SLOT_NAMES[slot], stack.to_nbt());
            }
        }
        nbt.insert("equipment", equipment);
    }
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    if zombie_kin(mob.kind) {
        if let Some(baby) = nbt.int("IsBaby") {
            mob.age = if baby != 0 { -1 } else { 0 };
        }
    }
    if mob.kind == MobKind::Chicken && nbt.int("IsChickenJockey").unwrap_or(0) != 0 {
        state(mob).chicken_jockey = true;
    }
    if mob.kind == MobKind::SkeletonHorse && nbt.int("SkeletonTrap").unwrap_or(0) != 0 {
        let time = nbt.int("SkeletonTrapTime").unwrap_or(0).max(0) as u32;
        state(mob).trap = Some(time);
    }
    if let Some(e) = nbt.compound("equipment").filter(|_| armed(mob.kind)) {
        let mut equipment = [Stack::EMPTY; 6];
        for (slot, name) in SLOT_NAMES.iter().enumerate() {
            if let Some(s) = e.compound(name) {
                equipment[slot] = Stack::from_nbt(s);
            }
        }
        if equipment.iter().any(|s| !s.is_empty()) {
            state(mob).equipment = equipment;
        }
    }
}

/// The mob as saved, its riders inside it.
pub(super) fn to_nbt(w: &World, mob: &Mob) -> Compound {
    let mut nbt = mob.to_nbt();
    let riders = mob.jockey.as_ref().map_or(&[][..], |j| &j.passengers[..]);
    let saved: Vec<Tag> = riders
        .iter()
        .filter_map(|r| w.mobs.get(r))
        .filter(|r| r.alive())
        .map(|r| Tag::Compound(to_nbt(w, r)))
        .collect();
    if !saved.is_empty() {
        nbt.insert("Passengers", saved);
    }
    nbt
}

/// A mob just loaded as `id` from `nbt`: its riders come back on it. Passengers that
/// are not mobs Levitas knows are kept as entities of their own.
pub(super) fn load_passengers(w: &mut World, id: i32, nbt: &Compound, chunk: (i32, i32)) {
    for tag in nbt.list("Passengers").unwrap_or_default() {
        let Some(entity) = tag.as_compound() else {
            continue;
        };
        match Mob::read(entity, &mut w.rng) {
            Some(mut rider) => {
                rider.vehicle = None;
                let r = next_entity_id();
                w.mobs.insert(r, rider);
                link(w, r, id);
                load_passengers(w, r, entity, chunk);
            }
            None => w
                .kept_entities
                .entry(chunk)
                .or_default()
                .push(entity.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn flat_world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-jockeys-{}-{}",
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
        for cx in -3..=3 {
            for cz in -3..=3 {
                w.chunk(cx, cz);
            }
        }
        w
    }

    fn add_player(w: &mut World, at: [f64; 3]) -> i32 {
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 11,
            properties: Vec::new(),
        };
        w.players.insert(id, Player::new(profile, out, at));
        id
    }

    fn add(w: &mut World, kind: MobKind, at: [f64; 3]) -> i32 {
        let mob = Mob::new(kind, at, &mut w.rng);
        let id = next_entity_id();
        w.mobs.insert(id, mob);
        id
    }

    #[test]
    fn a_rider_goes_where_its_mount_goes_and_is_left_when_it_dies() {
        let mut w = flat_world();
        // Someone near, or the monsters despawn.
        add_player(&mut w, [0.5, -60.0, 30.5]);
        let spider = add(&mut w, MobKind::Spider, [0.5, -60.0, 0.5]);
        let skeleton = add(&mut w, MobKind::Skeleton, [3.5, -60.0, 3.5]);
        link(&mut w, skeleton, spider);
        assert_eq!(w.mobs[&skeleton].vehicle, Some(spider));
        assert_eq!(w.mobs[&skeleton].pos, w.mobs[&spider].pos);
        w.mobs.get_mut(&spider).unwrap().pos = [5.5, -60.0, 2.5];
        w.tick_mobs();
        let (s, k) = (&w.mobs[&spider], &w.mobs[&skeleton]);
        assert_eq!(k.pos, s.pos, "carried along");
        assert_eq!(s.jockey.as_ref().unwrap().passengers, vec![skeleton]);

        let mut dead = w.mobs.remove(&spider).unwrap();
        w.kill_mob(spider, &mut dead);
        w.mobs.insert(spider, dead);
        assert_eq!(w.mobs[&skeleton].vehicle, None, "off when its mount died");
        assert_eq!(mount_of(&w.mobs[&skeleton]), None);
    }

    #[test]
    fn a_mount_lets_go_of_a_dead_rider_and_the_rider_steers() {
        let mut w = flat_world();
        let chicken = add(&mut w, MobKind::Chicken, [0.5, -60.0, 0.5]);
        let zombie = add(&mut w, MobKind::Zombie, [0.5, -60.0, 0.5]);
        link(&mut w, zombie, chicken);
        w.mobs.get_mut(&zombie).unwrap().target = Some(([9.5, 0.5], 40));
        let mut c = w.mobs.remove(&chicken).unwrap();
        keep_seats(&mut w, chicken, &mut c);
        assert_eq!(c.target.map(|t| t.0), Some([9.5, 0.5]), "the rider steers");
        w.mobs.insert(chicken, c);

        let mut z = w.mobs.remove(&zombie).unwrap();
        w.kill_mob(zombie, &mut z);
        w.mobs.insert(zombie, z);
        assert!(w.mobs[&chicken]
            .jockey
            .as_ref()
            .unwrap()
            .passengers
            .is_empty());
    }

    #[test]
    fn riders_are_saved_inside_their_mount_and_come_back_on_it() {
        let mut w = flat_world();
        let chicken = add(&mut w, MobKind::Chicken, [0.5, -60.0, 0.5]);
        let zombie = add(&mut w, MobKind::Zombie, [0.5, -60.0, 0.5]);
        w.mobs.get_mut(&zombie).unwrap().age = -1;
        state(w.mobs.get_mut(&chicken).unwrap()).chicken_jockey = true;
        link(&mut w, zombie, chicken);
        assert!(carried(&w, &w.mobs[&zombie]));
        assert!(!carried(&w, &w.mobs[&chicken]));
        let nbt = to_nbt(&w, &w.mobs[&chicken]);
        assert_eq!(nbt.int("IsChickenJockey"), Some(1));
        let passengers = nbt.list("Passengers").expect("riders saved");
        assert_eq!(passengers.len(), 1);
        let rider = passengers[0].as_compound().unwrap();
        assert_eq!(rider.str("id"), Some("minecraft:zombie"));
        assert_eq!(rider.int("IsBaby"), Some(1));

        let mut back = flat_world();
        let mob = Mob::read(&nbt, &mut back.rng).unwrap();
        assert!(mob.to_nbt().get("Passengers").is_none(), "not kept twice");
        let id = next_entity_id();
        back.mobs.insert(id, mob);
        load_passengers(&mut back, id, &nbt, (0, 0));
        let rider = back
            .mobs
            .iter()
            .find(|(_, m)| m.kind == MobKind::Zombie)
            .map(|(&r, m)| (r, m.age, m.vehicle))
            .unwrap();
        assert_eq!(rider.1, -1, "still a baby");
        assert_eq!(rider.2, Some(id));
        assert!(chicken_jockey(&back.mobs[&id]));
        assert_eq!(
            back.mobs[&id].jockey.as_ref().unwrap().passengers,
            vec![rider.0]
        );
    }

    #[test]
    fn spiders_carry_skeletons_and_babies_ride_chickens_at_the_wiki_odds() {
        let mut w = flat_world();
        let (mut spiders, mut jockeys) = (0, 0);
        for _ in 0..20_000 {
            let mob = Mob::new(MobKind::Spider, [0.5, -60.0, 0.5], &mut w.rng);
            let id = spawn_natural(&mut w, mob);
            spiders += 1;
            if w.mobs[&id]
                .jockey
                .as_ref()
                .is_some_and(|j| !j.passengers.is_empty())
            {
                let rider = w.mobs[&id].jockey.as_ref().unwrap().passengers[0];
                assert_eq!(w.mobs[&rider].kind, MobKind::Skeleton);
                jockeys += 1;
            }
            w.mobs.clear();
        }
        let share = f64::from(jockeys) / f64::from(spiders);
        assert!((0.005..0.016).contains(&share), "{share}");

        let (mut babies, mut on_chickens) = (0, 0);
        for _ in 0..40_000 {
            let mob = Mob::new(MobKind::Zombie, [0.5, -60.0, 0.5], &mut w.rng);
            let id = spawn_natural(&mut w, mob);
            if w.mobs[&id].age < 0 {
                babies += 1;
                assert!(w.mobs[&id].size().1 < 1.0, "small");
                assert!(speed_factor(&w.mobs[&id]) > 1.0, "quick");
                if let Some(m) = w.mobs[&id].vehicle {
                    assert_eq!(w.mobs[&m].kind, MobKind::Chicken);
                    assert!(chicken_jockey(&w.mobs[&m]));
                    on_chickens += 1;
                }
            }
            w.mobs.clear();
        }
        let share = f64::from(babies) / 40_000.0;
        assert!((0.04..0.06).contains(&share), "{share}");
        // No chickens about: 5% of the other 95%.
        let jockey = f64::from(on_chickens) / f64::from(babies);
        assert!((0.02..0.08).contains(&jockey), "{jockey}");
    }

    #[test]
    fn a_baby_takes_a_chicken_close_by() {
        let mut w = flat_world();
        let chicken = add(&mut w, MobKind::Chicken, [2.5, -60.0, 2.5]);
        let mut took = false;
        for _ in 0..400 {
            let id = next_entity_id();
            let mut baby = Mob::new(MobKind::Zombie, [0.5, -60.0, 0.5], &mut w.rng);
            baby.age = -1;
            w.mobs.insert(id, baby);
            chicken_for(&mut w, id, [0.5, -60.0, 0.5], MobKind::Zombie);
            if w.mobs[&id].vehicle == Some(chicken) {
                took = true;
                break;
            }
            let others: Vec<i32> = w.mobs.keys().copied().filter(|&m| m != chicken).collect();
            for m in others {
                w.mobs.remove(&m);
            }
        }
        assert!(took);
        assert!(chicken_jockey(&w.mobs[&chicken]));
    }

    #[test]
    fn drowned_spawn_armed_and_throw_their_tridents() {
        let mut w = flat_world();
        let (mut tridents, mut rods, mut shells) = (0, 0, 0);
        for _ in 0..20_000 {
            let mut d = Mob::new(MobKind::Drowned, [0.5, -60.0, 0.5], &mut w.rng);
            arm_drowned(&mut w, &mut d);
            tridents += i32::from(holds(&d, MAIN_HAND, "minecraft:trident"));
            rods += i32::from(holds(&d, MAIN_HAND, "minecraft:fishing_rod"));
            shells += i32::from(holds(&d, OFF_HAND, "minecraft:nautilus_shell"));
        }
        let share = |n: i32| f64::from(n) / 20_000.0;
        assert!(
            (0.05..0.075).contains(&share(tridents)),
            "{}",
            share(tridents)
        );
        assert!((0.028..0.047).contains(&share(rods)), "{}", share(rods));
        assert!((0.022..0.038).contains(&share(shells)), "{}", share(shells));

        let player = add_player(&mut w, [10.5, -60.0, 0.5]);
        let mut d = Mob::new(MobKind::Drowned, [0.5, -60.0, 0.5], &mut w.rng);
        state(&mut d).equipment[MAIN_HAND] = item("minecraft:trident").unwrap();
        d.hunting = Some(player);
        let id = next_entity_id();
        throw_trident(&mut w, id, &mut d);
        assert_eq!(w.arrows.len(), 1, "thrown");
        assert!(w.arrows.values().all(|a| a.trident.is_some() && !a.pickup));
        throw_trident(&mut w, id, &mut d);
        assert_eq!(w.arrows.len(), 1, "not again at once");
        assert!(spawn_extras(id, &d).len() == 1, "shown holding it");
        let nbt = d.to_nbt();
        let main = nbt
            .compound("equipment")
            .and_then(|e| e.compound("mainhand"));
        assert_eq!(main.and_then(|m| m.str("id")), Some("minecraft:trident"));
    }

    #[test]
    fn reinforcements_come_at_the_spot_odds_and_hunt_the_attacker() {
        let mut w = flat_world();
        // Night, so that it is dark enough.
        w.day_time = 18_000;
        let player = add_player(&mut w, [0.5, -60.0, 0.5]);
        let mut zombie = Mob::new(MobKind::Zombie, [2.5, -60.0, 2.5], &mut w.rng);
        state(&mut zombie).reinforce = Some(1.0);
        let mut called = None;
        for _ in 0..20 {
            called = reinforce(&mut w, &mut zombie, player);
            if called.is_some() {
                break;
            }
            state(&mut zombie).reinforce = Some(1.0);
        }
        let help = called.expect("a spot was found");
        let h = &w.mobs[&help];
        assert_eq!(h.kind, MobKind::Zombie);
        assert_eq!(h.hunting, Some(player));
        let away = (0..3).map(|i| (h.pos[i] - 0.5).powi(2)).sum::<f64>().sqrt();
        assert!(away >= 7.0, "{away}");
        assert!(
            zombie.jockey.as_ref().unwrap().reinforce.unwrap() < 1.0,
            "less likely now"
        );

        let mut pig = Mob::new(MobKind::ZombifiedPiglin, [2.5, -60.0, 2.5], &mut w.rng);
        assert_eq!(
            reinforce_chance(&mut w, &mut pig),
            0.0,
            "zombified piglins call none"
        );
    }

    #[test]
    fn regional_difficulty_follows_the_wiki_ranges() {
        let mut w = flat_world();
        w.day_time = 0;
        assert_eq!(regional_difficulty(&w, Difficulty::Easy), 0.75);
        assert_eq!(regional_difficulty(&w, Difficulty::Normal), 1.5);
        assert_eq!(regional_difficulty(&w, Difficulty::Hard), 2.25);
        w.day_time = 96 * 24_000; // full moon, long after day 63
        assert!((regional_difficulty(&w, Difficulty::Hard) - 3.75).abs() < 1e-9);
        assert_eq!(regional_difficulty(&w, Difficulty::Peaceful), 0.0);
    }

    #[test]
    fn a_skeleton_trap_turns_into_four_horsemen_when_a_player_comes_near() {
        let mut w = flat_world();
        let trap = spawn_trap(&mut w, [0.5, -60.0, 0.5]);
        assert!(!w.bolts.is_empty(), "shown with lightning");
        let nbt = w.mobs[&trap].to_nbt();
        assert_eq!(nbt.int("SkeletonTrap"), Some(1));
        w.tick_mobs();
        assert_eq!(w.mobs.len(), 1, "nobody near yet");

        add_player(&mut w, [6.5, -60.0, 0.5]);
        w.tick_mobs();
        let horses: Vec<i32> = w
            .mobs
            .iter()
            .filter(|(_, m)| m.kind == MobKind::SkeletonHorse)
            .map(|(&id, _)| id)
            .collect();
        let skeletons: Vec<&Mob> = w
            .mobs
            .values()
            .filter(|m| m.kind == MobKind::Skeleton)
            .collect();
        assert_eq!(horses.len(), 4);
        assert_eq!(skeletons.len(), 4);
        for s in &skeletons {
            assert!(s.persistent);
            assert!(wears_helmet(s));
            let j = s.jockey.as_ref().unwrap();
            assert!(!j.equipment[HEAD].enchants.is_empty(), "enchanted helmet");
            assert!(!j.equipment[MAIN_HAND].enchants.is_empty(), "enchanted bow");
            let mount = j.mount.expect("on a horse");
            assert!(horses.contains(&mount));
            assert_eq!(s.vehicle, Some(mount));
        }
        for h in &horses {
            let m = &w.mobs[h];
            assert_eq!(m.jockey.as_ref().unwrap().passengers.len(), 1);
            assert!(m.jockey.as_ref().unwrap().trap.is_none());
            assert!(m.persistent);
        }
        assert_eq!(w.mobs[&trap].to_nbt().int("SkeletonTrap"), Some(0));
    }

    #[test]
    fn a_trap_goes_away_after_fifteen_minutes() {
        let mut w = flat_world();
        let trap = spawn_trap(&mut w, [0.5, -60.0, 0.5]);
        state(w.mobs.get_mut(&trap).unwrap()).trap = Some(TRAP_TICKS - 1);
        w.tick_mobs();
        assert!(!w.mobs.contains_key(&trap));
    }
}
