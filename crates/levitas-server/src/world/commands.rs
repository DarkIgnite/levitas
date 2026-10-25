//! Chat commands. A small built-in set until there is a command framework and plugins.

use super::*;

const HELP: &str = "Commands: /gamemode <survival|creative>, /give <item> [count], /kill";

impl World {
    pub(super) fn command(&mut self, id: i32, command: &str) {
        let mut args = command.split_whitespace();
        let reply = match args.next() {
            Some("gamemode" | "gm") => match args.next() {
                Some("survival" | "s" | "0") => self.set_game_mode(id, GAME_MODE_SURVIVAL),
                Some("creative" | "c" | "1") => self.set_game_mode(id, GAME_MODE_CREATIVE),
                _ => "Usage: /gamemode <survival|creative>".to_owned(),
            },
            Some("give") => match args.next() {
                Some(item) => {
                    let count = args.next().and_then(|c| c.parse().ok()).unwrap_or(1);
                    self.give(id, item, count)
                }
                None => "Usage: /give <item> [count]".to_owned(),
            },
            Some("kill") => {
                self.damage(id, f32::MAX, survival::Damage::Kill);
                return;
            }
            Some("help") => HELP.to_owned(),
            _ => format!("Unknown command: /{command}. {HELP}"),
        };
        self.send(id, system_chat(&reply));
    }

    fn set_game_mode(&mut self, id: i32, mode: u8) -> String {
        let Some(p) = self.players.get_mut(&id) else {
            return String::new();
        };
        p.mode = mode;
        p.fall_distance = 0.0;
        let uuid = p.profile.uuid;
        self.send(
            id,
            Writer::packet(clientbound::GAME_EVENT)
                .u8(GAME_EVENT_CHANGE_GAME_MODE)
                .f32(f32::from(mode)),
        );
        self.broadcast(
            Writer::packet(clientbound::PLAYER_INFO_UPDATE)
                .u8(PLAYER_INFO_UPDATE_GAME_MODE)
                .varint(1)
                .uuid(uuid)
                .varint(i32::from(mode)),
            None,
        );
        let name = if mode == GAME_MODE_CREATIVE {
            "Creative"
        } else {
            "Survival"
        };
        format!("Set own game mode to {name} Mode")
    }

    fn give(&mut self, id: i32, item: &str, count: i32) -> String {
        let name = if item.contains(':') {
            item.to_owned()
        } else {
            format!("minecraft:{item}")
        };
        let Some(item_id) = ItemInfo::id_of(&name).filter(|&i| i != 0) else {
            return format!("Unknown item: {name}");
        };
        let Some(p) = self.players.get_mut(&id) else {
            return String::new();
        };
        let count = count.clamp(1, 64 * 36);
        let (changed, left) = p.inventory.add(item_id, count);
        let updates: Vec<Writer> = changed
            .into_iter()
            .map(|slot| p.inventory.set_slot_packet(slot))
            .collect();
        for update in updates {
            self.send(id, update);
        }
        format!("Gave {} [{name}]", count - left)
    }
}
