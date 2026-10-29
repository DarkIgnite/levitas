//! Who may join and who may do what: `ops.json`, `whitelist.json` and
//! `banned-players.json`, in vanilla's formats and in the server folder, so lists from
//! a vanilla, Paper or Leaf server carry over.
//!
//! Players are matched by name (ignoring case) or UUID. Operators may use every
//! command; other players only the few vanilla allows everyone (`/help`, `/list`).

use std::fs;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use serde_json::{json, Value};
use tracing::warn;

use crate::world::uuid_string;

const OPS: &str = "ops.json";
const WHITELIST: &str = "whitelist.json";
const BANS: &str = "banned-players.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub uuid: u128,
    /// Why a banned player was banned.
    pub reason: String,
}

struct Lists {
    /// Until lists are loaded from a server folder (in tests), everyone may do
    /// everything.
    loaded: bool,
    dir: PathBuf,
    ops: Vec<Entry>,
    whitelist: Vec<Entry>,
    bans: Vec<Entry>,
    whitelist_on: bool,
}

static LISTS: OnceLock<RwLock<Lists>> = OnceLock::new();

fn lists() -> &'static RwLock<Lists> {
    LISTS.get_or_init(|| {
        RwLock::new(Lists {
            loaded: false,
            dir: PathBuf::new(),
            ops: Vec::new(),
            whitelist: Vec::new(),
            bans: Vec::new(),
            whitelist_on: false,
        })
    })
}

/// Reads the lists from `dir`; `whitelist_on` is `white-list` from server.properties.
pub fn load(dir: PathBuf, whitelist_on: bool) {
    let read = |file: &str| -> Vec<Entry> {
        let Ok(text) = fs::read_to_string(dir.join(file)) else {
            return Vec::new();
        };
        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else {
            warn!("{file} is not a JSON list; ignoring it");
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|v| {
                Some(Entry {
                    name: v["name"].as_str()?.to_owned(),
                    uuid: v["uuid"]
                        .as_str()
                        .and_then(|u| u128::from_str_radix(&u.replace('-', ""), 16).ok())
                        .unwrap_or(0),
                    reason: v["reason"].as_str().unwrap_or("").to_owned(),
                })
            })
            .collect()
    };
    let mut l = lists().write().expect("access lists");
    l.ops = read(OPS);
    l.whitelist = read(WHITELIST);
    l.bans = read(BANS);
    l.whitelist_on = whitelist_on;
    l.dir = dir;
    l.loaded = true;
}

fn matches(e: &Entry, name: &str, uuid: u128) -> bool {
    e.name.eq_ignore_ascii_case(name) || (uuid != 0 && e.uuid == uuid)
}

pub fn is_op(name: &str, uuid: u128) -> bool {
    let l = lists().read().expect("access lists");
    !l.loaded || l.ops.iter().any(|e| matches(e, name, uuid))
}

/// Why a player may not join, if they may not.
pub fn refusal(name: &str, uuid: u128) -> Option<String> {
    let l = lists().read().expect("access lists");
    if let Some(ban) = l.bans.iter().find(|e| matches(e, name, uuid)) {
        let reason = if ban.reason.is_empty() {
            "Banned by an operator."
        } else {
            &ban.reason
        };
        return Some(format!(
            "You are banned from this server.\nReason: {reason}"
        ));
    }
    let listed = l.whitelist.iter().any(|e| matches(e, name, uuid))
        || l.ops.iter().any(|e| matches(e, name, uuid));
    if l.whitelist_on && !listed {
        return Some("You are not white-listed on this server!".to_owned());
    }
    None
}

/// The lists that can be changed by command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum List {
    Ops,
    Whitelist,
    Bans,
}

/// Adds a player to a list. Returns whether they were not on it yet.
pub fn add(list: List, name: &str, uuid: u128, reason: &str) -> bool {
    let mut l = lists().write().expect("access lists");
    let entries = entries(&mut l, list);
    if entries.iter().any(|e| matches(e, name, uuid)) {
        return false;
    }
    entries.push(Entry {
        name: name.to_owned(),
        uuid,
        reason: reason.to_owned(),
    });
    save(&l, list);
    true
}

/// Takes a player off a list. Returns whether they were on it.
pub fn remove(list: List, name: &str) -> bool {
    let mut l = lists().write().expect("access lists");
    let entries = entries(&mut l, list);
    let before = entries.len();
    entries.retain(|e| !e.name.eq_ignore_ascii_case(name));
    let removed = entries.len() != before;
    if removed {
        save(&l, list);
    }
    removed
}

pub fn names(list: List) -> Vec<String> {
    let mut l = lists().write().expect("access lists");
    entries(&mut l, list)
        .iter()
        .map(|e| e.name.clone())
        .collect()
}

pub fn set_whitelist(on: bool) {
    lists().write().expect("access lists").whitelist_on = on;
}

fn entries(l: &mut Lists, list: List) -> &mut Vec<Entry> {
    match list {
        List::Ops => &mut l.ops,
        List::Whitelist => &mut l.whitelist,
        List::Bans => &mut l.bans,
    }
}

fn save(l: &Lists, list: List) {
    if l.dir.as_os_str().is_empty() {
        return; // not loaded from a folder (tests)
    }
    let (file, entries) = match list {
        List::Ops => (OPS, &l.ops),
        List::Whitelist => (WHITELIST, &l.whitelist),
        List::Bans => (BANS, &l.bans),
    };
    let json: Vec<Value> = entries
        .iter()
        .map(|e| {
            let uuid = uuid_string(e.uuid);
            match list {
                List::Ops => json!({
                    "uuid": uuid, "name": e.name, "level": 4, "bypassesPlayerLimit": false
                }),
                List::Whitelist => json!({ "uuid": uuid, "name": e.name }),
                List::Bans => json!({
                    "uuid": uuid, "name": e.name, "created": "", "source": "Server",
                    "expires": "forever", "reason": e.reason
                }),
            }
        })
        .collect();
    let text = serde_json::to_string_pretty(&json).unwrap_or_default();
    if let Err(e) = fs::write(l.dir.join(file), text) {
        warn!("could not save {file}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_bans_and_the_whitelist_decide_who_joins() {
        assert!(add(List::Ops, "Alice", 1, ""));
        assert!(!add(List::Ops, "alice", 0, ""), "already an operator");
        assert!(is_op("ALICE", 0));
        assert!(add(List::Bans, "Mallory", 3, "griefing"));
        assert!(refusal("Mallory", 3).unwrap().contains("griefing"));
        set_whitelist(true);
        assert!(refusal("Bob", 2).is_some());
        assert!(refusal("Alice", 1).is_none(), "operators are always let in");
        add(List::Whitelist, "Bob", 2, "");
        assert!(refusal("Bob", 2).is_none());
        set_whitelist(false);
        assert!(remove(List::Bans, "mallory"));
        assert!(refusal("Mallory", 3).is_none());
        remove(List::Ops, "Alice");
        remove(List::Whitelist, "Bob");
    }
}
