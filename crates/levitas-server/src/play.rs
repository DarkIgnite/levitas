//! The play state for one player: joining the world, chunk streaming as the player moves,
//! keep-alives and chat.
//!
//! Each player currently lives in their own copy of the flat world; shared world state
//! and seeing other players come with region ticking (see docs/ARCHITECTURE.md).

use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use levitas_protocol::{Reader, Writer};
use levitas_registry::packet::play::{clientbound, serverbound};
use levitas_registry::SyncedRegistry;
use tokio::sync::mpsc;
use tracing::info;

use crate::connection::{read_frame, Conn, Outgoing, Profile};
use crate::world::{FlatWorld, SURFACE_Y};

/// Chunk radius sent around the player.
pub const VIEW_DISTANCE: i32 = 8;
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);

const GAME_EVENT_START_WAITING_FOR_CHUNKS: u8 = 13;
const GAME_MODE_CREATIVE: u8 = 1;
/// Player info actions: add player, update game mode, update listed, update latency.
const PLAYER_INFO_ACTIONS: u8 = 0x01 | 0x04 | 0x08 | 0x10;

pub async fn run(conn: Conn, profile: Profile, world: Arc<FlatWorld>) -> io::Result<()> {
    let Conn { mut reader, out } = conn;
    // Packets are read on their own task so a half-read packet is never dropped when the
    // main loop wakes up for something else.
    let (tx, mut incoming) = mpsc::channel(128);
    let reader_task = tokio::spawn(async move {
        while let Ok(packet) = read_frame(&mut reader).await {
            if tx.send(packet).await.is_err() {
                break;
            }
        }
    });

    let mut player = Player {
        out,
        profile,
        world,
        center: (0, 0),
        loaded: HashSet::new(),
    };
    let result = player.play(&mut incoming).await;
    reader_task.abort();
    info!(name = player.profile.name, "left the game");
    result
}

struct Player {
    out: Outgoing,
    profile: Profile,
    world: Arc<FlatWorld>,
    center: (i32, i32),
    loaded: HashSet<(i32, i32)>,
}

impl Player {
    async fn play(&mut self, incoming: &mut mpsc::Receiver<Vec<u8>>) -> io::Result<()> {
        self.join().await?;
        let mut keep_alive = tokio::time::interval(KEEP_ALIVE_INTERVAL);
        loop {
            tokio::select! {
                packet = incoming.recv() => match packet {
                    Some(packet) => self.handle(&packet).await?,
                    None => return Ok(()),
                },
                _ = keep_alive.tick() => {
                    let id = std::time::UNIX_EPOCH.elapsed().unwrap_or_default().as_millis() as i64;
                    self.out.send(Writer::packet(clientbound::KEEP_ALIVE).i64(id)).await?;
                }
            }
        }
    }

    async fn join(&mut self) -> io::Result<()> {
        let overworld = SyncedRegistry::get("minecraft:dimension_type")
            .and_then(|r| r.id_of("minecraft:overworld"))
            .expect("overworld dimension type is registered");
        self.out
            .queue(
                Writer::packet(clientbound::LOGIN)
                    .i32(1) // entity ID
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
            )
            .await?;
        self.out
            .queue(
                Writer::packet(clientbound::GAME_EVENT)
                    .u8(GAME_EVENT_START_WAITING_FOR_CHUNKS)
                    .f32(0.0),
            )
            .await?;
        // Add the player to their own tab list.
        self.out
            .queue(
                Writer::packet(clientbound::PLAYER_INFO_UPDATE)
                    .u8(PLAYER_INFO_ACTIONS)
                    .varint(1)
                    .uuid(self.profile.uuid)
                    .string(&self.profile.name)
                    .varint(0) // add player: no profile properties
                    .varint(i32::from(GAME_MODE_CREATIVE)) // update game mode
                    .bool(true) // update listed
                    .varint(0), // update latency
            )
            .await?;
        self.send_chunks().await?;
        self.out
            .queue(
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
            )
            .await?;
        self.system_message(&format!(
            "Welcome to Levitas, {}! This is a flat test world.",
            self.profile.name
        ))
        .await?;
        info!(name = self.profile.name, "joined the world");
        Ok(())
    }

    async fn handle(&mut self, packet: &[u8]) -> io::Result<()> {
        let mut r = Reader::new(packet);
        match r.varint()? {
            serverbound::MOVE_PLAYER_POS | serverbound::MOVE_PLAYER_POS_ROT => {
                let x = r.f64()?;
                let _y = r.f64()?;
                let z = r.f64()?;
                let chunk = ((x / 16.0).floor() as i32, (z / 16.0).floor() as i32);
                if chunk != self.center {
                    self.center = chunk;
                    self.send_chunks().await?;
                }
            }
            serverbound::CHAT => {
                let message = r.string(256)?.to_owned();
                info!(
                    name = self.profile.name,
                    "<{}> {message}", self.profile.name
                );
                let line = format!("<{}> {message}", self.profile.name);
                self.system_message(&line).await?;
            }
            serverbound::CHAT_COMMAND => {
                let command = r.string(32767)?.to_owned();
                self.system_message(&format!("Unknown command: /{command}"))
                    .await?;
            }
            // Teleport confirmations, keep-alive replies, settings, rotation-only moves
            // and everything else need no response yet.
            _ => {}
        }
        Ok(())
    }

    /// Recenters the client's chunk view on `self.center`, unloading chunks that left the
    /// view distance and sending the ones that entered it.
    async fn send_chunks(&mut self) -> io::Result<()> {
        let (cx, cz) = self.center;
        self.out
            .queue(
                Writer::packet(clientbound::SET_CHUNK_CACHE_CENTER)
                    .varint(cx)
                    .varint(cz),
            )
            .await?;

        let in_view = |&(x, z): &(i32, i32)| {
            (x - cx).abs() <= VIEW_DISTANCE && (z - cz).abs() <= VIEW_DISTANCE
        };
        let gone: Vec<_> = self
            .loaded
            .iter()
            .copied()
            .filter(|c| !in_view(c))
            .collect();
        for (x, z) in gone {
            self.loaded.remove(&(x, z));
            self.out
                .queue(
                    Writer::packet(clientbound::FORGET_LEVEL_CHUNK)
                        .i32(z)
                        .i32(x),
                )
                .await?;
        }

        let mut new = Vec::new();
        for x in cx - VIEW_DISTANCE..=cx + VIEW_DISTANCE {
            for z in cz - VIEW_DISTANCE..=cz + VIEW_DISTANCE {
                if !self.loaded.contains(&(x, z)) {
                    new.push((x, z));
                }
            }
        }
        // Nearest first, so the area around the player appears first.
        new.sort_by_key(|&(x, z)| (x - cx).pow(2) + (z - cz).pow(2));
        self.out
            .queue(Writer::packet(clientbound::CHUNK_BATCH_START))
            .await?;
        for &(x, z) in &new {
            self.out.queue(self.world.chunk_packet(x, z)).await?;
            self.loaded.insert((x, z));
        }
        self.out
            .queue(Writer::packet(clientbound::CHUNK_BATCH_FINISHED).varint(new.len() as i32))
            .await?;
        self.out.flush().await
    }

    async fn system_message(&mut self, text: &str) -> io::Result<()> {
        self.out
            .send(
                Writer::packet(clientbound::SYSTEM_CHAT)
                    .text(text)
                    .bool(false), // chat, not the action bar
            )
            .await
    }
}
