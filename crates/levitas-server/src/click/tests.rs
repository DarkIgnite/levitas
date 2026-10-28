use std::sync::OnceLock;

use super::*;

fn recipes() -> &'static Recipes {
    static RECIPES: OnceLock<Recipes> = OnceLock::new();
    RECIPES.get_or_init(Recipes::load)
}

fn item(name: &str) -> i32 {
    ItemInfo::id_of(&format!("minecraft:{name}")).unwrap()
}

fn stack(name: &str, count: i32) -> Stack {
    Stack::new(item(name), count)
}

/// A window with its own storage, for clicking around in tests.
struct Test {
    layout: Layout,
    slots: Vec<Stack>,
    carried: Stack,
    offhand: Stack,
    drag: Drag,
    creative: bool,
}

impl Test {
    fn new(layout: Layout) -> Self {
        Test {
            layout,
            slots: vec![Stack::EMPTY; layout.len()],
            carried: Stack::EMPTY,
            offhand: Stack::EMPTY,
            drag: Drag::default(),
            creative: false,
        }
    }

    fn click(&mut self, slot: i16, button: u8, mode: i32) -> Vec<Stack> {
        let mut w = Window {
            layout: self.layout,
            slots: &mut self.slots,
            carried: &mut self.carried,
            offhand: &mut self.offhand,
            drag: &mut self.drag,
            creative: self.creative,
            recipes: recipes(),
        };
        click(&mut w, Click { slot, button, mode })
    }
}

#[test]
fn pick_up_place_split_and_swap() {
    let mut t = Test::new(Layout::Chest(27));
    t.slots[0] = stack("dirt", 10);
    t.slots[1] = stack("stone", 5);
    t.click(0, 1, PICKUP); // right click takes half, rounded up
    assert_eq!(
        (t.carried, t.slots[0]),
        (stack("dirt", 5), stack("dirt", 5))
    );
    t.click(2, 1, PICKUP); // right click places one
    assert_eq!(
        (t.carried, t.slots[2]),
        (stack("dirt", 4), stack("dirt", 1))
    );
    t.click(0, 0, PICKUP); // left click merges all
    assert_eq!((t.carried, t.slots[0]), (Stack::EMPTY, stack("dirt", 9)));
    t.click(1, 0, PICKUP);
    t.click(0, 0, PICKUP); // different items swap
    assert_eq!(
        (t.carried, t.slots[0]),
        (stack("dirt", 9), stack("stone", 5))
    );
    let thrown = t.click(OUTSIDE, 1, PICKUP); // right click outside drops one
    assert_eq!(
        (thrown, t.carried),
        (vec![stack("dirt", 1)], stack("dirt", 8))
    );
}

#[test]
fn shift_click_in_the_inventory() {
    let mut t = Test::new(Layout::Inventory);
    t.slots[9] = stack("dirt", 10);
    t.slots[10] = stack("iron_helmet", 1);
    t.click(9, 0, QUICK_MOVE); // main inventory goes to the hotbar
    assert_eq!((t.slots[9], t.slots[36]), (Stack::EMPTY, stack("dirt", 10)));
    t.click(36, 0, QUICK_MOVE); // and back
    assert_eq!(t.slots[9], stack("dirt", 10));
    t.click(10, 0, QUICK_MOVE); // armor goes on
    assert_eq!(t.slots[5], stack("iron_helmet", 1));
    t.click(5, 0, QUICK_MOVE); // and off again into the inventory
    assert_eq!(t.slots[5], Stack::EMPTY);
}

#[test]
fn shift_click_between_chest_and_inventory() {
    let mut t = Test::new(Layout::Chest(27));
    t.slots[0] = stack("dirt", 64);
    t.click(0, 0, QUICK_MOVE); // into the player's inventory, hotbar end first
    assert_eq!(t.slots[27 + 35], stack("dirt", 64));
    t.slots[27] = stack("dirt", 10);
    t.slots[5] = stack("dirt", 60);
    t.click(27, 0, QUICK_MOVE); // tops up matching stacks first
    assert_eq!(t.slots[5], stack("dirt", 64));
    assert_eq!(t.slots[0], stack("dirt", 6));
}

#[test]
fn crafting_results_use_up_ingredients() {
    let mut t = Test::new(Layout::Inventory);
    t.carried = stack("oak_log", 3);
    t.click(1, 0, PICKUP);
    assert_eq!(t.slots[0], stack("oak_planks", 4));
    t.click(0, 0, PICKUP);
    assert_eq!(
        (t.carried, t.slots[1]),
        (stack("oak_planks", 4), stack("oak_log", 2))
    );
    // Shift-clicking the result crafts until the logs run out.
    t.click(0, 0, QUICK_MOVE);
    assert_eq!(t.slots[1], Stack::EMPTY);
    assert_eq!(t.slots[0], Stack::EMPTY);
    assert_eq!(t.slots[44], stack("oak_planks", 8));
}

#[test]
fn milk_buckets_leave_buckets() {
    let mut t = Test::new(Layout::Crafting);
    for (i, s) in [
        stack("milk_bucket", 1),
        stack("milk_bucket", 1),
        stack("milk_bucket", 1),
        stack("sugar", 1),
        stack("egg", 1),
        stack("sugar", 1),
        stack("wheat", 1),
        stack("wheat", 1),
        stack("wheat", 1),
    ]
    .into_iter()
    .enumerate()
    {
        t.slots[1 + i] = s;
    }
    t.click(4, 0, PICKUP);
    t.click(4, 0, PICKUP); // nudge the grid so the result is computed
    assert_eq!(t.slots[0], stack("cake", 1));
    t.click(0, 0, PICKUP);
    assert_eq!(t.carried, stack("cake", 1));
    assert_eq!(t.slots[1], stack("bucket", 1));
    assert_eq!(t.slots[4], Stack::EMPTY);
}

#[test]
fn shift_click_into_furnace_slots() {
    let mut t = Test::new(Layout::Furnace(crate::recipes::Cooker::Furnace));
    t.slots[3] = stack("raw_iron", 5);
    t.slots[4] = stack("coal", 5);
    t.slots[5] = stack("dirt", 5);
    t.click(3, 0, QUICK_MOVE);
    t.click(4, 0, QUICK_MOVE);
    t.click(5, 0, QUICK_MOVE);
    assert_eq!(t.slots[0], stack("raw_iron", 5));
    assert_eq!(t.slots[1], stack("coal", 5));
    assert_eq!(t.slots[30], stack("dirt", 5), "dirt goes to the hotbar");
    // Nothing can be put into the output.
    t.carried = stack("dirt", 1);
    t.click(2, 0, PICKUP);
    assert_eq!(t.slots[2], Stack::EMPTY);
}

#[test]
fn number_keys_and_offhand() {
    let mut t = Test::new(Layout::Chest(27));
    t.slots[0] = stack("dirt", 3);
    t.click(0, 2, SWAP); // into hotbar slot 3
    assert_eq!(
        (t.slots[0], t.slots[27 + 27 + 2]),
        (Stack::EMPTY, stack("dirt", 3))
    );
    t.click(27 + 27 + 2, OFFHAND_BUTTON, SWAP);
    assert_eq!(t.offhand, stack("dirt", 3));
}

#[test]
fn dragging_spreads_the_cursor() {
    let mut t = Test::new(Layout::Chest(27));
    t.carried = stack("dirt", 10);
    t.click(OUTSIDE, 0, QUICK_CRAFT); // start a left drag
    for s in [0, 1, 2] {
        t.click(s, 1, QUICK_CRAFT);
    }
    t.click(OUTSIDE, 2, QUICK_CRAFT);
    assert_eq!(&t.slots[0..3], &[stack("dirt", 3); 3]);
    assert_eq!(t.carried, stack("dirt", 1));
    // A right drag puts one in each.
    t.carried = stack("stone", 5);
    t.click(OUTSIDE, 4, QUICK_CRAFT);
    for s in [5, 6] {
        t.click(s, 5, QUICK_CRAFT);
    }
    t.click(OUTSIDE, 6, QUICK_CRAFT);
    assert_eq!(
        (t.slots[5], t.slots[6], t.carried),
        (stack("stone", 1), stack("stone", 1), stack("stone", 3))
    );
}

#[test]
fn double_click_collects_partial_stacks_first() {
    let mut t = Test::new(Layout::Chest(27));
    t.slots[1] = stack("dirt", 64);
    t.slots[2] = stack("dirt", 20);
    t.carried = stack("dirt", 30);
    t.click(0, 0, PICKUP_ALL);
    assert_eq!(t.carried, stack("dirt", 64));
    assert_eq!((t.slots[1], t.slots[2]), (stack("dirt", 50), Stack::EMPTY));
}

#[test]
fn throwing_and_cloning() {
    let mut t = Test::new(Layout::Chest(27));
    t.slots[0] = stack("dirt", 10);
    assert_eq!(t.click(0, 0, THROW), vec![stack("dirt", 1)]);
    assert_eq!(t.click(0, 1, THROW), vec![stack("dirt", 9)]);
    t.slots[1] = stack("stone", 1);
    t.click(1, 2, CLONE);
    assert!(t.carried.is_empty(), "survival players cannot clone");
    t.creative = true;
    t.click(1, 2, CLONE);
    assert_eq!(t.carried, stack("stone", 64));
}
