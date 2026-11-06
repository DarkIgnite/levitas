//! The crafter, as the Minecraft Wiki describes it: a 3x3 grid that crafts when a
//! redstone pulse reaches it, throwing the result out of its front (or into the
//! container there). Slots can be switched off by clicking them while empty; a
//! comparator counts the slots that are filled or off; hoppers fill the emptiest
//! slots first.

use levitas_nbt::Compound;

use super::*;
use crate::blocks::{offset, prop, with_bool, Dir};

/// Slots in the grid.
pub const GRID: usize = 9;
/// Ticks from the pulse to the craft.
const CRAFT_DELAY: u64 = 4;
const LEVEL_EVENT_CRAFTED: i32 = 1049;
const LEVEL_EVENT_FAILED: i32 = 1050;
/// Container data index of "triggered" (0 to 8 are the slots' on/off states).
const DATA_TRIGGERED: i16 = 9;

/// A crafter's block entity.
#[derive(Default)]
pub struct Crafter {
    pub items: [Stack; GRID],
    /// Slots switched off, one bit each.
    pub disabled: u16,
    pub viewers: HashSet<i32>,
}

impl Crafter {
    pub fn is_disabled(&self, slot: usize) -> bool {
        self.disabled & (1 << slot) != 0
    }

    pub fn read_nbt(nbt: &Compound) -> Self {
        let mut c = Crafter::default();
        for &slot in nbt.int_array("disabled_slots").unwrap_or_default() {
            if (0..GRID as i32).contains(&slot) {
                c.disabled |= 1 << slot;
            }
        }
        c
    }

    pub fn write_nbt(&self, nbt: &mut Compound) {
        let off: Vec<i32> = (0..GRID as i32)
            .filter(|&s| self.is_disabled(s as usize))
            .collect();
        nbt.insert("disabled_slots", off);
        nbt.insert("triggered", 0);
        nbt.insert("crafting_ticks_remaining", 0);
    }

    /// What a comparator reads: slots that hold something or are off.
    pub fn signal(&self) -> u8 {
        (0..GRID)
            .filter(|&s| self.is_disabled(s) || !self.items[s].is_empty())
            .count() as u8
    }

    /// Where a hopper puts an item: the enabled slot that can take it with the fewest
    /// items in it (empty ones count as none).
    pub fn insert_slot(&self, stack: &Stack) -> Option<usize> {
        (0..GRID)
            .filter(|&s| !self.is_disabled(s))
            .filter(|&s| {
                let have = self.items[s];
                have.is_empty() || (have.same_kind(stack) && have.count < have.max_stack())
            })
            .min_by_key(|&s| self.items[s].count)
    }
}

/// The way a crafter's front faces, from its `orientation` (`north_up`, `down_east`).
pub fn front(state: u16) -> Dir {
    let first = prop(state, "orientation")
        .and_then(|o| o.split('_').next())
        .unwrap_or("north");
    Dir::ALL
        .into_iter()
        .find(|d| d.name() == first)
        .unwrap_or(Dir::North)
}

fn data(window: i32, index: i16, value: i16) -> Writer {
    Writer::packet(clientbound::CONTAINER_SET_DATA)
        .varint(window)
        .i16(index)
        .i16(value)
}

impl World {
    /// Shows a player who opened a crafter which of its slots are off.
    pub(super) fn send_crafter_state(&mut self, id: i32, pos: BlockPos) {
        let Some(window) = self
            .players
            .get(&id)
            .and_then(|p| p.open.as_ref())
            .map(|o| o.window)
        else {
            return;
        };
        let Some(BlockEntity::Crafter(c)) = self.block_entities.get(&pos) else {
            return;
        };
        let mut packets: Vec<Writer> = (0..GRID)
            .map(|s| data(window, s as i16, i16::from(c.is_disabled(s))))
            .collect();
        let triggered = prop(self.block(pos), "triggered") == Some("true");
        packets.push(data(window, DATA_TRIGGERED, i16::from(triggered)));
        for p in packets {
            self.send(id, p);
        }
    }

    /// A player switches a crafter slot off or on (only an empty one).
    pub(super) fn crafter_slot_state(&mut self, id: i32, window: i32, slot: i32, enabled: bool) {
        let Some(open) = self.players.get(&id).and_then(|p| p.open.as_ref()) else {
            return;
        };
        let super::containers::View::Chest(pos, GRID) = open.view else {
            return;
        };
        if open.window != window || !(0..GRID as i32).contains(&slot) {
            return;
        }
        let slot = slot as usize;
        let Some(BlockEntity::Crafter(c)) = self.block_entities.get_mut(&pos) else {
            return;
        };
        if !c.items[slot].is_empty() {
            return;
        }
        if enabled {
            c.disabled &= !(1 << slot);
        } else {
            c.disabled |= 1 << slot;
        }
        let viewers: Vec<i32> = c.viewers.iter().copied().collect();
        self.dirty.insert(pos.chunk());
        self.redstone_dirty.push(pos);
        for v in viewers {
            self.send_crafter_state(v, pos);
        }
    }

    /// A pulse reaching a crafter: it crafts a moment later.
    pub(super) fn react_crafter(&mut self, pos: BlockPos, state: u16) {
        let powered = self.power_at(pos) > 0;
        let triggered = prop(state, "triggered") == Some("true");
        if powered && !triggered {
            let on = with_bool(state, "triggered", true);
            self.set_block(pos, on);
            self.schedule(pos, on, CRAFT_DELAY);
        } else if !powered && triggered {
            let off = with_bool(with_bool(state, "triggered", false), "crafting", false);
            self.set_block(pos, off);
        }
    }

    /// The crafter crafts what its grid makes: one of each ingredient is used (leaving
    /// buckets and bottles behind), and the result comes out of the front.
    pub(super) fn crafter_craft(&mut self, pos: BlockPos, state: u16) {
        let Some(BlockEntity::Crafter(c)) = self.block_entities.get(&pos) else {
            return;
        };
        let grid = c.items;
        let result = self.recipes.craft(&grid, 3);
        if result.is_empty() {
            self.level_event(LEVEL_EVENT_FAILED, pos, 0);
            return;
        }
        let Some(BlockEntity::Crafter(c)) = self.block_entities.get_mut(&pos) else {
            return;
        };
        let mut leftovers = Vec::new();
        for s in &mut c.items {
            if s.is_empty() {
                continue;
            }
            let item = s.item;
            *s = s.with_count(s.count - 1);
            if let Some(rest) = crate::click::crafting_remainder(item) {
                if s.is_empty() {
                    *s = Stack::new(rest, 1);
                } else {
                    leftovers.push(Stack::new(rest, 1));
                }
            }
        }
        self.dirty.insert(pos.chunk());
        self.redstone_dirty.push(pos);
        self.notify_viewers(pos, &(0..GRID).collect::<Vec<_>>(), None);
        let facing = front(state);
        let out = offset(pos, facing);
        for stack in std::iter::once(result).chain(leftovers) {
            let rest = if self.block_entities.contains_key(&out) {
                self.insert_stack(out, facing.opposite(), stack)
            } else {
                stack
            };
            if !rest.is_empty() {
                let step = facing.step().map(f64::from);
                let at =
                    [0, 1, 2].map(|i| f64::from([pos.x, pos.y, pos.z][i]) + 0.5 + 0.7 * step[i]);
                self.spawn_dropped(rest, at, step.map(|v| v * 0.1));
            }
        }
        self.set_block(pos, with_bool(state, "crafting", true));
        self.level_event(LEVEL_EVENT_CRAFTED, pos, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;

    #[test]
    fn crafters_craft_on_a_pulse_and_respect_off_slots() {
        let dir = std::env::temp_dir().join(format!("levitas-crafter-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let pos = BlockPos::new(3, -60, 3);
        let state = blocks::with(
            BlockInfo::by_name("minecraft:crafter")
                .unwrap()
                .default_state,
            "orientation",
            "east_up",
        );
        w.set_block(pos, state);
        let mut c = Crafter::default();
        let planks = ItemInfo::id_of("minecraft:oak_planks").unwrap();
        // Two planks stacked make sticks.
        c.items[0] = Stack::new(planks, 2);
        c.items[3] = Stack::new(planks, 2);
        c.disabled = 1 << 8;
        assert_eq!(c.signal(), 3);
        assert_eq!(
            c.insert_slot(&Stack::new(planks, 1)),
            Some(1),
            "an empty slot first"
        );
        w.block_entities
            .insert(pos, BlockEntity::Crafter(Box::new(c)));
        w.crafter_craft(pos, state);
        let sticks = ItemInfo::id_of("minecraft:stick").unwrap();
        assert!(
            w.items.values().any(|i| i.stack_ref().item == sticks),
            "sticks came out"
        );
        let Some(BlockEntity::Crafter(c)) = w.block_entities.get(&pos) else {
            panic!("crafter");
        };
        assert_eq!((c.items[0].count, c.items[3].count), (1, 1));
        assert_eq!(front(state), Dir::East);
        // Saved with its switched-off slot.
        let saved = w.block_entities[&pos].to_nbt(pos);
        let (_, back) = BlockEntity::from_nbt(&saved).unwrap();
        let BlockEntity::Crafter(back) = back else {
            panic!("crafter");
        };
        assert!(back.is_disabled(8));
    }
}
