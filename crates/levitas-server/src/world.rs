//! The world task owns every chunk and player and runs the 20 TPS game loop. Connection
//! tasks talk to it only through [`Event`]s and receive ready-to-write packet frames, so
//! world state needs no locks.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use levitas_protocol::{angle, varint, BlockPos, Writer};
use levitas_registry::packet::play::clientbound;
use levitas_registry::{block, block_has_tag, entity_type, SyncedRegistry, ITEM_BLOCK};
use levitas_world::{is_air, Chunk, MAX_Y, MIN_Y};
use tokio::sync::mpsc;
use tracing::info;

use crate::connection::Profile;
use crate::flat::{FlatGenerator, SURFACE_Y};

/// Chunk radius sent around each player.
pub const VIEW_DISTANCE: i32 = 8;
/// Frames a player may have queued before they are disconnected as too slow.
pub const OUTGOING_CAPACITY: usize = 4096;
const TICK: Duration = Duration::from_millis(50);

const GAME_MODE_CREATIVE: u8 = 1;
const GAME_EVENT_START_WAITING_FOR_CHUNKS: u8 = 13;
/// Player info actions: add player, update game mode, update listed, update latency.
const PLAYER_INFO_ACTIONS: u8 = 0x01 | 0x04 | 0x08 | 0x10;
const LEVEL_EVENT_BLOCK_BREAK: i32 = 2001;
const DIG_START: i32 = 0;
const DIG_FINISH: i32 = 2;
/// Inventory slots of the hotbar and the offhand in the player's inventory menu.
const HOTBAR_SLOTS: std::ops::RangeInclusive<i16> = 36..=44;
const OFFHAND_SLOT: i16 = 45;

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
        sequence: i32,
    },
    CreativeSlot {
        id: i32,
        slot: i16,
        item: i32,
    },
    HeldSlot {
        id: i32,
        slot: i16,
    },
    Swing {
        id: i32,
        hand: i32,
    },
    Chat {
        id: i32,
        message: String,
    },
    Command {
        id: i32,
        command: String,
    },
}

/// Starts the world task and returns the channel connections send events to.
pub fn spawn() -> mpsc::Sender<Event> {
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(World::new().run(rx));
    tx
}

struct Player {
    profile: Profile,
    out: mpsc::Sender<Frame>,
    pos: [f64; 3],
    yaw: f32,
    pitch: f32,
    on_ground: bool,
    /// Position other players last saw, to compute relative moves without drift.
    sent_pos: [f64; 3],
    moved: bool,
    center: (i32, i32),
    loaded: HashSet<(i32, i32)>,
    hotbar: [i32; 9],
    offhand: i32,
    held: usize,
    /// Set when the player's outgoing queue overflowed or closed.
    kicked: bool,
}

impl Player {
    fn deliver(&mut self, frame: &Frame) {
        if self.out.try_send(frame.clone()).is_err() {
            self.kicked = true;
        }
    }
}

struct World {
    chunks: HashMap<(i32, i32), Chunk>,
    players: HashMap<i32, Player>,
    generator: FlatGenerator,
    /// Light data shared by every chunk packet: full sky light, no block light.
    light: Vec<u8>,
}

fn frame(packet: Writer) -> Frame {
    packet.into_frame().into()
}

impl World {
    fn new() -> Self {
        Self {
            chunks: HashMap::new(),
            players: HashMap::new(),
            generator: FlatGenerator::new(),
            light: full_sky_light(),
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
                sequence,
            } => self.use_item_on(id, hand, pos, face, sequence),
            Event::CreativeSlot { id, slot, item } => {
                if let Some(p) = self.players.get_mut(&id) {
                    if HOTBAR_SLOTS.contains(&slot) {
                        p.hotbar[(slot - HOTBAR_SLOTS.start()) as usize] = item;
                    } else if slot == OFFHAND_SLOT {
                        p.offhand = item;
                    }
                }
            }
            Event::HeldSlot { id, slot } => {
                if let (Some(p), Ok(slot)) = (self.players.get_mut(&id), usize::try_from(slot)) {
                    if slot < 9 {
                        p.held = slot;
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
            Event::Chat { id, message } => {
                if let Some(p) = self.players.get(&id) {
                    let line = format!("<{}> {message}", p.profile.name);
                    info!("{line}");
                    self.broadcast(system_chat(&line), None);
                }
            }
            Event::Command { id, command } => {
                self.send(id, system_chat(&format!("Unknown command: /{command}")));
            }
        }
    }

    fn join(&mut self, id: i32, profile: Profile, out: mpsc::Sender<Frame>) {
        let name = profile.name.clone();
        let player = Player {
            profile,
            out,
            pos: [0.5, SURFACE_Y, 0.5],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            sent_pos: [0.5, SURFACE_Y, 0.5],
            moved: false,
            center: (0, 0),
            loaded: HashSet::new(),
            hotbar: [0; 9],
            offhand: 0,
            held: 0,
            kicked: false,
        };
        self.players.insert(id, player);

        let overworld = SyncedRegistry::get("minecraft:dimension_type")
            .and_then(|r| r.id_of("minecraft:overworld"))
            .expect("overworld dimension type");
        self.send(
            id,
            Writer::packet(clientbound::LOGIN)
                .i32(id)
                .bool(false) // hardcore
                .varint(1)
                .string("minecraft:overworld") // dimension names
                .varint(100) // max players (unused by the client)
                .varint(VIEW_DISTANCE)
                .varint(VIEW_DISTANCE) // simulation distance
                .bool(false) // reduced debug info
                .bool(true) // enable respawn screen
                .bool(false) // limited crafting
                .varint(overworld)
                .string("minecraft:overworld")
                .i64(0) // hashed seed
                .u8(GAME_MODE_CREATIVE)
                .i8(-1) // previous game mode: none
                .bool(false) // debug world
                .bool(true) // flat world
                .bool(false) // no death location
                .varint(0) // portal cooldown
                .varint(63) // sea level
                .bool(false), // enforces secure chat
        );
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

        let others: Vec<Writer> = self
            .players
            .iter()
            .filter(|(&other, _)| other != id)
            .flat_map(|(&other, p)| spawn_player(other, p))
            .collect();
        for packet in others {
            self.send(id, packet);
        }
        for packet in spawn_player(id, &self.players[&id]) {
            self.broadcast(packet, Some(id));
        }

        self.update_view(id);
        self.send(
            id,
            Writer::packet(clientbound::PLAYER_POSITION)
                .varint(1) // teleport ID
                .f64(0.5)
                .f64(SURFACE_Y)
                .f64(0.5)
                .f64(0.0)
                .f64(0.0)
                .f64(0.0) // velocity
                .f32(0.0)
                .f32(0.0) // yaw, pitch
                .i32(0), // all values absolute
        );
        self.send(
            id,
            system_chat(&format!(
                "Welcome to Levitas, {name}! This is a flat test world."
            )),
        );
        self.broadcast(system_chat(&format!("{name} joined the game")), Some(id));
        info!(name, "joined the world ({} online)", self.players.len());
    }

    fn leave(&mut self, id: i32) {
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
        if let Some((yaw, pitch)) = rot {
            p.yaw = yaw;
            p.pitch = pitch;
        }
        p.on_ground = on_ground;
        p.moved = true;
        let Some(pos) = pos else {
            return;
        };
        p.pos = pos;
        let chunk = (
            (pos[0] / 16.0).floor() as i32,
            (pos[2] / 16.0).floor() as i32,
        );
        if chunk != p.center {
            p.center = chunk;
            self.update_view(id);
        }
    }

    fn dig(&mut self, id: i32, status: i32, pos: BlockPos, sequence: i32) {
        // Creative players break blocks instantly when they start digging.
        if status == DIG_START || status == DIG_FINISH {
            let old = self.block(pos);
            if !is_air(old) {
                self.set_block(pos, block::AIR);
                let effect = Writer::packet(clientbound::LEVEL_EVENT)
                    .i32(LEVEL_EVENT_BLOCK_BREAK)
                    .position(pos)
                    .i32(i32::from(old))
                    .bool(false);
                self.broadcast(effect, Some(id));
            }
        }
        if (0..=DIG_FINISH).contains(&status) {
            self.send(
                id,
                Writer::packet(clientbound::BLOCK_CHANGED_ACK).varint(sequence),
            );
        }
    }

    fn use_item_on(&mut self, id: i32, hand: i32, pos: BlockPos, face: i32, sequence: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let item = if hand == 0 {
            p.hotbar[p.held]
        } else {
            p.offhand
        };
        let state = usize::try_from(item)
            .ok()
            .and_then(|i| ITEM_BLOCK.get(i).copied())
            .unwrap_or(block::AIR);
        if state != block::AIR {
            // Placing on grass or similar replaces it; otherwise place against the face.
            let target = if block_has_tag(self.block(pos), "minecraft:replaceable") {
                pos
            } else {
                pos.offset(face)
            };
            let current = self.block(target);
            let in_world = (MIN_Y..=MAX_Y).contains(&target.y);
            if in_world && block_has_tag(current, "minecraft:replaceable") {
                self.set_block(target, state);
            } else {
                // Undo the client's predicted placement.
                self.send(id, block_update(target, current));
            }
        }
        self.send(
            id,
            Writer::packet(clientbound::BLOCK_CHANGED_ACK).varint(sequence),
        );
    }

    /// Broadcasts entity movement of players who moved since the last tick.
    fn tick(&mut self) {
        let mut packets = Vec::new();
        for (&id, p) in self.players.iter_mut().filter(|(_, p)| p.moved) {
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

    fn chunk(&mut self, x: i32, z: i32) -> &mut Chunk {
        let generator = &self.generator;
        self.chunks
            .entry((x, z))
            .or_insert_with(|| generator.generate(x, z))
    }

    fn chunk_frame(&mut self, x: i32, z: i32) -> Frame {
        let mut body = Vec::new();
        varint::write(&mut body, 0); // heightmaps: none, the client computes them
        self.chunk(x, z).encode_sections(&mut body);
        varint::write(&mut body, 0); // block entities
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
        let update = frame(block_update(pos, state));
        for p in self.players.values_mut() {
            if p.loaded.contains(&(cx, cz)) {
                p.deliver(&update);
            }
        }
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

fn player_info(players: &[&Player]) -> Writer {
    let mut packet = Writer::packet(clientbound::PLAYER_INFO_UPDATE)
        .u8(PLAYER_INFO_ACTIONS)
        .varint(players.len() as i32);
    for p in players {
        packet = packet
            .uuid(p.profile.uuid)
            .string(&p.profile.name)
            .varint(0) // add player: no profile properties
            .varint(i32::from(GAME_MODE_CREATIVE)) // update game mode
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
