//! Containers: the player inventory with its 2x2 crafting grid, crafting tables, chests,
//! barrels and furnaces.
//!
//! Clicks are not simulated on the server yet. The client reports the slots a click
//! changed; the server applies them only if no items appeared out of nowhere. The one
//! exception is taking a crafting result, which must also use up the ingredients.

use levitas_nbt::Compound;
use levitas_registry::{block_entity_type, menu, BlockInfo, STATE_BLOCK};

use super::*;
use crate::blocks;
use crate::click;
use crate::inventory::{items_from_nbt, items_to_nbt};
use crate::recipes::{fuel_remainder, fuel_ticks};

const CHEST_SLOTS: usize = 27;
const FURNACE_INPUT: usize = 0;
const FURNACE_FUEL: usize = 1;
const FURNACE_OUTPUT: usize = 2;
/// Window IDs cycle through 1..=100 like vanilla's.
const MAX_WINDOW_ID: i32 = 100;
/// Block event that sets how many players have a chest open (it animates the lid).
const BLOCK_EVENT_CHEST_VIEWERS: u8 = 1;
/// The player inventory part of a container window: main inventory then hotbar.
const PLAYER_PART: std::ops::Range<usize> = 9..45;

pub enum BlockEntity {
    Chest(Chest),
    Furnace(Furnace),
}

pub struct Chest {
    /// Block entity type: chests and barrels share this storage.
    kind: i32,
    items: Box<[Stack; CHEST_SLOTS]>,
    viewers: HashSet<i32>,
}

#[derive(Default)]
pub struct Furnace {
    items: [Stack; 3],
    /// Ticks left on the current fuel, and the fuel's full burn time.
    burn_left: i32,
    burn_total: i32,
    cook: i32,
    cook_total: i32,
    viewers: HashSet<i32>,
}

impl BlockEntity {
    /// The vanilla saved form, as stored in a chunk's `block_entities` list.
    pub fn to_nbt(&self, pos: BlockPos) -> Compound {
        let (id, items) = match self {
            BlockEntity::Chest(c) if c.kind == block_entity_type::BARREL => {
                ("minecraft:barrel", &c.items[..])
            }
            BlockEntity::Chest(c) if c.kind == block_entity_type::TRAPPED_CHEST => {
                ("minecraft:trapped_chest", &c.items[..])
            }
            BlockEntity::Chest(c) => ("minecraft:chest", &c.items[..]),
            BlockEntity::Furnace(f) => ("minecraft:furnace", &f.items[..]),
        };
        let mut nbt = Compound::new()
            .with("id", id)
            .with("x", pos.x)
            .with("y", pos.y)
            .with("z", pos.z)
            .with("keepPacked", false)
            .with("Items", items_to_nbt(items.iter().copied().enumerate()));
        if let BlockEntity::Furnace(f) = self {
            let short = |v: i32| v.clamp(0, i32::from(i16::MAX)) as i16;
            nbt.insert("lit_time_remaining", short(f.burn_left));
            nbt.insert("lit_total_time", short(f.burn_total));
            nbt.insert("cooking_time_spent", short(f.cook));
            nbt.insert("cooking_total_time", short(f.cook_total));
        }
        nbt
    }

    /// Reads a saved block entity Levitas understands, with its position.
    pub fn from_nbt(nbt: &Compound) -> Option<(BlockPos, BlockEntity)> {
        let pos = BlockPos::new(
            nbt.int("x")? as i32,
            nbt.int("y")? as i32,
            nbt.int("z")? as i32,
        );
        let mut entity = match nbt.str("id")? {
            "minecraft:chest" => Self::for_block(block::CHEST)?,
            "minecraft:trapped_chest" => Self::for_block(block::TRAPPED_CHEST)?,
            "minecraft:barrel" => Self::for_block(block::BARREL)?,
            "minecraft:furnace" => Self::for_block(block::FURNACE)?,
            _ => return None,
        };
        let items = entity.items_mut();
        for (slot, stack) in items_from_nbt(nbt.list("Items").unwrap_or_default()) {
            if let Some(s) = items.get_mut(slot) {
                *s = stack;
            }
        }
        if let BlockEntity::Furnace(f) = &mut entity {
            // Names before 1.21.4 are read too, for older worlds.
            let int =
                |new: &str, old: &str| nbt.int(new).or_else(|| nbt.int(old)).unwrap_or(0) as i32;
            f.burn_left = int("lit_time_remaining", "BurnTime");
            f.burn_total = int("lit_total_time", "BurnTime");
            f.cook = int("cooking_time_spent", "CookTime");
            f.cook_total = int("cooking_total_time", "CookTimeTotal");
        }
        Some((pos, entity))
    }

    /// The block entity a newly placed block needs, if any.
    pub fn for_block(state: u16) -> Option<Self> {
        let chest = |kind| {
            Some(BlockEntity::Chest(Chest {
                kind,
                items: Box::new([Stack::EMPTY; CHEST_SLOTS]),
                viewers: HashSet::new(),
            }))
        };
        match BlockInfo::of(state).name {
            "minecraft:chest" => chest(block_entity_type::CHEST),
            "minecraft:trapped_chest" => chest(block_entity_type::TRAPPED_CHEST),
            "minecraft:barrel" => chest(block_entity_type::BARREL),
            "minecraft:furnace" => Some(BlockEntity::Furnace(Furnace::default())),
            _ => None,
        }
    }

    pub fn type_id(&self) -> i32 {
        match self {
            BlockEntity::Chest(c) => c.kind,
            BlockEntity::Furnace(_) => block_entity_type::FURNACE,
        }
    }

    fn items(&self) -> &[Stack] {
        match self {
            BlockEntity::Chest(c) => &c.items[..],
            BlockEntity::Furnace(f) => &f.items,
        }
    }

    fn items_mut(&mut self) -> &mut [Stack] {
        match self {
            BlockEntity::Chest(c) => &mut c.items[..],
            BlockEntity::Furnace(f) => &mut f.items,
        }
    }

    fn viewers_mut(&mut self) -> &mut HashSet<i32> {
        match self {
            BlockEntity::Chest(c) => &mut c.viewers,
            BlockEntity::Furnace(f) => &mut f.viewers,
        }
    }
}

/// What an open window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The player's own inventory (window 0) with the 2x2 crafting grid.
    Inventory,
    Crafting,
    Furnace(BlockPos),
    Chest(BlockPos),
    /// Both halves of a double chest: the right half's 27 slots, then the left half's.
    DoubleChest(BlockPos, BlockPos),
}

impl View {
    /// Slots before the player inventory part of the window.
    fn container_len(self) -> usize {
        match self {
            View::Inventory => 0,
            View::Crafting => 10,
            View::Furnace(_) => 3,
            View::Chest(_) => CHEST_SLOTS,
            View::DoubleChest(..) => 2 * CHEST_SLOTS,
        }
    }

    /// The block entities shown in the window, each with its first window slot.
    fn parts(self) -> Vec<(BlockPos, usize)> {
        match self {
            View::Chest(pos) | View::Furnace(pos) => vec![(pos, 0)],
            View::DoubleChest(first, second) => vec![(first, 0), (second, CHEST_SLOTS)],
            View::Inventory | View::Crafting => Vec::new(),
        }
    }

    /// The slot arrangement of the window.
    fn layout(self) -> click::Layout {
        match self {
            View::Inventory => click::Layout::Inventory,
            View::Crafting => click::Layout::Crafting,
            View::Furnace(_) => click::Layout::Furnace,
            View::Chest(_) | View::DoubleChest(..) => click::Layout::Chest(self.container_len()),
        }
    }
}

/// A container window the player has open besides their inventory.
pub struct Open {
    pub window: i32,
    pub view: View,
    /// The crafting table grid; it lives in the window, not in the world.
    grid: [Stack; 9],
    result: Stack,
}

impl World {
    /// Opens the container at `pos` if `state` is one. Returns whether it was.
    pub(super) fn open_container(&mut self, id: i32, pos: BlockPos, state: u16) -> bool {
        let (view, menu, title) = match BlockInfo::of(state).name {
            "minecraft:crafting_table" => (View::Crafting, menu::CRAFTING, "Crafting"),
            "minecraft:chest" | "minecraft:trapped_chest" => {
                let partner = blocks::chest_partner(state, pos)
                    .filter(|&q| blocks::same_block(self.block(q), state));
                match partner {
                    // The right half comes first in a double chest's window.
                    Some(q) if blocks::prop(state, "type") == Some("right") => {
                        (View::DoubleChest(pos, q), menu::GENERIC_9X6, "Large Chest")
                    }
                    Some(q) => (View::DoubleChest(q, pos), menu::GENERIC_9X6, "Large Chest"),
                    None => (View::Chest(pos), menu::GENERIC_9X3, "Chest"),
                }
            }
            "minecraft:barrel" => (View::Chest(pos), menu::GENERIC_9X3, "Barrel"),
            "minecraft:furnace" => (View::Furnace(pos), menu::FURNACE, "Furnace"),
            _ => return false,
        };
        self.close_container(id);
        let mut first_viewer = false;
        for (part, _) in view.parts() {
            let part_state = self.block(part);
            if let Some(entity) = BlockEntity::for_block(part_state) {
                self.block_entities.entry(part).or_insert(entity);
            }
            if let Some(entity) = self.block_entities.get_mut(&part) {
                let viewers = entity.viewers_mut();
                viewers.insert(id);
                first_viewer |= viewers.len() == 1;
            }
        }

        let p = self.players.get_mut(&id).expect("player exists");
        p.next_window = p.next_window % MAX_WINDOW_ID + 1;
        let window = p.next_window;
        p.open = Some(Open {
            window,
            view,
            grid: [Stack::EMPTY; 9],
            result: Stack::EMPTY,
        });
        self.send(
            id,
            Writer::packet(clientbound::OPEN_SCREEN)
                .varint(window)
                .varint(menu)
                .text(title),
        );
        self.resync_window(id, window, view);
        match view {
            View::Chest(pos) => self.chest_viewers_changed(pos),
            View::DoubleChest(first, second) => {
                self.chest_viewers_changed(first);
                self.chest_viewers_changed(second);
            }
            View::Furnace(pos) => {
                if let Some(BlockEntity::Furnace(f)) = self.block_entities.get(&pos) {
                    let data = furnace_data(window, f);
                    for packet in data {
                        self.send(id, packet);
                    }
                }
            }
            _ => {}
        }
        // The screen first, then the sound, as vanilla orders them.
        if first_viewer {
            self.container_opened(view, true);
        }
        true
    }

    /// The server's view of every slot in a window.
    fn window_slots(&self, id: i32, view: View) -> Vec<Stack> {
        let p = &self.players[&id];
        let mut slots = match view {
            View::Inventory => return p.inventory.slots.to_vec(),
            View::Crafting => {
                let open = p.open.as_ref().expect("crafting window is open");
                let mut slots = vec![open.result];
                slots.extend_from_slice(&open.grid);
                slots
            }
            View::Chest(_) | View::Furnace(_) | View::DoubleChest(..) => {
                let mut slots = vec![Stack::EMPTY; view.container_len()];
                for (part, first) in view.parts() {
                    if let Some(entity) = self.block_entities.get(&part) {
                        let items = entity.items();
                        slots[first..first + items.len()].copy_from_slice(items);
                    }
                }
                slots
            }
        };
        slots.extend_from_slice(&p.inventory.slots[PLAYER_PART]);
        slots
    }

    /// Stores window slots back where they live. Crafting results are never stored: they
    /// are recomputed from the grid.
    fn store_window(&mut self, id: i32, view: View, slots: &[Stack]) {
        let n = view.container_len();
        if view != View::Inventory {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.slots[PLAYER_PART].copy_from_slice(&slots[n..n + PLAYER_PART.len()]);
        }
        match view {
            View::Inventory => {
                let p = self.players.get_mut(&id).expect("player exists");
                p.inventory.slots.copy_from_slice(slots);
            }
            View::Crafting => {
                let p = self.players.get_mut(&id).expect("player exists");
                let open = p.open.as_mut().expect("crafting window is open");
                open.result = slots[0];
                open.grid.copy_from_slice(&slots[1..10]);
            }
            View::Chest(_) | View::Furnace(_) | View::DoubleChest(..) => {
                for (part, first) in view.parts() {
                    if let Some(entity) = self.block_entities.get_mut(&part) {
                        let items = entity.items_mut();
                        let len = items.len();
                        items.copy_from_slice(&slots[first..first + len]);
                        self.dirty.insert(part.chunk());
                    }
                }
            }
        }
    }

    fn resync_window(&mut self, id: i32, window: i32, view: View) {
        let slots = self.window_slots(id, view);
        let p = self.players.get_mut(&id).expect("player exists");
        let content = p.inventory.window_content_packet(window, &slots);
        self.send(id, content);
    }

    /// Runs a click on the server and checks the client predicted the same; if not, the
    /// client gets the real window contents.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn container_click(
        &mut self,
        id: i32,
        window: i32,
        slot: i16,
        button: u8,
        mode: i32,
        changed: Vec<(i16, Stack)>,
        carried: Stack,
    ) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let view = match &p.open {
            _ if window == 0 => View::Inventory,
            Some(open) if open.window == window => open.view,
            _ => return,
        };
        let layout = view.layout();
        let before = self.window_slots(id, view);
        let mut slots = before.clone();
        let p = self.players.get_mut(&id).expect("player exists");
        let mut cursor = p.inventory.carried;
        let mut offhand = p.inventory.slots[OFFHAND];
        let mut drag = std::mem::take(&mut p.drag);
        let thrown = click::click(
            &mut click::Window {
                layout,
                slots: &mut slots,
                carried: &mut cursor,
                offhand: &mut offhand,
                drag: &mut drag,
                creative: p.mode == GAME_MODE_CREATIVE,
                recipes: &self.recipes,
            },
            click::Click { slot, button, mode },
        );
        p.drag = drag;
        p.inventory.carried = cursor;
        if view != View::Inventory {
            p.inventory.slots[OFFHAND] = offhand;
        }
        self.store_window(id, view, &slots);
        for stack in thrown {
            self.throw_from_player(id, stack);
        }

        // Compare with what the client says happened. Only the server computes crafting
        // results, so those are sent rather than counted as a disagreement.
        let mut client = before.clone();
        for (s, stack) in changed {
            if let Some(s) = usize::try_from(s).ok().filter(|&s| s < client.len()) {
                client[s] = stack;
            }
        }
        let same = |a: &Stack, b: &Stack| {
            (a.is_empty() && b.is_empty()) || (a.item == b.item && a.count == b.count)
        };
        let result = layout.result_slot();
        let agrees = same(&carried, &cursor)
            && (0..slots.len()).all(|i| Some(i) == result || same(&client[i], &slots[i]));
        if !agrees {
            self.resync_window(id, window, view);
        } else if let Some(r) = result.filter(|&r| !same(&client[r], &slots[r])) {
            let p = self.players.get_mut(&id).expect("player exists");
            let update = p.inventory.window_slot_packet(window, r, slots[r]);
            self.send(id, update);
        }
        let parts = view.parts();
        for (i, &(part, first)) in parts.iter().enumerate() {
            let end = parts
                .get(i + 1)
                .map_or(view.container_len(), |&(_, next)| next);
            let changed: Vec<usize> = (first..end)
                .filter(|&s| before[s] != slots[s])
                .map(|s| s - first)
                .collect();
            self.notify_viewers(part, &changed, Some(id));
        }
    }

    /// Sends changed container slots to everyone (but `except`) viewing the container.
    fn notify_viewers(&mut self, pos: BlockPos, slots: &[usize], except: Option<i32>) {
        let Some(entity) = self.block_entities.get_mut(&pos) else {
            return;
        };
        let items = entity.items().to_vec();
        let viewers: Vec<i32> = entity.viewers_mut().iter().copied().collect();
        for viewer in viewers.into_iter().filter(|&v| Some(v) != except) {
            let Some(p) = self.players.get_mut(&viewer) else {
                continue;
            };
            let Some((window, first)) = p.open.as_ref().and_then(|o| {
                let first = o.view.parts().into_iter().find(|&(q, _)| q == pos)?.1;
                Some((o.window, first))
            }) else {
                continue;
            };
            let updates: Vec<Writer> = slots
                .iter()
                .map(|&s| p.inventory.window_slot_packet(window, first + s, items[s]))
                .collect();
            for update in updates {
                self.send(viewer, update);
            }
        }
    }

    /// The first player opening a container, or the last one closing it: chests and
    /// barrels make a sound, and barrels show their open state.
    fn container_opened(&mut self, view: View, open: bool) {
        let parts = view.parts();
        let Some(&(first, _)) = parts.first() else {
            return;
        };
        let state = self.block(first);
        let action = if open { "open" } else { "close" };
        // Double chests sound once, from between their halves.
        let n = parts.len() as f64;
        let centre = [0, 1, 2].map(|axis| {
            parts
                .iter()
                .map(|(p, _)| f64::from([p.x, p.y, p.z][axis]) + 0.5)
                .sum::<f64>()
                / n
        });
        let pitch = self.rng.f64() as f32 * 0.1 + 0.9;
        match blocks::name(state) {
            "minecraft:chest" | "minecraft:trapped_chest" => {
                let sound = format!("minecraft:block.chest.{action}");
                self.play_sound_at(&sound, centre, 0.5, pitch, None);
            }
            "minecraft:barrel" => {
                self.set_block(first, blocks::with_bool(state, "open", open));
                // The sound comes from the barrel's lid side.
                let step = blocks::prop(state, "facing")
                    .and_then(blocks::Dir::from_name)
                    .map_or([0; 3], blocks::Dir::step);
                let at = [0, 1, 2].map(|i| centre[i] + f64::from(step[i]) * 0.5);
                let sound = format!("minecraft:block.barrel.{action}");
                self.play_sound_at(&sound, at, 0.5, pitch, None);
            }
            _ => {}
        }
    }

    fn chest_viewers_changed(&mut self, pos: BlockPos) {
        let Some(BlockEntity::Chest(chest)) = self.block_entities.get(&pos) else {
            return;
        };
        let viewers = chest.viewers.len().min(255) as u8;
        let state = self.block(pos);
        if !blocks::is_chest(state) {
            return; // only chests have a lid to animate
        }
        self.broadcast(
            Writer::packet(clientbound::BLOCK_EVENT)
                .position(pos)
                .u8(BLOCK_EVENT_CHEST_VIEWERS)
                .u8(viewers)
                .varint(i32::from(STATE_BLOCK[usize::from(state)])),
            None,
        );
    }

    /// Handles the client closing a window.
    pub(super) fn container_closed(&mut self, id: i32, window: i32) {
        if window == 0 {
            // The 2x2 grid and cursor go back into the inventory, as in vanilla.
            let Some(p) = self.players.get_mut(&id) else {
                return;
            };
            let returned: Vec<Stack> = CRAFTING_GRID
                .map(|s| std::mem::take(&mut p.inventory.slots[s]))
                .collect();
            p.inventory.slots[0] = Stack::EMPTY; // the result of the now empty grid
            self.return_to_inventory(id, returned);
        } else if self
            .players
            .get(&id)
            .and_then(|p| p.open.as_ref())
            .is_some_and(|o| o.window == window)
        {
            self.close_container(id);
        }
    }

    /// Closes the player's open container, if any, putting crafting grid and cursor items
    /// back into their inventory.
    pub(super) fn close_container(&mut self, id: i32) {
        let Some(open) = self.players.get_mut(&id).and_then(|p| p.open.take()) else {
            return;
        };
        let mut returned = Vec::new();
        match open.view {
            View::Crafting => returned.extend(open.grid),
            View::Chest(_) | View::Furnace(_) | View::DoubleChest(..) => {
                let mut last_viewer = false;
                for (part, _) in open.view.parts() {
                    if let Some(entity) = self.block_entities.get_mut(&part) {
                        let viewers = entity.viewers_mut();
                        last_viewer |= viewers.remove(&id) && viewers.is_empty();
                    }
                    self.chest_viewers_changed(part);
                }
                if last_viewer {
                    self.container_opened(open.view, false);
                }
            }
            View::Inventory => {}
        }
        self.return_to_inventory(id, returned);
    }

    /// Puts stacks and the cursor stack into the inventory, throwing what does not fit,
    /// then resends the inventory.
    fn return_to_inventory(&mut self, id: i32, mut stacks: Vec<Stack>) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        stacks.push(std::mem::take(&mut p.inventory.carried));
        let mut overflow = Vec::new();
        for stack in stacks.into_iter().filter(|s| !s.is_empty()) {
            let (_, left) = p.inventory.add(stack);
            if left > 0 {
                overflow.push(stack.with_count(left));
            }
        }
        let content = p.inventory.content_packet();
        self.send(id, content);
        for stack in overflow {
            self.throw_from_player(id, stack);
        }
    }

    /// Removes the block entity at `pos` (its block was broken): its items drop and
    /// anyone viewing it has the window closed.
    pub(super) fn remove_block_entity(&mut self, pos: BlockPos) {
        let viewers: Vec<i32> = match self.block_entities.get_mut(&pos) {
            Some(entity) => entity.viewers_mut().iter().copied().collect(),
            None => return,
        };
        for viewer in viewers {
            let window = self
                .players
                .get(&viewer)
                .and_then(|p| p.open.as_ref())
                .map(|o| o.window);
            if let Some(window) = window {
                self.close_container(viewer);
                self.send(
                    viewer,
                    Writer::packet(clientbound::CONTAINER_CLOSE).varint(window),
                );
            }
        }
        let entity = self.block_entities.remove(&pos).expect("checked above");
        for &stack in entity.items() {
            self.drop_at_block(pos, stack);
        }
    }

    /// Block entities in a chunk, as `(position, type)`, for the chunk packet.
    pub(super) fn block_entities_in(&self, cx: i32, cz: i32) -> Vec<(BlockPos, i32)> {
        self.block_entities
            .iter()
            .filter(|(pos, _)| pos.chunk() == (cx, cz))
            .map(|(&pos, e)| (pos, e.type_id()))
            .collect()
    }

    pub(super) fn tick_furnaces(&mut self) {
        let mut lit_changes = Vec::new();
        let mut updates = Vec::new();
        for (&pos, entity) in self.block_entities.iter_mut() {
            let BlockEntity::Furnace(f) = entity else {
                continue;
            };
            let was_lit = f.burn_left > 0;
            let mut changed = Vec::new();
            let input = f.items[FURNACE_INPUT];
            let recipe = (!input.is_empty())
                .then(|| self.recipes.smelting(input.item))
                .flatten();
            let output = f.items[FURNACE_OUTPUT];
            let can_smelt = recipe.is_some_and(|r| {
                output.is_empty()
                    || (output.item == r.result.item
                        && output.count + r.result.count <= output.max_stack())
            });

            if f.burn_left > 0 {
                f.burn_left -= 1;
            }
            let fuel = f.items[FURNACE_FUEL];
            if f.burn_left == 0 && can_smelt && !fuel.is_empty() && fuel_ticks(fuel.item) > 0 {
                f.burn_left = fuel_ticks(fuel.item);
                f.burn_total = f.burn_left;
                f.items[FURNACE_FUEL] = match fuel_remainder(fuel.item) {
                    Some(remainder) => Stack::new(remainder, 1),
                    None if fuel.count > 1 => Stack::new(fuel.item, fuel.count - 1),
                    None => Stack::EMPTY,
                };
                changed.push(FURNACE_FUEL);
            }
            if let Some(r) = recipe {
                f.cook_total = r.cook_time;
            }
            match recipe {
                Some(r) if f.burn_left > 0 && can_smelt => {
                    f.cook += 1;
                    if f.cook >= f.cook_total {
                        f.cook = 0;
                        f.items[FURNACE_INPUT].count -= 1;
                        if f.items[FURNACE_INPUT].count <= 0 {
                            f.items[FURNACE_INPUT] = Stack::EMPTY;
                        }
                        let out = &mut f.items[FURNACE_OUTPUT];
                        if out.is_empty() {
                            *out = r.result;
                        } else {
                            out.count += r.result.count;
                        }
                        changed.extend([FURNACE_INPUT, FURNACE_OUTPUT]);
                    }
                }
                _ => f.cook = (f.cook - 2).max(0),
            }

            if was_lit != (f.burn_left > 0) {
                lit_changes.push((pos, f.burn_left > 0));
            }
            if f.burn_left > 0 || f.cook > 0 || !changed.is_empty() {
                self.dirty.insert(pos.chunk());
            }
            if !f.viewers.is_empty() {
                updates.push((pos, changed));
            }
        }

        for (pos, changed) in updates {
            self.notify_viewers(pos, &changed, None);
            let Some(BlockEntity::Furnace(f)) = self.block_entities.get(&pos) else {
                continue;
            };
            let viewers: Vec<i32> = f.viewers.iter().copied().collect();
            for viewer in viewers {
                let Some(window) = self
                    .players
                    .get(&viewer)
                    .and_then(|p| p.open.as_ref())
                    .map(|o| o.window)
                else {
                    continue;
                };
                let Some(BlockEntity::Furnace(f)) = self.block_entities.get(&pos) else {
                    break;
                };
                for packet in furnace_data(window, f) {
                    self.send(viewer, packet);
                }
            }
        }
        for (pos, lit) in lit_changes {
            let state = self.block(pos);
            let info = BlockInfo::of(state);
            if let Some(new) = info.with(state, "lit", if lit { "true" } else { "false" }) {
                self.set_block(pos, new);
            }
        }
    }
}

/// The furnace's progress bars: fuel left, fuel total, cook progress, cook total.
fn furnace_data(window: i32, f: &Furnace) -> Vec<Writer> {
    [f.burn_left, f.burn_total, f.cook, f.cook_total]
        .into_iter()
        .enumerate()
        .map(|(property, value)| {
            Writer::packet(clientbound::CONTAINER_SET_DATA)
                .varint(window)
                .i16(property as i16)
                .i16(value.clamp(0, i32::from(i16::MAX)) as i16)
        })
        .collect()
}
