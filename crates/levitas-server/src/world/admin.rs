//! Running the server: operators, the whitelist, bans, kicks, `/list` and `/say`, from
//! the game (operators only) or the server console, which may do anything.

use super::*;
use crate::access::{self, List};

/// Entity event telling a client its permission level: 24 for none up to 28 for 4.
const ENTITY_EVENT_PERMISSION_0: i8 = 24;
const ENTITY_EVENT_PERMISSION_4: i8 = 28;

/// Game events telling clients about rules they act on themselves.
const GAME_EVENT_IMMEDIATE_RESPAWN: u8 = 11;
const ENTITY_EVENT_REDUCED_DEBUG_INFO: i8 = 22;
const ENTITY_EVENT_FULL_DEBUG_INFO: i8 = 23;

/// Tells a client the difficulty (never locked).
pub fn difficulty_packet() -> Writer {
    Writer::packet(clientbound::CHANGE_DIFFICULTY)
        .u8(crate::rules::difficulty() as u8)
        .bool(false)
}

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
            .unwrap_or_else(|| format!("Unknown console command: {line}. Try op, deop, whitelist, ban, pardon, kick, list, say, gamerule, difficulty, stop."));
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
            ["gamerule", rule] => match crate::rules::get_text(rule) {
                Some(value) => format!("Gamerule {rule} is currently set to: {value}"),
                None => format!("Unknown game rule: {rule}"),
            },
            ["gamerule", rule, value] => match crate::rules::set(rule, value) {
                Ok(()) => {
                    self.rule_changed(crate::rules::short_name(rule).unwrap_or(rule));
                    format!("Gamerule {rule} is now set to: {value}")
                }
                Err(e) => e,
            },
            ["difficulty"] => format!("The difficulty is {}", crate::rules::difficulty().name()),
            ["difficulty", name] => match crate::rules::Difficulty::parse(name) {
                Some(d) if d == crate::rules::difficulty() => {
                    format!(
                        "The difficulty did not change; it is already set to {}",
                        d.name()
                    )
                }
                Some(d) => {
                    crate::rules::set_difficulty(d);
                    self.announce(difficulty_packet());
                    format!("The difficulty has been set to {}", d.name())
                }
                None => "Usage: /difficulty <peaceful|easy|normal|hard>".to_owned(),
            },
            ["say", message @ ..] if !message.is_empty() => {
                let line = format!("[{by}] {}", message.join(" "));
                self.announce(system_chat(&line));
                line
            }
            _ => return None,
        };
        Some(reply)
    }

    /// Tells everyone's client about a rule it acts on itself.
    fn rule_changed(&mut self, rule: &str) {
        match rule {
            "immediate_respawn" => {
                let on = crate::rules::bool(rule);
                self.announce(
                    Writer::packet(clientbound::GAME_EVENT)
                        .u8(GAME_EVENT_IMMEDIATE_RESPAWN)
                        .f32(if on { 1.0 } else { 0.0 }),
                );
            }
            "reduced_debug_info" => {
                let event = if crate::rules::bool(rule) {
                    ENTITY_EVENT_REDUCED_DEBUG_INFO
                } else {
                    ENTITY_EVENT_FULL_DEBUG_INFO
                };
                let ids: Vec<i32> = self.players.keys().copied().collect();
                for id in ids {
                    self.send(id, entity_event(id, event));
                }
            }
            _ => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;
    use crate::rules::{self, Difficulty};

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-rules-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        )
        .0
    }

    fn player(w: &mut World) -> i32 {
        let (out, frames) = mpsc::channel(100_000);
        std::mem::forget(frames); // keep the channel open
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, out, [0.5, -60.0, 0.5]));
        id
    }

    #[test]
    fn game_rules_and_difficulty_from_the_console() {
        let mut w = world();
        w.console("gamerule keep_inventory true");
        assert!(rules::bool("keep_inventory"));
        w.console("gamerule keep_inventory sometimes");
        assert!(rules::bool("keep_inventory"), "bad values change nothing");
        w.console("difficulty hard");
        assert_eq!(rules::difficulty(), Difficulty::Hard);
        rules::set("keep_inventory", "false").unwrap();
        rules::set_difficulty(Difficulty::Normal);
    }

    #[test]
    fn kept_inventories_and_harmless_falls() {
        let mut w = world();
        let id = player(&mut w);
        let dirt = Stack::new(ItemInfo::id_of("minecraft:dirt").unwrap(), 5);
        w.players.get_mut(&id).unwrap().inventory.slots[10] = dirt;

        rules::set("fall_damage", "false").unwrap();
        w.damage(id, 5.0, survival::Damage::Fall);
        assert_eq!(w.players[&id].health, 20.0);

        rules::set("keep_inventory", "true").unwrap();
        w.damage(id, 100.0, survival::Damage::Kill);
        assert!(!w.players[&id].alive);
        assert_eq!(w.players[&id].inventory.slots[10], dirt);
        rules::set("keep_inventory", "false").unwrap();
        rules::set("fall_damage", "true").unwrap();
    }
}

#[cfg(test)]
mod naming_tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn anvils_rename_and_name_tags_name_mobs() {
        let dir = std::env::temp_dir().join(format!("levitas-names-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let (out, frames) = mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        w.players
            .insert(alice, Player::new(profile, out, [2.5, -60.0, 2.5]));
        w.players.get_mut(&alice).unwrap().xp_level = 5;
        let anvil = BlockPos::new(3, -60, 3);
        let state = BlockInfo::by_name("minecraft:anvil").unwrap().default_state;
        w.set_block(anvil, state);
        assert!(w.open_container(alice, anvil, state));
        let tag = Stack::new(ItemInfo::id_of("minecraft:name_tag").unwrap(), 1);
        w.players
            .get_mut(&alice)
            .unwrap()
            .open
            .as_mut()
            .unwrap()
            .grid[0] = tag;
        w.rename_item(alice, "Bessie");
        let window = w.players[&alice].open.as_ref().unwrap().window;
        w.container_click(alice, window, 2, 0, 0, Vec::new(), Stack::EMPTY);
        let named = w.players[&alice].inventory.carried;
        assert_eq!((named.item, named.name), (tag.item, Some("Bessie")));
        assert_eq!(w.players[&alice].xp_level, 4, "one level for the name");

        // The tag names a cow, which keeps the name when saved.
        let p = w.players.get_mut(&alice).unwrap();
        p.inventory.carried = Stack::EMPTY;
        let held = p.hand_slot(0);
        p.inventory.slots[held] = named;
        let cow = next_entity_id();
        w.mobs.insert(
            cow,
            super::mobs::Mob::new(super::mobs::MobKind::Cow, [3.5, -60.0, 2.5], &mut w.rng),
        );
        w.use_on_mob(alice, cow, 0);
        assert_eq!(w.mobs[&cow].name, Some("Bessie"));
        assert!(w.players[&alice].inventory.slots[held].is_empty());
    }
}
