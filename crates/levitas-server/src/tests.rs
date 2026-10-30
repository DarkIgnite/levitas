//! End-to-end protocol tests against a server running in-process.

use levitas_protocol::{BlockPos, Reader, Writer, PROTOCOL_VERSION};
use levitas_registry::packet::{configuration, login, play};
use levitas_registry::{block, entity_type, ItemInfo};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use crate::connection::{handle, read_frame};
use crate::world::{self, view_distance, Event};

/// A fresh, empty world folder for one test.
fn temp_world() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("levitas-test-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Starts a server on the flat world in `dir`; returns its address and its world
/// channel.
async fn server_in(dir: PathBuf) -> (SocketAddr, mpsc::Sender<Event>) {
    server_with(dir, world::WorldKind::Flat).await
}

async fn server_with(dir: PathBuf, kind: world::WorldKind) -> (SocketAddr, mpsc::Sender<Event>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let world = world::spawn(
        dir,
        world::NewWorld {
            kind,
            seed: Some(12345),
        },
    );
    let events = world.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(handle(stream, world.clone()));
        }
    });
    (addr, events)
}

/// Starts a server with its own fresh world and returns its address.
async fn server() -> SocketAddr {
    server_in(temp_world()).await.0
}

async fn shutdown(world: &mpsc::Sender<Event>) {
    let (done, saved) = oneshot::channel();
    world.send(Event::Shutdown { done }).await.unwrap();
    saved.await.unwrap();
}

async fn send(client: &mut TcpStream, packet: Writer) {
    client.write_all(&packet.into_frame()).await.unwrap();
}

/// Reads packets until one with `id` arrives, returning its body after the ID.
async fn read_until(client: &mut TcpStream, id: i32) -> Vec<u8> {
    loop {
        let packet = read_frame(client).await.unwrap();
        let mut r = Reader::new(&packet);
        if r.varint().unwrap() == id {
            return packet[packet.len() - r.remaining()..].to_vec();
        }
    }
}

fn handshake(intent: i32) -> Writer {
    Writer::packet(0x00)
        .varint(PROTOCOL_VERSION)
        .string("localhost")
        .u16(25565)
        .varint(intent)
}

/// Logs in and completes configuration, leaving the client in the play state.
async fn join(addr: SocketAddr, name: &str, uuid: u128) -> TcpStream {
    join_with_login(addr, name, uuid).await.0
}

/// Joins and returns the login packet's body too.
async fn join_with_login(addr: SocketAddr, name: &str, uuid: u128) -> (TcpStream, Vec<u8>) {
    let mut client = TcpStream::connect(addr).await.unwrap();
    send(&mut client, handshake(2)).await;
    send(
        &mut client,
        Writer::packet(login::serverbound::HELLO)
            .string(name)
            .uuid(uuid),
    )
    .await;
    let body = read_until(&mut client, login::clientbound::LOGIN_FINISHED).await;
    let mut r = Reader::new(&body);
    assert_eq!(r.uuid().unwrap(), uuid);
    assert_eq!(r.string(16).unwrap(), name);
    send(
        &mut client,
        Writer::packet(login::serverbound::LOGIN_ACKNOWLEDGED),
    )
    .await;

    read_until(&mut client, configuration::clientbound::SELECT_KNOWN_PACKS).await;
    send(
        &mut client,
        Writer::packet(configuration::serverbound::SELECT_KNOWN_PACKS)
            .varint(1)
            .string("minecraft")
            .string("core")
            .string(levitas_protocol::GAME_VERSION),
    )
    .await;
    read_until(
        &mut client,
        configuration::clientbound::FINISH_CONFIGURATION,
    )
    .await;
    send(
        &mut client,
        Writer::packet(configuration::serverbound::FINISH_CONFIGURATION),
    )
    .await;
    let login = read_until(&mut client, play::clientbound::LOGIN).await;
    (client, login)
}

#[tokio::test]
async fn status_and_ping() {
    let mut client = TcpStream::connect(server().await).await.unwrap();
    send(&mut client, handshake(1)).await;
    send(&mut client, Writer::packet(0x00)).await;

    let body = read_until(&mut client, 0x00).await;
    let status: serde_json::Value =
        serde_json::from_str(Reader::new(&body).string(32767).unwrap()).unwrap();
    assert_eq!(status["version"]["protocol"], PROTOCOL_VERSION);

    send(&mut client, Writer::packet(0x01).i64(1234)).await;
    let body = read_until(&mut client, 0x01).await;
    assert_eq!(Reader::new(&body).i64().unwrap(), 1234);
}

#[tokio::test]
async fn joins_and_streams_chunks() {
    let mut client = join(server().await, "DarkIgnite", 7).await;
    let side = 2 * view_distance() + 1;
    let body = read_until(&mut client, play::clientbound::CHUNK_BATCH_FINISHED).await;
    assert_eq!(Reader::new(&body).varint().unwrap(), side * side);
    read_until(&mut client, play::clientbound::PLAYER_POSITION).await;

    // Walking one chunk east loads one new column of chunks.
    send(
        &mut client,
        Writer::packet(play::serverbound::MOVE_PLAYER_POS)
            .f64(16.5)
            .f64(-60.0)
            .f64(0.5)
            .u8(1),
    )
    .await;
    let body = read_until(&mut client, play::clientbound::CHUNK_BATCH_FINISHED).await;
    assert_eq!(Reader::new(&body).varint().unwrap(), side);
}

#[tokio::test]
async fn generated_world_streams_real_terrain() {
    let dir = temp_world();
    let (addr, world) = server_with(dir.clone(), world::WorldKind::Noise).await;
    let mut client = join(addr, "DarkIgnite", 7).await;
    let side = 2 * view_distance() + 1;
    let (mut chunks, mut spawn_y) = (0, None);
    while chunks < side * side || spawn_y.is_none() {
        let packet = read_frame(&mut client).await.unwrap();
        let mut r = Reader::new(&packet);
        match r.varint().unwrap() {
            play::clientbound::LEVEL_CHUNK_WITH_LIGHT => chunks += 1,
            play::clientbound::PLAYER_POSITION => {
                r.varint().unwrap();
                r.f64().unwrap();
                spawn_y = Some(r.f64().unwrap());
            }
            _ => {}
        }
    }
    // Players spawn on the surface of real terrain, well above the bedrock.
    let spawn_y = spawn_y.unwrap();
    assert!(spawn_y > 0.0, "spawned at y {spawn_y}");

    // Generated chunks are saved, so the same terrain comes back after a restart.
    shutdown(&world).await;
    let regions = std::fs::read_dir(
        dir.join("dimensions")
            .join("minecraft")
            .join("overworld")
            .join("region"),
    )
    .unwrap()
    .count();
    assert!(regions > 0, "no region files saved");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn players_share_one_world() {
    let addr = server().await;
    let mut alice = join(addr, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    let mut bob = join(addr, "Bob", 2).await;

    // Each sees the other's player entity.
    let body = read_until(&mut bob, play::clientbound::ADD_ENTITY).await;
    let mut r = Reader::new(&body);
    r.varint().unwrap();
    assert_eq!(r.uuid().unwrap(), 1);
    assert_eq!(r.varint().unwrap(), entity_type::PLAYER);
    let body = read_until(&mut alice, play::clientbound::ADD_ENTITY).await;
    let mut r = Reader::new(&body);
    r.varint().unwrap();
    assert_eq!(r.uuid().unwrap(), 2);
    read_until(&mut bob, play::clientbound::PLAYER_POSITION).await;

    // In creative, Alice breaks the grass under spawn; Bob sees it disappear.
    send(
        &mut alice,
        Writer::packet(play::serverbound::CHAT_COMMAND).string("gamemode creative"),
    )
    .await;
    let grass = BlockPos::new(0, -61, 0);
    send(
        &mut alice,
        Writer::packet(play::serverbound::PLAYER_ACTION)
            .varint(0)
            .position(grass)
            .u8(1)
            .varint(5),
    )
    .await;
    let body = read_until(&mut alice, play::clientbound::BLOCK_CHANGED_ACK).await;
    assert_eq!(Reader::new(&body).varint().unwrap(), 5);
    let body = read_until(&mut bob, play::clientbound::BLOCK_UPDATE).await;
    let mut r = Reader::new(&body);
    assert_eq!(r.position().unwrap(), grass);
    assert_eq!(r.varint().unwrap(), i32::from(block::AIR));

    // Alice puts stone in her hotbar and places it back in the hole.
    let stone_item = ItemInfo::id_of("minecraft:stone").unwrap();
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CREATIVE_MODE_SLOT)
            .i16(36)
            .varint(1)
            .varint(stone_item)
            .varint(0)
            .varint(0),
    )
    .await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::USE_ITEM_ON)
            .varint(0)
            .position(BlockPos::new(0, -62, 0))
            .varint(1) // top face of the dirt below
            .f32(0.5)
            .f32(1.0)
            .f32(0.5)
            .bool(false)
            .bool(false)
            .varint(6),
    )
    .await;
    let body = read_until(&mut bob, play::clientbound::BLOCK_UPDATE).await;
    let mut r = Reader::new(&body);
    assert_eq!(r.position().unwrap(), grass);
    assert_eq!(r.varint().unwrap(), i32::from(block::STONE));

    // Chat goes to everyone.
    send(
        &mut alice,
        Writer::packet(play::serverbound::CHAT).string("hello bob"),
    )
    .await;
    loop {
        let body = read_until(&mut bob, play::clientbound::SYSTEM_CHAT).await;
        if String::from_utf8_lossy(&body).contains("<Alice> hello bob") {
            break;
        }
    }
}

#[tokio::test]
async fn survival_drops_pickup_death_and_respawn() {
    let addr = server().await;
    let mut alice = join(addr, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;

    // Grass takes 0.9 s by hand: starting does nothing, finishing in time breaks it.
    let grass = BlockPos::new(0, -61, 0);
    dig(&mut alice, grass, 900).await;
    let body = read_until(&mut alice, play::clientbound::BLOCK_UPDATE).await;
    let mut r = Reader::new(&body);
    assert_eq!(r.position().unwrap(), grass);
    assert_eq!(r.varint().unwrap(), i32::from(block::AIR));

    // It drops dirt, which Alice picks up after stepping into the hole.
    let body = read_until(&mut alice, play::clientbound::ADD_ENTITY).await;
    let mut r = Reader::new(&body);
    r.varint().unwrap();
    r.uuid().unwrap();
    assert_eq!(r.varint().unwrap(), entity_type::ITEM);
    send(
        &mut alice,
        Writer::packet(play::serverbound::MOVE_PLAYER_POS)
            .f64(0.5)
            .f64(-61.0)
            .f64(0.5)
            .u8(1),
    )
    .await;
    read_until(&mut alice, play::clientbound::TAKE_ITEM_ENTITY).await;
    let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
    loop {
        let body = read_until(&mut alice, play::clientbound::CONTAINER_SET_SLOT).await;
        let mut r = Reader::new(&body);
        let (_window, _state, slot) = (r.varint().unwrap(), r.varint().unwrap(), r.i16().unwrap());
        let (count, item) = (r.varint().unwrap(), r.varint().unwrap());
        if slot == 36 {
            assert_eq!((count, item), (1, dirt));
            break;
        }
    }

    // /kill shows the death screen; respawning sends her back to spawn.
    send(
        &mut alice,
        Writer::packet(play::serverbound::CHAT_COMMAND).string("kill"),
    )
    .await;
    read_until(&mut alice, play::clientbound::PLAYER_COMBAT_KILL).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::CLIENT_COMMAND).varint(0),
    )
    .await;
    read_until(&mut alice, play::clientbound::RESPAWN).await;
    let body = read_until(&mut alice, play::clientbound::SET_HEALTH).await;
    assert_eq!(Reader::new(&body).f32().unwrap(), 20.0);
}

/// A stack in the hashed form clients send in container clicks.
fn hashed(packet: Writer, item: i32, count: i32) -> Writer {
    if count == 0 {
        packet.bool(false)
    } else {
        packet
            .bool(true)
            .varint(item)
            .varint(count)
            .varint(0)
            .varint(0)
    }
}

/// A container click: `changed` slots as (slot, item, count), then the cursor stack.
fn click(window: i32, slot: i16, changed: &[(i16, i32, i32)], carried: (i32, i32)) -> Writer {
    let mut packet = Writer::packet(play::serverbound::CONTAINER_CLICK)
        .varint(window)
        .varint(0) // state ID
        .i16(slot)
        .u8(0) // button
        .varint(0) // mode: pickup
        .varint(changed.len() as i32);
    for &(s, item, count) in changed {
        packet = hashed(packet.i16(s), item, count);
    }
    hashed(packet, carried.0, carried.1)
}

async fn command(client: &mut TcpStream, command: &str) {
    send(
        client,
        Writer::packet(play::serverbound::CHAT_COMMAND).string(command),
    )
    .await;
}

async fn give(client: &mut TcpStream, item: &str, count: i32) {
    command(client, &format!("give {item} {count}")).await;
    loop {
        let body = read_until(client, play::clientbound::SYSTEM_CHAT).await;
        if String::from_utf8_lossy(&body).contains("Gave") {
            return;
        }
    }
}

/// Reads set-slot packets until one for `window`/`slot` arrives; returns (item, count).
async fn slot_update(client: &mut TcpStream, window: i32, slot: i16) -> (i32, i32) {
    loop {
        let body = read_until(client, play::clientbound::CONTAINER_SET_SLOT).await;
        let mut r = Reader::new(&body);
        let (w, _state, s) = (r.varint().unwrap(), r.varint().unwrap(), r.i16().unwrap());
        if (w, s) == (window, slot) {
            let count = r.varint().unwrap();
            return if count == 0 {
                (0, 0)
            } else {
                (r.varint().unwrap(), count)
            };
        }
    }
}

fn use_on(pos: BlockPos, face: i32, sequence: i32) -> Writer {
    Writer::packet(play::serverbound::USE_ITEM_ON)
        .varint(0)
        .position(pos)
        .varint(face)
        .f32(0.5)
        .f32(1.0)
        .f32(0.5)
        .bool(false)
        .bool(false)
        .varint(sequence)
}

/// Reads an open-screen packet and returns (window, menu type).
async fn opened(client: &mut TcpStream) -> (i32, i32) {
    let body = read_until(client, play::clientbound::OPEN_SCREEN).await;
    let mut r = Reader::new(&body);
    (r.varint().unwrap(), r.varint().unwrap())
}

#[tokio::test]
async fn crafting_in_the_inventory_grid() {
    let item = |name| ItemInfo::id_of(name).unwrap();
    let (log, planks) = (item("minecraft:oak_log"), item("minecraft:oak_planks"));
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "oak_log", 1).await;

    // Pick the log up from the hotbar and drop it into the grid: planks appear.
    send(&mut alice, click(0, 36, &[(36, 0, 0)], (log, 1))).await;
    send(&mut alice, click(0, 1, &[(1, log, 1)], (0, 0))).await;
    assert_eq!(slot_update(&mut alice, 0, 0).await, (planks, 4));

    // Taking the result uses up the log and puts the planks on the cursor. Items cannot
    // appear out of nowhere: a click claiming diamonds makes the server resend the real
    // inventory.
    send(
        &mut alice,
        click(0, 0, &[(0, 0, 0), (1, 0, 0)], (planks, 4)),
    )
    .await;
    send(
        &mut alice,
        click(0, 10, &[(10, item("minecraft:diamond"), 64)], (planks, 4)),
    )
    .await;
    let body = read_until(&mut alice, play::clientbound::CONTAINER_SET_CONTENT).await;
    let mut r = Reader::new(&body);
    let (window, _state, len) = (
        r.varint().unwrap(),
        r.varint().unwrap(),
        r.varint().unwrap(),
    );
    assert_eq!((window, len), (0, 46));
    let mut stacks = Vec::new();
    for _ in 0..=len {
        let count = r.varint().unwrap();
        let item = if count > 0 { r.varint().unwrap() } else { 0 };
        if count > 0 {
            r.varint().unwrap();
            r.varint().unwrap();
        }
        stacks.push((item, count));
    }
    // The click itself (putting the planks down in slot 10) happens; the claimed
    // diamonds do not.
    assert_eq!(stacks[1], (0, 0), "grid is empty");
    assert!(!stacks.contains(&(item("minecraft:diamond"), 64)));
    assert_eq!(stacks[10], (planks, 4), "planks put down");
    assert_eq!(stacks[46], (0, 0), "nothing left on the cursor");
}

#[tokio::test]
async fn chests_are_shared_and_persist() {
    let dirt = ItemInfo::id_of("minecraft:dirt").unwrap();
    let addr = server().await;
    let mut alice = join(addr, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "chest", 1).await;

    // Place the chest on the grass next to spawn, then open it.
    let chest = BlockPos::new(2, -60, 0);
    send(&mut alice, use_on(BlockPos::new(2, -61, 0), 1, 1)).await;
    let body = read_until(&mut alice, play::clientbound::BLOCK_UPDATE).await;
    assert_eq!(Reader::new(&body).position().unwrap(), chest);
    send(&mut alice, use_on(chest, 1, 2)).await;
    let (window, menu) = opened(&mut alice).await;
    assert_eq!(menu, levitas_registry::menu::GENERIC_9X3);

    // Move five dirt from the hotbar (window slot 54) into the chest's first slot.
    give(&mut alice, "dirt", 5).await;
    send(&mut alice, click(window, 54, &[(54, 0, 0)], (dirt, 5))).await;
    send(&mut alice, click(window, 0, &[(0, dirt, 5)], (0, 0))).await;

    // Bob opens the same chest and finds the dirt.
    let mut bob = join(addr, "Bob", 2).await;
    read_until(&mut bob, play::clientbound::PLAYER_POSITION).await;
    send(&mut bob, use_on(chest, 1, 1)).await;
    let (bob_window, _) = opened(&mut bob).await;
    let body = read_until(&mut bob, play::clientbound::CONTAINER_SET_CONTENT).await;
    let mut r = Reader::new(&body);
    assert_eq!(r.varint().unwrap(), bob_window);
    r.varint().unwrap(); // state ID
    assert_eq!(r.varint().unwrap(), 27 + 36);
    assert_eq!((r.varint().unwrap(), r.varint().unwrap()), (5, dirt));
}

#[tokio::test]
async fn furnaces_smelt_with_fuel() {
    let item = |name| ItemInfo::id_of(name).unwrap();
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "furnace", 1).await;
    let furnace = BlockPos::new(-2, -60, 0);
    send(&mut alice, use_on(BlockPos::new(-2, -61, 0), 1, 1)).await;
    read_until(&mut alice, play::clientbound::BLOCK_UPDATE).await;
    give(&mut alice, "raw_iron", 1).await;
    give(&mut alice, "coal", 1).await;

    send(&mut alice, use_on(furnace, 1, 2)).await;
    let (window, menu) = opened(&mut alice).await;
    assert_eq!(menu, levitas_registry::menu::FURNACE);
    // Hotbar slots 36 and 37 are window slots 30 and 31.
    let (raw_iron, coal) = (item("minecraft:raw_iron"), item("minecraft:coal"));
    send(&mut alice, click(window, 30, &[(30, 0, 0)], (raw_iron, 1))).await;
    send(&mut alice, click(window, 0, &[(0, raw_iron, 1)], (0, 0))).await;
    send(&mut alice, click(window, 31, &[(31, 0, 0)], (coal, 1))).await;
    send(&mut alice, click(window, 1, &[(1, coal, 1)], (0, 0))).await;

    // The furnace lights up, and after 10 seconds the iron is done.
    let body = read_until(&mut alice, play::clientbound::BLOCK_UPDATE).await;
    let mut r = Reader::new(&body);
    assert_eq!(r.position().unwrap(), furnace);
    let lit = r.varint().unwrap() as u16;
    assert_eq!(
        levitas_registry::BlockInfo::of(lit).property(lit, "lit"),
        Some("true")
    );
    assert_eq!(
        slot_update(&mut alice, window, 2).await,
        (item("minecraft:iron_ingot"), 1)
    );
}

/// Sends a chat message and waits for it to come back, so every packet sent before it
/// has been handled by the world.
async fn sync(client: &mut TcpStream, name: &str) {
    send(
        client,
        Writer::packet(play::serverbound::CHAT).string("sync"),
    )
    .await;
    let line = format!("<{name}> sync");
    loop {
        let body = read_until(client, play::clientbound::SYSTEM_CHAT).await;
        if String::from_utf8_lossy(&body).contains(&line) {
            return;
        }
    }
}

#[tokio::test]
async fn world_and_players_survive_a_restart() {
    let item = |name| ItemInfo::id_of(name).unwrap();
    let (dirt, stone) = (item("minecraft:dirt"), item("minecraft:stone"));
    let dir = temp_world();
    let chest = BlockPos::new(2, -60, 0);

    let (addr, world) = server_in(dir.clone()).await;
    let mut alice = join(addr, "Alice", 0xA11CE).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "chest", 1).await;
    send(&mut alice, use_on(BlockPos::new(2, -61, 0), 1, 1)).await;
    give(&mut alice, "dirt", 5).await;
    send(&mut alice, use_on(chest, 1, 2)).await;
    let (window, _) = opened(&mut alice).await;
    send(&mut alice, click(window, 54, &[(54, 0, 0)], (dirt, 5))).await;
    send(&mut alice, click(window, 0, &[(0, dirt, 5)], (0, 0))).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::CONTAINER_CLOSE).varint(window),
    )
    .await;
    give(&mut alice, "stone", 3).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::MOVE_PLAYER_POS_ROT)
            .f64(5.5)
            .f64(-60.0)
            .f64(3.5)
            .f32(45.0)
            .f32(10.0)
            .u8(1),
    )
    .await;
    sync(&mut alice, "Alice").await;
    shutdown(&world).await;
    read_until(&mut alice, play::clientbound::DISCONNECT).await;

    assert!(dir.join("level.dat").is_file());
    assert!(dir
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region")
        .join("r.0.0.mca")
        .is_file());
    for name in [
        "game_rules",
        "weather",
        "world_clocks",
        "world_gen_settings",
    ] {
        assert!(dir
            .join("data")
            .join("minecraft")
            .join(format!("{name}.dat"))
            .is_file());
    }
    assert!(dir
        .join("players")
        .join("data")
        .join(format!("{}.dat", world::uuid_string(0xA11CE)))
        .is_file());

    // A new server on the same folder has everything back.
    let (addr, _world) = server_in(dir.clone()).await;
    let mut alice = join(addr, "Alice", 0xA11CE).await;
    let body = read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    let mut r = Reader::new(&body);
    r.varint().unwrap();
    assert_eq!(
        (r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap()),
        (5.5, -60.0, 3.5)
    );
    let body = read_until(&mut alice, play::clientbound::CONTAINER_SET_CONTENT).await;
    let mut r = Reader::new(&body);
    let (_window, _state, len) = (
        r.varint().unwrap(),
        r.varint().unwrap(),
        r.varint().unwrap(),
    );
    let mut slots = Vec::new();
    for _ in 0..len {
        let count = r.varint().unwrap();
        if count > 0 {
            slots.push((r.varint().unwrap(), count));
            r.varint().unwrap();
            r.varint().unwrap();
        } else {
            slots.push((0, 0));
        }
    }
    assert_eq!(slots[36], (stone, 3), "inventory was restored");

    send(&mut alice, use_on(chest, 1, 1)).await;
    let (window, _) = opened(&mut alice).await;
    let body = read_until(&mut alice, play::clientbound::CONTAINER_SET_CONTENT).await;
    let mut r = Reader::new(&body);
    assert_eq!(r.varint().unwrap(), window);
    r.varint().unwrap();
    r.varint().unwrap();
    assert_eq!(
        (r.varint().unwrap(), r.varint().unwrap()),
        (5, dirt),
        "chest contents were restored"
    );
    let _ = std::fs::remove_dir_all(dir);
}

fn use_at(pos: BlockPos, face: i32, cursor: [f32; 3], sequence: i32) -> Writer {
    Writer::packet(play::serverbound::USE_ITEM_ON)
        .varint(0)
        .position(pos)
        .varint(face)
        .f32(cursor[0])
        .f32(cursor[1])
        .f32(cursor[2])
        .bool(false)
        .bool(false)
        .varint(sequence)
}

/// Reads block updates until one for `pos` arrives; returns the new state.
async fn block_at(client: &mut TcpStream, pos: BlockPos) -> u16 {
    loop {
        let body = read_until(client, play::clientbound::BLOCK_UPDATE).await;
        let mut r = Reader::new(&body);
        if r.position().unwrap() == pos {
            return r.varint().unwrap() as u16;
        }
    }
}

fn prop(state: u16, name: &str) -> Option<&'static str> {
    levitas_registry::BlockInfo::of(state).property(state, name)
}

async fn look(client: &mut TcpStream, yaw: f32) {
    send(
        client,
        Writer::packet(play::serverbound::MOVE_PLAYER_ROT)
            .f32(yaw)
            .f32(0.0)
            .u8(1),
    )
    .await;
}

/// Starts digging, waits `millis` like a client holding the mouse, then finishes.
async fn dig(client: &mut TcpStream, pos: BlockPos, millis: u64) {
    let action = |status: i32, sequence: i32| {
        Writer::packet(play::serverbound::PLAYER_ACTION)
            .varint(status)
            .position(pos)
            .u8(1)
            .varint(sequence)
    };
    send(client, action(0, 90)).await;
    tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
    send(client, action(2, 91)).await;
}

#[tokio::test]
async fn stairs_and_doors_follow_the_player() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    look(&mut alice, 180.0).await; // facing north
    give(&mut alice, "oak_stairs", 1).await;
    send(&mut alice, use_on(BlockPos::new(3, -61, 0), 1, 1)).await;
    let stairs = block_at(&mut alice, BlockPos::new(3, -60, 0)).await;
    assert_eq!(prop(stairs, "facing"), Some("north"));
    assert_eq!(prop(stairs, "half"), Some("bottom"));

    // A door fills two blocks and opens both halves when used.
    give(&mut alice, "oak_door", 1).await;
    send(&mut alice, use_on(BlockPos::new(5, -61, 0), 1, 2)).await;
    let lower = block_at(&mut alice, BlockPos::new(5, -60, 0)).await;
    let upper = block_at(&mut alice, BlockPos::new(5, -59, 0)).await;
    assert_eq!(
        (prop(lower, "half"), prop(upper, "half")),
        (Some("lower"), Some("upper"))
    );
    send(
        &mut alice,
        use_at(BlockPos::new(5, -59, 0), 3, [0.5, 0.5, 1.0], 3),
    )
    .await;
    assert_eq!(
        prop(block_at(&mut alice, BlockPos::new(5, -60, 0)).await, "open"),
        Some("true")
    );
    assert_eq!(
        prop(block_at(&mut alice, BlockPos::new(5, -59, 0)).await, "open"),
        Some("true")
    );
}

#[tokio::test]
async fn torches_pop_off_when_their_wall_goes() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "stone", 1).await;
    let wall = BlockPos::new(7, -60, 0);
    send(&mut alice, use_on(BlockPos::new(7, -61, 0), 1, 1)).await;
    block_at(&mut alice, wall).await;
    give(&mut alice, "torch", 1).await;
    send(&mut alice, use_at(wall, 5, [1.0, 0.5, 0.5], 2)).await;
    let torch_pos = BlockPos::new(8, -60, 0);
    let torch = block_at(&mut alice, torch_pos).await;
    assert_eq!(
        levitas_registry::BlockInfo::of(torch).name,
        "minecraft:wall_torch"
    );
    assert_eq!(prop(torch, "facing"), Some("east"));

    command(&mut alice, "gamemode creative").await;
    dig(&mut alice, wall, 0).await;
    assert_eq!(block_at(&mut alice, torch_pos).await, block::AIR);
    // The torch drops as an item.
    let body = read_until(&mut alice, play::clientbound::SET_ENTITY_DATA).await;
    let mut r = Reader::new(&body);
    r.varint().unwrap();
    let (_index, _serializer, count, item) = (
        r.u8().unwrap(),
        r.varint().unwrap(),
        r.varint().unwrap(),
        r.varint().unwrap(),
    );
    assert_eq!(
        (item, count),
        (ItemInfo::id_of("minecraft:torch").unwrap(), 1)
    );
}

#[tokio::test]
async fn double_chests_have_54_slots() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    look(&mut alice, 180.0).await;
    give(&mut alice, "chest", 2).await;
    send(&mut alice, use_on(BlockPos::new(10, -61, 0), 1, 1)).await;
    block_at(&mut alice, BlockPos::new(10, -60, 0)).await;
    send(&mut alice, use_on(BlockPos::new(11, -61, 0), 1, 2)).await;
    let second = block_at(&mut alice, BlockPos::new(11, -60, 0)).await;
    assert_ne!(prop(second, "type"), Some("single"));

    send(&mut alice, use_on(BlockPos::new(10, -60, 0), 1, 3)).await;
    let (window, menu) = opened(&mut alice).await;
    assert_eq!(menu, levitas_registry::menu::GENERIC_9X6);
    let body = read_until(&mut alice, play::clientbound::CONTAINER_SET_CONTENT).await;
    let mut r = Reader::new(&body);
    assert_eq!(r.varint().unwrap(), window);
    r.varint().unwrap();
    assert_eq!(r.varint().unwrap(), 54 + 36);
}

#[tokio::test]
async fn buttons_spring_back() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "stone", 1).await;
    let wall = BlockPos::new(13, -60, 0);
    send(&mut alice, use_on(BlockPos::new(13, -61, 0), 1, 1)).await;
    block_at(&mut alice, wall).await;
    give(&mut alice, "stone_button", 1).await;
    send(&mut alice, use_at(wall, 2, [0.5, 0.5, 0.0], 2)).await;
    let button_pos = BlockPos::new(13, -60, -1);
    let button = block_at(&mut alice, button_pos).await;
    assert_eq!(
        (prop(button, "face"), prop(button, "facing")),
        (Some("wall"), Some("north"))
    );

    send(&mut alice, use_at(button_pos, 2, [0.5, 0.5, 0.0], 3)).await;
    assert_eq!(
        prop(block_at(&mut alice, button_pos).await, "powered"),
        Some("true")
    );
    // A stone button pops back out after 20 ticks.
    assert_eq!(
        prop(block_at(&mut alice, button_pos).await, "powered"),
        Some("false")
    );
}

#[tokio::test]
async fn blocks_cannot_be_placed_inside_players() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "dirt", 1).await;
    // Alice stands at (0.5, -60, 0.5): the block above the grass under her.
    send(&mut alice, use_on(BlockPos::new(0, -61, 0), 1, 1)).await;
    assert_eq!(
        block_at(&mut alice, BlockPos::new(0, -60, 0)).await,
        block::AIR
    );
}

/// Reads sound packets until one arrives; returns its sound event ID.
async fn next_sound(client: &mut TcpStream) -> i32 {
    let body = read_until(client, play::clientbound::SOUND).await;
    Reader::new(&body).varint().unwrap() - 1
}

#[tokio::test]
async fn chests_and_barrels_sound_and_barrels_open() {
    let sound = |name| levitas_registry::sound_id(name).unwrap();
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "chest", 1).await;
    let chest = BlockPos::new(3, -60, 0);
    send(&mut alice, use_on(BlockPos::new(3, -61, 0), 1, 1)).await;
    block_at(&mut alice, chest).await;

    // The opener hears it too: chest sounds come from the server.
    send(&mut alice, use_on(chest, 1, 2)).await;
    let (window, _) = opened(&mut alice).await;
    assert_eq!(
        next_sound(&mut alice).await,
        sound("minecraft:block.chest.open")
    );
    send(
        &mut alice,
        Writer::packet(play::serverbound::CONTAINER_CLOSE).varint(window),
    )
    .await;
    assert_eq!(
        next_sound(&mut alice).await,
        sound("minecraft:block.chest.close")
    );

    give(&mut alice, "barrel", 1).await;
    let barrel = BlockPos::new(5, -60, 0);
    send(&mut alice, use_on(BlockPos::new(5, -61, 0), 1, 3)).await;
    block_at(&mut alice, barrel).await;
    send(&mut alice, use_on(barrel, 1, 4)).await;
    assert_eq!(
        prop(block_at(&mut alice, barrel).await, "open"),
        Some("true")
    );
    assert_eq!(
        next_sound(&mut alice).await,
        sound("minecraft:block.barrel.open")
    );
}

/// Reads set-slot packets for the player inventory until `slot` changes; returns the
/// item, count and damage.
async fn slot_with_damage(client: &mut TcpStream, slot: i16) -> (i32, i32, i32) {
    loop {
        let body = read_until(client, play::clientbound::CONTAINER_SET_SLOT).await;
        let mut r = Reader::new(&body);
        let (window, _state, s) = (r.varint().unwrap(), r.varint().unwrap(), r.i16().unwrap());
        if (window, s) != (0, slot) {
            continue;
        }
        let count = r.varint().unwrap();
        if count == 0 {
            return (0, 0, 0);
        }
        let item = r.varint().unwrap();
        let mut damage = 0;
        let (added, _removed) = (r.varint().unwrap(), r.varint().unwrap());
        for _ in 0..added {
            if r.varint().unwrap() == levitas_registry::component::DAMAGE {
                damage = r.varint().unwrap();
            }
        }
        return (item, count, damage);
    }
}

#[tokio::test]
async fn mining_takes_time_and_wears_tools() {
    let stone = BlockPos::new(3, -60, 0);
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "stone", 1).await;
    send(&mut alice, use_on(BlockPos::new(3, -61, 0), 1, 1)).await;
    block_at(&mut alice, stone).await;

    // Stone takes 7.5 s by hand: finishing at once is refused and the block stays.
    dig(&mut alice, stone, 0).await;
    assert_eq!(block_at(&mut alice, stone).await, block::STONE);

    // A wooden pickaxe needs 1.15 s, and loses one point of durability.
    give(&mut alice, "wooden_pickaxe", 1).await;
    dig(&mut alice, stone, 1200).await;
    assert_eq!(block_at(&mut alice, stone).await, block::AIR);
    let pickaxe = ItemInfo::id_of("minecraft:wooden_pickaxe").unwrap();
    assert_eq!(slot_with_damage(&mut alice, 36).await, (pickaxe, 1, 1));

    // Shift-clicking the pickaxe into the main inventory keeps its damage. The client
    // reports nothing here, so the server answers with the whole inventory.
    send(
        &mut alice,
        Writer::packet(play::serverbound::CONTAINER_CLICK)
            .varint(0)
            .varint(0)
            .i16(36)
            .u8(0)
            .varint(1) // quick move
            .varint(0)
            .bool(false),
    )
    .await;
    let body = read_until(&mut alice, play::clientbound::CONTAINER_SET_CONTENT).await;
    let mut r = Reader::new(&body);
    let (_window, _state, len) = (
        r.varint().unwrap(),
        r.varint().unwrap(),
        r.varint().unwrap(),
    );
    let mut slots = Vec::new();
    for _ in 0..len {
        let count = r.varint().unwrap();
        if count == 0 {
            slots.push((0, 0, 0));
            continue;
        }
        let item = r.varint().unwrap();
        let mut damage = 0;
        let (added, _removed) = (r.varint().unwrap(), r.varint().unwrap());
        for _ in 0..added {
            r.varint().unwrap(); // component type: only damage is ever sent
            damage = r.varint().unwrap();
        }
        slots.push((item, count, damage));
    }
    assert_eq!(slots[36], (0, 0, 0));
    assert_eq!(slots[9], (pickaxe, 1, 1));
}

#[tokio::test]
async fn torches_light_up_their_surroundings() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "torch", 1).await;
    send(&mut alice, use_on(BlockPos::new(3, -61, 5), 1, 1)).await;
    // The light reaches into neighboring chunks too; find the torch's own chunk.
    let body = loop {
        let body = read_until(&mut alice, play::clientbound::LIGHT_UPDATE).await;
        let mut r = Reader::new(&body);
        if (r.varint().unwrap(), r.varint().unwrap()) == (0, 0) {
            break body;
        }
    };
    let mut r = Reader::new(&body);
    r.varint().unwrap();
    r.varint().unwrap();
    // A mask is its bytes, lowest bits first.
    let mut bitset = || {
        let len = r.varint().unwrap() as usize;
        let bytes = r.bytes(len).unwrap();
        bytes
            .iter()
            .enumerate()
            .fold(0u64, |acc, (i, &b)| acc | u64::from(b) << (8 * i))
    };
    let (_sky, block_lit) = (bitset(), bitset());
    let (_sky_dark, _block_dark) = (bitset(), bitset());
    // The torch at y = -60 sits in section 0 of the world, light section 1.
    assert_ne!(block_lit & 0b10, 0, "block light of the torch's section");
    for _ in 0..r.varint().unwrap() {
        let len = r.varint().unwrap();
        r.bytes(len as usize).unwrap();
    }
    assert!(r.varint().unwrap() >= 1);
    r.varint().unwrap();
    let nibbles = r.bytes(2048).unwrap();
    // Light section 1 is the lowest one with block light, so it comes first.
    let i = (((-60i32 & 15) as usize) * 16 + 5) * 16 + 3;
    assert_eq!((nibbles[i >> 1] >> ((i & 1) * 4)) & 15, 14);
}

/// Stands at (`x` + 0.5, -60, `z` + 0.5) on the flat world, looking straight down.
async fn stand_looking_down(client: &mut TcpStream, x: i32, z: i32) {
    send(
        client,
        Writer::packet(play::serverbound::MOVE_PLAYER_POS_ROT)
            .f64(f64::from(x) + 0.5)
            .f64(-60.0)
            .f64(f64::from(z) + 0.5)
            .f32(0.0)
            .f32(90.0)
            .u8(1),
    )
    .await;
}

fn use_item(sequence: i32) -> Writer {
    Writer::packet(play::serverbound::USE_ITEM)
        .varint(0)
        .varint(sequence)
        .f32(0.0)
        .f32(90.0)
}

fn fluid_level(state: u16, fluid: &str) -> Option<u8> {
    let info = levitas_registry::BlockInfo::of(state);
    (info.name == fluid).then(|| info.property(state, "level").unwrap().parse().unwrap())
}

#[tokio::test]
async fn water_flows_from_a_bucket_and_is_scooped_back() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "water_bucket", 1).await;
    stand_looking_down(&mut alice, 5, 5).await;
    send(&mut alice, use_item(1)).await;
    let source = BlockPos::new(5, -60, 5);
    assert_eq!(
        fluid_level(block_at(&mut alice, source).await, "minecraft:water"),
        Some(0)
    );
    let (item, _) = slot_update(&mut alice, 0, 36).await;
    assert_eq!(item, ItemInfo::id_of("minecraft:bucket").unwrap());

    // A tick of flow later, water spreads one block out on every side, one level weaker.
    let side = BlockPos::new(6, -60, 5);
    assert_eq!(
        fluid_level(block_at(&mut alice, side).await, "minecraft:water"),
        Some(1)
    );

    // Scooping the source up dries the flow.
    send(&mut alice, use_item(2)).await;
    assert_eq!(block_at(&mut alice, source).await, block::AIR);
    let (item, _) = slot_update(&mut alice, 0, 36).await;
    assert_eq!(item, ItemInfo::id_of("minecraft:water_bucket").unwrap());
    // The flow weakens step by step until it is gone.
    while block_at(&mut alice, side).await != block::AIR {}
}

#[tokio::test]
async fn water_turns_a_lava_source_into_obsidian() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "gamemode creative").await;
    give(&mut alice, "lava_bucket", 1).await;
    stand_looking_down(&mut alice, 5, 5).await;
    send(&mut alice, use_item(1)).await;
    let lava = BlockPos::new(5, -60, 5);
    assert_eq!(
        fluid_level(block_at(&mut alice, lava).await, "minecraft:lava"),
        Some(0)
    );

    // Water two blocks away flows next to the lava before the lava moves.
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CARRIED_ITEM).i16(1),
    )
    .await;
    give(&mut alice, "water_bucket", 1).await;
    stand_looking_down(&mut alice, 7, 5).await;
    send(&mut alice, use_item(2)).await;
    loop {
        if block_at(&mut alice, lava).await == block::OBSIDIAN {
            break;
        }
    }
}

#[tokio::test]
async fn sand_falls_when_unsupported_below() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "gamemode creative").await;

    // Place dirt at (5, -60, 5).
    give(&mut alice, "dirt", 1).await;
    send(&mut alice, use_on(BlockPos::new(5, -61, 5), 1, 1)).await;
    assert_eq!(
        block_at(&mut alice, BlockPos::new(5, -60, 5)).await,
        block::DIRT
    );

    // Switch to hotbar 1 and place sand on top of dirt at (5, -59, 5).
    give(&mut alice, "sand", 1).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CARRIED_ITEM).i16(1),
    )
    .await;
    send(&mut alice, use_on(BlockPos::new(5, -60, 5), 1, 2)).await;
    let sand_state = block_at(&mut alice, BlockPos::new(5, -59, 5)).await;
    assert_eq!(
        levitas_registry::BlockInfo::of(sand_state).name,
        "minecraft:sand"
    );

    // Break the dirt block at (5, -60, 5).
    send(
        &mut alice,
        Writer::packet(play::serverbound::PLAYER_ACTION)
            .varint(0)
            .position(BlockPos::new(5, -60, 5))
            .u8(1)
            .varint(3),
    )
    .await;
    assert_eq!(
        block_at(&mut alice, BlockPos::new(5, -60, 5)).await,
        block::AIR
    );

    // The sand block should fall:
    // First (5, -59, 5) becomes AIR when it spawns as an entity.
    assert_eq!(
        block_at(&mut alice, BlockPos::new(5, -59, 5)).await,
        block::AIR
    );
    // Then it lands at (5, -60, 5).
    let landed = block_at(&mut alice, BlockPos::new(5, -60, 5)).await;
    assert_eq!(
        levitas_registry::BlockInfo::of(landed).name,
        "minecraft:sand"
    );
}

#[tokio::test]
async fn sand_breaks_into_item_on_torch() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "gamemode creative").await;

    // Place a torch on top of grass at (2, -60, 2).
    give(&mut alice, "torch", 1).await;
    send(&mut alice, use_on(BlockPos::new(2, -61, 2), 1, 1)).await;
    let torch_pos = BlockPos::new(2, -60, 2);
    assert_eq!(
        levitas_registry::BlockInfo::of(block_at(&mut alice, torch_pos).await).name,
        "minecraft:torch"
    );

    // Place support dirt at (3, -60, 2) and (3, -59, 2), then (2, -59, 2).
    give(&mut alice, "dirt", 1).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CARRIED_ITEM).i16(1),
    )
    .await;
    send(&mut alice, use_on(BlockPos::new(3, -61, 2), 1, 2)).await;
    block_at(&mut alice, BlockPos::new(3, -60, 2)).await;
    send(&mut alice, use_on(BlockPos::new(3, -60, 2), 1, 3)).await;
    block_at(&mut alice, BlockPos::new(3, -59, 2)).await;
    send(&mut alice, use_on(BlockPos::new(3, -59, 2), 4, 4)).await; // west face -> (2, -59, 2)
    block_at(&mut alice, BlockPos::new(2, -59, 2)).await;

    // Place sand above torch at (2, -58, 2).
    give(&mut alice, "sand", 1).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CARRIED_ITEM).i16(2),
    )
    .await;
    send(&mut alice, use_on(BlockPos::new(2, -59, 2), 1, 5)).await;
    block_at(&mut alice, BlockPos::new(2, -58, 2)).await;

    // Break support dirt at (2, -59, 2).
    send(
        &mut alice,
        Writer::packet(play::serverbound::PLAYER_ACTION)
            .varint(0)
            .position(BlockPos::new(2, -59, 2))
            .u8(1)
            .varint(6),
    )
    .await;
    assert_eq!(
        block_at(&mut alice, BlockPos::new(2, -59, 2)).await,
        block::AIR
    );

    // The sand turns into an entity: (2, -58, 2) becomes AIR.
    assert_eq!(
        block_at(&mut alice, BlockPos::new(2, -58, 2)).await,
        block::AIR
    );

    // It lands on the torch and shatters into a dropped item (entity type ITEM).
    let body = read_until(&mut alice, play::clientbound::ADD_ENTITY).await;
    let mut r = Reader::new(&body);
    r.varint().unwrap();
    r.uuid().unwrap();
    assert_eq!(r.varint().unwrap(), levitas_registry::entity_type::ITEM);
}

#[tokio::test]
async fn concrete_powder_solidifies_in_water() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "gamemode creative").await;

    // Alice stands looking down at (2, 2) to place water.
    give(&mut alice, "water_bucket", 1).await;
    stand_looking_down(&mut alice, 2, 2).await;
    sync(&mut alice, "Alice").await;
    send(&mut alice, use_item(1)).await;
    let water_pos = BlockPos::new(2, -60, 2);
    loop {
        if fluid_level(block_at(&mut alice, water_pos).await, "minecraft:water") == Some(0) {
            break;
        }
    }

    // Step aside so Alice does not obstruct placing the pillar.
    send(
        &mut alice,
        Writer::packet(play::serverbound::MOVE_PLAYER_POS_ROT)
            .f64(0.5)
            .f64(-60.0)
            .f64(0.5)
            .f32(0.0)
            .f32(0.0)
            .u8(1),
    )
    .await;
    sync(&mut alice, "Alice").await;

    // Place support dirt: (3, -60, 2) and (3, -59, 2), then (2, -59, 2).
    give(&mut alice, "dirt", 1).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CARRIED_ITEM).i16(1),
    )
    .await;
    send(&mut alice, use_on(BlockPos::new(3, -61, 2), 1, 2)).await;
    block_at(&mut alice, BlockPos::new(3, -60, 2)).await;
    send(&mut alice, use_on(BlockPos::new(3, -60, 2), 1, 3)).await;
    block_at(&mut alice, BlockPos::new(3, -59, 2)).await;
    send(&mut alice, use_on(BlockPos::new(3, -59, 2), 4, 4)).await;
    block_at(&mut alice, BlockPos::new(2, -59, 2)).await;

    // Place yellow concrete powder at (2, -58, 2).
    give(&mut alice, "yellow_concrete_powder", 1).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CARRIED_ITEM).i16(2),
    )
    .await;
    send(&mut alice, use_on(BlockPos::new(2, -59, 2), 1, 5)).await;
    block_at(&mut alice, BlockPos::new(2, -58, 2)).await;

    // Break support dirt at (2, -59, 2).
    send(
        &mut alice,
        Writer::packet(play::serverbound::PLAYER_ACTION)
            .varint(0)
            .position(BlockPos::new(2, -59, 2))
            .u8(1)
            .varint(6),
    )
    .await;

    // When the falling yellow concrete powder enters the water at (2, -60, 2),
    // it solidifies into yellow concrete!
    let solid = loop {
        let s = block_at(&mut alice, water_pos).await;
        if levitas_registry::BlockInfo::of(s).name == "minecraft:yellow_concrete" {
            break s;
        }
    };
    assert_eq!(
        levitas_registry::BlockInfo::of(solid).name,
        "minecraft:yellow_concrete"
    );
}

#[tokio::test]
async fn falling_anvil_damages_player_and_crushes_torch() {
    let addr = server().await;
    let mut alice = join(addr, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "gamemode creative").await;

    let mut bob = join(addr, "Bob", 2).await;
    read_until(&mut bob, play::clientbound::PLAYER_POSITION).await;

    // Alice places a torch on top of grass at (2, -60, 2).
    give(&mut alice, "torch", 1).await;
    send(&mut alice, use_on(BlockPos::new(2, -61, 2), 1, 1)).await;
    let torch_pos = BlockPos::new(2, -60, 2);
    assert_eq!(
        levitas_registry::BlockInfo::of(block_at(&mut alice, torch_pos).await).name,
        "minecraft:torch"
    );

    // Alice builds a support dirt pillar to y = -57, then places dirt at (2, -57, 2).
    give(&mut alice, "dirt", 64).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CARRIED_ITEM).i16(1),
    )
    .await;
    send(&mut alice, use_on(BlockPos::new(3, -61, 2), 1, 2)).await;
    block_at(&mut alice, BlockPos::new(3, -60, 2)).await;
    send(&mut alice, use_on(BlockPos::new(3, -60, 2), 1, 3)).await;
    block_at(&mut alice, BlockPos::new(3, -59, 2)).await;
    send(&mut alice, use_on(BlockPos::new(3, -59, 2), 1, 4)).await;
    block_at(&mut alice, BlockPos::new(3, -58, 2)).await;
    send(&mut alice, use_on(BlockPos::new(3, -58, 2), 1, 5)).await;
    block_at(&mut alice, BlockPos::new(3, -57, 2)).await;
    send(&mut alice, use_on(BlockPos::new(3, -57, 2), 4, 6)).await; // West face -> (2, -57, 2)
    block_at(&mut alice, BlockPos::new(2, -57, 2)).await;

    // Alice places an anvil at (2, -56, 2).
    give(&mut alice, "anvil", 1).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CARRIED_ITEM).i16(2),
    )
    .await;
    send(&mut alice, use_on(BlockPos::new(2, -57, 2), 1, 7)).await;
    block_at(&mut alice, BlockPos::new(2, -56, 2)).await;

    // Bob steps into (2.5, -60.0, 2.5) right where the anvil will land.
    send(
        &mut bob,
        Writer::packet(play::serverbound::MOVE_PLAYER_POS_ROT)
            .f64(2.5)
            .f64(-60.0)
            .f64(2.5)
            .f32(0.0)
            .f32(0.0)
            .u8(1),
    )
    .await;
    sync(&mut bob, "Bob").await;

    // Alice breaks the support dirt at (2, -57, 2).
    send(
        &mut alice,
        Writer::packet(play::serverbound::PLAYER_ACTION)
            .varint(0)
            .position(BlockPos::new(2, -57, 2))
            .u8(1)
            .varint(8),
    )
    .await;

    // Anvil falls: Bob takes damage.
    read_until(&mut bob, play::clientbound::DAMAGE_EVENT).await;
    let health_body = read_until(&mut bob, play::clientbound::SET_HEALTH).await;
    let mut r = Reader::new(&health_body);
    let health = r.f32().unwrap();
    assert!(
        health < 20.0,
        "Bob should take damage from the falling anvil (got {health})"
    );

    // Anvil lands at (2, -60, 2), crushing the torch.
    let landing = loop {
        let s = block_at(&mut alice, torch_pos).await;
        if levitas_registry::BlockInfo::of(s).name.contains("anvil") {
            break s;
        }
    };
    assert!(levitas_registry::BlockInfo::of(landing)
        .name
        .contains("anvil"));
}

/// Reads time updates until one shows a time of day accepted by `want`.
async fn day_time_until(client: &mut TcpStream, want: impl Fn(i64) -> bool) -> i64 {
    loop {
        let body = read_until(client, play::clientbound::SET_TIME).await;
        let mut r = Reader::new(&body);
        // The world's age, then the clocks; the overworld's comes first.
        let _game_time = r.i64().unwrap();
        assert!(r.varint().unwrap() >= 1, "no clocks");
        let _clock = r.varint().unwrap();
        let day_time = r.varlong().unwrap();
        if want(day_time) {
            return day_time;
        }
    }
}

#[tokio::test]
async fn days_pass_and_the_time_can_be_set() {
    let mut alice = join(server().await, "Alice", 1).await;
    // Joining tells the client the time; then it keeps being sent as the day moves on.
    let start = day_time_until(&mut alice, |_| true).await;
    day_time_until(&mut alice, |t| t > start).await;
    command(&mut alice, "time set night").await;
    day_time_until(&mut alice, |t| t.rem_euclid(24_000) == 13_000).await;
}

#[tokio::test]
async fn rain_starts_on_command() {
    let mut alice = join(server().await, "Alice", 1).await;
    command(&mut alice, "weather rain").await;
    loop {
        let body = read_until(&mut alice, play::clientbound::GAME_EVENT).await;
        if body[0] == 1 {
            break; // started raining
        }
    }
}

#[tokio::test]
async fn sleeping_through_the_night_and_respawning_at_the_bed() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "red_bed", 1).await;
    send(&mut alice, use_on(BlockPos::new(4, -61, 0), 1, 1)).await;
    let foot = BlockPos::new(4, -60, 0);
    assert!(crate::blocks::is_bed(block_at(&mut alice, foot).await));

    // In the day the bed only becomes the respawn point.
    send(&mut alice, use_on(foot, 1, 2)).await;
    command(&mut alice, "time set midnight").await;
    send(&mut alice, use_on(foot, 1, 3)).await;
    // The player lies down: their pose becomes sleeping.
    loop {
        let body = read_until(&mut alice, play::clientbound::SET_ENTITY_DATA).await;
        let mut r = Reader::new(&body);
        r.varint().unwrap();
        if (r.u8().unwrap(), r.varint().unwrap(), r.varint().unwrap()) == (6, 20, 2) {
            break;
        }
    }
    // After 100 ticks asleep the night is skipped to the next morning.
    day_time_until(&mut alice, |t| t.rem_euclid(24_000) == 0 && t > 0).await;
    read_until(&mut alice, play::clientbound::ANIMATE).await;

    // Dying brings the player back on their bed rather than at the world spawn.
    command(&mut alice, "kill").await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::CLIENT_COMMAND).varint(0),
    )
    .await;
    read_until(&mut alice, play::clientbound::RESPAWN).await;
    let body = read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    let mut r = Reader::new(&body);
    r.varint().unwrap();
    let (x, y, z) = (r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap());
    assert!(
        (x - 4.5).abs() < 2.0 && (z - 0.5).abs() < 2.0,
        "respawned at {x} {y} {z}"
    );
}

#[tokio::test]
async fn straw_beds_are_slept_in_once() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "straw_bed", 1).await;
    send(&mut alice, use_on(BlockPos::new(4, -61, 0), 1, 1)).await;
    let foot = BlockPos::new(4, -60, 0);
    assert!(crate::blocks::is_straw_bed(
        block_at(&mut alice, foot).await
    ));
    command(&mut alice, "time set midnight").await;
    send(&mut alice, use_on(foot, 1, 2)).await;
    // Sleeping works as in any bed, but no respawn point is set.
    loop {
        let body = read_until(&mut alice, play::clientbound::SET_ENTITY_DATA).await;
        let mut r = Reader::new(&body);
        r.varint().unwrap();
        if (r.u8().unwrap(), r.varint().unwrap(), r.varint().unwrap()) == (6, 20, 2) {
            break;
        }
    }
    day_time_until(&mut alice, |t| t.rem_euclid(24_000) == 0 && t > 0).await;
    // Morning: the straw bed is used up and gone, leaving nothing behind.
    assert_eq!(block_at(&mut alice, foot).await, block::AIR);
}

#[tokio::test]
async fn chunks_carry_heightmaps_for_rain() {
    let mut alice = join(server().await, "Alice", 1).await;
    let body = read_until(&mut alice, play::clientbound::LEVEL_CHUNK_WITH_LIGHT).await;
    let mut r = Reader::new(&body);
    r.i32().unwrap();
    r.i32().unwrap();
    let mut kinds = Vec::new();
    for _ in 0..r.varint().unwrap() {
        let kind = r.varint().unwrap();
        let longs: Vec<i64> = (0..r.varint().unwrap()).map(|_| r.i64().unwrap()).collect();
        // The flat world's grass at y = -61 is 4 blocks above the bottom of the world.
        assert_eq!(longs[0] & 0x1FF, 4, "heightmap {kind}");
        kinds.push(kind);
    }
    assert_eq!(kinds, [1, 4, 5]);
}

#[tokio::test]
async fn joining_players_get_the_command_tree() {
    let mut alice = join(server().await, "Alice", 1).await;
    let body = read_until(&mut alice, play::clientbound::COMMANDS).await;
    let mut r = Reader::new(&body);
    let count = r.varint().unwrap() as usize;
    let mut names = Vec::new();
    let mut root_children = Vec::new();
    for i in 0..count {
        let flags = r.u8().unwrap();
        let children: Vec<usize> = (0..r.varint().unwrap())
            .map(|_| r.varint().unwrap() as usize)
            .collect();
        assert!(
            children.iter().all(|&c| c < count),
            "node {i} points outside"
        );
        if flags & 0x08 != 0 {
            assert!((r.varint().unwrap() as usize) < count);
        }
        let name = match flags & 0x03 {
            0 => {
                root_children = children;
                String::new()
            }
            1 => r.string(32767).unwrap().to_owned(),
            _ => {
                let name = r.string(32767).unwrap().to_owned();
                let parser = r.varint().unwrap();
                if parser == levitas_registry::command_argument::BRIGADIER_INTEGER {
                    let limits = r.u8().unwrap();
                    for _ in 0..(limits & 1) + (limits >> 1 & 1) {
                        r.i32().unwrap();
                    }
                } else if parser == levitas_registry::command_argument::RESOURCE {
                    r.string(32767).unwrap(); // the registry
                } else if parser == levitas_registry::command_argument::BRIGADIER_STRING {
                    r.varint().unwrap(); // word, quotable or greedy
                }
                name
            }
        };
        names.push(name);
    }
    assert_eq!(r.varint().unwrap(), 0, "root index");
    assert_eq!(r.remaining(), 0);
    let mut top: Vec<&str> = root_children.iter().map(|&c| names[c].as_str()).collect();
    top.sort_unstable();
    assert_eq!(
        top,
        [
            "ban",
            "deop",
            "difficulty",
            "effect",
            "execute",
            "experience",
            "gamemode",
            "gamerule",
            "give",
            "gm",
            "help",
            "kick",
            "kill",
            "list",
            "op",
            "pardon",
            "say",
            "summon",
            "teleport",
            "time",
            "tp",
            "weather",
            "whitelist",
            "xp"
        ]
    );
}

/// Reads entity spawns until a mob of type `kind` appears; returns its entity ID.
async fn spawned(client: &mut TcpStream, kind: i32) -> i32 {
    loop {
        let body = read_until(client, play::clientbound::ADD_ENTITY).await;
        let mut r = Reader::new(&body);
        let id = r.varint().unwrap();
        r.uuid().unwrap();
        if r.varint().unwrap() == kind {
            return id;
        }
    }
}

/// A right-click (`kind` 0) or an attack (`kind` 1) on an entity.
fn interact(target: i32, kind: i32) -> Writer {
    if kind == 1 {
        return Writer::packet(play::serverbound::ATTACK).varint(target);
    }
    Writer::packet(play::serverbound::INTERACT)
        .varint(target)
        .varint(0) // main hand
        .lp_vec3([0.0, 1.0, 0.0])
        .bool(false)
}

/// Reads dropped item data until an item accepted by `want` shows up.
async fn dropped_item(client: &mut TcpStream, want: impl Fn(&str) -> bool) -> (i32, i32) {
    loop {
        let body = read_until(client, play::clientbound::SET_ENTITY_DATA).await;
        let mut r = Reader::new(&body);
        r.varint().unwrap();
        if (r.u8().unwrap(), r.varint().unwrap()) != (8, 7) {
            continue; // not an item entity's stack
        }
        let count = r.varint().unwrap();
        let item = r.varint().unwrap();
        if want(ItemInfo::get(item).unwrap().name) {
            return (item, count);
        }
    }
}

#[tokio::test]
async fn cows_can_be_killed_for_beef() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "diamond_sword", 1).await;
    command(&mut alice, "summon cow").await;
    let cow = spawned(&mut alice, entity_type::COW).await;
    // A diamond sword deals 7 at full charge; a cow has 10 health.
    // Knocked back, the cow runs; a few swings make sure two land.
    for _ in 0..4 {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        send(&mut alice, interact(cow, 1)).await;
    }
    loop {
        let body = read_until(&mut alice, play::clientbound::ENTITY_EVENT).await;
        let mut r = Reader::new(&body);
        if (r.i32().unwrap(), r.u8().unwrap()) == (cow, 3) {
            break; // dying
        }
    }
    let (_, count) = dropped_item(&mut alice, |name| name == "minecraft:beef").await;
    assert!((1..=3).contains(&count));
}

#[tokio::test]
async fn sheep_can_be_sheared() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "shears", 1).await;
    command(&mut alice, "summon sheep").await;
    let sheep = spawned(&mut alice, entity_type::SHEEP).await;
    send(&mut alice, interact(sheep, 0)).await;
    let (_, count) = dropped_item(&mut alice, |name| name.ends_with("_wool")).await;
    assert!((1..=3).contains(&count));
}

#[tokio::test]
async fn animals_are_saved_with_the_world() {
    let dir = temp_world();
    let (addr, world) = server_in(dir.clone()).await;
    let mut alice = join(addr, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "summon pig").await;
    spawned(&mut alice, entity_type::PIG).await;
    shutdown(&world).await;
    assert!(dir
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("entities")
        .is_dir());

    let (addr, _world) = server_in(dir.clone()).await;
    let mut alice = join(addr, "Alice", 1).await;
    spawned(&mut alice, entity_type::PIG).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn generated_chunks_come_with_animals() {
    let dir = temp_world();
    let (addr, _world) = server_with(dir.clone(), world::WorldKind::Noise).await;
    let mut alice = join(addr, "Alice", 1).await;
    let animals = [
        entity_type::COW,
        entity_type::PIG,
        entity_type::SHEEP,
        entity_type::CHICKEN,
    ];
    let found = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let body = read_until(&mut alice, play::clientbound::ADD_ENTITY).await;
            let mut r = Reader::new(&body);
            r.varint().unwrap();
            r.uuid().unwrap();
            if animals.contains(&r.varint().unwrap()) {
                break;
            }
        }
    })
    .await;
    assert!(found.is_ok(), "no animals near spawn");
    let _ = std::fs::remove_dir_all(dir);
}

/// Reads health updates until the player's health drops below `below`.
async fn health_below(client: &mut TcpStream, below: f32) -> f32 {
    loop {
        let body = read_until(client, play::clientbound::SET_HEALTH).await;
        let health = Reader::new(&body).f32().unwrap();
        if health < below {
            return health;
        }
    }
}

#[tokio::test]
async fn zombies_attack_players_at_night() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "time set midnight").await;
    command(&mut alice, "summon zombie").await;
    // A zombie hits for 3 (it may strike in the very tick it appears).
    let health = health_below(&mut alice, 20.0).await;
    assert_eq!(health, 17.0);
}

#[tokio::test]
async fn creepers_explode_next_to_players() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "time set midnight").await;
    command(&mut alice, "summon creeper").await;
    spawned(&mut alice, entity_type::CREEPER).await;
    // After hissing it blows the grass under the player away, hurts the player, and
    // everyone near sees the explosion.
    assert_eq!(
        block_at(&mut alice, BlockPos::new(0, -61, 0)).await,
        block::AIR
    );
    health_below(&mut alice, 20.0).await;
    let body = read_until(&mut alice, play::clientbound::EXPLODE).await;
    let mut r = Reader::new(&body);
    let centre = (r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap());
    assert!(
        (centre.0 - 0.5).abs() < 3.0 && (centre.2 - 0.5).abs() < 3.0,
        "{centre:?}"
    );
}

#[tokio::test]
async fn monsters_spawn_in_the_dark() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "time set midnight").await;
    let monsters = [
        entity_type::ZOMBIE,
        entity_type::SPIDER,
        entity_type::CREEPER,
    ];
    let found = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let body = read_until(&mut alice, play::clientbound::ADD_ENTITY).await;
            let mut r = Reader::new(&body);
            r.varint().unwrap();
            r.uuid().unwrap();
            if monsters.contains(&r.varint().unwrap()) {
                break;
            }
        }
    })
    .await;
    assert!(found.is_ok(), "no monsters spawned at midnight");
}

/// Reads damage events until one for entity `target`.
async fn damaged(client: &mut TcpStream, target: i32) {
    loop {
        let body = read_until(client, play::clientbound::DAMAGE_EVENT).await;
        if Reader::new(&body).varint().unwrap() == target {
            return;
        }
    }
}

#[tokio::test]
async fn bows_shoot_arrows_that_hurt_mobs() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "bow", 1).await;
    give(&mut alice, "arrow", 8).await;
    let stand = |z: f64, pitch: f32| {
        Writer::packet(play::serverbound::MOVE_PLAYER_POS_ROT)
            .f64(0.5)
            .f64(-60.0)
            .f64(z)
            .f32(0.0)
            .f32(pitch)
            .u8(1)
    };
    // Arrows spread a little, like vanilla, and cows wander, so allow a few shots, each
    // at a fresh cow summoned where the player stood, from six blocks back.
    for sequence in 1..=6 {
        send(&mut alice, stand(0.5, 0.0)).await;
        command(&mut alice, "summon cow").await;
        let cow = spawned(&mut alice, entity_type::COW).await;
        send(&mut alice, stand(-5.5, 8.0)).await;
        send(&mut alice, use_item(sequence)).await;
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        send(
            &mut alice,
            Writer::packet(play::serverbound::PLAYER_ACTION)
                .varint(5) // release the item in use
                .position(BlockPos::new(0, 0, 0))
                .u8(0)
                .varint(sequence),
        )
        .await;
        spawned(&mut alice, entity_type::ARROW).await;
        let hit =
            tokio::time::timeout(std::time::Duration::from_secs(2), damaged(&mut alice, cow)).await;
        if hit.is_ok() {
            return;
        }
    }
    panic!("six arrows missed their cows");
}

#[tokio::test]
async fn skeletons_shoot_players() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "time set midnight").await;
    command(&mut alice, "summon skeleton").await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::MOVE_PLAYER_POS)
            .f64(0.5)
            .f64(-60.0)
            .f64(8.5)
            .u8(1),
    )
    .await;
    spawned(&mut alice, entity_type::ARROW).await;
    health_below(&mut alice, 20.0).await;
}

/// Reads experience updates until one accepted by `want`: (bar, level, total).
async fn experience_until(client: &mut TcpStream, want: impl Fn(f32, i32, i32) -> bool) {
    loop {
        let body = read_until(client, play::clientbound::SET_EXPERIENCE).await;
        let mut r = Reader::new(&body);
        let (bar, level, total) = (r.f32().unwrap(), r.varint().unwrap(), r.varint().unwrap());
        if want(bar, level, total) {
            return;
        }
    }
}

#[tokio::test]
async fn killing_mobs_gives_experience() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "diamond_sword", 1).await;
    command(&mut alice, "summon cow").await;
    let cow = spawned(&mut alice, entity_type::COW).await;
    // Knocked back, the cow runs; a few swings make sure two land.
    for _ in 0..4 {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        send(&mut alice, interact(cow, 1)).await;
    }
    let orb = spawned(&mut alice, entity_type::EXPERIENCE_ORB).await;
    // The orb drops right next to the player, who absorbs it: it flies in, then goes.
    // (The beef may be picked up first.)
    loop {
        let body = read_until(&mut alice, play::clientbound::TAKE_ITEM_ENTITY).await;
        if Reader::new(&body).varint().unwrap() == orb {
            break;
        }
    }
    loop {
        let body = read_until(&mut alice, play::clientbound::REMOVE_ENTITIES).await;
        let mut r = Reader::new(&body);
        assert_eq!(r.varint().unwrap(), 1);
        if r.varint().unwrap() == orb {
            break;
        }
    }
    experience_until(&mut alice, |_, _, total| (1..=3).contains(&total)).await;
}

#[tokio::test]
async fn experience_levels_and_survives_a_restart() {
    let dir = temp_world();
    let (addr, world) = server_in(dir.clone()).await;
    let mut alice = join(addr, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "xp add 10").await;
    // 7 points make level 1; 3 of the 9 for level 2 remain.
    experience_until(&mut alice, |bar, level, total| {
        level == 1 && total == 10 && (bar - 3.0 / 9.0).abs() < 1e-4
    })
    .await;
    shutdown(&world).await;

    let (addr, _world) = server_in(dir.clone()).await;
    let mut alice = join(addr, "Alice", 1).await;
    experience_until(&mut alice, |_, level, total| level == 1 && total == 10).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn hoes_till_and_bone_meal_grows_crops() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "wooden_hoe", 1).await;
    let soil = BlockPos::new(3, -61, 0);
    send(&mut alice, use_on(soil, 1, 1)).await;
    let tilled = block_at(&mut alice, soil).await;
    assert_eq!(
        levitas_registry::BlockInfo::of(tilled).name,
        "minecraft:farmland"
    );
    // Plant wheat with the seeds in the next hotbar slot, then bone meal it.
    send(
        &mut alice,
        Writer::packet(play::serverbound::SET_CARRIED_ITEM).i16(1),
    )
    .await;
    give(&mut alice, "wheat_seeds", 1).await;
    send(&mut alice, use_on(soil, 1, 2)).await;
    let crop = BlockPos::new(3, -60, 0);
    assert_eq!(prop(block_at(&mut alice, crop).await, "age"), Some("0"));
    // The seeds are used up, so the bone meal lands in the selected slot.
    give(&mut alice, "bone_meal", 1).await;
    send(&mut alice, use_on(crop, 1, 3)).await;
    let grown: u8 = prop(block_at(&mut alice, crop).await, "age")
        .unwrap()
        .parse()
        .unwrap();
    assert!((2..=5).contains(&grown), "age {grown}");
}

#[tokio::test]
async fn fed_animals_breed_babies() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "wheat", 2).await;
    command(&mut alice, "summon cow").await;
    let first = spawned(&mut alice, entity_type::COW).await;
    command(&mut alice, "summon cow").await;
    let second = spawned(&mut alice, entity_type::COW).await;
    send(&mut alice, interact(first, 0)).await;
    send(&mut alice, interact(second, 0)).await;
    // Side by side and in love, they breed: the experience drops at once, then the
    // calf appears.
    spawned(&mut alice, entity_type::EXPERIENCE_ORB).await;
    let calf = loop {
        let id = spawned(&mut alice, entity_type::COW).await;
        if id != first && id != second {
            break id;
        }
    };
    loop {
        let body = read_until(&mut alice, play::clientbound::SET_ENTITY_DATA).await;
        let mut r = Reader::new(&body);
        if r.varint().unwrap() == calf && (r.u8().unwrap(), r.varint().unwrap()) == (16, 8) {
            assert!(r.bool().unwrap(), "the calf is a baby");
            break;
        }
    }
}

#[tokio::test]
async fn lava_burns_players() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "lava_bucket", 1).await;
    stand_looking_down(&mut alice, 0, 0).await;
    send(&mut alice, use_item(1)).await;
    // Lava sets the player alight, then hurts for 4 (in the same tick).
    loop {
        let body = read_until(&mut alice, play::clientbound::SET_ENTITY_DATA).await;
        let mut r = Reader::new(&body);
        r.varint().unwrap();
        if (r.u8().unwrap(), r.varint().unwrap()) == (0, 0) {
            assert_ne!(r.u8().unwrap() & 0x01, 0, "on fire");
            break;
        }
    }
    assert_eq!(health_below(&mut alice, 20.0).await, 16.0);
}

#[tokio::test]
async fn players_can_hit_each_other() {
    let addr = server().await;
    let mut alice = join(addr, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    let mut bob = join(addr, "Bob", 2).await;
    read_until(&mut bob, play::clientbound::PLAYER_POSITION).await;
    let bob_id = spawned(&mut alice, entity_type::PLAYER).await;
    give(&mut alice, "diamond_sword", 1).await;

    // A fully charged diamond sword deals 7 and knocks Bob back.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    send(&mut alice, interact(bob_id, 1)).await;
    assert_eq!(health_below(&mut bob, 20.0).await, 13.0);
    read_until(&mut bob, play::clientbound::SET_ENTITY_MOTION).await;

    // Hit again at once: a weak blow does not get through the invulnerability.
    send(&mut alice, interact(bob_id, 1)).await;
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    send(&mut alice, interact(bob_id, 1)).await;
    // Bob may have healed a little from saturation in between.
    let health = health_below(&mut bob, 13.0).await;
    assert!((6.0..7.0).contains(&health), "{health}");
}

#[tokio::test]
async fn flint_and_steel_lights_tnt() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "tnt", 1).await;
    let ground = BlockPos::new(3, -61, 0);
    send(&mut alice, use_on(ground, 1, 1)).await;
    let tnt = BlockPos::new(3, -60, 0);
    assert_eq!(
        levitas_registry::BlockInfo::of(block_at(&mut alice, tnt).await).name,
        "minecraft:tnt"
    );
    give(&mut alice, "flint_and_steel", 1).await;
    send(&mut alice, use_on(tnt, 1, 2)).await;
    assert_eq!(block_at(&mut alice, tnt).await, block::AIR);
    spawned(&mut alice, entity_type::TNT).await;
    read_until(&mut alice, play::clientbound::EXPLODE).await;
}

/// Reads effect updates until one for `effect` arrives; returns its amplifier.
async fn effect_added(client: &mut TcpStream, effect: i32) -> i32 {
    loop {
        let body = read_until(client, play::clientbound::UPDATE_MOB_EFFECT).await;
        let mut r = Reader::new(&body);
        r.varint().unwrap();
        if r.varint().unwrap() == effect {
            return r.varint().unwrap();
        }
    }
}

#[tokio::test]
async fn golden_apples_give_effects_and_milk_clears_them() {
    use levitas_registry::mob_effect;
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "golden_apple", 1).await;
    send(&mut alice, use_item(1)).await;
    assert_eq!(effect_added(&mut alice, mob_effect::REGENERATION).await, 1);
    assert_eq!(effect_added(&mut alice, mob_effect::ABSORPTION).await, 0);

    give(&mut alice, "milk_bucket", 1).await;
    send(&mut alice, use_item(2)).await;
    // The empty bucket is left in hand, then the effects go.
    loop {
        let body = read_until(&mut alice, play::clientbound::CONTAINER_SET_SLOT).await;
        let mut r = Reader::new(&body);
        r.varint().unwrap();
        r.varint().unwrap();
        r.i16().unwrap();
        if r.varint().unwrap() == 1 {
            let item = r.varint().unwrap();
            if ItemInfo::get(item).unwrap().name == "minecraft:bucket" {
                break;
            }
        }
    }
    let body = read_until(&mut alice, play::clientbound::REMOVE_MOB_EFFECT).await;
    let mut r = Reader::new(&body);
    r.varint().unwrap();
    assert!([mob_effect::REGENERATION, mob_effect::ABSORPTION].contains(&r.varint().unwrap()));
}

#[tokio::test]
async fn poison_hurts_but_never_kills() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(&mut alice, "effect give poison 60 4").await;
    effect_added(&mut alice, levitas_registry::mob_effect::POISON).await;
    // Level V poison hurts every tick, down to one health and no further (natural
    // regeneration may lift it a little in between).
    let health = health_below(&mut alice, 1.5).await;
    assert!((1.0..1.5).contains(&health), "{health}");
}

/// The dimension name a login or respawn packet's spawn information names.
fn spawn_dimension(r: &mut Reader<'_>) -> String {
    r.varint().unwrap(); // dimension type
    r.string(256).unwrap().to_owned()
}

#[tokio::test]
async fn players_travel_to_the_nether_and_respawn_in_the_overworld() {
    let addr = server().await;
    let mut alice = join(addr, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    command(
        &mut alice,
        "execute in minecraft:the_nether run tp 8.5 70 8.5",
    )
    .await;
    let body = read_until(&mut alice, play::clientbound::RESPAWN).await;
    assert_eq!(
        spawn_dimension(&mut Reader::new(&body)),
        "minecraft:the_nether"
    );
    // Nether chunks follow once generated.
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    read_until(&mut alice, play::clientbound::LEVEL_CHUNK_WITH_LIGHT).await;

    // Logging out and back in keeps her in the Nether.
    drop(alice);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (mut alice, body) = join_with_login(addr, "Alice", 1).await;
    let mut r = Reader::new(&body);
    r.i32().unwrap();
    r.bool().unwrap();
    for _ in 0..r.varint().unwrap() {
        r.string(256).unwrap();
    }
    for _ in 0..3 {
        r.varint().unwrap();
    }
    for _ in 0..3 {
        r.bool().unwrap();
    }
    assert_eq!(spawn_dimension(&mut r), "minecraft:the_nether");
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;

    // Dying there brings her back to the overworld.
    command(&mut alice, "kill").await;
    read_until(&mut alice, play::clientbound::PLAYER_COMBAT_KILL).await;
    send(
        &mut alice,
        Writer::packet(play::serverbound::CLIENT_COMMAND).varint(0),
    )
    .await;
    let body = read_until(&mut alice, play::clientbound::RESPAWN).await;
    assert_eq!(
        spawn_dimension(&mut Reader::new(&body)),
        "minecraft:overworld"
    );
}

#[tokio::test]
async fn ender_pearls_carry_their_thrower_for_five_damage() {
    let mut alice = join(server().await, "Alice", 1).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    give(&mut alice, "ender_pearl", 1).await;
    stand_looking_down(&mut alice, 4, 4).await;
    send(&mut alice, use_item(1)).await;
    spawned(&mut alice, entity_type::ENDER_PEARL).await;
    read_until(&mut alice, play::clientbound::PLAYER_POSITION).await;
    assert_eq!(health_below(&mut alice, 20.0).await, 15.0);
}
