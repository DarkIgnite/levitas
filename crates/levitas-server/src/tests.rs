//! End-to-end protocol tests against a server running in-process.

use levitas_protocol::{BlockPos, Reader, Writer, PROTOCOL_VERSION};
use levitas_registry::packet::{configuration, login, play};
use levitas_registry::{block, entity_type, ItemInfo};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use crate::connection::{handle, read_frame};
use crate::world::{self, VIEW_DISTANCE};

/// Starts a server with its own world and returns its address.
async fn server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let world = world::spawn();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(handle(stream, world.clone()));
        }
    });
    addr
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
async fn join(addr: std::net::SocketAddr, name: &str, uuid: u128) -> TcpStream {
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

    // Grass is not instant to break: starting does nothing, finishing breaks it.
    let grass = BlockPos::new(0, -61, 0);
    for (status, sequence) in [(0, 1), (2, 2)] {
        send(
            &mut alice,
            Writer::packet(play::serverbound::PLAYER_ACTION)
                .varint(status)
                .position(grass)
                .u8(1)
                .varint(sequence),
        )
        .await;
    }
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
    // inventory, which shows an empty grid and the planks on the cursor.
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
    assert_eq!(stacks[1], (0, 0), "grid is empty");
    assert!(!stacks.contains(&(item("minecraft:diamond"), 64)));
    assert_eq!(stacks[46], (planks, 4), "planks on the cursor");
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
