//! Container clicks, simulated on the server the way vanilla menus handle them: picking
//! up and placing, shift-clicking, number-key swaps, throwing, dragging, double-click
//! collecting and creative cloning, including taking crafting results.
//!
//! The client predicts the same thing and reports the slots it changed; the caller
//! compares that report with the result here and resends the window if they differ.

use std::ops::Range;

use levitas_registry::ItemInfo;

use crate::inventory::Stack;
use crate::potions;
use crate::recipes::{fuel_ticks, Cooker, Recipes};
use crate::repair;

/// Click modes as numbered by the protocol.
const PICKUP: i32 = 0;
const QUICK_MOVE: i32 = 1;
const SWAP: i32 = 2;
const CLONE: i32 = 3;
const THROW: i32 = 4;
const QUICK_CRAFT: i32 = 5;
const PICKUP_ALL: i32 = 6;
/// The slot number for clicks outside the window.
const OUTSIDE: i16 = -999;
/// The swap button for the offhand (the F key).
const OFFHAND_BUTTON: u8 = 40;
/// A guard for the crafting loops of shift- and ctrl-Q clicks.
const MAX_CRAFTS: usize = 64;

/// The shape of a window: which slots hold what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// The player inventory: result, 2x2 grid, armor, main, hotbar, offhand.
    Inventory,
    /// A crafting table: result, 3x3 grid, then the player's main inventory and hotbar.
    Crafting,
    /// A furnace: input, fuel, output, then main inventory and hotbar.
    Furnace(Cooker),
    /// A chest-like container with this many slots, then main inventory and hotbar.
    Chest(usize),
    /// An enchanting table: the item, the lapis, then main inventory and hotbar.
    Enchanting,
    /// An anvil: left and right inputs, the result, then main inventory and hotbar.
    Anvil,
    /// A grindstone: two inputs, the result, then main inventory and hotbar.
    Grindstone,
    /// A stonecutter: the input, the result, then main inventory and hotbar.
    Stonecutter,
    /// A smithing table: template, base, addition, result, then main inventory and
    /// hotbar.
    Smithing,
    /// A brewing stand: three bottles, the ingredient, the fuel, then main inventory
    /// and hotbar.
    Brewing,
}

/// What taking a result costs or gives, settled by the world after the click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Used {
    /// Levels paid at an anvil.
    Levels(i32),
    /// Experience a grindstone gives back, before its random part.
    Experience(i32),
}

impl Layout {
    fn container_len(self) -> usize {
        match self {
            Layout::Inventory => 9,
            Layout::Crafting => 10,
            Layout::Furnace(_) => 3,
            Layout::Chest(n) => n,
            Layout::Enchanting => 2,
            Layout::Anvil | Layout::Grindstone => 3,
            Layout::Stonecutter => 2,
            Layout::Smithing => 4,
            Layout::Brewing => 5,
        }
    }

    #[cfg(test)]
    pub fn len(self) -> usize {
        match self {
            Layout::Inventory => 46,
            _ => self.container_len() + 36,
        }
    }

    fn main(self) -> Range<usize> {
        let start = self.container_len();
        start..start + 27
    }

    fn hotbar(self) -> Range<usize> {
        let start = self.container_len() + 27;
        start..start + 9
    }

    pub fn result_slot(self) -> Option<usize> {
        match self {
            Layout::Inventory | Layout::Crafting => Some(0),
            Layout::Anvil | Layout::Grindstone => Some(2),
            Layout::Stonecutter => Some(1),
            Layout::Smithing => Some(3),
            _ => None,
        }
    }

    /// The crafting grid's slots and side length.
    fn grid(self) -> Option<(Range<usize>, usize)> {
        match self {
            Layout::Inventory => Some((1..5, 2)),
            Layout::Crafting => Some((1..10, 3)),
            _ => None,
        }
    }

    /// The armor slot an item is worn in, for the player inventory.
    fn armor_slot(self, stack: &Stack) -> Option<usize> {
        if self != Layout::Inventory {
            return None;
        }
        match ItemInfo::get(stack.item)?.equip_slot? {
            "head" => Some(5),
            "chest" => Some(6),
            "legs" => Some(7),
            "feet" => Some(8),
            _ => None,
        }
    }

    fn may_place(self, slot: usize, stack: &Stack) -> bool {
        if Some(slot) == self.result_slot() {
            return false;
        }
        match self {
            Layout::Inventory if (5..9).contains(&slot) => self.armor_slot(stack) == Some(slot),
            Layout::Furnace(_) if slot == 2 => false,
            Layout::Furnace(_) if slot == 1 => fuel_ticks(stack.item) > 0 || is_bucket(stack),
            Layout::Enchanting if slot == 1 => is_lapis(stack),
            Layout::Brewing => match slot {
                0..=2 => potions::is_potion(*stack) || is_glass_bottle(stack),
                3 => potions::is_ingredient(*stack),
                4 => is_blaze_powder(stack),
                _ => true,
            },
            _ => true,
        }
    }

    /// How many of `stack` fit in `slot`.
    fn limit(self, slot: usize, stack: &Stack) -> i32 {
        let armor = self == Layout::Inventory && (5..9).contains(&slot);
        let bucket_fuel = matches!(self, Layout::Furnace(_)) && slot == 1 && is_bucket(stack);
        let enchanted = self == Layout::Enchanting && slot == 0;
        let bottle = self == Layout::Brewing && slot < 3;
        if armor || bucket_fuel || enchanted || bottle {
            1
        } else {
            stack.max_stack()
        }
    }
}

fn is_lapis(stack: &Stack) -> bool {
    ItemInfo::get(stack.item).is_some_and(|i| i.name == "minecraft:lapis_lazuli")
}

fn is_bucket(stack: &Stack) -> bool {
    ItemInfo::get(stack.item).is_some_and(|i| i.name == "minecraft:bucket")
}

fn is_glass_bottle(stack: &Stack) -> bool {
    ItemInfo::get(stack.item).is_some_and(|i| i.name == "minecraft:glass_bottle")
}

fn is_blaze_powder(stack: &Stack) -> bool {
    ItemInfo::get(stack.item).is_some_and(|i| i.name == "minecraft:blaze_powder")
}

/// What a crafting ingredient leaves behind in the grid (a bucket from milk, ...).
fn crafting_remainder(item: i32) -> Option<i32> {
    let remainder = match ItemInfo::get(item)?.name {
        "minecraft:milk_bucket"
        | "minecraft:water_bucket"
        | "minecraft:lava_bucket"
        | "minecraft:powder_snow_bucket" => "minecraft:bucket",
        "minecraft:honey_bottle" | "minecraft:dragon_breath" => "minecraft:glass_bottle",
        _ => return None,
    };
    ItemInfo::id_of(remainder)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Click {
    pub slot: i16,
    pub button: u8,
    pub mode: i32,
}

/// A drag (quick craft) in progress: its kind (0 left, 1 right, 2 middle) and the slots
/// it has passed over.
#[derive(Debug, Default)]
pub struct Drag {
    kind: Option<u8>,
    slots: Vec<usize>,
}

/// The window being clicked in.
pub struct Window<'a> {
    pub layout: Layout,
    pub slots: &'a mut [Stack],
    pub carried: &'a mut Stack,
    /// The player's offhand, for windows that do not show it (number-key swaps with F).
    pub offhand: &'a mut Stack,
    pub drag: &'a mut Drag,
    pub creative: bool,
    pub recipes: &'a Recipes,
    /// The player's experience level, for paying at an anvil.
    pub xp_level: i32,
    /// Set when taking a result cost levels or gave experience.
    pub used: Option<Used>,
    /// The stonecutter recipe picked, as an index into the options for its input.
    pub choice: Option<usize>,
}

/// Applies one click. Returns the stacks the player threw out.
pub fn click(w: &mut Window, c: Click) -> Vec<Stack> {
    let mut thrown = Vec::new();
    let slot = usize::try_from(c.slot).ok().filter(|&s| s < w.slots.len());
    if c.mode != QUICK_CRAFT && w.drag.kind.is_some() {
        *w.drag = Drag::default();
    }
    match (c.mode, slot) {
        (PICKUP, None) if c.slot == OUTSIDE => {
            if !w.carried.is_empty() {
                let amount = if c.button == 0 { w.carried.count } else { 1 };
                thrown.push(w.carried.with_count(amount));
                *w.carried = w.carried.with_count(w.carried.count - amount);
            }
        }
        (PICKUP, Some(s)) => pickup(w, s, c.button == 0),
        (QUICK_MOVE, Some(s)) => quick_move(w, s),
        (SWAP, Some(s)) => swap(w, s, c.button),
        (CLONE, Some(s)) => {
            let stack = w.slots[s];
            if w.creative && w.carried.is_empty() && !stack.is_empty() {
                *w.carried = stack.with_count(stack.max_stack());
            }
        }
        (THROW, Some(s)) if w.carried.is_empty() => {
            if Some(s) == w.layout.result_slot() {
                let times = if c.button == 1 { MAX_CRAFTS } else { 1 };
                for _ in 0..times {
                    let taken = take_result(w);
                    if taken.is_empty() {
                        break;
                    }
                    thrown.push(taken);
                }
            } else {
                let stack = w.slots[s];
                let amount = if c.button == 0 { 1 } else { stack.count };
                if !stack.is_empty() {
                    thrown.push(stack.with_count(amount));
                    w.slots[s] = stack.with_count(stack.count - amount);
                }
            }
        }
        (QUICK_CRAFT, _) => drag(w, slot, c.button),
        (PICKUP_ALL, Some(s)) if c.button == 0 => collect(w, s),
        _ => {}
    }
    refresh_result(w);
    thrown
}

/// Recomputes the result from the inputs: a crafting grid's, or an anvil's or
/// grindstone's. An anvil shows nothing the player cannot pay for.
fn refresh_result(w: &mut Window) {
    match w.layout {
        Layout::Anvil => {
            let affordable = |r: &repair::AnvilResult| {
                w.creative || (r.cost < repair::TOO_EXPENSIVE && r.cost <= w.xp_level)
            };
            w.slots[2] = repair::anvil(w.slots[0], w.slots[1])
                .filter(affordable)
                .map_or(Stack::EMPTY, |r| r.result);
        }
        Layout::Grindstone => {
            w.slots[2] = repair::grindstone(w.slots[0], w.slots[1]).map_or(Stack::EMPTY, |r| r.0);
        }
        Layout::Smithing => {
            w.slots[3] = w.recipes.smithing(w.slots[0], w.slots[1], w.slots[2]);
        }
        Layout::Stonecutter => {
            let options = w.recipes.stonecutting(w.slots[0].item);
            w.slots[1] = match w.choice {
                Some(i) if !w.slots[0].is_empty() => options.get(i).copied().unwrap_or_default(),
                _ => Stack::EMPTY,
            };
        }
        _ => {
            if let Some((grid, size)) = w.layout.grid() {
                w.slots[0] = w.recipes.craft(&w.slots[grid], size);
            }
        }
    }
}

/// The level cost of the anvil's current result, for showing on the screen.
pub fn anvil_cost(slots: &[Stack]) -> i32 {
    repair::anvil(slots[0], slots[1]).map_or(0, |r| r.cost)
}

/// Takes the result: one craft, using up one of each ingredient, or an anvil's or
/// grindstone's work, using up their inputs.
fn take_result(w: &mut Window) -> Stack {
    match w.layout {
        Layout::Anvil => {
            let Some(r) = repair::anvil(w.slots[0], w.slots[1]) else {
                return Stack::EMPTY;
            };
            if w.slots[2].is_empty() {
                return Stack::EMPTY;
            }
            w.slots[0] = w.slots[0].with_count(w.slots[0].count - 1);
            w.slots[1] = w.slots[1].with_count(w.slots[1].count - r.uses);
            w.used = Some(Used::Levels(r.cost));
            refresh_result(w);
            return r.result;
        }
        Layout::Smithing => {
            let result = w.slots[3];
            if result.is_empty() {
                return Stack::EMPTY;
            }
            for s in 0..3 {
                w.slots[s] = w.slots[s].with_count(w.slots[s].count - 1);
            }
            refresh_result(w);
            return result;
        }
        Layout::Stonecutter => {
            let result = w.slots[1];
            if result.is_empty() {
                return Stack::EMPTY;
            }
            w.slots[0] = w.slots[0].with_count(w.slots[0].count - 1);
            if w.slots[0].is_empty() {
                w.choice = None;
            }
            refresh_result(w);
            return result;
        }
        Layout::Grindstone => {
            let Some((result, xp)) = repair::grindstone(w.slots[0], w.slots[1]) else {
                return Stack::EMPTY;
            };
            for s in 0..2 {
                w.slots[s] = w.slots[s].with_count(w.slots[s].count - 1);
            }
            w.used = Some(Used::Experience(xp));
            refresh_result(w);
            return result;
        }
        _ => {}
    }
    let Some((grid, _)) = w.layout.grid() else {
        return Stack::EMPTY;
    };
    let result = w.slots[0];
    if result.is_empty() {
        return Stack::EMPTY;
    }
    for i in grid {
        let s = w.slots[i];
        if s.is_empty() {
            continue;
        }
        w.slots[i] = match crafting_remainder(s.item) {
            Some(remainder) if s.count == 1 => Stack::new(remainder, 1),
            _ => s.with_count(s.count - 1),
        };
    }
    refresh_result(w);
    result
}

fn pickup(w: &mut Window, s: usize, left: bool) {
    let layout = w.layout;
    if Some(s) == layout.result_slot() {
        let result = w.slots[s];
        let fits = w.carried.is_empty()
            || (w.carried.same_kind(&result)
                && w.carried.count + result.count <= w.carried.max_stack());
        if !result.is_empty() && fits {
            let taken = take_result(w);
            let count = if w.carried.is_empty() {
                0
            } else {
                w.carried.count
            };
            *w.carried = taken.with_count(count + taken.count);
        }
        return;
    }
    let slot = w.slots[s];
    let carried = *w.carried;
    if carried.is_empty() {
        if !slot.is_empty() {
            let take = if left {
                slot.count
            } else {
                (slot.count + 1) / 2
            };
            *w.carried = slot.with_count(take);
            w.slots[s] = slot.with_count(slot.count - take);
        }
    } else if layout.may_place(s, &carried) {
        let limit = layout.limit(s, &carried);
        let wanted = if left { carried.count } else { 1 };
        if slot.is_empty() {
            let put = wanted.min(limit);
            w.slots[s] = carried.with_count(put);
            *w.carried = carried.with_count(carried.count - put);
        } else if slot.same_kind(&carried) {
            let put = wanted.min(limit - slot.count).max(0);
            w.slots[s] = slot.with_count(slot.count + put);
            *w.carried = carried.with_count(carried.count - put);
        } else if carried.count <= limit {
            w.slots[s] = carried;
            *w.carried = slot;
        }
    } else if slot.same_kind(&carried) {
        // A slot that takes nothing (furnace output): its items join the cursor.
        let take = slot.count.min(carried.max_stack() - carried.count).max(0);
        *w.carried = carried.with_count(carried.count + take);
        w.slots[s] = slot.with_count(slot.count - take);
    }
}

/// Moves `stack` into `range` (backwards if `reverse`): onto matching stacks first, then
/// into the first empty slot that takes it. Returns what is left.
fn move_into(w: &mut Window, mut stack: Stack, range: Range<usize>, reverse: bool) -> Stack {
    let order: Vec<usize> = if reverse {
        range.rev().collect()
    } else {
        range.collect()
    };
    if stack.max_stack() > 1 {
        for &i in &order {
            let s = w.slots[i];
            if stack.is_empty() {
                break;
            }
            if !s.is_empty() && s.same_kind(&stack) && w.layout.may_place(i, &stack) {
                let room = (w.layout.limit(i, &stack) - s.count).max(0);
                let moved = room.min(stack.count);
                w.slots[i] = s.with_count(s.count + moved);
                stack = stack.with_count(stack.count - moved);
            }
        }
    }
    if !stack.is_empty() {
        let empty = order
            .into_iter()
            .find(|&i| w.slots[i].is_empty() && w.layout.may_place(i, &stack));
        if let Some(i) = empty {
            let put = stack.count.min(w.layout.limit(i, &stack));
            w.slots[i] = stack.with_count(put);
            stack = stack.with_count(stack.count - put);
        }
    }
    stack
}

/// Shift-click: where a stack goes depends on the window and where it came from.
fn quick_move(w: &mut Window, s: usize) {
    let layout = w.layout;
    let player = layout.main().start..layout.hotbar().end;
    if Some(s) == layout.result_slot() {
        // Craft again and again while the whole result fits in the inventory.
        for _ in 0..MAX_CRAFTS {
            let result = w.slots[s];
            if result.is_empty() {
                break;
            }
            let saved: Vec<Stack> = w.slots[player.clone()].to_vec();
            if !move_into(w, result, player.clone(), true).is_empty() {
                w.slots[player.clone()].copy_from_slice(&saved);
                break;
            }
            take_result(w);
            if !w.slots[s].same_kind(&result) {
                break;
            }
        }
        return;
    }
    let stack = w.slots[s];
    if stack.is_empty() {
        return;
    }
    let (main, hotbar) = (layout.main(), layout.hotbar());
    let in_player = player.contains(&s);
    let rest = match layout {
        Layout::Inventory => {
            if in_player {
                match layout.armor_slot(&stack) {
                    Some(armor) if w.slots[armor].is_empty() => {
                        move_into(w, stack, armor..armor + 1, false)
                    }
                    _ if main.contains(&s) => move_into(w, stack, hotbar, false),
                    _ => move_into(w, stack, main, false),
                }
            } else {
                // Grid, armor and offhand go to the inventory.
                move_into(w, stack, 9..45, false)
            }
        }
        Layout::Crafting => {
            if in_player {
                let rest = move_into(w, stack, 1..10, false);
                if rest != stack {
                    rest
                } else if main.contains(&s) {
                    move_into(w, stack, hotbar, false)
                } else {
                    move_into(w, stack, main, false)
                }
            } else {
                move_into(w, stack, player, false)
            }
        }
        Layout::Furnace(kind) => {
            if s == 2 {
                move_into(w, stack, player, true)
            } else if !in_player {
                move_into(w, stack, player, false)
            } else if w.recipes.cooking(kind, stack.item).is_some() {
                move_into(w, stack, 0..1, false)
            } else if fuel_ticks(stack.item) > 0 {
                move_into(w, stack, 1..2, false)
            } else if main.contains(&s) {
                move_into(w, stack, hotbar, false)
            } else {
                move_into(w, stack, main, false)
            }
        }
        Layout::Chest(n) => {
            if in_player {
                move_into(w, stack, 0..n, false)
            } else {
                move_into(w, stack, player, true)
            }
        }
        Layout::Brewing => {
            if !in_player {
                move_into(w, stack, player, true)
            } else if is_blaze_powder(&stack) {
                // Blaze powder fuels first, then brews.
                let rest = move_into(w, stack, 4..5, false);
                if rest.is_empty() {
                    rest
                } else {
                    move_into(w, rest, 3..4, false)
                }
            } else if potions::is_ingredient(stack) {
                move_into(w, stack, 3..4, false)
            } else if potions::is_potion(stack) {
                move_into(w, stack, 0..3, false)
            } else if main.contains(&s) {
                move_into(w, stack, hotbar, false)
            } else {
                move_into(w, stack, main, false)
            }
        }
        Layout::Smithing => {
            if !in_player {
                move_into(w, stack, player, true)
            } else if let Some(slot) = w.recipes.smithing_slot(stack.item) {
                move_into(w, stack, slot..slot + 1, false)
            } else if main.contains(&s) {
                move_into(w, stack, hotbar, false)
            } else {
                move_into(w, stack, main, false)
            }
        }
        Layout::Stonecutter => {
            if !in_player {
                move_into(w, stack, player, true)
            } else if !w.recipes.stonecutting(stack.item).is_empty() {
                move_into(w, stack, 0..1, false)
            } else if main.contains(&s) {
                move_into(w, stack, hotbar, false)
            } else {
                move_into(w, stack, main, false)
            }
        }
        Layout::Anvil | Layout::Grindstone => {
            if !in_player {
                move_into(w, stack, player, true)
            } else {
                let rest = move_into(w, stack, 0..2, false);
                if rest != stack {
                    rest
                } else if main.contains(&s) {
                    move_into(w, stack, hotbar, false)
                } else {
                    move_into(w, stack, main, false)
                }
            }
        }
        Layout::Enchanting => {
            if !in_player {
                move_into(w, stack, player, true)
            } else if is_lapis(&stack) {
                move_into(w, stack, 1..2, false)
            } else if w.slots[0].is_empty() {
                move_into(w, stack, 0..1, false)
            } else if main.contains(&s) {
                move_into(w, stack, hotbar, false)
            } else {
                move_into(w, stack, main, false)
            }
        }
    };
    w.slots[s] = rest;
}

/// Number keys swap a slot with a hotbar slot; F swaps it with the offhand.
fn swap(w: &mut Window, s: usize, button: u8) {
    let layout = w.layout;
    let hotbar_slot = match button {
        b @ 0..=8 => Some(layout.hotbar().start + usize::from(b)),
        OFFHAND_BUTTON if layout == Layout::Inventory => Some(45),
        OFFHAND_BUTTON => None, // the offhand is outside this window
        _ => return,
    };
    if hotbar_slot == Some(s) {
        return;
    }
    let held = hotbar_slot.map_or(*w.offhand, |h| w.slots[h]);
    let set_held = |w: &mut Window, stack: Stack| match hotbar_slot {
        Some(h) => w.slots[h] = stack,
        None => *w.offhand = stack,
    };
    if Some(s) == layout.result_slot() {
        if held.is_empty() {
            let taken = take_result(w);
            set_held(w, taken);
        }
        return;
    }
    let slot = w.slots[s];
    if held.is_empty() {
        set_held(w, slot);
        w.slots[s] = Stack::EMPTY;
    } else if layout.may_place(s, &held) {
        let limit = layout.limit(s, &held);
        if slot.is_empty() {
            let put = held.count.min(limit);
            w.slots[s] = held.with_count(put);
            set_held(w, held.with_count(held.count - put));
        } else if held.count <= limit {
            w.slots[s] = held;
            set_held(w, slot);
        }
    }
}

/// Dragging the cursor stack over slots spreads it: evenly (left), one each (right) or
/// a full stack each (middle, creative).
fn drag(w: &mut Window, slot: Option<usize>, button: u8) {
    let (stage, kind) = (button & 3, (button >> 2) & 3);
    match stage {
        0 => {
            *w.drag = Drag::default();
            if !w.carried.is_empty() && (kind < 2 || (kind == 2 && w.creative)) {
                w.drag.kind = Some(kind);
            }
        }
        1 => {
            let (Some(k), Some(s)) = (w.drag.kind, slot) else {
                *w.drag = Drag::default();
                return;
            };
            let target = w.slots[s];
            let fits = (target.is_empty() || target.same_kind(w.carried))
                && w.layout.may_place(s, w.carried)
                && (k == 2 || w.carried.count as usize > w.drag.slots.len());
            if fits && !w.drag.slots.contains(&s) {
                w.drag.slots.push(s);
            }
        }
        2 => {
            let done = std::mem::take(w.drag);
            let Some(k) = done.kind else {
                return;
            };
            if done.slots.len() == 1 && k < 2 {
                pickup(w, done.slots[0], k == 0);
                return;
            }
            let source = *w.carried;
            let mut left = source.count;
            let per = match k {
                0 => source.count / done.slots.len().max(1) as i32,
                1 => 1,
                _ => source.max_stack(),
            };
            for &s in &done.slots {
                let target = w.slots[s];
                if !(target.is_empty() || target.same_kind(&source)) {
                    continue;
                }
                let existing = if target.is_empty() { 0 } else { target.count };
                let mut add = (existing + per).min(w.layout.limit(s, &source)) - existing;
                if k < 2 {
                    add = add.min(left);
                    left -= add;
                }
                w.slots[s] = source.with_count(existing + add);
            }
            if k < 2 {
                *w.carried = source.with_count(left);
            }
        }
        _ => *w.drag = Drag::default(),
    }
}

/// Double-click: gathers stacks matching the cursor, partial stacks first.
fn collect(w: &mut Window, s: usize) {
    let carried = *w.carried;
    if carried.is_empty() || !w.slots[s].is_empty() {
        return;
    }
    let max = carried.max_stack();
    let mut count = carried.count;
    for full_stacks_too in [false, true] {
        for i in 0..w.slots.len() {
            if count >= max {
                break;
            }
            let stack = w.slots[i];
            let partial = stack.count < stack.max_stack();
            if Some(i) == w.layout.result_slot()
                || stack.is_empty()
                || !stack.same_kind(&carried)
                || !(partial || full_stacks_too)
            {
                continue;
            }
            let take = stack.count.min(max - count);
            count += take;
            w.slots[i] = stack.with_count(stack.count - take);
        }
    }
    *w.carried = carried.with_count(count);
}

#[cfg(test)]
mod tests;
