//! The world task owns every chunk, player and dropped item and runs the 20 TPS game
//! loop. Connection tasks talk to it only through [`Event`]s and receive ready-to-write
//! packet frames, so world state needs no locks.

mod building;
mod commands;
mod containers;
mod items;
mod storage;
mod survival;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use levitas_protocol::{angle, varint, BlockPos, Writer};
use levitas_registry::packet::play::clientbound;
use levitas_registry::{block, block_has_tag, entity_type, BlockInfo, ItemInfo, SyncedRegistry};
use levitas_world::{is_air, Chunk, RegionStorage, MAX_Y, MIN_Y};
use tokio::sync::{mpsc, oneshot};
use tracing::info;

use crate::connection::Profile;
use crate::flat::{FlatGenerator, SURFACE_Y};
use crate::inventory::{Inventory, Stack, CRAFTING_GRID, HOTBAR, OFFHAND, SLOTS};
use crate::loot::{LootContext, LootTables};
use crate::recipes::Recipes;
use crate::rng::Rng;
use building::ScheduledTick;
use containers::{BlockEntity, Open};
use items::ItemEntity;
#[cfg(test)]
pub use storage::uuid_string;
use storage::{LevelData, AUTOSAVE_TICKS, UNLOAD_TICKS};
use survival::MAX_HEALTH;

/// Chunk radius sent around each player.
pub const VIEW_DISTANCE: i32 = 8;
/// Frames a player may have queued before they are disconnected as too slow.
pub const OUTGOING_CAPACITY: usize = 4096;
const TICK: Duration = Duration::from_millis(50);

pub const GAME_MODE_SURVIVAL: u8 = 0;
pub const GAME_MODE_CREATIVE: u8 = 1;
const GAME_EVENT_CHANGE_GAME_MODE: u8 = 3;
const GAME_EVENT_START_WAITING_FOR_CHUNKS: u8 = 13;
/// Player info actions: add player, update game mode, update listed, update latency.
const PLAYER_INFO_ACTIONS: u8 = 0x01 | 0x04 | 0x08 | 0x10;
const PLAYER_INFO_UPDATE_GAME_MODE: u8 = 0x04;
const LEVEL_EVENT_BLOCK_BREAK: i32 = 2001;
const DIG_START: i32 = 0;
const DIG_CANCEL: i32 = 1;
const DIG_FINISH: i32 = 2;
/// A dig is accepted once the client's time would have broken 70% of the block, which
/// allows for network lag, as vanilla does.
const DIG_TOLERANCE: f32 = 0.7;
const ENTITY_EVENT_MAINHAND_BREAK: i8 = 47;
const DROP_STACK: i32 = 3;
const DROP_ITEM: i32 = 4;
const RELEASE_USE_ITEM: i32 = 5;
/// Creative clients set this slot to drop an item from the inventory screen.
const CREATIVE_DROP_SLOT: i16 = -1;

/// A length-prefixed packet, shared by every player it is sent to.
pub type Frame = Arc<[u8]>;

static NEXT_ENTITY_ID: AtomicI32 = AtomicI32::new(1);

pub fn next_entity_id() -> i32 {
    NEXT_ENTITY_ID.fetch_add(1, Ordering::Relaxed)
}

/// Something a player did, sent from their connection task to the world.
pub enum Event {
    Join {
        id: i32,
        profile: Profile,
        out: mpsc::Sender<Frame>,
    },
    Leave {
        id: i32,
    },
    Move {
        id: i32,
        pos: Option<[f64; 3]>,
        rot: Option<(f32, f32)>,
        on_ground: bool,
    },
    Dig {
        id: i32,
        status: i32,
        pos: BlockPos,
        sequence: i32,
    },
    UseItemOn {
        id: i32,
        hand: i32,
        pos: BlockPos,
        face: i32,
        /// Where on the face the player clicked, each coordinate in 0..=1.
        cursor: [f32; 3],
        sequence: i32,
    },
    UseItem {
        id: i32,
        hand: i32,
        sequence: i32,
    },
    CreativeSlot {
        id: i32,
        slot: i16,
        stack: Stack,
    },
    /// A click in an open container. The client reports the resulting slots, which the
    /// server trusts for now (no server-side click simulation yet).
    ContainerClick {
        id: i32,
        window: i32,
        slot: i16,
        button: u8,
        mode: i32,
        changed: Vec<(i16, Stack)>,
        carried: Stack,
    },
    ContainerClose {
        id: i32,
        window: i32,
    },
    HeldSlot {
        id: i32,
        slot: i16,
    },
    Swing {
        id: i32,
        hand: i32,
    },
    Sprint {
        id: i32,
        sprinting: bool,
    },
    Sneak {
        id: i32,
        sneaking: bool,
    },
    Respawn {
        id: i32,
    },
    Chat {
        id: i32,
        message: String,
    },
    Command {
        id: i32,
        command: String,
    },
    /// Save everything and disconnect everyone; `done` is signalled when finished.
    Shutdown {
        done: oneshot::Sender<()>,
    },
}

/// Starts the world task for the world saved in `dir` (created if missing) and returns
/// the channel connections send events to.
pub fn spawn(dir: impl Into<PathBuf>) -> mpsc::Sender<Event> {
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(World::new(dir.into()).run(rx));
    tx
}

struct Player {
    profile: Profile,
    out: mpsc::Sender<Frame>,
    mode: u8,
    pos: [f64; 3],
    yaw: f32,
    pitch: f32,
    on_ground: bool,
    /// Position other players last saw, to compute relative moves without drift.
    sent_pos: [f64; 3],
    moved: bool,
    center: (i32, i32),
    loaded: HashSet<(i32, i32)>,
    inventory: Inventory,
    held: usize,
    health: f32,
    food: i32,
    saturation: f32,
    exhaustion: f32,
    /// Ticks since the last natural heal or starvation damage.
    food_timer: u32,
    fall_distance: f64,
    sprinting: bool,
    sneaking: bool,
    alive: bool,
    /// Hand and start tick of the food being eaten.
    eating: Option<(i32, u64)>,
    /// The block being dug in survival and the tick digging started.
    digging: Option<(BlockPos, u64)>,
    /// A drag across inventory slots in progress.
    drag: crate::click::Drag,
    /// Health, food or saturation changed and the client has not been told.
    health_dirty: bool,
    /// A container window open besides the inventory.
    open: Option<Open>,
    next_window: i32,
    /// Set when the player's outgoing queue overflowed or closed.
    kicked: bool,
}

impl Player {
    fn new(profile: Profile, out: mpsc::Sender<Frame>, spawn: [f64; 3]) -> Self {
        Self {
            profile,
            out,
            mode: GAME_MODE_SURVIVAL,
            pos: spawn,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            sent_pos: spawn,
            moved: false,
            center: (
                (spawn[0] / 16.0).floor() as i32,
                (spawn[2] / 16.0).floor() as i32,
            ),
            loaded: HashSet::new(),
            inventory: Inventory::new(),
            held: 0,
            health: MAX_HEALTH,
            food: 20,
            saturation: 5.0,
            exhaustion: 0.0,
            food_timer: 0,
            fall_distance: 0.0,
            sprinting: false,
            sneaking: false,
            alive: true,
            eating: None,
            digging: None,
            drag: Default::default(),
            health_dirty: false,
            open: None,
            next_window: 0,
            kicked: false,
        }
    }

    fn deliver(&mut self, frame: &Frame) {
        if self.out.try_send(frame.clone()).is_err() {
            self.kicked = true;
        }
    }

    fn survival(&self) -> bool {
        self.mode == GAME_MODE_SURVIVAL
    }

    /// Inventory slot of the item in `hand` (0 main hand, 1 offhand).
    fn hand_slot(&self, hand: i32) -> usize {
        if hand == 0 {
            HOTBAR + self.held
        } else {
            OFFHAND
        }
    }
}

struct World {
    /// The world folder: level.dat, region/ and playerdata/.
    dir: PathBuf,
    storage: RegionStorage,
    chunks: HashMap<(i32, i32), Chunk>,
    /// Chunks changed since they were last saved.
    dirty: HashSet<(i32, i32)>,
    spawn: [f64; 3],
    players: HashMap<i32, Player>,
    items: HashMap<i32, ItemEntity>,
    block_entities: HashMap<BlockPos, BlockEntity>,
    scheduled: Vec<ScheduledTick>,
    generator: FlatGenerator,
    /// Light data shared by every chunk packet: full sky light, no block light.
    light: Vec<u8>,
    loot: LootTables,
    recipes: Recipes,
    rng: Rng,
    tick: u64,
}

fn frame(packet: Writer) -> Frame {
    packet.into_frame().into()
}

impl World {
    fn new(dir: PathBuf) -> Self {
        let level = LevelData::load(&dir);
        info!("world folder: {}", dir.display());
        Self {
            storage: RegionStorage::new(dir.join("region")),
            dir,
            chunks: HashMap::new(),
            dirty: HashSet::new(),
            spawn: [
                f64::from(level.spawn[0]) + 0.5,
                f64::from(level.spawn[1]),
                f64::from(level.spawn[2]) + 0.5,
            ],
            players: HashMap::new(),
            items: HashMap::new(),
            block_entities: HashMap::new(),
            scheduled: Vec::new(),
            generator: FlatGenerator::new(),
            light: full_sky_light(),
            loot: LootTables::load(),
            recipes: Recipes::load(),
            rng: Rng::from_time(),
            tick: level.time.max(0) as u64,
        }
    }

    async fn run(mut self, mut events: mpsc::Receiver<Event>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some(event) => self.handle(event),
                    None => return,
                },
                _ = tick.tick() => self.tick(),
            }
            let kicked: Vec<i32> = self
                .players
                .iter()
                .filter(|(_, p)| p.kicked)
                .map(|(&id, _)| id)
                .collect();
            for id in kicked {
                self.leave(id);
            }
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Join { id, profile, out } => self.join(id, profile, out),
            Event::Leave { id } => self.leave(id),
            Event::Move {
                id,
                pos,
                rot,
                on_ground,
            } => self.move_player(id, pos, rot, on_ground),
            Event::Dig {
                id,
                status,
                pos,
                sequence,
            } => self.dig(id, status, pos, sequence),
            Event::UseItemOn {
                id,
                hand,
                pos,
                face,
                cursor,
                sequence,
            } => self.use_item_on(id, hand, pos, face, cursor, sequence),
            Event::UseItem { id, hand, sequence } => self.use_item(id, hand, sequence),
            Event::CreativeSlot { id, slot, stack } => self.creative_slot(id, slot, stack),
            Event::ContainerClick {
                id,
                window,
                slot,
                button,
                mode,
                changed,
                carried,
            } => self.container_click(id, window, slot, button, mode, changed, carried),
            Event::ContainerClose { id, window } => self.container_closed(id, window),
            Event::HeldSlot { id, slot } => {
                if let (Some(p), Ok(slot)) = (self.players.get_mut(&id), usize::try_from(slot)) {
                    if slot < 9 {
                        p.held = slot;
                        p.eating = None;
                    }
                }
            }
            Event::Swing { id, hand } => {
                let animation = if hand == 0 { 0 } else { 3 };
                let packet = Writer::packet(clientbound::ANIMATE)
                    .varint(id)
                    .u8(animation);
                self.broadcast(packet, Some(id));
            }
            Event::Sprint { id, sprinting } => {
                if let Some(p) = self.players.get_mut(&id) {
                    p.sprinting = sprinting;
                }
            }
            Event::Sneak { id, sneaking } => {
                if let Some(p) = self.players.get_mut(&id) {
                    p.sneaking = sneaking;
                }
            }
            Event::Respawn { id } => self.respawn(id),
            Event::Chat { id, message } => {
                if let Some(p) = self.players.get(&id) {
                    let line = format!("<{}> {message}", p.profile.name);
                    info!("{line}");
                    self.broadcast(system_chat(&line), None);
                }
            }
            Event::Command { id, command } => self.command(id, &command),
            Event::Shutdown { done } => {
                self.shutdown();
                let _ = done.send(());
            }
        }
    }

    fn join(&mut self, id: i32, profile: Profile, out: mpsc::Sender<Frame>) {
        let name = profile.name.clone();
        self.players
            .insert(id, Player::new(profile, out, self.spawn));
        self.load_player(id);
        let mode = self.players[&id].mode;

        let mut login = Writer::packet(clientbound::LOGIN)
            .i32(id)
            .bool(false) // hardcore
            .varint(1)
            .string("minecraft:overworld") // dimension names
            .varint(100) // max players (unused by the client)
            .varint(VIEW_DISTANCE)
            .varint(VIEW_DISTANCE) // simulation distance
            .bool(false) // reduced debug info
            .bool(true) // enable respawn screen
            .bool(false); // limited crafting
        login = spawn_info(login, mode).bool(false); // enforces secure chat
        self.send(id, login);
        self.send(
            id,
            Writer::packet(clientbound::GAME_EVENT)
                .u8(GAME_EVENT_START_WAITING_FOR_CHUNKS)
                .f32(0.0),
        );

        // Everyone's tab list entry to the newcomer, the newcomer's to everyone else.
        let all: Vec<&Player> = self.players.values().collect();
        let everyone = player_info(&all);
        let newcomer = player_info(&[&self.players[&id]]);
        self.send(id, everyone);
        self.broadcast(newcomer, Some(id));
        for packet in spawn_player(id, &self.players[&id]) {
            self.broadcast(packet, Some(id));
        }
        self.send_entities_to(id);

        self.update_view(id);
        let p = &self.players[&id];
        let position = position_packet(p.pos, p.yaw, p.pitch);
        self.send(id, position);
        self.send_status(id);
        self.send(
            id,
            system_chat(&format!(
                "Welcome to Levitas, {name}! Survival flat world. Try /gamemode creative, /give, /kill."
            )),
        );
        self.broadcast(system_chat(&format!("{name} joined the game")), Some(id));
        info!(name, "joined the world ({} online)", self.players.len());
    }

    fn leave(&mut self, id: i32) {
        self.close_container(id);
        self.save_player(id);
        let Some(player) = self.players.remove(&id) else {
            return;
        };
        self.broadcast(
            Writer::packet(clientbound::REMOVE_ENTITIES)
                .varint(1)
                .varint(id),
            None,
        );
        self.broadcast(
            Writer::packet(clientbound::PLAYER_INFO_REMOVE)
                .varint(1)
                .uuid(player.profile.uuid),
            None,
        );
        let name = &player.profile.name;
        self.broadcast(system_chat(&format!("{name} left the game")), None);
        info!(name, "left the game ({} online)", self.players.len());
    }

    /// Spawns every other player and every dropped item for `id`, after joining or
    /// respawning (the client forgets all entities on respawn).
    fn send_entities_to(&mut self, id: i32) {
        let mut packets: Vec<Writer> = self
            .players
            .iter()
            .filter(|(&other, p)| other != id && p.alive)
            .flat_map(|(&other, p)| spawn_player(other, p))
            .collect();
        for (&item_id, item) in &self.items {
            packets.extend(items::spawn_packets(item_id, item));
        }
        for packet in packets {
            self.send(id, packet);
        }
    }

    /// Health, food and the whole inventory.
    fn send_status(&mut self, id: i32) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let health = survival::health_packet(p);
        let content = p.inventory.content_packet();
        p.health_dirty = false;
        self.send(id, health);
        self.send(id, content);
    }

    fn move_player(
        &mut self,
        id: i32,
        pos: Option<[f64; 3]>,
        rot: Option<(f32, f32)>,
        on_ground: bool,
    ) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if !p.alive {
            return;
        }
        if let Some((yaw, pitch)) = rot {
            p.yaw = yaw;
            p.pitch = pitch;
        }
        if let Some(pos) = pos {
            let dy = pos[1] - p.pos[1];
            if p.survival() {
                let horizontal = (pos[0] - p.pos[0]).hypot(pos[2] - p.pos[2]);
                if p.sprinting {
                    p.exhaustion += 0.1 * horizontal as f32;
                }
                if p.on_ground && !on_ground && dy > 0.0 {
                    p.exhaustion += if p.sprinting { 0.2 } else { 0.05 };
                }
                if !on_ground && dy < 0.0 {
                    p.fall_distance -= dy;
                }
            }
            p.pos = pos;
        }
        if !p.survival() {
            p.fall_distance = 0.0;
        }
        let fallen = if on_ground {
            std::mem::take(&mut p.fall_distance)
        } else {
            0.0
        };
        p.on_ground = on_ground;
        p.moved = true;

        let chunk = (
            (p.pos[0] / 16.0).floor() as i32,
            (p.pos[2] / 16.0).floor() as i32,
        );
        if chunk != p.center {
            p.center = chunk;
            self.update_view(id);
        }
        if fallen > 3.0 {
            self.damage(id, (fallen - 3.0).ceil() as f32, survival::Damage::Fall);
        }
    }

    fn dig(&mut self, id: i32, status: i32, pos: BlockPos, sequence: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let (alive, survival) = (p.alive, p.survival());
        let tool = p.inventory.held(p.held).item;
        let started = p.digging;
        match status {
            DIG_START | DIG_FINISH if alive => {
                let state = self.block(pos);
                if is_air(state) {
                    // Nothing to do.
                } else if !survival {
                    self.break_block(id, pos, state);
                } else {
                    let progress = survival::break_progress(state, tool);
                    let breaks = if status == DIG_START {
                        progress >= 1.0
                    } else {
                        // The client says it finished: check it dug long enough.
                        started.is_some_and(|(at, tick)| {
                            at == pos && progress * (self.tick - tick + 1) as f32 >= DIG_TOLERANCE
                        })
                    };
                    let p = self.players.get_mut(&id).expect("player exists");
                    p.digging = (status == DIG_START && !breaks).then_some((pos, self.tick));
                    if breaks {
                        self.break_block(id, pos, state);
                    } else if status == DIG_FINISH {
                        // Too fast: put the block back on the client.
                        self.send(id, block_update(pos, state));
                    }
                }
            }
            DIG_CANCEL => {
                if let Some(p) = self.players.get_mut(&id) {
                    p.digging = None;
                }
            }
            DROP_STACK | DROP_ITEM if alive => self.drop_held(id, status == DROP_STACK),
            RELEASE_USE_ITEM => {
                if let Some(p) = self.players.get_mut(&id) {
                    p.eating = None;
                }
            }
            _ => {}
        }
        if (DIG_START..=DIG_FINISH).contains(&status) {
            self.ack(id, sequence);
        }
    }

    fn break_block(&mut self, id: i32, pos: BlockPos, state: u16) {
        self.set_block(pos, block::AIR);
        self.remove_block_entity(pos);
        let effect = Writer::packet(clientbound::LEVEL_EVENT)
            .i32(LEVEL_EVENT_BLOCK_BREAK)
            .position(pos)
            .i32(i32::from(state))
            .bool(false);
        self.broadcast(effect, Some(id));

        let p = self.players.get_mut(&id).expect("player exists");
        let survival = p.survival();
        if survival {
            p.exhaustion += 0.005;
            let tool = p.inventory.held(p.held).item;
            if survival::harvestable(state, tool) {
                let drops = self.loot.block_drops(&mut LootContext {
                    state,
                    tool,
                    rng: &mut self.rng,
                });
                for (item, count) in drops {
                    self.drop_at_block(pos, Stack::new(item, count));
                }
            }
            self.wear_tool(id, state);
        } else if let Some(other) = crate::blocks::other_part(state, pos) {
            // In creative the other half goes too, without dropping anything.
            if crate::blocks::same_block(self.block(other), state) {
                self.set_block(other, block::AIR);
                self.update_neighbors(other);
            }
        }
        self.update_neighbors(pos);
    }

    /// A tool that broke a block loses durability (blocks that break instantly cost none).
    fn wear_tool(&mut self, id: i32, state: u16) {
        if BlockInfo::of(state).hardness == 0.0 {
            return;
        }
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let slot = p.hand_slot(0);
        let stack = p.inventory.slots[slot];
        let Some(tool) = ItemInfo::get(stack.item).and_then(|i| i.tool) else {
            return;
        };
        self.damage_item(id, slot, tool.damage_per_block, ENTITY_EVENT_MAINHAND_BREAK);
    }

    /// Uses up durability of the item in `slot`; when none is left the item breaks, with
    /// the break sound and particles (`break_event`) for everyone nearby.
    pub(super) fn damage_item(&mut self, id: i32, slot: usize, amount: i32, break_event: i8) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let mut stack = p.inventory.slots[slot];
        let max = ItemInfo::get(stack.item).map_or(0, |i| i.max_damage);
        if stack.is_empty() || max <= 0 || amount <= 0 {
            return;
        }
        stack.damage += amount;
        let broke = stack.damage >= max;
        p.inventory.slots[slot] = if broke { Stack::EMPTY } else { stack };
        let update = p.inventory.set_slot_packet(slot);
        self.send(id, update);
        if broke {
            self.broadcast(entity_event(id, break_event), None);
        }
    }

    fn creative_slot(&mut self, id: i32, slot: i16, stack: Stack) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        if p.mode != GAME_MODE_CREATIVE {
            return;
        }
        if slot == CREATIVE_DROP_SLOT {
            if !stack.is_empty() {
                self.throw_from_player(id, stack);
            }
        } else if let Some(s) = usize::try_from(slot).ok().filter(|&s| s < SLOTS) {
            p.inventory.slots[s] = stack;
        }
    }

    /// Broadcasts entity movement and runs survival and item updates.
    fn tick(&mut self) {
        self.tick += 1;
        let mut packets = Vec::new();
        for (&id, p) in self.players.iter_mut().filter(|(_, p)| p.moved && p.alive) {
            p.moved = false;
            let delta = [0, 1, 2].map(|i| ((p.pos[i] - p.sent_pos[i]) * 4096.0).round());
            let movement = if delta.iter().all(|d| d.abs() <= f64::from(i16::MAX)) {
                for (sent, d) in p.sent_pos.iter_mut().zip(delta) {
                    *sent += d / 4096.0;
                }
                Writer::packet(clientbound::MOVE_ENTITY_POS_ROT)
                    .varint(id)
                    .i16(delta[0] as i16)
                    .i16(delta[1] as i16)
                    .i16(delta[2] as i16)
                    .u8(angle(p.yaw))
                    .u8(angle(p.pitch))
                    .bool(p.on_ground)
            } else {
                p.sent_pos = p.pos;
                Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                    .varint(id)
                    .f64(p.pos[0])
                    .f64(p.pos[1])
                    .f64(p.pos[2])
                    .f64(0.0)
                    .f64(0.0)
                    .f64(0.0) // velocity
                    .f32(p.yaw)
                    .f32(p.pitch)
                    .bool(p.on_ground)
            };
            let head = Writer::packet(clientbound::ROTATE_HEAD)
                .varint(id)
                .u8(angle(p.yaw));
            packets.push((id, movement));
            packets.push((id, head));
        }
        for (id, packet) in packets {
            self.broadcast(packet, Some(id));
        }
        self.tick_scheduled();
        self.tick_survival();
        self.tick_items();
        self.tick_furnaces();
        if self.tick % AUTOSAVE_TICKS == 0 {
            self.save_all();
        }
        if self.tick % UNLOAD_TICKS == 0 {
            self.unload_unseen_chunks();
        }
    }

    /// Recenters a player's chunk view, unloading chunks that left the view distance and
    /// sending the ones that entered it.
    fn update_view(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let (cx, cz) = p.center;
        let in_view = |&(x, z): &(i32, i32)| {
            (x - cx).abs() <= VIEW_DISTANCE && (z - cz).abs() <= VIEW_DISTANCE
        };
        let gone: Vec<_> = p.loaded.iter().copied().filter(|c| !in_view(c)).collect();
        let mut new = Vec::new();
        for x in cx - VIEW_DISTANCE..=cx + VIEW_DISTANCE {
            for z in cz - VIEW_DISTANCE..=cz + VIEW_DISTANCE {
                if !p.loaded.contains(&(x, z)) {
                    new.push((x, z));
                }
            }
        }
        // Nearest first, so the area around the player appears first.
        new.sort_by_key(|&(x, z)| (x - cx).pow(2) + (z - cz).pow(2));
        let chunk_frames: Vec<Frame> = new.iter().map(|&(x, z)| self.chunk_frame(x, z)).collect();

        let p = self.players.get_mut(&id).expect("player exists");
        p.deliver(&frame(
            Writer::packet(clientbound::SET_CHUNK_CACHE_CENTER)
                .varint(cx)
                .varint(cz),
        ));
        for (x, z) in gone {
            p.loaded.remove(&(x, z));
            p.deliver(&frame(
                Writer::packet(clientbound::FORGET_LEVEL_CHUNK)
                    .i32(z)
                    .i32(x),
            ));
        }
        p.deliver(&frame(Writer::packet(clientbound::CHUNK_BATCH_START)));
        for (chunk, f) in new.iter().zip(&chunk_frames) {
            p.loaded.insert(*chunk);
            p.deliver(f);
        }
        p.deliver(&frame(
            Writer::packet(clientbound::CHUNK_BATCH_FINISHED).varint(new.len() as i32),
        ));
    }

    /// A chunk, loaded from disk or generated if it is not in memory yet.
    fn chunk(&mut self, x: i32, z: i32) -> &mut Chunk {
        if !self.chunks.contains_key(&(x, z)) {
            let chunk = self
                .load_chunk(x, z)
                .unwrap_or_else(|| self.generator.generate(x, z));
            self.chunks.insert((x, z), chunk);
        }
        self.chunks.get_mut(&(x, z)).expect("just inserted")
    }

    fn chunk_frame(&mut self, x: i32, z: i32) -> Frame {
        let mut body = Vec::new();
        varint::write(&mut body, 0); // heightmaps: none, the client computes them
        self.chunk(x, z).encode_sections(&mut body);
        let entities = self.block_entities_in(x, z);
        varint::write(&mut body, entities.len() as i32);
        for (pos, kind) in entities {
            body.push(((pos.x & 15) << 4 | (pos.z & 15)) as u8);
            body.extend_from_slice(&(pos.y as i16).to_be_bytes());
            varint::write(&mut body, kind);
            body.extend_from_slice(&[0x0A, 0x00]); // empty NBT compound: no extra data
        }
        body.extend_from_slice(&self.light);
        frame(
            Writer::packet(clientbound::LEVEL_CHUNK_WITH_LIGHT)
                .i32(x)
                .i32(z)
                .bytes(&body),
        )
    }

    fn block(&mut self, pos: BlockPos) -> u16 {
        let (cx, cz) = pos.chunk();
        self.chunk(cx, cz)
            .get((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize)
    }

    /// Changes a block and tells every player who has its chunk loaded.
    fn set_block(&mut self, pos: BlockPos, state: u16) {
        let (cx, cz) = pos.chunk();
        self.chunk(cx, cz)
            .set((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize, state);
        self.dirty.insert((cx, cz));
        let update = frame(block_update(pos, state));
        for p in self.players.values_mut() {
            if p.loaded.contains(&(cx, cz)) {
                p.deliver(&update);
            }
        }
    }

    /// Confirms the client's predicted block change with this sequence number.
    fn ack(&mut self, id: i32, sequence: i32) {
        self.send(
            id,
            Writer::packet(clientbound::BLOCK_CHANGED_ACK).varint(sequence),
        );
    }

    fn send(&mut self, id: i32, packet: Writer) {
        if let Some(p) = self.players.get_mut(&id) {
            p.deliver(&frame(packet));
        }
    }

    fn broadcast(&mut self, packet: Writer, except: Option<i32>) {
        let f = frame(packet);
        for (&id, p) in self.players.iter_mut() {
            if Some(id) != except {
                p.deliver(&f);
            }
        }
    }
}

/// The spawn information shared by the login and respawn packets.
fn spawn_info(packet: Writer, mode: u8) -> Writer {
    let overworld = SyncedRegistry::get("minecraft:dimension_type")
        .and_then(|r| r.id_of("minecraft:overworld"))
        .expect("overworld dimension type");
    packet
        .varint(overworld)
        .string("minecraft:overworld")
        .i64(0) // hashed seed
        .u8(mode)
        .i8(-1) // previous game mode: none
        .bool(false) // debug world
        .bool(true) // flat world
        .bool(false) // no death location
        .varint(0) // portal cooldown
        .varint(63) // sea level
}

fn position_packet(pos: [f64; 3], yaw: f32, pitch: f32) -> Writer {
    Writer::packet(clientbound::PLAYER_POSITION)
        .varint(1) // teleport ID
        .f64(pos[0])
        .f64(pos[1])
        .f64(pos[2])
        .f64(0.0)
        .f64(0.0)
        .f64(0.0) // velocity
        .f32(yaw)
        .f32(pitch)
        .i32(0) // all values absolute
}

fn system_chat(text: &str) -> Writer {
    Writer::packet(clientbound::SYSTEM_CHAT)
        .text(text)
        .bool(false) // chat, not the action bar
}

fn block_update(pos: BlockPos, state: u16) -> Writer {
    Writer::packet(clientbound::BLOCK_UPDATE)
        .position(pos)
        .varint(i32::from(state))
}

fn entity_event(id: i32, event: i8) -> Writer {
    Writer::packet(clientbound::ENTITY_EVENT).i32(id).i8(event)
}

fn player_info(players: &[&Player]) -> Writer {
    let mut packet = Writer::packet(clientbound::PLAYER_INFO_UPDATE)
        .u8(PLAYER_INFO_ACTIONS)
        .varint(players.len() as i32);
    for p in players {
        packet = packet
            .uuid(p.profile.uuid)
            .string(&p.profile.name)
            .varint(0) // add player: no profile properties
            .varint(i32::from(p.mode)) // update game mode
            .bool(true) // update listed
            .varint(0); // update latency
    }
    packet
}

/// Packets that make a player's entity appear for others.
fn spawn_player(id: i32, p: &Player) -> [Writer; 2] {
    [
        Writer::packet(clientbound::ADD_ENTITY)
            .varint(id)
            .uuid(p.profile.uuid)
            .varint(entity_type::PLAYER)
            .f64(p.pos[0])
            .f64(p.pos[1])
            .f64(p.pos[2])
            .u8(angle(p.pitch))
            .u8(angle(p.yaw))
            .u8(angle(p.yaw)) // head yaw
            .varint(0) // entity data
            .i16(0)
            .i16(0)
            .i16(0), // velocity
        Writer::packet(clientbound::ROTATE_HEAD)
            .varint(id)
            .u8(angle(p.yaw)),
    ]
}

/// Light data for a chunk packet: full sky light in every section, no block light.
fn full_sky_light() -> Vec<u8> {
    // Light covers one extra section below and above the world.
    const LIGHT_SECTIONS: usize = levitas_world::chunk::SECTIONS + 2;
    let all = (1u64 << LIGHT_SECTIONS) - 1;
    let mut out = Vec::new();
    let bitset = |out: &mut Vec<u8>, bits: u64| {
        if bits == 0 {
            varint::write(out, 0);
        } else {
            varint::write(out, 1);
            out.extend_from_slice(&bits.to_be_bytes());
        }
    };
    bitset(&mut out, all); // sky light mask
    bitset(&mut out, 0); // block light mask
    bitset(&mut out, 0); // empty sky light mask
    bitset(&mut out, all); // empty block light mask
    varint::write(&mut out, LIGHT_SECTIONS as i32);
    for _ in 0..LIGHT_SECTIONS {
        varint::write(&mut out, 2048);
        out.resize(out.len() + 2048, 0xFF);
    }
    varint::write(&mut out, 0); // block light arrays
    out
}
