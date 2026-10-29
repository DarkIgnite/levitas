//! Custom names, as anvils and name tags give them.
//!
//! Stacks are small `Copy` values passed around everywhere, so a name is kept as a
//! `&'static str` from an interner: each distinct name is stored once for the life of
//! the server. Typing in an anvil's name box sends every prefix, so the interner is
//! capped; past the cap, new names are not taken.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use levitas_nbt::Tag;

/// The longest name an anvil gives, in characters.
pub const MAX_LEN: usize = 50;
/// How many distinct names are kept at most.
const CAPACITY: usize = 1 << 16;

/// The interned copy of `name`, or `None` if the interner is full.
pub fn intern(name: &str) -> Option<&'static str> {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .expect("the name interner");
    if let Some(&known) = names.get(name) {
        return Some(known);
    }
    if names.len() >= CAPACITY {
        return None;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    Some(leaked)
}

/// A name as a player may give it: no formatting codes or control characters, and at
/// most [`MAX_LEN`] characters.
pub fn clean(name: &str) -> String {
    name.chars()
        .filter(|&c| c != '\u{a7}' && !c.is_control())
        .take(MAX_LEN)
        .collect()
}

/// The plain text of a saved text component: NBT (1.21.5 and later) or JSON in a string
/// (older worlds).
pub fn plain_text(tag: &Tag) -> String {
    fn of_json(v: &serde_json::Value) -> String {
        match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(parts) => parts.iter().map(of_json).collect(),
            serde_json::Value::Object(o) => {
                let mut text = o.get("text").map(of_json).unwrap_or_default();
                if let Some(extra) = o.get("extra") {
                    text.push_str(&of_json(extra));
                }
                text
            }
            _ => String::new(),
        }
    }
    match tag {
        Tag::String(s) if s.starts_with('{') || s.starts_with('"') || s.starts_with('[') => {
            serde_json::from_str(s).map_or_else(|_| s.clone(), |v| of_json(&v))
        }
        Tag::String(s) => s.clone(),
        Tag::Compound(c) => {
            let mut text = c.str("text").unwrap_or("").to_owned();
            for part in c.list("extra").unwrap_or_default() {
                text.push_str(&plain_text(part));
            }
            text
        }
        Tag::List(parts) => parts.iter().map(plain_text).collect(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_stored_once_and_cleaned() {
        let a = intern("Excalibur").unwrap();
        let b = intern(&String::from("Excalibur")).unwrap();
        assert!(std::ptr::eq(a, b));
        assert_eq!(clean("\u{a7}cRed\u{7}"), "cRed");
        assert_eq!(clean(&"x".repeat(80)).len(), MAX_LEN);
        assert_eq!(plain_text(&Tag::String(r#"{"text":"Bob"}"#.into())), "Bob");
    }
}
