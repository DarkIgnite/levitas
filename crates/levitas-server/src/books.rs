//! Book contents, as book and quills and written books keep them.
//!
//! Stacks are small `Copy` values, so a book's pages live here, interned like custom
//! names: a stack holds the index of its contents (`Stack::book`). Each distinct book
//! is stored once for the life of the server; past a cap, new contents are not kept.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use levitas_nbt::{Compound, Tag};
use levitas_protocol::Writer;

/// Most pages a book holds, characters a page holds, and characters in a title.
pub const MAX_PAGES: usize = 100;
pub const MAX_PAGE: usize = 1024;
pub const MAX_TITLE: usize = 32;
/// How many distinct books are kept at most.
const CAPACITY: usize = 1 << 16;

/// What is in a book. A signed one has a title and an author.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Book {
    pub pages: Vec<String>,
    pub title: Option<String>,
    pub author: String,
    /// 0 original, 1 copy of the original, 2 copy of a copy (no more copies), 3 tattered.
    pub generation: u8,
}

struct Store {
    books: Vec<Book>,
    index: HashMap<Book, u32>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(Store {
            books: Vec::new(),
            index: HashMap::new(),
        })
    })
}

/// The handle of a book with these contents, or `None` if the store is full.
pub fn intern(book: Book) -> Option<u32> {
    let mut s = store().lock().expect("the book store");
    if let Some(&id) = s.index.get(&book) {
        return Some(id);
    }
    if s.books.len() >= CAPACITY {
        return None;
    }
    let id = s.books.len() as u32;
    s.books.push(book.clone());
    s.index.insert(book, id);
    Some(id)
}

/// The contents behind a handle.
pub fn get(id: u32) -> Book {
    store()
        .lock()
        .expect("the book store")
        .books
        .get(id as usize)
        .cloned()
        .unwrap_or_default()
}

/// A page or title as a player may write it: no control characters (but line breaks),
/// and at most `max` characters.
pub fn clean(text: &str, max: usize) -> String {
    text.chars()
        .filter(|&c| c == '\n' || !c.is_control())
        .take(max)
        .collect()
}

/// The `minecraft:writable_book_content` component's saved form.
pub fn writable_nbt(book: &Book) -> Compound {
    let pages: Vec<Tag> = book
        .pages
        .iter()
        .map(|p| Tag::Compound(Compound::new().with("raw", p.as_str())))
        .collect();
    Compound::new().with("pages", pages)
}

/// The `minecraft:written_book_content` component's saved form.
pub fn written_nbt(book: &Book) -> Compound {
    let pages: Vec<Tag> = book
        .pages
        .iter()
        .map(|p| Tag::Compound(Compound::new().with("raw", p.as_str())))
        .collect();
    Compound::new()
        .with(
            "title",
            Compound::new().with("raw", book.title.as_deref().unwrap_or("")),
        )
        .with("author", book.author.as_str())
        .with("generation", i32::from(book.generation))
        .with("pages", pages)
        .with("resolved", true)
}

/// Reads either component back.
pub fn from_nbt(c: &Compound, written: bool) -> Book {
    let text = |t: &Tag| match t {
        Tag::String(s) => s.to_string(),
        Tag::Compound(c) => c
            .get("raw")
            .map(crate::names::plain_text)
            .unwrap_or_default(),
        other => crate::names::plain_text(other),
    };
    let pages = c
        .list("pages")
        .unwrap_or_default()
        .iter()
        .take(MAX_PAGES)
        .map(text)
        .collect();
    if !written {
        return Book {
            pages,
            ..Book::default()
        };
    }
    Book {
        pages,
        title: Some(c.get("title").map(text).unwrap_or_default()),
        author: c.str("author").unwrap_or("").to_owned(),
        generation: c.int("generation").unwrap_or(0).clamp(0, 3) as u8,
    }
}

/// The component on the network: plain strings for a book and quill; for a written
/// book, its title, author, generation and pages as text components.
pub fn write_component(mut packet: Writer, book: &Book, written: bool) -> Writer {
    if !written {
        packet = packet.varint(book.pages.len() as i32);
        for p in &book.pages {
            packet = packet.string(p).bool(false);
        }
        return packet;
    }
    packet = packet
        .string(book.title.as_deref().unwrap_or(""))
        .bool(false)
        .string(&book.author)
        .varint(i32::from(book.generation))
        .varint(book.pages.len() as i32);
    for p in &book.pages {
        packet = packet.text(p).bool(false);
    }
    packet.bool(true)
}
