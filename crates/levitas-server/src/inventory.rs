//! The player inventory, in the slot layout of the player's inventory menu:
//! 0 crafting result, 1-4 crafting grid, 5-8 armor, 9-35 main, 36-44 hotbar, 45 offhand.

use levitas_nbt::{Compound, Tag};
use levitas_protocol::Writer;
use levitas_registry::ItemInfo;

pub const SLOTS: usize = 46;
pub const CRAFTING_GRID: std::ops::RangeInclusive<usize> = 1..=4;
pub const HOTBAR: usize = 36;
pub const OFFHAND: usize = 45;

/// An item stack. A count of zero or less means the slot is empty.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stack {
    pub item: i32,
    pub count: i32,
}

impl Stack {
    pub const EMPTY: Stack = Stack { item: 0, count: 0 };

    pub fn new(item: i32, count: i32) -> Self {
        Self { item, count }
    }

    pub fn is_empty(&self) -> bool {
        self.count <= 0 || self.item == 0
    }

    pub fn max_stack(&self) -> i32 {
        ItemInfo::get(self.item).map_or(64, |i| i32::from(i.max_stack))
    }

    /// The vanilla saved form: `{id, count}`. Item components are not tracked yet.
    pub fn to_nbt(self) -> Compound {
        let name = ItemInfo::get(self.item).map_or("minecraft:air", |i| i.name);
        Compound::new().with("id", name).with("count", self.count)
    }

    /// Reads a saved stack; unknown items come back empty.
    pub fn from_nbt(nbt: &Compound) -> Stack {
        let Some(item) = nbt.str("id").and_then(ItemInfo::id_of) else {
            return Stack::EMPTY;
        };
        let count = nbt.int("count").unwrap_or(1) as i32;
        Stack::new(item, count)
    }
}

/// Stacks as a saved item list: each entry is a stack with a `Slot` byte.
pub fn items_to_nbt(stacks: impl IntoIterator<Item = (usize, Stack)>) -> Vec<Tag> {
    stacks
        .into_iter()
        .filter(|(_, s)| !s.is_empty())
        .map(|(slot, s)| Tag::Compound(s.to_nbt().with("Slot", slot as i8)))
        .collect()
}

/// Reads a saved item list as `(slot, stack)` pairs.
pub fn items_from_nbt(list: &[Tag]) -> Vec<(usize, Stack)> {
    list.iter()
        .filter_map(Tag::as_compound)
        .filter_map(|c| {
            let slot = usize::try_from(c.int("Slot")? as i8).ok()?;
            let stack = Stack::from_nbt(c);
            (!stack.is_empty()).then_some((slot, stack))
        })
        .collect()
}

/// Appends a stack in the protocol's slot format (without components).
pub fn write_stack(packet: Writer, stack: Stack) -> Writer {
    if stack.is_empty() {
        packet.varint(0)
    } else {
        packet
            .varint(stack.count)
            .varint(stack.item)
            .varint(0) // components to add
            .varint(0) // components to remove
    }
}

pub struct Inventory {
    pub slots: [Stack; SLOTS],
    /// The stack on the cursor while the inventory screen is open.
    pub carried: Stack,
    /// Increases with every change the server sends, as the protocol expects.
    pub state_id: i32,
}

impl Inventory {
    pub fn new() -> Self {
        Self {
            slots: [Stack::EMPTY; SLOTS],
            carried: Stack::EMPTY,
            state_id: 0,
        }
    }

    pub fn held(&self, hotbar_slot: usize) -> Stack {
        self.slots[HOTBAR + hotbar_slot]
    }

    /// Adds items the way vanilla does: onto matching stacks first, then into empty
    /// slots, hotbar before main inventory. Returns the slots changed and how many items
    /// did not fit.
    pub fn add(&mut self, item: i32, mut count: i32) -> (Vec<usize>, i32) {
        let max = Stack::new(item, 1).max_stack();
        let order: Vec<usize> = (HOTBAR..HOTBAR + 9).chain(9..HOTBAR).collect();
        let mut changed = Vec::new();
        for &slot in &order {
            let s = &mut self.slots[slot];
            if count > 0 && !s.is_empty() && s.item == item && s.count < max {
                let moved = count.min(max - s.count);
                s.count += moved;
                count -= moved;
                changed.push(slot);
            }
        }
        for &slot in &order {
            if count > 0 && self.slots[slot].is_empty() {
                let moved = count.min(max);
                self.slots[slot] = Stack::new(item, moved);
                count -= moved;
                changed.push(slot);
            }
        }
        (changed, count)
    }

    /// Removes up to `amount` items from a slot and returns them.
    pub fn take(&mut self, slot: usize, amount: i32) -> Stack {
        let s = &mut self.slots[slot];
        if s.is_empty() {
            return Stack::EMPTY;
        }
        let taken = amount.min(s.count);
        s.count -= taken;
        let item = s.item;
        if s.count <= 0 {
            *s = Stack::EMPTY;
        }
        Stack::new(item, taken)
    }

    /// Empties the inventory, returning everything that was in it.
    pub fn drain(&mut self) -> Vec<Stack> {
        let all = self
            .slots
            .iter()
            .chain([&self.carried])
            .copied()
            .filter(|s| !s.is_empty())
            .collect();
        self.slots = [Stack::EMPTY; SLOTS];
        self.carried = Stack::EMPTY;
        all
    }

    fn next_state_id(&mut self) -> i32 {
        self.state_id = self.state_id.wrapping_add(1) & 0x7FFF;
        self.state_id
    }

    /// Updates one slot of the player inventory window.
    pub fn set_slot_packet(&mut self, slot: usize) -> Writer {
        let stack = self.slots[slot];
        self.window_slot_packet(0, slot, stack)
    }

    /// Updates one slot of any window the player has open.
    pub fn window_slot_packet(&mut self, window: i32, slot: usize, stack: Stack) -> Writer {
        use levitas_registry::packet::play::clientbound;
        let state_id = self.next_state_id();
        let packet = Writer::packet(clientbound::CONTAINER_SET_SLOT)
            .varint(window)
            .varint(state_id)
            .i16(slot as i16);
        write_stack(packet, stack)
    }

    /// The whole player inventory window.
    pub fn content_packet(&mut self) -> Writer {
        let slots = self.slots;
        self.window_content_packet(0, &slots)
    }

    /// Every slot of a window, plus the cursor stack.
    pub fn window_content_packet(&mut self, window: i32, slots: &[Stack]) -> Writer {
        use levitas_registry::packet::play::clientbound;
        let state_id = self.next_state_id();
        let mut packet = Writer::packet(clientbound::CONTAINER_SET_CONTENT)
            .varint(window)
            .varint(state_id)
            .varint(slots.len() as i32);
        for &s in slots {
            packet = write_stack(packet, s);
        }
        write_stack(packet, self.carried)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> i32 {
        ItemInfo::id_of(name).unwrap()
    }

    #[test]
    fn add_fills_stacks_then_hotbar_then_main() {
        let mut inv = Inventory::new();
        let dirt = item("minecraft:dirt");
        assert_eq!(inv.add(dirt, 70), (vec![HOTBAR, HOTBAR + 1], 0));
        assert_eq!(inv.slots[HOTBAR], Stack::new(dirt, 64));
        assert_eq!(inv.slots[HOTBAR + 1], Stack::new(dirt, 6));
        assert_eq!(inv.add(dirt, 10), (vec![HOTBAR + 1], 0));
        assert_eq!(inv.slots[HOTBAR + 1].count, 16);
    }

    #[test]
    fn unstackable_items_take_one_slot_each() {
        let mut inv = Inventory::new();
        let pickaxe = item("minecraft:iron_pickaxe");
        let (changed, left) = inv.add(pickaxe, 2);
        assert_eq!(changed.len(), 2);
        assert_eq!(left, 0);
    }

    #[test]
    fn full_inventory_reports_leftover() {
        let mut inv = Inventory::new();
        let stone = item("minecraft:stone");
        let (_, left) = inv.add(stone, 64 * 37);
        assert_eq!(left, 64);
    }

    #[test]
    fn take() {
        let mut inv = Inventory::new();
        let dirt = item("minecraft:dirt");
        inv.add(dirt, 5);
        assert_eq!(inv.take(HOTBAR, 2), Stack::new(dirt, 2));
        assert_eq!(inv.slots[HOTBAR], Stack::new(dirt, 3));
        assert_eq!(inv.take(HOTBAR, 9), Stack::new(dirt, 3));
        assert!(inv.slots[HOTBAR].is_empty());
    }
}
