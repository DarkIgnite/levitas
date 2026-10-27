//! Chat commands. A small built-in set until there is a command framework and plugins.

use super::*;

const HELP: &str = "Commands: /gamemode <survival|creative>, /give <item> [count], /kill, \
                    /time <set|add|query> ..., /weather <clear|rain|thunder> [seconds]";

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
            Some("time") => self.time_command(args.next(), args.next()),
            Some("weather") => match args.next() {
                Some(kind) => {
                    let ticks = args
                        .next()
                        .and_then(|s| s.parse::<i32>().ok())
                        .map(|s| s * 20);
                    if self.set_weather(kind, ticks) {
                        format!("Set the weather to {kind}")
                    } else {
                        "Usage: /weather <clear|rain|thunder> [seconds]".to_owned()
                    }
                }
                None => "Usage: /weather <clear|rain|thunder> [seconds]".to_owned(),
            },
            Some("help") => HELP.to_owned(),
            _ => format!("Unknown command: /{command}. {HELP}"),
        };
        self.send(id, system_chat(&reply));
    }

    fn time_command(&mut self, action: Option<&str>, value: Option<&str>) -> String {
        const USAGE: &str =
            "Usage: /time <set|add> <ticks|day|noon|night|midnight> or /time query daytime";
        let named = |v: &str| match v {
            "day" => Some(1_000),
            "noon" => Some(6_000),
            "night" => Some(13_000),
            "midnight" => Some(18_000),
            other => other.parse::<i64>().ok(),
        };
        match (action, value.and_then(named)) {
            (Some("set"), Some(time)) => {
                // Like vanilla, setting the time keeps the day count.
                let day = self.day_time - self.day_time.rem_euclid(time::DAY_TICKS);
                self.day_time = day + time.rem_euclid(time::DAY_TICKS);
            }
            (Some("add"), Some(ticks)) => self.day_time += ticks,
            (Some("query"), _) => {
                return format!("The time is {}", self.day_time.rem_euclid(time::DAY_TICKS));
            }
            _ => return USAGE.to_owned(),
        }
        self.broadcast(self.time_packet(), None);
        format!(
            "Set the time to {}",
            self.day_time.rem_euclid(time::DAY_TICKS)
        )
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
        let (changed, left) = p.inventory.add(Stack::new(item_id, count));
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
