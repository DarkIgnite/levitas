//! The connection side of the play state: decodes the player's packets into world
//! [`Event`]s and writes the frames the world sends back.

use std::io;
use std::time::Duration;

use levitas_protocol::{Reader, Writer};
use levitas_registry::packet::play::{clientbound, serverbound};
use tokio::sync::mpsc;

use crate::connection::{invalid, read_packet, ClientReader, Conn, Outgoing, Profile};
use crate::inventory::Stack;
use crate::world::{self, Event, Frame, OUTGOING_CAPACITY};

const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);
/// The protocol caps the slots one container click may change.
const MAX_CHANGED_SLOTS: i32 = 128;
const CLIENT_COMMAND_RESPAWN: i32 = 0;
const CLIENT_COMMAND_REQUEST_STATS: i32 = 1;
/// Flag in the player input packet for the sneak key.
const INPUT_FORWARD: u8 = 0x01;
const INPUT_SNEAK: u8 = 0x20;
const PLAYER_COMMAND_LEAVE_BED: i32 = 0;
/// Most components read from one creative-mode item.
const MAX_COMPONENTS: i32 = 64;
const PLAYER_COMMAND_START_SPRINTING: i32 = 1;
const PLAYER_COMMAND_STOP_SPRINTING: i32 = 2;

pub async fn run(conn: Conn, profile: Profile, world: mpsc::Sender<Event>) -> io::Result<()> {
    let Conn {
        mut reader,
        out,
        compressed,
        skin_parts,
        main_hand,
    } = conn;
    let id = world::next_entity_id();
    let (tx, rx) = mpsc::channel(OUTGOING_CAPACITY);
    let writer = tokio::spawn(write_loop(out, rx));
    // Events go to whichever dimension the player is in; the world switches this.
    let (route, current) = tokio::sync::watch::channel(world.clone());
    if world
        .send(Event::Join {
            id,
            profile,
            out: tx,
            route,
            settings: (skin_parts, main_hand),
        })
        .await
        .is_err()
    {
        writer.abort();
        return Ok(());
    }
    let result = read_loop(&mut reader, id, &current, compressed).await;
    let world = current.borrow().clone();
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
    reader: &mut ClientReader,
    id: i32,
    route: &tokio::sync::watch::Receiver<mpsc::Sender<Event>>,
    compressed: bool,
) -> io::Result<()> {
    loop {
        let packet = match read_packet(reader, compressed).await {
            Ok(packet) => packet,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        if let Some(event) = decode(id, &packet)? {
            let world = route.borrow().clone();
            if world.send(event).await.is_err() {
                return Ok(());
            }
        }
    }
}

/// A stack from a creative client, kept only as far as every client can read it back:
/// an unknown item, potion, trim or enchantment ID would disconnect whoever is sent it.
fn valid_creative_stack(mut stack: Stack) -> Stack {
    if levitas_registry::ItemInfo::get(stack.item).is_none() {
        return Stack::EMPTY;
    }
    stack.count = stack.count.clamp(1, 99);
    if stack
        .potion
        .is_some_and(|p| crate::potions::name(p).is_none())
    {
        stack.potion = None;
    }
    let in_registry = |name: &str, id: u16| {
        levitas_registry::SyncedRegistry::get(name)
            .is_some_and(|r| usize::from(id) < r.entries.len())
    };
    if stack.trim.is_some_and(|t| {
        !in_registry("minecraft:trim_material", t.material)
            || !in_registry("minecraft:trim_pattern", t.pattern)
    }) {
        stack.trim = None;
    }
    stack
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
                // Each component comes with its length, so ones Levitas does not track
                // yet can be skipped.
                let added = r.varint()?;
                let _removed = r.varint()?;
                for _ in 0..added.clamp(0, MAX_COMPONENTS) {
                    let kind = r.varint()?;
                    let length = usize::try_from(r.varint()?)
                        .map_err(|_| invalid("negative component length".to_owned()))?;
                    let data = r.bytes(length)?;
                    if kind == levitas_registry::component::DAMAGE {
                        stack.damage = Reader::new(data).varint()?.max(0);
                    } else if kind == levitas_registry::component::CUSTOM_NAME {
                        if let Ok(tag) = levitas_nbt::from_network(data) {
                            let name = crate::names::clean(&crate::names::plain_text(&tag));
                            stack.name = crate::names::intern(&name);
                        }
                    } else if kind == levitas_registry::component::POTION_CONTENTS {
                        let mut c = Reader::new(data);
                        if c.bool()? {
                            stack.potion = u8::try_from(c.varint()?).ok();
                        }
                    } else if kind == levitas_registry::component::TRIM {
                        let mut t = Reader::new(data);
                        let (material, pattern) = (t.varint()?, t.varint()?);
                        // Only trims that name registry entries are kept.
                        if material > 0 && pattern > 0 {
                            stack.trim = Some(crate::inventory::Trim {
                                material: (material - 1) as u16,
                                pattern: (pattern - 1) as u16,
                            });
                        }
                    } else if kind == levitas_registry::component::DYED_COLOR {
                        stack.color = Some(Reader::new(data).i32()? & 0xFF_FFFF);
                    } else if kind == levitas_registry::component::REPAIR_COST {
                        stack.repair_cost = Reader::new(data).varint()?.max(0);
                    } else if kind == levitas_registry::component::ENCHANTMENTS
                        || kind == levitas_registry::component::STORED_ENCHANTMENTS
                    {
                        stack.enchants =
                            crate::enchanting::Enchantments::read(&mut Reader::new(data))?;
                    }
                }
                stack = valid_creative_stack(stack);
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
        serverbound::EDIT_BOOK => {
            let slot = r.varint()?;
            let count = r.varint()?.clamp(0, 100);
            let mut pages = Vec::new();
            for _ in 0..count {
                pages.push(r.string(8192)?.to_owned());
            }
            let title = if r.bool()? {
                Some(r.string(128)?.to_owned())
            } else {
                None
            };
            Event::EditBook {
                id,
                slot,
                pages,
                title,
            }
        }
        serverbound::CONTAINER_SLOT_STATE_CHANGED => Event::SlotState {
            id,
            slot: r.varint()?,
            window: r.varint()?,
            enabled: r.bool()?,
        },
        serverbound::CONTAINER_BUTTON_CLICK => Event::ContainerButton {
            id,
            window: r.varint()?,
            button: r.varint()?,
        },
        serverbound::MOVE_VEHICLE => Event::MoveVehicle {
            id,
            pos: [r.f64()?, r.f64()?, r.f64()?],
            yaw: r.f32()?,
        },
        serverbound::PADDLE_BOAT => Event::PaddleBoat {
            id,
            left: r.bool()?,
            right: r.bool()?,
        },
        serverbound::RENAME_ITEM => Event::RenameItem {
            id,
            name: r.string(32767)?.to_owned(),
        },
        serverbound::SELECT_TRADE => Event::SelectTrade {
            id,
            index: r.varint()?,
        },
        serverbound::SET_BEACON => {
            let has_primary = r.bool()?;
            let primary = if has_primary { Some(r.varint()?) } else { None };
            let has_secondary = r.bool()?;
            let secondary = if has_secondary {
                Some(r.varint()?)
            } else {
                None
            };
            Event::SetBeacon {
                id,
                primary,
                secondary,
            }
        }
        serverbound::SIGN_UPDATE => {
            let pos = r.position()?;
            let mut lines: [String; 4] = Default::default();
            for line in &mut lines {
                *line = r.string(384)?.to_owned();
            }
            // The text slot: 0 is the back, 1 the front.
            let front = r.varint()? == 1;
            Event::SignUpdate {
                id,
                pos,
                front,
                lines,
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
        serverbound::PLAYER_INPUT => {
            let flags = r.u8()?;
            Event::Sneak {
                id,
                sneaking: flags & INPUT_SNEAK != 0,
                forward: flags & INPUT_FORWARD != 0,
            }
        }
        serverbound::CLIENT_INFORMATION => match crate::connection::client_settings(&mut r) {
            Some((skin_parts, main_hand)) => Event::Settings {
                id,
                skin_parts,
                main_hand,
            },
            None => return Ok(None),
        },
        serverbound::CLIENT_COMMAND => match r.varint()? {
            CLIENT_COMMAND_RESPAWN => Event::Respawn { id },
            CLIENT_COMMAND_REQUEST_STATS => Event::RequestStats { id },
            _ => return Ok(None),
        },
        serverbound::INTERACT => {
            let target = r.varint()?;
            let hand = r.varint()?;
            let at = r.lp_vec3()?;
            Event::UseEntityAt {
                id,
                target,
                hand,
                at,
            }
        }
        serverbound::ATTACK => Event::Attack {
            id,
            target: r.varint()?,
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
        // A punch swings the main hand.
        serverbound::PUNCH => Event::Swing { id, hand: 0 },
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

#[cfg(test)]
mod creative_tests {
    use super::*;

    #[test]
    fn creative_stacks_keep_only_ids_clients_know() {
        assert!(valid_creative_stack(Stack::new(1_000_000, 1)).is_empty());
        let stone = levitas_registry::ItemInfo::id_of("minecraft:stone").unwrap();
        let mut s = Stack::new(stone, 500);
        s.potion = Some(250);
        s.trim = Some(crate::inventory::Trim {
            material: 60_000,
            pattern: 0,
        });
        let s = valid_creative_stack(s);
        assert_eq!((s.item, s.count), (stone, 99));
        assert!(s.potion.is_none() && s.trim.is_none());
        let frame = Writer::packet(0)
            .varint(2)
            .varint(9999)
            .varint(1)
            .varint(0)
            .varint(3)
            .into_frame();
        let mut r = Reader::new(&frame);
        r.varint().unwrap(); // length
        r.varint().unwrap(); // packet ID
        let e = crate::enchanting::Enchantments::read(&mut r).unwrap();
        assert_eq!(e.iter().count(), 1, "the unknown enchantment is dropped");
    }
}
