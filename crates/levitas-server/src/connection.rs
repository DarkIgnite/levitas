//! Per-connection protocol handling up to the play state: handshake, server list status,
//! login (offline mode) and configuration.

use std::io;

use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use levitas_protocol::{varint, Reader, Writer, GAME_VERSION, MAX_PACKET_LEN, PROTOCOL_VERSION};
use levitas_registry::packet::{configuration, login as login_ids};
use levitas_registry::{SYNCED_REGISTRIES, TAGS};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::info;

use crate::auth::{self, encrypt, Decrypting, Encryptor, Property};
use crate::play;
use crate::world::Event;

/// Intent sent at the end of the handshake.
const INTENT_STATUS: i32 = 1;
const INTENT_LOGIN: i32 = 2;
const INTENT_TRANSFER: i32 = 3;

/// What packets are read from: decrypted once online mode sets that up.
pub type ClientReader = Decrypting<BufReader<OwnedReadHalf>>;

/// A client connection, encrypted and compressed once login sets that up.
pub struct Conn {
    pub reader: ClientReader,
    pub out: Outgoing,
    /// Whether packets from the client come compressed.
    pub compressed: bool,
    /// What the client said about itself: the skin layers it shows (bit mask) and its
    /// main hand (0 left, 1 right).
    pub skin_parts: u8,
    pub main_hand: u8,
}

impl Conn {
    fn new(stream: TcpStream) -> Self {
        let (read, write) = stream.into_split();
        Self {
            reader: Decrypting::new(BufReader::new(read)),
            out: Outgoing {
                writer: BufWriter::new(write),
                compression: None,
                cipher: None,
            },
            compressed: false,
            skin_parts: 0,
            main_hand: 1,
        }
    }

    pub async fn read(&mut self) -> io::Result<Vec<u8>> {
        read_packet(&mut self.reader, self.compressed).await
    }

    pub async fn queue(&mut self, packet: Writer) -> io::Result<()> {
        self.out.queue(packet).await
    }

    pub async fn send(&mut self, packet: Writer) -> io::Result<()> {
        self.out.send(packet).await
    }
}

/// The sending half of a connection. Packets are buffered until flushed.
pub struct Outgoing {
    writer: BufWriter<OwnedWriteHalf>,
    /// Packets at least this long go out compressed, once compression is on.
    compression: Option<usize>,
    /// Encrypts everything sent, once online mode sets it up.
    cipher: Option<Encryptor>,
}

impl Outgoing {
    /// Queues a packet; call [`Outgoing::flush`] to send it.
    pub async fn queue(&mut self, packet: Writer) -> io::Result<()> {
        self.write_frame(&packet.into_frame()).await
    }

    /// Queues an already length-prefixed packet, compressing it if that is on.
    pub async fn write_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        let mut bytes = match self.compression {
            Some(threshold) => compress_frame(frame, threshold)?,
            None => frame.to_vec(),
        };
        if let Some(cipher) = self.cipher.as_mut() {
            encrypt(cipher, &mut bytes);
        }
        self.writer.write_all(&bytes).await
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.writer.flush().await
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
    /// The skin, from Mojang in online mode.
    pub properties: Vec<Property>,
}

pub async fn handle(stream: TcpStream, world: mpsc::Sender<Event>) -> io::Result<()> {
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
        "players": {
            "max": crate::config::get().max_players,
            "online": crate::world::online(),
            "sample": []
        },
        "description": { "text": crate::config::get().motd },
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

    // Online mode: the client proves who it is through Mojang, and from here on all
    // goes encrypted. Offline, the name and UUID it sent are trusted.
    let (name, uuid, properties) = if crate::config::get().online_mode {
        match authenticate(conn, &name).await? {
            Some(profile) => profile,
            None => {
                let message = json!({ "text": "Failed to verify username!" }).to_string();
                conn.send(
                    Writer::packet(login_ids::clientbound::LOGIN_DISCONNECT).string(&message),
                )
                .await?;
                info!(name, "failed to verify with Mojang");
                return Ok(None);
            }
        }
    } else {
        (name, uuid, Vec::new())
    };

    // Banned, not on the whitelist, or no room: turned away.
    let full = crate::world::online() >= crate::config::get().max_players as i32
        && !crate::access::is_op(&name, uuid);
    let refusal = crate::access::refusal(&name, uuid)
        .or_else(|| full.then(|| "The server is full!".to_owned()));
    if let Some(reason) = refusal {
        info!(name, "turned away: {reason}");
        let message = json!({ "text": reason }).to_string();
        conn.send(Writer::packet(login_ids::clientbound::LOGIN_DISCONNECT).string(&message))
            .await?;
        return Ok(None);
    }

    // From here on, long packets are compressed both ways.
    let threshold = crate::config::get().compression_threshold;
    if threshold >= 0 {
        conn.send(Writer::packet(login_ids::clientbound::LOGIN_COMPRESSION).varint(threshold))
            .await?;
        conn.out.compression = Some(threshold as usize);
        conn.compressed = true;
    }

    // The game profile, then an ID for the session.
    let mut packet = Writer::packet(login_ids::clientbound::LOGIN_FINISHED)
        .uuid(uuid)
        .string(&name);
    packet = write_properties(packet, &properties).uuid(auth::session_id());
    conn.send(packet).await?;
    let packet = conn.read().await?;
    expect_id(
        &mut Reader::new(&packet),
        login_ids::serverbound::LOGIN_ACKNOWLEDGED,
        "login acknowledged",
    )?;
    info!(name, "logged in");
    Ok(Some(Profile {
        name,
        uuid,
        properties,
    }))
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
    wait_in_configuration(conn, serverbound::SELECT_KNOWN_PACKS).await?;

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
    wait_in_configuration(conn, serverbound::FINISH_CONFIGURATION).await
}

/// Like `wait_for`, but notes the client's settings when they go by.
async fn wait_in_configuration(conn: &mut Conn, id: i32) -> io::Result<()> {
    loop {
        let packet = conn.read().await?;
        let mut r = Reader::new(&packet);
        let got = r.varint()?;
        if got == configuration::serverbound::CLIENT_INFORMATION {
            if let Some((skin, hand)) = client_settings(&mut r) {
                conn.skin_parts = skin;
                conn.main_hand = hand;
            }
        } else if got == id {
            return Ok(());
        }
    }
}

/// The skin layers and main hand from a client information packet (after its ID).
pub fn client_settings(r: &mut Reader) -> Option<(u8, u8)> {
    r.string(16).ok()?; // locale
    r.u8().ok()?; // view distance
    r.varint().ok()?; // chat mode
    r.bool().ok()?; // chat colours
    let skin = r.u8().ok()?;
    let hand = r.varint().ok()?;
    Some((skin, u8::from(hand != 0)))
}

/// Online mode's handshake: the encryption request and response, then the session
/// server's word. The player's real UUID, name and skin if Mojang knows them.
async fn authenticate(
    conn: &mut Conn,
    name: &str,
) -> io::Result<Option<(String, u128, Vec<Property>)>> {
    let keys = auth::keys();
    let token = auth::verify_token();
    conn.send(
        Writer::packet(login_ids::clientbound::HELLO)
            .string("") // server ID, empty since 1.7
            .varint(keys.public_der.len() as i32)
            .bytes(&keys.public_der)
            .varint(token.len() as i32)
            .bytes(&token)
            .bool(true), // the client should authenticate with Mojang
    )
    .await?;
    let packet = conn.read().await?;
    let mut r = Reader::new(&packet);
    expect_id(&mut r, login_ids::serverbound::KEY, "encryption response")?;
    let secret_len = r.varint()?;
    let secret = keys.decrypt(r.bytes(usize::try_from(secret_len).unwrap_or(0))?)?;
    let token_len = r.varint()?;
    let echoed = keys.decrypt(r.bytes(usize::try_from(token_len).unwrap_or(0))?)?;
    if echoed != token {
        return Err(invalid("the verify token does not match".to_owned()));
    }
    let (encryptor, decryptor) = auth::ciphers(&secret)?;
    conn.out.cipher = Some(encryptor);
    conn.reader.cipher = Some(decryptor);
    let hash = auth::server_hash(&secret, &keys.public_der);
    Ok(auth::has_joined(name, &hash)
        .await?
        .map(|(uuid, name, properties)| (name, uuid, properties)))
}

/// A profile's properties as the login and player list packets carry them.
pub fn write_properties(mut packet: Writer, properties: &[Property]) -> Writer {
    packet = packet.varint(properties.len() as i32);
    for p in properties {
        packet = packet.string(&p.name).string(&p.value);
        packet = match &p.signature {
            Some(signature) => packet.bool(true).string(signature),
            None => packet.bool(false),
        };
    }
    packet
}

/// A length-prefixed packet in the compressed format: the uncompressed length (0 if
/// the packet is too short to be worth compressing) and the zlib data.
pub fn compress_frame(frame: &[u8], threshold: usize) -> io::Result<Vec<u8>> {
    let mut r = Reader::new(frame);
    let len = r.varint()? as usize;
    let body = &frame[frame.len() - len..];
    let mut inner = Vec::with_capacity(len + 5);
    if body.len() >= threshold {
        varint::write(&mut inner, body.len() as i32);
        let mut encoder = ZlibEncoder::new(inner, Compression::default());
        encoder.write_all(body)?;
        inner = encoder.finish()?;
    } else {
        varint::write(&mut inner, 0);
        inner.extend_from_slice(body);
    }
    let mut out = Vec::with_capacity(inner.len() + 5);
    varint::write(&mut out, inner.len() as i32);
    out.extend_from_slice(&inner);
    Ok(out)
}

/// The packet inside a compressed one.
pub fn decompress_packet(packet: &[u8]) -> io::Result<Vec<u8>> {
    let mut r = Reader::new(packet);
    let data_len = r.varint()?;
    let rest = &packet[packet.len() - r.remaining()..];
    if data_len == 0 {
        return Ok(rest.to_vec());
    }
    let data_len = usize::try_from(data_len)
        .ok()
        .filter(|&l| l <= MAX_UNCOMPRESSED)
        .ok_or_else(|| invalid(format!("bad uncompressed length {data_len}")))?;
    let mut out = Vec::with_capacity(data_len);
    ZlibDecoder::new(rest)
        .take(data_len as u64 + 1)
        .read_to_end(&mut out)?;
    if out.len() != data_len {
        return Err(invalid("compressed packet has the wrong length".to_owned()));
    }
    Ok(out)
}

/// The largest packet a client may send once uncompressed, as vanilla allows.
const MAX_UNCOMPRESSED: usize = 8 * 1024 * 1024;

/// Reads one packet, uncompressing it if compression is on.
pub async fn read_packet<R: AsyncRead + Unpin>(
    reader: &mut R,
    compressed: bool,
) -> io::Result<Vec<u8>> {
    let packet = read_frame(reader).await?;
    if compressed {
        decompress_packet(&packet)
    } else {
        Ok(packet)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_settings_give_the_skin_layers_and_the_main_hand() {
        let packet = Writer::packet(0)
            .string("en_us")
            .u8(12) // view distance
            .varint(0) // chat mode
            .bool(true)
            .u8(0x7F) // all layers
            .varint(0) // left hand
            .bool(false)
            .bool(true)
            .varint(0)
            .into_frame();
        let mut r = Reader::new(&packet);
        r.varint().unwrap(); // length
        r.varint().unwrap(); // packet ID
        assert_eq!(client_settings(&mut r), Some((0x7F, 0)));
    }

    #[test]
    fn compressed_packets_round_trip() {
        for size in [10usize, 5000] {
            let packet = Writer::packet(0x27).bytes(&vec![7u8; size]);
            let frame = packet.into_frame();
            let compressed = compress_frame(&frame, 256).unwrap();
            if size > 256 {
                assert!(compressed.len() < frame.len() / 4, "long packets shrink");
            }
            // Strip the outer length, as reading a frame does.
            let mut r = Reader::new(&compressed);
            let len = r.varint().unwrap() as usize;
            let inner = &compressed[compressed.len() - len..];
            let mut f = Reader::new(&frame);
            let body_len = f.varint().unwrap() as usize;
            let body = &frame[frame.len() - body_len..];
            assert_eq!(decompress_packet(inner).unwrap(), body);
        }
    }
}
