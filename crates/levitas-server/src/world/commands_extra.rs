//! More of vanilla's commands, from the Minecraft Wiki's command pages: players picked
//! by name or selector (`@s`, `@a`, `@p`, `@r`), `/give` and `/gamemode` and `/kill` for
//! others, `/clear`, `/tp` to and between players, `/msg` and `/me`, `/setblock`, `/fill`,
//! `/enchant`, `/setworldspawn`, `/seed` and `/save-all`.
//!
//! Each dimension is a world of its own here, so a target is a player in the same
//! dimension as whoever ran the command.

use super::*;
use crate::enchanting::definition;

/// The most blocks `/fill` changes at once when the game rule is not lower.
const FILL_LIMIT_DEFAULT: i32 = 32_768;

/// What a coordinate argument can be: `~` and `~1.5` are relative to where the sender is.
fn coordinate(word: &str, own: f64) -> Option<f64> {
    match word.strip_prefix('~') {
        Some("") => Some(own),
        Some(offset) => offset.parse::<f64>().ok().map(|o| own + o),
        None => word.parse().ok(),
    }
}

/// Whether a word is the start of a coordinate (`12`, `-3.5`, `~`, `~2`).
fn looks_like_coordinate(word: &str) -> bool {
    word.starts_with('~') || word.parse::<f64>().is_ok()
}

/// A block as commands write it: `oak_stairs`, `minecraft:oak_stairs[facing=north]`.
fn parse_block(spec: &str) -> Result<u16, String> {
    let (name, props) = match spec.split_once('[') {
        Some((name, rest)) => (name, rest.strip_suffix(']').ok_or("Expected ']'")?),
        None => (spec, ""),
    };
    let full = if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    };
    let info = BlockInfo::by_name(&full).ok_or_else(|| format!("Unknown block type '{name}'"))?;
    let mut pairs = Vec::new();
    for part in props.split(',').filter(|p| !p.is_empty()) {
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| format!("Expected '=' in '{part}'"))?;
        let known = info
            .properties
            .iter()
            .find(|(k, _)| *k == key.trim())
            .ok_or_else(|| format!("{name} does not have the property '{key}'"))?;
        if !known.1.contains(&value.trim()) {
            return Err(format!("'{value}' is not a value of '{key}' for {name}"));
        }
        pairs.push((key.trim(), value.trim()));
    }
    Ok(info.state_with(pairs))
}

/// The plain text of a title argument: bare words, a quoted string or `{"text":"..."}`.
fn title_text(words: &[&str]) -> String {
    let raw = words.join(" ");
    if let Ok(serde_json::Value::String(s)) = serde_json::from_str(&raw) {
        return s;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
        if let Some(s) = v.get("text").and_then(serde_json::Value::as_str) {
            return s.to_owned();
        }
    }
    raw
}

impl World {
    /// `/spawnpoint [targets] [x y z]`: where the players come back after dying, bed or
    /// not (the overworld; a bed set later takes over).
    fn spawnpoint_command(&mut self, id: i32, args: &[&str]) -> String {
        let (who, coords) = match args {
            [] => ("@s", &[][..]),
            [who] if who.starts_with('@') || !looks_like_coordinate(who) => (*who, &[][..]),
            [who, rest @ ..] if !looks_like_coordinate(who) => (*who, rest),
            coords => ("@s", coords),
        };
        let at = if coords.is_empty() {
            self.block_pos(id, &["~", "~", "~"])
        } else {
            self.block_pos(id, coords)
        };
        let Some(at) = at else {
            return "Usage: /spawnpoint [targets] [x y z]".to_owned();
        };
        let ids = match self.targets(id, who) {
            Ok(ids) => ids,
            Err(e) => return e,
        };
        for &target in &ids {
            if let Some(p) = self.players.get_mut(&target) {
                p.respawn = Some(at);
                p.respawn_forced = true;
            }
        }
        format!(
            "Set spawn point to {}, {}, {} for {}",
            at.x,
            at.y,
            at.z,
            self.names_of(&ids)
        )
    }

    /// `/title <targets> title|subtitle|actionbar <text>`, `clear`, `reset` and
    /// `times <fade in> <stay> <fade out>` (ticks).
    fn title_command(&mut self, id: i32, args: &[&str]) -> String {
        const USAGE: &str =
            "Usage: /title <targets> title|subtitle|actionbar|clear|reset|times ...";
        let (Some(&who), Some(&what)) = (args.first(), args.get(1)) else {
            return USAGE.to_owned();
        };
        let ids = match self.targets(id, who) {
            Ok(ids) => ids,
            Err(e) => return e,
        };
        let make: Box<dyn Fn() -> Writer> = match what {
            "title" | "subtitle" | "actionbar" if args.len() > 2 => {
                let text = title_text(&args[2..]);
                let id = match what {
                    "title" => clientbound::SET_TITLE_TEXT,
                    "subtitle" => clientbound::SET_SUBTITLE_TEXT,
                    _ => clientbound::SET_ACTION_BAR_TEXT,
                };
                Box::new(move || Writer::packet(id).text(&text))
            }
            "clear" => Box::new(|| Writer::packet(clientbound::CLEAR_TITLES).bool(false)),
            "reset" => Box::new(|| Writer::packet(clientbound::CLEAR_TITLES).bool(true)),
            "times" => {
                let times: Vec<i32> = args[2..].iter().filter_map(|a| a.parse().ok()).collect();
                let &[fade_in, stay, fade_out] = &times[..] else {
                    return "Usage: /title <targets> times <fade in> <stay> <fade out>".to_owned();
                };
                Box::new(move || {
                    Writer::packet(clientbound::SET_TITLES_ANIMATION)
                        .i32(fade_in)
                        .i32(stay)
                        .i32(fade_out)
                })
            }
            _ => return USAGE.to_owned(),
        };
        for &target in &ids {
            self.send(target, make());
        }
        format!("Showed the title to {}", self.names_of(&ids))
    }

    /// The players a target word names, in this dimension: a name, `@s` (the sender),
    /// `@a` (everyone), `@p` (the nearest to the sender) or `@r` (a random one).
    pub(super) fn targets(&mut self, sender: i32, word: &str) -> Result<Vec<i32>, String> {
        let found: Vec<i32> = match word {
            "@s" => vec![sender],
            "@a" => {
                let mut all: Vec<i32> = self.players.keys().copied().collect();
                all.sort_unstable();
                all
            }
            "@p" => {
                let from = self.players.get(&sender).map_or([0.0; 3], |p| p.pos);
                let near = self
                    .players
                    .iter()
                    .min_by(|a, b| {
                        let d = |p: [f64; 3]| (0..3).map(|i| (p[i] - from[i]).powi(2)).sum::<f64>();
                        d(a.1.pos).total_cmp(&d(b.1.pos))
                    })
                    .map(|(&id, _)| id);
                near.into_iter().collect()
            }
            "@r" => {
                let mut all: Vec<i32> = self.players.keys().copied().collect();
                all.sort_unstable();
                if all.is_empty() {
                    all
                } else {
                    vec![all[self.rng.below(all.len() as u32) as usize]]
                }
            }
            name if name.starts_with('@') => {
                return Err(format!("Unsupported selector {name}: use @s, @a, @p or @r"));
            }
            name => self
                .players
                .iter()
                .filter(|(_, p)| p.profile.name.eq_ignore_ascii_case(name))
                .map(|(&id, _)| id)
                .collect(),
        };
        if found.is_empty() {
            Err("No player was found".to_owned())
        } else {
            Ok(found)
        }
    }

    fn names_of(&self, ids: &[i32]) -> String {
        match ids {
            [one] => self.players[one].profile.name.clone(),
            many => format!("{} players", many.len()),
        }
    }

    /// A block position from three words, relative to the sender's block.
    fn block_pos(&self, sender: i32, words: &[&str]) -> Option<BlockPos> {
        let own = self.players.get(&sender)?.pos;
        let c = |i: usize| coordinate(words.get(i)?, own[i]).map(|v| v.floor() as i32);
        Some(BlockPos::new(c(0)?, c(1)?, c(2)?))
    }

    /// Runs one of the commands of this module; `None` if `words` is not one, or is a form
    /// the older code in `commands.rs` handles.
    pub(super) fn extra_command(&mut self, id: i32, words: &[&str]) -> Option<String> {
        let (&name, args) = words.split_first()?;
        Some(match name {
            "give" => {
                let (&who, rest) = args.split_first()?;
                if rest.is_empty() {
                    return None; // /give <item>
                }
                let ids = self.targets(id, who).ok()?;
                let count = rest.get(1).and_then(|c| c.parse().ok()).unwrap_or(1);
                let mut reply = String::new();
                for &target in &ids {
                    reply = self.give(target, rest[0], count);
                }
                if reply.starts_with("Unknown") {
                    reply
                } else {
                    format!("{} to {}", reply, self.names_of(&ids))
                }
            }
            "gamemode" | "gm" => {
                let [mode, who] = args else {
                    return None;
                };
                let mode = match *mode {
                    "survival" | "s" | "0" => GAME_MODE_SURVIVAL,
                    "creative" | "c" | "1" => GAME_MODE_CREATIVE,
                    _ => return Some("Only survival and creative exist here so far".to_owned()),
                };
                match self.targets(id, who) {
                    Ok(ids) => {
                        for &target in &ids {
                            self.set_game_mode(target, mode);
                        }
                        format!("Set {} game mode", self.names_of(&ids))
                    }
                    Err(e) => e,
                }
            }
            "kill" => {
                let [who] = args else {
                    return None;
                };
                match self.targets(id, who) {
                    Ok(ids) => {
                        for &target in &ids {
                            self.damage(target, f32::MAX, survival::Damage::Kill);
                        }
                        format!("Killed {}", self.names_of(&ids))
                    }
                    Err(e) => e,
                }
            }
            "clear" => self.clear_command(id, args),
            "tp" | "teleport" => return self.tp_players(id, args),
            "msg" | "tell" | "w" => {
                let (&who, text) = args.split_first()?;
                if text.is_empty() {
                    return Some("Usage: /msg <player> <message>".to_owned());
                }
                let text = text.join(" ");
                let from = self.players.get(&id)?.profile.name.clone();
                match self.targets(id, who) {
                    Ok(ids) => {
                        for &target in &ids {
                            self.send(
                                target,
                                system_chat(&format!("{from} whispers to you: {text}")),
                            );
                        }
                        format!("You whisper to {}: {text}", self.names_of(&ids))
                    }
                    // Not in this dimension: a player somewhere else on the server.
                    Err(e) => match self.tab.name_of(who).filter(|_| !who.starts_with('@')) {
                        Some(name) => {
                            self.direct_elsewhere(
                                &name,
                                system_chat(&format!("{from} whispers to you: {text}")),
                            );
                            format!("You whisper to {name}: {text}")
                        }
                        None => e,
                    },
                }
            }
            "me" => {
                if args.is_empty() {
                    return Some("Usage: /me <action>".to_owned());
                }
                let from = self.players.get(&id)?.profile.name.clone();
                self.announce(system_chat(&format!("* {from} {}", args.join(" "))));
                return Some(String::new());
            }
            "seed" => format!("Seed: [{}]", self.seed),
            "save-all" => {
                self.save_all();
                "Saved the game".to_owned()
            }
            "setworldspawn" => {
                let at = if args.is_empty() {
                    self.block_pos(id, &["~", "~", "~"])?
                } else {
                    match self.block_pos(id, args) {
                        Some(pos) => pos,
                        None => return Some("Usage: /setworldspawn [x y z]".to_owned()),
                    }
                };
                self.spawn = [
                    f64::from(at.x) + 0.5,
                    f64::from(at.y),
                    f64::from(at.z) + 0.5,
                ];
                format!("Set the world spawn point to {}, {}, {}", at.x, at.y, at.z)
            }
            "title" => self.title_command(id, args),
            "spawnpoint" => self.spawnpoint_command(id, args),
            "setblock" => self.setblock_command(id, args),
            "fill" => self.fill_command(id, args),
            "enchant" => self.enchant_command(id, args),
            _ => return None,
        })
    }

    /// `/tp <player>`, `/tp <player> <destination>`, `/tp <player> <x> <y> <z>`. The old
    /// code takes the forms with three coordinates only.
    fn tp_players(&mut self, id: i32, args: &[&str]) -> Option<String> {
        if args.is_empty() || (args.len() == 3 && args.iter().all(|a| looks_like_coordinate(a))) {
            return None;
        }
        let reply = (|| -> Result<String, String> {
            let (who, destination): (Vec<i32>, [f64; 3]) = match args {
                [to] => {
                    let to = self.targets(id, to)?;
                    let pos = self.players[&to[0]].pos;
                    (vec![id], pos)
                }
                [who, to] => {
                    let who = self.targets(id, who)?;
                    let to = self.targets(id, to)?;
                    let pos = self.players[&to[0]].pos;
                    (who, pos)
                }
                [who, x, y, z] => {
                    let who = self.targets(id, who)?;
                    let own = self.players[&id].pos;
                    let at = [x, y, z].map(|w| *w);
                    let mut pos = [0.0; 3];
                    for i in 0..3 {
                        pos[i] = coordinate(at[i], own[i]).ok_or("Invalid coordinates")?;
                    }
                    (who, pos)
                }
                _ => return Err("Usage: /tp <player> [<player>|<x> <y> <z>]".to_owned()),
            };
            for &target in &who {
                self.teleport(target, destination);
            }
            Ok(format!("Teleported {}", self.names_of(&who)))
        })();
        Some(reply.unwrap_or_else(|e| e))
    }

    /// `/clear [players] [item] [count]`: takes items from inventories; a count of 0 only
    /// counts them.
    fn clear_command(&mut self, id: i32, args: &[&str]) -> String {
        let (who, rest) = match args.split_first() {
            Some((&who, rest)) => (who, rest),
            None => ("@s", args),
        };
        let ids = match self.targets(id, who) {
            Ok(ids) => ids,
            Err(e) => return e,
        };
        let item = match rest.first() {
            Some(name) => {
                let full = if name.contains(':') {
                    (*name).to_owned()
                } else {
                    format!("minecraft:{name}")
                };
                match ItemInfo::id_of(&full) {
                    Some(item) => Some(item),
                    None => return format!("Unknown item: {full}"),
                }
            }
            None => None,
        };
        let limit = rest.get(1).and_then(|c| c.parse::<i32>().ok());
        let mut total = 0;
        for &target in &ids {
            let p = self.players.get_mut(&target).expect("target exists");
            let mut left = limit.unwrap_or(i32::MAX);
            for slot in 0..p.inventory.slots.len() {
                let stack = p.inventory.slots[slot];
                if stack.is_empty() || item.is_some_and(|i| i != stack.item) {
                    continue;
                }
                let take = if limit == Some(0) {
                    stack.count
                } else {
                    stack.count.min(left)
                };
                total += take;
                if limit != Some(0) {
                    left -= take;
                    p.inventory.slots[slot] = stack.with_count(stack.count - take);
                }
            }
            if limit != Some(0) {
                let content = p.inventory.content_packet();
                self.send(target, content);
            }
        }
        match (total, limit) {
            (0, _) => "No items were found".to_owned(),
            (n, Some(0)) => format!("Found {n} matching items"),
            (n, _) => format!("Removed {n} items from {}", self.names_of(&ids)),
        }
    }

    /// `/setblock <x> <y> <z> <block> [destroy|keep|replace]`.
    fn setblock_command(&mut self, id: i32, args: &[&str]) -> String {
        const USAGE: &str = "Usage: /setblock <x> <y> <z> <block> [destroy|keep|replace]";
        let (Some(pos), Some(spec)) = (self.block_pos(id, args), args.get(3)) else {
            return USAGE.to_owned();
        };
        let state = match parse_block(spec) {
            Ok(state) => state,
            Err(e) => return e,
        };
        let Some(old) = self.loaded_block(pos) else {
            return "That position is not loaded".to_owned();
        };
        match args.get(4).copied() {
            Some("keep") if !is_air(old) => return "Could not set the block".to_owned(),
            Some("destroy") => self.break_block_for_command(pos, old),
            Some("replace") | None | Some("keep") => {}
            Some(_) => return USAGE.to_owned(),
        }
        self.place_by_command(pos, state);
        format!("Changed the block at {}, {}, {}", pos.x, pos.y, pos.z)
    }

    /// Puts `state` at `pos` the way a command does: the old block's contents go, the new
    /// block's come, and neighbours are told.
    fn place_by_command(&mut self, pos: BlockPos, state: u16) {
        self.remove_block_entity(pos);
        self.set_block_updating(pos, state);
        if let Some(entity) = BlockEntity::for_block(state) {
            self.block_entities.insert(pos, entity);
        }
    }

    /// Breaks a block with its drops and effect, as `destroy` does.
    fn break_block_for_command(&mut self, pos: BlockPos, old: u16) {
        self.broadcast(
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(2001)
                .position(pos)
                .i32(i32::from(old))
                .bool(false),
            None,
        );
        self.pop_block(pos, old);
    }

    /// `/fill <x1> <y1> <z1> <x2> <y2> <z2> <block> [replace|destroy|keep|hollow|outline]`.
    fn fill_command(&mut self, id: i32, args: &[&str]) -> String {
        const USAGE: &str = "Usage: /fill <x1> <y1> <z1> <x2> <y2> <z2> <block> [replace|destroy|keep|hollow|outline]";
        let (Some(a), Some(b), Some(spec)) = (
            self.block_pos(id, args.get(..3).unwrap_or(&[])),
            self.block_pos(id, args.get(3..6).unwrap_or(&[])),
            args.get(6),
        ) else {
            return USAGE.to_owned();
        };
        let state = match parse_block(spec) {
            Ok(state) => state,
            Err(e) => return e,
        };
        let mode = args.get(7).copied().unwrap_or("replace");
        if !matches!(mode, "replace" | "destroy" | "keep" | "hollow" | "outline") {
            return USAGE.to_owned();
        }
        let (min, max) = (
            BlockPos::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
            BlockPos::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
        );
        let volume = i64::from(max.x - min.x + 1)
            * i64::from(max.y - min.y + 1)
            * i64::from(max.z - min.z + 1);
        let limit = crate::rules::int("max_block_modifications").max(0);
        let limit = if limit == 0 {
            FILL_LIMIT_DEFAULT
        } else {
            limit
        };
        if volume > i64::from(limit) {
            return format!(
                "Too many blocks in the specified area (maximum {limit}, specified {volume})"
            );
        }
        // Everything must be loaded before anything changes.
        for x in min.x..=max.x {
            for z in min.z..=max.z {
                for y in [min.y, max.y] {
                    if self.loaded_block(BlockPos::new(x, y, z)).is_none() {
                        return "That position is not loaded".to_owned();
                    }
                }
            }
        }
        let edge = |p: BlockPos| {
            p.x == min.x
                || p.x == max.x
                || p.y == min.y
                || p.y == max.y
                || p.z == min.z
                || p.z == max.z
        };
        let mut changed = 0;
        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    let pos = BlockPos::new(x, y, z);
                    let Some(old) = self.loaded_block(pos) else {
                        continue;
                    };
                    let target = match mode {
                        "hollow" if !edge(pos) => block::AIR,
                        "outline" if !edge(pos) => continue,
                        "keep" if !is_air(old) => continue,
                        _ => state,
                    };
                    if old == target {
                        continue;
                    }
                    if mode == "destroy" {
                        self.break_block_for_command(pos, old);
                    }
                    self.place_by_command(pos, target);
                    changed += 1;
                }
            }
        }
        if changed == 0 {
            "No blocks were filled".to_owned()
        } else {
            format!("{changed} blocks were filled")
        }
    }

    /// `/enchant [player] <enchantment> [level]`: on the item in the main hand.
    fn enchant_command(&mut self, id: i32, args: &[&str]) -> String {
        const USAGE: &str = "Usage: /enchant <player> <enchantment> [level]";
        let (who, rest) = match args {
            [first, rest @ ..] if !rest.is_empty() && self.targets(id, first).is_ok() => {
                (*first, rest)
            }
            _ => ("@s", args),
        };
        let Some(&wanted) = rest.first() else {
            return USAGE.to_owned();
        };
        let full = if wanted.contains(':') {
            wanted.to_owned()
        } else {
            format!("minecraft:{wanted}")
        };
        let Some(enchantment) = SyncedRegistry::get("minecraft:enchantment")
            .and_then(|r| r.id_of(&full))
            .and_then(definition)
        else {
            return format!("Unknown enchantment: {full}");
        };
        let level = rest.get(1).and_then(|l| l.parse::<u32>().ok()).unwrap_or(1);
        if level == 0 || level > enchantment.max_level {
            return format!(
                "{} does not support level {level} (maximum {})",
                full, enchantment.max_level
            );
        }
        let ids = match self.targets(id, who) {
            Ok(ids) => ids,
            Err(e) => return e,
        };
        let mut done = 0;
        for &target in &ids {
            let p = self.players.get_mut(&target).expect("target exists");
            let slot = p.hand_slot(0);
            let mut stack = p.inventory.slots[slot];
            if stack.is_empty() {
                continue;
            }
            if !enchantment.supports(stack.item)
                || stack
                    .enchants
                    .iter()
                    .any(|(other, _)| other != enchantment.id && !enchantment.compatible(other))
            {
                continue;
            }
            stack.enchants.set(enchantment.id, level);
            p.inventory.slots[slot] = stack;
            let update = p.inventory.set_slot_packet(slot);
            self.send(target, update);
            done += 1;
        }
        match done {
            0 => "Nothing changed. The target either has no item in their main hand or the \
                  enchantment cannot be applied"
                .to_owned(),
            n => format!("Applied enchantment {full} {level} to {n} target(s)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-commands-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        for cx in -1..=1 {
            for cz in -1..=1 {
                w.chunk(cx, cz);
            }
        }
        w
    }

    fn join(w: &mut World, name: &str, at: [f64; 3]) -> i32 {
        let (out, _frames) = mpsc::channel(100_000);
        std::mem::forget(_frames);
        let id = next_entity_id();
        let profile = Profile {
            name: name.to_owned(),
            uuid: id as u128,
            properties: Vec::new(),
        };
        w.players.insert(id, Player::new(profile, out, at));
        id
    }

    fn run(w: &mut World, id: i32, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        w.extra_command(id, &words)
            .unwrap_or_else(|| "(not mine)".to_owned())
    }

    #[test]
    fn selectors_pick_players() {
        let mut w = world();
        let a = join(&mut w, "Alice", [0.5, -60.0, 0.5]);
        let b = join(&mut w, "Bob", [10.5, -60.0, 0.5]);
        assert_eq!(w.targets(a, "@s"), Ok(vec![a]));
        assert_eq!(w.targets(a, "bob"), Ok(vec![b]));
        assert_eq!(w.targets(a, "@p"), Ok(vec![a]));
        assert_eq!(w.targets(b, "@p"), Ok(vec![b]));
        let all = w.targets(a, "@a").unwrap();
        assert_eq!(all.len(), 2);
        assert!(w.targets(a, "Nobody").is_err());
        assert!(w.targets(a, "@e").unwrap_err().contains("Unsupported"));
    }

    #[test]
    fn players_are_given_cleared_killed_and_moved() {
        let mut w = world();
        let a = join(&mut w, "Alice", [0.5, -60.0, 0.5]);
        let b = join(&mut w, "Bob", [10.5, -60.0, 0.5]);
        assert!(run(&mut w, a, "give Bob diamond 5").contains("Gave 5"));
        let diamond = ItemInfo::id_of("minecraft:diamond").unwrap();
        let count = |w: &World, id: i32| -> i32 {
            w.players[&id]
                .inventory
                .slots
                .iter()
                .filter(|s| s.item == diamond)
                .map(|s| s.count)
                .sum()
        };
        assert_eq!(count(&w, b), 5);
        // The older forms are left to the old code.
        assert_eq!(run(&mut w, a, "give diamond 5"), "(not mine)");
        // Counting and clearing.
        assert!(run(&mut w, a, "clear Bob diamond 0").contains("Found 5"));
        assert_eq!(count(&w, b), 5);
        assert!(run(&mut w, a, "clear Bob diamond 2").contains("Removed 2"));
        assert_eq!(count(&w, b), 3);
        assert!(run(&mut w, a, "clear Bob").contains("Removed 3"));
        assert_eq!(count(&w, b), 0);
        assert!(run(&mut w, a, "clear Bob").contains("No items"));
        // Teleporting a player to another.
        run(&mut w, a, "tp Bob Alice");
        assert!((w.players[&b].pos[0] - 0.5).abs() < 1e-9);
        assert_eq!(run(&mut w, a, "tp 1 2 3"), "(not mine)");
        // Killing.
        assert!(run(&mut w, a, "kill Bob").contains("Killed Bob"));
        assert!(!w.players[&b].alive);
        assert!(run(&mut w, a, "gamemode creative Alice").contains("game mode"));
        assert_eq!(w.players[&a].mode, GAME_MODE_CREATIVE);
    }

    #[test]
    fn blocks_are_set_and_filled() {
        let mut w = world();
        let a = join(&mut w, "Alice", [0.5, -60.0, 0.5]);
        assert!(run(&mut w, a, "setblock 3 -60 3 stone").contains("Changed"));
        assert_eq!(w.block(BlockPos::new(3, -60, 3)), block::STONE);
        assert!(run(
            &mut w,
            a,
            "setblock ~2 ~ ~ oak_stairs[facing=north,half=top]"
        )
        .contains("Changed"));
        let stairs = w.block(BlockPos::new(2, -60, 0));
        assert_eq!(crate::blocks::prop(stairs, "facing"), Some("north"));
        assert_eq!(crate::blocks::prop(stairs, "half"), Some("top"));
        assert!(run(&mut w, a, "setblock 3 -60 3 dirt keep").contains("Could not"));
        assert!(run(&mut w, a, "setblock 3 -60 3 nonsense").contains("Unknown block"));
        assert!(run(&mut w, a, "setblock 3 -60 3 oak_stairs[facing=up]").contains("not a value"));
        // A hollow box: only its shell is made.
        assert!(run(&mut w, a, "fill 5 -60 5 9 -56 9 glass hollow").contains("blocks were filled"));
        assert_eq!(
            BlockInfo::of(w.block(BlockPos::new(5, -60, 5))).name,
            "minecraft:glass"
        );
        assert!(is_air(w.block(BlockPos::new(7, -58, 7))));
        // Too much at once.
        assert!(run(&mut w, a, "fill 0 -60 0 100 100 100 stone").contains("Too many blocks"));
        // Outside what is loaded.
        assert!(run(&mut w, a, "fill 1000 -60 1000 1001 -60 1001 stone").contains("not loaded"));
    }

    #[test]
    fn enchanting_the_item_in_hand() {
        let mut w = world();
        let a = join(&mut w, "Alice", [0.5, -60.0, 0.5]);
        assert!(run(&mut w, a, "enchant Alice sharpness 3").contains("Nothing changed"));
        let sword = ItemInfo::id_of("minecraft:diamond_sword").unwrap();
        let slot = HOTBAR + w.players[&a].held;
        w.players.get_mut(&a).unwrap().inventory.slots[slot] = Stack::new(sword, 1);
        assert!(run(&mut w, a, "enchant Alice sharpness 3").contains("Applied"));
        assert_eq!(
            w.players[&a].inventory.slots[slot]
                .enchants
                .level_of("minecraft:sharpness"),
            3
        );
        assert!(run(&mut w, a, "enchant sharpness 9").contains("does not support level 9"));
        assert!(run(&mut w, a, "enchant Alice nonsense").contains("Unknown enchantment"));
        // Smite does not go with sharpness.
        assert!(run(&mut w, a, "enchant Alice smite 1").contains("Nothing changed"));
    }

    #[test]
    fn talking_and_the_world_spawn() {
        let mut w = world();
        let a = join(&mut w, "Alice", [0.5, -60.0, 0.5]);
        let _b = join(&mut w, "Bob", [10.5, -60.0, 0.5]);
        assert!(run(&mut w, a, "msg Bob hello there").contains("You whisper to Bob: hello there"));
        assert!(run(&mut w, a, "msg Nobody hi").contains("No player"));
        assert_eq!(run(&mut w, a, "me waves"), "");
        assert!(run(&mut w, a, "seed").starts_with("Seed: ["));
        assert!(run(&mut w, a, "setworldspawn 4 -60 4").contains("4, -60, 4"));
        assert_eq!(w.spawn, [4.5, -60.0, 4.5]);
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;

    #[test]
    fn title_text_takes_words_quotes_and_json() {
        assert_eq!(title_text(&["Hello", "there"]), "Hello there");
        assert_eq!(title_text(&["\"Quoted", "words\""]), "Quoted words");
        assert_eq!(
            title_text(&["{\"text\":\"Json\",\"color\":\"red\"}"]),
            "Json"
        );
    }
}

#[cfg(test)]
mod spawnpoint_tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn spawnpoint_sets_a_respawn_that_needs_no_bed() {
        let dir = std::env::temp_dir().join(format!("levitas-sp-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let profile = Profile {
            uuid: 3,
            name: "Homer".to_owned(),
            properties: Vec::new(),
        };
        w.players
            .insert(1, Player::new(profile, tx, [0.5, -60.0, 0.5]));
        let said = w.spawnpoint_command(1, &["Homer", "10", "-60", "12"]);
        assert!(said.contains("10, -60, 12"), "{said}");
        let at = w.respawn_point(1);
        assert_eq!(at, [10.5, -60.0, 12.5]);
        // It survives saving and loading.
        let nbt = w.players[&1].to_nbt();
        let mut p = Player::new(
            Profile {
                uuid: 3,
                name: "Homer".to_owned(),
                properties: Vec::new(),
            },
            tokio::sync::mpsc::channel(1).0,
            [0.0; 3],
        );
        p.load_nbt(&nbt);
        assert!(p.respawn_forced);
        assert_eq!(p.respawn, Some(BlockPos::new(10, -60, 12)));
    }
}
