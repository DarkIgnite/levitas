//! Per-connection protocol handling up to the play state: handshake, server list status,
//! login (offline mode) and configuration.

use std::io;
use std::sync::Arc;

use levitas_protocol::{Reader, Writer, GAME_VERSION, MAX_PACKET_LEN, PROTOCOL_VERSION};
use levitas_registry::packet::{configuration, login as login_ids};
use levitas_registry::{SYNCED_REGISTRIES, TAGS};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tracing::info;

use crate::play;
use crate::world::FlatWorld;

/// Intent sent at the end of the handshake.
const INTENT_STATUS: i32 = 1;
const INTENT_LOGIN: i32 = 2;
const INTENT_TRANSFER: i32 = 3;

/// A client connection without compression or encryption.
pub struct Conn {
    pub reader: BufReader<OwnedReadHalf>,
    pub out: Outgoing,
}

impl Conn {
    fn new(stream: TcpStream) -> Self {
        let (read, write) = stream.into_split();
        Self {
            reader: BufReader::new(read),
            out: Outgoing(BufWriter::new(write)),
        }
    }

    pub async fn read(&mut self) -> io::Result<Vec<u8>> {
        read_frame(&mut self.reader).await
    }

    pub async fn queue(&mut self, packet: Writer) -> io::Result<()> {
        self.out.queue(packet).await
    }

    pub async fn send(&mut self, packet: Writer) -> io::Result<()> {
        self.out.send(packet).await
    }
}

/// The sending half of a connection. Packets are buffered until flushed.
pub struct Outgoing(BufWriter<OwnedWriteHalf>);

impl Outgoing {
    /// Queues a packet; call [`Outgoing::flush`] to send it.
    pub async fn queue(&mut self, packet: Writer) -> io::Result<()> {
        self.0.write_all(&packet.into_frame()).await
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.0.flush().await
    }

    pub async fn send(&mut self, packet: Writer) -> io::Result<()> {
        self.queue(packet).await?;
        self.flush().await
    }
}

/// The identity a client logged in with.
pub struct Profile {
    pub name: String,
    pub uuid: u128,
}

pub async fn handle(stream: TcpStream, world: Arc<FlatWorld>) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let mut conn = Conn::new(stream);

    let packet = conn.read().await?;
    let mut r = Reader::new(&packet);
    expect_id(&mut r, 0x00, "handshake")?;
    let protocol = r.varint()?;
    let _address = r.string(255)?;
    let _port = r.u16()?;
    match r.varint()? {
        INTENT_STATUS => status(&mut conn).await,
        INTENT_LOGIN | INTENT_TRANSFER => {
            let Some(profile) = login(&mut conn, protocol).await? else {
                return Ok(());
            };
            configure(&mut conn).await?;
            play::run(conn, profile, world).await
        }
        other => Err(invalid(format!("unknown handshake intent {other}"))),
    }
}

async fn status(conn: &mut Conn) -> io::Result<()> {
    loop {
        let packet = conn.read().await?;
        let mut r = Reader::new(&packet);
        match r.varint()? {
            // Status Request
            0x00 => {
                let response = status_json().to_string();
                conn.send(Writer::packet(0x00).string(&response)).await?;
            }
            // Ping Request: echo the payload back and close.
            0x01 => {
                let payload = r.i64()?;
                return conn.send(Writer::packet(0x01).i64(payload)).await;
            }
            id => return Err(invalid(format!("unexpected status packet 0x{id:02x}"))),
        }
    }
}

fn status_json() -> serde_json::Value {
    json!({
        "version": { "name": format!("Levitas {GAME_VERSION}"), "protocol": PROTOCOL_VERSION },
        "players": { "max": 100, "online": 0, "sample": [] },
        "description": {
            "text": "Levitas",
            "color": "aqua",
            "extra": [{ "text": " - lightweight server in Rust", "color": "gray" }]
        },
        "enforcesSecureChat": false
    })
}

/// Runs the login phase. Returns `None` if the client was turned away.
async fn login(conn: &mut Conn, protocol: i32) -> io::Result<Option<Profile>> {
    let packet = conn.read().await?;
    let mut r = Reader::new(&packet);
    expect_id(&mut r, login_ids::serverbound::HELLO, "login start")?;
    let name = r.string(16)?.to_owned();
    let uuid = r.uuid()?;

    if protocol != PROTOCOL_VERSION {
        let reason =
            format!("Levitas runs Minecraft {GAME_VERSION}. Please join with that version.");
        info!(name, protocol, "login rejected: wrong version");
        let message = json!({ "text": reason }).to_string();
        conn.send(Writer::packet(login_ids::clientbound::LOGIN_DISCONNECT).string(&message))
            .await?;
        return Ok(None);
    }

    // Offline mode: trust the name and UUID the client sent.
    conn.send(
        Writer::packet(login_ids::clientbound::LOGIN_FINISHED)
            .uuid(uuid)
            .string(&name)
            .varint(0), // no profile properties (skins come with online mode)
    )
    .await?;
    let packet = conn.read().await?;
    expect_id(
        &mut Reader::new(&packet),
        login_ids::serverbound::LOGIN_ACKNOWLEDGED,
        "login acknowledged",
    )?;
    info!(name, "logged in");
    Ok(Some(Profile { name, uuid }))
}

/// Sends registries and tags, then waits for the client to finish configuration.
async fn configure(conn: &mut Conn) -> io::Result<()> {
    use configuration::{clientbound, serverbound};

    conn.queue(
        Writer::packet(clientbound::CUSTOM_PAYLOAD)
            .string("minecraft:brand")
            .string("Levitas"),
    )
    .await?;
    // Tell the client we use the vanilla data it already has, so registry entries can be
    // sent by name only.
    conn.send(
        Writer::packet(clientbound::SELECT_KNOWN_PACKS)
            .varint(1)
            .string("minecraft")
            .string("core")
            .string(GAME_VERSION),
    )
    .await?;
    wait_for(conn, serverbound::SELECT_KNOWN_PACKS).await?;

    for registry in SYNCED_REGISTRIES {
        let mut packet = Writer::packet(clientbound::REGISTRY_DATA)
            .string(registry.name)
            .varint(registry.entries.len() as i32);
        for entry in registry.entries {
            packet = packet.string(entry).bool(false);
        }
        conn.queue(packet).await?;
    }

    let mut packet = Writer::packet(clientbound::UPDATE_TAGS).varint(TAGS.len() as i32);
    for registry in TAGS {
        packet = packet
            .string(registry.registry)
            .varint(registry.tags.len() as i32);
        for (name, ids) in registry.tags {
            packet = packet.string(name).varint(ids.len() as i32);
            for &id in *ids {
                packet = packet.varint(id as i32);
            }
        }
    }
    conn.queue(packet).await?;

    conn.send(Writer::packet(clientbound::FINISH_CONFIGURATION))
        .await?;
    wait_for(conn, serverbound::FINISH_CONFIGURATION).await
}

/// Reads packets until one with `id` arrives, ignoring others (client settings, brand).
async fn wait_for(conn: &mut Conn, id: i32) -> io::Result<()> {
    loop {
        let packet = conn.read().await?;
        if Reader::new(&packet).varint()? == id {
            return Ok(());
        }
    }
}

/// Reads one length-prefixed packet from an uncompressed connection.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut len = 0usize;
    for i in 0..3 {
        let byte = reader.read_u8().await?;
        len |= usize::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            break;
        }
        if i == 2 {
            return Err(invalid("packet length prefix longer than 3 bytes".into()));
        }
    }
    if len == 0 || len > MAX_PACKET_LEN {
        return Err(invalid(format!("invalid packet length {len}")));
    }
    let mut packet = vec![0; len];
    reader.read_exact(&mut packet).await?;
    Ok(packet)
}

fn expect_id(r: &mut Reader<'_>, expected: i32, name: &str) -> io::Result<()> {
    let id = r.varint()?;
    if id != expected {
        return Err(invalid(format!(
            "expected {name} (0x{expected:02x}), got 0x{id:02x}"
        )));
    }
    Ok(())
}

pub fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
