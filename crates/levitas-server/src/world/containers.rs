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
use crate::recipes::{fuel_remainder, fuel_ticks, Cooker};

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
    Campfire(super::campfire::Campfire),
    /// A block entity holding nothing Levitas tracks (daylight detectors, bells).
    Plain(Plain),
    Spawner(super::spawners::Spawner),
}

pub struct Plain {
    kind: i32,
    viewers: HashSet<i32>,
}

/// Plain storage: chests, barrels, hoppers, droppers and dispensers.
pub struct Chest {
    /// Block entity type: these blocks share this storage.
    pub kind: i32,
    pub items: Box<[Stack]>,
    viewers: HashSet<i32>,
    /// Ticks until a hopper moves its next item.
    pub cooldown: u32,
}

/// Slots of a hopper, and of a dropper or dispenser.
pub const HOPPER_SLOTS: usize = 5;
pub const DISPENSER_SLOTS: usize = 9;

#[derive(Default)]
pub struct Furnace {
    kind: Cooker,
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
            BlockEntity::Chest(c) if c.kind == block_entity_type::HOPPER => {
                ("minecraft:hopper", &c.items[..])
            }
            BlockEntity::Chest(c) if c.kind == block_entity_type::DROPPER => {
                ("minecraft:dropper", &c.items[..])
            }
            BlockEntity::Chest(c) if c.kind == block_entity_type::DISPENSER => {
                ("minecraft:dispenser", &c.items[..])
            }
            BlockEntity::Chest(c) => ("minecraft:chest", &c.items[..]),
            BlockEntity::Furnace(f) => (cooker_id(f.kind), &f.items[..]),
            BlockEntity::Campfire(c) if c.soul => ("minecraft:soul_campfire", &c.items[..]),
            BlockEntity::Campfire(c) => ("minecraft:campfire", &c.items[..]),
            BlockEntity::Plain(p) if p.kind == block_entity_type::BELL => {
                ("minecraft:bell", &[][..])
            }
            BlockEntity::Plain(_) => ("minecraft:daylight_detector", &[][..]),
            BlockEntity::Spawner(_) => ("minecraft:mob_spawner", &[][..]),
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
        if let BlockEntity::Campfire(c) = self {
            c.to_nbt(&mut nbt);
        }
        if let BlockEntity::Spawner(s) = self {
            s.to_nbt(&mut nbt);
        }
        nbt
    }

    /// What clients need to draw the block entity, for chunk and update packets.
    pub fn network_data(&self) -> Option<Compound> {
        match self {
            BlockEntity::Campfire(c) => Some(c.network_data()),
            BlockEntity::Spawner(s) => Some(s.network_data()),
            _ => None,
        }
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
            "minecraft:hopper" => Self::for_block(block::HOPPER)?,
            "minecraft:dropper" => Self::for_block(block::DROPPER)?,
            "minecraft:dispenser" => Self::for_block(block::DISPENSER)?,
            "minecraft:furnace" => Self::for_block(block::FURNACE)?,
            "minecraft:smoker" => Self::for_block(block::SMOKER)?,
            "minecraft:blast_furnace" => Self::for_block(block::BLAST_FURNACE)?,
            "minecraft:campfire" => Self::for_block(block::CAMPFIRE)?,
            "minecraft:soul_campfire" => Self::for_block(block::SOUL_CAMPFIRE)?,
            "minecraft:bell" => Self::for_block(block::BELL)?,
            "minecraft:mob_spawner" => {
                BlockEntity::Spawner(super::spawners::Spawner::read_nbt(nbt)?)
            }
            "minecraft:daylight_detector" => Self::for_block(block::DAYLIGHT_DETECTOR)?,
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
        if let BlockEntity::Campfire(c) = &mut entity {
            c.read_nbt(nbt);
        }
        Some((pos, entity))
    }

    /// The block entity a newly placed block needs, if any.
    pub fn for_block(state: u16) -> Option<Self> {
        let store = |kind, slots| {
            Some(BlockEntity::Chest(Chest {
                kind,
                items: vec![Stack::EMPTY; slots].into_boxed_slice(),
                viewers: HashSet::new(),
                cooldown: 0,
            }))
        };
        let chest = |kind| store(kind, CHEST_SLOTS);
        match BlockInfo::of(state).name {
            "minecraft:chest" => chest(block_entity_type::CHEST),
            "minecraft:trapped_chest" => chest(block_entity_type::TRAPPED_CHEST),
            "minecraft:barrel" => chest(block_entity_type::BARREL),
            "minecraft:hopper" => store(block_entity_type::HOPPER, HOPPER_SLOTS),
            "minecraft:dropper" => store(block_entity_type::DROPPER, DISPENSER_SLOTS),
            "minecraft:dispenser" => store(block_entity_type::DISPENSER, DISPENSER_SLOTS),
            "minecraft:furnace" => Some(BlockEntity::Furnace(Furnace::default())),
            "minecraft:smoker" => Some(BlockEntity::Furnace(Furnace {
                kind: Cooker::Smoker,
                ..Furnace::default()
            })),
            "minecraft:blast_furnace" => Some(BlockEntity::Furnace(Furnace {
                kind: Cooker::BlastFurnace,
                ..Furnace::default()
            })),
            "minecraft:bell" => Some(BlockEntity::Plain(Plain {
                kind: block_entity_type::BELL,
                viewers: HashSet::new(),
            })),
            "minecraft:daylight_detector" => Some(BlockEntity::Plain(Plain {
                kind: block_entity_type::DAYLIGHT_DETECTOR,
                viewers: HashSet::new(),
            })),
            name @ ("minecraft:campfire" | "minecraft:soul_campfire") => {
                Some(BlockEntity::Campfire(super::campfire::Campfire {
                    soul: name == "minecraft:soul_campfire",
                    ..Default::default()
                }))
            }
            _ => None,
        }
    }

    pub fn type_id(&self) -> i32 {
        match self {
            BlockEntity::Chest(c) => c.kind,
            BlockEntity::Furnace(f) => match f.kind {
                Cooker::Furnace => block_entity_type::FURNACE,
                Cooker::Smoker => block_entity_type::SMOKER,
                Cooker::BlastFurnace => block_entity_type::BLAST_FURNACE,
                Cooker::Campfire => block_entity_type::CAMPFIRE,
            },
            BlockEntity::Campfire(_) => block_entity_type::CAMPFIRE,
            BlockEntity::Plain(p) => p.kind,
            BlockEntity::Spawner(_) => block_entity_type::MOB_SPAWNER,
        }
    }

    /// The stacks inside, for comparators.
    pub fn stacks(&self) -> &[Stack] {
        self.items()
    }

    pub fn stacks_mut(&mut self) -> &mut [Stack] {
        self.items_mut()
    }

    fn items(&self) -> &[Stack] {
        match self {
            BlockEntity::Chest(c) => &c.items[..],
            BlockEntity::Furnace(f) => &f.items,
            BlockEntity::Campfire(c) => &c.items,
            BlockEntity::Plain(_) | BlockEntity::Spawner(_) => &[],
        }
    }

    pub fn items_mut(&mut self) -> &mut [Stack] {
        match self {
            BlockEntity::Chest(c) => &mut c.items[..],
            BlockEntity::Furnace(f) => &mut f.items,
            BlockEntity::Campfire(c) => &mut c.items,
            BlockEntity::Plain(_) | BlockEntity::Spawner(_) => &mut [],
        }
    }

    fn viewers_mut(&mut self) -> &mut HashSet<i32> {
        match self {
            BlockEntity::Chest(c) => &mut c.viewers,
            BlockEntity::Furnace(f) => &mut f.viewers,
            BlockEntity::Campfire(c) => &mut c.viewers,
            BlockEntity::Plain(p) => &mut p.viewers,
            BlockEntity::Spawner(s) => &mut s.viewers,
        }
    }
}

/// What an open window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The player's own inventory (window 0) with the 2x2 crafting grid.
    Inventory,
    Crafting,
    Furnace(BlockPos, Cooker),
    /// Plain storage with this many slots.
    Chest(BlockPos, usize),
    /// Both halves of a double chest: the right half's 27 slots, then the left half's.
    DoubleChest(BlockPos, BlockPos),
    /// An enchanting table; its two slots live in the window, like a crafting grid.
    Enchanting(BlockPos),
    /// An anvil or a grindstone: two inputs and a result, living in the window.
    Anvil(BlockPos),
    Grindstone(BlockPos),
    Stonecutter(BlockPos),
    Smithing(BlockPos),
}

impl View {
    /// Slots before the player inventory part of the window.
    fn container_len(self) -> usize {
        match self {
            View::Inventory => 0,
            View::Crafting => 10,
            View::Furnace(..) => 3,
            View::Chest(_, slots) => slots,
            View::DoubleChest(..) => 2 * CHEST_SLOTS,
            View::Enchanting(_) => 2,
            View::Anvil(_) | View::Grindstone(_) => 3,
            View::Stonecutter(_) => 2,
            View::Smithing(_) => 4,
        }
    }

    /// The block entities shown in the window, each with its first window slot.
    fn parts(self) -> Vec<(BlockPos, usize)> {
        match self {
            View::Chest(pos, _) | View::Furnace(pos, _) => vec![(pos, 0)],
            View::DoubleChest(first, second) => vec![(first, 0), (second, CHEST_SLOTS)],
            View::Inventory
            | View::Crafting
            | View::Enchanting(_)
            | View::Anvil(_)
            | View::Grindstone(_)
            | View::Stonecutter(_)
            | View::Smithing(_) => Vec::new(),
        }
    }

    /// The slot arrangement of the window.
    fn layout(self) -> click::Layout {
        match self {
            View::Inventory => click::Layout::Inventory,
            View::Crafting => click::Layout::Crafting,
            View::Furnace(_, kind) => click::Layout::Furnace(kind),
            View::Chest(..) | View::DoubleChest(..) => click::Layout::Chest(self.container_len()),
            View::Enchanting(_) => click::Layout::Enchanting,
            View::Anvil(_) => click::Layout::Anvil,
            View::Grindstone(_) => click::Layout::Grindstone,
            View::Stonecutter(_) => click::Layout::Stonecutter,
            View::Smithing(_) => click::Layout::Smithing,
        }
    }
}

/// A container window the player has open besides their inventory.
pub struct Open {
    pub window: i32,
    pub view: View,
    /// The crafting table grid, or the enchanting table's item and lapis; they live in
    /// the window, not in the world.
    pub grid: [Stack; 9],
    result: Stack,
    /// What the enchanting table offers in its three slots.
    pub offers: [super::enchanting_table::Offer; 3],
    /// The stonecutter recipe picked.
    pub choice: Option<usize>,
}

impl World {
    /// Opens the container at `pos` if `state` is one. Returns whether it was.
    pub(super) fn open_container(&mut self, id: i32, pos: BlockPos, state: u16) -> bool {
        let (view, menu, title) = match BlockInfo::of(state).name {
            "minecraft:crafting_table" => (View::Crafting, menu::CRAFTING, "Crafting"),
            "minecraft:enchanting_table" => (View::Enchanting(pos), menu::ENCHANTMENT, "Enchant"),
            "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil" => {
                (View::Anvil(pos), menu::ANVIL, "Repair & Name")
            }
            "minecraft:grindstone" => (
                View::Grindstone(pos),
                menu::GRINDSTONE,
                "Repair & Disenchant",
            ),
            "minecraft:stonecutter" => (View::Stonecutter(pos), menu::STONECUTTER, "Stonecutter"),
            "minecraft:smithing_table" => (View::Smithing(pos), menu::SMITHING, "Upgrade Gear"),
            "minecraft:chest" | "minecraft:trapped_chest" => {
                let partner = blocks::chest_partner(state, pos)
                    .filter(|&q| blocks::same_block(self.block(q), state));
                match partner {
                    // The right half comes first in a double chest's window.
                    Some(q) if blocks::prop(state, "type") == Some("right") => {
                        (View::DoubleChest(pos, q), menu::GENERIC_9X6, "Large Chest")
                    }
                    Some(q) => (View::DoubleChest(q, pos), menu::GENERIC_9X6, "Large Chest"),
                    None => (View::Chest(pos, CHEST_SLOTS), menu::GENERIC_9X3, "Chest"),
                }
            }
            "minecraft:barrel" => (View::Chest(pos, CHEST_SLOTS), menu::GENERIC_9X3, "Barrel"),
            "minecraft:hopper" => (View::Chest(pos, HOPPER_SLOTS), menu::HOPPER, "Item Hopper"),
            "minecraft:dropper" => (
                View::Chest(pos, DISPENSER_SLOTS),
                menu::GENERIC_3X3,
                "Dropper",
            ),
            "minecraft:dispenser" => (
                View::Chest(pos, DISPENSER_SLOTS),
                menu::GENERIC_3X3,
                "Dispenser",
            ),
            "minecraft:furnace" => (
                View::Furnace(pos, Cooker::Furnace),
                menu::FURNACE,
                "Furnace",
            ),
            "minecraft:smoker" => (View::Furnace(pos, Cooker::Smoker), menu::SMOKER, "Smoker"),
            "minecraft:blast_furnace" => (
                View::Furnace(pos, Cooker::BlastFurnace),
                menu::BLAST_FURNACE,
                "Blast Furnace",
            ),
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
            offers: Default::default(),
            choice: None,
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
            View::Chest(pos, _) => self.chest_viewers_changed(pos),
            View::DoubleChest(first, second) => {
                self.chest_viewers_changed(first);
                self.chest_viewers_changed(second);
            }
            View::Enchanting(_) => self.refresh_offers(id),
            View::Anvil(_) => self.send_anvil_cost(id),
            View::Furnace(pos, _) => {
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
            View::Enchanting(_) => {
                let open = p.open.as_ref().expect("enchanting window is open");
                open.grid[..2].to_vec()
            }
            View::Anvil(_) | View::Grindstone(_) => {
                let open = p.open.as_ref().expect("window is open");
                open.grid[..3].to_vec()
            }
            View::Stonecutter(_) => {
                let open = p.open.as_ref().expect("window is open");
                open.grid[..2].to_vec()
            }
            View::Smithing(_) => {
                let open = p.open.as_ref().expect("window is open");
                open.grid[..4].to_vec()
            }
            View::Chest(..) | View::Furnace(..) | View::DoubleChest(..) => {
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
            View::Enchanting(_) => {
                let p = self.players.get_mut(&id).expect("player exists");
                let open = p.open.as_mut().expect("enchanting window is open");
                open.grid[..2].copy_from_slice(&slots[..2]);
            }
            View::Anvil(_) | View::Grindstone(_) => {
                let p = self.players.get_mut(&id).expect("player exists");
                let open = p.open.as_mut().expect("window is open");
                open.grid[..3].copy_from_slice(&slots[..3]);
            }
            View::Stonecutter(_) => {
                let p = self.players.get_mut(&id).expect("player exists");
                let open = p.open.as_mut().expect("window is open");
                open.grid[..2].copy_from_slice(&slots[..2]);
            }
            View::Smithing(_) => {
                let p = self.players.get_mut(&id).expect("player exists");
                let open = p.open.as_mut().expect("window is open");
                open.grid[..4].copy_from_slice(&slots[..4]);
            }
            View::Chest(..) | View::Furnace(..) | View::DoubleChest(..) => {
                for (part, first) in view.parts() {
                    if let Some(entity) = self.block_entities.get_mut(&part) {
                        let items = entity.items_mut();
                        let len = items.len();
                        items.copy_from_slice(&slots[first..first + len]);
                        self.dirty.insert(part.chunk());
                        self.redstone_dirty.push(part);
                    }
                }
            }
        }
    }

    pub(super) fn resync_window(&mut self, id: i32, window: i32, view: View) {
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
        let mut w = click::Window {
            layout,
            slots: &mut slots,
            carried: &mut cursor,
            offhand: &mut offhand,
            drag: &mut drag,
            creative: p.mode == GAME_MODE_CREATIVE,
            recipes: &self.recipes,
            xp_level: p.xp_level,
            used: None,
            choice: p.open.as_ref().and_then(|o| o.choice),
        };
        let thrown = click::click(&mut w, click::Click { slot, button, mode });
        let used = w.used;
        let choice = w.choice;
        if let Some(open) = p.open.as_mut() {
            let input_changed = before.first() != slots.first();
            // A different item in the stonecutter forgets the picked recipe.
            open.choice = if matches!(view, View::Stonecutter(_))
                && input_changed
                && slots.first().is_none_or(|s| !before[0].same_kind(s))
            {
                None
            } else {
                choice
            };
        }
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
        if matches!(view, View::Enchanting(_)) {
            self.refresh_offers(id);
        }
        match (view, used) {
            (View::Anvil(pos), Some(click::Used::Levels(cost))) => self.anvil_used(id, pos, cost),
            (View::Grindstone(pos), Some(click::Used::Experience(xp))) => {
                self.grindstone_used(pos, xp)
            }
            _ => {}
        }
        if let View::Anvil(_) = view {
            self.send_anvil_cost(id);
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
    pub(super) fn notify_viewers(&mut self, pos: BlockPos, slots: &[usize], except: Option<i32>) {
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

    /// A button in a window: an enchanting table's slots, or a stonecutter's recipes.
    pub(super) fn window_button(&mut self, id: i32, window: i32, button: i32) {
        let view = self
            .players
            .get(&id)
            .and_then(|p| p.open.as_ref())
            .filter(|o| o.window == window)
            .map(|o| o.view);
        match view {
            Some(View::Enchanting(_)) => self.enchant_button(id, window, button),
            Some(View::Stonecutter(_)) => {
                let p = self.players.get_mut(&id).expect("player exists");
                let open = p.open.as_mut().expect("window is open");
                let options = self.recipes.stonecutting(open.grid[0].item);
                let Some(&result) = usize::try_from(button).ok().and_then(|b| options.get(b))
                else {
                    return;
                };
                open.choice = Some(button as usize);
                open.grid[1] = result;
                let update = p.inventory.window_slot_packet(window, 1, result);
                self.send(id, update);
            }
            _ => {}
        }
    }

    /// Tells the player what the anvil's result costs.
    fn send_anvil_cost(&mut self, id: i32) {
        let Some(open) = self.players.get(&id).and_then(|p| p.open.as_ref()) else {
            return;
        };
        let cost = click::anvil_cost(&open.grid[..3]);
        let packet = Writer::packet(clientbound::CONTAINER_SET_DATA)
            .varint(open.window)
            .i16(0)
            .i16(cost.min(i32::from(i16::MAX)) as i16);
        self.send(id, packet);
    }

    /// The player paid for an anvil's work. The anvil wears: 12% of uses chip it a
    /// stage, and a damaged anvil breaks.
    fn anvil_used(&mut self, id: i32, pos: BlockPos, cost: i32) {
        if self.players.get(&id).is_some_and(|p| p.survival()) {
            self.give_levels(id, -cost);
            if self.rng.f64() < 0.12 {
                let state = self.block(pos);
                let next = match blocks::name(state) {
                    "minecraft:anvil" => Some("minecraft:chipped_anvil"),
                    "minecraft:chipped_anvil" => Some("minecraft:damaged_anvil"),
                    _ => None,
                };
                match next.and_then(BlockInfo::by_name) {
                    Some(info) => {
                        let facing = blocks::prop(state, "facing").unwrap_or("north");
                        let worn = blocks::with(info.default_state, "facing", facing);
                        self.set_block(pos, worn);
                    }
                    None => {
                        self.set_block_updating(pos, block::AIR);
                        self.close_container(id);
                        self.play_sound("minecraft:block.anvil.destroy", pos, None);
                        return;
                    }
                }
            }
        }
        self.play_sound("minecraft:block.anvil.use", pos, None);
    }

    /// A grindstone gave its result: some experience comes back, at the grindstone.
    fn grindstone_used(&mut self, pos: BlockPos, base: i32) {
        let half = (base + 1) / 2;
        let xp = half + self.rng.below(half.max(0) as u32 + 1) as i32;
        if xp > 0 {
            let centre = [pos.x, pos.y, pos.z].map(|v| f64::from(v) + 0.5);
            self.spawn_xp(centre, xp);
        }
        self.play_sound("minecraft:block.grindstone.use", pos, None);
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
            View::Enchanting(_) => returned.extend(&open.grid[..2]),
            View::Anvil(_) | View::Grindstone(_) => returned.extend(&open.grid[..2]),
            View::Stonecutter(_) => returned.push(open.grid[0]),
            View::Smithing(_) => returned.extend(&open.grid[..3]),
            View::Chest(..) | View::Furnace(..) | View::DoubleChest(..) => {
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

    /// Block entities in a chunk, as `(position, type, network data)`, for the chunk
    /// packet.
    pub(super) fn block_entities_in(
        &self,
        cx: i32,
        cz: i32,
    ) -> Vec<(BlockPos, i32, Option<Compound>)> {
        self.block_entities
            .iter()
            .filter(|(pos, _)| pos.chunk() == (cx, cz))
            .map(|(&pos, e)| (pos, e.type_id(), e.network_data()))
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
                .then(|| self.recipes.cooking(f.kind, input.item))
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
                // Smokers and blast furnaces burn through fuel twice as fast.
                f.burn_left = match f.kind {
                    Cooker::Furnace => fuel_ticks(fuel.item),
                    _ => fuel_ticks(fuel.item) / 2,
                };
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
            self.redstone_dirty.push(pos);
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

/// The saved id of a cooker's block entity.
fn cooker_id(kind: Cooker) -> &'static str {
    match kind {
        Cooker::Furnace => "minecraft:furnace",
        Cooker::Smoker => "minecraft:smoker",
        Cooker::BlastFurnace => "minecraft:blast_furnace",
        Cooker::Campfire => "minecraft:campfire",
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
