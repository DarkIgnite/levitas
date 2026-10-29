//! Signs and hanging signs, following the Minecraft Wiki: placing one opens its editor,
//! right-clicking either side edits that side again, dyes colour the text, glow ink
//! sacs make it glow (ink sacs undo that), and honeycomb waxes the sign so it can no
//! longer be changed.

use std::collections::HashSet;

use levitas_nbt::{Compound, Tag};
use levitas_registry::block_entity_type;

use super::*;
use crate::blocks;

/// The longest line accepted from a client; their editor keeps lines far shorter.
const MAX_LINE: usize = 384;

/// The sixteen dye colours, which sign text comes in.
const COLORS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub lines: [String; 4],
    pub color: &'static str,
    pub glowing: bool,
}

impl Default for Side {
    fn default() -> Self {
        Side {
            lines: Default::default(),
            color: "black",
            glowing: false,
        }
    }
}

impl Side {
    fn to_nbt(&self) -> Compound {
        Compound::new()
            .with(
                "messages",
                self.lines
                    .iter()
                    .map(|l| Tag::String(l.clone()))
                    .collect::<Vec<_>>(),
            )
            .with("color", self.color)
            .with("has_glowing_text", self.glowing)
    }

    fn from_nbt(nbt: Option<&Compound>) -> Side {
        let mut side = Side::default();
        let Some(nbt) = nbt else {
            return side;
        };
        for (line, message) in side
            .lines
            .iter_mut()
            .zip(nbt.list("messages").unwrap_or_default())
        {
            *line = plain_text(message);
        }
        side.color = nbt
            .str("color")
            .and_then(|c| COLORS.iter().find(|&&k| k == c))
            .copied()
            .unwrap_or("black");
        side.glowing = nbt.int("has_glowing_text").unwrap_or(0) != 0;
        side
    }
}

/// The plain text of a saved message: a text component as NBT (1.21.5 and later), or
/// as JSON in a string (older worlds).
fn plain_text(tag: &Tag) -> String {
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

pub struct Sign {
    pub hanging: bool,
    pub front: Side,
    pub back: Side,
    pub waxed: bool,
    /// The player with the editor open, who alone may change the text.
    pub editor: Option<i32>,
    pub viewers: HashSet<i32>,
}

impl Sign {
    pub fn new(hanging: bool) -> Self {
        Sign {
            hanging,
            front: Side::default(),
            back: Side::default(),
            waxed: false,
            editor: None,
            viewers: HashSet::new(),
        }
    }

    pub fn type_id(&self) -> i32 {
        if self.hanging {
            block_entity_type::HANGING_SIGN
        } else {
            block_entity_type::SIGN
        }
    }

    pub fn id(&self) -> &'static str {
        if self.hanging {
            "minecraft:hanging_sign"
        } else {
            "minecraft:sign"
        }
    }

    pub fn write_nbt(&self, nbt: &mut Compound) {
        nbt.insert("front_text", self.front.to_nbt());
        nbt.insert("back_text", self.back.to_nbt());
        nbt.insert("is_waxed", self.waxed);
    }

    pub fn read_nbt(hanging: bool, nbt: &Compound) -> Self {
        Sign {
            front: Side::from_nbt(nbt.compound("front_text")),
            back: Side::from_nbt(nbt.compound("back_text")),
            waxed: nbt.int("is_waxed").unwrap_or(0) != 0,
            ..Sign::new(hanging)
        }
    }

    /// What clients need to draw the text.
    pub fn network_data(&self) -> Compound {
        let mut nbt = Compound::new();
        self.write_nbt(&mut nbt);
        nbt
    }

    fn side_mut(&mut self, front: bool) -> &mut Side {
        if front {
            &mut self.front
        } else {
            &mut self.back
        }
    }
}

/// Whether a block is a sign of any kind, and whether it hangs.
pub fn sign_kind(state: u16) -> Option<bool> {
    let name = BlockInfo::of(state).name;
    name.ends_with("_sign")
        .then(|| name.ends_with("_hanging_sign"))
}

/// Whether a player at `eye` (only its x and z matter) looks at the front of the sign at `pos`: the side its text
/// faces, from the block's rotation or facing.
fn front_faces(state: u16, pos: BlockPos, eye: [f64; 3]) -> bool {
    let normal = match blocks::prop(state, "rotation").and_then(|r| r.parse::<f64>().ok()) {
        Some(r) => {
            // Rotation 0 faces south (+z), and each step turns 22.5 degrees clockwise.
            let angle = (r * 22.5).to_radians();
            [-angle.sin(), angle.cos()]
        }
        None => match blocks::prop(state, "facing") {
            Some("north") => [0.0, -1.0],
            Some("south") => [0.0, 1.0],
            Some("west") => [-1.0, 0.0],
            _ => [1.0, 0.0],
        },
    };
    let dx = eye[0] - (f64::from(pos.x) + 0.5);
    let dz = eye[2] - (f64::from(pos.z) + 0.5);
    dx * normal[0] + dz * normal[1] >= 0.0
}

impl World {
    /// A sign was just placed: its placer gets the editor for the front.
    pub(super) fn sign_placed(&mut self, id: i32, pos: BlockPos) {
        if let Some(BlockEntity::Sign(sign)) = self.block_entities.get_mut(&pos) {
            sign.editor = Some(id);
            self.send(id, open_editor(pos, true));
        }
    }

    /// Right-clicking a sign: dyeing, glowing or waxing it with the item in hand, or
    /// else editing the side the player faces. Returns whether the click was used.
    pub(super) fn use_sign(&mut self, id: i32, slot: usize, pos: BlockPos, state: u16) -> bool {
        if sign_kind(state).is_none() {
            return false;
        }
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let stack = p.inventory.slots[slot];
        let survival = p.survival();
        let front = front_faces(state, pos, p.pos);
        let item = ItemInfo::get(stack.item).map_or("", |i| i.name);
        let Some(BlockEntity::Sign(sign)) = self.block_entities.get_mut(&pos) else {
            return false;
        };
        if sign.waxed {
            return true; // waxed signs take no changes, and open no editor
        }
        let side = sign.side_mut(front);
        let dye = item
            .strip_prefix("minecraft:")
            .and_then(|n| n.strip_suffix("_dye"))
            .and_then(|c| COLORS.iter().find(|&&k| k == c).copied());
        let applied = match (item, dye) {
            (_, Some(color)) if side.color != color => {
                side.color = color;
                true
            }
            ("minecraft:glow_ink_sac", _) if !side.glowing => {
                side.glowing = true;
                true
            }
            ("minecraft:ink_sac", _) if side.glowing => {
                side.glowing = false;
                true
            }
            ("minecraft:honeycomb", _) => {
                sign.waxed = true;
                true
            }
            _ => false,
        };
        if applied {
            if survival {
                let p = self.players.get_mut(&id).expect("player exists");
                p.inventory.take(slot, 1);
                let update = p.inventory.set_slot_packet(slot);
                self.send(id, update);
            }
            self.sign_changed(pos);
            return true;
        }
        // Someone else is already writing on it.
        if sign
            .editor
            .is_some_and(|e| e != id && self.players.contains_key(&e))
        {
            return true;
        }
        sign.editor = Some(id);
        self.send(id, open_editor(pos, front));
        true
    }

    /// The text a player typed into a sign's editor.
    pub(super) fn update_sign(&mut self, id: i32, pos: BlockPos, front: bool, lines: [String; 4]) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let centre = [pos.x, pos.y, pos.z].map(|v| f64::from(v) + 0.5);
        let near = (0..3).all(|i| (p.pos[i] - centre[i]).abs() <= 8.0);
        let Some(BlockEntity::Sign(sign)) = self.block_entities.get_mut(&pos) else {
            return;
        };
        if !near || sign.waxed || sign.editor != Some(id) {
            return;
        }
        sign.editor = None;
        let side = sign.side_mut(front);
        for (line, text) in side.lines.iter_mut().zip(lines) {
            // Formatting codes are not for players to type.
            *line = text
                .chars()
                .filter(|&c| c != '\u{a7}' && !c.is_control())
                .take(MAX_LINE)
                .collect();
        }
        self.sign_changed(pos);
    }

    /// Shows a sign's new text to everyone and saves it.
    fn sign_changed(&mut self, pos: BlockPos) {
        let Some(BlockEntity::Sign(sign)) = self.block_entities.get(&pos) else {
            return;
        };
        let data = levitas_nbt::to_network(&Tag::Compound(sign.network_data()));
        let kind = sign.type_id();
        self.broadcast(
            Writer::packet(clientbound::BLOCK_ENTITY_DATA)
                .position(pos)
                .varint(kind)
                .bytes(&data),
            None,
        );
        self.dirty.insert(pos.chunk());
    }
}

fn open_editor(pos: BlockPos, front: bool) -> Writer {
    Writer::packet(clientbound::OPEN_SIGN_EDITOR)
        .position(pos)
        .bool(front)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-signs-{}-{}",
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

    fn player(w: &mut World, name: &str, at: [f64; 3]) -> i32 {
        let (out, frames) = mpsc::channel(100_000);
        std::mem::forget(frames); // keep the channel open
        let id = next_entity_id();
        let profile = Profile {
            name: name.to_owned(),
            uuid: id as u128,
            properties: Vec::new(),
        };
        w.players.insert(id, Player::new(profile, out, at));
        id
    }

    fn give(w: &mut World, id: i32, item: &str) {
        let p = w.players.get_mut(&id).unwrap();
        let slot = p.hand_slot(0);
        p.inventory.slots[slot] =
            Stack::new(ItemInfo::id_of(&format!("minecraft:{item}")).unwrap(), 4);
    }

    fn sign_at(w: &World, pos: BlockPos) -> &Sign {
        match w.block_entities.get(&pos) {
            Some(BlockEntity::Sign(s)) => s,
            _ => panic!("no sign at {pos:?}"),
        }
    }

    #[test]
    fn placing_writing_dyeing_and_waxing_a_sign() {
        let mut w = world();
        w.chunk(0, 0);
        // Alice stands south of where the sign goes, facing north.
        let alice = player(&mut w, "Alice", [2.5, -60.0, 6.5]);
        let bob = player(&mut w, "Bob", [2.5, -60.0, 7.5]);
        w.players.get_mut(&alice).unwrap().yaw = 180.0;
        give(&mut w, alice, "oak_sign");
        let ground = BlockPos::new(2, -61, 3);
        let pos = BlockPos::new(2, -60, 3);
        w.use_item_on(alice, 0, ground, 1, [0.5, 1.0, 0.5], 1);
        assert_eq!(sign_kind(w.block(pos)), Some(false));
        assert_eq!(sign_at(&w, pos).editor, Some(alice));

        // Only the player with the editor open may write.
        let lines = || {
            [
                "Shop".to_owned(),
                "".into(),
                "\u{a7}cfree".into(),
                "".into(),
            ]
        };
        w.update_sign(bob, pos, true, lines());
        assert_eq!(sign_at(&w, pos).front.lines[0], "");
        w.update_sign(alice, pos, true, lines());
        let sign = sign_at(&w, pos);
        assert_eq!(sign.front.lines[0], "Shop");
        assert_eq!(sign.front.lines[2], "cfree", "no formatting codes");
        assert_eq!(sign.editor, None);

        // Dye colours the side the player faces; glow ink makes it glow.
        give(&mut w, alice, "red_dye");
        w.use_item_on(alice, 0, pos, 3, [0.5, 0.5, 1.0], 2);
        give(&mut w, alice, "glow_ink_sac");
        w.use_item_on(alice, 0, pos, 3, [0.5, 0.5, 1.0], 3);
        let sign = sign_at(&w, pos);
        assert_eq!((sign.front.color, sign.front.glowing), ("red", true));
        assert_eq!(sign.back.color, "black");

        // Right-clicking with an empty hand edits again, until it is waxed.
        give(&mut w, alice, "honeycomb");
        w.use_item_on(alice, 0, pos, 3, [0.5, 0.5, 1.0], 4);
        assert!(sign_at(&w, pos).waxed);
        let p = w.players.get_mut(&alice).unwrap();
        let slot = p.hand_slot(0);
        p.inventory.slots[slot] = Stack::EMPTY;
        w.use_item_on(alice, 0, pos, 3, [0.5, 0.5, 1.0], 5);
        assert_eq!(sign_at(&w, pos).editor, None);
        w.update_sign(alice, pos, true, Default::default());
        assert_eq!(sign_at(&w, pos).front.lines[0], "Shop");

        // Saved and loaded, the text stays.
        let nbt = w.block_entities[&pos].to_nbt(pos);
        let Some((_, BlockEntity::Sign(back))) = BlockEntity::from_nbt(&nbt) else {
            panic!("sign NBT does not load");
        };
        assert_eq!(back.front, sign_at(&w, pos).front);
    }

    #[test]
    fn signs_keep_their_text_through_saving() {
        let mut sign = Sign::new(false);
        sign.front.lines[0] = "Shop".to_owned();
        sign.front.color = "red";
        sign.back.glowing = true;
        sign.waxed = true;
        let mut nbt = Compound::new();
        sign.write_nbt(&mut nbt);
        let back = Sign::read_nbt(false, &nbt);
        assert_eq!(back.front, sign.front);
        assert_eq!(back.back, sign.back);
        assert!(back.waxed);
    }

    #[test]
    fn old_worlds_store_lines_as_json() {
        assert_eq!(plain_text(&Tag::String(r#"{"text":"Hi"}"#.into())), "Hi");
        assert_eq!(plain_text(&Tag::String(r#""Hello""#.into())), "Hello");
        assert_eq!(
            plain_text(&Tag::String(
                r#"{"text":"a","extra":[{"text":"b"}]}"#.into()
            )),
            "ab"
        );
        assert_eq!(plain_text(&Tag::String("plain".into())), "plain");
    }

    #[test]
    fn the_front_is_where_the_text_faces() {
        let sign = BlockInfo::by_name("minecraft:oak_sign")
            .unwrap()
            .default_state;
        // Rotation 0: the text faces south.
        let pos = BlockPos::new(0, 0, 0);
        assert!(front_faces(sign, pos, [0.5, 1.0, 3.0]));
        assert!(!front_faces(sign, pos, [0.5, 1.0, -3.0]));
    }
}
