//! End-to-end protocol tests against a server running in-process.

use levitas_protocol::{BlockPos, Reader, Writer, PROTOCOL_VERSION};
use levitas_registry::packet::{configuration, login, play};
use levitas_registry::{block, entity_type};
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

    // Alice breaks the grass under spawn; Bob sees it disappear.
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
    let stone_item = levitas_registry::ITEM_BLOCK
        .iter()
        .position(|&b| b == block::STONE)
        .unwrap() as i32;
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
