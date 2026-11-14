//! Leads and leash knots, following the Minecraft Wiki (Lead, and its "Leash knot"
//! section).
//!
//! - Using a lead on a leashable mob (the wiki's list; wolves, nautiluses and zombie
//!   nautiluses only while not hostile) ties it to the player; each mob takes a lead of
//!   its own, used up outside creative. Clients draw the lead from the Link Entities
//!   packet, sent again to every player who starts seeing the mob.
//! - A leashed mob stays near its holder: beyond 6 blocks (10 for a happy ghast) it
//!   walks back and is pulled along, the harder the further; beyond 12 blocks (16 for a
//!   happy ghast) the lead snaps and drops.
//! - Using any fence while holding leashed mobs within 7 blocks of it ties them to a
//!   leash knot on that fence. Using the knot ties the player's mobs to it as well, or,
//!   if the player holds none, hands all the mobs tied there to the player. Hitting the
//!   knot, or the fence going away, unties everything on it.
//! - Using a leashed mob again (held by this player or tied to a knot) takes the lead
//!   off; a mob tied to a knot is untied only with an empty hand, so it can still be fed,
//!   sheared or ridden. Using a lead on a mob someone else leads snaps that lead and
//!   leashes the mob to the player. A lead taken off or broken drops at the mob, except
//!   that taking one off in creative drops nothing. A mob that dies or vanishes drops its
//!   lead; so does one whose holder leaves or dies.
//! - A trader llama leashed by a player leaves its wandering trader and no longer
//!   despawns with it (Trader Llama, "Despawning").
//! - Saved as vanilla: the mob's `leash` tag holds its holder's UUID (`{UUID: [I; ..]}`)
//!   or, for a knot, the fence's position as an int array. Knots are saved with their
//!   chunk as `minecraft:leash_knot` entities. A mob whose holder cannot be found within
//!   5 seconds of loading drops its lead; one tied to a fence gets a knot there again,
//!   or drops its lead if the fence is gone. A trader llama saved on its wandering
//!   trader's lead follows that trader again.

use std::collections::{HashMap, HashSet};

use levitas_nbt::{Compound, Tag};
use levitas_registry::{block_has_tag, entity_type};

use super::mobs::{Mob, MobKind};
use super::saved_entities::{doubles, triple_of, uuid_ints, uuid_of};
use super::*;

/// Where the module keeps its knots in `World::kin_state`.
const STATE_KEY: &str = "leads";
/// Beyond this a leashed mob heads back to its holder (a happy ghast's is longer).
const FREE: f64 = 6.0;
const HAPPY_GHAST_FREE: f64 = 10.0;
/// Beyond this the lead snaps.
const BREAK: f64 = 12.0;
const HAPPY_GHAST_BREAK: f64 = 16.0;
/// How far from a fence a player's leashed mobs may be to be tied to it.
const TIE_RANGE: f64 = 7.0;
/// Ticks a loaded mob waits for its holder to turn up before dropping its lead.
const HOLDER_WAIT_TICKS: u32 = 100;
/// The strongest pull a stretched lead gives in one tick, in blocks per tick.
const MAX_PULL: f64 = 0.4;
/// A knot sits this high in its fence block.
const KNOT_HEIGHT: f64 = 0.375;

/// What holds a leashed mob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leash {
    /// A player, by entity ID, and their UUID (what is saved).
    Player { id: i32, uuid: u128 },
    /// A leash knot, by entity ID, on the fence at `pos`.
    Knot { id: i32, pos: BlockPos },
    /// As read from a save, until the holder is found: an entity by UUID, with the
    /// ticks waited for it so far...
    Uuid { uuid: u128, waited: u32 },
    /// ... or the fence a knot ties it to.
    Fence(BlockPos),
}

impl Leash {
    /// The holder's entity ID, once known.
    fn holder(self) -> Option<i32> {
        match self {
            Leash::Player { id, .. } | Leash::Knot { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// A leash knot on a fence.
pub struct Knot {
    pub pos: BlockPos,
    uuid: u128,
}

impl Knot {
    pub fn chunk(&self) -> (i32, i32) {
        self.pos.chunk()
    }

    fn centre(&self) -> [f64; 3] {
        let p = self.pos;
        [
            f64::from(p.x) + 0.5,
            f64::from(p.y) + KNOT_HEIGHT,
            f64::from(p.z) + 0.5,
        ]
    }

    pub fn to_nbt(&self) -> Compound {
        let p = self.pos;
        Compound::new()
            .with("id", "minecraft:leash_knot")
            .with("UUID", uuid_ints(self.uuid))
            .with("Pos", doubles(self.centre()))
            .with("Motion", doubles([0.0; 3]))
            .with("Rotation", vec![Tag::Float(0.0), Tag::Float(0.0)])
            .with("block_pos", vec![p.x, p.y, p.z])
    }

    pub fn from_nbt(nbt: &Compound) -> Option<Knot> {
        if nbt.str("id")? != "minecraft:leash_knot" {
            return None;
        }
        let pos = match nbt.int_array("block_pos") {
            Some(&[x, y, z]) => BlockPos::new(x, y, z),
            _ => {
                let at = triple_of(nbt, "Pos")?;
                BlockPos::new(
                    at[0].floor() as i32,
                    at[1].floor() as i32,
                    at[2].floor() as i32,
                )
            }
        };
        let uuid = nbt.int_array("UUID").and_then(uuid_of).unwrap_or(0);
        Some(Knot { pos, uuid })
    }
}

/// The knots in the world, and the leashed mobs with where each was seen last (the
/// index the tick walks, and how it tells that one vanished).
#[derive(Default)]
struct Leads {
    knots: HashMap<i32, Knot>,
    leashed: HashMap<i32, [f64; 3]>,
    /// The goal each mob pulled by its lead last had its path worked out for.
    goals: HashMap<i32, BlockPos>,
}

fn state(w: &World) -> Option<&Leads> {
    w.kin_state
        .get(STATE_KEY)
        .and_then(|s| s.downcast_ref::<Leads>())
}

fn state_mut(w: &mut World) -> &mut Leads {
    let entry = w
        .kin_state
        .entry(STATE_KEY)
        .or_insert_with(|| Box::new(Leads::default()));
    if !entry.is::<Leads>() {
        *entry = Box::new(Leads::default());
    }
    entry
        .downcast_mut::<Leads>()
        .expect("just made sure it is one")
}

/// Whether a lead can be put on this mob (the wiki's list).
pub fn leashable(mob: &Mob) -> bool {
    use MobKind::*;
    match mob.kind {
        Allay | Armadillo | Axolotl | Bee | Camel | CamelHusk | Cat | Chicken | CopperGolem
        | Cow | Dolphin | Donkey | Fox | Frog | GlowSquid | Goat | HappyGhast | Hoglin | Horse
        | IronGolem | Llama | Mooshroom | Mule | Ocelot | Parrot | Pig | PolarBear | Rabbit
        | Sheep | SkeletonHorse | Sniffer | SnowGolem | Squid | Strider | SulfurCube
        | TraderLlama | Zoglin | ZombieHorse => true,
        // Only while not hostile.
        Wolf | Nautilus | ZombieNautilus => mob.anger == 0 && mob.hunting.is_none(),
        _ => false,
    }
}

fn reach_of(kind: MobKind) -> (f64, f64) {
    if kind == MobKind::HappyGhast {
        (HAPPY_GHAST_FREE, HAPPY_GHAST_BREAK)
    } else {
        (FREE, BREAK)
    }
}

/// Whether a player uses up leads, and gets them back when they come off: everyone but
/// creative players (adventure ones too).
fn pays(p: &Player) -> bool {
    p.mode != GAME_MODE_CREATIVE
}

fn holds_lead(stack: Stack) -> bool {
    ItemInfo::get(stack.item).is_some_and(|i| i.name == "minecraft:lead")
}

/// How far apart two block positions are, block by block.
fn blocks_apart(a: BlockPos, b: BlockPos) -> i32 {
    (a.x - b.x).abs() + (a.y - b.y).abs() + (a.z - b.z).abs()
}

fn is_fence(state: u16) -> bool {
    block_has_tag(state, "minecraft:fences")
}

/// The Link Entities packet: `mob` held by `holder`, or let go (-1).
fn link(mob: i32, holder: i32) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_LINK)
        .i32(mob)
        .i32(holder)
}

fn knot_spawn(id: i32, knot: &Knot) -> Writer {
    let at = knot.centre();
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(knot.uuid)
        .varint(entity_type::LEASH_KNOT)
        .f64(at[0])
        .f64(at[1])
        .f64(at[2])
        .lp_vec3([0.0, 0.0, 0.0])
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(0)
}

/// Entity data showing a new viewer what holds the mob (sent after its spawn).
pub(super) fn spawn_extras(id: i32, mob: &Mob) -> Vec<Writer> {
    mob.leash
        .and_then(Leash::holder)
        .map(|holder| link(id, holder))
        .into_iter()
        .collect()
}

pub(super) fn write_nbt(mob: &Mob, nbt: &mut Compound) {
    match mob.leash {
        Some(Leash::Player { uuid, .. } | Leash::Uuid { uuid, .. }) => {
            nbt.insert("leash", Compound::new().with("UUID", uuid_ints(uuid)));
        }
        Some(Leash::Knot { pos, .. } | Leash::Fence(pos)) => {
            nbt.insert("leash", vec![pos.x, pos.y, pos.z]);
        }
        None => {}
    }
}

pub(super) fn read_nbt(mob: &mut Mob, nbt: &Compound) {
    mob.leash = if let Some(&[x, y, z]) = nbt.int_array("leash") {
        Some(Leash::Fence(BlockPos::new(x, y, z)))
    } else {
        nbt.compound("leash")
            .and_then(|c| c.int_array("UUID"))
            .and_then(uuid_of)
            .map(|uuid| Leash::Uuid { uuid, waited: 0 })
    };
}

impl World {
    /// Adds a mob that came with a lead (loaded from its chunk) to those the tick looks
    /// after.
    pub(super) fn track_leash(&mut self, mob: i32) {
        if let Some(pos) = self
            .mobs
            .get(&mob)
            .filter(|m| m.leash.is_some())
            .map(|m| m.pos)
        {
            state_mut(self).leashed.insert(mob, pos);
        }
    }

    /// Puts `mob` on a lead held by `holder`, telling its viewers.
    fn leash_mob(&mut self, mob: i32, holder: Leash) {
        let Some(m) = self.mobs.get_mut(&mob) else {
            return;
        };
        m.leash = Some(holder);
        if m.kind == MobKind::TraderLlama && holder.holder().is_some() {
            // Taken from its trader: it stays (Trader Llama, "Despawning").
            m.leader = None;
            m.persistent = true;
        }
        let pos = m.pos;
        state_mut(self).leashed.insert(mob, pos);
        if let Some(h) = holder.holder() {
            let m = self.mobs.get(&mob).expect("checked above");
            let viewers: Vec<i32> = m.viewers.iter().copied().collect();
            for v in viewers {
                self.send(v, link(mob, h));
            }
        }
    }

    /// Takes the lead off `mob`, dropping it as an item if `drop`.
    fn unleash_mob(&mut self, mob: i32, drop: bool, sound: &str) {
        let Some(m) = self.mobs.get_mut(&mob) else {
            return;
        };
        if m.leash.take().is_none() {
            return;
        }
        m.leash_pull = false;
        let pos = m.pos;
        let viewers: Vec<i32> = m.viewers.iter().copied().collect();
        let leads = state_mut(self);
        leads.leashed.remove(&mob);
        leads.goals.remove(&mob);
        for v in viewers {
            self.send(v, link(mob, -1));
        }
        self.play_sound_at(sound, pos, 1.0, 1.0, None);
        if drop {
            self.drop_lead(pos);
        }
    }

    fn drop_lead(&mut self, at: [f64; 3]) {
        if let Some(lead) = ItemInfo::id_of("minecraft:lead") {
            self.scatter_item([at[0], at[1] + 0.5, at[2]], Stack::new(lead, 1));
        }
    }

    /// The mobs `holder` holds directly.
    fn held_by(&self, holder: i32) -> Vec<i32> {
        let Some(leads) = state(self) else {
            return Vec::new();
        };
        leads
            .leashed
            .keys()
            .copied()
            .filter(|id| {
                self.mobs
                    .get(id)
                    .is_some_and(|m| m.leash.and_then(Leash::holder) == Some(holder))
            })
            .collect()
    }

    /// A player uses `slot`'s item on a mob: leashing, unleashing or taking over its
    /// lead. Returns whether a lead was involved (nothing else happens then).
    pub(super) fn use_lead_on_mob(&mut self, player: i32, target: i32, slot: usize) -> bool {
        let (Some(p), Some(mob)) = (self.players.get(&player), self.mobs.get(&target)) else {
            return false;
        };
        let stack = p.inventory.slots[slot];
        let holding_lead = holds_lead(stack);
        let drop = pays(p);
        let uuid = p.profile.uuid;
        let to_player = Leash::Player { id: player, uuid };
        match mob.leash {
            Some(Leash::Player { id, .. }) if id == player => {
                self.unleash_mob(target, drop, "minecraft:item.lead.untied");
                true
            }
            // With something in hand it is fed, sheared, ridden ... as usual.
            Some(Leash::Knot { .. }) if stack.is_empty() => {
                self.unleash_mob(target, drop, "minecraft:item.lead.untied");
                true
            }
            Some(_) if holding_lead => {
                // The old lead snaps; this player's takes its place.
                self.unleash_mob(target, true, "minecraft:item.lead.break");
                self.take_lead(player, slot);
                self.leash_mob(target, to_player);
                let at = self.mobs[&target].pos;
                self.play_sound_at("minecraft:item.lead.tied", at, 1.0, 1.0, None);
                true
            }
            Some(_) => false,
            None if holding_lead && leashable(mob) => {
                self.take_lead(player, slot);
                self.leash_mob(target, to_player);
                let at = self.mobs[&target].pos;
                self.play_sound_at("minecraft:item.lead.tied", at, 1.0, 1.0, None);
                true
            }
            None => false,
        }
    }

    fn take_lead(&mut self, player: i32, slot: usize) {
        let Some(p) = self.players.get_mut(&player) else {
            return;
        };
        if pays(p) {
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(player, update);
        }
    }

    /// The knot on the fence at `pos`, made if there is none.
    fn knot_at(&mut self, pos: BlockPos) -> i32 {
        if let Some((&id, _)) = state(self).and_then(|s| s.knots.iter().find(|(_, k)| k.pos == pos))
        {
            return id;
        }
        let uuid = (u128::from(self.rng.next_u64()) << 64) | u128::from(self.rng.next_u64());
        self.add_knot(Knot { pos, uuid })
    }

    fn add_knot(&mut self, knot: Knot) -> i32 {
        let id = next_entity_id();
        self.broadcast(knot_spawn(id, &knot), None);
        self.mob_chunks.insert(knot.chunk());
        state_mut(self).knots.insert(id, knot);
        id
    }

    fn remove_knot(&mut self, id: i32) -> Option<Knot> {
        let knot = state_mut(self).knots.remove(&id)?;
        self.broadcast(items::remove_entity(id), None);
        Some(knot)
    }

    /// Ties the mobs `player` leads within reach of `pos` to the knot there. Returns
    /// whether there were any.
    fn tie_to_fence(&mut self, player: i32, pos: BlockPos) -> bool {
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        let mobs: Vec<i32> = self
            .held_by(player)
            .into_iter()
            .filter(|id| {
                let at = self.mobs[id].pos;
                (0..3).map(|i| (at[i] - centre[i]).powi(2)).sum::<f64>() <= TIE_RANGE * TIE_RANGE
            })
            .collect();
        if mobs.is_empty() {
            return false;
        }
        let knot = self.knot_at(pos);
        for mob in mobs {
            self.leash_mob(mob, Leash::Knot { id: knot, pos });
        }
        self.play_sound_at("minecraft:item.lead.tied", centre, 1.0, 1.0, None);
        true
    }

    /// A player uses a block: on a fence, their leashed mobs are tied to it. Returns
    /// whether that happened.
    pub(super) fn use_fence(&mut self, player: i32, pos: BlockPos, state: u16) -> bool {
        // Sneaking with something in hand uses the item instead (a lead still ties).
        let uses = self.players.get(&player).is_some_and(|p| {
            let stack = p.inventory.slots[p.hand_slot(0)];
            p.alive && (!p.sneaking || stack.is_empty() || holds_lead(stack))
        });
        uses && is_fence(state) && self.tie_to_fence(player, pos)
    }

    /// A player uses an entity: a knot takes the player's mobs, or gives its own to a
    /// player who leads none. Returns whether `target` is a knot.
    pub(super) fn use_knot(&mut self, player: i32, target: i32) -> bool {
        let Some(knot) = state(self).and_then(|s| s.knots.get(&target)) else {
            return false;
        };
        let pos = knot.pos;
        let near = self
            .players
            .get(&player)
            .is_some_and(|p| super::hanging::reaches(p, knot.centre()));
        if !near {
            return true;
        }
        if self.tie_to_fence(player, pos) {
            return true;
        }
        let uuid = self.players[&player].profile.uuid;
        let tied = self.held_by(target);
        for &mob in &tied {
            self.leash_mob(mob, Leash::Player { id: player, uuid });
        }
        if !tied.is_empty() {
            let at = self.players[&player].pos;
            self.play_sound_at("minecraft:item.lead.untied", at, 1.0, 1.0, None);
        }
        self.remove_knot(target);
        true
    }

    /// A player hits an entity: a knot unties all its mobs and goes. Returns whether
    /// `target` is a knot.
    pub(super) fn hit_knot(&mut self, attacker: i32, target: i32) -> bool {
        let Some(centre) = state(self).and_then(|s| s.knots.get(&target).map(Knot::centre)) else {
            return false;
        };
        let Some(p) = self
            .players
            .get(&attacker)
            .filter(|p| super::hanging::reaches(p, centre))
        else {
            return true; // out of reach, or dead
        };
        let drop = pays(p);
        self.break_knot(target, drop);
        true
    }

    fn break_knot(&mut self, id: i32, drop: bool) {
        for mob in self.held_by(id) {
            self.unleash_mob(mob, drop, "minecraft:item.lead.untied");
        }
        if let Some(knot) = self.remove_knot(id) {
            self.play_sound_at("minecraft:item.lead.untied", knot.centre(), 1.0, 1.0, None);
        }
    }

    /// Once a tick: knots lose their fences, loaded leads find their holders, and
    /// leashed mobs are kept near what holds them. Only the leashed mobs are looked at.
    pub(super) fn tick_leads(&mut self) {
        if state(self).is_none_or(|s| s.leashed.is_empty() && s.knots.is_empty()) {
            return; // nothing on a lead and no knots
        }
        // Knots whose fence is gone break.
        let fenceless: Vec<i32> = state(self)
            .map(|s| {
                s.knots
                    .iter()
                    .filter(|(_, k)| self.loaded_state(k.pos).is_some_and(|s| !is_fence(s)))
                    .map(|(&id, _)| id)
                    .collect()
            })
            .unwrap_or_default();
        for id in fenceless {
            self.break_knot(id, true);
        }

        let leashed: Vec<(i32, [f64; 3])> = state(self)
            .map(|s| s.leashed.iter().map(|(&id, &at)| (id, at)).collect())
            .unwrap_or_default();
        for (id, at) in leashed {
            let Some(m) = self.mobs.get(&id) else {
                // Gone without being unloaded (into a hive, converted away, ...): the
                // lead drops where it was. One unloaded with its chunk comes back with it.
                let leads = state_mut(self);
                leads.leashed.remove(&id);
                leads.goals.remove(&id);
                let chunk = (
                    (at[0].floor() as i32).div_euclid(16),
                    (at[2].floor() as i32).div_euclid(16),
                );
                if self.chunks.contains_key(&chunk) {
                    self.drop_lead(at);
                }
                continue;
            };
            if m.leash.is_none() {
                state_mut(self).leashed.remove(&id);
                continue;
            }
            if self.chunks.contains_key(&m.chunk()) {
                self.tick_leashed(id);
            }
            if let Some(pos) = self
                .mobs
                .get(&id)
                .filter(|m| m.leash.is_some())
                .map(|m| m.pos)
            {
                state_mut(self).leashed.insert(id, pos);
            }
        }

        // Knots with nothing tied to them go.
        let mut used = HashSet::new();
        let mut fences = HashSet::new();
        for id in state(self).into_iter().flat_map(|s| s.leashed.keys()) {
            match self.mobs.get(id).and_then(|m| m.leash) {
                Some(Leash::Knot { id: knot, .. }) => {
                    used.insert(knot);
                }
                Some(Leash::Fence(pos)) => {
                    fences.insert(pos);
                }
                _ => {}
            }
        }
        let empty: Vec<i32> = state(self)
            .map(|s| {
                s.knots
                    .iter()
                    .filter(|&(id, k)| {
                        self.chunks.contains_key(&k.chunk())
                            && !used.contains(id)
                            && !fences.contains(&k.pos)
                    })
                    .map(|(&id, _)| id)
                    .collect()
            })
            .unwrap_or_default();
        for id in empty {
            self.remove_knot(id);
        }
    }

    fn tick_leashed(&mut self, id: i32) {
        let Some(mob) = self.mobs.get(&id) else {
            return;
        };
        let Some(leash) = mob.leash else {
            return;
        };
        if !mob.alive() {
            self.unleash_mob(id, true, "minecraft:item.lead.untied");
            return;
        }
        // Holders of loaded leads.
        let leash = match leash {
            Leash::Uuid { uuid, waited } => {
                let found = self
                    .players
                    .iter()
                    .find(|(_, p)| p.alive && p.profile.uuid == uuid)
                    .map(|(&pid, _)| pid);
                // A mob holder (looked for now and then: it takes a walk over all mobs).
                let by_mob = if found.is_none() && waited.is_multiple_of(20) {
                    self.mobs
                        .iter()
                        .find(|(_, m)| m.uuid == uuid && m.alive())
                        .map(|(&hid, m)| (hid, m.kind))
                } else {
                    None
                };
                match (found, by_mob) {
                    (Some(pid), _) => {
                        let holder = Leash::Player { id: pid, uuid };
                        self.leash_mob(id, holder);
                        holder
                    }
                    (None, Some((trader, MobKind::WanderingTrader)))
                        if mob.kind == MobKind::TraderLlama =>
                    {
                        // Back with its trader, which leads it as Levitas's traders do.
                        let m = self.mobs.get_mut(&id).expect("checked above");
                        m.leash = None;
                        m.leader = Some(trader);
                        state_mut(self).leashed.remove(&id);
                        return;
                    }
                    // Levitas has no mobs leading mobs: such a lead snaps too.
                    (None, Some(_)) => {
                        self.unleash_mob(id, true, "minecraft:item.lead.break");
                        return;
                    }
                    (None, None) if waited >= HOLDER_WAIT_TICKS => {
                        self.unleash_mob(id, true, "minecraft:item.lead.break");
                        return;
                    }
                    (None, None) => {
                        let m = self.mobs.get_mut(&id).expect("checked above");
                        m.leash = Some(Leash::Uuid {
                            uuid,
                            waited: waited + 1,
                        });
                        return;
                    }
                }
            }
            Leash::Fence(pos) => match self.loaded_state(pos) {
                None => return, // its chunk is not loaded yet
                Some(s) if is_fence(s) => {
                    let knot = self.knot_at(pos);
                    let holder = Leash::Knot { id: knot, pos };
                    self.leash_mob(id, holder);
                    holder
                }
                Some(_) => {
                    self.unleash_mob(id, true, "minecraft:item.lead.break");
                    return;
                }
            },
            held => held,
        };
        // Where the lead is held, and the block to walk back to (the holder's feet).
        let at = match leash {
            Leash::Player { id: pid, .. } => self.players.get(&pid).filter(|p| p.alive).map(|p| {
                let feet = BlockPos::new(
                    p.pos[0].floor() as i32,
                    p.pos[1].floor() as i32,
                    p.pos[2].floor() as i32,
                );
                ([p.pos[0], p.pos[1] + 1.0, p.pos[2]], feet)
            }),
            Leash::Knot { id: knot, pos } => state(self)
                .and_then(|s| s.knots.get(&knot))
                .map(|k| (k.centre(), pos)),
            _ => None,
        };
        let Some((at, feet)) = at else {
            // The holder left, died or went.
            self.unleash_mob(id, true, "minecraft:item.lead.break");
            return;
        };
        let mut mob = self.mobs.remove(&id).expect("checked above");
        let (free, snap) = reach_of(mob.kind);
        let d = [0, 1, 2].map(|i| at[i] - mob.pos[i]);
        let distance = d.iter().map(|v| v * v).sum::<f64>().sqrt();
        if distance > snap {
            self.mobs.insert(id, mob);
            self.unleash_mob(id, true, "minecraft:item.lead.break");
            return;
        }
        // Wandering stays within the lead.
        if let Some((t, _)) = mob.target {
            if (t[0] - at[0]).hypot(t[1] - at[2]) > free {
                mob.target = None;
                mob.path.clear();
            }
        }
        if distance > free {
            // Back towards the holder, and pulled the harder the more the lead stretches.
            // The lead steers it then (its own pathing waits), working its path out again
            // only now and then or when the holder has moved on.
            mob.leash_pull = true;
            if mob.kind.flies() {
                mob.fly_goal = Some(at);
            } else {
                let goals = &mut state_mut(self).goals;
                let search = goals.get(&id).is_none_or(|&g| blocks_apart(g, feet) > 2);
                if search {
                    goals.insert(id, feet);
                }
                self.steer_along(&mut mob, feet, search);
            }
            let pull = ((distance - free) * 0.05).min(MAX_PULL) / distance;
            mob.push([d[0] * pull, 0.0, d[2] * pull]);
            if d[1] > 1.0 {
                mob.vel[1] = mob.vel[1].max(d[1] * pull);
            }
        } else if mob.leash_pull {
            // Back within the lead: its own ways again.
            mob.leash_pull = false;
            mob.path.clear();
            state_mut(self).goals.remove(&id);
        }
        self.mobs.insert(id, mob);
    }

    /// What players joining are sent: the knots.
    pub(super) fn knot_packets(&self) -> Vec<Writer> {
        state(self)
            .map(|s| s.knots.iter().map(|(&id, k)| knot_spawn(id, k)).collect())
            .unwrap_or_default()
    }

    /// The saved forms of the knots in chunk (`cx`, `cz`).
    pub(super) fn saved_knots(&self, cx: i32, cz: i32) -> Vec<Compound> {
        state(self)
            .map(|s| {
                s.knots
                    .values()
                    .filter(|k| k.chunk() == (cx, cz))
                    .map(Knot::to_nbt)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Chunks with knots in them.
    pub(super) fn knot_chunks(&self) -> Vec<(i32, i32)> {
        state(self)
            .map(|s| s.knots.values().map(Knot::chunk).collect())
            .unwrap_or_default()
    }

    /// Takes the knots of an unloading chunk out of the world: mobs still about that
    /// are tied to them wait for the fence to be loaded again.
    pub(super) fn unload_knots(&mut self, cx: i32, cz: i32) -> bool {
        let gone: Vec<(i32, BlockPos)> = state(self)
            .map(|s| {
                s.knots
                    .iter()
                    .filter(|(_, k)| k.chunk() == (cx, cz))
                    .map(|(&id, k)| (id, k.pos))
                    .collect()
            })
            .unwrap_or_default();
        for &(id, pos) in &gone {
            for mob in self.held_by(id) {
                if let Some(m) = self.mobs.get_mut(&mob) {
                    m.leash = Some(Leash::Fence(pos));
                }
            }
            self.remove_knot(id);
        }
        !gone.is_empty()
    }

    /// Brings back a knot from a loaded chunk. Returns whether `nbt` was one.
    pub(super) fn load_knot(&mut self, nbt: &Compound) -> bool {
        let Some(knot) = Knot::from_nbt(nbt) else {
            return false;
        };
        let taken = state(self).is_some_and(|s| s.knots.values().any(|k| k.pos == knot.pos));
        if !taken {
            self.add_knot(knot);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world_with_player() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-leads-{}-{}",
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
        w.players
            .insert(alice, Player::new(profile, out, [0.5, -60.0, 0.5]));
        (w, alice)
    }

    fn hold(w: &mut World, id: i32, item: &str, count: i32) -> usize {
        let p = w.players.get_mut(&id).unwrap();
        let slot = p.hand_slot(0);
        p.inventory.slots[slot] = Stack::new(ItemInfo::id_of(item).unwrap(), count);
        slot
    }

    fn cow(w: &mut World, at: [f64; 3]) -> i32 {
        let id = next_entity_id();
        let mob = Mob::new(MobKind::Cow, at, &mut w.rng);
        w.mobs.insert(id, mob);
        id
    }

    fn leads_on_ground(w: &World) -> i32 {
        let lead = ItemInfo::id_of("minecraft:lead").unwrap();
        w.items
            .values()
            .filter(|i| i.stack().item == lead)
            .map(|i| i.stack().count)
            .sum()
    }

    fn fence(w: &mut World, pos: BlockPos) {
        let state = BlockInfo::by_name("minecraft:oak_fence")
            .unwrap()
            .default_state;
        w.set_block(pos, state);
    }

    #[test]
    fn a_lead_leashes_a_cow_and_using_it_again_drops_the_lead() {
        let (mut w, alice) = world_with_player();
        let id = cow(&mut w, [2.5, -60.0, 0.5]);
        let slot = hold(&mut w, alice, "minecraft:lead", 2);
        assert!(w.use_lead_on_mob(alice, id, slot));
        assert_eq!(
            w.mobs[&id].leash,
            Some(Leash::Player {
                id: alice,
                uuid: 11
            })
        );
        assert_eq!(w.players[&alice].inventory.slots[slot].count, 1);
        assert_eq!(spawn_extras(id, &w.mobs[&id]).len(), 1);
        // Again: the lead comes off and drops.
        assert!(w.use_lead_on_mob(alice, id, slot));
        assert!(w.mobs[&id].leash.is_none());
        assert_eq!(leads_on_ground(&w), 1);
        assert!(spawn_extras(id, &w.mobs[&id]).is_empty());
    }

    #[test]
    fn only_leashable_mobs_take_a_lead() {
        let (mut w, alice) = world_with_player();
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        let id = next_entity_id();
        let zombie = Mob::new(MobKind::Zombie, [2.5, -60.0, 0.5], &mut w.rng);
        w.mobs.insert(id, zombie);
        assert!(!w.use_lead_on_mob(alice, id, slot));
        assert!(w.mobs[&id].leash.is_none());
        let mut wolf = Mob::new(MobKind::Wolf, [0.5, -60.0, 0.5], &mut w.rng);
        assert!(leashable(&wolf));
        wolf.anger = 100;
        assert!(!leashable(&wolf), "an angry wolf cannot be leashed");
    }

    #[test]
    fn leashed_mobs_are_pulled_back_and_the_lead_snaps_when_too_far() {
        let (mut w, alice) = world_with_player();
        let id = cow(&mut w, [9.5, -60.0, 0.5]);
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        w.use_lead_on_mob(alice, id, slot);
        // Nine blocks off: heading back, and pulled.
        w.tick_leads();
        let mob = &w.mobs[&id];
        assert!(mob.target.is_some_and(|(t, _)| t[0] < 9.5));
        assert!(mob.knockback[0] < 0.0, "pulled towards the holder");
        assert!(mob.leash.is_some());
        // Beyond twelve: snapped, the lead on the ground.
        w.mobs.get_mut(&id).unwrap().pos = [14.5, -60.0, 0.5];
        w.tick_leads();
        assert!(w.mobs[&id].leash.is_none());
        assert_eq!(leads_on_ground(&w), 1);
    }

    #[test]
    fn leashed_mobs_are_tied_to_a_fence_and_untied_from_the_knot() {
        let (mut w, alice) = world_with_player();
        let post = BlockPos::new(2, -60, 2);
        fence(&mut w, post);
        let a = cow(&mut w, [3.5, -60.0, 0.5]);
        let b = cow(&mut w, [0.5, -60.0, 3.5]);
        let slot = hold(&mut w, alice, "minecraft:lead", 2);
        w.use_lead_on_mob(alice, a, slot);
        w.use_lead_on_mob(alice, b, slot);
        let fence_state = w.block(post);
        assert!(w.use_fence(alice, post, fence_state));
        let knot = *state(&w).unwrap().knots.keys().next().unwrap();
        for m in [a, b] {
            assert_eq!(
                w.mobs[&m].leash,
                Some(Leash::Knot {
                    id: knot,
                    pos: post
                })
            );
        }
        // Using the knot with no mobs in hand gives them back to the player.
        assert!(w.use_knot(alice, knot));
        assert!(state(&w).unwrap().knots.is_empty());
        assert!(matches!(w.mobs[&a].leash, Some(Leash::Player { id, .. }) if id == alice));
        // Tied again, then the fence is broken: the knot goes and both leads drop.
        w.use_fence(alice, post, fence_state);
        assert_eq!(state(&w).unwrap().knots.len(), 1);
        w.set_block(post, levitas_registry::block::AIR);
        w.tick_leads();
        assert!(state(&w).unwrap().knots.is_empty());
        assert!(w.mobs[&a].leash.is_none() && w.mobs[&b].leash.is_none());
        assert_eq!(leads_on_ground(&w), 2);
    }

    #[test]
    fn hitting_a_knot_unties_its_mobs() {
        let (mut w, alice) = world_with_player();
        let post = BlockPos::new(2, -60, 2);
        fence(&mut w, post);
        let a = cow(&mut w, [3.5, -60.0, 0.5]);
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        w.use_lead_on_mob(alice, a, slot);
        let fence_state = w.block(post);
        w.use_fence(alice, post, fence_state);
        let knot = *state(&w).unwrap().knots.keys().next().unwrap();
        assert!(w.hit_knot(alice, knot));
        assert!(w.mobs[&a].leash.is_none());
        assert_eq!(leads_on_ground(&w), 1);
        assert!(state(&w).unwrap().knots.is_empty());
    }

    #[test]
    fn a_leashed_mob_that_vanishes_drops_its_lead() {
        let (mut w, alice) = world_with_player();
        let id = cow(&mut w, [2.5, -60.0, 0.5]);
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        w.use_lead_on_mob(alice, id, slot);
        w.tick_leads();
        // Gone without its chunk unloading (as into a hive).
        w.mobs.remove(&id);
        w.tick_leads();
        assert_eq!(leads_on_ground(&w), 1);
    }

    #[test]
    fn leashes_are_saved_as_vanilla() {
        let (mut w, _) = world_with_player();
        let mut mob = Mob::new(MobKind::Pig, [0.5, -60.0, 0.5], &mut w.rng);
        mob.leash = Some(Leash::Player { id: 5, uuid: 11 });
        let nbt = mob.to_nbt();
        let saved = nbt.compound("leash").and_then(|c| c.int_array("UUID"));
        assert_eq!(saved.and_then(uuid_of), Some(11));
        let back = Mob::read(&nbt, &mut w.rng).unwrap();
        assert_eq!(
            back.leash,
            Some(Leash::Uuid {
                uuid: 11,
                waited: 0
            })
        );
        mob.leash = Some(Leash::Knot {
            id: 3,
            pos: BlockPos::new(1, 2, 3),
        });
        let nbt = mob.to_nbt();
        assert_eq!(nbt.int_array("leash"), Some(&[1, 2, 3][..]));
        let back = Mob::read(&nbt, &mut w.rng).unwrap();
        assert_eq!(back.leash, Some(Leash::Fence(BlockPos::new(1, 2, 3))));
        // Unleashed, a kept copy does not bring the old leash back.
        let mut back = back;
        back.leash = None;
        assert!(back.to_nbt().get("leash").is_none());
    }

    #[test]
    fn loaded_leashes_find_their_holders_or_drop() {
        let (mut w, alice) = world_with_player();
        // Tied to a fence that is there: a knot appears on it.
        let post = BlockPos::new(2, -60, 2);
        fence(&mut w, post);
        let a = cow(&mut w, [3.5, -60.0, 0.5]);
        w.mobs.get_mut(&a).unwrap().leash = Some(Leash::Fence(post));
        // Held by Alice, who is online.
        let b = cow(&mut w, [1.5, -60.0, 0.5]);
        w.mobs.get_mut(&b).unwrap().leash = Some(Leash::Uuid {
            uuid: 11,
            waited: 0,
        });
        // Held by someone who never comes.
        let c = cow(&mut w, [0.5, -60.0, 1.5]);
        w.mobs.get_mut(&c).unwrap().leash = Some(Leash::Uuid {
            uuid: 99,
            waited: 0,
        });
        for id in [a, b, c] {
            w.track_leash(id);
        }
        w.tick_leads();
        assert!(matches!(w.mobs[&a].leash, Some(Leash::Knot { pos, .. }) if pos == post));
        assert_eq!(state(&w).unwrap().knots.len(), 1);
        assert!(matches!(w.mobs[&b].leash, Some(Leash::Player { id, .. }) if id == alice));
        for _ in 0..=HOLDER_WAIT_TICKS {
            w.tick_leads();
        }
        assert!(w.mobs[&c].leash.is_none());
        assert_eq!(leads_on_ground(&w), 1);
    }

    #[test]
    fn knots_are_saved_with_their_chunk() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-knots-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let new_world = || {
            World::new(
                dir.clone(),
                NewWorld {
                    kind: WorldKind::Flat,
                    seed: Some(0),
                },
            )
            .0
        };
        let mut w = new_world();
        w.chunk(0, 0);
        let post = BlockPos::new(2, -60, 2);
        fence(&mut w, post);
        let id = cow(&mut w, [3.5, -60.0, 0.5]);
        let knot = w.knot_at(post);
        w.leash_mob(
            id,
            Leash::Knot {
                id: knot,
                pos: post,
            },
        );
        w.save_all();

        let mut w = new_world();
        w.chunk(0, 0);
        let knots: Vec<BlockPos> = state(&w)
            .map(|s| s.knots.values().map(|k| k.pos).collect())
            .unwrap_or_default();
        assert_eq!(knots, vec![post]);
        let cow = w.mobs.values().find(|m| m.kind == MobKind::Cow).unwrap();
        assert_eq!(cow.leash, Some(Leash::Fence(post)));
        w.tick_leads();
        let cow = w.mobs.values().find(|m| m.kind == MobKind::Cow).unwrap();
        assert!(matches!(cow.leash, Some(Leash::Knot { pos, .. }) if pos == post));
        assert_eq!(
            state(&w).unwrap().knots.len(),
            1,
            "the saved knot, not a new one"
        );
    }

    #[test]
    fn a_pulled_mob_does_not_search_again_every_tick() {
        let (mut w, alice) = world_with_player();
        let id = cow(&mut w, [7.5, -60.0, 0.5]);
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        w.use_lead_on_mob(alice, id, slot);
        // The holder up in the air, where no path leads.
        w.players.get_mut(&alice).unwrap().pos = [0.5, -55.0, 0.5];
        w.tick_leads();
        assert!(w.mobs[&id].leash_pull);
        let first = w.mobs[&id].path_ticks;
        w.tick_leads();
        assert_eq!(w.mobs[&id].path_ticks, first - 1, "no new search");
        // Its own AI leaves the steering to the lead meanwhile.
        let mut mob = w.mobs.remove(&id).unwrap();
        let target = mob.target;
        w.follow_path(&mut mob, BlockPos::new(20, -60, 20));
        assert_eq!(mob.target, target);
        w.mobs.insert(id, mob);
        // Back within the lead: its own ways again.
        w.players.get_mut(&alice).unwrap().pos = [6.5, -60.0, 0.5];
        w.tick_leads();
        assert!(!w.mobs[&id].leash_pull);
    }

    #[test]
    fn adventure_players_use_up_leads_too() {
        let (mut w, alice) = world_with_player();
        w.players.get_mut(&alice).unwrap().mode = 2;
        let id = cow(&mut w, [2.5, -60.0, 0.5]);
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        w.use_lead_on_mob(alice, id, slot);
        assert!(w.players[&alice].inventory.slots[slot].is_empty());
        // Taking it off gives it back.
        w.use_lead_on_mob(alice, id, slot);
        assert_eq!(leads_on_ground(&w), 1);
        // A creative player neither uses one up nor gets one.
        w.players.get_mut(&alice).unwrap().mode = GAME_MODE_CREATIVE;
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        w.use_lead_on_mob(alice, id, slot);
        assert_eq!(w.players[&alice].inventory.slots[slot].count, 1);
        w.use_lead_on_mob(alice, id, slot);
        assert_eq!(leads_on_ground(&w), 1);
    }

    #[test]
    fn a_mob_tied_to_a_knot_is_untied_only_with_an_empty_hand() {
        let (mut w, alice) = world_with_player();
        let post = BlockPos::new(2, -60, 2);
        fence(&mut w, post);
        let a = cow(&mut w, [3.5, -60.0, 0.5]);
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        w.use_lead_on_mob(alice, a, slot);
        let fence_state = w.block(post);
        w.use_fence(alice, post, fence_state);
        let slot = hold(&mut w, alice, "minecraft:wheat", 1);
        assert!(!w.use_lead_on_mob(alice, a, slot), "fed as usual");
        assert!(matches!(w.mobs[&a].leash, Some(Leash::Knot { .. })));
        w.players.get_mut(&alice).unwrap().inventory.slots[slot] = Stack::EMPTY;
        assert!(w.use_lead_on_mob(alice, a, slot));
        assert!(w.mobs[&a].leash.is_none());
    }

    #[test]
    fn knots_cannot_be_hit_from_afar_and_sneaking_places_blocks() {
        let (mut w, alice) = world_with_player();
        let post = BlockPos::new(2, -60, 2);
        fence(&mut w, post);
        let a = cow(&mut w, [3.5, -60.0, 0.5]);
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        w.use_lead_on_mob(alice, a, slot);
        let fence_state = w.block(post);
        // Sneaking with a block in hand: no knot.
        hold(&mut w, alice, "minecraft:dirt", 1);
        w.players.get_mut(&alice).unwrap().sneaking = true;
        assert!(!w.use_fence(alice, post, fence_state));
        w.players.get_mut(&alice).unwrap().sneaking = false;
        assert!(w.use_fence(alice, post, fence_state));
        let knot = *state(&w).unwrap().knots.keys().next().unwrap();
        w.players.get_mut(&alice).unwrap().pos = [40.5, -60.0, 0.5];
        assert!(w.hit_knot(alice, knot));
        assert!(w.mobs[&a].leash.is_some(), "out of reach");
        assert_eq!(state(&w).unwrap().knots.len(), 1);
    }

    #[test]
    fn a_trader_llama_taken_from_its_trader_stays() {
        let (mut w, alice) = world_with_player();
        let trader = next_entity_id();
        let t = Mob::new(MobKind::WanderingTrader, [0.5, -60.0, 3.5], &mut w.rng);
        w.mobs.insert(trader, t);
        let llama = next_entity_id();
        let mut l = Mob::new(MobKind::TraderLlama, [2.5, -60.0, 0.5], &mut w.rng);
        l.leader = Some(trader);
        w.mobs.insert(llama, l);
        let slot = hold(&mut w, alice, "minecraft:lead", 1);
        w.use_lead_on_mob(alice, llama, slot);
        assert!(w.mobs[&llama].leader.is_none());
        assert!(w.mobs[&llama].persistent);
        // The trader goes: the llama does not.
        w.mobs.remove(&trader);
        w.tick_mobs();
        assert!(w.mobs.contains_key(&llama));
    }

    #[test]
    fn a_trader_llama_saved_on_its_traders_lead_follows_it_again() {
        let (mut w, _) = world_with_player();
        let trader = next_entity_id();
        let t = Mob::new(MobKind::WanderingTrader, [0.5, -60.0, 3.5], &mut w.rng);
        let trader_uuid = t.uuid;
        w.mobs.insert(trader, t);
        let llama = next_entity_id();
        let mut l = Mob::new(MobKind::TraderLlama, [2.5, -60.0, 0.5], &mut w.rng);
        l.leash = Some(Leash::Uuid {
            uuid: trader_uuid,
            waited: 0,
        });
        w.mobs.insert(llama, l);
        w.track_leash(llama);
        w.tick_leads();
        assert_eq!(w.mobs[&llama].leader, Some(trader));
        assert!(w.mobs[&llama].leash.is_none());
        assert_eq!(leads_on_ground(&w), 0);
    }
}
