//! The enchanting table window: offers from bookshelves and the player's enchantment
//! seed, and enchanting for lapis and levels.

use super::containers::View;
use super::*;
use crate::enchanting::{self, Enchantments, MAX_BOOKSHELVES};
use crate::rng::Rng;

/// One of the table's three slots: the level it costs and the enchantment it hints at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Offer {
    pub cost: u32,
    pub hint: Option<(i32, u32)>,
}

impl World {
    /// How many bookshelves power a table: those two blocks away on its own level or one
    /// up, with nothing between them and the table.
    fn bookshelves(&self, table: BlockPos) -> u32 {
        let mut count = 0;
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                if dx.abs() != 2 && dz.abs() != 2 {
                    continue;
                }
                for dy in 0..=1 {
                    let shelf = BlockPos::new(table.x + dx, table.y + dy, table.z + dz);
                    let between = BlockPos::new(table.x + dx / 2, table.y + dy, table.z + dz / 2);
                    let powers = self
                        .loaded_block(shelf)
                        .is_some_and(|s| block_has_tag(s, "minecraft:enchantment_power_provider"));
                    let open = self.loaded_block(between).is_some_and(|s| {
                        block_has_tag(s, "minecraft:enchantment_power_transmitter")
                    });
                    if powers && open {
                        count += 1;
                    }
                }
            }
        }
        count.min(MAX_BOOKSHELVES)
    }

    /// Works out what the table offers for the item in it and tells the client.
    pub(super) fn refresh_offers(&mut self, id: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let Some(open) = &p.open else {
            return;
        };
        let View::Enchanting(table) = open.view else {
            return;
        };
        let (window, item, seed) = (open.window, open.grid[0], p.enchant_seed);
        let enchantable = !item.is_empty()
            && item.enchants.is_empty()
            && ItemInfo::get(item.item).is_some_and(|i| i.enchantability > 0);
        let mut offers = [Offer::default(); 3];
        if enchantable {
            let shelves = self.bookshelves(table);
            let costs =
                enchanting::slot_costs(&mut Rng::new(seed as u32 as u64), shelves, item.item);
            for (slot, offer) in offers.iter_mut().enumerate() {
                let picked = offer_enchantments(seed, slot, item.item, costs[slot]);
                *offer = Offer {
                    cost: costs[slot],
                    hint: picked.first().copied(),
                };
            }
        }
        let p = self.players.get_mut(&id).expect("player exists");
        p.open.as_mut().expect("window open").offers = offers;

        let property = |index: usize, value: i32| {
            Writer::packet(clientbound::CONTAINER_SET_DATA)
                .varint(window)
                .i16(index as i16)
                .i16(value as i16)
        };
        let mut packets = Vec::new();
        for (slot, offer) in offers.iter().enumerate() {
            packets.push(property(slot, offer.cost as i32));
            packets.push(property(4 + slot, offer.hint.map_or(-1, |(e, _)| e)));
            packets.push(property(7 + slot, offer.hint.map_or(-1, |(_, l)| l as i32)));
        }
        // The seed only picks the glyphs shown on the slots.
        packets.push(property(3, seed & 0x7FFF));
        for packet in packets {
            self.send(id, packet);
        }
    }

    /// The player clicked one of the table's three slots.
    pub(super) fn enchant_button(&mut self, id: i32, window: i32, button: i32) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let Some(open) = p.open.as_ref().filter(|o| o.window == window) else {
            return;
        };
        let (View::Enchanting(table), Ok(slot)) = (open.view, usize::try_from(button)) else {
            return;
        };
        let Some(offer) = open.offers.get(slot).copied().filter(|o| o.cost > 0) else {
            return;
        };
        let (item, lapis) = (open.grid[0], open.grid[1]);
        let price = slot as i32 + 1;
        let survival = p.survival();
        if survival && (lapis.count < price || p.xp_level < offer.cost as i32) {
            return;
        }
        let picked = offer_enchantments(p.enchant_seed, slot, item.item, offer.cost);
        if picked.is_empty() {
            return;
        }
        let mut enchants = Enchantments::NONE;
        for (e, level) in picked {
            enchants.set(e, level);
        }
        let mut result = Stack { enchants, ..item };
        if ItemInfo::get(item.item).is_some_and(|i| i.name == "minecraft:book") {
            if let Some(book) = ItemInfo::id_of("minecraft:enchanted_book") {
                result.item = book;
            }
        }
        let new_seed = self.rng.next_u64() as i32;
        let p = self.players.get_mut(&id).expect("player exists");
        p.enchant_seed = new_seed;
        let open = p.open.as_mut().expect("window open");
        open.grid[0] = result;
        if survival {
            open.grid[1] = lapis.with_count(lapis.count - price);
        }
        if survival {
            self.give_levels(id, -price);
        }
        let view = View::Enchanting(table);
        self.resync_window(id, window, view);
        self.play_sound("minecraft:block.enchantment_table.use", table, None);
        self.refresh_offers(id);
    }
}

/// The enchantments behind one of the table's slots. Seeded by the player's seed and the
/// slot, so the hint shown matches what enchanting gives.
fn offer_enchantments(seed: i32, slot: usize, item: i32, cost: u32) -> Vec<(i32, u32)> {
    if cost == 0 {
        return Vec::new();
    }
    let mut rng = Rng::new((seed as u32 as u64) << 2 | slot as u64);
    enchanting::select(&mut rng, item, cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn a_full_table_enchants_for_lapis_and_levels() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-enchanting-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut world, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        for cx in -1..=0 {
            for cz in -1..=0 {
                world.chunk(cx, cz);
            }
        }
        let (out, _frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        world
            .players
            .insert(id, Player::new(profile, out, [0.5, -60.0, 0.5]));
        world.players.get_mut(&id).unwrap().xp_level = 30;

        let table = BlockPos::new(0, -60, 0);
        let state = |n: &str| {
            levitas_registry::BlockInfo::by_name(n)
                .unwrap()
                .default_state
        };
        world.set_block(table, state("minecraft:enchanting_table"));
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                if dx.abs() == 2 || dz.abs() == 2 {
                    world.set_block(BlockPos::new(dx, -60, dz), state("minecraft:bookshelf"));
                }
            }
        }
        assert_eq!(world.bookshelves(table), 15);

        let table_state = world.block(table);
        assert!(world.open_container(id, table, table_state));
        let item = |n: &str| ItemInfo::id_of(n).unwrap();
        let open = world.players.get_mut(&id).unwrap().open.as_mut().unwrap();
        open.grid[0] = Stack::new(item("minecraft:diamond_sword"), 1);
        open.grid[1] = Stack::new(item("minecraft:lapis_lazuli"), 3);
        let window = open.window;
        world.refresh_offers(id);
        let offers = world.players[&id].open.as_ref().unwrap().offers;
        assert_eq!(offers[2].cost, 30);
        assert!(offers[2].hint.is_some());

        world.enchant_button(id, window, 2);
        let p = &world.players[&id];
        let open = p.open.as_ref().unwrap();
        let (e, level) = offers[2].hint.unwrap();
        assert_eq!(open.grid[0].enchants.level(e), level, "the hint was honest");
        assert!(open.grid[1].is_empty(), "three lapis used");
        assert_eq!(p.xp_level, 27);
    }
}
