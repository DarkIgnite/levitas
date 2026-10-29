//! Running the server: operators, the whitelist, bans, kicks, `/list` and `/say`, from
//! the game (operators only) or the server console, which may do anything.

use super::*;
use crate::access::{self, List};

/// Entity event telling a client its permission level: 24 for none up to 28 for 4.
const ENTITY_EVENT_PERMISSION_0: i8 = 24;
const ENTITY_EVENT_PERMISSION_4: i8 = 28;

/// Commands every player may use; the rest need an operator.
pub const EVERYONE: [&str; 2] = ["help", "list"];

impl World {
    /// Tells a player's client what they may do: their permission level and the
    /// commands it should offer.
    pub(super) fn send_permissions(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let op = p.op;
        let level = if op {
            ENTITY_EVENT_PERMISSION_4
        } else {
            ENTITY_EVENT_PERMISSION_0
        };
        self.send(id, entity_event(id, level));
        self.send(id, commands::command_tree(op));
    }

    /// A line typed into the server console.
    pub(super) fn console(&mut self, line: &str) {
        let line = line.trim().trim_start_matches('/');
        if line.is_empty() {
            return;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let reply = self
            .admin_command(&words, "Server")
            .unwrap_or_else(|| format!("Unknown console command: {line}. Try op, deop, whitelist, ban, pardon, kick, list, say, stop."));
        info!("{reply}");
    }

    /// Runs an administration command; `None` if `words` is not one. `by` is who ran it.
    pub(super) fn admin_command(&mut self, words: &[&str], by: &str) -> Option<String> {
        let reply = match words {
            ["op", name] => {
                let uuid = self.uuid_of(name);
                if access::add(List::Ops, name, uuid, "") {
                    self.set_op(name, true);
                    format!("Made {name} a server operator")
                } else {
                    "Nothing changed. The player already is an operator".to_owned()
                }
            }
            ["deop", name] => {
                if access::remove(List::Ops, name) {
                    self.set_op(name, false);
                    format!("Made {name} no longer a server operator")
                } else {
                    "Nothing changed. The player is not an operator".to_owned()
                }
            }
            ["whitelist", "on"] => {
                access::set_whitelist(true);
                "Whitelist is now turned on".to_owned()
            }
            ["whitelist", "off"] => {
                access::set_whitelist(false);
                "Whitelist is now turned off".to_owned()
            }
            ["whitelist", "add", name] => {
                let uuid = self.uuid_of(name);
                if access::add(List::Whitelist, name, uuid, "") {
                    format!("Added {name} to the whitelist")
                } else {
                    "Player is already whitelisted".to_owned()
                }
            }
            ["whitelist", "remove", name] => {
                if access::remove(List::Whitelist, name) {
                    format!("Removed {name} from the whitelist")
                } else {
                    "Player is not whitelisted".to_owned()
                }
            }
            ["whitelist", "list"] => {
                let names = access::names(List::Whitelist);
                format!(
                    "There are {} whitelisted player(s): {}",
                    names.len(),
                    names.join(", ")
                )
            }
            ["ban", name, reason @ ..] => {
                let reason = if reason.is_empty() {
                    "Banned by an operator.".to_owned()
                } else {
                    reason.join(" ")
                };
                let uuid = self.uuid_of(name);
                if access::add(List::Bans, name, uuid, &reason) {
                    self.kick_everywhere(
                        name,
                        &format!("You are banned from this server.\nReason: {reason}"),
                    );
                    format!("Banned {name}: {reason}")
                } else {
                    "Nothing changed. The player is already banned".to_owned()
                }
            }
            ["pardon", name] => {
                if access::remove(List::Bans, name) {
                    format!("Unbanned {name}")
                } else {
                    "Nothing changed. The player isn't banned".to_owned()
                }
            }
            ["kick", name, reason @ ..] => {
                let reason = if reason.is_empty() {
                    "Kicked by an operator".to_owned()
                } else {
                    reason.join(" ")
                };
                self.kick_everywhere(name, &reason);
                format!("Kicked {name}: {reason}")
            }
            ["list"] => {
                let names: Vec<String> = self
                    .players
                    .values()
                    .map(|p| p.profile.name.clone())
                    .collect();
                format!(
                    "There are {} of a max of {} players online here: {}",
                    names.len(),
                    crate::config::get().max_players,
                    names.join(", ")
                )
            }
            ["say", message @ ..] if !message.is_empty() => {
                let line = format!("[{by}] {}", message.join(" "));
                self.announce(system_chat(&line));
                line
            }
            _ => return None,
        };
        Some(reply)
    }

    /// A player's UUID if they are online here, else 0 (they are matched by name).
    fn uuid_of(&self, name: &str) -> u128 {
        self.players
            .values()
            .find(|p| p.profile.name.eq_ignore_ascii_case(name))
            .map_or(0, |p| p.profile.uuid)
    }

    fn set_op(&mut self, name: &str, op: bool) {
        let ids: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.profile.name.eq_ignore_ascii_case(name))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            if let Some(p) = self.players.get_mut(&id) {
                p.op = op;
            }
            self.send_permissions(id);
        }
        // Operators in other dimensions hear of it too.
        self.forward_admin(Event::OpChanged {
            name: name.to_owned(),
            op,
        });
    }

    /// Disconnects a player wherever they are.
    pub(super) fn kick_everywhere(&mut self, name: &str, reason: &str) {
        self.kick_here(name, reason);
        self.forward_admin(Event::Kick {
            name: name.to_owned(),
            reason: reason.to_owned(),
        });
    }

    pub(super) fn kick_here(&mut self, name: &str, reason: &str) {
        let ids: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.profile.name.eq_ignore_ascii_case(name))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.send(id, Writer::packet(clientbound::DISCONNECT).text(reason));
            if let Some(p) = self.players.get_mut(&id) {
                p.kicked = true;
            }
        }
    }

    pub(super) fn op_changed_here(&mut self, name: &str, op: bool) {
        let ids: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.profile.name.eq_ignore_ascii_case(name))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            if let Some(p) = self.players.get_mut(&id) {
                p.op = op;
            }
            self.send_permissions(id);
        }
    }

    fn forward_admin(&mut self, event: Event) {
        // Only kicks and operator changes are forwarded; each goes to every other
        // dimension once.
        for link in self.links.values() {
            let copy = match &event {
                Event::Kick { name, reason } => Event::Kick {
                    name: name.clone(),
                    reason: reason.clone(),
                },
                Event::OpChanged { name, op } => Event::OpChanged {
                    name: name.clone(),
                    op: *op,
                },
                _ => return,
            };
            let link = link.clone();
            tokio::spawn(async move {
                let _ = link.send(copy).await;
            });
        }
    }
}
