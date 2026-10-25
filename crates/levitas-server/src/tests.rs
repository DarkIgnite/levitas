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
use crate::world::{self, Event, VIEW_DISTANCE};

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
            .string("1.21.8"),
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
    read_until(&mut client, play::clientbound::LOGIN).await;
    client
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
    let side = 2 * VIEW_DISTANCE + 1;
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
    let side = 2 * VIEW_DISTANCE + 1;
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
    let regions = std::fs::read_dir(dir.join("region")).unwrap().count();
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
    assert!(dir.join("region").join("r.0.0.mca").is_file());
    assert!(dir
        .join("playerdata")
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
        for _ in 0..r.varint().unwrap() {
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
        for _ in 0..r.varint().unwrap() {
            r.varint().unwrap(); // component type: only damage is ever sent
            damage = r.varint().unwrap();
        }
        r.varint().unwrap(); // components removed
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
    let mut bitset = || {
        let longs = r.varint().unwrap();
        (0..longs).fold(0u64, |acc, _| acc | r.i64().unwrap() as u64)
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
