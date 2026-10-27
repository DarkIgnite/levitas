//! The connection side of the play state: decodes the player's packets into world
//! [`Event`]s and writes the frames the world sends back.

use std::io;
use std::time::Duration;

use levitas_protocol::{Reader, Writer};
use levitas_registry::packet::play::{clientbound, serverbound};
use tokio::io::BufReader;
use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::mpsc;

use crate::connection::{invalid, read_frame, Conn, Outgoing, Profile};
use crate::inventory::Stack;
use crate::world::{self, Event, Frame, OUTGOING_CAPACITY};

const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);
/// The protocol caps the slots one container click may change.
const MAX_CHANGED_SLOTS: i32 = 128;
const CLIENT_COMMAND_RESPAWN: i32 = 0;
/// Flag in the player input packet for the sneak key.
const INPUT_SNEAK: u8 = 0x20;
const PLAYER_COMMAND_LEAVE_BED: i32 = 0;
const PLAYER_COMMAND_START_SPRINTING: i32 = 1;
const PLAYER_COMMAND_STOP_SPRINTING: i32 = 2;

pub async fn run(conn: Conn, profile: Profile, world: mpsc::Sender<Event>) -> io::Result<()> {
    let Conn { mut reader, out } = conn;
    let id = world::next_entity_id();
    let (tx, rx) = mpsc::channel(OUTGOING_CAPACITY);
    let writer = tokio::spawn(write_loop(out, rx));
    if world
        .send(Event::Join {
            id,
            profile,
            out: tx,
        })
        .await
        .is_err()
    {
        writer.abort();
        return Ok(());
    }
    let result = read_loop(&mut reader, id, &world).await;
    let _ = world.send(Event::Leave { id }).await;
    writer.abort();
    result
}

/// Writes frames from the world as they come, flushing whenever the queue runs dry.
async fn write_loop(mut out: Outgoing, mut frames: mpsc::Receiver<Frame>) -> io::Result<()> {
    let mut keep_alive = tokio::time::interval(KEEP_ALIVE_INTERVAL);
    loop {
        tokio::select! {
            frame = frames.recv() => {
                // The world dropped this player (left, or too slow to keep up).
                let Some(frame) = frame else { return Ok(()) };
                out.write_frame(&frame).await?;
                while let Ok(frame) = frames.try_recv() {
                    out.write_frame(&frame).await?;
                }
                out.flush().await?;
            }
            _ = keep_alive.tick() => {
                let id = std::time::UNIX_EPOCH.elapsed().unwrap_or_default().as_millis() as i64;
                out.send(Writer::packet(clientbound::KEEP_ALIVE).i64(id)).await?;
            }
        }
    }
}

async fn read_loop(
    reader: &mut BufReader<OwnedReadHalf>,
    id: i32,
    world: &mpsc::Sender<Event>,
) -> io::Result<()> {
    loop {
        let packet = match read_frame(reader).await {
            Ok(packet) => packet,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        if let Some(event) = decode(id, &packet)? {
            if world.send(event).await.is_err() {
                return Ok(());
            }
        }
    }
}

/// Turns a serverbound play packet into a world event. Packets the server does not act
/// on yet (teleport confirmations, keep-alive replies, settings, ...) give `None`.
fn decode(id: i32, packet: &[u8]) -> io::Result<Option<Event>> {
    let mut r = Reader::new(packet);
    let on_ground = |flags: u8| flags & 1 != 0;
    let event = match r.varint()? {
        serverbound::MOVE_PLAYER_POS => {
            let pos = [r.f64()?, r.f64()?, r.f64()?];
            Event::Move {
                id,
                pos: Some(pos),
                rot: None,
                on_ground: on_ground(r.u8()?),
            }
        }
        serverbound::MOVE_PLAYER_POS_ROT => {
            let pos = [r.f64()?, r.f64()?, r.f64()?];
            let rot = (r.f32()?, r.f32()?);
            Event::Move {
                id,
                pos: Some(pos),
                rot: Some(rot),
                on_ground: on_ground(r.u8()?),
            }
        }
        serverbound::MOVE_PLAYER_ROT => {
            let rot = (r.f32()?, r.f32()?);
            Event::Move {
                id,
                pos: None,
                rot: Some(rot),
                on_ground: on_ground(r.u8()?),
            }
        }
        serverbound::MOVE_PLAYER_STATUS_ONLY => Event::Move {
            id,
            pos: None,
            rot: None,
            on_ground: on_ground(r.u8()?),
        },
        serverbound::PLAYER_ACTION => {
            let status = r.varint()?;
            let pos = r.position()?;
            let _face = r.u8()?;
            Event::Dig {
                id,
                status,
                pos,
                sequence: r.varint()?,
            }
        }
        serverbound::USE_ITEM_ON => {
            let hand = r.varint()?;
            let pos = r.position()?;
            let face = r.varint()?;
            let cursor = [r.f32()?, r.f32()?, r.f32()?].map(|c| c.clamp(0.0, 1.0));
            let _inside_block = r.bool()?;
            let _world_border_hit = r.bool()?;
            Event::UseItemOn {
                id,
                hand,
                pos,
                face,
                cursor,
                sequence: r.varint()?,
            }
        }
        serverbound::SET_CREATIVE_MODE_SLOT => {
            let slot = r.i16()?;
            let count = r.varint()?;
            let mut stack = Stack::EMPTY;
            if count > 0 {
                stack = Stack::new(r.varint()?, count);
                // Components the server tracks come first if present; the rest are
                // skipped, since their encodings vary by type.
                let added = r.varint()?;
                if added > 0 && r.varint()? == levitas_registry::component::DAMAGE {
                    stack.damage = r.varint()?.max(0);
                }
            }
            Event::CreativeSlot { id, slot, stack }
        }
        serverbound::CONTAINER_CLICK => {
            let window = r.varint()?;
            let _state_id = r.varint()?;
            let slot = r.i16()?;
            let button = r.u8()?;
            let mode = r.varint()?;
            let count = r.varint()?;
            if !(0..=MAX_CHANGED_SLOTS).contains(&count) {
                return Err(invalid(format!("container click changes {count} slots")));
            }
            let mut changed = Vec::new();
            for _ in 0..count {
                let slot = r.i16()?;
                changed.push((slot, hashed_stack(&mut r)?));
            }
            Event::ContainerClick {
                id,
                window,
                slot,
                button,
                mode,
                changed,
                carried: hashed_stack(&mut r)?,
            }
        }
        serverbound::CONTAINER_CLOSE => Event::ContainerClose {
            id,
            window: r.varint()?,
        },
        serverbound::USE_ITEM => Event::UseItem {
            id,
            hand: r.varint()?,
            sequence: r.varint()?,
        },
        serverbound::PLAYER_INPUT => Event::Sneak {
            id,
            sneaking: r.u8()? & INPUT_SNEAK != 0,
        },
        serverbound::CLIENT_COMMAND => match r.varint()? {
            CLIENT_COMMAND_RESPAWN => Event::Respawn { id },
            _ => return Ok(None),
        },
        serverbound::PLAYER_COMMAND => {
            let _entity = r.varint()?;
            match r.varint()? {
                PLAYER_COMMAND_LEAVE_BED => Event::LeaveBed { id },
                PLAYER_COMMAND_START_SPRINTING => Event::Sprint {
                    id,
                    sprinting: true,
                },
                PLAYER_COMMAND_STOP_SPRINTING => Event::Sprint {
                    id,
                    sprinting: false,
                },
                _ => return Ok(None),
            }
        }
        serverbound::SET_CARRIED_ITEM => Event::HeldSlot { id, slot: r.i16()? },
        serverbound::SWING => Event::Swing {
            id,
            hand: r.varint()?,
        },
        serverbound::CHAT => Event::Chat {
            id,
            message: r.string(256)?.to_owned(),
        },
        serverbound::CHAT_COMMAND => Event::Command {
            id,
            command: r.string(32767)?.to_owned(),
        },
        _ => return Ok(None),
    };
    Ok(Some(event))
}

/// Reads a hashed item stack (sent by container clicks since 1.21.5): the item and count,
/// plus hashes of its components, which are skipped.
fn hashed_stack(r: &mut Reader<'_>) -> io::Result<Stack> {
    if !r.bool()? {
        return Ok(Stack::EMPTY);
    }
    let item = r.varint()?;
    let count = r.varint()?;
    for _ in 0..r.varint()? {
        let _component = r.varint()?;
        let _hash = r.i32()?;
    }
    for _ in 0..r.varint()? {
        let _component = r.varint()?;
    }
    Ok(Stack::new(item, count))
}
