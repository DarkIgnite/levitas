//! Llamas and trader llamas: spitting, and the carpets they wear.
//!
//! Taming, feeding, health, coats and strength live in `horses.rs`; this module adds
//! what only llamas do. Behaviour follows the Minecraft Wiki (Llama, Trader Llama and
//! Llama Spit pages):
//! - Llamas are neutral: they spit back at a player or mob that hurts them, and spit
//!   at wild (untamed) wolves without being provoked. A spit that strays into another
//!   llama starts a fight between the two. Trader llamas are also hostile towards
//!   zombies, husks, drowned, zombie villagers, pillagers, vindicators, evokers and
//!   illusioners, the mobs that go after their wandering trader. On Peaceful they never
//!   spit.
//! - Llama spit is a projectile with gravity 0.06 and drag 0.99 per tick that deals 1
//!   damage to what it hits (scaled by difficulty, as its damage type says, and blocked
//!   by a raised shield). It stops at any block, even non-solid ones and water.
//! - A tamed adult llama wears a wool carpet in its body slot (saved under
//!   `equipment.body`). Using a carpet puts one on only while it wears none and is not
//!   ridden; otherwise the click is left to `horses.rs` (mounting). Shears take it off
//!   (not while it is ridden).

use levitas_nbt::{Compound, Tag};

use super::mobs::{yaw_towards, Mob, MobKind};
use super::*;
use crate::world::survival::Damage;

/// Equipment slot of body armour, where a llama wears its carpet.
const SLOT_BODY: u8 = 6;
/// How far a llama sees what it spits at, and gives up on a foe.
const SPIT_RANGE: f64 = 20.0;
const FORGET_RANGE: f64 = 32.0;
/// How far a llama notices wild wolves, and a trader llama the mobs it fights.
const NOTICE_RANGE: f64 = 16.0;
/// Ticks between spits, and before the first one at a new foe.
const SPIT_INTERVAL: u32 = 40;
const SPIT_WINDUP: u32 = 10;
/// Llama spit: its speed, pull of gravity, drag, size, damage and lifetime.
const SPIT_SPEED: f64 = 1.5;
const SPIT_GRAVITY: f64 = 0.06;
const SPIT_DRAG: f64 = 0.99;
const SPIT_RADIUS: f64 = 0.125;
const SPIT_DAMAGE: f32 = 1.0;
const SPIT_LIFE: u32 = 100;
/// Steps a spit's flight is checked in, each tick.
const SPIT_SUBSTEPS: u32 = 4;
const PLAYER_WIDTH: f64 = 0.6;
const PLAYER_HEIGHT: f64 = 1.8;

/// What a llama is angry with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foe {
    Player(i32),
    Mob(i32),
}

/// A spit in flight.
#[derive(Debug, Clone)]
pub struct Spit {
    id: i32,
    pos: [f64; 3],
    vel: [f64; 3],
    age: u32,
}

/// A llama's own state, kept in `Mob::kin`.
#[derive(Debug, Default)]
pub struct Llama {
    /// The carpet it wears (an item name), if any.
    pub carpet: Option<String>,
    pub foe: Option<Foe>,
    /// Ticks until it may spit again.
    pub cooldown: u32,
    /// Its spits still in flight.
    pub spits: Vec<Spit>,
}

fn is_llama(kind: MobKind) -> bool {
    matches!(kind, MobKind::Llama | MobKind::TraderLlama)
}

/// The llama state of a mob, made on first use.
fn state(mob: &mut Mob) -> &mut Llama {
    if !mob.kin.as_ref().is_some_and(|k| k.is::<Llama>()) {
        mob.kin = Some(Box::new(Llama::default()));
    }
    mob.kin
        .as_mut()
        .and_then(|k| k.downcast_mut::<Llama>())
        .expect("just made")
}

fn peek(mob: &Mob) -> Option<&Llama> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Llama>())
}

/// The carpet a llama wears, if any.
pub fn carpet(mob: &Mob) -> Option<&str> {
    peek(mob).and_then(|l| l.carpet.as_deref())
}

fn is_carpet(item: &str) -> bool {
    levitas_registry::item_has_tag(item, "minecraft:wool_carpets")
}

/// Mobs a trader llama fights to keep its trader safe.
fn trader_enemy(kind: MobKind) -> bool {
    matches!(
        kind,
        MobKind::Zombie
            | MobKind::Husk
            | MobKind::Drowned
            | MobKind::ZombieVillager
            | MobKind::Pillager
            | MobKind::Vindicator
            | MobKind::Evoker
            | MobKind::Illusioner
    )
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn angle(degrees: f64) -> u8 {
    (degrees.rem_euclid(360.0) / 360.0 * 256.0) as i32 as u8
}

/// The carpet a llama wears, or none, as its viewers should see it.
fn carpet_packet(id: i32, carpet: Option<&str>) -> Writer {
    let packet = Writer::packet(clientbound::SET_EQUIPMENT)
        .varint(id)
        .u8(SLOT_BODY);
    let stack = carpet
        .and_then(ItemInfo::id_of)
        .map_or(Stack::EMPTY, |item| Stack::new(item, 1));
    crate::inventory::write_stack(packet, stack)
}

fn spit_spawn(spit: &Spit, shooter: i32) -> Writer {
    let v = spit.vel;
    let yaw = (-v[0].atan2(v[2])).to_degrees();
    let pitch = (-v[1].atan2(v[0].hypot(v[2]))).to_degrees();
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(spit.id)
        .uuid(spit.id as u128)
        .varint(entity_type::LLAMA_SPIT)
        .f64(spit.pos[0])
        .f64(spit.pos[1])
        .f64(spit.pos[2])
        .lp_vec3(spit.vel)
        .u8(angle(pitch))
        .u8(angle(yaw))
        .u8(angle(yaw))
        .varint(shooter)
}

/// One tick of a spit's flight: it moves, slows and falls.
fn fly(spit: &mut Spit) {
    for (p, v) in spit.pos.iter_mut().zip(spit.vel) {
        *p += v;
    }
    for v in &mut spit.vel {
        *v *= SPIT_DRAG;
    }
    spit.vel[1] -= SPIT_GRAVITY;
    spit.age += 1;
}

/// The velocity that sends a spit from `from` to `to`, aimed high by as much as it
/// will fall on the way.
fn aim(from: [f64; 3], to: [f64; 3]) -> [f64; 3] {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let flat = d[0].hypot(d[2]);
    // Ticks of flight, and the drop over them (gravity adds up tick by tick).
    let ticks = flat / SPIT_SPEED;
    let drop = SPIT_GRAVITY * ticks * (ticks + 1.0) / 2.0;
    let d = [d[0], d[1] + drop, d[2]];
    let len = distance(d, [0.0; 3]).max(1e-6);
    d.map(|c| c / len * SPIT_SPEED)
}

/// What a spit hits on its way.
enum Hit {
    Block,
    Player(i32),
    Mob(i32),
}

/// Whether `at` is inside a box of `width` and `height` standing at `feet`, grown by
/// the size of a spit.
fn inside(at: [f64; 3], feet: [f64; 3], width: f64, height: f64) -> bool {
    let half = width / 2.0 + SPIT_RADIUS;
    (at[0] - feet[0]).abs() <= half
        && (at[2] - feet[2]).abs() <= half
        && at[1] >= feet[1] - SPIT_RADIUS
        && at[1] <= feet[1] + height + SPIT_RADIUS
}

pub(super) fn spawned(_w: &mut World, _mob: &mut Mob, _biome: &str) {}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if !is_llama(mob.kind) {
        return false;
    }
    let mut llama = std::mem::take(state(mob));
    let hits = w.fly_spits(id, &mut llama);
    if !hits.is_empty() {
        *state(mob) = llama;
        w.land_spits(id, mob, hits);
        llama = std::mem::take(state(mob));
    }
    if crate::rules::difficulty() == crate::rules::Difficulty::Peaceful {
        llama.foe = None;
    }
    llama.cooldown = llama.cooldown.saturating_sub(1);
    w.check_foe(id, mob, &mut llama);
    if let Some(foe) = llama.foe {
        w.fight(id, mob, &mut llama, foe);
    }
    *state(mob) = llama;
    false
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    let Some(mob) = w.mobs.get(&target) else {
        return false;
    };
    let tame = mob.owner.is_some();
    let adult = mob.age >= 0;
    let ridden = mob.horse.as_ref().is_some_and(|h| h.rider.is_some());
    let worn = carpet(mob).map(str::to_owned);
    let survival = w.players.get(&player).is_some_and(|p| p.survival());
    // Only onto a bare back, and not while ridden: otherwise the click mounts it.
    if is_carpet(held) && tame && adult && !ridden && worn.is_none() {
        let mob = w.mobs.get_mut(&target).expect("checked");
        state(mob).carpet = Some(held.to_owned());
        let pos = mob.pos;
        let m = w.mobs.remove(&target).expect("checked");
        w.send_to_viewers(&m, carpet_packet(target, Some(held)));
        w.mobs.insert(target, m);
        w.play_sound_at("minecraft:entity.llama.swag", pos, 0.5, 1.0, None);
        if survival {
            let p = w.players.get_mut(&player).expect("player exists");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            w.send(player, update);
        }
        return true;
    }
    if held == "minecraft:shears" && !ridden {
        let Some(old) = worn else {
            return false;
        };
        let mob = w.mobs.get_mut(&target).expect("checked");
        state(mob).carpet = None;
        let pos = mob.pos;
        let m = w.mobs.remove(&target).expect("checked");
        w.send_to_viewers(&m, carpet_packet(target, None));
        w.mobs.insert(target, m);
        w.play_sound_at("minecraft:item.llama_carpet.unequip", pos, 1.0, 1.0, None);
        if let Some(item) = ItemInfo::id_of(&old) {
            w.scatter_item([pos[0], pos[1] + 1.0, pos[2]], Stack::new(item, 1));
        }
        if survival {
            w.damage_item(player, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
        }
        return true;
    }
    false
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, attacker: Option<i32>) {
    if !is_llama(mob.kind) {
        return;
    }
    if mob.health <= 0.0 {
        // A dying llama no longer ticks its spits: take them off the clients now.
        if let Some(llama) = mob.kin.as_mut().and_then(|k| k.downcast_mut::<Llama>()) {
            for spit in llama.spits.drain(..) {
                w.broadcast(items::remove_entity(spit.id), None);
            }
        }
        return;
    }
    match attacker {
        Some(pid) => {
            if w.players.get(&pid).is_some_and(|p| p.survival()) {
                provoke(state(mob), Foe::Player(pid));
            }
        }
        // The hook names only players; a mob that hurt it is one going after it.
        None => {
            if let Some(mid) = w.hunter_of(id, mob.pos) {
                provoke(state(mob), Foe::Mob(mid));
            }
        }
    }
}

/// Turns a llama on a new foe; it winds up before the first spit, but being hit again
/// by the same foe does not hold it back.
fn provoke(llama: &mut Llama, foe: Foe) {
    if llama.foe != Some(foe) {
        llama.foe = Some(foe);
        llama.cooldown = llama.cooldown.max(SPIT_WINDUP);
    }
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    match carpet(mob) {
        Some(c) if is_llama(mob.kind) => vec![carpet_packet(id, Some(c))],
        _ => Vec::new(),
    }
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    if !is_llama(mob.kind) {
        return;
    }
    // `horses.rs` has written what else it wears (kept from when it was loaded).
    let mut equipment = nbt
        .get("equipment")
        .and_then(Tag::as_compound)
        .cloned()
        .unwrap_or_default();
    equipment.remove("body");
    if let Some(c) = carpet(mob) {
        equipment.insert("body", Compound::new().with("id", c).with("count", 1i32));
    }
    // Written even when empty, so that a carpet taken off is not brought back from the
    // copy kept of what was loaded.
    nbt.insert("equipment", equipment);
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    if !is_llama(mob.kind) {
        return;
    }
    let body = match nbt.get("equipment").and_then(Tag::as_compound) {
        Some(equipment) => equipment.get("body"),
        // Before 1.21.5 the carpet was kept as body armour of its own. Only then: the old
        // tags are carried along in the kept copy, and once `equipment` is written they
        // must not bring back a carpet that was taken off.
        None => nbt.get("body_armor_item").or_else(|| nbt.get("DecorItem")),
    }
    .and_then(Tag::as_compound)
    .and_then(|b| b.str("id"))
    .filter(|id| is_carpet(id))
    .map(str::to_owned);
    if let Some(c) = body {
        state(mob).carpet = Some(c);
    }
}

impl World {
    /// Whether a foe is still worth spitting at, and if there is none, whether there is
    /// a wild wolf about (or, for a trader llama, a mob after its trader).
    fn check_foe(&mut self, id: i32, mob: &Mob, llama: &mut Llama) {
        let still = match llama.foe {
            Some(Foe::Player(pid)) => self.players.get(&pid).is_some_and(|p| {
                p.alive && p.survival() && distance(p.pos, mob.pos) <= FORGET_RANGE
            }),
            Some(Foe::Mob(mid)) => self
                .mobs
                .get(&mid)
                .is_some_and(|m| m.alive() && distance(m.pos, mob.pos) <= FORGET_RANGE),
            None => false,
        };
        if !still {
            llama.foe = None;
        }
        if llama.foe.is_some()
            || crate::rules::difficulty() == crate::rules::Difficulty::Peaceful
            || !(self.tick + u64::from(id.unsigned_abs())).is_multiple_of(10)
        {
            return;
        }
        let trader = mob.kind == MobKind::TraderLlama;
        llama.foe = self
            .mobs
            .iter()
            .filter(|(_, m)| {
                m.alive()
                    && ((m.kind == MobKind::Wolf && m.owner.is_none())
                        || (trader && trader_enemy(m.kind)))
            })
            .map(|(&mid, m)| (mid, distance(m.pos, mob.pos)))
            .filter(|&(_, d)| d <= NOTICE_RANGE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(mid, _)| Foe::Mob(mid));
        if llama.foe.is_some() {
            llama.cooldown = llama.cooldown.max(SPIT_WINDUP);
        }
    }

    /// Faces its foe, closes in when too far or out of sight, and spits.
    fn fight(&mut self, id: i32, mob: &mut Mob, llama: &mut Llama, foe: Foe) {
        let (feet, height) = match foe {
            Foe::Player(pid) => (self.players[&pid].pos, PLAYER_HEIGHT),
            Foe::Mob(mid) => {
                let m = &self.mobs[&mid];
                (m.pos, m.size().1)
            }
        };
        let at = [feet[0], feet[1] + height * 0.6, feet[2]];
        let (width, own_height) = mob.size();
        let mouth = [mob.pos[0], mob.pos[1] + own_height * 0.8, mob.pos[2]];
        let yaw = yaw_towards(mob.pos, at);
        mob.yaw = yaw;
        mob.head_yaw = yaw;
        // It stands its ground instead of running off.
        mob.panic = 0;
        let ridden = mob.horse.as_ref().is_some_and(|h| h.rider.is_some());
        let dist = distance(mouth, at);
        let seen = self.line_of_sight(mouth, at);
        if !ridden {
            if dist > SPIT_RANGE * 0.6 || !seen {
                mob.target = Some(([feet[0], feet[2]], 20));
            } else {
                mob.target = None;
                mob.path.clear();
            }
        }
        if llama.cooldown > 0 || dist > SPIT_RANGE || !seen {
            return;
        }
        llama.cooldown = SPIT_INTERVAL;
        // From just in front of its mouth, so the spit starts clear of its own head.
        let r = f64::from(yaw).to_radians();
        let ahead = width / 2.0 + 0.1;
        let from = [
            mouth[0] - r.sin() * ahead,
            mouth[1],
            mouth[2] + r.cos() * ahead,
        ];
        let mut vel = aim(from, at);
        for v in &mut vel {
            *v += (self.rng.f64() - 0.5) * 0.05;
        }
        let spit = Spit {
            id: next_entity_id(),
            pos: from,
            vel,
            age: 0,
        };
        self.broadcast(spit_spawn(&spit, id), None);
        let pitch = 0.8 + self.rng.f64() as f32 * 0.4;
        self.play_sound_at("minecraft:entity.llama.spit", mob.pos, 1.0, pitch, None);
        llama.spits.push(spit);
    }

    /// Where a spit's path this tick first runs into something.
    fn spit_hit(&self, shooter: i32, spit: &Spit) -> Option<Hit> {
        for step in 1..=SPIT_SUBSTEPS {
            let f = f64::from(step) / f64::from(SPIT_SUBSTEPS);
            let at = [0, 1, 2].map(|i| spit.pos[i] + spit.vel[i] * f);
            for (&pid, p) in &self.players {
                if p.alive && inside(at, p.pos, PLAYER_WIDTH, PLAYER_HEIGHT) {
                    return Some(Hit::Player(pid));
                }
            }
            for (&mid, m) in &self.mobs {
                let (width, height) = m.size();
                if mid != shooter && m.alive() && inside(at, m.pos, width, height) {
                    return Some(Hit::Mob(mid));
                }
            }
            let cell = BlockPos::new(
                at[0].floor() as i32,
                at[1].floor() as i32,
                at[2].floor() as i32,
            );
            // Any block stops it, even grass, signs or water.
            if self.loaded_block(cell).is_none_or(|s| !is_air(s)) {
                return Some(Hit::Block);
            }
        }
        None
    }

    /// The nearest living mob going after `id`, which is what hurt it if a player did not.
    fn hunter_of(&self, id: i32, at: [f64; 3]) -> Option<i32> {
        self.mobs
            .iter()
            .filter(|(&mid, m)| mid != id && m.alive() && m.hunting == Some(id))
            .map(|(&mid, m)| (mid, distance(m.pos, at)))
            .filter(|&(_, d)| d <= FORGET_RANGE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(mid, _)| mid)
    }

    /// Moves a llama's spits along, and returns those that hit someone (taken off the
    /// clients already), with the way each was flying.
    fn fly_spits(&mut self, shooter: i32, llama: &mut Llama) -> Vec<(Hit, [f64; 2])> {
        let mut hits = Vec::new();
        if llama.spits.is_empty() {
            return hits;
        }
        let mut i = 0;
        while i < llama.spits.len() {
            let spit = &llama.spits[i];
            let hit = self.spit_hit(shooter, spit);
            if hit.is_none() && spit.age < SPIT_LIFE {
                fly(&mut llama.spits[i]);
                i += 1;
                continue;
            }
            let spit = llama.spits.swap_remove(i);
            self.broadcast(items::remove_entity(spit.id), None);
            let flat = spit.vel[0].hypot(spit.vel[2]).max(1e-6);
            let away = [spit.vel[0] / flat, spit.vel[2] / flat];
            if let Some(hit @ (Hit::Player(_) | Hit::Mob(_))) = hit {
                hits.push((hit, away));
            }
        }
        hits
    }

    /// Lands a llama's spits that hit someone. The llama is put back among the mobs
    /// meanwhile (a stand-in is left in its place), so the hits know who spat: the
    /// difficulty scales them, a shield facing it blocks them, and wolves rally.
    fn land_spits(&mut self, shooter: i32, mob: &mut Mob, hits: Vec<(Hit, [f64; 2])>) {
        let name = if mob.kind == MobKind::TraderLlama {
            "Trader Llama"
        } else {
            "Llama"
        };
        let stand_in = Mob::new(mob.kind, mob.pos, &mut self.rng);
        self.mobs.insert(shooter, std::mem::replace(mob, stand_in));
        for (hit, away) in hits {
            match hit {
                Hit::Player(pid) => {
                    self.damage_from(pid, SPIT_DAMAGE, Damage::Mob(name), Some(shooter));
                }
                Hit::Mob(mid) => {
                    let landed = self.hit_mob(mid, SPIT_DAMAGE, None, away, "minecraft:spit");
                    // A llama hit by another's spit spits back.
                    if let Some(m) = self.mobs.get_mut(&mid).filter(|m| landed && m.alive()) {
                        if is_llama(m.kind) {
                            provoke(state(m), Foe::Mob(shooter));
                        }
                    }
                }
                Hit::Block => {}
            }
        }
        if let Some(real) = self.mobs.remove(&shooter) {
            *mob = real;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-llamas-{}-{}",
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
            .insert(alice, Player::new(profile, out, [2.5, -60.0, 2.5]));
        (w, alice)
    }

    fn hold(w: &mut World, id: i32, item: &str) {
        w.players.get_mut(&id).unwrap().inventory.slots[36] =
            Stack::new(ItemInfo::id_of(item).unwrap(), 1);
    }

    fn add(w: &mut World, kind: MobKind, pos: [f64; 3]) -> i32 {
        let id = next_entity_id();
        let mut mob = Mob::new(kind, pos, &mut w.rng);
        mob.age = 0;
        w.mobs.insert(id, mob);
        id
    }

    fn run(w: &mut World, id: i32, ticks: u32) {
        for _ in 0..ticks {
            w.tick += 1;
            let mut m = w.mobs.remove(&id).unwrap();
            tick(w, id, &mut m);
            w.mobs.insert(id, m);
        }
    }

    #[test]
    fn a_tamed_llama_wears_one_carpet_until_shears_take_it() {
        let (mut w, alice) = world_with_player();
        let llama = add(&mut w, MobKind::Llama, [4.5, -60.0, 2.5]);
        hold(&mut w, alice, "minecraft:red_carpet");
        // A wild llama will not take one.
        assert!(!use_on(&mut w, alice, llama, 36, "minecraft:red_carpet"));
        w.mobs.get_mut(&llama).unwrap().owner = Some(11);
        assert!(use_on(&mut w, alice, llama, 36, "minecraft:red_carpet"));
        assert_eq!(carpet(&w.mobs[&llama]), Some("minecraft:red_carpet"));
        assert!(w.players[&alice].inventory.slots[36].is_empty(), "used up");

        // Saved in the body slot, beside what else it wears, and loaded back.
        let nbt = w.mobs[&llama].to_nbt();
        let body = nbt
            .get("equipment")
            .and_then(Tag::as_compound)
            .and_then(|e| e.get("body"))
            .and_then(Tag::as_compound)
            .and_then(|b| b.str("id"))
            .map(str::to_owned);
        assert_eq!(body.as_deref(), Some("minecraft:red_carpet"));
        let back = Mob::read(&nbt, &mut w.rng).unwrap();
        assert_eq!(carpet(&back), Some("minecraft:red_carpet"));
        assert_eq!(spawn_extras(llama, &back).len(), 1);

        // Another carpet does not go over it: the click is left to `horses.rs`.
        let items = w.items.len();
        hold(&mut w, alice, "minecraft:blue_carpet");
        assert!(!use_on(&mut w, alice, llama, 36, "minecraft:blue_carpet"));
        assert_eq!(carpet(&w.mobs[&llama]), Some("minecraft:red_carpet"));

        // Shears take it off, and then the new one goes on.
        hold(&mut w, alice, "minecraft:shears");
        assert!(use_on(&mut w, alice, llama, 36, "minecraft:shears"));
        assert_eq!(carpet(&w.mobs[&llama]), None);
        assert_eq!(w.items.len(), items + 1);
        // With nothing on, shears are left to `horses.rs`.
        assert!(!use_on(&mut w, alice, llama, 36, "minecraft:shears"));
        hold(&mut w, alice, "minecraft:blue_carpet");
        assert!(use_on(&mut w, alice, llama, 36, "minecraft:blue_carpet"));
        assert_eq!(carpet(&w.mobs[&llama]), Some("minecraft:blue_carpet"));

        // Taken off, it stays off after saving, even though it was loaded with one.
        let mut loaded = Mob::read(&nbt, &mut w.rng).unwrap();
        state(&mut loaded).carpet = None;
        let again = Mob::read(&loaded.to_nbt(), &mut w.rng).unwrap();
        assert_eq!(carpet(&again), None);
    }

    #[test]
    fn an_old_style_carpet_loads_but_does_not_come_back_once_removed() {
        let (mut w, _) = world_with_player();
        let llama = add(&mut w, MobKind::Llama, [4.5, -60.0, 2.5]);
        let mut nbt = w.mobs[&llama].to_nbt();
        nbt.remove("equipment");
        nbt.insert(
            "DecorItem",
            Compound::new()
                .with("id", "minecraft:lime_carpet")
                .with("count", 1i32),
        );
        let mut old = Mob::read(&nbt, &mut w.rng).unwrap();
        assert_eq!(carpet(&old), Some("minecraft:lime_carpet"));
        state(&mut old).carpet = None;
        let again = Mob::read(&old.to_nbt(), &mut w.rng).unwrap();
        assert_eq!(carpet(&again), None, "sheared off for good");
    }

    #[test]
    fn a_ridden_llama_takes_no_carpet() {
        let (mut w, alice) = world_with_player();
        let llama = add(&mut w, MobKind::Llama, [4.5, -60.0, 2.5]);
        let m = w.mobs.get_mut(&llama).unwrap();
        m.owner = Some(11);
        let Some(horse) = m.horse.as_mut() else {
            return;
        };
        horse.rider = Some(alice);
        hold(&mut w, alice, "minecraft:red_carpet");
        assert!(!use_on(&mut w, alice, llama, 36, "minecraft:red_carpet"));
        assert_eq!(carpet(&w.mobs[&llama]), None);
    }

    #[test]
    fn a_llama_spits_at_a_mob_that_hurts_it() {
        let (mut w, alice) = world_with_player();
        w.players.remove(&alice);
        let llama = add(&mut w, MobKind::Llama, [2.5, -60.0, 2.5]);
        let zombie = add(&mut w, MobKind::Zombie, [6.5, -60.0, 2.5]);
        w.mobs.get_mut(&zombie).unwrap().hunting = Some(llama);
        let mut m = w.mobs.remove(&llama).unwrap();
        hurt(&mut w, llama, &mut m, None);
        w.mobs.insert(llama, m);
        assert_eq!(peek(&w.mobs[&llama]).unwrap().foe, Some(Foe::Mob(zombie)));
    }

    #[test]
    fn a_dying_llama_takes_its_spits_away() {
        let (mut w, _) = world_with_player();
        let llama = add(&mut w, MobKind::Llama, [2.5, -60.0, 2.5]);
        let mut m = w.mobs.remove(&llama).unwrap();
        state(&mut m).spits.push(Spit {
            id: next_entity_id(),
            pos: [2.5, -58.0, 2.5],
            vel: [0.5, 0.0, 0.0],
            age: 0,
        });
        m.health = 0.0;
        hurt(&mut w, llama, &mut m, None);
        assert!(peek(&m).unwrap().spits.is_empty());
    }

    #[test]
    fn a_hurt_llama_spits_back_and_the_spit_hurts() {
        let (mut w, alice) = world_with_player();
        let llama = add(&mut w, MobKind::Llama, [8.5, -60.0, 2.5]);
        let mut m = w.mobs.remove(&llama).unwrap();
        hurt(&mut w, llama, &mut m, Some(alice));
        w.mobs.insert(llama, m);
        assert_eq!(peek(&w.mobs[&llama]).unwrap().foe, Some(Foe::Player(alice)));
        let before = w.players[&alice].health;
        run(&mut w, llama, SPIT_WINDUP + 1);
        assert_eq!(peek(&w.mobs[&llama]).unwrap().spits.len(), 1, "spat");
        run(&mut w, llama, 10);
        assert!(peek(&w.mobs[&llama]).unwrap().spits.is_empty(), "landed");
        assert!(w.players[&alice].health < before, "hit for damage");
        // The hit knew who spat, and the llama came back whole.
        assert_eq!(
            w.players[&alice].hurt_by.map(|(kind, _)| kind),
            Some(entity_type::LLAMA)
        );
        assert_eq!(peek(&w.mobs[&llama]).unwrap().foe, Some(Foe::Player(alice)));
    }

    #[test]
    fn llamas_spit_at_wild_wolves_but_not_tamed_ones() {
        let (mut w, alice) = world_with_player();
        w.players.remove(&alice);
        let llama = add(&mut w, MobKind::Llama, [2.5, -60.0, 2.5]);
        let wolf = add(&mut w, MobKind::Wolf, [8.5, -60.0, 2.5]);
        w.mobs.get_mut(&wolf).unwrap().owner = Some(5);
        run(&mut w, llama, 30);
        assert!(peek(&w.mobs[&llama]).is_none_or(|l| l.foe.is_none()));
        w.mobs.get_mut(&wolf).unwrap().owner = None;
        let health = w.mobs[&wolf].health;
        run(&mut w, llama, 40);
        assert_eq!(peek(&w.mobs[&llama]).unwrap().foe, Some(Foe::Mob(wolf)));
        assert!(w.mobs[&wolf].health < health, "spat on");
    }

    #[test]
    fn trader_llamas_fight_the_mobs_after_their_trader() {
        let (mut w, alice) = world_with_player();
        w.players.remove(&alice);
        let llama = add(&mut w, MobKind::TraderLlama, [2.5, -60.0, 2.5]);
        let zombie = add(&mut w, MobKind::Zombie, [9.5, -60.0, 2.5]);
        run(&mut w, llama, 20);
        assert_eq!(peek(&w.mobs[&llama]).unwrap().foe, Some(Foe::Mob(zombie)));
        // A plain llama leaves zombies alone.
        let plain = add(&mut w, MobKind::Llama, [2.5, -60.0, 6.5]);
        run(&mut w, plain, 20);
        assert!(peek(&w.mobs[&plain]).is_none_or(|l| l.foe.is_none()));
    }

    #[test]
    fn a_llama_hit_by_another_llamas_spit_spits_back() {
        let (mut w, alice) = world_with_player();
        w.players.remove(&alice);
        let a = add(&mut w, MobKind::Llama, [2.5, -60.0, 2.5]);
        let b = add(&mut w, MobKind::Llama, [8.5, -60.0, 2.5]);
        let mut m = w.mobs.remove(&a).unwrap();
        state(&mut m).foe = Some(Foe::Mob(b));
        w.mobs.insert(a, m);
        run(&mut w, a, 30);
        assert_eq!(peek(&w.mobs[&b]).unwrap().foe, Some(Foe::Mob(a)));
    }

    #[test]
    fn spit_falls_and_stops_at_blocks() {
        let (w, _) = world_with_player();
        let mut spit = Spit {
            id: 1,
            pos: [0.5, -58.0, 0.5],
            vel: [SPIT_SPEED, 0.0, 0.0],
            age: 0,
        };
        fly(&mut spit);
        assert!((spit.vel[0] - SPIT_SPEED * SPIT_DRAG).abs() < 1e-9);
        assert!((spit.vel[1] + SPIT_GRAVITY).abs() < 1e-9);
        // Straight down into the flat world's floor.
        let falling = Spit {
            id: 2,
            pos: [0.5, -59.5, 0.5],
            vel: [0.0, -1.0, 0.0],
            age: 0,
        };
        assert!(matches!(w.spit_hit(0, &falling), Some(Hit::Block)));
        let v = aim([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]);
        assert!(v[1] > 0.0, "aimed high");
        assert!((distance(v, [0.0; 3]) - SPIT_SPEED).abs() < 1e-9);
    }
}
