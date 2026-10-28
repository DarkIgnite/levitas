//! Moving players between dimensions. Each dimension is its own world task, so a
//! traveller is handed over whole: the old dimension lets go of them, points their
//! connection at the new one, and sends them there with everything they carry.

use tokio::sync::watch;

use super::dimension::Dimension;
use super::*;

/// Where a connection sends its events: the dimension its player is in.
pub type Route = watch::Sender<mpsc::Sender<Event>>;

/// A player on their way into this dimension.
pub struct Arrival {
    pub(super) id: i32,
    pub(super) player: Box<Player>,
    /// Just logging in (saved in this dimension), rather than travelling.
    pub(super) logging_in: bool,
    /// Coming back from death in another dimension, to respawn here.
    pub(super) respawning: bool,
}

/// What the data-kept field of the respawn packet keeps: attributes and entity data.
const KEEP_ALL_DATA: u8 = 0x03;

impl World {
    /// Hands player `id` over to `to`, arriving at `pos`. Returns whether they left.
    pub(super) fn travel(&mut self, id: i32, to: Dimension, pos: [f64; 3]) -> bool {
        self.hand_over(id, to, Some(pos), false, false)
    }

    pub(super) fn hand_over(
        &mut self,
        id: i32,
        to: Dimension,
        pos: Option<[f64; 3]>,
        logging_in: bool,
        respawning: bool,
    ) -> bool {
        let Some(link) = self.links.get(&to).cloned() else {
            return false;
        };
        if !logging_in {
            self.wake_up(id);
            self.close_container(id);
        }
        let Some(mut player) = self.players.remove(&id) else {
            return false;
        };
        if !logging_in {
            self.broadcast(
                Writer::packet(clientbound::REMOVE_ENTITIES)
                    .varint(1)
                    .varint(id),
                None,
            );
        }
        if let Some(pos) = pos {
            player.pos = pos;
            player.sent_pos = pos;
            player.center = (
                (pos[0] / 16.0).floor() as i32,
                (pos[2] / 16.0).floor() as i32,
            );
        }
        // The client forgets its chunks and entities when it changes dimension.
        player.loaded.clear();
        player.digging = None;
        player.eating = None;
        player.drawing = None;
        player.fall_distance = 0.0;
        if let Some(route) = &player.route {
            route.send_replace(link.clone());
        }
        let arrival = Arrival {
            id,
            player: Box::new(player),
            logging_in,
            respawning,
        };
        tokio::spawn(async move {
            let _ = link.send(Event::Arrive(Box::new(arrival))).await;
        });
        true
    }

    /// A player arriving from another dimension, or logging in to this one.
    pub(super) fn arrive(&mut self, arrival: Arrival) {
        let Arrival {
            id,
            player,
            logging_in,
            respawning,
        } = arrival;
        self.players.insert(id, *player);
        if logging_in {
            self.welcome(id);
            return;
        }
        if respawning {
            self.respawn(id);
            return;
        }
        let p = &self.players[&id];
        let (pos, yaw, pitch, mode) = (p.pos, p.yaw, p.pitch, p.mode);
        let respawn = spawn_info(Writer::packet(clientbound::RESPAWN), mode, self.dimension)
            .u8(KEEP_ALL_DATA);
        self.send(id, respawn);
        self.send(
            id,
            Writer::packet(clientbound::GAME_EVENT)
                .u8(GAME_EVENT_START_WAITING_FOR_CHUNKS)
                .f32(0.0),
        );
        self.update_view(id);
        self.send(id, position_packet(pos, yaw, pitch));
        self.send_status(id);
        self.send_time_and_weather(id);
        self.send_entities_to(id);
        for packet in spawn_player(id, &self.players[&id]) {
            self.broadcast(packet, Some(id));
        }
    }

    /// Sends a message to everyone in every dimension but player `except`.
    pub(super) fn announce_except(&mut self, packet: Writer, except: i32) {
        let f = frame(packet);
        for (&id, p) in self.players.iter_mut() {
            if id != except {
                p.deliver(&f);
            }
        }
        self.forward(f);
    }

    fn forward(&mut self, f: Frame) {
        for link in self.links.values() {
            let link = link.clone();
            let f = f.clone();
            tokio::spawn(async move {
                let _ = link.send(Event::Announce { frame: f }).await;
            });
        }
    }

    /// Sends a message to everyone in every dimension.
    pub(super) fn announce(&mut self, packet: Writer) {
        let f = frame(packet);
        for p in self.players.values_mut() {
            p.deliver(&f);
        }
        self.forward(f);
    }
}
