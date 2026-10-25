//! End-to-end protocol tests against a server running in-process.

use std::sync::Arc;

use levitas_protocol::{Reader, Writer, PROTOCOL_VERSION};
use levitas_registry::packet::{configuration, login, play};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use crate::connection::{handle, read_frame};
use crate::world::FlatWorld;

async fn connect() -> TcpStream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = handle(stream, Arc::new(FlatWorld::new())).await;
    });
    TcpStream::connect(addr).await.unwrap()
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

#[tokio::test]
async fn status_and_ping() {
    let mut client = connect().await;
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
async fn joins_flat_world() {
    let mut client = connect().await;
    send(&mut client, handshake(2)).await;
    send(
        &mut client,
        Writer::packet(login::serverbound::HELLO)
            .string("DarkIgnite")
            .uuid(7),
    )
    .await;

    let body = read_until(&mut client, login::clientbound::LOGIN_FINISHED).await;
    let mut r = Reader::new(&body);
    assert_eq!(r.uuid().unwrap(), 7);
    assert_eq!(r.string(16).unwrap(), "DarkIgnite");
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
    let body = read_until(&mut client, play::clientbound::CHUNK_BATCH_FINISHED).await;
    let side = 2 * crate::play::VIEW_DISTANCE + 1;
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
