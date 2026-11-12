//! Frogs, tadpoles and frogspawn.
//!
//! Behaviour follows the Minecraft Wiki (Frog, Tadpole, Frogspawn):
//! - Frogs come in three variants, by the biome they spawn or grow up in: warm in
//!   `#minecraft:spawns_warm_variant_frogs` (deserts, jungles, savannas, badlands, the
//!   Nether, warm oceans, mangrove swamps), cold in `#minecraft:spawns_cold_variant_frogs`
//!   (snowy and frozen biomes, groves, the deep dark, the End), temperate anywhere else.
//!   The variant is an ID in the `minecraft:frog_variant` registry and is saved by name,
//!   as vanilla does (`variant`).
//! - Frogs eat slimes and magma cubes of size 1 (`#minecraft:frog_food`) with their
//!   tongue: the mob is pulled in and vanishes without a death animation. A slime leaves
//!   one slimeball, a magma cube a froglight whose colour follows the frog: ochre for
//!   temperate, pearlescent for warm, verdant for cold (the slime and magma cube loot
//!   tables in `generated/` say the same).
//! - Two frogs bred with slimeballs make no baby: one of them goes to the nearest water
//!   block with air above (and water beside it) and lays frogspawn on it.
//! - Frogspawn sits on a water source and breaks when the source below is gone. It
//!   hatches into 2 to 5 tadpoles after 3600 to 12000 ticks, a delay that the game rule
//!   `random_tick_speed` does not change, so it is a scheduled block tick.
//! - Tadpoles grow into frogs after 20 minutes; each slimeball fed to one takes a tenth
//!   off the time left. The frog's variant comes from the biome where it grows up.

use levitas_nbt::Compound;

use super::breeding::BABY_AGE;
use super::mobs::{Mob, MobKind};
use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};

const DATA_POSE: u8 = 6;
// Ageable Mob takes 16 (baby) and 17 (age locked); Animal adds nothing.
const DATA_FROG_VARIANT: u8 = 18;
const DATA_FROG_TONGUE_TARGET: u8 = 19;
const SERIALIZER_OPTIONAL_VARINT: i32 = 19;
const SERIALIZER_POSE: i32 = 20;
const SERIALIZER_FROG_VARIANT: i32 = 27;
const POSE_STANDING: i32 = 0;
const POSE_USING_TONGUE: i32 = 9;

const TEMPERATE: &str = "minecraft:temperate";
const WARM: &str = "minecraft:warm";
const COLD: &str = "minecraft:cold";

/// How far a frog notices a small slime or magma cube, and how far its tongue reaches.
const HUNT_RANGE: f64 = 10.0;
const TONGUE_REACH: f64 = 3.0;
/// Ticks from the tongue shooting out to the prey being swallowed.
const TONGUE_TICKS: u8 = 6;
/// Ticks a frog waits after eating before it hunts again (the wiki gives no figure).
const EAT_COOLDOWN: u32 = 100;
/// Frogs look for prey only this often, to keep their turn cheap.
const HUNT_EVERY: u64 = 10;
/// How far a frog carrying spawn looks for water, and how close it must get to lay.
const SPAWN_SEARCH: i32 = 8;
const SPAWN_SEARCH_DOWN: i32 = 3;
const SPAWN_SEARCH_UP: i32 = 2;
const LAY_DISTANCE: f64 = 2.0;
/// Ticks between two searches for water while none is found.
const SPAWN_RETRY: u64 = 40;
/// Ticks from frogspawn being laid to it hatching.
const HATCH_DELAY_MIN: u32 = 3600;
const HATCH_DELAY_MAX: u32 = 12_000;
/// Tadpoles hatched from one frogspawn.
const TADPOLES_MIN: u32 = 2;
const TADPOLES_MAX: u32 = 5;
/// Ticks for a tadpole to grow up (vanilla saves its `Age` counting up to this).
const TADPOLE_GROW_TICKS: i32 = 24_000;

/// A frog's or tadpole's own state, kept in `Mob::kin`.
#[derive(Clone, Copy, Default)]
struct FrogState {
    /// Its variant (frog) or age (tadpole) has been settled: a mob from a spawn egg or
    /// command is settled on its first turn, by the biome it is in.
    settled: bool,
    /// Prey on its tongue and ticks until it is swallowed.
    tongue: Option<(i32, u8)>,
    cooldown: u32,
    /// Bred, and carrying frogspawn to lay; where, once found.
    pregnant: bool,
    spot: Option<BlockPos>,
}

fn state(mob: &mut Mob) -> &mut FrogState {
    if !mob.kin.as_ref().is_some_and(|k| k.is::<FrogState>()) {
        mob.kin = Some(Box::new(FrogState::default()));
    }
    mob.kin
        .as_mut()
        .and_then(|k| k.downcast_mut::<FrogState>())
        .expect("just set")
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

fn frog_variant_registry() -> Option<&'static SyncedRegistry> {
    SyncedRegistry::get("minecraft:frog_variant")
}

/// The network ID of a frog variant, by name; temperate (vanilla's default) for a name
/// the registry does not know.
fn variant_id(name: &str) -> u8 {
    frog_variant_registry()
        .and_then(|r| r.id_of(name).or_else(|| r.id_of(TEMPERATE)))
        .map_or(0, |i| i as u8)
}

/// The name of a frog variant, by network ID (temperate if unknown).
fn variant_name(id: u8) -> &'static str {
    frog_variant_registry()
        .and_then(|r| r.entries.get(usize::from(id)).copied())
        .unwrap_or(TEMPERATE)
}

/// The frog a biome grows, as 26.3's frog variants' spawn conditions say.
fn variant_for(biome: &str) -> u8 {
    let in_tag = |tag: &str| {
        let Some(id) = SyncedRegistry::get("minecraft:worldgen/biome").and_then(|r| r.id_of(biome))
        else {
            return false;
        };
        levitas_registry::TagRegistry::get("minecraft:worldgen/biome")
            .is_some_and(|t| t.members(tag).contains(&i64::from(id)))
    };
    let name = if in_tag("minecraft:spawns_warm_variant_frogs") {
        WARM
    } else if in_tag("minecraft:spawns_cold_variant_frogs") {
        COLD
    } else {
        TEMPERATE
    };
    variant_id(name)
}

/// The froglight a magma cube leaves when a frog of `variant` eats it.
fn froglight_for(variant: u8) -> &'static str {
    match variant_name(variant) {
        WARM => "minecraft:pearlescent_froglight",
        COLD => "minecraft:verdant_froglight",
        _ => "minecraft:ochre_froglight",
    }
}

fn variant_data(id: i32, mob: &Mob) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_FROG_VARIANT)
        .varint(SERIALIZER_FROG_VARIANT)
        .varint(i32::from(mob.variant))
        .u8(0xFF)
}

/// The tongue shooting out at `target`, or drawn back in.
fn tongue_data(id: i32, target: Option<i32>) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_FROG_TONGUE_TARGET)
        .varint(SERIALIZER_OPTIONAL_VARINT)
        .varint(target.map_or(0, |t| t + 1))
        .u8(DATA_POSE)
        .varint(SERIALIZER_POSE)
        .varint(if target.is_some() {
            POSE_USING_TONGUE
        } else {
            POSE_STANDING
        })
        .u8(0xFF)
}

/// Whether frogspawn can be laid on the water at `pos`: a water source with air above
/// and more water beside it.
fn spawn_spot(w: &World, pos: BlockPos) -> bool {
    let water = |p: BlockPos| {
        w.loaded_block(p)
            .and_then(fluid_at)
            .is_some_and(|f| f.fluid == Fluid::Water)
    };
    let source = w
        .loaded_block(pos)
        .and_then(fluid_at)
        .is_some_and(|f| f.fluid == Fluid::Water && f.source);
    let above = w
        .loaded_block(BlockPos::new(pos.x, pos.y + 1, pos.z))
        .is_some_and(is_air);
    source
        && above
        && [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .any(|&(dx, dz)| water(BlockPos::new(pos.x + dx, pos.y, pos.z + dz)))
}

/// The nearest water a frog at `at` can lay frogspawn on.
fn find_spawn_spot(w: &World, at: [f64; 3]) -> Option<BlockPos> {
    let here = block_at(at);
    let mut best: Option<(i32, BlockPos)> = None;
    for dy in -SPAWN_SEARCH_DOWN..=SPAWN_SEARCH_UP {
        for dx in -SPAWN_SEARCH..=SPAWN_SEARCH {
            for dz in -SPAWN_SEARCH..=SPAWN_SEARCH {
                let d = dx * dx + dy * dy + dz * dz;
                if best.is_some_and(|(b, _)| b <= d) {
                    continue;
                }
                let pos = BlockPos::new(here.x + dx, here.y + dy, here.z + dz);
                if spawn_spot(w, pos) {
                    best = Some((d, pos));
                }
            }
        }
    }
    best.map(|(_, pos)| pos)
}

fn spot_centre(spot: BlockPos) -> [f64; 3] {
    [
        f64::from(spot.x) + 0.5,
        f64::from(spot.y) + 0.5,
        f64::from(spot.z) + 0.5,
    ]
}

pub(super) fn spawned(_w: &mut World, mob: &mut Mob, biome: &str) {
    match mob.kind {
        MobKind::Frog => mob.variant = variant_for(biome),
        MobKind::Tadpole if mob.age >= 0 => mob.age = BABY_AGE,
        _ => {}
    }
    state(mob).settled = true;
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    let mut s = *state(mob);
    if !s.settled {
        s.settled = true;
        match mob.kind {
            MobKind::Frog => {
                let at = block_at(mob.pos);
                let biome = w.biome_at(at.x, at.y, at.z).unwrap_or_default();
                mob.variant = variant_for(&biome);
                w.send_to_viewers(mob, variant_data(id, mob));
            }
            // A tadpole from a spawn egg or command starts its 20 minutes now.
            MobKind::Tadpole if mob.age >= 0 => mob.age = BABY_AGE,
            _ => {}
        }
    }
    let busy = match mob.kind {
        MobKind::Tadpole => {
            if mob.age >= 0 {
                grow_up(w, id, mob);
                s = *state(mob);
            }
            false
        }
        MobKind::Frog => frog_turn(w, id, mob, &mut s),
        _ => false,
    };
    *state(mob) = s;
    busy
}

/// A frog's own turn: eating with its tongue, hunting, and carrying frogspawn to water.
/// Returns whether it stands still (its tongue is out).
fn frog_turn(w: &mut World, id: i32, mob: &mut Mob, s: &mut FrogState) -> bool {
    s.cooldown = s.cooldown.saturating_sub(1);
    if let Some((prey, ticks)) = s.tongue {
        if ticks > 1 {
            s.tongue = Some((prey, ticks - 1));
            mob.target = None;
            return true;
        }
        s.tongue = None;
        s.cooldown = EAT_COOLDOWN;
        swallow(w, mob, prey);
        w.send_to_viewers(mob, tongue_data(id, None));
        return true;
    }
    if s.pregnant {
        carry_spawn(w, mob, s);
        return false;
    }
    if s.cooldown > 0 || !w.tick.is_multiple_of(HUNT_EVERY) {
        return false;
    }
    let prey = w
        .mobs
        .iter()
        .filter(|(_, m)| {
            m.alive()
                && m.kind.family() == MobKind::MagmaCube
                && m.slime_size == 1
                && distance(m.pos, mob.pos) <= HUNT_RANGE
        })
        .map(|(&pid, m)| (pid, m.pos, distance(m.pos, mob.pos)))
        .min_by(|a, b| a.2.total_cmp(&b.2));
    let Some((pid, at, d)) = prey else {
        return false;
    };
    if d > TONGUE_REACH {
        mob.target = Some(([at[0], at[2]], 20));
        mob.fly_goal = Some(at);
        return false;
    }
    s.tongue = Some((pid, TONGUE_TICKS));
    mob.target = None;
    mob.path.clear();
    mob.yaw = super::mobs::yaw_towards(mob.pos, at);
    mob.head_yaw = mob.yaw;
    w.send_to_viewers(mob, tongue_data(id, Some(pid)));
    w.play_sound_at("minecraft:entity.frog.tongue", mob.pos, 2.0, 1.0, None);
    true
}

/// The prey on a frog's tongue is pulled in and eaten: it vanishes and leaves a
/// slimeball (slime) or a froglight of the frog's colour (magma cube).
fn swallow(w: &mut World, frog: &Mob, prey: i32) {
    let edible = w.mobs.get(&prey).is_some_and(|m| {
        m.alive()
            && m.kind.family() == MobKind::MagmaCube
            && m.slime_size == 1
            && distance(m.pos, frog.pos) <= TONGUE_REACH + 1.0
    });
    if !edible {
        return;
    }
    let eaten = w.mobs.remove(&prey).expect("checked");
    w.send_to_viewers(&eaten, items::remove_entity(prey));
    let drop = if eaten.kind == MobKind::MagmaCube {
        froglight_for(frog.variant)
    } else {
        "minecraft:slime_ball"
    };
    if let Some(item) = ItemInfo::id_of(drop) {
        w.scatter_item(frog.pos, Stack::new(item, 1));
    }
    w.play_sound_at("minecraft:entity.frog.eat", frog.pos, 1.0, 1.0, None);
}

/// A frog carrying spawn heads for the nearest water and lays it there.
fn carry_spawn(w: &mut World, mob: &mut Mob, s: &mut FrogState) {
    if s.spot.is_some_and(|spot| !spawn_spot(w, spot)) {
        s.spot = None;
    }
    if s.spot.is_none() {
        if !w.tick.is_multiple_of(SPAWN_RETRY) {
            return;
        }
        s.spot = find_spawn_spot(w, mob.pos);
    }
    let Some(spot) = s.spot else {
        return;
    };
    let goal = spot_centre(spot);
    let above = BlockPos::new(spot.x, spot.y + 1, spot.z);
    if distance(mob.pos, goal) > LAY_DISTANCE {
        mob.target = Some(([goal[0], goal[2]], 20));
        mob.fly_goal = Some(goal);
        return;
    }
    if let Some(spawn) = BlockInfo::by_name("minecraft:frogspawn") {
        w.set_block_updating(above, spawn.default_state);
        schedule_hatch(w, above, spawn.default_state);
    }
    w.play_sound_at("minecraft:entity.frog.lay_spawn", goal, 0.25, 1.0, None);
    s.pregnant = false;
    s.spot = None;
    mob.target = None;
    mob.fly_goal = None;
}

/// A tadpole grown up turns into a frog of the biome it is in.
fn grow_up(w: &mut World, id: i32, mob: &mut Mob) {
    let at = block_at(mob.pos);
    let biome = w.biome_at(at.x, at.y, at.z).unwrap_or_default();
    w.send_to_viewers(mob, items::remove_entity(id));
    mob.kind = MobKind::Frog;
    mob.variant = variant_for(&biome);
    mob.age = 0;
    mob.health = mob.max_health();
    mob.kin = Some(Box::new(FrogState {
        settled: true,
        ..FrogState::default()
    }));
    for packet in mob.spawn_packets(id) {
        w.send_to_viewers(mob, packet);
    }
    w.play_sound_at("minecraft:entity.tadpole.grow_up", mob.pos, 0.15, 1.0, None);
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    // Frogs eat slimeballs through the shared breeding code; tadpoles grow with them.
    if held != "minecraft:slime_ball" {
        return false;
    }
    let Some(mob) = w.mobs.get_mut(&target) else {
        return false;
    };
    if mob.kind != MobKind::Tadpole || mob.age >= 0 {
        return false;
    }
    mob.age += -mob.age / 10;
    let pos = mob.pos;
    w.play_sound_at("minecraft:entity.tadpole.flop", pos, 0.5, 1.2, None);
    if w.players.get(&player).is_some_and(|p| p.survival()) {
        let p = w.players.get_mut(&player).expect("checked");
        p.inventory.take(slot, 1);
        let update = p.inventory.set_slot_packet(slot);
        w.send(player, update);
    }
    true
}

pub(super) fn hurt(w: &mut World, id: i32, mob: &mut Mob, _attacker: Option<i32>) {
    // A frog hurt mid-meal lets go of its prey.
    if mob.kind == MobKind::Frog && state(mob).tongue.take().is_some() {
        w.send_to_viewers(mob, tongue_data(id, None));
    }
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    match mob.kind {
        MobKind::Frog => vec![variant_data(id, mob)],
        _ => Vec::new(),
    }
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    match mob.kind {
        MobKind::Frog => nbt.insert("variant", variant_name(mob.variant)),
        // Vanilla counts a tadpole's age up from 0 to 24000.
        MobKind::Tadpole => nbt.insert("Age", (mob.age + TADPOLE_GROW_TICKS).max(0)),
        _ => {}
    }
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    match mob.kind {
        MobKind::Frog => {
            // A frog saved without a variant is temperate, as in vanilla.
            mob.variant = variant_id(nbt.str("variant").unwrap_or(TEMPERATE));
        }
        MobKind::Tadpole => {
            // Vanilla's age counts up; one saved below 0 is already the countdown.
            let age = nbt.int("Age").unwrap_or(0);
            if age >= 0 {
                mob.age =
                    (age.min(i64::from(TADPOLE_GROW_TICKS)) as i32 - TADPOLE_GROW_TICKS).min(-1);
            }
        }
        _ => {}
    }
    state(mob).settled = true;
}

fn is_frogspawn(state: u16) -> bool {
    crate::blocks::name(state) == "minecraft:frogspawn"
}

/// Whether frogspawn at `pos` still sits on a water source.
fn on_water_source(w: &World, pos: BlockPos) -> bool {
    w.loaded_block(BlockPos::new(pos.x, pos.y - 1, pos.z))
        .and_then(fluid_at)
        .is_some_and(|f| f.fluid == Fluid::Water && f.source)
}

/// Starts the hatching timer of frogspawn just placed at `pos`.
fn schedule_hatch(w: &mut World, pos: BlockPos, state: u16) {
    let delay = HATCH_DELAY_MIN + w.rng.below(HATCH_DELAY_MAX - HATCH_DELAY_MIN + 1);
    w.schedule(pos, state, u64::from(delay));
}

/// A random tick of frogspawn: without a water source below it breaks, and frogspawn
/// placed without a hatching timer (by a command, say) gets one.
pub(super) fn block_tick(w: &mut World, pos: BlockPos, block_state: u16) -> bool {
    if !is_frogspawn(block_state) {
        return false;
    }
    if !on_water_source(w, pos) {
        w.set_block_updating(pos, block::AIR);
    } else if !w.is_scheduled(pos, block_state) {
        schedule_hatch(w, pos, block_state);
    }
    true
}

/// The scheduled tick of frogspawn: it hatches into tadpoles in the water below, or
/// just breaks if the water source is gone. Returns whether `pos` held frogspawn.
/// `World::tick_scheduled` calls it for frogspawn.
pub(super) fn hatch(w: &mut World, pos: BlockPos) -> bool {
    if !w.loaded_block(pos).is_some_and(is_frogspawn) {
        return false;
    }
    w.set_block_updating(pos, block::AIR);
    if !on_water_source(w, pos) {
        return true;
    }
    let at = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) - 0.6,
        f64::from(pos.z) + 0.5,
    ];
    w.play_sound_at("minecraft:block.frogspawn.hatch", at, 0.6, 1.2, None);
    let count = TADPOLES_MIN + w.rng.below(TADPOLES_MAX - TADPOLES_MIN + 1);
    for _ in 0..count {
        let spread = |w: &mut World| (w.rng.f64() - 0.5) * 0.4;
        let p = [at[0] + spread(w), at[1], at[2] + spread(w)];
        let mut tadpole = Mob::new(MobKind::Tadpole, p, &mut w.rng);
        tadpole.age = BABY_AGE;
        state(&mut tadpole).settled = true;
        w.mobs.insert(next_entity_id(), tadpole);
    }
    true
}

pub(super) fn bred(_w: &mut World, _id: i32, mob: &mut Mob) -> bool {
    let s = state(mob);
    s.pregnant = true;
    s.spot = None;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-frogs-{}-{}",
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

    fn hold(w: &mut World, id: i32, item: &str, count: i32) {
        w.players.get_mut(&id).unwrap().inventory.slots[36] =
            Stack::new(ItemInfo::id_of(item).unwrap(), count);
    }

    fn has_item(w: &World, name: &str) -> bool {
        let id = ItemInfo::id_of(name).unwrap();
        w.items.values().any(|i| i.stack().item == id)
    }

    /// A pond two blocks wide sunk into the flat ground at (3, -61, 3).
    fn pond(w: &mut World) -> BlockPos {
        w.set_block(BlockPos::new(3, -61, 3), block::WATER);
        w.set_block(BlockPos::new(4, -61, 3), block::WATER);
        BlockPos::new(3, -61, 3)
    }

    /// Ticks left on each frogspawn hatching timer in chunk (0, 0).
    fn frogspawn_timers(w: &World) -> Vec<i64> {
        w.scheduled_ticks_in(0, 0)
            .iter()
            .filter(|t| t.str("i") == Some("minecraft:frogspawn"))
            .filter_map(|t| t.int("t"))
            .collect()
    }

    fn small(kind: MobKind, w: &mut World, at: [f64; 3]) -> i32 {
        let mut m = Mob::new(kind, at, &mut w.rng);
        m.slime_size = 1;
        m.health = 1.0;
        let id = next_entity_id();
        w.mobs.insert(id, m);
        id
    }

    fn frog(w: &mut World, at: [f64; 3], variant: &str) -> Mob {
        let mut f = Mob::new(MobKind::Frog, at, &mut w.rng);
        f.variant = variant_id(variant);
        state(&mut f).settled = true;
        f
    }

    #[test]
    fn biomes_give_their_variants() {
        assert_eq!(variant_name(variant_for("minecraft:swamp")), TEMPERATE);
        assert_eq!(variant_name(variant_for("minecraft:plains")), TEMPERATE);
        assert_eq!(variant_name(variant_for("minecraft:mangrove_swamp")), WARM);
        assert_eq!(variant_name(variant_for("minecraft:jungle")), WARM);
        assert_eq!(variant_name(variant_for("minecraft:nether_wastes")), WARM);
        assert_eq!(variant_name(variant_for("minecraft:snowy_plains")), COLD);
        assert_eq!(variant_name(variant_for("minecraft:the_end")), COLD);
        assert_eq!(
            froglight_for(variant_id(COLD)),
            "minecraft:verdant_froglight"
        );
    }

    #[test]
    fn frogs_eat_small_slimes_for_slimeballs() {
        let (mut w, _) = world_with_player();
        let big = next_entity_id();
        let mut large = Mob::new(MobKind::Slime, [5.5, -60.0, 3.5], &mut w.rng);
        large.slime_size = 4;
        w.mobs.insert(big, large);
        let slime = small(MobKind::Slime, &mut w, [4.5, -60.0, 3.5]);
        let mut f = frog(&mut w, [3.5, -60.0, 3.5], TEMPERATE);
        for _ in 0..20 {
            w.tick += 1;
            tick(&mut w, 1, &mut f);
        }
        assert!(!w.mobs.contains_key(&slime), "eaten");
        assert!(w.mobs.contains_key(&big), "too big to eat");
        assert!(has_item(&w, "minecraft:slime_ball"));
    }

    #[test]
    fn frogs_turn_magma_cubes_into_froglights_of_their_colour() {
        let (mut w, _) = world_with_player();
        let cube = small(MobKind::MagmaCube, &mut w, [4.5, -60.0, 3.5]);
        let mut f = frog(&mut w, [3.5, -60.0, 3.5], WARM);
        for _ in 0..20 {
            w.tick += 1;
            tick(&mut w, 1, &mut f);
        }
        assert!(!w.mobs.contains_key(&cube));
        assert!(has_item(&w, "minecraft:pearlescent_froglight"));
        assert!(!has_item(&w, "minecraft:ochre_froglight"));
    }

    #[test]
    fn bred_frogs_lay_frogspawn_that_hatches_into_tadpoles() {
        let (mut w, _) = world_with_player();
        let water = pond(&mut w);
        let mut f = frog(&mut w, [3.5, -60.0, 3.5], TEMPERATE);
        assert!(bred(&mut w, 1, &mut f), "no baby frog");
        tick(&mut w, 1, &mut f);
        let spawn = BlockPos::new(water.x, water.y + 1, water.z);
        assert_eq!(crate::blocks::name(w.block(spawn)), "minecraft:frogspawn");
        assert!(!state(&mut f).pregnant);
        let timers = frogspawn_timers(&w);
        assert_eq!(timers.len(), 1, "hatching scheduled");
        assert!((3600..=12_000).contains(&timers[0]), "{timers:?}");
        assert!(hatch(&mut w, spawn));
        assert!(is_air(w.block(spawn)), "hatched");
        let tadpoles = w
            .mobs
            .values()
            .filter(|m| m.kind == MobKind::Tadpole && m.age == BABY_AGE)
            .count();
        assert!((2..=5).contains(&tadpoles), "{tadpoles}");
    }

    #[test]
    fn frogspawn_breaks_without_water() {
        let (mut w, _) = world_with_player();
        let pos = BlockPos::new(3, -60, 3);
        let spawn = BlockInfo::by_name("minecraft:frogspawn")
            .unwrap()
            .default_state;
        w.set_block(pos, spawn);
        assert!(block_tick(&mut w, pos, spawn));
        assert!(is_air(w.block(pos)));
        // Flowing water below is not enough, and the hatching tick breaks it too.
        w.set_block(BlockPos::new(3, -61, 3), block::WATER + 1);
        w.set_block(pos, spawn);
        assert!(block_tick(&mut w, pos, spawn));
        assert!(is_air(w.block(pos)));
        w.set_block(pos, spawn);
        assert!(hatch(&mut w, pos));
        assert!(is_air(w.block(pos)));
        assert!(!w.mobs.values().any(|m| m.kind == MobKind::Tadpole));
    }

    #[test]
    fn placed_frogspawn_gets_one_hatching_timer() {
        let (mut w, _) = world_with_player();
        let water = pond(&mut w);
        let pos = BlockPos::new(water.x, water.y + 1, water.z);
        let spawn = BlockInfo::by_name("minecraft:frogspawn")
            .unwrap()
            .default_state;
        w.set_block(pos, spawn);
        assert!(block_tick(&mut w, pos, spawn));
        assert!(block_tick(&mut w, pos, spawn));
        assert!(w.is_scheduled(pos, spawn));
        assert_eq!(frogspawn_timers(&w).len(), 1);
        assert_eq!(w.block(pos), spawn, "random ticks do not hatch it");
        assert!(!hatch(&mut w, BlockPos::new(0, -60, 0)), "not frogspawn");
    }

    #[test]
    fn tadpoles_grow_into_frogs_of_their_biome_faster_when_fed() {
        let (mut w, alice) = world_with_player();
        let id = next_entity_id();
        let mut t = Mob::new(MobKind::Tadpole, [3.5, -60.0, 3.5], &mut w.rng);
        t.age = BABY_AGE;
        w.mobs.insert(id, t);
        hold(&mut w, alice, "minecraft:slime_ball", 2);
        assert!(use_on(&mut w, alice, id, 36, "minecraft:slime_ball"));
        assert_eq!(w.mobs[&id].age, BABY_AGE + 2400);
        assert_eq!(w.players[&alice].inventory.slots[36].count, 1);
        let mut t = w.mobs.remove(&id).unwrap();
        state(&mut t).settled = true;
        t.age = -1;
        t.age += 1; // as the shared breeding code does each tick
        tick(&mut w, id, &mut t);
        assert_eq!(t.kind, MobKind::Frog);
        let at = block_at(t.pos);
        let biome = w.biome_at(at.x, at.y, at.z).unwrap();
        assert_eq!(t.variant, variant_for(&biome));
        assert_eq!(t.health, MobKind::Frog.max_health());
    }

    #[test]
    fn hatched_tadpoles_do_not_grow_up_at_once() {
        let (mut w, _) = world_with_player();
        let water = pond(&mut w);
        let spawn = BlockPos::new(water.x, water.y + 1, water.z);
        let block = BlockInfo::by_name("minecraft:frogspawn")
            .unwrap()
            .default_state;
        w.set_block(spawn, block);
        assert!(hatch(&mut w, spawn));
        let ids: Vec<i32> = w.mobs.keys().copied().collect();
        for id in ids {
            let mut t = w.mobs.remove(&id).unwrap();
            tick(&mut w, id, &mut t);
            assert_eq!(t.kind, MobKind::Tadpole);
            assert_eq!(t.age, BABY_AGE);
        }
    }

    #[test]
    fn spawn_egg_tadpoles_start_growing() {
        let (mut w, _) = world_with_player();
        let mut t = Mob::new(MobKind::Tadpole, [3.5, -60.0, 3.5], &mut w.rng);
        t.age = 0;
        tick(&mut w, 1, &mut t);
        assert_eq!(t.kind, MobKind::Tadpole);
        assert_eq!(t.age, BABY_AGE);
    }

    #[test]
    fn frogs_and_tadpoles_save_as_vanilla() {
        let (mut w, _) = world_with_player();
        let f = frog(&mut w, [3.5, -60.0, 3.5], COLD);
        let mut nbt = Compound::new();
        write_nbt(&f, &mut nbt);
        assert_eq!(nbt.str("variant"), Some(COLD));
        let mut back = Mob::new(MobKind::Frog, [3.5, -60.0, 3.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        assert_eq!(back.variant, variant_id(COLD));
        // No variant saved, or an unknown one: temperate, as in vanilla.
        let mut back = Mob::new(MobKind::Frog, [3.5, -60.0, 3.5], &mut w.rng);
        back.variant = variant_id(COLD);
        read_nbt(&mut back, &Compound::new());
        assert_eq!(variant_name(back.variant), TEMPERATE);
        assert_eq!(variant_name(variant_id("minecraft:nonsense")), TEMPERATE);

        let mut t = Mob::new(MobKind::Tadpole, [3.5, -60.0, 3.5], &mut w.rng);
        t.age = -18_000;
        let mut nbt = Compound::new();
        write_nbt(&t, &mut nbt);
        assert_eq!(nbt.int("Age"), Some(6000));
        let mut back = Mob::new(MobKind::Tadpole, [3.5, -60.0, 3.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        assert_eq!(back.age, -18_000);
    }
}
