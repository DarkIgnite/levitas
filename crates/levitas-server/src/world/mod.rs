//! The world task owns every chunk, player and dropped item and runs the 20 TPS game
//! loop. Connection tasks talk to it only through [`Event`]s and receive ready-to-write
//! packet frames, so world state needs no locks.

mod arrows;
mod breeding;
mod building;
mod campfire;
mod combat;
mod commands;
mod containers;
mod devices;
pub mod dimension;
mod effects;
mod enchanting_table;
mod experience;
mod explosion;
mod falling;
mod fire;
mod fluids;
mod hazards;
mod hoppers;
mod items;
mod mobs;
mod monsters;
mod nature;
mod pathfinding;
mod piston;
mod portal;
mod redstone;
mod storage;
mod survival;
mod time;
mod tnt;
pub mod travel;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use levitas_protocol::{angle, varint, BlockPos, Writer};
use levitas_registry::packet::play::clientbound;
use levitas_registry::{block, block_has_tag, entity_type, BlockInfo, ItemInfo, SyncedRegistry};
use levitas_world::chunk::pack_heightmap;

use self::dimension::Dimension;
use levitas_world::{is_air, Chunk, LightEngine, RegionStorage, MAX_Y, MIN_Y};
use tokio::sync::{mpsc, oneshot};
use tracing::info;

use crate::connection::Profile;
use crate::flat::FlatGenerator;
use crate::inventory::{Inventory, Stack, CRAFTING_GRID, HOTBAR, OFFHAND, SLOTS};
use crate::loot::{LootContext, LootTables};
use crate::recipes::Recipes;
use crate::rng::Rng;
use crate::terrain::{default_workers, Generated, GenerationPool, Terrain};
use building::ScheduledTick;
use containers::{BlockEntity, Open};
use items::ItemEntity;
use levitas_pumpkin::PumpkinTerrain;
#[cfg(test)]
pub use storage::uuid_string;
pub use storage::WorldKind;
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
/// Heightmap kinds in the chunk packet.
const HEIGHTMAP_WORLD_SURFACE: i32 = 1;
const HEIGHTMAP_MOTION_BLOCKING: i32 = 4;
const HEIGHTMAP_MOTION_BLOCKING_NO_LEAVES: i32 = 5;
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
/// Breaking sound and particles for a helmet; the other armor slots follow it.
const ENTITY_EVENT_HEAD_BREAK: i8 = 49;
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
        /// Lets the world point the connection at another dimension.
        route: travel::Route,
    },
    /// A player handed over from another dimension.
    Arrive(Box<travel::Arrival>),
    /// A message for every player here, sent from another dimension.
    Announce {
        frame: Frame,
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
    /// One of a window's buttons, such as an enchanting table's slots.
    ContainerButton {
        id: i32,
        window: i32,
        button: i32,
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
    /// The player clicked "Leave Bed".
    LeaveBed {
        id: i32,
    },
    /// The player attacked an entity.
    Attack {
        id: i32,
        target: i32,
    },
    /// The player used the item in `hand` on an entity.
    UseEntity {
        id: i32,
        target: i32,
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

/// How to create a world when `dir` has none yet.
#[derive(Debug, Clone, Copy)]
pub struct NewWorld {
    pub kind: WorldKind,
    /// The seed; random when `None`.
    pub seed: Option<i64>,
}

/// Starts the world tasks for the world saved in `dir` (created as `new` if missing),
/// one per dimension, and returns the overworld's channel, where connections start.
pub fn spawn(dir: impl Into<PathBuf>, new: NewWorld) -> mpsc::Sender<Event> {
    let dir = dir.into();
    let (overworld, generated) = World::new(dir.clone(), new);
    // The other dimensions share the overworld's seed and kind.
    let shared = NewWorld {
        kind: overworld.kind,
        seed: Some(overworld.seed),
    };
    let mut worlds = vec![(overworld, generated)];
    for dimension in Dimension::ALL.into_iter().skip(1) {
        worlds.push(World::new_in(dir.clone(), shared, dimension));
    }
    let channels: Vec<_> = worlds.iter().map(|_| mpsc::channel(1024)).collect();
    let senders: HashMap<Dimension, mpsc::Sender<Event>> = worlds
        .iter()
        .zip(&channels)
        .map(|((w, _), (tx, _))| (w.dimension, tx.clone()))
        .collect();
    let overworld_tx = senders[&Dimension::Overworld].clone();
    for ((mut world, generated), (_, rx)) in worlds.into_iter().zip(channels) {
        world.links = senders
            .iter()
            .filter(|(&d, _)| d != world.dimension)
            .map(|(&d, tx)| (d, tx.clone()))
            .collect();
        tokio::spawn(world.run(rx, generated));
    }
    overworld_tx
}

/// The terrain generator for a dimension of a world of this kind. The Nether is the
/// Nether in flat worlds too.
fn terrain_for(kind: WorldKind, seed: i64, dimension: Dimension) -> Arc<dyn Terrain> {
    match (dimension, kind) {
        (Dimension::Nether, _) => Arc::new(PumpkinTerrain::nether(seed)),
        (Dimension::Overworld, WorldKind::Flat) => Arc::new(FlatGenerator::new()),
        (Dimension::Overworld, WorldKind::Noise) => Arc::new(PumpkinTerrain::new(seed)),
    }
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
    /// The head of the bed the player is lying in, and for how many ticks.
    sleeping: Option<BlockPos>,
    sleep_ticks: u32,
    /// The bed the player respawns at.
    respawn: Option<BlockPos>,
    /// Tick the attack charge last started over (after an attack or switching items).
    attack_reset: u64,
    /// Hand and start tick of a bow being drawn.
    drawing: Option<(i32, u64)>,
    /// Experience level, points towards the next level, and points collected in all.
    xp_level: i32,
    xp_points: i32,
    xp_total: i32,
    /// Ticks of air left under water (300 when full), ticks left burning, and ticks
    /// until lava, fire or magma can hurt again.
    air: i32,
    fire_ticks: i32,
    hazard_cooldown: u32,
    /// Where the player's connection sends its events; switched on changing dimension.
    route: Option<travel::Route>,
    /// Ticks spent standing in a portal, and ticks until one may take them again.
    portal_ticks: u32,
    portal_cooldown: u32,
    /// Ticks left of the short invulnerability after being hurt, and the hit that
    /// started it: only harder hits get through meanwhile, and only by the difference.
    hurt_ticks: u32,
    last_damage: f32,
    /// Active status effects, and the extra health absorption gives.
    effects: Vec<effects::ActiveEffect>,
    absorption: f32,
    /// Seeds what enchanting tables offer; changes with every enchanting.
    enchant_seed: i32,
}

impl Player {
    fn new(profile: Profile, out: mpsc::Sender<Frame>, spawn: [f64; 3]) -> Self {
        let profile_seed = profile.uuid as i32;
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
            sleeping: None,
            sleep_ticks: 0,
            respawn: None,
            attack_reset: 0,
            drawing: None,
            xp_level: 0,
            xp_points: 0,
            xp_total: 0,
            air: 300,
            fire_ticks: 0,
            hazard_cooldown: 0,
            route: None,
            portal_ticks: 0,
            portal_cooldown: 0,
            hurt_ticks: 0,
            last_damage: 0.0,
            effects: Vec::new(),
            absorption: 0.0,
            enchant_seed: profile_seed,
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

    /// Where the player's eyes are, which way they look, and how far they reach.
    fn eye_ray(&self) -> ([f64; 3], [f64; 3], f64) {
        let eye_height = if self.sneaking { 1.27 } else { 1.62 };
        let eye = [self.pos[0], self.pos[1] + eye_height, self.pos[2]];
        let (yaw, pitch) = (
            f64::from(self.yaw).to_radians(),
            f64::from(self.pitch).to_radians(),
        );
        let look = [
            -yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        ];
        let reach = if self.survival() { 4.5 } else { 5.0 };
        (eye, look, reach)
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
    /// Which dimension this world task runs.
    dimension: Dimension,
    /// The other dimensions' world tasks.
    links: HashMap<Dimension, mpsc::Sender<Event>>,
    /// Nether portals in this dimension, for travellers to come out of.
    portals: Vec<portal::KnownPortal>,
    /// The world folder: level.dat, region/ and playerdata/.
    dir: PathBuf,
    storage: RegionStorage,
    /// Mobs are saved apart from blocks, in `entities/` region files like vanilla.
    entity_storage: RegionStorage,
    mobs: HashMap<i32, mobs::Mob>,
    arrows: HashMap<i32, arrows::Arrow>,
    orbs: HashMap<i32, experience::XpOrb>,
    /// Loaded chunks that held mobs at the last save, so emptied ones are saved too.
    mob_chunks: HashSet<(i32, i32)>,
    biome_spawns: mobs::BiomeSpawns,
    chunks: HashMap<(i32, i32), Chunk>,
    /// Chunks changed since they were last saved.
    dirty: HashSet<(i32, i32)>,
    spawn: [f64; 3],
    players: HashMap<i32, Player>,
    items: HashMap<i32, ItemEntity>,
    falling_blocks: HashMap<i32, falling::FallingBlock>,
    /// Lit TNT waiting to explode.
    tnt: HashMap<i32, tnt::PrimedTnt>,
    /// Blocks changed this tick, for redstone to look around.
    redstone_dirty: Vec<BlockPos>,
    /// Comparators' output levels, which their block states do not hold.
    comparators: HashMap<BlockPos, u8>,
    /// Blocks a piston is moving, waiting to land.
    landings: Vec<piston::Landing>,
    block_entities: HashMap<BlockPos, BlockEntity>,
    scheduled: Vec<ScheduledTick>,
    terrain: Arc<dyn Terrain>,
    /// Workers generating chunks for expensive terrain.
    generation: Option<GenerationPool>,
    /// Chunks requested from the workers and not back yet.
    generating: HashSet<(i32, i32)>,
    /// Keeps light right as blocks change.
    light: LightEngine,
    fluid_ticks: fluids::FluidTicks,
    /// Time of day in ticks (0 is sunrise); unlike `tick`, sleeping moves it on.
    day_time: i64,
    weather: time::Weather,
    seed: i64,
    kind: WorldKind,
    loot: LootTables,
    recipes: Recipes,
    rng: Rng,
    tick: u64,
}

fn frame(packet: Writer) -> Frame {
    packet.into_frame().into()
}

impl World {
    /// The overworld of the world in `dir`.
    fn new(
        dir: PathBuf,
        new: NewWorld,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<Vec<Generated>>) {
        Self::new_in(dir, new, Dimension::Overworld)
    }

    fn new_in(
        dir: PathBuf,
        new: NewWorld,
        dimension: Dimension,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<Vec<Generated>>) {
        let existing = LevelData::load(&dir);
        let created = existing.is_none();
        let mut level = existing.unwrap_or_else(|| LevelData {
            spawn: [0, 0, 0],
            time: 0,
            day_time: 0,
            weather: time::Weather::default(),
            seed: new
                .seed
                .unwrap_or_else(|| Rng::from_time().next_u64() as i64),
            kind: new.kind,
        });
        let terrain = terrain_for(level.kind, level.seed, dimension);
        if created && dimension == Dimension::Overworld {
            level.spawn = terrain.spawn_point();
            info!("created a new {:?} world, seed {}", level.kind, level.seed);
        }
        if dimension == Dimension::Overworld {
            info!("world folder: {}", dir.display());
        }
        let folder = dimension.folder(&dir);
        let (generation, generated) = if terrain.is_cheap() {
            (None, tokio::sync::mpsc::unbounded_channel().1)
        } else {
            let (pool, generated) = GenerationPool::start(Arc::clone(&terrain), default_workers());
            (Some(pool), generated)
        };
        let world = Self {
            dimension,
            links: HashMap::new(),
            portals: Self::load_portals(&folder),
            storage: RegionStorage::new(folder.join("region")),
            entity_storage: RegionStorage::new(folder.join("entities")),
            mobs: HashMap::new(),
            arrows: HashMap::new(),
            orbs: HashMap::new(),
            mob_chunks: HashSet::new(),
            biome_spawns: mobs::BiomeSpawns::load(),
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
            falling_blocks: HashMap::new(),
            tnt: HashMap::new(),
            redstone_dirty: Vec::new(),
            comparators: HashMap::new(),
            landings: Vec::new(),
            block_entities: HashMap::new(),
            scheduled: Vec::new(),
            terrain,
            generation,
            generating: HashSet::new(),
            light: LightEngine::new(),
            fluid_ticks: fluids::FluidTicks::default(),
            day_time: level.day_time,
            weather: level.weather.clone(),
            seed: level.seed,
            kind: level.kind,
            loot: LootTables::load(),
            recipes: Recipes::load(),
            rng: Rng::from_time(),
            tick: level.time.max(0) as u64,
        };
        (world, generated)
    }

    async fn run(
        mut self,
        mut events: mpsc::Receiver<Event>,
        mut generated: tokio::sync::mpsc::UnboundedReceiver<Vec<Generated>>,
    ) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some(event) => self.handle(event),
                    None => return,
                },
                _ = tick.tick() => self.tick(),
                Some(area) = generated.recv() => self.area_generated(area),
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
            Event::Join {
                id,
                profile,
                out,
                route,
            } => self.join(id, profile, out, route),
            Event::Arrive(arrival) => self.arrive(*arrival),
            Event::Announce { frame } => {
                for p in self.players.values_mut() {
                    p.deliver(&frame);
                }
            }
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
            Event::ContainerButton { id, window, button } => self.window_button(id, window, button),
            Event::HeldSlot { id, slot } => {
                if let (Some(p), Ok(slot)) = (self.players.get_mut(&id), usize::try_from(slot)) {
                    if slot < 9 {
                        p.held = slot;
                        p.eating = None;
                        p.attack_reset = self.tick;
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
            Event::LeaveBed { id } => self.wake_up(id),
            Event::Attack { id, target } => self.attack(id, target),
            Event::UseEntity { id, target, hand } => self.use_on_mob(id, target, hand),
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
                    self.announce(system_chat(&line));
                }
            }
            Event::Command { id, command } => self.command(id, &command),
            Event::Shutdown { done } => {
                // The overworld, where shutdown starts, has the other dimensions save
                // too; `done` waits for all of them.
                let mut others = Vec::new();
                let links = if self.dimension == Dimension::Overworld {
                    self.links.values().cloned().collect()
                } else {
                    Vec::new()
                };
                for link in links {
                    let (tx, rx) = oneshot::channel();
                    if link.try_send(Event::Shutdown { done: tx }).is_ok() {
                        others.push(rx);
                    }
                }
                self.shutdown();
                tokio::spawn(async move {
                    for rx in others {
                        let _ = rx.await;
                    }
                    let _ = done.send(());
                });
            }
        }
    }

    fn join(&mut self, id: i32, profile: Profile, out: mpsc::Sender<Frame>, route: travel::Route) {
        let mut player = Player::new(profile, out, self.spawn);
        player.route = Some(route);
        self.players.insert(id, player);
        let saved_in = self.load_player(id);
        // A player who logged out in another dimension goes back there.
        if let Some(other) = saved_in.filter(|&d| d != self.dimension) {
            if self.hand_over(id, other, None, true, false, false) {
                return;
            }
        }
        self.welcome(id);
    }

    /// Logs a player in to this dimension: the login packet, the world around them,
    /// everyone else.
    fn welcome(&mut self, id: i32) {
        let name = self.players[&id].profile.name.clone();
        let mode = self.players[&id].mode;

        let mut login = Writer::packet(clientbound::LOGIN)
            .i32(id)
            .bool(false) // hardcore
            .varint(Dimension::ALL.len() as i32) // dimension names
            .string(Dimension::Overworld.name())
            .string(Dimension::Nether.name())
            .varint(100) // max players (unused by the client)
            .varint(VIEW_DISTANCE)
            .varint(VIEW_DISTANCE) // simulation distance
            .bool(false) // reduced debug info
            .bool(true) // enable respawn screen
            .bool(false); // limited crafting
        login = spawn_info(login, mode, self.dimension).bool(false); // enforces secure chat
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
        self.send_time_and_weather(id);
        self.send(id, commands::command_tree());
        let recipes = self.recipes.update_packet();
        self.send(id, recipes);
        self.send(
            id,
            system_chat(&format!(
                "Welcome to Levitas, {name}! Survival world. Try /gamemode creative, /give, /kill."
            )),
        );
        self.announce_except(system_chat(&format!("{name} joined the game")), id);
        info!(name, "joined the world ({} online)", self.players.len());
    }

    fn leave(&mut self, id: i32) {
        self.wake_up(id);
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
        let name = player.profile.name.clone();
        self.announce(system_chat(&format!("{name} left the game")));
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
        for (&falling_id, falling) in &self.falling_blocks {
            packets.push(falling::spawn_packet(falling_id, falling));
        }
        for (&tnt_id, tnt) in &self.tnt {
            packets.extend(tnt::spawn_packets(tnt_id, tnt));
        }
        packets.extend(self.arrow_packets());
        packets.extend(self.orb_packets());
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
        let xp = experience::xp_packet(p);
        p.health_dirty = false;
        self.send(id, health);
        self.send(id, content);
        self.send(id, xp);
        self.send_effects(id);
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
        // Water breaks a fall, and so does climbing.
        let feet = BlockPos::new(
            p.pos[0].floor() as i32,
            p.pos[1].floor() as i32,
            p.pos[2].floor() as i32,
        );
        let at_feet = self
            .chunks
            .get(&feet.chunk())
            .filter(|_| (MIN_Y..=MAX_Y).contains(&feet.y))
            .map_or(block::AIR, |c| {
                c.get((feet.x & 15) as usize, feet.y, (feet.z & 15) as usize)
            });
        let p = self.players.get_mut(&id).expect("player exists");
        if crate::blocks::fluid::fluid_at(at_feet)
            .is_some_and(|f| f.fluid == crate::blocks::fluid::Fluid::Water)
            || block_has_tag(at_feet, "minecraft:climbable")
        {
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
        let ground_pos = BlockPos::new(feet.x, feet.y - 1, feet.z);
        let at_ground = self
            .chunks
            .get(&ground_pos.chunk())
            .filter(|_| (MIN_Y..=MAX_Y).contains(&ground_pos.y))
            .map_or(block::AIR, |c| {
                c.get(
                    (ground_pos.x & 15) as usize,
                    ground_pos.y,
                    (ground_pos.z & 15) as usize,
                )
            });
        if (crate::blocks::is_stalagmite(at_feet) || crate::blocks::is_stalagmite(at_ground))
            && fallen > 0.0
        {
            let dmg = (fallen * 2.0 - 1.0).floor().max(1.0) as f32;
            self.damage(id, dmg, survival::Damage::FallingStalactite);
        } else {
            // Jump boost lets players fall further unhurt; slow falling saves them.
            let p = &self.players[&id];
            let safe = 3.0 + f64::from(p.effect_level(levitas_registry::mob_effect::JUMP_BOOST));
            let slow = p
                .effect(levitas_registry::mob_effect::SLOW_FALLING)
                .is_some();
            if fallen > safe && !slow {
                self.damage(id, (fallen - safe).ceil() as f32, survival::Damage::Fall);
            }
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
                if status == DIG_START
                    && survival
                    && crate::blocks::name(state) == "minecraft:note_block"
                {
                    self.play_note(pos);
                }
                if is_air(state) {
                    // Nothing to do.
                } else if !survival {
                    self.break_block(id, pos, state);
                } else {
                    let efficiency = self.players[&id]
                        .inventory
                        .held(self.players[&id].held)
                        .enchants
                        .level_of("minecraft:efficiency");
                    let progress = survival::break_progress_with(state, tool, efficiency)
                        * self.players[&id].dig_speed_factor();
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
                self.release_bow(id);
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
            let held_enchants = p.inventory.held(p.held).enchants;
            if survival::harvestable(state, tool) {
                let drops = self.loot.block_drops(&mut LootContext {
                    state,
                    tool,
                    rng: &mut self.rng,
                    enchants: held_enchants,
                    entity: Default::default(),
                });
                for (item, count) in drops {
                    self.drop_at_block(pos, Stack::new(item, count));
                }
                if held_enchants.level_of("minecraft:silk_touch") == 0 {
                    self.mined_xp(pos, state);
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
        // Unbreaking spares each point of wear: tools by level in level + 1, armor less.
        let unbreaking = stack.enchants.level_of("minecraft:unbreaking");
        let amount = if unbreaking > 0 {
            let armor = (5..9).contains(&slot);
            (0..amount)
                .filter(|_| {
                    let r = self.rng.f64();
                    if armor {
                        r < 0.6 + 0.4 / f64::from(unbreaking + 1)
                    } else {
                        r < 1.0 / f64::from(unbreaking + 1)
                    }
                })
                .count() as i32
        } else {
            amount
        };
        if amount == 0 {
            return;
        }
        let p = self.players.get_mut(&id).expect("player exists");
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
        self.send_light_changes();
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
        self.tick_pressure_plates();
        self.tick_pistons();
        self.tick_redstone();
        self.tick_fluids();
        self.tick_random();
        if self.dimension.has_sky() {
            self.tick_time();
        }
        self.tick_falling_blocks();
        self.tick_tnt();
        self.tick_mobs();
        self.tick_arrows();
        self.tick_orbs();
        if self.dimension == Dimension::Overworld {
            self.tick_monster_spawning();
        }
        self.tick_survival();
        self.tick_hazards();
        self.tick_portals();
        self.tick_effects();
        self.tick_items();
        self.tick_furnaces();
        self.tick_campfires();
        self.tick_hoppers();
        self.tick_daylight_detectors();
        if self.tick.is_multiple_of(AUTOSAVE_TICKS) {
            self.save_all();
        }
        if self.tick.is_multiple_of(UNLOAD_TICKS) {
            self.unload_unseen_chunks();
        }
    }

    /// Moves a player within this dimension.
    pub(super) fn teleport(&mut self, id: i32, pos: [f64; 3]) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        p.pos = pos;
        p.moved = true;
        p.fall_distance = 0.0;
        p.center = (
            (pos[0] / 16.0).floor() as i32,
            (pos[2] / 16.0).floor() as i32,
        );
        let (yaw, pitch) = (p.yaw, p.pitch);
        self.update_view(id);
        self.send(id, position_packet(pos, yaw, pitch));
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
        // Nearest first, so the area around the player appears first. Chunks that still
        // have to be generated are sent once they are ready.
        new.sort_by_key(|&(x, z)| (x - cx).pow(2) + (z - cz).pow(2));
        new.retain(|&(x, z)| self.ensure_chunk(x, z));
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

    /// A chunk, loaded from disk or generated if it is not in memory yet. This generates
    /// on the spot if needed; [`World::ensure_chunk`] is for chunks a player is waiting for.
    fn chunk(&mut self, x: i32, z: i32) -> &mut Chunk {
        if !self.chunks.contains_key(&(x, z)) {
            match self.load_chunk(x, z) {
                Some(chunk) => {
                    self.chunks.insert((x, z), chunk);
                    self.load_mobs(x, z);
                }
                None if self.terrain.is_cheap() => {
                    // Cheap terrain is only saved once changed, but its mobs always are.
                    self.chunks.insert((x, z), self.terrain.generate(x, z));
                    self.load_mobs(x, z);
                }
                None => {
                    // Real terrain comes a whole area at a time; keep all of it.
                    let size = self.terrain.area();
                    let area = self
                        .terrain
                        .generate_area(x.div_euclid(size), z.div_euclid(size));
                    self.area_generated(area);
                }
            }
        }
        self.chunks.get_mut(&(x, z)).expect("just inserted")
    }

    /// Makes a chunk available if that is quick (in memory, on disk, or cheap terrain);
    /// otherwise asks the workers for it. Returns whether it is available now.
    fn ensure_chunk(&mut self, x: i32, z: i32) -> bool {
        if self.chunks.contains_key(&(x, z)) {
            return true;
        }
        if let Some(chunk) = self.load_chunk(x, z) {
            self.chunks.insert((x, z), chunk);
            self.load_mobs(x, z);
            return true;
        }
        match &self.generation {
            Some(pool) => {
                let size = self.terrain.area();
                let area = (x.div_euclid(size), z.div_euclid(size));
                if self.generating.insert(area) {
                    pool.request(area.0, area.1);
                }
                false
            }
            None => {
                self.chunk(x, z);
                true
            }
        }
    }

    /// An area the workers finished: keep its chunks, and send them to everyone waiting
    /// for them.
    fn area_generated(&mut self, area: Vec<Generated>) {
        let size = self.terrain.area();
        if let Some(&((x, z), _)) = area.first() {
            self.generating
                .remove(&(x.div_euclid(size), z.div_euclid(size)));
        }
        let mut new = Vec::with_capacity(area.len());
        let mut fresh = Vec::new();
        for (pos, chunk) in area {
            // Chunks generated on the spot meanwhile, or saved before, stay as they are.
            if self.chunks.contains_key(&pos) {
                continue;
            }
            match self.load_chunk(pos.0, pos.1) {
                Some(saved) => {
                    self.chunks.insert(pos, saved);
                    self.load_mobs(pos.0, pos.1);
                }
                None => {
                    self.chunks.insert(pos, chunk);
                    self.dirty.insert(pos);
                    fresh.push(pos);
                }
            }
            new.push(pos);
        }
        for (x, z) in fresh {
            self.spawn_animals_in_chunk(x, z);
        }
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for id in ids {
            let p = &self.players[&id];
            let waiting: Vec<(i32, i32)> = new
                .iter()
                .copied()
                .filter(|&(x, z)| {
                    (x - p.center.0).abs() <= VIEW_DISTANCE
                        && (z - p.center.1).abs() <= VIEW_DISTANCE
                        && !p.loaded.contains(&(x, z))
                })
                .collect();
            if waiting.is_empty() {
                continue;
            }
            let frames: Vec<Frame> = waiting
                .iter()
                .map(|&(x, z)| self.chunk_frame(x, z))
                .collect();
            let p = self.players.get_mut(&id).expect("player exists");
            p.deliver(&frame(Writer::packet(clientbound::CHUNK_BATCH_START)));
            for (pos, chunk) in waiting.iter().zip(&frames) {
                p.loaded.insert(*pos);
                p.deliver(chunk);
            }
            p.deliver(&frame(
                Writer::packet(clientbound::CHUNK_BATCH_FINISHED).varint(waiting.len() as i32),
            ));
        }
    }

    fn chunk_frame(&mut self, x: i32, z: i32) -> Frame {
        let mut body = Vec::new();
        // The client places rain, snow and their sounds by these heightmaps, and does
        // not compute them itself.
        let chunk = self.chunk(x, z);
        let motion_blocking = |s: u16| {
            !levitas_registry::collision_boxes(s).is_empty()
                || crate::blocks::fluid::fluid_at(s).is_some()
        };
        let heightmaps = [
            (HEIGHTMAP_WORLD_SURFACE, chunk.heightmap(|s| !is_air(s))),
            (HEIGHTMAP_MOTION_BLOCKING, chunk.heightmap(motion_blocking)),
            (
                HEIGHTMAP_MOTION_BLOCKING_NO_LEAVES,
                chunk.heightmap(|s| motion_blocking(s) && !block_has_tag(s, "minecraft:leaves")),
            ),
        ];
        varint::write(&mut body, heightmaps.len() as i32);
        for (kind, heights) in heightmaps {
            varint::write(&mut body, kind);
            // Heights count from the bottom of the dimension.
            let below = (16 * self.dimension.span().first) as u16;
            let heights = heights.map(|h| h.saturating_sub(below));
            let longs = pack_heightmap(&heights);
            varint::write(&mut body, longs.len() as i32);
            for long in longs {
                body.extend_from_slice(&long.to_be_bytes());
            }
        }
        let span = self.dimension.span();
        self.chunk(x, z).encode_sections(&mut body, span);
        let entities = self.block_entities_in(x, z);
        varint::write(&mut body, entities.len() as i32);
        for (pos, kind, data) in entities {
            body.push(((pos.x & 15) << 4 | (pos.z & 15)) as u8);
            body.extend_from_slice(&(pos.y as i16).to_be_bytes());
            varint::write(&mut body, kind);
            match data {
                Some(data) => body
                    .extend_from_slice(&levitas_nbt::to_network(&levitas_nbt::Tag::Compound(data))),
                None => body.extend_from_slice(&[0x0A, 0x00]), // empty compound: no data
            }
        }
        self.chunk(x, z).light.encode(&mut body, span);
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
        let old =
            self.chunk(cx, cz)
                .set((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize, state);
        self.dirty.insert((cx, cz));
        if old != state {
            self.redstone_changed(pos);
        }
        self.light
            .block_changed(&mut self.chunks, [pos.x, pos.y, pos.z], old, state);
        self.fluids_changed_around(pos);
        let update = frame(block_update(pos, state));
        for p in self.players.values_mut() {
            if p.loaded.contains(&(cx, cz)) {
                p.deliver(&update);
            }
        }
    }

    /// Sends the light that changed since the last tick to players who see it, and
    /// remembers to save it.
    fn send_light_changes(&mut self) {
        for ((x, z), sky, block) in self.light.take_changes() {
            let Some(chunk) = self.chunks.get(&(x, z)) else {
                continue;
            };
            self.dirty.insert((x, z));
            let mut body = Vec::new();
            chunk
                .light
                .encode_sections(&mut body, sky, block, self.dimension.span());
            let update = frame(
                Writer::packet(clientbound::LIGHT_UPDATE)
                    .varint(x)
                    .varint(z)
                    .bytes(&body),
            );
            for p in self.players.values_mut() {
                if p.loaded.contains(&(x, z)) {
                    p.deliver(&update);
                }
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
fn spawn_info(packet: Writer, mode: u8, dimension: Dimension) -> Writer {
    packet
        .varint(dimension.type_id())
        .string(dimension.name())
        .i64(0) // hashed seed
        .u8(mode)
        .i8(-1) // previous game mode: none
        .bool(false) // debug world
        .bool(true) // flat world
        .bool(false) // no death location
        .varint(0) // portal cooldown
        .varint(dimension.sea_level())
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
