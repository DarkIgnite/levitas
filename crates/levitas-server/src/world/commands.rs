//! Chat commands. A small built-in set until there is a command framework and plugins.
//!
//! The client learns the commands' syntax from a Brigadier command tree sent when a
//! player joins ([`command_tree`]); it uses it for tab completion and highlighting.

use levitas_registry::command_argument;

use super::*;

const HELP: &str = "Commands: /gamemode <survival|creative>, /give <item> [count], /kill, \
                    /time <set|add|query> ..., /tp <x> <y> <z>, /effect <give|clear> ..., /weather <clear|rain|thunder> [seconds], /summon <mob>, /xp <add|query> ...";

impl World {
    pub(super) fn command(&mut self, id: i32, command: &str) {
        let words: Vec<&str> = command.split_whitespace().collect();
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let (op, name) = (p.op, p.profile.name.clone());
        let first = words.first().copied().unwrap_or("");
        if !op && !admin::EVERYONE.contains(&first) {
            self.send(
                id,
                system_chat("You do not have permission to use this command (ask an operator)"),
            );
            return;
        }
        if let Some(reply) = self.admin_command(&words, &name) {
            info!("{name} ran /{command}: {reply}");
            self.send(id, system_chat(&reply));
            return;
        }
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
            Some("summon") => match args.next() {
                Some(mob) => self.summon(id, mob),
                None => format!("Usage: /summon <{}>", mobs::SUMMONABLE.join("|")),
            },
            Some("effect") => {
                let rest: Vec<&str> = args.collect();
                self.effect_command(id, &rest)
            }
            Some("tp" | "teleport") => {
                let rest: Vec<&str> = args.collect();
                self.tp_command(id, self.dimension, &rest)
            }
            Some("execute") => {
                let rest: Vec<&str> = args.collect();
                match rest.as_slice() {
                    ["in", dimension, "run", "tp" | "teleport", coords @ ..] => {
                        match super::dimension::Dimension::from_name(dimension) {
                            Some(d) => self.tp_command(id, d, coords),
                            None => format!("Unknown dimension: {dimension}"),
                        }
                    }
                    _ => "Only /execute in <dimension> run tp <x> <y> <z> works so far".to_owned(),
                }
            }
            Some("xp" | "experience") => self.xp_command(id, args.next(), args.next(), args.next()),
            Some("help") => HELP.to_owned(),
            _ => format!("Unknown command: /{command}. {HELP}"),
        };
        self.send(id, system_chat(&reply));
    }

    /// `/tp <x> <y> <z>`, with `~` for the player's own coordinates, into `dimension`.
    fn tp_command(
        &mut self,
        id: i32,
        dimension: super::dimension::Dimension,
        coords: &[&str],
    ) -> String {
        const USAGE: &str = "Usage: /tp <x> <y> <z>";
        let Some(p) = self.players.get(&id) else {
            return String::new();
        };
        let [x, y, z] = coords else {
            return USAGE.to_owned();
        };
        let mut pos = [0.0; 3];
        for (i, s) in [x, y, z].into_iter().enumerate() {
            let value = match s.strip_prefix('~') {
                Some("") => Some(p.pos[i]),
                Some(offset) => offset.parse::<f64>().ok().map(|o| p.pos[i] + o),
                None => s.parse::<f64>().ok(),
            };
            let Some(value) = value else {
                return USAGE.to_owned();
            };
            pos[i] = value;
        }
        if dimension != self.dimension {
            let name = dimension.name();
            return if self.travel(id, dimension, pos) {
                format!("Teleported to {name}")
            } else {
                format!("Cannot reach {name}")
            };
        }
        self.teleport(id, pos);
        format!("Teleported to {:.1}, {:.1}, {:.1}", pos[0], pos[1], pos[2])
    }

    fn xp_command(
        &mut self,
        id: i32,
        action: Option<&str>,
        amount: Option<&str>,
        unit: Option<&str>,
    ) -> String {
        const USAGE: &str = "Usage: /xp add <amount> [points|levels] or /xp query";
        match (action, amount.and_then(|a| a.parse::<i32>().ok())) {
            (Some("add"), Some(n)) if unit == Some("levels") => {
                self.give_levels(id, n);
                format!("Gave {n} experience levels")
            }
            (Some("add"), Some(n)) if n >= 0 => {
                self.give_xp(id, n);
                format!("Gave {n} experience points")
            }
            (Some("query"), _) => {
                let p = &self.players[&id];
                format!("You are level {} with {} points", p.xp_level, p.xp_points)
            }
            _ => USAGE.to_owned(),
        }
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
                let value = match value {
                    Some("gametime") => self.tick as i64,
                    Some("day") => self.day_time.div_euclid(time::DAY_TICKS),
                    _ => self.day_time.rem_euclid(time::DAY_TICKS),
                };
                return format!("The time is {value}");
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

/// One node of a Brigadier command tree.
enum Node {
    Root,
    Literal(&'static str),
    /// An argument: its name, parser ID and parser properties.
    Argument(&'static str, i32, Vec<u8>),
}

struct TreeNode {
    node: Node,
    executable: bool,
    children: Vec<usize>,
    redirect: Option<usize>,
}

/// Builds a command tree, nodes listed parents first.
#[derive(Default)]
struct Tree {
    nodes: Vec<TreeNode>,
}

impl Tree {
    fn add(&mut self, parent: Option<usize>, node: Node, executable: bool) -> usize {
        let index = self.nodes.len();
        self.nodes.push(TreeNode {
            node,
            executable,
            children: Vec::new(),
            redirect: None,
        });
        if let Some(parent) = parent {
            self.nodes[parent].children.push(index);
        }
        index
    }

    fn literal(&mut self, parent: usize, name: &'static str, executable: bool) -> usize {
        self.add(Some(parent), Node::Literal(name), executable)
    }

    fn argument(
        &mut self,
        parent: usize,
        name: &'static str,
        parser: i32,
        props: Vec<u8>,
    ) -> usize {
        self.add(Some(parent), Node::Argument(name, parser, props), true)
    }

    fn packet(&self) -> Writer {
        const ROOT: u8 = 0;
        const LITERAL: u8 = 1;
        const ARGUMENT: u8 = 2;
        const EXECUTABLE: u8 = 0x04;
        const REDIRECT: u8 = 0x08;
        let mut packet = Writer::packet(clientbound::COMMANDS).varint(self.nodes.len() as i32);
        for n in &self.nodes {
            let kind = match n.node {
                Node::Root => ROOT,
                Node::Literal(_) => LITERAL,
                Node::Argument(..) => ARGUMENT,
            };
            let mut flags = kind;
            if n.executable {
                flags |= EXECUTABLE;
            }
            if n.redirect.is_some() {
                flags |= REDIRECT;
            }
            packet = packet.u8(flags).varint(n.children.len() as i32);
            for &child in &n.children {
                packet = packet.varint(child as i32);
            }
            if let Some(target) = n.redirect {
                packet = packet.varint(target as i32);
            }
            packet = match &n.node {
                Node::Root => packet,
                Node::Literal(name) => packet.string(name),
                Node::Argument(name, parser, props) => {
                    packet.string(name).varint(*parser).bytes(props)
                }
            };
        }
        packet.varint(0) // the root is node 0
    }
}

/// Properties of a `brigadier:integer` argument with a minimum.
fn at_least(min: i32) -> Vec<u8> {
    let mut props = vec![0x01];
    props.extend_from_slice(&min.to_be_bytes());
    props
}

/// The syntax of every command, for the client's tab completion.
/// The commands a player may use, for their client's tab completion: everything for
/// operators, only `/help` and `/list` for others.
pub(super) fn command_tree(op: bool) -> Writer {
    let integer = command_argument::BRIGADIER_INTEGER;
    let word = command_argument::BRIGADIER_STRING;
    let mut t = Tree::default();
    let root = t.add(None, Node::Root, false);
    t.literal(root, "help", true);
    t.literal(root, "list", true);
    if !op {
        return t.packet();
    }

    // Running the server.
    for name in ["op", "deop", "pardon"] {
        let node = t.literal(root, name, false);
        t.argument(node, "player", word, vec![0]); // a single word
    }
    for name in ["ban", "kick"] {
        let node = t.literal(root, name, false);
        let player = t.argument(node, "player", word, vec![0]);
        t.argument(player, "reason", word, vec![2]); // the rest of the line
    }
    let whitelist = t.literal(root, "whitelist", false);
    for name in ["on", "off", "list"] {
        t.literal(whitelist, name, true);
    }
    for name in ["add", "remove"] {
        let node = t.literal(whitelist, name, false);
        t.argument(node, "player", word, vec![0]);
    }
    let say = t.literal(root, "say", false);
    t.argument(say, "message", word, vec![2]);

    let gamemode = t.literal(root, "gamemode", false);
    for mode in ["survival", "creative"] {
        t.literal(gamemode, mode, true);
    }
    let gm = t.literal(root, "gm", false);
    t.nodes[gm].redirect = Some(gamemode);

    let give = t.literal(root, "give", false);
    let item = t.argument(give, "item", command_argument::ITEM_STACK, Vec::new());
    t.argument(item, "count", integer, at_least(1));

    t.literal(root, "kill", true);
    let summon = t.literal(root, "summon", false);
    for mob in mobs::SUMMONABLE {
        t.literal(summon, mob, true);
    }
    let time = t.literal(root, "time", false);
    let set = t.literal(time, "set", false);
    for name in ["day", "noon", "night", "midnight"] {
        t.literal(set, name, true);
    }
    t.argument(set, "time", integer, at_least(0));
    let add = t.literal(time, "add", false);
    t.argument(add, "time", integer, at_least(0));
    let query = t.literal(time, "query", false);
    for name in ["daytime", "gametime", "day"] {
        t.literal(query, name, true);
    }

    for name in ["xp", "experience"] {
        let xp = t.literal(root, name, false);
        let add = t.literal(xp, "add", false);
        let amount = t.argument(add, "amount", integer, at_least(0));
        for unit in ["points", "levels"] {
            t.literal(amount, unit, true);
        }
        t.literal(xp, "query", true);
    }

    let effect = t.literal(root, "effect", false);
    let give = t.literal(effect, "give", false);
    let mut registry = Vec::new();
    let name = b"minecraft:mob_effect";
    registry.push(name.len() as u8);
    registry.extend_from_slice(name);
    let which = t.argument(give, "effect", command_argument::RESOURCE, registry.clone());
    let seconds = t.argument(which, "seconds", integer, at_least(1));
    t.literal(which, "infinite", true);
    t.argument(seconds, "amplifier", integer, at_least(0));
    let clear = t.literal(effect, "clear", true);
    t.argument(clear, "effect", command_argument::RESOURCE, registry);

    for name in ["tp", "teleport"] {
        let tp = t.literal(root, name, false);
        t.argument(tp, "location", command_argument::VEC3, Vec::new());
    }
    let execute = t.literal(root, "execute", false);
    let within = t.literal(execute, "in", false);
    let dimension = t.argument(within, "dimension", command_argument::DIMENSION, Vec::new());
    let run = t.literal(dimension, "run", false);
    t.nodes[run].redirect = Some(root);

    let weather = t.literal(root, "weather", false);
    for kind in ["clear", "rain", "thunder"] {
        let node = t.literal(weather, kind, true);
        t.argument(node, "duration", integer, at_least(0));
    }
    t.packet()
}
