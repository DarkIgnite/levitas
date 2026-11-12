//! Dolphins.
//!
//! Behaviour follows the Minecraft Wiki (Dolphin, Dolphin's Grace):
//! - Moisture: in water or rain a dolphin's moisture is full (2400 ticks); out of both
//!   it falls by one a tick, and at zero the dolphin dries out, taking damage.
//! - Air: dolphins breathe air. Submerged, they last about four minutes (4800 ticks)
//!   before drowning; running low, they swim up to the surface for a breath. Water
//!   Breathing keeps them from drowning.
//! - Dolphin's Grace: a player sprint-swimming within 9 blocks of a dolphin gets the
//!   effect for 5 seconds, renewed while they keep sprint-swimming within 15 blocks;
//!   the dolphin follows such a player.
//! - Fed raw fish (cod, salmon, pufferfish or tropical fish), a dolphin swims towards
//!   the nearest structure of the `minecraft:dolphin_located` tag (ocean ruins and
//!   shipwrecks), leading the player there. A baby fed fish grows up a tenth faster.
//! - Neutral: when a player hurts one (and does not kill it at once), it and the
//!   adult dolphins of its pod attack that player: 2.5 damage on Easy, 3 on Normal and
//!   4.5 on Hard, none on Peaceful. Babies are passive. Hostile dolphins stay hostile
//!   when fed.
//! - On land, a dolphin looks for water nearby.
//! - 10% of dolphins spawn as babies.

use levitas_nbt::Compound;
use levitas_registry::mob_effect;

use super::mobs::{Mob, MobKind};
use super::survival::Damage;
use super::*;
use crate::rules::Difficulty;

/// Moisture when wet, in ticks out of water before drying out.
const MAX_MOISTURE: i32 = 2400;
/// Air under water when full, in ticks.
const MAX_AIR: i32 = 4800;
/// Below this much air a dolphin heads for the surface.
const SURFACE_BELOW: i32 = 1200;
/// How high a dolphin's eyes are above its feet.
const EYE_HEIGHT: f64 = 0.3;
/// Dolphin's Grace: granted within this range, renewed within the larger one, for
/// this many ticks.
const GRACE_RANGE: f64 = 9.0;
const GRACE_KEEP_RANGE: f64 = 15.0;
const GRACE_TICKS: i32 = 100;
/// Renew the effect once fewer ticks than this are left of it.
const GRACE_RENEW_BELOW: i32 = 80;
/// How far the pod of a hurt dolphin joins in, and how far they chase.
const POD_RANGE: f64 = 16.0;
const CHASE_RANGE: f64 = 32.0;
const REACH: f64 = 1.8;
const ATTACK_COOLDOWN: u32 = 20;
/// How many placement regions out the search for a structure goes (about 50 chunks).
const SEARCH_REGIONS: i32 = 2;
/// The structures of the `minecraft:dolphin_located` tag (`#ocean_ruin`, `#shipwreck`).
const LOCATED: [&str; 4] = [
    "minecraft:ocean_ruin_cold",
    "minecraft:ocean_ruin_warm",
    "minecraft:shipwreck",
    "minecraft:shipwreck_beached",
];
/// Ticks a dolphin keeps leading towards its structure before giving up.
const LEAD_TICKS: u32 = 2400;
/// Close enough to the structure, horizontally.
const ARRIVED: f64 = 4.0;
/// How far ahead a leading dolphin aims each time.
const LEAD_STEP: f64 = 8.0;
/// How far a stranded dolphin looks for water.
const WATER_SEARCH: i32 = 8;

/// Entity data after Ageable Mob's baby (16) and age-locked (17) flags.
const DATA_GOT_FISH: u8 = 18;
const DATA_MOISTURE: u8 = 19;
const SERIALIZER_INT: i32 = 1;
const SERIALIZER_BOOL: i32 = 8;
/// The spectator game mode, which mobs ignore like creative.
const GAME_MODE_SPECTATOR: u8 = 3;

/// A dolphin's own state, kept in `Mob::kin`.
#[derive(Debug, Clone)]
pub struct Dolphin {
    pub moisture: i32,
    pub air: i32,
    /// Whether a player gave it fish.
    pub got_fish: bool,
    /// The structure it is leading to, and for how much longer.
    pub treasure: Option<[i32; 3]>,
    lead_ticks: u32,
}

impl Default for Dolphin {
    fn default() -> Self {
        Self {
            moisture: MAX_MOISTURE,
            air: MAX_AIR,
            got_fish: false,
            treasure: None,
            lead_ticks: 0,
        }
    }
}

fn state(mob: &mut Mob) -> &mut Dolphin {
    if !mob.kin.as_ref().is_some_and(|k| k.is::<Dolphin>()) {
        mob.kin = Some(Box::new(Dolphin::default()));
    }
    mob.kin
        .as_mut()
        .and_then(|k| k.downcast_mut::<Dolphin>())
        .expect("just made")
}

fn peek(mob: &Mob) -> Option<&Dolphin> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Dolphin>())
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn block_at(pos: [f64; 3]) -> BlockPos {
    BlockPos::new(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    )
}

/// A dolphin's bite on the current difficulty (none on Peaceful).
fn bite() -> f32 {
    match crate::rules::difficulty() {
        Difficulty::Peaceful => 0.0,
        Difficulty::Easy => 2.5,
        Difficulty::Normal => 3.0,
        Difficulty::Hard => 4.5,
    }
}

pub(super) fn spawned(w: &mut World, mob: &mut Mob, _biome: &str) {
    if mob.age == 0 && w.rng.below(10) == 0 {
        mob.age = -24_000;
    }
    state(mob);
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    let wet = w.in_water(mob.pos) || w.raining_at(block_at(mob.pos));
    {
        let s = state(mob);
        if wet {
            s.moisture = MAX_MOISTURE;
        } else {
            s.moisture -= 1;
        }
    }
    if state(mob).moisture <= 0 && w.tick.is_multiple_of(20) {
        w.hurt_mob(id, mob, 1.0, "minecraft:dry_out");
    }
    if mob.alive() {
        breathe(w, id, mob);
    }
    if !mob.alive() {
        return false;
    }
    grant_grace(w, mob);
    if w.in_water(mob.pos) {
        swim_goal(w, id, mob);
    } else {
        seek_water(w, mob);
    }
    false
}

/// Air: used up under water, refilled at the surface; drowning when it runs out.
fn breathe(w: &mut World, id: i32, mob: &mut Mob) {
    let eye = [mob.pos[0], mob.pos[1] + EYE_HEIGHT, mob.pos[2]];
    let submerged = w.in_water(eye);
    let protected = mob.effect_level(mob_effect::WATER_BREATHING) > 0;
    let s = state(mob);
    if !submerged {
        s.air = MAX_AIR;
        return;
    }
    if protected {
        return;
    }
    s.air -= 1;
    if s.air <= -20 {
        s.air = 0;
        w.hurt_mob(id, mob, 2.0, "minecraft:drown");
    }
}

/// Dolphin's Grace for players sprint-swimming about.
fn grant_grace(w: &mut World, mob: &Mob) {
    let hunted = mob.hunting;
    let near: Vec<i32> = w
        .players
        .iter()
        .filter(|(&pid, p)| {
            if !p.alive || !p.swimming || p.mode == GAME_MODE_SPECTATOR || Some(pid) == hunted {
                return false;
            }
            let d = distance(p.pos, mob.pos);
            let has = p.effect(mob_effect::DOLPHINS_GRACE);
            let range = if has.is_some() {
                GRACE_KEEP_RANGE
            } else {
                GRACE_RANGE
            };
            d <= range && has.is_none_or(|e| e.duration < GRACE_RENEW_BELOW)
        })
        .map(|(&pid, _)| pid)
        .collect();
    for pid in near {
        w.add_effect(
            pid,
            levitas_registry::Effect {
                id: mob_effect::DOLPHINS_GRACE,
                duration: GRACE_TICKS,
                amplifier: 0,
            },
        );
    }
}

/// Where a dolphin in water heads: the surface for air, its prey, its structure, or a
/// sprint-swimming player. Otherwise the shared swim AI wanders.
fn swim_goal(w: &mut World, id: i32, mob: &mut Mob) {
    mob.attack_cooldown = mob.attack_cooldown.saturating_sub(1);
    // Air comes first, even mid-fight; it still bites on the way up if it can.
    if state(mob).air < SURFACE_BELOW {
        if let Some(goal) = surface_above(w, mob.pos) {
            chase(w, id, mob);
            mob.fly_goal = Some(goal);
            return;
        }
    }
    if chase(w, id, mob) {
        return;
    }
    if lead(w, mob) {
        return;
    }
    let swimmer = w
        .players
        .values()
        .filter(|p| {
            p.alive
                && p.swimming
                && p.mode != GAME_MODE_SPECTATOR
                && distance(p.pos, mob.pos) <= GRACE_KEEP_RANGE
        })
        .map(|p| p.pos)
        .min_by(|a, b| distance(*a, mob.pos).total_cmp(&distance(*b, mob.pos)));
    if let Some(at) = swimmer {
        mob.fly_goal = Some(at);
    }
}

/// An angry dolphin goes after the player who hurt its pod. Returns whether it is.
fn chase(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    let Some(pid) = mob.hunting else {
        return false;
    };
    let prey = w
        .players
        .get(&pid)
        .filter(|p| p.alive && p.mode != GAME_MODE_CREATIVE && p.mode != GAME_MODE_SPECTATOR)
        .map(|p| p.pos)
        .filter(|&at| distance(at, mob.pos) <= CHASE_RANGE);
    let Some(at) = prey.filter(|_| mob.age >= 0 && bite() > 0.0) else {
        mob.hunting = None;
        return false;
    };
    mob.panic = 0;
    mob.fly_goal = Some([at[0], at[1] + 0.5, at[2]]);
    if mob.attack_cooldown == 0 && distance(at, mob.pos) <= REACH {
        mob.attack_cooldown = ATTACK_COOLDOWN;
        if w.damage_from(pid, bite(), Damage::Mob("Dolphin"), Some(id)) {
            w.play_sound_at("minecraft:entity.dolphin.attack", mob.pos, 1.0, 1.0, None);
        }
    }
    true
}

/// The top of the water above `pos`, where a dolphin can breathe, searched up the
/// whole water column so deep-ocean dolphins find it too.
fn surface_above(w: &World, pos: [f64; 3]) -> Option<[f64; 3]> {
    let base = block_at(pos);
    (base.y..levitas_world::MAX_Y)
        .find(|&y| !w.in_water([pos[0], f64::from(y) + 1.0 + EYE_HEIGHT * 0.5, pos[2]]))
        .map(|y| [pos[0], f64::from(y) + 0.95, pos[2]])
}

/// A fed dolphin swims towards its structure. Returns whether it is leading.
fn lead(w: &World, mob: &mut Mob) -> bool {
    let pos = mob.pos;
    let s = state(mob);
    let Some([tx, _, tz]) = s.treasure else {
        return false;
    };
    let dx = f64::from(tx) + 0.5 - pos[0];
    let dz = f64::from(tz) + 0.5 - pos[2];
    let flat = dx.hypot(dz);
    s.lead_ticks = s.lead_ticks.saturating_sub(1);
    if flat < ARRIVED || s.lead_ticks == 0 {
        s.treasure = None;
        s.lead_ticks = 0;
        return false;
    }
    let step = flat.min(LEAD_STEP);
    let ahead = [pos[0] + dx / flat * step, pos[1], pos[2] + dz / flat * step];
    // Keep to the water: down a little if the way ahead is shallower.
    let goal = (0..4)
        .map(|dy| [ahead[0], ahead[1] - f64::from(dy), ahead[2]])
        .find(|&g| w.in_water(g))
        .unwrap_or(ahead);
    mob.panic = 0;
    mob.fly_goal = Some(goal);
    true
}

/// A dolphin on land makes for the nearest water.
fn seek_water(w: &mut World, mob: &mut Mob) {
    if mob.target.is_some() || !w.tick.is_multiple_of(20) {
        return;
    }
    let base = block_at(mob.pos);
    let mut best: Option<(i32, [f64; 2])> = None;
    for dx in -WATER_SEARCH..=WATER_SEARCH {
        for dz in -WATER_SEARCH..=WATER_SEARCH {
            let d = dx * dx + dz * dz;
            if best.is_some_and(|(b, _)| b <= d) {
                continue;
            }
            for dy in -2..=1 {
                let at = [
                    f64::from(base.x + dx) + 0.5,
                    f64::from(base.y + dy) + 0.5,
                    f64::from(base.z + dz) + 0.5,
                ];
                if w.in_water(at) {
                    best = Some((d, [at[0], at[2]]));
                    break;
                }
            }
        }
    }
    if let Some((_, at)) = best {
        mob.target = Some((at, 100));
    }
}

/// The nearest structure a dolphin can lead to from `pos`, if any.
fn locate(w: &World, pos: [f64; 3]) -> Option<[i32; 3]> {
    let (x, z) = (pos[0].floor() as i32, pos[2].floor() as i32);
    LOCATED
        .iter()
        .filter_map(|s| w.terrain.structure_near(s, x, z, SEARCH_REGIONS))
        .min_by_key(|p| {
            let (dx, dz) = (i64::from(p[0] - x), i64::from(p[2] - z));
            dx * dx + dz * dz
        })
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    if !matches!(
        held,
        "minecraft:cod" | "minecraft:salmon" | "minecraft:pufferfish" | "minecraft:tropical_fish"
    ) {
        return false;
    }
    let Some(mut mob) = w.mobs.remove(&target) else {
        return false;
    };
    if !mob.alive() {
        w.mobs.insert(target, mob);
        return false;
    }
    if mob.age < 0 {
        mob.age += -mob.age / 10;
    }
    let found = locate(w, mob.pos);
    {
        let s = state(&mut mob);
        s.got_fish = true;
        s.treasure = found;
        s.lead_ticks = if found.is_some() { LEAD_TICKS } else { 0 };
    }
    w.play_sound_at("minecraft:entity.dolphin.eat", mob.pos, 0.75, 1.0, None);
    w.send_to_viewers(&mob, got_fish_data(target, true));
    w.mobs.insert(target, mob);
    if let Some(p) = w.players.get_mut(&player) {
        if p.survival() {
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            w.send(player, update);
        }
    }
    true
}

pub(super) fn hurt(w: &mut World, _id: i32, mob: &mut Mob, attacker: Option<i32>) {
    let Some(attacker) = attacker else {
        return;
    };
    if mob.health <= 0.0 {
        return;
    }
    if mob.age >= 0 {
        mob.hunting = Some(attacker);
    }
    let at = mob.pos;
    for m in w.mobs.values_mut() {
        if m.kind == MobKind::Dolphin && m.alive() && m.age >= 0 && distance(m.pos, at) <= POD_RANGE
        {
            m.hunting = Some(attacker);
        }
    }
}

fn got_fish_data(id: i32, got: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_GOT_FISH)
        .varint(SERIALIZER_BOOL)
        .bool(got)
        .u8(0xFF)
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let s = peek(mob).cloned().unwrap_or_default();
    vec![Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_GOT_FISH)
        .varint(SERIALIZER_BOOL)
        .bool(s.got_fish)
        .u8(DATA_MOISTURE)
        .varint(SERIALIZER_INT)
        .varint(s.moisture)
        .u8(0xFF)]
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let s = peek(mob).cloned().unwrap_or_default();
    nbt.insert("Moistness", s.moisture);
    nbt.insert("GotFish", s.got_fish);
    nbt.insert("Air", s.air.clamp(i32::from(i16::MIN), MAX_AIR) as i16);
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let s = state(mob);
    if let Some(m) = nbt.int("Moistness") {
        s.moisture = m.clamp(i64::from(i32::MIN), i64::from(MAX_MOISTURE)) as i32;
    }
    s.got_fish = nbt.int("GotFish").unwrap_or(0) != 0;
    if let Some(a) = nbt.int("Air") {
        s.air = a.clamp(-20, i64::from(MAX_AIR)) as i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    /// A flat world with a 13 x 13 pool three blocks deep (water from y -63 to -61,
    /// air above), and a player at its middle.
    fn pool() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-dolphins-{}-{}",
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
        for x in 0..13 {
            for z in 0..13 {
                for y in -63..-60 {
                    w.set_block(BlockPos::new(x, y, z), block::WATER);
                }
            }
        }
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 12,
            properties: Vec::new(),
        };
        w.players
            .insert(alice, Player::new(profile, out, [6.5, -62.5, 6.5]));
        (w, alice)
    }

    fn dolphin(w: &mut World, at: [f64; 3]) -> Mob {
        let mut mob = Mob::new(MobKind::Dolphin, at, &mut w.rng);
        mob.age = 0;
        mob
    }

    fn hold(w: &mut World, id: i32, item: &str) {
        w.players.get_mut(&id).unwrap().inventory.slots[36] =
            Stack::new(ItemInfo::id_of(item).unwrap(), 1);
    }

    #[test]
    fn dries_out_on_land_after_two_minutes() {
        let (mut w, _) = pool();
        let mut d = dolphin(&mut w, [20.5, -60.0, 20.5]);
        let full = d.health;
        for _ in 0..MAX_MOISTURE - 40 {
            w.tick += 1;
            tick(&mut w, 1, &mut d);
        }
        assert_eq!(d.health, full, "still moist");
        for _ in 0..100 {
            w.tick += 1;
            tick(&mut w, 1, &mut d);
        }
        assert!(d.health < full, "dried out");
        // Back in the water it is wet again.
        d.pos = [6.5, -62.5, 6.5];
        tick(&mut w, 1, &mut d);
        assert_eq!(state(&mut d).moisture, MAX_MOISTURE);
    }

    #[test]
    fn stranded_dolphins_head_for_water() {
        let (mut w, _) = pool();
        let mut d = dolphin(&mut w, [16.5, -60.0, 6.5]);
        w.tick = 20;
        tick(&mut w, 1, &mut d);
        let ([x, _], _) = d.target.expect("found water");
        assert!(x < 13.0, "towards the pool");
    }

    #[test]
    fn drowns_without_air_and_surfaces_to_breathe() {
        let (mut w, _) = pool();
        let mut d = dolphin(&mut w, [6.5, -62.8, 6.5]);
        let full = d.health;
        for _ in 0..MAX_AIR - SURFACE_BELOW + 1 {
            breathe(&mut w, 1, &mut d);
        }
        assert_eq!(d.health, full);
        swim_goal(&mut w, 1, &mut d);
        let goal = d.fly_goal.expect("heading up");
        assert!(
            goal[1] > -61.0 && goal[1] < -60.0,
            "to the top of the water"
        );
        assert!(w.in_water(goal));
        assert!(!w.in_water([goal[0], goal[1] + EYE_HEIGHT, goal[2]]), "air");
        // Air before a fight: an angry dolphin low on air still goes up.
        let alice = *w.players.keys().next().unwrap();
        d.hunting = Some(alice);
        d.fly_goal = None;
        swim_goal(&mut w, 1, &mut d);
        assert_eq!(d.fly_goal, Some(goal), "surfaces rather than chases");
        d.hunting = None;
        for _ in 0..SURFACE_BELOW + 40 {
            breathe(&mut w, 1, &mut d);
        }
        assert!(d.health < full, "drowning");
        // At the surface it breathes again.
        d.pos = goal;
        breathe(&mut w, 1, &mut d);
        assert_eq!(state(&mut d).air, MAX_AIR);
        // Water Breathing keeps the air in.
        d.pos = [6.5, -62.8, 6.5];
        d.effects.push(super::super::effects::ActiveEffect {
            id: mob_effect::WATER_BREATHING,
            amplifier: 0,
            duration: 1000,
        });
        breathe(&mut w, 1, &mut d);
        assert_eq!(state(&mut d).air, MAX_AIR);
    }

    #[test]
    fn finds_the_surface_of_deep_water() {
        let (mut w, _) = pool();
        for y in -60..-10 {
            w.set_block(BlockPos::new(3, y, 3), block::WATER);
        }
        let goal = surface_above(&w, [3.5, -62.5, 3.5]).expect("surface");
        assert!(
            goal[1] > -11.0 && goal[1] < -10.0,
            "top of a 53-deep column"
        );
    }

    #[test]
    fn sprint_swimmers_nearby_get_dolphins_grace() {
        let (mut w, alice) = pool();
        let mut d = dolphin(&mut w, [2.5, -62.5, 6.5]);
        tick(&mut w, 1, &mut d);
        assert!(w.players[&alice]
            .effect(mob_effect::DOLPHINS_GRACE)
            .is_none());
        w.players.get_mut(&alice).unwrap().swimming = true;
        tick(&mut w, 1, &mut d);
        let grace = w.players[&alice].effect(mob_effect::DOLPHINS_GRACE);
        assert_eq!(grace.map(|e| e.duration), Some(GRACE_TICKS));
        assert_eq!(d.fly_goal, Some(w.players[&alice].pos), "follows them");
        // Spectators get nothing and are not followed.
        let (mut w, eve) = pool();
        {
            let p = w.players.get_mut(&eve).unwrap();
            p.swimming = true;
            p.mode = GAME_MODE_SPECTATOR;
        }
        let mut d = dolphin(&mut w, [2.5, -62.5, 6.5]);
        tick(&mut w, 1, &mut d);
        assert!(w.players[&eve].effect(mob_effect::DOLPHINS_GRACE).is_none());
        assert_ne!(d.fly_goal, Some(w.players[&eve].pos));
        // Too far away: nothing.
        let (mut w, bob) = pool();
        w.players.get_mut(&bob).unwrap().swimming = true;
        let mut far = dolphin(&mut w, [6.5, -62.5, 6.5]);
        far.pos[0] += GRACE_RANGE + 1.0;
        grant_grace(&mut w, &far);
        assert!(w.players[&bob].effect(mob_effect::DOLPHINS_GRACE).is_none());
    }

    #[test]
    fn eats_fish_but_finds_nothing_in_a_flat_world() {
        let (mut w, alice) = pool();
        let id = next_entity_id();
        let mut baby = dolphin(&mut w, [4.5, -62.5, 4.5]);
        baby.age = -24_000;
        w.mobs.insert(id, baby);
        hold(&mut w, alice, "minecraft:dirt");
        assert!(!use_on(&mut w, alice, id, 36, "minecraft:dirt"));
        hold(&mut w, alice, "minecraft:cod");
        assert!(use_on(&mut w, alice, id, 36, "minecraft:cod"));
        assert!(w.players[&alice].inventory.slots[36].is_empty(), "eaten");
        let m = w.mobs.get_mut(&id).unwrap();
        assert_eq!(m.age, -21_600, "grew a tenth");
        let s = state(m);
        assert!(s.got_fish);
        assert_eq!(s.treasure, None, "no ocean ruins on a flat world");
        // Any raw fish will do; cooked fish will not.
        hold(&mut w, alice, "minecraft:tropical_fish");
        assert!(use_on(&mut w, alice, id, 36, "minecraft:tropical_fish"));
        hold(&mut w, alice, "minecraft:cooked_cod");
        assert!(!use_on(&mut w, alice, id, 36, "minecraft:cooked_cod"));
        // A dying dolphin takes nothing.
        let mut m = w.mobs.remove(&id).unwrap();
        w.kill_mob(id, &mut m);
        w.mobs.insert(id, m);
        hold(&mut w, alice, "minecraft:cod");
        assert!(!use_on(&mut w, alice, id, 36, "minecraft:cod"));
        assert!(!w.players[&alice].inventory.slots[36].is_empty(), "kept");
    }

    #[test]
    fn fed_dolphins_lead_towards_their_structure() {
        let (mut w, _) = pool();
        let mut d = dolphin(&mut w, [2.5, -62.5, 6.5]);
        {
            let s = state(&mut d);
            s.got_fish = true;
            s.treasure = Some([200, -62, 6]);
            s.lead_ticks = LEAD_TICKS;
        }
        swim_goal(&mut w, 1, &mut d);
        let goal = d.fly_goal.expect("leading");
        assert!(
            (goal[0] - (2.5 + LEAD_STEP)).abs() < 1e-9,
            "east, towards it"
        );
        // Arrived: it stops leading.
        state(&mut d).treasure = Some([3, -62, 6]);
        d.fly_goal = None;
        swim_goal(&mut w, 1, &mut d);
        assert_eq!(state(&mut d).treasure, None);
        assert_eq!(d.fly_goal, None);
    }

    #[test]
    fn hurting_one_turns_the_pod_on_the_player() {
        let (mut w, alice) = pool();
        let hit = next_entity_id();
        let near = next_entity_id();
        let far = next_entity_id();
        let calf = next_entity_id();
        let first = dolphin(&mut w, [5.5, -62.5, 6.5]);
        w.mobs.insert(hit, first);
        let second = dolphin(&mut w, [8.5, -62.5, 6.5]);
        w.mobs.insert(near, second);
        let third = dolphin(&mut w, [60.5, -62.5, 6.5]);
        w.mobs.insert(far, third);
        let mut fourth = dolphin(&mut w, [7.5, -62.5, 6.5]);
        fourth.age = -1000;
        w.mobs.insert(calf, fourth);
        assert!(w.hit_mob(hit, 1.0, Some(alice), [0.0, 0.0], "minecraft:player_attack"));
        assert_eq!(w.mobs[&hit].hunting, Some(alice));
        assert_eq!(w.mobs[&near].hunting, Some(alice));
        assert_eq!(w.mobs[&far].hunting, None, "out of range");
        assert_eq!(w.mobs[&calf].hunting, None, "babies are passive");

        // It bites.
        let health = w.players[&alice].health;
        let mut m = w.mobs.remove(&hit).unwrap();
        m.pos = [6.5, -62.5, 6.0];
        m.attack_cooldown = 0;
        swim_goal(&mut w, hit, &mut m);
        assert!(w.players[&alice].health < health, "bitten");
        assert_eq!(m.panic, 0, "fights rather than flees");
        // A creative player is left alone.
        w.players.get_mut(&alice).unwrap().mode = GAME_MODE_CREATIVE;
        swim_goal(&mut w, hit, &mut m);
        assert_eq!(m.hunting, None);
    }

    #[test]
    fn saves_and_loads_its_state() {
        let (mut w, _) = pool();
        let mut d = dolphin(&mut w, [6.5, -62.5, 6.5]);
        {
            let s = state(&mut d);
            s.moisture = 1234;
            s.air = 999;
            s.got_fish = true;
        }
        let mut nbt = Compound::new();
        write_nbt(&d, &mut nbt);
        let mut back = dolphin(&mut w, [6.5, -62.5, 6.5]);
        read_nbt(&mut back, &nbt);
        let s = state(&mut back);
        assert_eq!((s.moisture, s.air, s.got_fish), (1234, 999, true));
        assert_eq!(spawn_extras(5, &back).len(), 1);
    }
}
