//! The player inventory, in the slot layout of the player's inventory menu:
//! 0 crafting result, 1-4 crafting grid, 5-8 armor, 9-35 main, 36-44 hotbar, 45 offhand.

use levitas_nbt::{Compound, Tag};
use levitas_protocol::Writer;
use levitas_registry::{component, ItemInfo, SyncedRegistry};

use crate::enchanting::Enchantments;

pub const SLOTS: usize = 46;
pub const CRAFTING_GRID: std::ops::RangeInclusive<usize> = 1..=4;
pub const HOTBAR: usize = 36;
pub const OFFHAND: usize = 45;

/// An item stack. A count of zero or less means the slot is empty.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stack {
    pub item: i32,
    pub count: i32,
    /// Durability used up (the `minecraft:damage` component); 0 when undamaged.
    pub damage: i32,
    /// Enchantments: on the item, or stored in an enchanted book.
    pub enchants: Enchantments,
    /// The anvil's prior work penalty (the `minecraft:repair_cost` component).
    pub repair_cost: i32,
    /// An armor trim (the `minecraft:trim` component).
    pub trim: Option<Trim>,
    /// The potion in a bottle or tipped arrow (the `minecraft:potion_contents`
    /// component), by potion ID.
    pub potion: Option<u8>,
}

/// An armor trim: entries of the trim material and trim pattern registries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trim {
    pub material: u16,
    pub pattern: u16,
}

impl Trim {
    fn registry(name: &str) -> Option<&'static SyncedRegistry> {
        SyncedRegistry::get(name)
    }

    /// A trim from its material and pattern names (`minecraft:gold`, `minecraft:bolt`).
    pub fn named(material: &str, pattern: &str) -> Option<Trim> {
        Some(Trim {
            material: Self::registry("minecraft:trim_material")?.id_of(material)? as u16,
            pattern: Self::registry("minecraft:trim_pattern")?.id_of(pattern)? as u16,
        })
    }

    pub fn material_name(self) -> &'static str {
        Self::registry("minecraft:trim_material")
            .and_then(|r| r.entries.get(usize::from(self.material)))
            .copied()
            .unwrap_or("minecraft:iron")
    }

    pub fn pattern_name(self) -> &'static str {
        Self::registry("minecraft:trim_pattern")
            .and_then(|r| r.entries.get(usize::from(self.pattern)))
            .copied()
            .unwrap_or("minecraft:coast")
    }
}

impl Stack {
    pub const EMPTY: Stack = Stack {
        item: 0,
        count: 0,
        damage: 0,
        enchants: Enchantments::NONE,
        repair_cost: 0,
        trim: None,
        potion: None,
    };

    pub fn new(item: i32, count: i32) -> Self {
        Self {
            item,
            count,
            damage: 0,
            enchants: Enchantments::NONE,
            repair_cost: 0,
            trim: None,
            potion: None,
        }
    }

    /// The same item with the same components, in another amount.
    pub fn with_count(self, count: i32) -> Self {
        if count <= 0 {
            Stack::EMPTY
        } else {
            Self { count, ..self }
        }
    }

    /// Whether two stacks can merge: the same item with the same components.
    pub fn same_kind(&self, other: &Stack) -> bool {
        self.item == other.item
            && self.damage == other.damage
            && self.enchants == other.enchants
            && self.repair_cost == other.repair_cost
            && self.trim == other.trim
            && self.potion == other.potion
    }

    pub fn is_empty(&self) -> bool {
        self.count <= 0 || self.item == 0
    }

    pub fn max_stack(&self) -> i32 {
        ItemInfo::get(self.item).map_or(64, |i| i32::from(i.max_stack))
    }

    /// Whether the enchantments are stored (an enchanted book) rather than active.
    pub fn stores_enchantments(&self) -> bool {
        ItemInfo::get(self.item).is_some_and(|i| i.name == "minecraft:enchanted_book")
    }

    /// The vanilla saved form: `{id, count, components}`. Of the components damage and
    /// enchantments are tracked so far.
    pub fn to_nbt(self) -> Compound {
        let name = ItemInfo::get(self.item).map_or("minecraft:air", |i| i.name);
        let mut nbt = Compound::new().with("id", name).with("count", self.count);
        let mut components = Compound::new();
        if self.damage > 0 {
            components.insert("minecraft:damage", self.damage);
        }
        if !self.enchants.is_empty() {
            let key = if self.stores_enchantments() {
                "minecraft:stored_enchantments"
            } else {
                "minecraft:enchantments"
            };
            components.insert(key, self.enchants.to_nbt());
        }
        if self.repair_cost > 0 {
            components.insert("minecraft:repair_cost", self.repair_cost);
        }
        if let Some(name) = self.potion.and_then(crate::potions::name) {
            components.insert(
                "minecraft:potion_contents",
                Compound::new().with("potion", name),
            );
        }
        if let Some(trim) = self.trim {
            components.insert(
                "minecraft:trim",
                Compound::new()
                    .with("material", trim.material_name())
                    .with("pattern", trim.pattern_name()),
            );
        }
        if !components.is_empty() {
            nbt.insert("components", components);
        }
        nbt
    }

    /// Reads a saved stack; unknown items come back empty.
    pub fn from_nbt(nbt: &Compound) -> Stack {
        let Some(item) = nbt.str("id").and_then(ItemInfo::id_of) else {
            return Stack::EMPTY;
        };
        let count = nbt.int("count").unwrap_or(1) as i32;
        let components = nbt.compound("components");
        let damage = components
            .and_then(|c| c.int("minecraft:damage"))
            .unwrap_or(0) as i32;
        let enchants = components
            .and_then(|c| {
                c.compound("minecraft:enchantments")
                    .or_else(|| c.compound("minecraft:stored_enchantments"))
            })
            .map_or(Enchantments::NONE, Enchantments::from_nbt);
        let repair_cost = components
            .and_then(|c| c.int("minecraft:repair_cost"))
            .unwrap_or(0)
            .clamp(0, i64::from(i32::MAX)) as i32;
        Stack {
            item,
            count,
            damage: damage.max(0),
            enchants,
            repair_cost,
            potion: components
                .and_then(|c| c.get("minecraft:potion_contents"))
                .and_then(|p| match p {
                    // A potion name alone is the short form.
                    Tag::String(name) => crate::potions::id(name),
                    Tag::Compound(c) => crate::potions::id(c.str("potion")?),
                    _ => None,
                }),
            trim: components
                .and_then(|c| c.compound("minecraft:trim"))
                .and_then(|t| Trim::named(t.str("material")?, t.str("pattern")?)),
        }
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

/// Appends a stack in the protocol's slot format: count, item, then the components that
/// differ from the item's defaults.
pub fn write_stack(packet: Writer, stack: Stack) -> Writer {
    if stack.is_empty() {
        return packet.varint(0);
    }
    let packet = packet.varint(stack.count).varint(stack.item);
    // Both counts come first (components to add, then to remove), then the components.
    let damaged = stack.damage > 0;
    let enchanted = !stack.enchants.is_empty();
    let worked = stack.repair_cost > 0;
    let trimmed = stack.trim.is_some();
    let brewed = stack.potion.is_some();
    let added = i32::from(damaged)
        + i32::from(enchanted)
        + i32::from(worked)
        + i32::from(trimmed)
        + i32::from(brewed);
    let mut packet = packet.varint(added).varint(0);
    if damaged {
        packet = packet.varint(component::DAMAGE).varint(stack.damage);
    }
    if enchanted {
        let kind = if stack.stores_enchantments() {
            component::STORED_ENCHANTMENTS
        } else {
            component::ENCHANTMENTS
        };
        packet = stack.enchants.write(packet.varint(kind));
    }
    if worked {
        packet = packet
            .varint(component::REPAIR_COST)
            .varint(stack.repair_cost);
    }
    if let Some(potion) = stack.potion {
        // The potion, then no custom colour, no custom effects and no custom name.
        packet = packet
            .varint(component::POTION_CONTENTS)
            .bool(true)
            .varint(i32::from(potion))
            .bool(false)
            .varint(0)
            .bool(false);
    }
    if let Some(trim) = stack.trim {
        // Registry holders: the entry's ID plus one (zero would be an inline value).
        packet = packet
            .varint(component::TRIM)
            .varint(i32::from(trim.material) + 1)
            .varint(i32::from(trim.pattern) + 1);
    }
    packet
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
    pub fn add(&mut self, stack: Stack) -> (Vec<usize>, i32) {
        let max = stack.max_stack();
        let mut count = stack.count;
        let order: Vec<usize> = (HOTBAR..HOTBAR + 9).chain(9..HOTBAR).collect();
        let mut changed = Vec::new();
        for &slot in &order {
            let s = &mut self.slots[slot];
            if count > 0 && !s.is_empty() && s.same_kind(&stack) && s.count < max {
                let moved = count.min(max - s.count);
                s.count += moved;
                count -= moved;
                changed.push(slot);
            }
        }
        for &slot in &order {
            if count > 0 && self.slots[slot].is_empty() {
                let moved = count.min(max);
                self.slots[slot] = stack.with_count(moved);
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
        let removed = s.with_count(taken);
        *s = s.with_count(s.count - taken);
        removed
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
        assert_eq!(inv.add(Stack::new(dirt, 70)), (vec![HOTBAR, HOTBAR + 1], 0));
        assert_eq!(inv.slots[HOTBAR], Stack::new(dirt, 64));
        assert_eq!(inv.slots[HOTBAR + 1], Stack::new(dirt, 6));
        assert_eq!(inv.add(Stack::new(dirt, 10)), (vec![HOTBAR + 1], 0));
        assert_eq!(inv.slots[HOTBAR + 1].count, 16);
    }

    #[test]
    fn unstackable_items_take_one_slot_each() {
        let mut inv = Inventory::new();
        let pickaxe = item("minecraft:iron_pickaxe");
        let (changed, left) = inv.add(Stack::new(pickaxe, 2));
        assert_eq!(changed.len(), 2);
        assert_eq!(left, 0);
    }

    #[test]
    fn full_inventory_reports_leftover() {
        let mut inv = Inventory::new();
        let stone = item("minecraft:stone");
        let (_, left) = inv.add(Stack::new(stone, 64 * 37));
        assert_eq!(left, 64);
    }

    #[test]
    fn take() {
        let mut inv = Inventory::new();
        let dirt = item("minecraft:dirt");
        inv.add(Stack::new(dirt, 5));
        assert_eq!(inv.take(HOTBAR, 2), Stack::new(dirt, 2));
        assert_eq!(inv.slots[HOTBAR], Stack::new(dirt, 3));
        assert_eq!(inv.take(HOTBAR, 9), Stack::new(dirt, 3));
        assert!(inv.slots[HOTBAR].is_empty());
    }

    #[test]
    fn damaged_tools_keep_their_damage() {
        let mut inv = Inventory::new();
        let pickaxe = Stack {
            damage: 12,
            ..Stack::new(item("minecraft:iron_pickaxe"), 1)
        };
        inv.add(pickaxe);
        assert_eq!(inv.slots[HOTBAR], pickaxe);
        let nbt = pickaxe.to_nbt();
        assert_eq!(Stack::from_nbt(&nbt), pickaxe);
        // Damaged and undamaged copies do not merge.
        let dirt = item("minecraft:dirt");
        assert!(Stack::new(dirt, 1).same_kind(&Stack::new(dirt, 5)));
        assert!(!pickaxe.same_kind(&Stack::new(pickaxe.item, 1)));
    }

    #[test]
    fn slots_list_both_component_counts_before_the_components() {
        let sword = item("minecraft:netherite_sword");
        let mut stack = Stack::new(sword, 1);
        stack.damage = 1;
        let damaged = write_stack(Writer::packet(0), stack).into_frame();
        let expected = Writer::packet(0)
            .varint(1)
            .varint(sword)
            .varint(1) // components to add
            .varint(0) // components to remove
            .varint(component::DAMAGE)
            .varint(1)
            .into_frame();
        assert_eq!(damaged, expected);
        let fresh = write_stack(Writer::packet(0), Stack::new(sword, 1)).into_frame();
        let expected = Writer::packet(0)
            .varint(1)
            .varint(sword)
            .varint(0)
            .varint(0)
            .into_frame();
        assert_eq!(fresh, expected);
    }

    #[test]
    fn enchanted_stacks_send_and_save_their_enchantments() {
        let sharpness = crate::enchanting::enchantment_id("minecraft:sharpness").unwrap();
        let mut stack = Stack::new(item("minecraft:diamond_sword"), 1);
        stack.enchants.set(sharpness, 5);
        let sent = write_stack(Writer::packet(0), stack).into_frame();
        let expected = Writer::packet(0)
            .varint(1)
            .varint(stack.item)
            .varint(1)
            .varint(0)
            .varint(component::ENCHANTMENTS)
            .varint(1)
            .varint(sharpness)
            .varint(5)
            .into_frame();
        assert_eq!(sent, expected);
        assert_eq!(Stack::from_nbt(&stack.to_nbt()), stack);
        // Books store theirs.
        let mut book = Stack::new(item("minecraft:enchanted_book"), 1);
        book.enchants = stack.enchants;
        let nbt = book.to_nbt();
        assert!(nbt
            .compound("components")
            .unwrap()
            .compound("minecraft:stored_enchantments")
            .is_some());
        assert!(!stack.same_kind(&Stack::new(stack.item, 1)));
    }
}
