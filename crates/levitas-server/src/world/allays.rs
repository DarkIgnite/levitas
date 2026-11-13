//! Allays: carrying items to a player or a note block, dancing and duplicating.
//!
//! Behaviour follows the Minecraft Wiki (Allay, Jukebox, Note Block):
//! - A player uses an item on an allay with an empty hand: it takes one of it into its
//!   main hand (players not in creative lose it) and from then likes that player. Using
//!   an empty main hand on it gives the item back (and lets go of what it carries). A
//!   named name tag does its usual job instead.
//! - An allay holding an item follows the player who gave it from up to 64 blocks
//!   away, staying close. It seeks dropped stacks of the same item (a potion or tipped
//!   arrow only of the same potion; it does not mind enchantments, names or other item
//!   data, but carries different kinds on separate trips) within 32 blocks of that
//!   player, takes them when about 1.3 blocks from them, up to a stack in its one
//!   inventory slot, and once full or out of finds throws them when within 3 blocks of
//!   the player. After a delivery it waits three seconds before seeking again. It never
//!   picks up items without `mobGriefing`, nor ones an allay threw. It looks around for
//!   items once a second, and follows the one it picked in between.
//! - A note block played within 16 blocks of an allay holding an item (and not focused
//!   on another one) becomes where it delivers for 30 seconds; playing it again
//!   restarts that.
//! - The player who gave it the item cannot hurt it. It heals 2 health a second, and
//!   drops what it holds and carries when it dies.
//! - Within 3 blocks of a jukebox playing a disc it dances. A dancing allay given an
//!   amethyst shard duplicates: the shard is used up and a second allay appears where
//!   it is; both then wait five minutes before they may duplicate again.
//! - Saved as vanilla: the held item in `equipment.mainhand`, what it carries in
//!   `Inventory`, `DuplicationCooldown`, and the player it likes in its `Brain`
//!   (`minecraft:liked_player`).

use levitas_nbt::{Compound, Tag};

use super::mobs::{yaw_towards, Mob, MobKind};
use super::*;

const DATA_DANCING: u8 = 16;
const DATA_CAN_DUPLICATE: u8 = 17;
const SERIALIZER_BOOL: i32 = 8;
/// The main hand, as the last entry of an equipment packet.
const SLOT_MAIN_HAND: u8 = 0;
/// How far from its player an allay still follows, and how far from the player (or
/// note block) it seeks items.
const FOLLOW_RANGE: f64 = 64.0;
const SEEK_RANGE: f64 = 32.0;
/// How close an item must be to be taken, and how close the player or note block to
/// throw it what was collected.
const PICKUP_REACH: f64 = 1.3;
const THROW_RANGE: f64 = 3.0;
/// Ticks after a delivery before it seeks items again.
const DELIVERY_COOLDOWN: u32 = 60;
/// How often an allay looks over all dropped items for one it wants.
const SCAN_EVERY: u64 = 20;
/// How far a note block is heard, and for how many ticks an allay stays with it.
const NOTE_RANGE: f64 = 16.0;
const NOTE_TICKS: u32 = 600;
/// How close to a playing jukebox an allay dances, and how often it looks for one.
const DANCE_RANGE: f64 = 3.0;
const DANCE_CHECK: u64 = 10;
/// Ticks before an allay may duplicate again: five minutes.
const DUPLICATION_COOLDOWN: u32 = 6000;
/// Stays this close to the player it follows.
const STAY_NEAR: f64 = 2.0;
/// Flying speeds in blocks per tick: about its business, and catching up from afar.
const SPEED: f64 = 0.2;
const CATCH_UP_SPEED: f64 = 0.35;
const CATCH_UP_BEYOND: f64 = 16.0;
/// Heals 1 health every this many ticks (2 a second).
const HEAL_EVERY: u64 = 10;
/// One in this many ticks it makes its ambient sound.
const AMBIENT_ONE_IN: u32 = 120;
/// How often the module looks for dying allays that still hold items.
const DEATH_CHECK: u64 = 5;
/// The pitches its item-thrown sound plays at.
const THROW_PITCHES: [f32; 11] = [
    0.375, 0.5625, 0.625, 0.75, 1.0, 1.125, 1.25, 1.5, 1.875, 2.0, 2.25,
];
const STATE_KEY: &str = "allays";

/// What the module keeps for an allay, in `Mob::kin`.
#[derive(Debug, Clone, PartialEq, Default)]
struct Allay {
    /// The item a player gave it, in its main hand.
    held: Stack,
    /// What it has collected and is bringing back.
    carried: Stack,
    /// The UUID of the player who gave it the item.
    liked: Option<u128>,
    /// The note block it delivers to instead, and for how many more ticks.
    note: Option<(BlockPos, u32)>,
    dancing: bool,
    /// Ticks before it may duplicate again.
    duplication_cooldown: u32,
    /// Ticks before it seeks items again after a delivery.
    delivery_cooldown: u32,
    /// The dropped item it is flying to, between full looks around.
    target_item: Option<i32>,
}

impl Allay {
    fn can_duplicate(&self) -> bool {
        self.duplication_cooldown == 0
    }

    /// Whether a dropped stack is one it is after: the same item as it holds (and the
    /// same potion, for potions and tipped arrows), and one it can carry with what it
    /// already has.
    fn wants(&self, stack: &Stack) -> bool {
        !self.held.is_empty()
            && stack.item == self.held.item
            && stack.potion == self.held.potion
            && (self.carried.is_empty()
                || (self.carried.same_kind(stack) && self.carried.count < self.carried.max_stack()))
    }
}

/// Items an allay threw, which allays leave lying. Lives in `World::kin_state`.
#[derive(Default)]
struct Thrown {
    ids: HashSet<i32>,
}

fn thrown(w: &mut World) -> &mut Thrown {
    let entry = w
        .kin_state
        .entry(STATE_KEY)
        .or_insert_with(|| Box::new(Thrown::default()));
    if !entry.is::<Thrown>() {
        *entry = Box::new(Thrown::default());
    }
    entry
        .downcast_mut::<Thrown>()
        .expect("just made sure it is one")
}

/// The items allays threw, if any were.
fn thrown_ids(w: &World) -> Option<&HashSet<i32>> {
    w.kin_state
        .get(STATE_KEY)
        .and_then(|s| s.downcast_ref::<Thrown>())
        .map(|t| &t.ids)
}

fn was_thrown(w: &World, id: i32) -> bool {
    thrown_ids(w).is_some_and(|ids| ids.contains(&id))
}

fn allay_of(mob: &Mob) -> Option<&Allay> {
    mob.kin.as_ref().and_then(|k| k.downcast_ref::<Allay>())
}

/// The allay's state, made fresh if it has none yet.
fn allay_mut(mob: &mut Mob) -> &mut Allay {
    if allay_of(mob).is_none() {
        mob.kin = Some(Box::new(Allay::default()));
    }
    mob.kin
        .as_mut()
        .and_then(|k| k.downcast_mut::<Allay>())
        .expect("just made sure it is one")
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

fn center(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

/// Where an allay goes to deliver at a note block: just above it.
fn above(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 1.2,
        f64::from(pos.z) + 0.5,
    ]
}

fn uuid_ints(uuid: u128) -> Vec<i32> {
    vec![
        (uuid >> 96) as i32,
        (uuid >> 64) as i32,
        (uuid >> 32) as i32,
        uuid as i32,
    ]
}

fn uuid_from(ints: &[i32]) -> Option<u128> {
    let [a, b, c, d] = <[i32; 4]>::try_from(ints).ok()?;
    Some(
        (u128::from(a as u32) << 96)
            | (u128::from(b as u32) << 64)
            | (u128::from(c as u32) << 32)
            | u128::from(d as u32),
    )
}

fn allay_data(id: i32, a: &Allay) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_DANCING)
        .varint(SERIALIZER_BOOL)
        .bool(a.dancing)
        .u8(DATA_CAN_DUPLICATE)
        .varint(SERIALIZER_BOOL)
        .bool(a.can_duplicate())
        .u8(0xFF)
}

fn hand_packet(id: i32, stack: Stack) -> Writer {
    let packet = Writer::packet(clientbound::SET_EQUIPMENT)
        .varint(id)
        .u8(SLOT_MAIN_HAND);
    crate::inventory::write_stack(packet, stack)
}

/// A note block was played at `pos`: allays holding an item within hearing deliver
/// there for the next 30 seconds, unless they are with another note block. Called for
/// a note block used by a player; `World::play_note` should call it for every note
/// (redstone too). Hearing the same note twice is harmless.
pub(super) fn note_played(w: &mut World, pos: BlockPos) {
    let at = center(pos);
    for mob in w.mobs.values_mut() {
        if mob.kind != MobKind::Allay || !mob.alive() || distance(mob.pos, at) > NOTE_RANGE {
            continue;
        }
        let a = allay_mut(mob);
        if a.held.is_empty() || a.note.is_some_and(|(other, _)| other != pos) {
            continue;
        }
        a.note = Some((pos, NOTE_TICKS));
        a.target_item = None;
    }
}

/// An allay died: it drops what it holds and what it carries. Safe to call twice.
pub(super) fn died(w: &mut World, mob: &mut Mob) {
    let Some(a) = mob.kin.as_mut().and_then(|k| k.downcast_mut::<Allay>()) else {
        return;
    };
    let drops = [std::mem::take(&mut a.held), std::mem::take(&mut a.carried)];
    for stack in drops.into_iter().filter(|s| !s.is_empty()) {
        w.scatter_item(mob.pos, stack);
    }
}

pub(super) fn spawned(_w: &mut World, mob: &mut Mob, _biome: &str) {
    mob.kin = Some(Box::new(Allay::default()));
}

pub(super) fn tick(w: &mut World, id: i32, mob: &mut Mob) -> bool {
    allay_mut(mob);
    let Some(mut kin) = mob.kin.take() else {
        return false;
    };
    let busy = match kin.downcast_mut::<Allay>() {
        Some(a) => w.tick_allay(id, mob, a),
        None => false,
    };
    mob.kin = Some(kin);
    busy
}

pub(super) fn use_on(w: &mut World, player: i32, target: i32, slot: usize, held: &str) -> bool {
    let Some(mut mob) = w.mobs.remove(&target) else {
        return false;
    };
    let took = w.use_on_allay(player, target, &mut mob, slot, held);
    w.mobs.insert(target, mob);
    took
}

pub(super) fn hurt(w: &mut World, _id: i32, mob: &mut Mob, _attacker: Option<i32>) {
    if mob.health <= 0.0 {
        died(w, mob);
    }
}

pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    let mut packets = Vec::new();
    if let Some(a) = allay_of(mob) {
        if !a.held.is_empty() {
            packets.push(hand_packet(id, a.held));
        }
        if a.dancing || !a.can_duplicate() {
            packets.push(allay_data(id, a));
        }
    }
    packets
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    let default = Allay::default();
    let a = allay_of(mob).unwrap_or(&default);
    // Always written, so that a copy kept from loading never brings back an item given
    // away since.
    let mut equipment = Compound::new();
    if !a.held.is_empty() {
        equipment.insert("mainhand", a.held.to_nbt());
    }
    nbt.insert("equipment", equipment);
    let inventory: Vec<Tag> = if a.carried.is_empty() {
        Vec::new()
    } else {
        vec![Tag::Compound(a.carried.to_nbt())]
    };
    nbt.insert("Inventory", inventory);
    nbt.insert("DuplicationCooldown", i64::from(a.duplication_cooldown));
    let mut memories = Compound::new();
    if let Some(uuid) = a.liked {
        memories.insert(
            "minecraft:liked_player",
            Compound::new().with("value", uuid_ints(uuid)),
        );
    }
    nbt.insert("Brain", Compound::new().with("memories", memories));
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    let held = nbt
        .compound("equipment")
        .and_then(|e| e.compound("mainhand"))
        .map_or(Stack::EMPTY, Stack::from_nbt);
    let carried = nbt
        .list("Inventory")
        .and_then(|l| l.iter().find_map(Tag::as_compound))
        .map_or(Stack::EMPTY, Stack::from_nbt);
    let liked = nbt
        .compound("Brain")
        .and_then(|b| b.compound("memories"))
        .and_then(|m| m.compound("minecraft:liked_player"))
        .and_then(|l| l.int_array("value"))
        .and_then(uuid_from);
    let duplication_cooldown = nbt
        .int("DuplicationCooldown")
        .unwrap_or(0)
        .clamp(0, i64::from(u32::MAX)) as u32;
    mob.kin = Some(Box::new(Allay {
        held,
        carried,
        liked,
        duplication_cooldown,
        ..Allay::default()
    }));
}

pub(super) fn damage_taken(
    w: &mut World,
    _id: i32,
    mob: &mut Mob,
    damage: f32,
    _damage_type: &str,
    attacker: Option<i32>,
) -> f32 {
    // The player who gave it its item cannot hurt it.
    let liked = allay_of(mob).and_then(|a| a.liked);
    let by_friend = attacker
        .and_then(|pid| w.players.get(&pid))
        .is_some_and(|p| Some(p.profile.uuid) == liked);
    if by_friend {
        0.0
    } else {
        damage
    }
}

/// Allays do not breed.
pub(super) fn may_love(_w: &World, _mob: &Mob) -> bool {
    false
}

pub(super) fn bred(_w: &mut World, _id: i32, _mob: &mut Mob) -> bool {
    false
}

pub(super) fn tick_world(w: &mut World) {
    // Allays that died some other way than a blow (lava, fire, magic, /kill) still drop
    // what they hold and carry; the dying ones are kept for a second, so this catches
    // them before they go.
    if w.tick.is_multiple_of(DEATH_CHECK) {
        let mut drops = Vec::new();
        for mob in w.mobs.values_mut() {
            if mob.kind != MobKind::Allay || mob.alive() {
                continue;
            }
            if let Some(a) = mob.kin.as_mut().and_then(|k| k.downcast_mut::<Allay>()) {
                for stack in [std::mem::take(&mut a.held), std::mem::take(&mut a.carried)] {
                    if !stack.is_empty() {
                        drops.push((mob.pos, stack));
                    }
                }
            }
        }
        for (at, stack) in drops {
            w.scatter_item(at, stack);
        }
    }
    // Forget thrown items that are gone (picked up, despawned).
    if !w.tick.is_multiple_of(100) || !w.kin_state.contains_key(STATE_KEY) {
        return;
    }
    let mut ids = std::mem::take(&mut thrown(w).ids);
    ids.retain(|id| w.items.contains_key(id));
    thrown(w).ids = ids;
}

pub(super) fn block_placed(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

pub(super) fn block_broken(_w: &mut World, _player: i32, _pos: BlockPos, _state: u16) {}

pub(super) fn block_used(
    w: &mut World,
    player: i32,
    slot: usize,
    pos: BlockPos,
    state: u16,
) -> bool {
    // Using a note block plays it (the shared code tunes it): allays hear that. Only a
    // note block with room above it makes a sound. Redstone-played notes need
    // `World::play_note` to call `note_played`.
    if crate::blocks::name(state) != "minecraft:note_block" {
        return false;
    }
    let Some(p) = w.players.get(&player) else {
        return false;
    };
    let plays = !(p.sneaking && !p.inventory.slots[slot].is_empty());
    if plays && is_air(w.block(BlockPos::new(pos.x, pos.y + 1, pos.z))) {
        note_played(w, pos);
    }
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
    /// An allay's turn. Returns whether it moved itself (it is about its business).
    fn tick_allay(&mut self, id: i32, mob: &mut Mob, a: &mut Allay) -> bool {
        if self.tick.is_multiple_of(HEAL_EVERY) && mob.health < mob.max_health() {
            mob.health = (mob.health + 1.0).min(mob.max_health());
        }
        if a.duplication_cooldown > 0 {
            a.duplication_cooldown -= 1;
            if a.duplication_cooldown == 0 {
                self.send_to_viewers(mob, allay_data(id, a));
            }
        }
        a.delivery_cooldown = a.delivery_cooldown.saturating_sub(1);
        if let Some((pos, ticks)) = a.note {
            let still = self
                .loaded_block(pos)
                .is_some_and(|s| crate::blocks::name(s) == "minecraft:note_block");
            a.note = (ticks > 1 && still && !a.held.is_empty()).then_some((pos, ticks - 1));
        }
        if self.tick.is_multiple_of(DANCE_CHECK) {
            let dancing = self.jukebox_playing_near(mob.pos);
            if dancing != a.dancing {
                a.dancing = dancing;
                self.send_to_viewers(mob, allay_data(id, a));
            }
        }
        if self.rng.below(AMBIENT_ONE_IN) == 0 {
            let (sound, pitch) = if a.held.is_empty() {
                (
                    "minecraft:entity.allay.ambient_without_item",
                    0.8 + self.rng.f64() as f32 * 0.4,
                )
            } else {
                (
                    "minecraft:entity.allay.ambient_with_item",
                    1.0 + self.rng.f64() as f32 * 0.5,
                )
            };
            self.play_sound_at(sound, mob.pos, 1.0, pitch, None);
        }
        if mob.panic > 0 || mob.vehicle.is_some() {
            return false; // flutters off for a while after being hurt
        }
        if a.held.is_empty() {
            a.target_item = None;
            if a.dancing {
                self.fly_allay(mob, mob.pos, 0.0);
                return true;
            }
            return false;
        }
        // Where it brings what it finds: its note block, or its player.
        let goal = match a.note {
            Some((pos, _)) => Some(above(pos)),
            None => self.liked_player_at(a.liked).and_then(|(_, at)| {
                let chest = [at[0], at[1] + 1.0, at[2]];
                (distance(chest, mob.pos) <= FOLLOW_RANGE).then_some(chest)
            }),
        };
        let Some(goal) = goal else {
            a.target_item = None;
            if a.dancing {
                self.fly_allay(mob, mob.pos, 0.0);
                return true;
            }
            return false; // no one to bring things to: it wanders
        };
        let full = !a.carried.is_empty() && a.carried.count >= a.carried.max_stack();
        if a.delivery_cooldown == 0 && !full && crate::rules::bool("mob_griefing") {
            if let Some((item_id, at)) = self.item_for_allay(id, mob.pos, goal, a) {
                if self.in_pickup_reach(mob.pos, at) {
                    self.allay_take(id, item_id, a);
                    a.target_item = None;
                } else {
                    self.fly_allay(mob, [at[0], at[1] + 0.2, at[2]], SPEED);
                }
                return true;
            }
        } else {
            a.target_item = None;
        }
        let away = distance(mob.pos, goal);
        if !a.carried.is_empty() {
            if away <= THROW_RANGE {
                self.allay_throw(mob, a, goal);
            } else {
                self.fly_allay(mob, goal, speed_for(away));
            }
            return true;
        }
        if away > STAY_NEAR {
            self.fly_allay(mob, goal, speed_for(away));
        } else {
            self.fly_allay(mob, mob.pos, 0.0);
        }
        true
    }

    /// A player uses `held` (in `slot`) on an allay: gives it, takes its item back, or
    /// makes a dancing one duplicate.
    fn use_on_allay(
        &mut self,
        player: i32,
        target: i32,
        mob: &mut Mob,
        slot: usize,
        held: &str,
    ) -> bool {
        let Some(p) = self.players.get(&player) else {
            return false;
        };
        let stack = p.inventory.slots[slot];
        let main_hand = slot == p.hand_slot(0);
        // Everyone but creative players gives the item away.
        let (consume, uuid) = (p.mode != GAME_MODE_CREATIVE, p.profile.uuid);
        let a = allay_mut(mob).clone();
        if stack.is_empty() || held.is_empty() || held == "minecraft:air" {
            // Only an empty main hand takes the item back.
            if a.held.is_empty() || !main_hand {
                return false;
            }
            // The item goes back into the player's hand; what it carried drops.
            let p = self.players.get_mut(&player).expect("checked");
            p.inventory.slots[slot] = a.held;
            let update = p.inventory.set_slot_packet(slot);
            self.send(player, update);
            if !a.carried.is_empty() {
                self.spawn_dropped(a.carried, mob.pos, [0.0, 0.1, 0.0]);
            }
            let s = allay_mut(mob);
            s.held = Stack::EMPTY;
            s.carried = Stack::EMPTY;
            s.liked = None;
            s.note = None;
            s.target_item = None;
            self.send_to_viewers(mob, hand_packet(target, Stack::EMPTY));
            self.play_sound_at(
                "minecraft:entity.allay.item_taken",
                mob.pos,
                1.0,
                1.25,
                None,
            );
            return true;
        }
        if held == "minecraft:amethyst_shard" && a.dancing && a.can_duplicate() {
            self.allay_takes_one(player, slot, consume);
            self.duplicate_allay(target, mob);
            return true;
        }
        // A lead leashes it, and a named name tag names it, as for any mob.
        let named_tag = held == "minecraft:name_tag" && stack.name.is_some();
        if !a.held.is_empty() || held == "minecraft:lead" || named_tag {
            return false;
        }
        self.allay_takes_one(player, slot, consume);
        mob.persistent = true;
        let s = allay_mut(mob);
        s.held = stack.with_count(1);
        s.liked = Some(uuid);
        s.delivery_cooldown = 0;
        s.target_item = None;
        self.send_to_viewers(mob, hand_packet(target, stack.with_count(1)));
        self.play_sound_at("minecraft:entity.allay.item_given", mob.pos, 1.0, 1.0, None);
        true
    }

    /// Uses up one item from the player's `slot`, unless `consume` is false (creative).
    fn allay_takes_one(&mut self, player: i32, slot: usize, consume: bool) {
        if !consume {
            return;
        }
        if let Some(p) = self.players.get_mut(&player) {
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(player, update);
        }
    }

    /// A dancing allay given an amethyst shard splits in two; both wait five minutes
    /// before they may do it again.
    fn duplicate_allay(&mut self, id: i32, mob: &mut Mob) {
        let a = allay_mut(mob);
        a.duplication_cooldown = DUPLICATION_COOLDOWN;
        let shown = allay_data(id, a);
        self.send_to_viewers(mob, shown);
        let mut twin = Mob::new(MobKind::Allay, mob.pos, &mut self.rng);
        twin.kin = Some(Box::new(Allay {
            duplication_cooldown: DUPLICATION_COOLDOWN,
            dancing: true,
            ..Allay::default()
        }));
        self.mobs.insert(next_entity_id(), twin);
        self.play_sound_at(
            "minecraft:block.amethyst_block.chime",
            mob.pos,
            1.0,
            1.0,
            None,
        );
    }

    /// The player with this UUID, if online and alive: their entity ID and position.
    fn liked_player_at(&self, liked: Option<u128>) -> Option<(i32, [f64; 3])> {
        let uuid = liked?;
        self.players
            .iter()
            .find(|(_, p)| p.alive && p.profile.uuid == uuid)
            .map(|(&pid, p)| (pid, p.pos))
    }

    /// Whether a jukebox within dancing range plays a disc.
    fn jukebox_playing_near(&self, at: [f64; 3]) -> bool {
        let r = DANCE_RANGE.ceil() as i32;
        let (x, y, z) = (
            at[0].floor() as i32,
            at[1].floor() as i32,
            at[2].floor() as i32,
        );
        for dx in -r..=r {
            for dy in -r..=r {
                for dz in -r..=r {
                    let pos = BlockPos::new(x + dx, y + dy, z + dz);
                    if let Some(BlockEntity::Jukebox(j)) = self.block_entities.get(&pos) {
                        if j.signal() > 0 && distance(center(pos), at) <= DANCE_RANGE + 0.5 {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Whether the dropped item `iid` is one the allay may go for with `goal` as where
    /// it delivers, and where it is.
    fn allay_may_take(&self, iid: i32, goal: [f64; 3], a: &Allay) -> Option<[f64; 3]> {
        let item = self.items.get(&iid)?;
        (item.can_be_picked_up()
            && a.wants(&item.stack_ref())
            && distance(item.pos(), goal) <= SEEK_RANGE
            && !was_thrown(self, iid))
        .then(|| item.pos())
    }

    /// The dropped item the allay is after, with where it is: the one it picked while
    /// still worth taking, otherwise (looking over all items only once a second, or when
    /// that one went) the nearest it wants within seeking range of `goal`.
    fn item_for_allay(
        &self,
        id: i32,
        from: [f64; 3],
        goal: [f64; 3],
        a: &mut Allay,
    ) -> Option<(i32, [f64; 3])> {
        let kept = a.target_item.take();
        if let Some(iid) = kept {
            if let Some(at) = self.allay_may_take(iid, goal, a) {
                a.target_item = Some(iid);
                return Some((iid, at));
            }
        }
        let due = (self.tick + id as u64).is_multiple_of(SCAN_EVERY);
        if kept.is_none() && !due {
            return None;
        }
        let thrown = thrown_ids(self);
        let found = self
            .items
            .iter()
            .filter(|(iid, item)| {
                item.can_be_picked_up()
                    && a.wants(&item.stack_ref())
                    && distance(item.pos(), goal) <= SEEK_RANGE
                    && !thrown.is_some_and(|t| t.contains(iid))
            })
            .map(|(&iid, item)| (iid, item.pos()))
            .min_by(|x, y| distance(x.1, from).total_cmp(&distance(y.1, from)));
        a.target_item = found.map(|(iid, _)| iid);
        found
    }

    fn in_pickup_reach(&self, allay: [f64; 3], item: [f64; 3]) -> bool {
        (item[0] - allay[0]).abs() <= PICKUP_REACH
            && (item[2] - allay[2]).abs() <= PICKUP_REACH
            && (allay[1] - PICKUP_REACH..=allay[1] + PICKUP_REACH + 0.6).contains(&item[1])
    }

    /// Takes as much of the dropped stack `item_id` as fits, with the pickup animation.
    fn allay_take(&mut self, id: i32, item_id: i32, a: &mut Allay) {
        let Some(stack) = self.items.get(&item_id).map(|i| i.stack_ref()) else {
            return;
        };
        let room = if a.carried.is_empty() {
            stack.max_stack()
        } else {
            a.carried.max_stack() - a.carried.count
        };
        let taken = stack.count.min(room);
        if taken <= 0 {
            return;
        }
        self.broadcast(
            Writer::packet(clientbound::TAKE_ITEM_ENTITY)
                .varint(item_id)
                .varint(id)
                .varint(taken),
            None,
        );
        a.carried = if a.carried.is_empty() {
            stack.with_count(taken)
        } else {
            a.carried.with_count(a.carried.count + taken)
        };
        if taken >= stack.count {
            self.items.remove(&item_id);
            self.broadcast(super::items::remove_entity(item_id), None);
        } else {
            self.set_item_stack(item_id, stack.with_count(stack.count - taken));
        }
    }

    /// Throws what it carried towards `goal`, and waits a little before seeking again.
    fn allay_throw(&mut self, mob: &Mob, a: &mut Allay, goal: [f64; 3]) {
        let stack = std::mem::take(&mut a.carried);
        let at = [mob.pos[0], mob.pos[1] + 0.3, mob.pos[2]];
        let (dx, dz) = (goal[0] - at[0], goal[2] - at[2]);
        let vel = [dx * 0.1, 0.1, dz * 0.1];
        let before = next_entity_id();
        self.spawn_dropped(stack, at, vel);
        let new = self
            .items
            .iter()
            .filter(|(&iid, item)| iid > before && item.pos() == at)
            .map(|(&iid, _)| iid)
            .max();
        if let Some(iid) = new {
            thrown(self).ids.insert(iid);
        }
        let pitch = THROW_PITCHES[self.rng.below(THROW_PITCHES.len() as u32) as usize];
        self.play_sound_at(
            "minecraft:entity.allay.item_thrown",
            mob.pos,
            1.0,
            pitch,
            None,
        );
        a.delivery_cooldown = DELIVERY_COOLDOWN;
        a.target_item = None;
    }

    /// Flies an allay towards `goal` at `speed`, blocks allowing, carried along by
    /// knockback. A speed of 0 hovers in place.
    fn fly_allay(&mut self, mob: &mut Mob, goal: [f64; 3], speed: f64) {
        let d = [0, 1, 2].map(|i| goal[i] - mob.pos[i]);
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let step = speed.min(length);
        let by = if length < 1e-6 {
            [0.0; 3]
        } else {
            d.map(|v| v / length * step)
        };
        if step > 0.0 && d[0].hypot(d[2]) > 1e-3 {
            mob.yaw = yaw_towards(mob.pos, goal);
            mob.head_yaw = mob.yaw;
        }
        let (width, height) = mob.size();
        let half = width / 2.0;
        let mut pos = mob.pos;
        let mut moved = [0.0; 3];
        for axis in 0..3 {
            let push = match axis {
                0 => mob.knockback[0],
                2 => mob.knockback[1],
                _ => 0.0,
            };
            let delta = self.clip_move(pos, half, height, axis, by[axis] + push);
            pos[axis] += delta;
            moved[axis] = delta;
        }
        for k in &mut mob.knockback {
            *k *= 0.91;
        }
        mob.vel = moved;
        mob.pos = pos;
        mob.target = None;
        mob.path.clear();
    }
}

/// Faster when far behind.
fn speed_for(away: f64) -> f64 {
    if away > CATCH_UP_BEYOND {
        CATCH_UP_SPEED
    } else {
        SPEED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    const ALICE_UUID: u128 = 21;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-allays-{}-{}",
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
            uuid: ALICE_UUID,
            properties: Vec::new(),
        };
        w.players
            .insert(alice, Player::new(profile, out, [0.5, -60.0, 0.5]));
        (w, alice)
    }

    fn item(name: &str, count: i32) -> Stack {
        Stack::new(ItemInfo::id_of(name).unwrap(), count)
    }

    fn hold_in(w: &mut World, id: i32, hand: i32, name: &str, count: i32) -> usize {
        let slot = w.players[&id].hand_slot(hand);
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = if name.is_empty() {
            Stack::EMPTY
        } else {
            item(name, count)
        };
        slot
    }

    fn hold(w: &mut World, id: i32, name: &str, count: i32) -> usize {
        hold_in(w, id, 0, name, count)
    }

    fn add_allay(w: &mut World, at: [f64; 3]) -> i32 {
        let id = next_entity_id();
        let mut mob = Mob::new(MobKind::Allay, at, &mut w.rng);
        spawned(w, &mut mob, "minecraft:plains");
        w.mobs.insert(id, mob);
        id
    }

    fn state(w: &World, id: i32) -> &Allay {
        allay_of(&w.mobs[&id]).expect("allay state")
    }

    /// Runs the allay's own turns, as the mob tick does.
    fn run(w: &mut World, id: i32, ticks: u32) {
        for _ in 0..ticks {
            w.tick += 1;
            let mut mob = w.mobs.remove(&id).unwrap();
            tick(w, id, &mut mob);
            w.mobs.insert(id, mob);
        }
    }

    fn give(w: &mut World, player: i32, allay: i32, name: &str) {
        let slot = hold(w, player, name, 5);
        assert!(use_on(w, player, allay, slot, name));
    }

    fn items_named(w: &World, name: &str) -> Vec<(i32, [f64; 3], i32)> {
        w.items
            .iter()
            .filter(|(_, i)| ItemInfo::get(i.stack().item).unwrap().name == name)
            .map(|(&id, i)| (id, i.pos(), i.stack().count))
            .collect()
    }

    #[test]
    fn it_takes_a_given_item_and_gives_it_back() {
        let (mut w, alice) = world_with_player();
        let allay = add_allay(&mut w, [1.5, -59.0, 0.5]);
        // An empty hand on an empty-handed allay does nothing.
        let slot = hold(&mut w, alice, "", 0);
        assert!(!use_on(&mut w, alice, allay, slot, "minecraft:air"));
        give(&mut w, alice, allay, "minecraft:diamond");
        let a = state(&w, allay);
        assert!(a.held.is_named("minecraft:diamond") && a.held.count == 1);
        assert_eq!(a.liked, Some(ALICE_UUID));
        assert_eq!(w.players[&alice].inventory.slots[slot].count, 4);
        assert!(w.mobs[&allay].persistent);
        assert_eq!(spawn_extras(allay, &w.mobs[&allay]).len(), 1);
        // Something else is not taken while it holds the diamond.
        let slot = hold(&mut w, alice, "minecraft:stone", 1);
        assert!(!use_on(&mut w, alice, allay, slot, "minecraft:stone"));
        // An empty off hand does not take the diamond back.
        let off = hold_in(&mut w, alice, 1, "", 0);
        assert!(!use_on(&mut w, alice, allay, off, "minecraft:air"));
        assert!(w.players[&alice].inventory.slots[off].is_empty());
        assert!(state(&w, allay).held.is_named("minecraft:diamond"));
        // An empty main hand does.
        let slot = hold(&mut w, alice, "", 0);
        assert!(use_on(&mut w, alice, allay, slot, "minecraft:air"));
        assert!(w.players[&alice].inventory.slots[slot].is_named("minecraft:diamond"));
        let a = state(&w, allay);
        assert!(a.held.is_empty() && a.liked.is_none());
    }

    #[test]
    fn only_creative_players_keep_what_they_give() {
        let (mut w, alice) = world_with_player();
        let allay = add_allay(&mut w, [1.5, -59.0, 0.5]);
        w.players.get_mut(&alice).unwrap().mode = 2; // adventure
        give(&mut w, alice, allay, "minecraft:diamond");
        let slot = w.players[&alice].hand_slot(0);
        assert_eq!(w.players[&alice].inventory.slots[slot].count, 4);
        let other = add_allay(&mut w, [2.5, -59.0, 0.5]);
        w.players.get_mut(&alice).unwrap().mode = GAME_MODE_CREATIVE;
        give(&mut w, alice, other, "minecraft:diamond");
        assert_eq!(w.players[&alice].inventory.slots[slot].count, 5);
        assert!(state(&w, other).held.is_named("minecraft:diamond"));
    }

    #[test]
    fn it_collects_matching_items_and_brings_them_to_its_player() {
        let (mut w, alice) = world_with_player();
        let allay = add_allay(&mut w, [1.5, -59.0, 0.5]);
        give(&mut w, alice, allay, "minecraft:diamond");
        w.spawn_item_moving(item("minecraft:diamond", 3), [10.5, -60.0, 6.5], [0.0; 3]);
        w.spawn_item_moving(item("minecraft:diamond", 2), [9.5, -60.0, 8.5], [0.0; 3]);
        w.spawn_item_moving(item("minecraft:stone", 4), [10.5, -60.0, 7.5], [0.0; 3]);
        // Far beyond 32 blocks of the player: left alone.
        w.spawn_item_moving(item("minecraft:diamond", 1), [40.5, -60.0, 0.5], [0.0; 3]);
        let mut carried = 0;
        for _ in 0..400 {
            run(&mut w, allay, 1);
            carried = carried.max(state(&w, allay).carried.count);
            if carried > 0 && state(&w, allay).carried.is_empty() {
                break;
            }
        }
        assert_eq!(carried, 5, "both diamond stacks collected");
        assert!(state(&w, allay).carried.is_empty(), "delivered");
        assert_eq!(state(&w, allay).delivery_cooldown, DELIVERY_COOLDOWN);
        let diamonds = items_named(&w, "minecraft:diamond");
        let near: Vec<_> = diamonds
            .iter()
            .filter(|(_, at, _)| distance(*at, [0.5, -59.0, 0.5]) <= THROW_RANGE + 1.0)
            .collect();
        assert_eq!(near.len(), 1, "{diamonds:?}");
        assert_eq!(near[0].2, 5);
        assert!(was_thrown(&w, near[0].0), "it leaves what it threw");
        assert!(diamonds.iter().any(|(_, at, _)| at[0] > 39.0));
        assert_eq!(items_named(&w, "minecraft:stone").len(), 1);
        // It stays close to its player, and does not pick up what it threw.
        run(&mut w, allay, 200);
        let at = w.mobs[&allay].pos;
        assert!(distance(at, [0.5, -59.0, 0.5]) <= STAY_NEAR + 0.5, "{at:?}");
        assert!(state(&w, allay).carried.is_empty());
    }

    #[test]
    fn it_tells_potions_apart_but_not_names() {
        let mut healing = item("minecraft:potion", 1);
        healing.potion = Some(1);
        let mut other = healing;
        other.potion = Some(2);
        let a = Allay {
            held: healing,
            ..Allay::default()
        };
        assert!(a.wants(&healing));
        assert!(!a.wants(&other));
        assert!(!a.wants(&item("minecraft:potion", 1)));
        let named = Stack {
            name: Some("Mine"),
            ..healing
        };
        assert!(a.wants(&named));
    }

    #[test]
    fn it_looks_for_items_once_a_second_and_keeps_its_pick() {
        let (mut w, alice) = world_with_player();
        let allay = add_allay(&mut w, [1.5, -59.0, 0.5]);
        give(&mut w, alice, allay, "minecraft:diamond");
        w.spawn_item_moving(item("minecraft:diamond", 1), [12.5, -60.0, 0.5], [0.0; 3]);
        // Within a second it has picked the diamond.
        run(&mut w, allay, SCAN_EVERY as u32);
        let first = state(&w, allay).target_item.expect("picked an item");
        // A nearer one appears: it sticks to its pick until that one goes.
        w.spawn_item_moving(item("minecraft:diamond", 1), [5.5, -60.0, 5.5], [0.0; 3]);
        run(&mut w, allay, 3);
        assert_eq!(state(&w, allay).target_item, Some(first));
        w.items.remove(&first);
        run(&mut w, allay, 1);
        let next = state(&w, allay).target_item.expect("another one");
        assert_ne!(next, first);
    }

    #[test]
    fn a_played_note_block_takes_the_deliveries_for_30_seconds() {
        let (mut w, alice) = world_with_player();
        let note = BlockPos::new(8, -60, 0);
        let note_state = BlockInfo::by_name("minecraft:note_block")
            .unwrap()
            .default_state;
        w.set_block(note, note_state);
        let allay = add_allay(&mut w, [1.5, -59.0, 0.5]);
        // An allay without an item does not care.
        let slot = hold(&mut w, alice, "", 0);
        block_used(&mut w, alice, slot, note, note_state);
        assert_eq!(state(&w, allay).note, None);
        give(&mut w, alice, allay, "minecraft:diamond");
        let slot = hold(&mut w, alice, "", 0);
        assert!(!block_used(&mut w, alice, slot, note, note_state));
        assert_eq!(state(&w, allay).note, Some((note, NOTE_TICKS)));
        w.spawn_item_moving(item("minecraft:diamond", 2), [3.5, -60.0, 4.5], [0.0; 3]);
        let mut delivered = false;
        let mut carried = false;
        for _ in 0..300 {
            run(&mut w, allay, 1);
            carried |= !state(&w, allay).carried.is_empty();
            if carried && state(&w, allay).carried.is_empty() {
                delivered = true;
                break;
            }
        }
        assert!(delivered);
        let diamonds = items_named(&w, "minecraft:diamond");
        assert_eq!(diamonds.len(), 1);
        assert!(distance(diamonds[0].1, above(note)) <= THROW_RANGE + 0.5);
        // After 30 seconds it goes back to its player.
        run(&mut w, allay, NOTE_TICKS);
        assert_eq!(state(&w, allay).note, None);
        // A muffled note block is not heard.
        w.set_block(BlockPos::new(8, -59, 0), block::STONE);
        block_used(&mut w, alice, slot, note, note_state);
        assert_eq!(state(&w, allay).note, None);
    }

    #[test]
    fn its_player_cannot_hurt_it_and_it_drops_its_items_on_death() {
        let (mut w, alice) = world_with_player();
        let allay = add_allay(&mut w, [1.5, -59.0, 0.5]);
        give(&mut w, alice, allay, "minecraft:diamond");
        let mut mob = w.mobs.remove(&allay).unwrap();
        allay_mut(&mut mob).carried = item("minecraft:diamond", 7);
        assert_eq!(
            damage_taken(
                &mut w,
                allay,
                &mut mob,
                4.0,
                "minecraft:player_attack",
                Some(alice)
            ),
            0.0
        );
        assert_eq!(
            damage_taken(&mut w, allay, &mut mob, 4.0, "minecraft:mob_attack", None),
            4.0
        );
        // It heals over time.
        mob.health = 10.0;
        w.mobs.insert(allay, mob);
        run(&mut w, allay, 20);
        assert!((w.mobs[&allay].health - 12.0).abs() < 1e-3);
        let mut mob = w.mobs.remove(&allay).unwrap();
        mob.health = 0.0;
        hurt(&mut w, allay, &mut mob, None);
        hurt(&mut w, allay, &mut mob, None);
        let counts: Vec<i32> = items_named(&w, "minecraft:diamond")
            .iter()
            .map(|d| d.2)
            .collect();
        assert_eq!(counts.iter().sum::<i32>(), 8, "{counts:?}");
    }

    #[test]
    fn an_allay_killed_without_a_blow_still_drops_its_items() {
        let (mut w, alice) = world_with_player();
        let allay = add_allay(&mut w, [1.5, -59.0, 0.5]);
        give(&mut w, alice, allay, "minecraft:diamond");
        let mut mob = w.mobs.remove(&allay).unwrap();
        allay_mut(&mut mob).carried = item("minecraft:diamond", 3);
        // Lava, magic and the like go straight to the death.
        w.kill_mob(allay, &mut mob);
        w.mobs.insert(allay, mob);
        let before: i32 = items_named(&w, "minecraft:diamond")
            .iter()
            .map(|d| d.2)
            .sum();
        for _ in 0..DEATH_CHECK {
            w.tick += 1;
            tick_world(&mut w);
        }
        let after: i32 = items_named(&w, "minecraft:diamond")
            .iter()
            .map(|d| d.2)
            .sum();
        assert_eq!(after - before, 4);
        // Only once.
        for _ in 0..DEATH_CHECK {
            w.tick += 1;
            tick_world(&mut w);
        }
        let again: i32 = items_named(&w, "minecraft:diamond")
            .iter()
            .map(|d| d.2)
            .sum();
        assert_eq!(again, after);
    }

    #[test]
    fn a_dancing_allay_duplicates_with_an_amethyst_shard() {
        let (mut w, alice) = world_with_player();
        let jukebox = BlockPos::new(3, -60, 3);
        let jukebox_state = BlockInfo::by_name("minecraft:jukebox")
            .unwrap()
            .default_state;
        w.set_block(jukebox, jukebox_state);
        let allay = add_allay(&mut w, [3.5, -58.5, 4.5]);
        run(&mut w, allay, DANCE_CHECK as u32);
        assert!(!state(&w, allay).dancing);
        // Not dancing: the shard is simply given.
        let far = add_allay(&mut w, [12.5, -58.5, 12.5]);
        give(&mut w, alice, far, "minecraft:amethyst_shard");
        assert!(state(&w, far).held.is_named("minecraft:amethyst_shard"));
        let slot = hold(&mut w, alice, "minecraft:music_disc_cat", 1);
        assert!(w.use_jukebox(alice, slot, jukebox, jukebox_state));
        run(&mut w, allay, DANCE_CHECK as u32);
        assert!(state(&w, allay).dancing);
        assert_eq!(spawn_extras(allay, &w.mobs[&allay]).len(), 1);
        let before = w.mobs.len();
        let slot = hold(&mut w, alice, "minecraft:amethyst_shard", 2);
        assert!(use_on(
            &mut w,
            alice,
            allay,
            slot,
            "minecraft:amethyst_shard"
        ));
        assert_eq!(w.mobs.len(), before + 1, "a second allay");
        assert_eq!(w.players[&alice].inventory.slots[slot].count, 1);
        assert_eq!(state(&w, allay).duplication_cooldown, DUPLICATION_COOLDOWN);
        assert!(state(&w, allay).held.is_empty());
        let twins = w
            .mobs
            .values()
            .filter(|m| {
                m.kind == MobKind::Allay
                    && allay_of(m).is_some_and(|a| a.duplication_cooldown == DUPLICATION_COOLDOWN)
            })
            .count();
        assert_eq!(twins, 2);
        // On cooldown the shard is given instead.
        assert!(use_on(
            &mut w,
            alice,
            allay,
            slot,
            "minecraft:amethyst_shard"
        ));
        assert_eq!(w.mobs.len(), before + 1);
        assert!(state(&w, allay).held.is_named("minecraft:amethyst_shard"));
        // The jukebox stops: so does the dance.
        let now = w.block(jukebox);
        assert!(w.use_jukebox(alice, slot, jukebox, now));
        run(&mut w, allay, DANCE_CHECK as u32);
        assert!(!state(&w, allay).dancing);
    }

    #[test]
    fn allays_are_saved_as_vanilla_and_never_despawn() {
        let (mut w, alice) = world_with_player();
        let allay = add_allay(&mut w, [1.5, -59.0, 0.5]);
        give(&mut w, alice, allay, "minecraft:diamond");
        let mut mob = w.mobs.remove(&allay).unwrap();
        {
            let a = allay_mut(&mut mob);
            a.carried = item("minecraft:diamond", 9);
            a.duplication_cooldown = 1234;
        }
        assert!(!w.despawns(&mob));
        let mut nbt = Compound::new();
        write_nbt(&mob, &mut nbt);
        let main = nbt
            .compound("equipment")
            .and_then(|e| e.compound("mainhand"))
            .expect("held item saved");
        assert_eq!(main.str("id"), Some("minecraft:diamond"));
        assert_eq!(nbt.list("Inventory").map(<[Tag]>::len), Some(1));
        assert!(matches!(
            nbt.get("DuplicationCooldown"),
            Some(Tag::Long(1234))
        ));
        let mut back = Mob::new(MobKind::Allay, [0.5, -59.0, 0.5], &mut w.rng);
        read_nbt(&mut back, &nbt);
        let a = allay_of(&back).unwrap();
        assert!(a.held.is_named("minecraft:diamond") && a.held.count == 1);
        assert_eq!(a.carried.count, 9);
        assert_eq!(a.duplication_cooldown, 1234);
        assert_eq!(a.liked, Some(ALICE_UUID));
        // A bare allay holds nothing and may duplicate.
        let mut bare = Mob::new(MobKind::Allay, [0.5, -59.0, 0.5], &mut w.rng);
        read_nbt(&mut bare, &Compound::new());
        assert_eq!(*allay_of(&bare).unwrap(), Allay::default());
        assert!(spawn_extras(1, &bare).is_empty());
        let mut nbt = Compound::new();
        write_nbt(&bare, &mut nbt);
        assert!(nbt
            .compound("equipment")
            .is_some_and(|e| e.get("mainhand").is_none()));
        assert_eq!(uuid_from(&uuid_ints(u128::MAX - 5)), Some(u128::MAX - 5));
    }
}
