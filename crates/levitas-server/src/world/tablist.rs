//! The player list every client shows, shared by all dimensions.
//!
//! Each dimension is a world of its own, but a player is in one list for the whole
//! server: a client needs a player's entry to show their entity, wherever they log in
//! and whichever dimension they travel to, and drops it only when they leave the game.
//! So the entries live here, in one place, shared by the dimensions of a server (a
//! [`TabList`] handle each), rather than in each dimension's players.

use std::sync::{Arc, Mutex};

use crate::auth::Property;

#[derive(Clone)]
struct Entry {
    uuid: u128,
    name: String,
    properties: Vec<Property>,
    mode: u8,
}

/// The server's player list; clones share it.
#[derive(Clone, Default)]
pub struct TabList(Arc<Mutex<Vec<Entry>>>);

impl TabList {
    fn list(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Adds a player who logged in (or replaces their entry).
    pub fn add(&self, uuid: u128, name: &str, properties: &[Property], mode: u8) {
        let mut list = self.list();
        list.retain(|e| e.uuid != uuid);
        list.push(Entry {
            uuid,
            name: name.to_owned(),
            properties: properties.to_vec(),
            mode,
        });
    }

    /// Takes a player who left the game out.
    pub fn remove(&self, uuid: u128) {
        self.list().retain(|e| e.uuid != uuid);
    }

    pub fn set_mode(&self, uuid: u128, mode: u8) {
        if let Some(e) = self.list().iter_mut().find(|e| e.uuid == uuid) {
            e.mode = mode;
        }
    }

    /// The proper name of an online player, from any spelling of it.
    pub fn name_of(&self, name: &str) -> Option<String> {
        self.list()
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .map(|e| e.name.clone())
    }

    /// A tab list update adding these players (`only`: just that one; otherwise everyone).
    pub fn packet(&self, only: Option<u128>) -> levitas_protocol::Writer {
        use levitas_registry::packet::play::clientbound;
        let list = self.list();
        let entries: Vec<&Entry> = list
            .iter()
            .filter(|e| only.is_none_or(|u| e.uuid == u))
            .collect();
        let mut packet = levitas_protocol::Writer::packet(clientbound::PLAYER_INFO_UPDATE)
            .u8(super::PLAYER_INFO_ACTIONS)
            .varint(entries.len() as i32);
        for e in entries {
            packet = packet.uuid(e.uuid).string(&e.name);
            packet = crate::connection::write_properties(packet, &e.properties)
                .varint(i32::from(e.mode)) // update game mode
                .bool(true) // update listed
                .varint(0); // update latency
        }
        packet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_follows_logins_modes_and_leaving() {
        let tabs = TabList::default();
        let (a, b) = (0xA11CE, 0xB0B);
        tabs.add(a, "TabAlice", &[], 0);
        tabs.add(b, "TabBob", &[], 1);
        assert!(tabs.name_of("tabalice").is_some());
        assert_eq!(tabs.name_of("tabbob").as_deref(), Some("TabBob"));
        // Another server's list is its own.
        assert!(TabList::default().name_of("tabalice").is_none());
        tabs.set_mode(b, 0);
        // Only the asked-for player, in the layout of a tab list update.
        let frame = tabs.packet(Some(b)).into_frame();
        let mut r = levitas_protocol::Reader::new(&frame);
        r.varint().unwrap(); // length
        r.varint().unwrap(); // packet ID
        r.u8().unwrap(); // actions
        assert_eq!(r.varint().unwrap(), 1);
        assert_eq!(r.uuid().unwrap(), b);
        assert_eq!(r.string(16).unwrap(), "TabBob");
        tabs.remove(a);
        tabs.remove(b);
        assert!(tabs.name_of("TabAlice").is_none());
    }
}
