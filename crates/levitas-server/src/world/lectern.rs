//! Lecterns and writing books, as the Minecraft Wiki describes them.
//!
//! A book and quill (or a written book) put on a lectern can be read by anyone from
//! the lectern's screen; turning a page gives a short redstone pulse, and a comparator
//! reads how far through the book the page is. A book and quill is written in its own
//! screen (the edit book packet) and signed into a written book; written books open
//! for reading when used.

use levitas_nbt::Compound;

use super::*;
use crate::blocks::{self, prop, with_bool};
use crate::books::{self, Book};

/// Container data index of the page shown.
const DATA_PAGE: i16 = 0;
/// Window buttons of the lectern screen.
const BUTTON_PREVIOUS: i32 = 1;
const BUTTON_NEXT: i32 = 2;
const BUTTON_TAKE: i32 = 3;
const BUTTON_PAGE: i32 = 100;
/// How long a page turn powers the lectern.
const PULSE_TICKS: u64 = 2;

/// A lectern's block entity.
#[derive(Default)]
pub struct Lectern {
    pub items: [Stack; 1],
    pub page: i32,
    pub viewers: HashSet<i32>,
}

fn is_book(stack: Stack) -> bool {
    ItemInfo::get(stack.item)
        .is_some_and(|i| matches!(i.name, "minecraft:writable_book" | "minecraft:written_book"))
}

fn pages(stack: Stack) -> i32 {
    stack.book.map_or(0, |b| books::get(b).pages.len() as i32)
}

impl Lectern {
    pub fn read_nbt(nbt: &Compound) -> Self {
        Lectern {
            items: [nbt.compound("Book").map_or(Stack::EMPTY, Stack::from_nbt)],
            page: nbt.int("Page").unwrap_or(0).max(0) as i32,
            viewers: HashSet::new(),
        }
    }

    pub fn write_nbt(&self, nbt: &mut Compound) {
        if !self.items[0].is_empty() {
            nbt.insert("Book", self.items[0].to_nbt());
            nbt.insert("Page", self.page);
        }
    }

    /// What a comparator reads: 1 on the first page up to 15 on the last.
    pub fn signal(&self) -> u8 {
        if self.items[0].is_empty() {
            return 0;
        }
        let total = pages(self.items[0]);
        let f = if total > 1 {
            self.page as f32 / (total - 1) as f32
        } else {
            1.0
        };
        ((f * 14.0).floor() as u8 + 1).min(15)
    }
}

impl World {
    /// Using a lectern: a book goes on it, or its screen opens. Returns whether the
    /// block was a lectern.
    pub(super) fn use_lectern(&mut self, id: i32, slot: usize, pos: BlockPos, state: u16) -> bool {
        if blocks::name(state) != "minecraft:lectern" {
            return false;
        }
        self.block_entities
            .entry(pos)
            .or_insert_with(|| BlockEntity::Lectern(Box::default()));
        let has_book = matches!(self.block_entities.get(&pos), Some(BlockEntity::Lectern(l)) if !l.items[0].is_empty());
        if has_book {
            self.open_lectern(id, pos);
            return true;
        }
        let Some(p) = self.players.get(&id) else {
            return true;
        };
        let held = p.inventory.slots[slot];
        if !is_book(held) {
            return false;
        }
        if p.survival() {
            let p = self.players.get_mut(&id).expect("checked");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        if let Some(BlockEntity::Lectern(l)) = self.block_entities.get_mut(&pos) {
            l.items[0] = held.with_count(1);
            l.page = 0;
        }
        self.set_block_updating(pos, with_bool(state, "has_book", true));
        self.lectern_changed(pos);
        self.play_sound("minecraft:item.book.put", pos, None);
        true
    }

    /// The lectern's reading screen: its one slot and the page.
    fn open_lectern(&mut self, id: i32, pos: BlockPos) {
        self.open_view(
            id,
            super::containers::View::Lectern(pos),
            levitas_registry::menu::LECTERN,
            "Lectern",
        );
        self.send_lectern_page(id, pos);
    }

    fn send_lectern_page(&mut self, id: i32, pos: BlockPos) {
        let Some(window) = self
            .players
            .get(&id)
            .and_then(|p| p.open.as_ref())
            .map(|o| o.window)
        else {
            return;
        };
        let page = match self.block_entities.get(&pos) {
            Some(BlockEntity::Lectern(l)) => l.page,
            _ => return,
        };
        self.send(
            id,
            Writer::packet(clientbound::CONTAINER_SET_DATA)
                .varint(window)
                .i16(DATA_PAGE)
                .i16(page.min(i32::from(i16::MAX)) as i16),
        );
    }

    /// A lectern screen's buttons: turning pages, jumping to one, taking the book.
    pub(super) fn lectern_button(&mut self, id: i32, pos: BlockPos, button: i32) {
        let Some(BlockEntity::Lectern(l)) = self.block_entities.get_mut(&pos) else {
            return;
        };
        let total = pages(l.items[0]).max(1);
        let old = l.page;
        match button {
            BUTTON_PREVIOUS => l.page = (l.page - 1).max(0),
            BUTTON_NEXT => l.page = (l.page + 1).min(total - 1),
            b if b >= BUTTON_PAGE => l.page = (b - BUTTON_PAGE).min(total - 1),
            BUTTON_TAKE => {
                let book = std::mem::take(&mut l.items[0]);
                l.page = 0;
                if book.is_empty() {
                    return;
                }
                self.close_container(id);
                if let Some(p) = self.players.get_mut(&id) {
                    let (changed, left) = p.inventory.add(book);
                    let updates: Vec<Writer> = changed
                        .into_iter()
                        .map(|s| p.inventory.set_slot_packet(s))
                        .collect();
                    let at = p.pos;
                    for u in updates {
                        self.send(id, u);
                    }
                    if left > 0 {
                        self.scatter_item(at, book);
                    }
                }
                let state = self.block(pos);
                self.set_block_updating(
                    pos,
                    with_bool(with_bool(state, "has_book", false), "powered", false),
                );
                self.lectern_changed(pos);
                // Everyone else reading it has the screen closed.
                let readers: Vec<(i32, i32)> = self
                    .players
                    .iter()
                    .filter_map(|(&pid, p)| {
                        let o = p.open.as_ref()?;
                        (o.view == super::containers::View::Lectern(pos)).then_some((pid, o.window))
                    })
                    .collect();
                for (pid, window) in readers {
                    self.close_container(pid);
                    self.send(
                        pid,
                        Writer::packet(clientbound::CONTAINER_CLOSE).varint(window),
                    );
                }
                return;
            }
            _ => return,
        }
        if l.page == old {
            return;
        }
        // A page turn: a pulse, and everyone reading sees the new page.
        let state = self.block(pos);
        let on = with_bool(state, "powered", true);
        self.set_block_updating(pos, on);
        self.schedule(pos, on, PULSE_TICKS);
        self.lectern_changed(pos);
        let readers: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| {
                p.open
                    .as_ref()
                    .is_some_and(|o| o.view == super::containers::View::Lectern(pos))
            })
            .map(|(&pid, _)| pid)
            .collect();
        for pid in readers {
            self.send_lectern_page(pid, pos);
        }
    }

    /// The pulse of a page turn ends.
    pub(super) fn lectern_tick(&mut self, pos: BlockPos, state: u16) {
        if blocks::name(state) == "minecraft:lectern" && prop(state, "powered") == Some("true") {
            self.set_block_updating(pos, with_bool(state, "powered", false));
        }
    }

    fn lectern_changed(&mut self, pos: BlockPos) {
        self.dirty.insert(pos.chunk());
        self.redstone_dirty.push(pos);
    }

    /// The edit book packet: the pages typed into a book and quill, and a title when it
    /// is signed, which makes it a written book by the player.
    pub(super) fn edit_book(
        &mut self,
        id: i32,
        slot: i32,
        pages: Vec<String>,
        title: Option<String>,
    ) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let slot = match slot {
            0..=8 => HOTBAR + slot as usize,
            40 => OFFHAND,
            _ => return,
        };
        let held = p.inventory.slots[slot];
        if ItemInfo::get(held.item).is_none_or(|i| i.name != "minecraft:writable_book") {
            return;
        }
        let pages: Vec<String> = pages
            .iter()
            .take(books::MAX_PAGES)
            .map(|t| books::clean(t, books::MAX_PAGE))
            .collect();
        let mut stack = held;
        match title {
            Some(title) => {
                let book = Book {
                    pages,
                    title: Some(books::clean(&title, books::MAX_TITLE)),
                    author: p.profile.name.clone(),
                    generation: 0,
                };
                let Some(written) = ItemInfo::id_of("minecraft:written_book") else {
                    return;
                };
                stack.item = written;
                stack.book = books::intern(book);
            }
            None => {
                stack.book = books::intern(Book {
                    pages,
                    ..Book::default()
                });
            }
        }
        p.inventory.slots[slot] = stack;
        let update = p.inventory.set_slot_packet(slot);
        self.send(id, update);
    }

    /// Using a written book opens it for reading. Returns whether it was one.
    pub(super) fn open_book(&mut self, id: i32, hand: i32) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let held = p.inventory.slots[p.hand_slot(hand)];
        if ItemInfo::get(held.item).is_none_or(|i| i.name != "minecraft:written_book") {
            return false;
        }
        self.send(id, Writer::packet(clientbound::OPEN_BOOK).varint(hand));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-lectern-{}-{}",
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
        w.chunk(0, 0);
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let id = next_entity_id();
        let profile = Profile {
            name: "Scribe".to_owned(),
            uuid: 6,
            properties: Vec::new(),
        };
        w.players
            .insert(id, Player::new(profile, out, [2.5, -60.0, 2.5]));
        (w, id)
    }

    #[test]
    fn books_are_written_signed_cloned_and_read_on_lecterns() {
        let (mut w, id) = world();
        let slot = w.players[&id].hand_slot(0);
        let quill = ItemInfo::id_of("minecraft:writable_book").unwrap();
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = Stack::new(quill, 1);
        w.edit_book(
            id,
            0,
            vec!["Once".into(), "upon".into(), "a time".into()],
            None,
        );
        let draft = w.players[&id].inventory.slots[slot];
        assert_eq!(books::get(draft.book.unwrap()).pages.len(), 3);
        w.edit_book(
            id,
            0,
            vec!["Once".into(), "upon".into(), "a time".into()],
            Some("Tale".into()),
        );
        let signed = w.players[&id].inventory.slots[slot];
        assert!(signed.is_written_book());
        let book = books::get(signed.book.unwrap());
        assert_eq!(
            (book.title.as_deref(), book.author.as_str()),
            (Some("Tale"), "Scribe")
        );
        // Saved and read back.
        assert_eq!(Stack::from_nbt(&signed.to_nbt()), signed);

        // Copied onto two books and quills: two copies one generation on.
        let r = crate::recipes::Recipes::load();
        let mut grid = vec![Stack::EMPTY; 9];
        grid[0] = signed;
        grid[1] = Stack::new(quill, 1);
        grid[2] = Stack::new(quill, 1);
        let copies = r.craft(&grid, 3);
        assert_eq!(copies.count, 2);
        assert_eq!(books::get(copies.book.unwrap()).generation, 1);

        // On a lectern: turning pages pulses and the comparator follows.
        let pos = BlockPos::new(4, -60, 4);
        let lectern = BlockInfo::by_name("minecraft:lectern")
            .unwrap()
            .default_state;
        w.set_block(pos, lectern);
        assert!(w.use_lectern(id, slot, pos, lectern));
        assert_eq!(prop(w.block(pos), "has_book"), Some("true"));
        assert_eq!(w.container_signal(pos), Some(1));
        let state = w.block(pos);
        assert!(w.use_lectern(id, slot, pos, state), "opens the screen");
        assert!(matches!(
            w.players[&id].open.as_ref().map(|o| o.view),
            Some(super::super::containers::View::Lectern(_))
        ));
        w.lectern_button(id, pos, BUTTON_NEXT);
        w.lectern_button(id, pos, BUTTON_NEXT);
        assert_eq!(w.container_signal(pos), Some(15), "the last page");
        assert_eq!(prop(w.block(pos), "powered"), Some("true"));
        w.lectern_button(id, pos, BUTTON_TAKE);
        assert_eq!(prop(w.block(pos), "has_book"), Some("false"));
        assert!(w.players[&id]
            .inventory
            .slots
            .iter()
            .any(|s| s.is_written_book()));
    }
}
