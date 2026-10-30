//! Crafting and smelting recipes, loaded from the vanilla recipe JSON. Shaped (including
//! mirrored), shapeless, smelting, stonecutting and smithing recipes are supported;
//! special recipes (fireworks, map cloning, dyeing, ...) and transmute recipes are not yet.

use std::collections::HashMap;

use levitas_protocol::Writer;
use levitas_registry::packet::play::clientbound;
use levitas_registry::{slot_display, ItemInfo, TagRegistry, RECIPES_JSON};
use serde_json::Value;

use crate::inventory::{write_template, Stack, Trim};

/// Item IDs an ingredient slot accepts, sorted.
type Ingredient = Vec<i32>;

/// A crafting recipe that turns one item into another, keeping its components: dyeing
/// a shulker box or a bundle (`crafting_transmute`).
struct Transmute {
    input: Ingredient,
    material: Ingredient,
    result: i32,
}

/// Dyeing leather armor in the crafting grid (`crafting_dye`).
struct Dyeing {
    target: Ingredient,
    dye: Ingredient,
}

/// A source item ringed by eight of a material, which take on its potion: tipped arrows
/// from a lingering potion (`crafting_imbue`).
struct Imbue {
    source: Ingredient,
    material: Ingredient,
    result: Stack,
}

/// Picks one part of a smithing recipe.
type Part = fn(&Smithing) -> &Ingredient;

/// A smithing table recipe.
struct Smithing {
    template: Ingredient,
    base: Ingredient,
    addition: Ingredient,
    outcome: Outcome,
}

enum Outcome {
    /// The base becomes this item (netherite upgrades).
    Becomes(i32),
    /// The base armor gets a trim of this pattern, in the addition's material.
    Trim(String),
}

struct Shaped {
    width: usize,
    height: usize,
    /// Row-major cells; `None` must be empty.
    cells: Vec<Option<Ingredient>>,
    result: Stack,
}

struct Shapeless {
    ingredients: Vec<Ingredient>,
    result: Stack,
}

#[derive(Clone)]
pub struct Smelting {
    pub result: Stack,
    pub cook_time: i32,
}

/// The blocks that cook: furnaces cook everything, smokers food and blast furnaces ores
/// and metal, both twice as fast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Cooker {
    #[default]
    Furnace,
    Smoker,
    BlastFurnace,
    /// Campfires cook food slowly, without fuel.
    Campfire,
}

pub struct Recipes {
    shaped: Vec<Shaped>,
    shapeless: Vec<Shapeless>,
    /// Cooking recipes by cooker and input item.
    cooking: HashMap<(Cooker, i32), Smelting>,
    /// Stonecutter recipes, in the order clients list them: what goes in, what comes out.
    stonecutting: Vec<(Ingredient, Stack)>,
    smithing: Vec<Smithing>,
    transmute: Vec<Transmute>,
    dyeing: Vec<Dyeing>,
    imbue: Vec<Imbue>,
    /// Whether two worn items of a kind can be combined in the grid
    /// (`crafting_special_repairitem`).
    repair: bool,
}

impl Recipes {
    pub fn load() -> Self {
        let all: serde_json::Map<String, Value> =
            serde_json::from_str(RECIPES_JSON).expect("recipes are valid JSON");
        let mut recipes = Recipes {
            shaped: Vec::new(),
            shapeless: Vec::new(),
            cooking: HashMap::new(),
            stonecutting: Vec::new(),
            smithing: Vec::new(),
            transmute: Vec::new(),
            dyeing: Vec::new(),
            imbue: Vec::new(),
            repair: false,
        };
        for recipe in all.values() {
            // Recipes naming unknown items or tags are skipped rather than half-loaded.
            let _ = recipes.add(recipe);
        }
        recipes
    }

    fn add(&mut self, recipe: &Value) -> Option<()> {
        match recipe["type"].as_str()? {
            "minecraft:crafting_shaped" => {
                let pattern: Vec<&str> = recipe["pattern"]
                    .as_array()?
                    .iter()
                    .map(|row| row.as_str())
                    .collect::<Option<_>>()?;
                let key = recipe["key"].as_object()?;
                let width = pattern.iter().map(|r| r.chars().count()).max()?;
                let mut cells = Vec::new();
                for row in &pattern {
                    let mut chars: Vec<char> = row.chars().collect();
                    chars.resize(width, ' ');
                    for c in chars {
                        cells.push(match c {
                            ' ' => None,
                            c => Some(ingredient(key.get(&c.to_string())?)?),
                        });
                    }
                }
                self.shaped.push(Shaped {
                    width,
                    height: pattern.len(),
                    cells,
                    result: result(&recipe["result"])?,
                });
            }
            "minecraft:crafting_transmute" => self.transmute.push(Transmute {
                input: ingredient(&recipe["input"])?,
                material: ingredient(&recipe["material"])?,
                result: ItemInfo::id_of(recipe["result"]["id"].as_str()?)?,
            }),
            "minecraft:crafting_dye" => self.dyeing.push(Dyeing {
                target: ingredient(&recipe["target"])?,
                dye: ingredient(&recipe["dye"])?,
            }),
            "minecraft:crafting_imbue" => self.imbue.push(Imbue {
                source: ingredient(&recipe["source"])?,
                material: ingredient(&recipe["material"])?,
                result: result(&recipe["result"])?,
            }),
            "minecraft:crafting_special_repairitem" => self.repair = true,
            "minecraft:crafting_shapeless" => {
                let ingredients = recipe["ingredients"]
                    .as_array()?
                    .iter()
                    .map(ingredient)
                    .collect::<Option<_>>()?;
                self.shapeless.push(Shapeless {
                    ingredients,
                    result: result(&recipe["result"])?,
                });
            }
            kind @ ("minecraft:smelting"
            | "minecraft:smoking"
            | "minecraft:blasting"
            | "minecraft:campfire_cooking") => {
                // Since 26.x a recipe gives a furnace's time; smokers and blast
                // furnaces work twice as fast.
                let (cooker, speed) = match kind {
                    "minecraft:smoking" => (Cooker::Smoker, 2),
                    "minecraft:blasting" => (Cooker::BlastFurnace, 2),
                    "minecraft:campfire_cooking" => (Cooker::Campfire, 1),
                    _ => (Cooker::Furnace, 1),
                };
                let default_time = if cooker == Cooker::Campfire { 600 } else { 200 };
                let time = recipe["cookingtime"].as_i64().unwrap_or(default_time) as i32;
                let smelting = Smelting {
                    result: result(&recipe["result"])?,
                    cook_time: (time / speed).max(1),
                };
                for item in ingredient(&recipe["ingredient"])? {
                    self.cooking
                        .entry((cooker, item))
                        .or_insert(smelting.clone());
                }
            }
            "minecraft:stonecutting" => {
                let input = ingredient(&recipe["ingredient"])?;
                self.stonecutting.push((input, result(&recipe["result"])?));
            }
            kind @ ("minecraft:smithing_transform" | "minecraft:smithing_trim") => {
                let outcome = if kind == "minecraft:smithing_transform" {
                    Outcome::Becomes(ItemInfo::id_of(recipe["result"]["id"].as_str()?)?)
                } else {
                    Outcome::Trim(recipe["pattern"].as_str()?.to_owned())
                };
                self.smithing.push(Smithing {
                    template: ingredient(&recipe["template"])?,
                    base: ingredient(&recipe["base"])?,
                    addition: ingredient(&recipe["addition"])?,
                    outcome,
                });
            }
            _ => {}
        }
        Some(())
    }

    /// The result of a square crafting grid (`size` 2 or 3, row-major), or an empty stack.
    pub fn craft(&self, grid: &[Stack], size: usize) -> Stack {
        let items: Vec<Option<i32>> = grid
            .iter()
            .map(|s| (!s.is_empty()).then_some(s.item))
            .collect();
        let Some(bounds) = Bounds::of(&items, size) else {
            return Stack::EMPTY;
        };
        for recipe in &self.shaped {
            if recipe.width == bounds.width()
                && recipe.height == bounds.height()
                && (recipe.matches(&items, size, &bounds, false)
                    || recipe.matches(&items, size, &bounds, true))
            {
                return recipe.result;
            }
        }
        let present: Vec<i32> = items.iter().flatten().copied().collect();
        for recipe in &self.shapeless {
            if recipe.ingredients.len() == present.len()
                && assign(
                    &recipe.ingredients,
                    &present,
                    &mut vec![false; present.len()],
                )
            {
                return recipe.result;
            }
        }
        self.craft_special(grid, size)
    }

    /// Recipes whose result depends on the stacks themselves, not just their items.
    fn craft_special(&self, grid: &[Stack], size: usize) -> Stack {
        let stacks: Vec<Stack> = grid.iter().copied().filter(|s| !s.is_empty()).collect();
        let has = |ingredient: &Ingredient, item: i32| ingredient.binary_search(&item).is_ok();
        if let [a, b] = stacks[..] {
            // Transmuting: the input becomes the result, components and all.
            for t in &self.transmute {
                for (input, material) in [(a, b), (b, a)] {
                    if has(&t.input, input.item)
                        && has(&t.material, material.item)
                        && input.item != t.result
                    {
                        return Stack {
                            item: t.result,
                            count: 1,
                            ..input
                        };
                    }
                }
            }
            if self.repair {
                let repaired = repair(a, b);
                if !repaired.is_empty() {
                    return repaired;
                }
            }
        }
        // Dyeing: one piece of armor and one or more dyes.
        for d in &self.dyeing {
            let (targets, rest): (Vec<Stack>, Vec<Stack>) =
                stacks.iter().partition(|s| has(&d.target, s.item));
            if let [target] = targets[..] {
                let colors: Option<Vec<i32>> = rest
                    .iter()
                    .map(|s| has(&d.dye, s.item).then(|| dye_color(s.item)).flatten())
                    .collect();
                if let Some(colors) = colors.filter(|c| !c.is_empty()) {
                    return Stack {
                        count: 1,
                        color: Some(mix_colors(target.color, &colors)),
                        ..target
                    };
                }
            }
        }
        // Imbuing: the source in the middle of a full 3x3 grid of the material.
        if size == 3 && stacks.len() == 9 {
            for i in &self.imbue {
                let centre = grid[4];
                let ringed = grid
                    .iter()
                    .enumerate()
                    .all(|(slot, s)| slot == 4 || has(&i.material, s.item));
                if ringed && has(&i.source, centre.item) {
                    return Stack {
                        potion: centre.potion,
                        ..i.result
                    };
                }
            }
        }
        Stack::EMPTY
    }

    /// What a stonecutter can make of `item`, in the order its screen lists them.
    pub fn stonecutting(&self, item: i32) -> Vec<Stack> {
        self.stonecutting
            .iter()
            .filter(|(input, _)| input.contains(&item))
            .map(|&(_, result)| result)
            .collect()
    }

    /// What a smithing table makes of a template, a base item and an addition: the base
    /// upgraded (diamond gear to netherite) or trimmed, keeping its enchantments and
    /// wear. Empty if no recipe fits, or the armor already has that very trim.
    pub fn smithing(&self, template: Stack, base: Stack, addition: Stack) -> Stack {
        let fits = |list: &Ingredient, s: Stack| !s.is_empty() && list.contains(&s.item);
        let Some(recipe) = self.smithing.iter().find(|r| {
            fits(&r.template, template) && fits(&r.base, base) && fits(&r.addition, addition)
        }) else {
            return Stack::EMPTY;
        };
        let mut result = base.with_count(1);
        match &recipe.outcome {
            Outcome::Becomes(item) => result.item = *item,
            Outcome::Trim(pattern) => {
                let material = ItemInfo::get(addition.item).and_then(|i| i.trim_material);
                let trim = material.and_then(|m| Trim::named(m, pattern));
                if trim.is_none() || trim == base.trim {
                    return Stack::EMPTY;
                }
                result.trim = trim;
            }
        }
        result
    }

    /// Which smithing slot (0 template, 1 base, 2 addition) an item may go in first.
    pub fn smithing_slot(&self, item: i32) -> Option<usize> {
        let in_any = |part: Part| self.smithing.iter().any(|r| part(r).contains(&item));
        if in_any(|r| &r.template) {
            Some(0)
        } else if in_any(|r| &r.base) {
            Some(1)
        } else if in_any(|r| &r.addition) {
            Some(2)
        } else {
            None
        }
    }

    /// Every item that fits a part of some smithing recipe, sorted.
    fn smithing_items(&self, part: Part) -> Vec<i32> {
        let mut items: Vec<i32> = self.smithing.iter().flat_map(|r| part(r).clone()).collect();
        items.sort_unstable();
        items.dedup();
        items
    }

    /// The recipe data clients need for their own screens: which items each cooker
    /// and smithing slot takes, and every stonecutter recipe.
    pub fn update_packet(&self) -> Writer {
        let sets = [
            ("minecraft:furnace_input", Cooker::Furnace),
            ("minecraft:smoker_input", Cooker::Smoker),
            ("minecraft:blast_furnace_input", Cooker::BlastFurnace),
            ("minecraft:campfire_input", Cooker::Campfire),
        ];
        let smithing: [(&str, Part); 3] = [
            ("minecraft:smithing_template", |r| &r.template),
            ("minecraft:smithing_base", |r| &r.base),
            ("minecraft:smithing_addition", |r| &r.addition),
        ];
        // What a brewing stand's bottle and ingredient slots take.
        let ids = |names: &[String]| -> Vec<i32> {
            let mut ids: Vec<i32> = names.iter().filter_map(|n| ItemInfo::id_of(n)).collect();
            ids.sort_unstable();
            ids
        };
        let bottles: Vec<String> = ["potion", "splash_potion", "lingering_potion"]
            .iter()
            .map(|n| format!("minecraft:{n}"))
            .collect();
        let brewing = [
            ("minecraft:brewing_input", ids(&bottles)),
            (
                "minecraft:brewing_reagent",
                ids(&crate::potions::reagents()),
            ),
        ];
        let mut packet = Writer::packet(clientbound::UPDATE_RECIPES)
            .varint((sets.len() + smithing.len() + brewing.len()) as i32);
        let write_set = |packet: Writer, name: &str, items: Vec<i32>| {
            let mut packet = packet.string(name).varint(items.len() as i32);
            for item in items {
                packet = packet.varint(item);
            }
            packet
        };
        for (name, cooker) in sets {
            let mut items: Vec<i32> = self
                .cooking
                .keys()
                .filter(|(c, _)| *c == cooker)
                .map(|&(_, item)| item)
                .collect();
            items.sort_unstable();
            packet = write_set(packet, name, items);
        }
        for (name, part) in smithing {
            packet = write_set(packet, name, self.smithing_items(part));
        }
        for (name, items) in brewing {
            packet = write_set(packet, name, items);
        }
        packet = packet.varint(self.stonecutting.len() as i32);
        for (input, result) in &self.stonecutting {
            // A holder set listed directly: its size plus one, then the items.
            packet = packet.varint(input.len() as i32 + 1);
            for &item in input {
                packet = packet.varint(item);
            }
            packet = write_template(packet.varint(slot_display::ITEM_STACK), *result);
        }
        packet
    }

    /// What `cooker` makes of `item`.
    pub fn cooking(&self, cooker: Cooker, item: i32) -> Option<&Smelting> {
        self.cooking.get(&(cooker, item))
    }
}

impl Shaped {
    fn matches(&self, items: &[Option<i32>], size: usize, b: &Bounds, mirrored: bool) -> bool {
        (0..self.height).all(|row| {
            (0..self.width).all(|col| {
                let pattern_col = if mirrored { self.width - 1 - col } else { col };
                let cell = &self.cells[row * self.width + pattern_col];
                let item = items[(b.top + row) * size + b.left + col];
                match (cell, item) {
                    (None, None) => true,
                    (Some(ingredient), Some(item)) => ingredient.binary_search(&item).is_ok(),
                    _ => false,
                }
            })
        })
    }
}

/// The smallest rectangle containing every non-empty grid cell.
struct Bounds {
    top: usize,
    left: usize,
    bottom: usize,
    right: usize,
}

impl Bounds {
    fn of(items: &[Option<i32>], size: usize) -> Option<Self> {
        let filled: Vec<(usize, usize)> = (0..items.len())
            .filter(|&i| items[i].is_some())
            .map(|i| (i / size, i % size))
            .collect();
        Some(Bounds {
            top: filled.iter().map(|c| c.0).min()?,
            left: filled.iter().map(|c| c.1).min()?,
            bottom: filled.iter().map(|c| c.0).max()?,
            right: filled.iter().map(|c| c.1).max()?,
        })
    }

    fn width(&self) -> usize {
        self.right - self.left + 1
    }

    fn height(&self) -> usize {
        self.bottom - self.top + 1
    }
}

/// Whether every ingredient can be matched to a distinct item (simple backtracking).
fn assign(ingredients: &[Ingredient], items: &[i32], used: &mut [bool]) -> bool {
    let Some((first, rest)) = ingredients.split_first() else {
        return true;
    };
    for i in 0..items.len() {
        if !used[i] && first.binary_search(&items[i]).is_ok() {
            used[i] = true;
            if assign(rest, items, used) {
                return true;
            }
            used[i] = false;
        }
    }
    false
}

/// An ingredient: an item ID, a `#tag`, or a list of those.
fn ingredient(value: &Value) -> Option<Ingredient> {
    let mut items = Vec::new();
    let specs: Vec<&str> = match value {
        Value::String(s) => vec![s.as_str()],
        Value::Array(list) => list.iter().map(Value::as_str).collect::<Option<_>>()?,
        _ => return None,
    };
    for spec in specs {
        match spec.strip_prefix('#') {
            Some(tag) => {
                let members = TagRegistry::get("minecraft:item")?.members(tag);
                if members.is_empty() {
                    return None;
                }
                items.extend(members.iter().map(|&id| id as i32));
            }
            None => items.push(ItemInfo::id_of(spec)?),
        }
    }
    items.sort_unstable();
    items.dedup();
    Some(items)
}

/// Two worn items of the same kind made into one: their uses left added up, plus 5% of
/// the item's durability, up to new. Only curses stay on it. Empty if they do not combine.
fn repair(a: Stack, b: Stack) -> Stack {
    let Some(info) = ItemInfo::get(a.item) else {
        return Stack::EMPTY;
    };
    let max = info.max_damage;
    if a.item != b.item || max <= 0 || a.count != 1 || b.count != 1 {
        return Stack::EMPTY;
    }
    let uses = (max - a.damage) + (max - b.damage) + max / 20;
    let mut repaired = Stack::new(a.item, 1);
    repaired.damage = (max - uses).max(0);
    for (id, level) in a.enchants.iter().chain(b.enchants.iter()) {
        if crate::enchanting::definition(id).is_some_and(|d| d.curse) {
            repaired
                .enchants
                .set(id, level.max(repaired.enchants.level(id)));
        }
    }
    repaired
}

/// The colour a dye gives leather armor, `0xRRGGBB` (from the Minecraft Wiki's table).
fn dye_color(item: i32) -> Option<i32> {
    let name = ItemInfo::get(item)?.name.strip_prefix("minecraft:")?;
    Some(match name.strip_suffix("_dye")? {
        "white" => 0xF9FFFE,
        "orange" => 0xF9801D,
        "magenta" => 0xC74EBD,
        "light_blue" => 0x3AB3DA,
        "yellow" => 0xFED83D,
        "lime" => 0x80C71F,
        "pink" => 0xF38BAA,
        "gray" => 0x474F52,
        "light_gray" => 0x9D9D97,
        "cyan" => 0x169C9C,
        "purple" => 0x8932B8,
        "blue" => 0x3C44AA,
        "brown" => 0x835432,
        "green" => 0x5E7C16,
        "red" => 0xB02E26,
        "black" => 0x1D1D21,
        _ => return None,
    })
}

/// Mixes the armor's colour (if it has one) with the dyes, as the wiki describes: the
/// average colour, brightened so that its brightest component is the average of each
/// colour's brightest component.
fn mix_colors(existing: Option<i32>, dyes: &[i32]) -> i32 {
    let rgb = |c: i32| [(c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF];
    let mut sum = [0; 3];
    let mut brightest = 0;
    let mut n = 0;
    for color in existing.iter().chain(dyes) {
        let c = rgb(*color);
        for i in 0..3 {
            sum[i] += c[i];
        }
        brightest += c.into_iter().max().unwrap_or(0);
        n += 1;
    }
    let average = sum.map(|s| s / n);
    let average_brightest = brightest / n;
    let top = average.into_iter().max().unwrap_or(0).max(1);
    let [r, g, b] = average.map(|c| (c as f32 * average_brightest as f32 / top as f32) as i32);
    (r << 16) | (g << 8) | b
}

fn result(value: &Value) -> Option<Stack> {
    let item = ItemInfo::id_of(value["id"].as_str()?)?;
    let count = value["count"].as_i64().unwrap_or(1) as i32;
    Some(Stack::new(item, count))
}

/// Burn time in ticks of an item used as furnace fuel, or 0. Fuel values are part of the
/// game code rather than the generated data, so the common ones are listed here.
pub fn fuel_ticks(item: i32) -> i32 {
    let Some(info) = ItemInfo::get(item) else {
        return 0;
    };
    let in_tag = |tag: &str| {
        TagRegistry::get("minecraft:item")
            .is_some_and(|t| t.members(tag).binary_search(&i64::from(item)).is_ok())
    };
    match info.name {
        "minecraft:lava_bucket" => 20_000,
        "minecraft:coal_block" => 16_000,
        "minecraft:dried_kelp_block" => 4_001,
        "minecraft:blaze_rod" => 2_400,
        "minecraft:coal" | "minecraft:charcoal" => 1_600,
        "minecraft:crafting_table" | "minecraft:chest" | "minecraft:barrel" => 300,
        "minecraft:stick" | "minecraft:bowl" => 100,
        "minecraft:bamboo" | "minecraft:scaffolding" => 50,
        name if name.starts_with("minecraft:wooden_") => 200,
        _ if in_tag("minecraft:logs") || in_tag("minecraft:planks") => 300,
        _ if in_tag("minecraft:wooden_stairs") || in_tag("minecraft:wooden_fences") => 300,
        _ if in_tag("minecraft:wooden_slabs") => 150,
        _ if in_tag("minecraft:saplings") || in_tag("minecraft:wool") => 100,
        _ if in_tag("minecraft:wool_carpets") => 67,
        _ => 0,
    }
}

/// What is left in the fuel slot after burning one of `item` (a lava bucket leaves a
/// bucket).
pub fn fuel_remainder(item: i32) -> Option<i32> {
    let name = ItemInfo::get(item)?.name;
    (name == "minecraft:lava_bucket").then(|| ItemInfo::id_of("minecraft:bucket"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> i32 {
        ItemInfo::id_of(name).unwrap()
    }

    fn grid(size: usize, cells: &[(usize, &str)]) -> Vec<Stack> {
        let mut g = vec![Stack::EMPTY; size * size];
        for &(i, name) in cells {
            g[i] = Stack::new(item(name), 1);
        }
        g
    }

    #[test]
    fn shapeless_planks_from_any_slot() {
        let r = Recipes::load();
        let planks = Stack::new(item("minecraft:oak_planks"), 4);
        assert_eq!(r.craft(&grid(2, &[(3, "minecraft:oak_log")]), 2), planks);
        assert_eq!(r.craft(&grid(3, &[(4, "minecraft:oak_log")]), 3), planks);
    }

    #[test]
    fn shaped_recipes_fit_anywhere_in_the_grid() {
        let r = Recipes::load();
        let sticks = Stack::new(item("minecraft:stick"), 4);
        assert_eq!(
            r.craft(
                &grid(
                    2,
                    &[(1, "minecraft:oak_planks"), (3, "minecraft:birch_planks")]
                ),
                2
            ),
            sticks
        );
        assert_eq!(
            r.craft(
                &grid(
                    3,
                    &[(2, "minecraft:oak_planks"), (5, "minecraft:oak_planks")]
                ),
                3
            ),
            sticks
        );
        let table = r.craft(
            &grid(
                2,
                &[
                    (0, "minecraft:oak_planks"),
                    (1, "minecraft:oak_planks"),
                    (2, "minecraft:oak_planks"),
                    (3, "minecraft:oak_planks"),
                ],
            ),
            2,
        );
        assert_eq!(table, Stack::new(item("minecraft:crafting_table"), 1));
    }

    #[test]
    fn three_by_three_and_mirrored() {
        let r = Recipes::load();
        let p = "minecraft:oak_planks";
        let s = "minecraft:stick";
        let pickaxe = grid(3, &[(0, p), (1, p), (2, p), (4, s), (7, s)]);
        assert_eq!(
            r.craft(&pickaxe, 3),
            Stack::new(item("minecraft:wooden_pickaxe"), 1)
        );
        // The axe pattern and its mirror image both work.
        let axe = grid(3, &[(0, p), (1, p), (3, p), (4, s), (7, s)]);
        let mirrored = grid(3, &[(1, p), (2, p), (5, p), (4, s), (7, s)]);
        let wooden_axe = Stack::new(item("minecraft:wooden_axe"), 1);
        assert_eq!(r.craft(&axe, 3), wooden_axe);
        assert_eq!(r.craft(&mirrored, 3), wooden_axe);
        assert!(r.craft(&grid(3, &[(0, p), (8, s)]), 3).is_empty());
    }

    #[test]
    fn smelting_and_fuel() {
        let r = Recipes::load();
        let iron = r
            .cooking(Cooker::Furnace, item("minecraft:raw_iron"))
            .unwrap();
        assert_eq!(iron.result, Stack::new(item("minecraft:iron_ingot"), 1));
        assert_eq!(iron.cook_time, 200);
        assert_eq!(fuel_ticks(item("minecraft:coal")), 1600);
        assert_eq!(fuel_ticks(item("minecraft:oak_planks")), 300);
        assert_eq!(fuel_ticks(item("minecraft:dirt")), 0);
        assert_eq!(
            fuel_remainder(item("minecraft:lava_bucket")),
            Some(item("minecraft:bucket"))
        );
    }

    #[test]
    fn smokers_cook_food_and_blast_furnaces_ores_twice_as_fast() {
        let r = Recipes::load();
        let beef = r.cooking(Cooker::Smoker, item("minecraft:beef")).unwrap();
        assert_eq!(beef.result.item, item("minecraft:cooked_beef"));
        assert_eq!(beef.cook_time, 100);
        assert!(r
            .cooking(Cooker::Smoker, item("minecraft:raw_iron"))
            .is_none());
        let iron = r
            .cooking(Cooker::BlastFurnace, item("minecraft:raw_iron"))
            .unwrap();
        assert_eq!(iron.cook_time, 100);
        assert!(r
            .cooking(Cooker::BlastFurnace, item("minecraft:beef"))
            .is_none());
    }

    #[test]
    fn worn_tools_combine_in_the_grid_keeping_only_curses() {
        let r = Recipes::load();
        let pickaxe = item("minecraft:iron_pickaxe");
        let mut a = Stack::new(pickaxe, 1);
        a.damage = 200;
        let unbreaking = crate::enchanting::Enchantments::from_nbt(
            &levitas_nbt::Compound::new()
                .with("minecraft:unbreaking", 3)
                .with("minecraft:vanishing_curse", 1),
        );
        a.enchants = unbreaking;
        let mut b = Stack::new(pickaxe, 1);
        b.damage = 150;
        let mut g = vec![Stack::EMPTY; 4];
        g[0] = a;
        g[3] = b;
        // 50 + 100 uses left, plus 5% of 250.
        let repaired = r.craft(&g, 2);
        assert_eq!((repaired.item, repaired.damage), (pickaxe, 250 - 162));
        assert_eq!(repaired.enchants.level_of("minecraft:vanishing_curse"), 1);
        assert_eq!(repaired.enchants.level_of("minecraft:unbreaking"), 0);
        // Different tools do not combine.
        g[3] = Stack::new(item("minecraft:stone_pickaxe"), 1);
        assert!(r.craft(&g, 2).is_empty());
    }

    #[test]
    fn leather_armor_takes_the_mix_of_its_dyes() {
        let r = Recipes::load();
        let boots = item("minecraft:leather_boots");
        let one = r.craft(
            &grid(
                2,
                &[(0, "minecraft:leather_boots"), (1, "minecraft:yellow_dye")],
            ),
            2,
        );
        assert_eq!((one.item, one.color), (boots, Some(0xFED83D)));
        // Red and yellow: their average, which already has the average brightness.
        let two = r.craft(
            &grid(
                3,
                &[
                    (0, "minecraft:leather_boots"),
                    (4, "minecraft:red_dye"),
                    (8, "minecraft:yellow_dye"),
                ],
            ),
            3,
        );
        assert_eq!(two.color, Some(0xD78331));
        // Dyed again, the old colour counts as one more dye.
        assert_eq!(mix_colors(Some(0xFED83D), &[0xFED83D]), 0xFED83D);
        // Something that is not a dye spoils it.
        assert!(r
            .craft(
                &grid(2, &[(0, "minecraft:leather_boots"), (1, "minecraft:stick")]),
                2
            )
            .is_empty());
        // The colour is kept when saved.
        assert_eq!(Stack::from_nbt(&two.to_nbt()), two);
    }

    #[test]
    fn shulker_boxes_change_colour_with_their_name_and_arrows_take_a_potion() {
        let r = Recipes::load();
        let mut boxed = Stack::new(item("minecraft:shulker_box"), 1);
        boxed.name = crate::names::intern("Ores");
        let mut g = vec![Stack::EMPTY; 4];
        g[0] = boxed;
        g[1] = Stack::new(item("minecraft:red_dye"), 1);
        let red = r.craft(&g, 2);
        assert_eq!(red.item, item("minecraft:red_shulker_box"));
        assert_eq!(red.name, boxed.name);

        let swiftness = crate::potions::id("minecraft:swiftness").unwrap();
        let mut g = vec![Stack::new(item("minecraft:arrow"), 1); 9];
        g[4] = Stack::new(item("minecraft:lingering_potion"), 1);
        g[4].potion = Some(swiftness);
        let tipped = r.craft(&g, 3);
        assert_eq!(
            (tipped.item, tipped.count, tipped.potion),
            (item("minecraft:tipped_arrow"), 8, Some(swiftness))
        );
        // Not with one arrow missing.
        g[0] = Stack::EMPTY;
        assert!(r.craft(&g, 3).is_empty());
    }

    #[test]
    fn clients_get_every_stonecutter_recipe() {
        let r = Recipes::load();
        assert!(r.stonecutting.len() > 200);
        let frame = r.update_packet().into_frame();
        assert!(frame.len() > 1000);
        let stone = item("minecraft:stone");
        assert!(r
            .stonecutting(stone)
            .iter()
            .any(|s| s.item == item("minecraft:stone_bricks")));
    }

    #[test]
    fn the_recipe_packet_is_laid_out_as_26_3_reads_it() {
        use levitas_protocol::Reader;
        let frame = Recipes::load().update_packet().into_frame();
        let mut r = Reader::new(&frame);
        r.varint().unwrap(); // length
        r.varint().unwrap(); // packet ID
        let mut sets = Vec::new();
        for _ in 0..r.varint().unwrap() {
            let name = r.string(32767).unwrap().to_owned();
            let items: Vec<i32> = (0..r.varint().unwrap())
                .map(|_| r.varint().unwrap())
                .collect();
            sets.push((name, items));
        }
        // A vanilla 26.3 server lists 21 brewing reagents and 3 kinds of bottle.
        let set = |name: &str| sets.iter().find(|(n, _)| n == name).unwrap().1.len();
        assert_eq!(set("minecraft:brewing_reagent"), 21);
        assert_eq!(set("minecraft:brewing_input"), 3);
        // Each stonecutter recipe: its input as an ID set (size + 1, then the items),
        // then an item stack template: item, count, no component changes.
        assert!(r.varint().unwrap() > 200);
        let inputs = r.varint().unwrap() - 1;
        for _ in 0..inputs {
            r.varint().unwrap();
        }
        assert_eq!(r.varint().unwrap(), slot_display::ITEM_STACK);
        let result = r.varint().unwrap();
        assert!(ItemInfo::get(result).is_some(), "item {result}");
        assert!((1..=64).contains(&r.varint().unwrap()));
        assert_eq!((r.varint().unwrap(), r.varint().unwrap()), (0, 0));
    }

    #[test]
    fn smithing_tables_trim_armor_once() {
        let r = Recipes::load();
        let template = Stack::new(item("minecraft:bolt_armor_trim_smithing_template"), 1);
        let gold = Stack::new(item("minecraft:gold_ingot"), 1);
        let armor = Stack::new(item("minecraft:iron_chestplate"), 1);
        let trimmed = r.smithing(template, armor, gold);
        let trim = trimmed.trim.expect("a trim");
        assert_eq!(trimmed.item, armor.item);
        assert_eq!(trim.material_name(), "minecraft:gold");
        assert_eq!(trim.pattern_name(), "minecraft:bolt");
        assert!(
            r.smithing(template, trimmed, gold).is_empty(),
            "already trimmed so"
        );
        let lapis = Stack::new(item("minecraft:lapis_lazuli"), 1);
        assert!(r.smithing(template, trimmed, lapis).trim.is_some());
        // Swords take no trims.
        let sword = Stack::new(item("minecraft:iron_sword"), 1);
        assert!(r.smithing(template, sword, gold).is_empty());
        // A trim survives saving.
        assert_eq!(Stack::from_nbt(&trimmed.to_nbt()), trimmed);
        assert_eq!(r.smithing_slot(template.item), Some(0));
        assert_eq!(r.smithing_slot(gold.item), Some(2));
    }
}
