//! Campfires: up to four raw foods cook on a lit one (30 seconds each) and pop off
//! when done; an unlit one lets them cool back. Shovels put campfires out. Players
//! see the food on top through the block entity's data.

use levitas_nbt::{Compound, Tag};

use super::*;
use crate::blocks::{self, prop, with};
use crate::recipes::Cooker;

pub const CAMPFIRE_SLOTS: usize = 4;

#[derive(Default)]
pub struct Campfire {
    pub soul: bool,
    pub items: [Stack; CAMPFIRE_SLOTS],
    pub cook: [i32; CAMPFIRE_SLOTS],
    pub total: [i32; CAMPFIRE_SLOTS],
    /// Never anyone: campfires have no window. Kept for the shared viewer bookkeeping.
    pub viewers: HashSet<i32>,
}

impl Campfire {
    pub fn to_nbt(&self, nbt: &mut Compound) {
        nbt.insert("CookingTimes", self.cook.to_vec());
        nbt.insert("CookingTotalTimes", self.total.to_vec());
    }

    pub fn read_nbt(&mut self, nbt: &Compound) {
        if let Some(times) = nbt.int_array("CookingTimes") {
            for (c, &t) in self.cook.iter_mut().zip(times) {
                *c = t;
            }
        }
        if let Some(times) = nbt.int_array("CookingTotalTimes") {
            for (c, &t) in self.total.iter_mut().zip(times) {
                *c = t;
            }
        }
    }

    /// What clients need to draw the food on top.
    pub fn network_data(&self) -> Compound {
        Compound::new().with(
            "Items",
            crate::inventory::items_to_nbt(self.items.iter().copied().enumerate()),
        )
    }
}

fn is_campfire(state: u16) -> bool {
    matches!(
        blocks::name(state),
        "minecraft:campfire" | "minecraft:soul_campfire"
    )
}

impl World {
    /// Something held used on a campfire: food goes on a lit one, a shovel puts it
    /// out. Returns whether anything happened.
    pub(super) fn use_on_campfire(&mut self, id: i32, slot: usize, pos: BlockPos) -> bool {
        let state = self.block(pos);
        if !is_campfire(state) {
            return false;
        }
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let held = p.inventory.slots[slot];
        let survival = p.survival();
        let lit = prop(state, "lit") == Some("true");
        let held_name = ItemInfo::get(held.item).map_or("", |i| i.name);

        if lit && held_name.ends_with("_shovel") {
            self.set_block_updating(pos, with(state, "lit", "false"));
            self.play_sound("minecraft:entity.generic.extinguish_fire", pos, None);
            if survival {
                self.damage_item(id, slot, 1, ENTITY_EVENT_MAINHAND_BREAK);
            }
            return true;
        }
        let Some(recipe) = self.recipes.cooking(Cooker::Campfire, held.item).cloned() else {
            return false;
        };
        let Some(BlockEntity::Campfire(c)) = self.block_entities.get_mut(&pos) else {
            return false;
        };
        let Some(free) = c.items.iter().position(Stack::is_empty) else {
            return false;
        };
        c.items[free] = held.with_count(1);
        c.cook[free] = 0;
        c.total[free] = recipe.cook_time;
        if survival {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        self.send_campfire(pos);
        self.dirty.insert(pos.chunk());
        true
    }

    /// Food cooks on lit campfires and cools on unlit ones; cooked food pops off.
    pub(super) fn tick_campfires(&mut self) {
        let positions: Vec<BlockPos> = self
            .block_entities
            .iter()
            .filter(|(_, e)| matches!(e, BlockEntity::Campfire(_)))
            .map(|(&pos, _)| pos)
            .collect();
        for pos in positions {
            let Some(state) = self.loaded_block(pos) else {
                continue;
            };
            let lit = prop(state, "lit") == Some("true");
            let Some(BlockEntity::Campfire(c)) = self.block_entities.get_mut(&pos) else {
                continue;
            };
            let mut done = Vec::new();
            for i in 0..CAMPFIRE_SLOTS {
                if c.items[i].is_empty() {
                    continue;
                }
                if lit {
                    c.cook[i] += 1;
                    if c.cook[i] >= c.total[i] {
                        done.push(c.items[i]);
                        c.items[i] = Stack::EMPTY;
                        c.cook[i] = 0;
                    }
                } else {
                    c.cook[i] = (c.cook[i] - 2).max(0);
                }
            }
            if done.is_empty() {
                continue;
            }
            for raw in done {
                if let Some(cooked) = self.recipes.cooking(Cooker::Campfire, raw.item) {
                    let centre = [pos.x, pos.y, pos.z].map(|v| f64::from(v) + 0.5);
                    self.scatter_item([centre[0], centre[1] + 0.5, centre[2]], cooked.result);
                }
            }
            self.send_campfire(pos);
            self.dirty.insert(pos.chunk());
        }
    }

    /// Shows a campfire's food to everyone.
    fn send_campfire(&mut self, pos: BlockPos) {
        let Some(BlockEntity::Campfire(c)) = self.block_entities.get(&pos) else {
            return;
        };
        let data = levitas_nbt::to_network(&Tag::Compound(c.network_data()));
        self.broadcast(
            Writer::packet(clientbound::BLOCK_ENTITY_DATA)
                .position(pos)
                .varint(levitas_registry::block_entity_type::CAMPFIRE)
                .bytes(&data),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    #[test]
    fn food_cooks_on_a_lit_campfire_and_shovels_put_it_out() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-campfire-{}-{}",
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
        world.chunk(0, 0);
        let (out, _frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        world
            .players
            .insert(id, Player::new(profile, out, [5.5, -60.0, 5.5]));
        let item = |n: &str| ItemInfo::id_of(n).unwrap();

        let pos = BlockPos::new(0, -60, 0);
        let campfire = levitas_registry::BlockInfo::by_name("minecraft:campfire")
            .unwrap()
            .default_state;
        assert_eq!(prop(campfire, "lit"), Some("true"));
        world.set_block(pos, campfire);
        world
            .block_entities
            .insert(pos, BlockEntity::for_block(campfire).unwrap());

        let slot = world.players[&id].hand_slot(0);
        world.players.get_mut(&id).unwrap().inventory.slots[slot] =
            Stack::new(item("minecraft:beef"), 2);
        assert!(world.use_on_campfire(id, slot, pos));
        assert_eq!(world.players[&id].inventory.slots[slot].count, 1);

        for _ in 0..599 {
            world.tick_campfires();
        }
        assert!(world.items.is_empty(), "not done yet");
        world.tick_campfires();
        assert!(world.items.values().any(
            |i| i.to_nbt().compound("Item").unwrap().str("id") == Some("minecraft:cooked_beef")
        ));

        world.players.get_mut(&id).unwrap().inventory.slots[slot] =
            Stack::new(item("minecraft:wooden_shovel"), 1);
        assert!(world.use_on_campfire(id, slot, pos));
        assert_eq!(prop(world.block(pos), "lit"), Some("false"));
    }
}
