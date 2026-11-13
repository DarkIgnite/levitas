//! Armadillos: rolling up into their shell, shedding and brushing scutes, and keeping
//! spiders away.
//!
//! Behaviour follows the Minecraft Wiki (Armadillo, Armadillo Scute, Brush):
//! - An armadillo rolls up when a threat is near: an undead mob, a player who is
//!   sprinting or riding something, or a player who hurt it lately. It looks for them
//!   in its hitbox grown by 7 blocks sideways and 2 up and down. Being hit by a player
//!   rolls it up at once. It does not roll up while fleeing, in water, on fire, in the
//!   air or sitting in a vehicle.
//! - Rolled up it does not walk, eat or follow food, and an armadillo in love that
//!   rolls up forgets about it. Damage it takes becomes `(damage - 1) / 2`, except from
//!   `/kill`. It stays rolled while the threat is there and unrolls once it has seen
//!   none for 3 seconds; water, fire, and magma, freezing or poison damage unroll it
//!   straight away.
//! - Its posture is shown with the armadillo state entity data: idle, rolling (the
//!   roll-up animation), scared (in its shell) and unrolling.
//! - An adult sheds an armadillo scute every 5 to 10 minutes. A brush used on one
//!   brushes off a scute and costs the brush 16 durability.
//! - Spiders and cave spiders flee from armadillos within 6 blocks that are not
//!   rolled up.
//! - Saved as vanilla: `scute_time` and `state`.
//!
//! Looking for threats walks over every player and mob, so an armadillo only looks
//! every few ticks (and not at all while it could not roll up anyway).

use levitas_nbt::Compound;

use super::mobs::{Mob, MobKind};
use super::*;

/// Entity data: the armadillo state (an enum serializer).
const DATA_STATE: u8 = 18;
const SERIALIZER_ARMADILLO_STATE: i32 = 36;
/// Ticks between scutes: 5 to 10 minutes.
const SCUTE_TIME: (u32, u32) = (6000, 12000);
/// Durability a brush loses for each scute brushed off.
const BRUSH_WEAR: i32 = 16;
/// Ticks without a threat before a scared armadillo unrolls.
const CALM_TICKS: u32 = 60;
/// How often an idle or scared armadillo looks for threats, in ticks.
const LOOK_EVERY: u64 = 5;
/// How long the roll-up and unroll animations are given, in ticks (estimates: the
/// wiki does not give them).
const ROLL_TICKS: u32 = 10;
const UNROLL_TICKS: u32 = 30;
/// How far beyond its hitbox an armadillo looks for threats.
const LOOK_SIDEWAYS: f64 = 7.0;
const LOOK_UP_DOWN: f64 = 2.0;
/// How long a player who hurt it stays a threat, in ticks.
const ATTACKER_MEMORY: u32 = 100;
/// Spiders keep this far from armadillos that are not rolled up.
const SPIDER_AVOID: f64 = 6.0;
/// How far a spider runs off, and how often spiders are checked.
const SPIDER_FLEE: f64 = 8.0;
const SPIDER_CHECK_EVERY: u64 = 10;
const PLAYER_SIZE: (f64, f64) = (0.6, 1.8);
const GAME_MODE_SPECTATOR: u8 = 3;
const ENTITY_EVENT_MAINHAND_BREAK: i8 = 47;
const ENTITY_EVENT_OFFHAND_BREAK: i8 = 48;

/// The armadillo state, in the order of its entity data enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Posture {
    Idle = 0,
    Rolling = 1,
    Scared = 2,
    Unrolling = 3,
}

impl Posture {
    /// The `state` tag it is saved as. A roll-up still under way is saved as scared,
    /// so that the armadillo comes back in its shell.
    fn saved(self) -> &'static str {
        match self {
            Posture::Idle => "idle",
            Posture::Rolling | Posture::Scared => "scared",
            Posture::Unrolling => "unrolling",
        }
    }

    /// Any other string is idle, as in vanilla.
    fn read(name: &str) -> Posture {
        match name {
            "rolling" | "scared" => Posture::Scared,
            "unrolling" => Posture::Unrolling,
            _ => Posture::Idle,
        }
    }
}

/// What the module keeps for an armadillo, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq)]
struct Armadillo {
    posture: Posture,
    /// Ticks in the current posture (for the animations).
    ticks: u32,
    /// Ticks without a threat while scared (counted in steps of `LOOK_EVERY`).
    calm: u32,
    /// Ticks until the next scute, rolled on its first turn as an adult.
    scute_time: Option<u32>,
    /// The player who last hurt it, and for how many more ticks that counts.
    attacker: Option<(i32, u32)>,
}

impl Armadillo {
    fn new() -> Armadillo {
        Armadillo {
            posture: Posture::Idle,
            ticks: 0,
            calm: 0,
            scute_time: None,
            attacker: None,
        }
    }

    fn rolled_up(&self) -> bool {
        self.posture != Posture::Idle
    }
}

fn armadillo_of(mob: &Mob) -> Option<&Armadillo> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Armadillo>())
}

/// Whether an armadillo is in its shell (rolling up, scared or unrolling).
pub(super) fn rolled_up(mob: &Mob) -> bool {
    armadillo_of(mob).is_some_and(Armadillo::rolled_up)
}

fn roll_scute_time(rng: &mut Rng) -> u32 {
    SCUTE_TIME.0 + rng.below(SCUTE_TIME.1 - SCUTE_TIME.0 + 1)
}

/// What a rolled-up armadillo takes of `damage`.
fn reduced(damage: f32) -> f32 {
    ((damage - 1.0) / 2.0).max(0.0)
}

fn state_data(id: i32, posture: Posture) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_STATE)
        .varint(SERIALIZER_ARMADILLO_STATE)
        .varint(posture as i32)
        .u8(0xFF)
}

/// Whether this is one of the ticks on which armadillo `id` looks for threats. The
/// id spreads armadillos over the ticks.
fn looks_now(tick: u64, id: i32) -> bool {
    tick.wrapping_add(u64::from(id.unsigned_abs()))
        .is_multiple_of(LOOK_EVERY)
}

/// Whether the boxes `a` (feet, width, height) and `b` overlap.
fn boxes_meet(a: ([f64; 3], f64, f64), b: ([f64; 3], f64, f64)) -> bool {
    let reach = (a.1 + b.1) / 2.0;
    (a.0[0] - b.0[0]).abs() < reach
        && (a.0[2] - b.0[2]).abs() < reach
        && a.0[1] < b.0[1] + b.2
        && b.0[1] < a.0[1] + a.2
}

/// Where a spider or cave spider at `at` should run to, away from the closest
/// armadillo within 6 blocks that is not rolled up, if there is one.
pub(super) fn spider_flees_to(w: &World, at: [f64; 3]) -> Option<[f64; 2]> {
    let (closest, distance) = w
        .mobs
        .values()
        .filter(|m| {
            m.kind == MobKind::Armadillo
                && m.alive()
                && !rolled_up(m)
                && (m.pos[1] - at[1]).abs() <= SPIDER_AVOID
        })
        .map(|m| (m.pos, (m.pos[0] - at[0]).hypot(m.pos[2] - at[2])))
        .filter(|&(_, d)| d <= SPIDER_AVOID)
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    let (dx, dz) = if distance < 1e-6 {
        (1.0, 0.0)
    } else {
        (
            (at[0] - closest[0]) / distance,
            (at[2] - closest[2]) / distance,
        )
    };
    Some([at[0] + dx * SPIDER_FLEE, at[2] + dz * SPIDER_FLEE])
}

pub(super) fn spawned(w: &mut World, mob: &mut Mob, _biome: &str) {
    let mut a = Armadillo::new();
    if mob.age >= 0 {
        a.scute_time = Some(roll_scute_time(&mut w.rng));
    }
    mob.kin = Some(Box::new(a));
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if armadillo_of(mob).is_none() {
        // From a spawn egg, or bred: set up on its first turn.
        mob.kin = Some(Box::new(Armadillo::new()));
    }
    let Some(mut kin) = mob.kin.take() else {
        return false;
    };
    if let Some(a) = kin.downcast_mut::<Armadillo>() {
        w.tick_armadillo(id, mob, a);
    }
    mob.kin = Some(kin);
    // It never rests in place: a rolled-up armadillo has no target, so it stands
    // still, but it still falls and can be knocked about.
    false
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    let Some(mob) = w.mobs.get(&target) else {
        return false;
    };
    if !mob.alive() {
        // Nothing to brush off one that is dying.
        return false;
    }
    if held == "minecraft:brush" {
        let (pos, baby) = (mob.pos, mob.age < 0);
        w.drop_scute(pos);
        // Each of its sounds plays at full, 80% or 90% of the usual pitch.
        let base = if baby { 1.3 } else { 0.8 };
        let pitch = (base + w.rng.f64() as f32 * 0.4) * [1.0, 0.8, 0.9][w.rng.below(3) as usize];
        w.play_sound_at("minecraft:entity.armadillo.brush", pos, 0.9, pitch, None);
        if w.players.get(&player).is_some_and(|p| p.survival()) {
            let event = if slot == OFFHAND {
                ENTITY_EVENT_OFFHAND_BREAK
            } else {
                ENTITY_EVENT_MAINHAND_BREAK
            };
            w.damage_item(player, slot, BRUSH_WEAR, event);
        }
        return true;
    }
    // In its shell it does not eat: food used on it does nothing.
    rolled_up(mob) && super::breeding::is_food(mob.kind, held)
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, attacker: Option<i32>) {
    let Some(attacker) = attacker else {
        // Fire, falls and the like: it runs about as other animals do.
        return;
    };
    let Some(mut kin) = mob.kin.take() else {
        return;
    };
    if let Some(a) = kin.downcast_mut::<Armadillo>() {
        a.attacker = Some((attacker, ATTACKER_MEMORY));
        match a.posture {
            Posture::Idle if may_roll(w, mob) => w.roll_up(id, mob, a),
            Posture::Scared => a.calm = 0,
            Posture::Unrolling => w.set_posture(id, mob, a, Posture::Scared),
            _ => {}
        }
    }
    mob.kin = Some(kin);
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    match armadillo_of(mob) {
        Some(a) if a.rolled_up() => vec![state_data(id, a.posture)],
        _ => Vec::new(),
    }
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let default = Armadillo::new();
    let a = armadillo_of(mob).unwrap_or(&default);
    // Babies (and adults yet to roll it) get a fresh wait when they need one.
    let scute_time = a.scute_time.unwrap_or(SCUTE_TIME.0);
    nbt.insert("scute_time", scute_time.min(i32::MAX as u32) as i32);
    nbt.insert("state", a.posture.saved());
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let mut a = Armadillo::new();
    a.posture = nbt.str("state").map_or(Posture::Idle, Posture::read);
    a.scute_time = nbt
        .int("scute_time")
        .map(|t| t.clamp(0, i64::from(u32::MAX)) as u32);
    mob.kin = Some(Box::new(a));
}

pub(super) fn damage_taken(
    w: &mut World,
    id: i32,
    mob: &mut Mob,
    damage: f32,
    damage_type: &str,
    _attacker: Option<i32>,
) -> f32 {
    if damage_type == "minecraft:generic_kill" {
        return damage;
    }
    let Some(mut kin) = mob.kin.take() else {
        return damage;
    };
    let mut taken = damage;
    if let Some(a) = kin.downcast_mut::<Armadillo>().filter(|a| a.rolled_up()) {
        taken = reduced(damage);
        // The hurt sound is the shared code's (see `needs_elsewhere`: it should be
        // `entity.armadillo.hurt_reduced` while rolled up).
        if matches!(
            damage_type,
            "minecraft:hot_floor" | "minecraft:freeze" | "minecraft:magic"
        ) {
            w.unroll(id, mob, a);
        }
    }
    mob.kin = Some(kin);
    taken
}

pub(super) fn may_love(_w: &World, mob: &Mob) -> bool {
    !rolled_up(mob)
}

pub(super) fn bred(_w: &mut World, _id: i32, _mob: &mut Mob) -> bool {
    false
}

pub(super) fn tick_world(w: &mut World) {
    if !w.tick.is_multiple_of(SPIDER_CHECK_EVERY) {
        return;
    }
    if !w.mobs.values().any(|m| m.kind == MobKind::Armadillo) {
        return;
    }
    // A hunting spider has its target set by the shared hunting every tick, so only
    // spiders that are not hunting are sent off here.
    let spiders: Vec<(i32, [f64; 3])> = w
        .mobs
        .iter()
        .filter(|(_, m)| {
            matches!(m.kind, MobKind::Spider | MobKind::CaveSpider)
                && m.alive()
                && m.hunting.is_none()
        })
        .map(|(&id, m)| (id, m.pos))
        .collect();
    for (id, at) in spiders {
        if let Some(to) = spider_flees_to(w, at) {
            if let Some(spider) = w.mobs.get_mut(&id) {
                spider.target = Some((to, SPIDER_CHECK_EVERY as u32 * 4));
                spider.path.clear();
            }
        }
    }
}

pub(super) fn block_placed(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

pub(super) fn block_broken(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

pub(super) fn block_used(
    _w: &mut World,
    _player: i32,
    _slot: usize,
    _pos: BlockPos,
    _state: u16,
) -> bool {
    false
}

pub(super) fn block_tick(_w: &mut World, _pos: BlockPos, _state: u16) -> bool {
    false
}

pub(super) fn ticks_randomly(_state: u16) -> bool {
    false
}

pub(super) fn scheduled_tick(_w: &mut World, _pos: BlockPos, _state: u16) -> bool {
    false
}

/// Whether an armadillo can roll up now: not fleeing, in water, on fire, in the air
/// or in a vehicle.
fn may_roll(w: &World, mob: &Mob) -> bool {
    mob.on_ground_now()
        && mob.panic == 0
        && mob.vehicle.is_none()
        && mob.fire_ticks == 0
        && !w.in_water(mob.pos)
}

impl World {
    /// An armadillo's turn: scutes, and rolling up and unrolling.
    fn tick_armadillo(&mut self, id: i32, mob: &mut Mob, a: &mut Armadillo) {
        if mob.age >= 0 {
            let time = a
                .scute_time
                .get_or_insert_with(|| roll_scute_time(&mut self.rng));
            *time = time.saturating_sub(1);
            if *time == 0 {
                *time = roll_scute_time(&mut self.rng);
                self.drop_scute(mob.pos);
                let pitch =
                    [1.0, 0.9][self.rng.below(2) as usize] * (0.8 + self.rng.f64() as f32 * 0.4);
                self.play_sound_at(
                    "minecraft:entity.armadillo.scute_drop",
                    mob.pos,
                    1.0,
                    pitch,
                    None,
                );
            }
        }
        if let Some((_, ticks)) = a.attacker.as_mut() {
            *ticks = ticks.saturating_sub(1);
            if *ticks == 0 {
                a.attacker = None;
            }
        }
        let attacker = a.attacker.map(|(p, _)| p);
        let looks = looks_now(self.tick, id);
        match a.posture {
            Posture::Idle => {
                if looks && may_roll(self, mob) && self.armadillo_threatened(mob, attacker) {
                    self.roll_up(id, mob, a);
                }
            }
            Posture::Rolling => {
                a.ticks += 1;
                if self.armadillo_hazard(mob) {
                    self.unroll(id, mob, a);
                } else if a.ticks >= ROLL_TICKS {
                    self.set_posture(id, mob, a, Posture::Scared);
                    let pitch = self.armadillo_pitch(mob);
                    self.play_sound_at(
                        "minecraft:entity.armadillo.land",
                        mob.pos,
                        1.0,
                        pitch,
                        None,
                    );
                }
            }
            Posture::Scared => {
                if self.armadillo_hazard(mob) {
                    self.unroll(id, mob, a);
                } else if looks {
                    if self.armadillo_threatened(mob, attacker) {
                        a.calm = 0;
                    } else {
                        // One look stands for the ticks since the last one.
                        a.calm += LOOK_EVERY as u32;
                        if a.calm >= CALM_TICKS {
                            self.unroll(id, mob, a);
                        }
                    }
                }
            }
            Posture::Unrolling => {
                a.ticks += 1;
                if !self.armadillo_hazard(mob)
                    && may_roll(self, mob)
                    && self.armadillo_threatened(mob, attacker)
                {
                    self.set_posture(id, mob, a, Posture::Scared);
                } else if a.ticks >= UNROLL_TICKS {
                    self.set_posture(id, mob, a, Posture::Idle);
                    let pitch = self.armadillo_pitch(mob);
                    self.play_sound_at(
                        "minecraft:entity.armadillo.unroll_finish",
                        mob.pos,
                        1.0,
                        pitch,
                        None,
                    );
                }
            }
        }
        if a.rolled_up() {
            // In its shell: no walking, fleeing, following food or looking about.
            mob.target = None;
            mob.path.clear();
            mob.panic = 0;
            mob.head_yaw = mob.yaw;
        }
    }

    /// Water and fire make a rolled-up armadillo unroll.
    fn armadillo_hazard(&self, mob: &Mob) -> bool {
        mob.fire_ticks > 0 || self.in_water(mob.pos)
    }

    /// Whether a threat is within an armadillo's sight box: an undead mob, a player
    /// sprinting or riding, or `attacker`.
    fn armadillo_threatened(&self, mob: &Mob, attacker: Option<i32>) -> bool {
        let (width, height) = mob.size();
        let sight = (
            [mob.pos[0], mob.pos[1] - LOOK_UP_DOWN, mob.pos[2]],
            width + 2.0 * LOOK_SIDEWAYS,
            height + 2.0 * LOOK_UP_DOWN,
        );
        let player_threat = self.players.iter().any(|(&pid, p)| {
            p.alive
                && p.mode != GAME_MODE_SPECTATOR
                && (p.sprinting || p.riding.is_some() || attacker == Some(pid))
                && boxes_meet(sight, (p.pos, PLAYER_SIZE.0, PLAYER_SIZE.1))
        });
        player_threat
            || self.mobs.values().any(|m| {
                m.kind.is_undead() && m.alive() && {
                    let (w, h) = m.size();
                    boxes_meet(sight, (m.pos, w, h))
                }
            })
    }

    /// Starts rolling up: the roll sound, and an armadillo in love forgets it.
    fn roll_up(&mut self, id: i32, mob: &mut Mob, a: &mut Armadillo) {
        self.set_posture(id, mob, a, Posture::Rolling);
        mob.love = 0;
        mob.target = None;
        mob.path.clear();
        mob.panic = 0;
        let pitch = self.armadillo_pitch(mob);
        self.play_sound_at("minecraft:entity.armadillo.roll", mob.pos, 1.0, pitch, None);
    }

    /// Starts unrolling.
    fn unroll(&mut self, id: i32, mob: &Mob, a: &mut Armadillo) {
        if matches!(a.posture, Posture::Idle | Posture::Unrolling) {
            return;
        }
        self.set_posture(id, mob, a, Posture::Unrolling);
        self.play_sound_at(
            "minecraft:entity.armadillo.unroll_start",
            mob.pos,
            1.0,
            1.0,
            None,
        );
    }

    fn set_posture(&mut self, id: i32, mob: &Mob, a: &mut Armadillo, posture: Posture) {
        a.posture = posture;
        a.ticks = 0;
        a.calm = 0;
        self.send_to_viewers(mob, state_data(id, posture));
    }

    /// Drops a scute at an armadillo's feet.
    fn drop_scute(&mut self, pos: [f64; 3]) {
        if let Some(item) = ItemInfo::id_of("minecraft:armadillo_scute") {
            // `scatter_item` lifts what it drops to a player's chest height; an
            // armadillo is far lower.
            self.scatter_item([pos[0], pos[1] - 1.0, pos[2]], Stack::new(item, 1));
        }
    }

    /// The pitch of an armadillo's sounds: 0.8 to 1.2, or 1.3 to 1.7 for a baby.
    fn armadillo_pitch(&mut self, mob: &Mob) -> f32 {
        let base = if mob.age < 0 { 1.3 } else { 0.8 };
        base + self.rng.f64() as f32 * 0.4
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-armadillos-{}-{}",
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
        for cx in -2..=2 {
            for cz in -2..=2 {
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
        // Far away, so that only what a test puts close counts.
        w.players
            .insert(alice, Player::new(profile, out, [-28.5, -60.0, -28.5]));
        (w, alice)
    }

    fn armadillo(w: &mut World, at: [f64; 3]) -> (i32, Mob) {
        let mut mob = Mob::new(MobKind::Armadillo, at, &mut w.rng);
        mob.set_on_ground(true);
        spawned(w, &mut mob, "minecraft:savanna");
        (next_entity_id(), mob)
    }

    /// One game tick for the armadillo.
    fn step(w: &mut World, id: i32, mob: &mut Mob) {
        w.tick += 1;
        tick(w, id, mob);
    }

    /// Enough ticks for it to have looked about once.
    fn look(w: &mut World, id: i32, mob: &mut Mob) {
        for _ in 0..LOOK_EVERY {
            step(w, id, mob);
        }
    }

    fn state(mob: &Mob) -> &Armadillo {
        armadillo_of(mob).expect("armadillo state")
    }

    fn set(mob: &mut Mob, f: impl FnOnce(&mut Armadillo)) {
        f(mob
            .kin
            .as_mut()
            .and_then(|k| k.downcast_mut::<Armadillo>())
            .expect("armadillo state"));
    }

    fn hold(w: &mut World, id: i32, item: &str) {
        w.players.get_mut(&id).unwrap().inventory.slots[36] =
            Stack::new(ItemInfo::id_of(item).unwrap(), 1);
    }

    #[test]
    fn sprinting_players_roll_it_up_until_they_are_gone() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = armadillo(&mut w, [0.5, -60.0, 0.5]);
        look(&mut w, id, &mut mob);
        assert_eq!(state(&mob).posture, Posture::Idle);
        // A player walking close by is no threat; sprinting is.
        w.players.get_mut(&alice).unwrap().pos = [4.5, -60.0, 0.5];
        look(&mut w, id, &mut mob);
        assert_eq!(state(&mob).posture, Posture::Idle);
        w.players.get_mut(&alice).unwrap().sprinting = true;
        mob.target = Some(([5.0, 5.0], 100));
        for _ in 0..LOOK_EVERY {
            if state(&mob).posture == Posture::Idle {
                step(&mut w, id, &mut mob);
            }
        }
        assert_eq!(state(&mob).posture, Posture::Rolling);
        assert!(mob.target.is_none(), "it stops walking");
        for _ in 0..ROLL_TICKS {
            step(&mut w, id, &mut mob);
        }
        assert_eq!(state(&mob).posture, Posture::Scared);
        assert_eq!(spawn_extras(id, &mob).len(), 1);
        // Stays rolled while the threat stays.
        for _ in 0..200 {
            step(&mut w, id, &mut mob);
        }
        assert_eq!(state(&mob).posture, Posture::Scared);
        // Gone: about three seconds later it unrolls, then stands again.
        w.players.get_mut(&alice).unwrap().pos = [-28.5, -60.0, -28.5];
        let mut calm = 0;
        while state(&mob).posture == Posture::Scared && calm < 200 {
            step(&mut w, id, &mut mob);
            calm += 1;
        }
        assert_eq!(state(&mob).posture, Posture::Unrolling);
        let slack = LOOK_EVERY as u32;
        assert!((CALM_TICKS - slack..=CALM_TICKS + slack).contains(&calm));
        for _ in 0..UNROLL_TICKS {
            step(&mut w, id, &mut mob);
        }
        assert_eq!(state(&mob).posture, Posture::Idle);
        assert!(spawn_extras(id, &mob).is_empty());
    }

    #[test]
    fn undead_and_riders_are_threats_but_not_far_ones() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = armadillo(&mut w, [0.5, -60.0, 0.5]);
        let zombie = next_entity_id();
        w.mobs.insert(
            zombie,
            Mob::new(MobKind::Zombie, [12.5, -60.0, 0.5], &mut w.rng),
        );
        look(&mut w, id, &mut mob);
        assert_eq!(state(&mob).posture, Posture::Idle, "too far");
        w.mobs.get_mut(&zombie).unwrap().pos = [7.0, -60.0, 0.5];
        look(&mut w, id, &mut mob);
        assert_eq!(state(&mob).posture, Posture::Rolling);
        w.mobs.remove(&zombie);
        let (id, mut mob) = armadillo(&mut w, [0.5, -60.0, 0.5]);
        let p = w.players.get_mut(&alice).unwrap();
        p.pos = [0.5, -60.0, 6.5];
        p.riding = Some(12345);
        look(&mut w, id, &mut mob);
        assert_eq!(state(&mob).posture, Posture::Rolling);
        // Not in water, though, nor in the air.
        let (id, mut wet) = armadillo(&mut w, [0.5, -60.0, 2.5]);
        w.set_block(BlockPos::new(0, -60, 2), block::WATER);
        look(&mut w, id, &mut wet);
        assert_eq!(state(&wet).posture, Posture::Idle);
        let (id, mut falling) = armadillo(&mut w, [0.5, -57.0, 0.5]);
        falling.set_on_ground(false);
        look(&mut w, id, &mut falling);
        assert_eq!(state(&falling).posture, Posture::Idle);
    }

    #[test]
    fn a_hit_rolls_it_up_and_the_shell_softens_blows() {
        let (mut w, alice) = world_with_player();
        let (id, mut mob) = armadillo(&mut w, [0.5, -60.0, 0.5]);
        mob.love = 200;
        step(&mut w, id, &mut mob);
        w.mobs.insert(id, mob);
        let full = w.mobs[&id].health;
        assert!(w.hit_mob(id, 3.0, Some(alice), [1.0, 0.0], "minecraft:player_attack"));
        let mob = w.mobs.get_mut(&id).unwrap();
        assert_eq!(mob.health, full - 3.0, "the first hit lands in full");
        assert_eq!(state(mob).posture, Posture::Rolling);
        assert_eq!(mob.love, 0, "it forgets about love");
        assert!(!may_love(&w, &w.mobs[&id]));
        let mut mob = w.mobs.remove(&id).unwrap();
        step(&mut w, id, &mut mob);
        assert_eq!(mob.panic, 0, "it does not run off");
        let taken = damage_taken(&mut w, id, &mut mob, 9.0, "minecraft:player_attack", None);
        assert_eq!(taken, 4.0);
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 1.0, "minecraft:mob_attack", None),
            0.0
        );
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 50.0, "minecraft:generic_kill", None),
            50.0
        );
        // The attacker stays a threat for a while even walking.
        w.players.get_mut(&alice).unwrap().pos = [3.5, -60.0, 0.5];
        for _ in 0..80 {
            step(&mut w, id, &mut mob);
        }
        assert_eq!(state(&mob).posture, Posture::Scared);
        // Freezing unrolls it at once.
        damage_taken(&mut w, id, &mut mob, 1.0, "minecraft:freeze", None);
        assert_eq!(state(&mob).posture, Posture::Unrolling);
        // Unrolled, damage is taken in full.
        set(&mut mob, |a| a.posture = Posture::Idle);
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 9.0, "minecraft:player_attack", None),
            9.0
        );
    }

    #[test]
    fn adults_shed_scutes_every_five_to_ten_minutes() {
        let (mut w, _) = world_with_player();
        let mut rng = Rng::new(3);
        for _ in 0..200 {
            assert!((6000..=12000).contains(&roll_scute_time(&mut rng)));
        }
        let (id, mut mob) = armadillo(&mut w, [0.5, -60.0, 0.5]);
        assert!(state(&mob).scute_time.is_some());
        set(&mut mob, |a| a.scute_time = Some(2));
        step(&mut w, id, &mut mob);
        assert!(w.items.is_empty());
        step(&mut w, id, &mut mob);
        let scutes = w
            .items
            .values()
            .filter(|i| ItemInfo::get(i.stack().item).unwrap().name == "minecraft:armadillo_scute")
            .count();
        assert_eq!(scutes, 1);
        assert!(state(&mob).scute_time.unwrap() >= SCUTE_TIME.0);
        // Babies do not shed.
        let (baby, mut young) = armadillo(&mut w, [2.5, -60.0, 0.5]);
        young.age = -24_000;
        set(&mut young, |a| a.scute_time = None);
        for _ in 0..10 {
            step(&mut w, baby, &mut young);
        }
        assert!(state(&young).scute_time.is_none());
    }

    #[test]
    fn brushing_gives_a_scute_and_wears_the_brush() {
        let (mut w, alice) = world_with_player();
        let (id, mob) = armadillo(&mut w, [-27.5, -60.0, -28.5]);
        w.mobs.insert(id, mob);
        hold(&mut w, alice, "minecraft:brush");
        assert!(use_on(&mut w, alice, id, 36, "minecraft:brush"));
        assert_eq!(w.items.len(), 1);
        assert_eq!(w.players[&alice].inventory.slots[36].damage, BRUSH_WEAR);
        // A fresh brush lasts four times.
        for _ in 0..3 {
            assert!(use_on(&mut w, alice, id, 36, "minecraft:brush"));
        }
        assert!(w.players[&alice].inventory.slots[36].is_empty(), "worn out");
        assert_eq!(w.items.len(), 4);
        // Other items are left to the shared code, food too while it is unrolled.
        assert!(!use_on(&mut w, alice, id, 36, "minecraft:stick"));
        assert!(!use_on(&mut w, alice, id, 36, "minecraft:spider_eye"));
        set(w.mobs.get_mut(&id).unwrap(), |a| {
            a.posture = Posture::Scared
        });
        assert!(use_on(&mut w, alice, id, 36, "minecraft:spider_eye"));
    }

    #[test]
    fn a_dying_armadillo_cannot_be_brushed() {
        let (mut w, alice) = world_with_player();
        let (id, mob) = armadillo(&mut w, [-27.5, -60.0, -28.5]);
        w.mobs.insert(id, mob);
        hold(&mut w, alice, "minecraft:brush");
        assert!(w.hit_mob(
            id,
            100.0,
            Some(alice),
            [1.0, 0.0],
            "minecraft:player_attack"
        ));
        assert!(w.mobs.get(&id).is_none_or(|m| !m.alive()));
        let before = w.items.len();
        assert!(!use_on(&mut w, alice, id, 36, "minecraft:brush"));
        let scutes = w
            .items
            .values()
            .filter(|i| ItemInfo::get(i.stack().item).unwrap().name == "minecraft:armadillo_scute")
            .count();
        assert_eq!(scutes, 0);
        assert_eq!(w.items.len(), before);
        assert_eq!(w.players[&alice].inventory.slots[36].damage, 0);
    }

    #[test]
    fn armadillos_are_saved_and_read_back_as_vanilla() {
        let (mut w, _) = world_with_player();
        let (_, mut mob) = armadillo(&mut w, [0.5, -60.0, 0.5]);
        set(&mut mob, |a| {
            a.scute_time = Some(1234);
            a.posture = Posture::Rolling;
        });
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        assert_eq!(nbt.int("scute_time"), Some(1234));
        assert_eq!(nbt.str("state"), Some("scared"));
        let mut back = Mob::new(MobKind::Armadillo, [0.5, -60.0, 0.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        assert_eq!(state(&back).posture, Posture::Scared);
        assert_eq!(state(&back).scute_time, Some(1234));
        let mut odd = Compound::new();
        odd.insert("state", "dancing");
        read_nbt(&mut back, &odd);
        assert_eq!(state(&back).posture, Posture::Idle);
        assert_eq!(Posture::read("unrolling"), Posture::Unrolling);
    }

    #[test]
    fn spiders_keep_away_from_unrolled_armadillos() {
        let (mut w, _) = world_with_player();
        let (id, mob) = armadillo(&mut w, [0.5, -60.0, 0.5]);
        w.mobs.insert(id, mob);
        let spider = next_entity_id();
        w.mobs.insert(
            spider,
            Mob::new(MobKind::Spider, [4.5, -60.0, 0.5], &mut w.rng),
        );
        let to = spider_flees_to(&w, [4.5, -60.0, 0.5]).expect("flees");
        assert!(to[0] > 4.5, "away from the armadillo");
        assert!(
            spider_flees_to(&w, [10.5, -60.0, 0.5]).is_none(),
            "far enough"
        );
        w.tick = SPIDER_CHECK_EVERY * 3;
        tick_world(&mut w);
        let target = w.mobs[&spider].target.expect("runs off").0;
        assert!(target[0] > 4.5);
        // A rolled-up armadillo does not scare them.
        set(w.mobs.get_mut(&id).unwrap(), |a| {
            a.posture = Posture::Scared
        });
        assert!(spider_flees_to(&w, [4.5, -60.0, 0.5]).is_none());
    }
}
