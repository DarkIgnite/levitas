//! Per-connection protocol handling: handshake, server list status, and login.
//!
//! Joining the world is not implemented yet, so login ends with a disconnect message.

use std::io;

use levitas_protocol::{Reader, Writer, GAME_VERSION, MAX_PACKET_LEN, PROTOCOL_VERSION};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::info;

/// Intent sent at the end of the handshake.
const INTENT_STATUS: i32 = 1;
const INTENT_LOGIN: i32 = 2;
const INTENT_TRANSFER: i32 = 3;

pub async fn handle(stream: TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let mut stream = BufReader::new(stream);

    let packet = read_frame(&mut stream).await?;
    let mut r = Reader::new(&packet);
    expect_id(&mut r, 0x00, "handshake")?;
    let protocol = r.varint()?;
    let _address = r.string(255)?;
    let _port = r.u16()?;
    match r.varint()? {
        INTENT_STATUS => status(&mut stream).await,
        INTENT_LOGIN | INTENT_TRANSFER => login(&mut stream, protocol).await,
        other => Err(invalid(format!("unknown handshake intent {other}"))),
    }
}

async fn status(stream: &mut BufReader<TcpStream>) -> io::Result<()> {
    loop {
        let packet = read_frame(stream).await?;
        let mut r = Reader::new(&packet);
        match r.varint()? {
            // Status Request
            0x00 => {
                let response = status_json().to_string();
                send(stream, Writer::packet(0x00).string(&response)).await?;
            }
            // Ping Request: echo the payload back and close.
            0x01 => {
                let payload = r.i64()?;
                return send(stream, Writer::packet(0x01).i64(payload)).await;
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

async fn login(stream: &mut BufReader<TcpStream>, protocol: i32) -> io::Result<()> {
    let packet = read_frame(stream).await?;
    let mut r = Reader::new(&packet);
    expect_id(&mut r, 0x00, "login start")?;
    let name = r.string(16)?;

    let reason = if protocol == PROTOCOL_VERSION {
        "Levitas is running, but joining the world is not implemented yet.".to_owned()
    } else {
        format!("Levitas runs Minecraft {GAME_VERSION}. Please join with that version.")
    };
    info!(name, protocol, "login rejected: {reason}");
    let message = json!({ "text": reason }).to_string();
    // Login Disconnect
    send(stream, Writer::packet(0x00).string(&message)).await
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

async fn send(stream: &mut BufReader<TcpStream>, packet: Writer) -> io::Result<()> {
    stream.write_all(&packet.into_frame()).await?;
    stream.flush().await
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

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    async fn connect() -> TcpStream {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = handle(stream).await;
        });
        TcpStream::connect(addr).await.unwrap()
    }

    fn handshake(intent: i32) -> Vec<u8> {
        Writer::packet(0x00)
            .varint(PROTOCOL_VERSION)
            .string("localhost")
            .u16(25565)
            .varint(intent)
            .into_frame()
    }

    #[tokio::test]
    async fn status_and_ping() {
        let mut client = connect().await;
        client.write_all(&handshake(INTENT_STATUS)).await.unwrap();
        client
            .write_all(&Writer::packet(0x00).into_frame())
            .await
            .unwrap();

        let response = read_frame(&mut client).await.unwrap();
        let mut r = Reader::new(&response);
        assert_eq!(r.varint().unwrap(), 0x00);
        let status: serde_json::Value = serde_json::from_str(r.string(32767).unwrap()).unwrap();
        assert_eq!(status["version"]["protocol"], PROTOCOL_VERSION);

        client
            .write_all(&Writer::packet(0x01).i64(1234).into_frame())
            .await
            .unwrap();
        let pong = read_frame(&mut client).await.unwrap();
        let mut r = Reader::new(&pong);
        assert_eq!(r.varint().unwrap(), 0x01);
        assert_eq!(r.i64().unwrap(), 1234);
    }

    #[tokio::test]
    async fn login_is_rejected_with_message() {
        let mut client = connect().await;
        client.write_all(&handshake(INTENT_LOGIN)).await.unwrap();
        let login_start = Writer::packet(0x00).string("DarkIgnite").uuid(0);
        client.write_all(&login_start.into_frame()).await.unwrap();

        let reply = read_frame(&mut client).await.unwrap();
        let mut r = Reader::new(&reply);
        assert_eq!(r.varint().unwrap(), 0x00);
        assert!(r.string(262144).unwrap().contains("not implemented"));
    }
}
