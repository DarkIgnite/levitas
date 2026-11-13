//! Bats: hanging from ceilings, fluttering about in the dark, and their own spawning.
//!
//! Behaviour follows the Minecraft Wiki (Bat, Mob spawning):
//! - A bat at rest hangs upside down from the underside of a solid block (entity data
//!   "is hanging"); it cannot hang from transparent or non-solid blocks such as glass,
//!   leaves or slabs. It turns now and then and stays until disturbed: a player coming
//!   close (even an invisible one, not a spectator), the block it hangs from going, or
//!   a hit. It then takes off (`minecraft:entity.bat.takeoff`).
//! - Awake, it flies about aimlessly: it keeps picking a new spot a few blocks away,
//!   often and with a pull upwards, and now and then settles on a ceiling it reaches.
//!   The shared flying moves it.
//! - Bats are the ambient category, which the shared spawning leaves out. Here, around
//!   each player (not spectators) in the Overworld, while fewer than 15 bats are within
//!   128 blocks, a group of up to 8 (the biome's ambient group) tries to appear in a
//!   random spot of the chunks around: in a biome that lists bats (every Overworld biome
//!   but the deep dark), at any height, not under the open sky, at light level 3 or
//!   less, with open space without liquid for its body and a block of
//!   `minecraft:bats_spawnable_on` (the Overworld's base stones) below, and no player
//!   within 24 blocks. The old Halloween rule (light up to 6 from October 20 to
//!   November 3) was taken out in 1.21.11, so 26.3 has none.
//! - Like monsters, bats despawn with no player within 128 blocks, and now and then
//!   beyond 32, unless named or persistent.
//! - They take no fall damage (the `fall_damage_immune` entity tag).
//! - Saved as vanilla: `BatFlags` (1 while hanging).
//!
//! Their ambient, hurt and death sounds go through the shared mob sounds.

use levitas_nbt::Compound;

use super::mobs::{Mob, MobKind};
use super::*;
use crate::blocks::fluid::fluid_at;
use crate::blocks::redstone::is_conductor;
use levitas_registry::collision_boxes;

const DATA_BAT_FLAGS: u8 = 16;
const SERIALIZER_BYTE: i32 = 0;
const FLAG_HANGING: u8 = 0x01;
/// Game mode 3: spectators neither wake bats nor bring them about.
const GAME_MODE_SPECTATOR: u8 = 3;
/// Players closer than this wake a hanging bat.
const WAKE_RANGE: f64 = 4.0;
/// Chances per tick: a flying bat settling on a ceiling it reached, picking a new spot
/// before it got to the last, and a hanging one turning about.
const HANG_ONE_IN: u32 = 100;
const NEW_GOAL_ONE_IN: u32 = 30;
const TURN_ONE_IN: u32 = 200;
/// How far it flies to its next spot: up to 7 blocks sideways, 2 down to 4 up.
const GOAL_REACH: u32 = 7;
const GOAL_DOWN: i32 = 2;
const GOAL_UP: i32 = 4;
/// How close the top of its head must be to a ceiling to hang from it.
const HANG_REACH: f64 = 0.5;
/// The ambient mob cap per player, and how far around a player it counts.
const AMBIENT_CAP: usize = 15;
const CAP_RANGE: f64 = 128.0;
/// Chunks around a player where bats may appear, and how close to a player they may not.
const SPAWN_CHUNKS: i32 = 8;
const SPAWN_DISTANCE: f64 = 24.0;
/// The darkest it must be, and the usual group size.
const SPAWN_LIGHT: i32 = 3;
const GROUP: u32 = 8;
const SPAWNABLE_ON: &str = "minecraft:bats_spawnable_on";
/// Despawning, as monsters do: at once beyond 128 blocks, one tick in 800 beyond 32.
const DESPAWN_FAR: f64 = 128.0;
const DESPAWN_NEAR: f64 = 32.0;
const DESPAWN_ONE_IN: u32 = 800;
const TAKEOFF_SOUND: &str = "minecraft:entity.bat.takeoff";

/// What the module keeps for a bat, in `Mob::kin`.
#[derive(Debug, Clone, Default, PartialEq)]
struct Bat {
    hanging: bool,
}

fn bat_of(mob: &Mob) -> Option<&Bat> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Bat>())
}

/// The bat's state, made on its first turn for bats that came in some other way
/// (summoned, from a spawn egg).
fn bat_mut(mob: &mut Mob) -> &mut Bat {
    if bat_of(mob).is_none() {
        mob.kin = Some(Box::new(Bat::default()));
    }
    mob.kin
        .as_mut()
        .and_then(|k| k.downcast_mut::<Bat>())
        .expect("bat state")
}

fn hanging(mob: &Mob) -> bool {
    bat_of(mob).is_some_and(|b| b.hanging)
}

fn flags_data(id: i32, hanging: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_BAT_FLAGS)
        .varint(SERIALIZER_BYTE)
        .u8(if hanging { FLAG_HANGING } else { 0 })
        .u8(0xFF)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn feet(pos: [f64; 3]) -> BlockPos {
    BlockPos::new(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    )
}

/// Whether `state` leaves room for a bat: nothing to bump into and no liquid.
fn open(state: u16) -> bool {
    collision_boxes(state).is_empty() && fluid_at(state).is_none()
}

/// Whether a bat can hang from `state`: a full, solid, opaque block (not glass, ice,
/// leaves, slabs or stairs).
fn holds_bat(state: u16) -> bool {
    is_conductor(state)
}

/// The biomes whose ambient spawns hold bats, with their group size.
fn bat_biomes() -> &'static std::collections::HashMap<String, u32> {
    static BIOMES: std::sync::OnceLock<std::collections::HashMap<String, u32>> =
        std::sync::OnceLock::new();
    BIOMES.get_or_init(|| {
        let biomes: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(levitas_registry::BIOMES_JSON).unwrap_or_default();
        biomes
            .into_iter()
            .filter_map(|(name, biome)| {
                let ambient = &biome["attributes"]["minecraft:gameplay/natural_mob_spawns"]
                    ["argument"]["spawns_by_category"]["ambient"];
                let bat = ambient
                    .as_array()?
                    .iter()
                    .find(|s| s["type"].as_str() == Some("minecraft:bat"))?;
                let count = &bat["count"];
                let size = count
                    .as_u64()
                    .or_else(|| count["max_inclusive"].as_u64())
                    .unwrap_or(u64::from(GROUP));
                Some((name, size.clamp(1, u64::from(GROUP)) as u32))
            })
            .collect()
    })
}

/// The group size bats come in where `biome` lets them, if it does.
fn bats_live_in(biome: &str) -> Option<u32> {
    bat_biomes().get(biome).copied()
}

pub(super) fn spawned(_w: &mut World, mob: &mut Mob, _biome: &str) {
    bat_mut(mob);
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    if bat_mut(mob).hanging {
        if !w.ceiling_holds(mob) || w.player_near(mob.pos, WAKE_RANGE) {
            w.wake_bat(id, mob);
            w.flutter(mob);
            return false;
        }
        mob.vel = [0.0; 3];
        mob.knockback = [0.0; 2];
        mob.fly_goal = None;
        if w.rng.below(TURN_ONE_IN) == 0 {
            mob.yaw = w.rng.f64() as f32 * 360.0;
        }
        return true;
    }
    if w.rng.below(HANG_ONE_IN) == 0 && !w.player_near(mob.pos, WAKE_RANGE) {
        if let Some(y) = w.hang_height(mob) {
            mob.pos[1] = y;
            mob.vel = [0.0; 3];
            mob.fly_goal = None;
            bat_mut(mob).hanging = true;
            w.send_to_viewers(mob, flags_data(id, true));
            return true;
        }
    }
    w.flutter(mob);
    false
}

pub(super) fn use_on(
    _w: &mut World,
    _player: i32,
    _target: i32,
    _slot: usize,
    _held: &str,
) -> bool {
    false
}

pub(super) fn hurt(_w: &mut World, _id: i32, _mob: &mut Mob, _attacker: Option<i32>) {}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    if hanging(mob) {
        vec![flags_data(id, true)]
    } else {
        Vec::new()
    }
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    nbt.insert("BatFlags", hanging(mob));
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let hanging = nbt.int("BatFlags").is_some_and(|v| v & 1 != 0);
    mob.kin = Some(Box::new(Bat { hanging }));
}

/// Bats take no fall damage; any other hit wakes a hanging one.
pub(super) fn damage_taken(
    w: &mut World,
    id: i32,
    mob: &mut Mob,
    damage: f32,
    damage_type: &str,
    _attacker: Option<i32>,
) -> f32 {
    if damage_type == "minecraft:fall" {
        return 0.0;
    }
    if damage > 0.0 && hanging(mob) {
        w.wake_bat(id, mob);
    }
    damage
}

pub(super) fn may_love(_w: &World, _mob: &Mob) -> bool {
    false
}

pub(super) fn bred(_w: &mut World, _id: i32, _mob: &mut Mob) -> bool {
    false
}

pub(super) fn tick_world(w: &mut World) {
    w.despawn_bats();
    w.spawn_bats();
}

pub(super) fn block_placed(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

/// The block a bat hangs from going is noticed on its next turn.
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

impl World {
    /// Whether a player who is not a spectator is within `range` of `pos`.
    fn player_near(&self, pos: [f64; 3], range: f64) -> bool {
        self.players
            .values()
            .any(|p| p.alive && p.mode != GAME_MODE_SPECTATOR && distance(p.pos, pos) < range)
    }

    /// Whether the block a hanging bat hangs from is still there to hold it.
    fn ceiling_holds(&self, mob: &Mob) -> bool {
        let top = mob.pos[1] + mob.size().1;
        let above = BlockPos::new(
            mob.pos[0].floor() as i32,
            (top + 0.05).floor() as i32,
            mob.pos[2].floor() as i32,
        );
        self.loaded_state(above).is_some_and(holds_bat)
    }

    /// Where a flying bat would hang from the ceiling just over its head (the y of its
    /// feet), if there is one within reach.
    fn hang_height(&self, mob: &Mob) -> Option<f64> {
        let height = mob.size().1;
        let top = mob.pos[1] + height;
        let ceiling = (top + HANG_REACH).floor();
        if ceiling < top - 1e-6 {
            return None; // the block its head is in: no ceiling over it yet
        }
        let above = BlockPos::new(
            mob.pos[0].floor() as i32,
            ceiling as i32,
            mob.pos[2].floor() as i32,
        );
        let state = self.loaded_state(above)?;
        if !holds_bat(state) {
            return None;
        }
        let y = ceiling - height;
        let at = [mob.pos[0], y, mob.pos[2]];
        self.mob_fits(MobKind::Bat, at).then_some(y)
    }

    /// A hanging bat takes off.
    fn wake_bat(&mut self, id: i32, mob: &mut Mob) {
        bat_mut(mob).hanging = false;
        self.send_to_viewers(mob, flags_data(id, false));
        let pitch = 0.8 + self.rng.f64() as f32 * 0.4;
        self.play_sound_at(TAKEOFF_SOUND, mob.pos, 0.05, pitch, None);
    }

    /// Erratic flight: a new spot a few blocks off whenever it gets near the last one,
    /// that one is blocked, or just now and then. The shared flying takes it there.
    fn flutter(&mut self, mob: &mut Mob) {
        let need = match mob.fly_goal {
            None => true,
            Some(g) => {
                distance(g, mob.pos) < 2.0
                    || !self.loaded_state(feet(g)).is_some_and(open)
                    || self.rng.below(NEW_GOAL_ONE_IN) == 0
            }
        };
        if !need {
            return;
        }
        let base = feet(mob.pos);
        let side = |rng: &mut Rng| rng.below(GOAL_REACH) as i32 - rng.below(GOAL_REACH) as i32;
        for _ in 0..4 {
            let dx = side(&mut self.rng);
            let dz = side(&mut self.rng);
            let dy = self.rng.below((GOAL_DOWN + GOAL_UP + 1) as u32) as i32 - GOAL_DOWN;
            let spot = BlockPos::new(base.x + dx, (base.y + dy).max(MIN_Y + 1), base.z + dz);
            let goal = [
                f64::from(spot.x) + 0.5,
                f64::from(spot.y) + 0.1,
                f64::from(spot.z) + 0.5,
            ];
            mob.fly_goal = Some(goal);
            if self.loaded_state(spot).is_some_and(open) {
                break;
            }
        }
    }

    /// Bats with no player close enough go, as monsters do. The list of those going
    /// only allocates on a tick when one does.
    fn despawn_bats(&mut self) {
        let mut gone = Vec::new();
        for (&id, m) in &self.mobs {
            if m.kind != MobKind::Bat
                || !m.alive()
                || m.persistent
                || m.name.is_some()
                || !self.chunks.contains_key(&m.chunk())
            {
                continue;
            }
            let nearest = self
                .players
                .values()
                .map(|p| distance(p.pos, m.pos))
                .fold(f64::INFINITY, f64::min);
            if nearest > DESPAWN_FAR
                || (nearest > DESPAWN_NEAR && self.rng.below(DESPAWN_ONE_IN) == 0)
            {
                gone.push(id);
            }
        }
        for id in gone {
            if let Some(mob) = self.mobs.remove(&id) {
                self.send_to_viewers(&mob, items::remove_entity(id));
            }
        }
    }

    /// How many bats are within the cap's range of `pos`.
    fn bats_near(&self, pos: [f64; 3]) -> usize {
        self.mobs
            .values()
            .filter(|m| m.kind == MobKind::Bat && m.alive() && distance(m.pos, pos) <= CAP_RANGE)
            .count()
    }

    /// One try per player and tick at a group of bats somewhere around them.
    fn spawn_bats(&mut self) {
        if self.dimension != super::dimension::Dimension::Overworld
            || !crate::rules::bool("spawn_mobs")
        {
            return;
        }
        let players: Vec<([f64; 3], (i32, i32))> = self
            .players
            .values()
            .filter(|p| p.alive && p.mode != GAME_MODE_SPECTATOR)
            .map(|p| (p.pos, p.center))
            .collect();
        for (pos, center) in players {
            if self.bats_near(pos) < AMBIENT_CAP {
                self.try_spawn_bats(center);
            }
        }
    }

    fn try_spawn_bats(&mut self, center: (i32, i32)) {
        let span = SPAWN_CHUNKS as u32 * 2 + 1;
        let cx = center.0 - SPAWN_CHUNKS + self.rng.below(span) as i32;
        let cz = center.1 - SPAWN_CHUNKS + self.rng.below(span) as i32;
        let x = cx * 16 + self.rng.below(16) as i32;
        let z = cz * 16 + self.rng.below(16) as i32;
        let Some(top) = self.top_block(x, z) else {
            return;
        };
        let bottom = self.dimension.span().min_y() + 1;
        if top <= bottom {
            return;
        }
        let y = bottom + self.rng.below((top - bottom) as u32) as i32;
        let Some(size) = self.biome_at(x, y, z).and_then(|b| bats_live_in(&b)) else {
            return;
        };
        let (mut gx, mut gz) = (x, z);
        for _ in 0..size {
            gx += self.rng.below(11) as i32 - 5;
            gz += self.rng.below(11) as i32 - 5;
            let pos = BlockPos::new(gx, y, gz);
            if self.bat_can_spawn(pos) {
                self.add_bat(pos);
            }
        }
    }

    /// Whether a bat may appear in the block at `pos` (see the module notes).
    fn bat_can_spawn(&self, pos: BlockPos) -> bool {
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        if self
            .players
            .values()
            .any(|p| distance(p.pos, centre) < SPAWN_DISTANCE)
        {
            return false;
        }
        let at = |dy: i32| self.loaded_block_at(BlockPos::new(pos.x, pos.y + dy, pos.z));
        let (Some(below), Some(feet), Some(head)) = (at(-1), at(0), at(1)) else {
            return false;
        };
        if !block_has_tag(below, SPAWNABLE_ON) || !open(feet) || fluid_at(head).is_some() {
            return false;
        }
        // Not under the open sky: something must be over it.
        if self.top_block(pos.x, pos.z).is_none_or(|top| top <= pos.y) {
            return false;
        }
        if self.light_at(pos).is_none() || self.light_level(pos) > SPAWN_LIGHT {
            return false;
        }
        let biome_ok = self
            .biome_at(pos.x, pos.y, pos.z)
            .is_some_and(|b| bats_live_in(&b).is_some());
        biome_ok && self.mob_fits(MobKind::Bat, centre)
    }

    fn add_bat(&mut self, pos: BlockPos) -> i32 {
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let mut mob = Mob::new(MobKind::Bat, at, &mut self.rng);
        mob.kin = Some(Box::new(Bat::default()));
        let id = next_entity_id();
        self.mobs.insert(id, mob);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-bats-{}-{}",
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
        // Far from the test spots near the origin, but close enough to keep bats.
        w.players
            .insert(alice, Player::new(profile, out, [-28.5, -60.0, -28.5]));
        (w, alice)
    }

    /// A sealed stone room: floor at y -61, air from -60 to -58, roof at -57, over
    /// x and z in `-r..=r`.
    fn cave(w: &mut World, r: i32) {
        for x in -r - 1..=r + 1 {
            for z in -r - 1..=r + 1 {
                for y in -61..=-57 {
                    let wall = x.abs() > r || z.abs() > r || y == -61 || y == -57;
                    let state = if wall { block::STONE } else { block::AIR };
                    w.set_block(BlockPos::new(x, y, z), state);
                }
            }
        }
    }

    fn bat(w: &mut World, at: [f64; 3], hanging: bool) -> (i32, Mob) {
        let mut mob = Mob::new(MobKind::Bat, at, &mut w.rng);
        mob.kin = Some(Box::new(Bat { hanging }));
        (next_entity_id(), mob)
    }

    #[test]
    fn bats_live_in_every_overworld_biome_but_the_deep_dark() {
        assert_eq!(bats_live_in("minecraft:plains"), Some(8));
        assert!(bats_live_in("minecraft:dripstone_caves").is_some());
        assert!(bats_live_in("minecraft:deep_dark").is_none());
        assert!(bats_live_in("minecraft:nether_wastes").is_none());
    }

    #[test]
    fn a_hanging_bat_stays_until_a_player_comes_close() {
        let (mut w, alice) = world_with_player();
        cave(&mut w, 3);
        // Hanging from the roof at -57: its head touches the block.
        let (id, mut mob) = bat(&mut w, [0.5, -57.0 - 0.9, 0.5], true);
        for _ in 0..50 {
            assert!(tick(&mut w, id, &mut mob));
        }
        assert!(hanging(&mob));
        assert_eq!(spawn_extras(id, &mob).len(), 1);
        w.players.get_mut(&alice).unwrap().pos = [1.5, -60.0, 0.5];
        assert!(!tick(&mut w, id, &mut mob));
        assert!(!hanging(&mob));
        assert!(mob.fly_goal.is_some());
        assert!(spawn_extras(id, &mob).is_empty());
    }

    #[test]
    fn spectators_do_not_wake_bats() {
        let (mut w, alice) = world_with_player();
        cave(&mut w, 3);
        let (id, mut mob) = bat(&mut w, [0.5, -57.9, 0.5], true);
        let p = w.players.get_mut(&alice).unwrap();
        p.pos = [1.5, -60.0, 0.5];
        p.mode = GAME_MODE_SPECTATOR;
        assert!(tick(&mut w, id, &mut mob));
        assert!(hanging(&mob));
    }

    #[test]
    fn a_bat_wakes_when_its_block_goes() {
        let (mut w, _) = world_with_player();
        cave(&mut w, 3);
        let (id, mut mob) = bat(&mut w, [0.5, -57.9, 0.5], true);
        assert!(tick(&mut w, id, &mut mob));
        w.set_block(BlockPos::new(0, -57, 0), block::AIR);
        assert!(!tick(&mut w, id, &mut mob));
        assert!(!hanging(&mob));
    }

    #[test]
    fn a_hit_wakes_a_bat_and_falls_do_not_hurt() {
        let (mut w, _) = world_with_player();
        cave(&mut w, 3);
        let (id, mut mob) = bat(&mut w, [0.5, -57.9, 0.5], true);
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 5.0, "minecraft:fall", None),
            0.0
        );
        assert!(hanging(&mob));
        assert_eq!(
            damage_taken(&mut w, id, &mut mob, 1.0, "minecraft:player_attack", None),
            1.0
        );
        assert!(!hanging(&mob));
    }

    #[test]
    fn a_flying_bat_hangs_only_from_a_solid_ceiling_in_reach() {
        let (mut w, _) = world_with_player();
        cave(&mut w, 3);
        // Head at -57.3: the roof is 0.3 above.
        let (_, mob) = bat(&mut w, [0.5, -58.2, 0.5], false);
        let y = w.hang_height(&mob).expect("hangs");
        assert!((y - (-57.0 - 0.9)).abs() < 1e-6, "{y}");
        // Low in the room, the roof is out of reach.
        let (_, low) = bat(&mut w, [0.5, -60.0, 0.5], false);
        assert!(w.hang_height(&low).is_none());
        // Under the open sky, nothing to hang from.
        let (_, outside) = bat(&mut w, [10.5, -58.2, 10.5], false);
        assert!(w.hang_height(&outside).is_none());
        // Given time, a bat under the roof settles there.
        let (id, mut mob) = bat(&mut w, [0.5, -58.0, 0.5], false);
        let mut settled = false;
        for _ in 0..2000 {
            mob.pos = [0.5, -58.0, 0.5];
            mob.fly_goal = None;
            if tick(&mut w, id, &mut mob) {
                settled = true;
                break;
            }
        }
        assert!(settled && hanging(&mob));
    }

    #[test]
    fn glass_does_not_hold_a_bat() {
        let (mut w, _) = world_with_player();
        cave(&mut w, 3);
        w.set_block(BlockPos::new(0, -57, 0), block::GLASS);
        // A flying bat under the glass does not settle there.
        let (_, mob) = bat(&mut w, [0.5, -58.2, 0.5], false);
        assert!(w.hang_height(&mob).is_none());
        // One that was hanging there lets go.
        let (id, mut mob) = bat(&mut w, [0.5, -57.9, 0.5], true);
        assert!(!tick(&mut w, id, &mut mob));
        assert!(!hanging(&mob));
    }

    #[test]
    fn flying_bats_keep_picking_spots_nearby() {
        let (mut w, _) = world_with_player();
        cave(&mut w, 6);
        let (id, mut mob) = bat(&mut w, [0.5, -59.5, 0.5], false);
        for _ in 0..200 {
            mob.fly_goal = None;
            tick(&mut w, id, &mut mob);
            if hanging(&mob) {
                bat_mut(&mut mob).hanging = false;
            }
            let g = mob.fly_goal.expect("a spot");
            assert!((g[0] - mob.pos[0]).abs() <= 7.0 && (g[2] - mob.pos[2]).abs() <= 7.0);
            assert!(g[1] - mob.pos[1] <= 4.6 && mob.pos[1] - g[1] <= 2.6);
        }
    }

    #[test]
    fn bats_spawn_only_in_dark_enclosed_stone() {
        let (mut w, _) = world_with_player();
        cave(&mut w, 3);
        let inside = BlockPos::new(0, -60, 0);
        assert!(w.bat_can_spawn(inside));
        // On grass under the sky: no.
        assert!(!w.bat_can_spawn(BlockPos::new(10, -60, 10)));
        // A stone floor under the open sky: no.
        w.set_block(BlockPos::new(12, -61, 12), block::STONE);
        assert!(!w.bat_can_spawn(BlockPos::new(12, -60, 12)));
        // Lit by glowstone: no.
        w.set_block(BlockPos::new(1, -58, 1), block::GLOWSTONE);
        assert!(!w.bat_can_spawn(inside));
    }

    #[test]
    fn bats_do_not_spawn_close_to_players() {
        let (mut w, alice) = world_with_player();
        cave(&mut w, 3);
        w.players.get_mut(&alice).unwrap().pos = [5.5, -60.0, 5.5];
        assert!(!w.bat_can_spawn(BlockPos::new(0, -60, 0)));
    }

    #[test]
    fn far_bats_despawn_and_named_ones_stay() {
        let (mut w, alice) = world_with_player();
        let (near, m) = bat(&mut w, [-20.5, -58.0, -20.5], false);
        w.mobs.insert(near, m);
        let (far, m) = bat(&mut w, [30.5, -58.0, 30.5], false);
        w.mobs.insert(far, m);
        let (named, mut m) = bat(&mut w, [31.5, -58.0, 31.5], false);
        m.name = Some("Bruce");
        w.mobs.insert(named, m);
        w.players.get_mut(&alice).unwrap().pos = [-200.5, -60.0, -200.5];
        w.despawn_bats();
        assert!(!w.mobs.contains_key(&far));
        assert!(!w.mobs.contains_key(&near));
        assert!(w.mobs.contains_key(&named));
        // A player close by keeps them.
        let (kept, m) = bat(&mut w, [0.5, -58.0, 0.5], false);
        w.mobs.insert(kept, m);
        w.players.get_mut(&alice).unwrap().pos = [3.5, -60.0, 0.5];
        for _ in 0..2000 {
            w.despawn_bats();
        }
        assert!(w.mobs.contains_key(&kept));
    }

    #[test]
    fn the_ambient_cap_stops_spawning() {
        let (mut w, alice) = world_with_player();
        cave(&mut w, 7);
        for i in 0..AMBIENT_CAP {
            let (id, m) = bat(&mut w, [i as f64, -58.0, 0.5], false);
            w.mobs.insert(id, m);
        }
        let pos = w.players[&alice].pos;
        assert!(w.bats_near(pos) >= AMBIENT_CAP);
        let before = w.mobs.len();
        for _ in 0..20_000 {
            w.spawn_bats();
        }
        assert_eq!(w.mobs.len(), before);
    }

    #[test]
    fn bats_appear_in_a_dark_cave_around_a_player() {
        let (mut w, alice) = world_with_player();
        cave(&mut w, 7);
        w.players.get_mut(&alice).unwrap().pos = [-28.5, -60.0, -28.5];
        for _ in 0..20_000 {
            w.spawn_bats();
            if w.mobs.values().any(|m| m.kind == MobKind::Bat) {
                break;
            }
        }
        let bats: Vec<&Mob> = w.mobs.values().filter(|m| m.kind == MobKind::Bat).collect();
        assert!(!bats.is_empty());
        for b in bats {
            assert!(b.pos[0].abs() < 8.0 && b.pos[2].abs() < 8.0);
            assert!((b.pos[1] - -60.0).abs() < 2.5);
        }
    }

    #[test]
    fn hanging_is_saved_as_vanilla_bat_flags() {
        let (mut w, _) = world_with_player();
        let (_, mob) = bat(&mut w, [0.5, -58.0, 0.5], true);
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        assert_eq!(nbt.int("BatFlags"), Some(1));
        let (_, mut fresh) = bat(&mut w, [0.5, -58.0, 0.5], false);
        read_nbt(&mut fresh, &nbt);
        assert!(hanging(&fresh));
    }
}
