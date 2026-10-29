//! Potions: what each one does, and what a brewing stand makes of them. The effects and
//! the brewing recipes are part of the game code rather than the generated data, so
//! they are written out here from the Minecraft Wiki's tables.

use levitas_registry::{mob_effect, potion, Effect, ItemInfo};

use crate::inventory::Stack;

/// Ticks a brewing stand takes over one brew.
pub const BREW_TICKS: i32 = 400;
/// Brews one blaze powder fuels.
pub const FUEL_USES: i32 = 20;

/// A potion's ID from its name, with or without the `minecraft:` prefix.
pub fn id(name: &str) -> Option<u8> {
    let full = if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    };
    potion::NAMES
        .iter()
        .position(|n| *n == full)
        .and_then(|i| u8::try_from(i).ok())
}

pub fn name(id: u8) -> Option<&'static str> {
    potion::NAMES.get(usize::from(id)).copied()
}

/// What drinking a potion does, by its short name.
fn table(short: &str) -> &'static [(i32, i32, u8)] {
    use mob_effect::*;
    const MIN: i32 = 1200;
    match short {
        "night_vision" => &[(NIGHT_VISION, 3 * MIN, 0)],
        "long_night_vision" => &[(NIGHT_VISION, 8 * MIN, 0)],
        "invisibility" => &[(INVISIBILITY, 3 * MIN, 0)],
        "long_invisibility" => &[(INVISIBILITY, 8 * MIN, 0)],
        "leaping" => &[(JUMP_BOOST, 3 * MIN, 0)],
        "long_leaping" => &[(JUMP_BOOST, 8 * MIN, 0)],
        "strong_leaping" => &[(JUMP_BOOST, 3 * MIN / 2, 1)],
        "fire_resistance" => &[(FIRE_RESISTANCE, 3 * MIN, 0)],
        "long_fire_resistance" => &[(FIRE_RESISTANCE, 8 * MIN, 0)],
        "swiftness" => &[(SPEED, 3 * MIN, 0)],
        "long_swiftness" => &[(SPEED, 8 * MIN, 0)],
        "strong_swiftness" => &[(SPEED, 3 * MIN / 2, 1)],
        "slowness" => &[(SLOWNESS, 3 * MIN / 2, 0)],
        "long_slowness" => &[(SLOWNESS, 4 * MIN, 0)],
        "strong_slowness" => &[(SLOWNESS, 400, 3)],
        "turtle_master" => &[(SLOWNESS, 400, 3), (RESISTANCE, 400, 2)],
        "long_turtle_master" => &[(SLOWNESS, 800, 3), (RESISTANCE, 800, 2)],
        "strong_turtle_master" => &[(SLOWNESS, 400, 5), (RESISTANCE, 400, 3)],
        "water_breathing" => &[(WATER_BREATHING, 3 * MIN, 0)],
        "long_water_breathing" => &[(WATER_BREATHING, 8 * MIN, 0)],
        "healing" => &[(INSTANT_HEALTH, 1, 0)],
        "strong_healing" => &[(INSTANT_HEALTH, 1, 1)],
        "harming" => &[(INSTANT_DAMAGE, 1, 0)],
        "strong_harming" => &[(INSTANT_DAMAGE, 1, 1)],
        "poison" => &[(POISON, 900, 0)],
        "long_poison" => &[(POISON, 1800, 0)],
        "strong_poison" => &[(POISON, 432, 1)],
        "regeneration" => &[(REGENERATION, 900, 0)],
        "long_regeneration" => &[(REGENERATION, 1800, 0)],
        "strong_regeneration" => &[(REGENERATION, 450, 1)],
        "strength" => &[(STRENGTH, 3 * MIN, 0)],
        "long_strength" => &[(STRENGTH, 8 * MIN, 0)],
        "strong_strength" => &[(STRENGTH, 3 * MIN / 2, 1)],
        "weakness" => &[(WEAKNESS, 3 * MIN / 2, 0)],
        "long_weakness" => &[(WEAKNESS, 4 * MIN, 0)],
        "luck" => &[(LUCK, 5 * MIN, 0)],
        "slow_falling" => &[(SLOW_FALLING, 3 * MIN / 2, 0)],
        "long_slow_falling" => &[(SLOW_FALLING, 4 * MIN, 0)],
        "wind_charged" => &[(WIND_CHARGED, 3 * MIN, 0)],
        "weaving" => &[(WEAVING, 3 * MIN, 0)],
        "oozing" => &[(OOZING, 3 * MIN, 0)],
        "infested" => &[(INFESTED, 3 * MIN, 0)],
        _ => &[], // water, mundane, thick, awkward
    }
}

/// The effects of potion `id`.
pub fn effects(id: u8) -> Vec<Effect> {
    let short = name(id).map_or("", |n| n.trim_start_matches("minecraft:"));
    table(short)
        .iter()
        .map(|&(id, duration, amplifier)| Effect {
            id,
            duration,
            amplifier,
        })
        .collect()
}

/// Whether an effect acts at once (healing, harming) rather than over time.
pub fn is_instant(effect: i32) -> bool {
    effect == mob_effect::INSTANT_HEALTH || effect == mob_effect::INSTANT_DAMAGE
}

/// Brewing recipes: (potion, ingredient item, result), by short names.
const MIXES: &[(&str, &str, &str)] = &[
    // From water.
    ("water", "nether_wart", "awkward"),
    ("water", "glowstone_dust", "thick"),
    ("water", "redstone", "mundane"),
    ("water", "sugar", "mundane"),
    ("water", "rabbit_foot", "mundane"),
    ("water", "glistering_melon_slice", "mundane"),
    ("water", "spider_eye", "mundane"),
    ("water", "magma_cream", "mundane"),
    ("water", "blaze_powder", "mundane"),
    ("water", "ghast_tear", "mundane"),
    ("water", "fermented_spider_eye", "weakness"),
    // From an awkward potion.
    ("awkward", "golden_carrot", "night_vision"),
    ("awkward", "magma_cream", "fire_resistance"),
    ("awkward", "rabbit_foot", "leaping"),
    ("awkward", "sugar", "swiftness"),
    ("awkward", "pufferfish", "water_breathing"),
    ("awkward", "glistering_melon_slice", "healing"),
    ("awkward", "spider_eye", "poison"),
    ("awkward", "ghast_tear", "regeneration"),
    ("awkward", "blaze_powder", "strength"),
    ("awkward", "turtle_helmet", "turtle_master"),
    ("awkward", "phantom_membrane", "slow_falling"),
    ("awkward", "breeze_rod", "wind_charged"),
    ("awkward", "cobweb", "weaving"),
    ("awkward", "slime_block", "oozing"),
    ("awkward", "stone", "infested"),
    // Fermented spider eyes turn effects around.
    ("night_vision", "fermented_spider_eye", "invisibility"),
    (
        "long_night_vision",
        "fermented_spider_eye",
        "long_invisibility",
    ),
    ("leaping", "fermented_spider_eye", "slowness"),
    ("long_leaping", "fermented_spider_eye", "long_slowness"),
    ("swiftness", "fermented_spider_eye", "slowness"),
    ("long_swiftness", "fermented_spider_eye", "long_slowness"),
    ("healing", "fermented_spider_eye", "harming"),
    ("strong_healing", "fermented_spider_eye", "strong_harming"),
    ("poison", "fermented_spider_eye", "harming"),
    ("long_poison", "fermented_spider_eye", "harming"),
    ("strong_poison", "fermented_spider_eye", "strong_harming"),
];

/// Potions redstone makes last longer.
const EXTENDABLE: &[&str] = &[
    "night_vision",
    "invisibility",
    "leaping",
    "fire_resistance",
    "swiftness",
    "slowness",
    "turtle_master",
    "water_breathing",
    "poison",
    "regeneration",
    "strength",
    "weakness",
    "slow_falling",
];

/// Potions glowstone makes stronger.
const AMPLIFIABLE: &[&str] = &[
    "leaping",
    "swiftness",
    "slowness",
    "turtle_master",
    "healing",
    "harming",
    "poison",
    "regeneration",
    "strength",
];

fn item_name(stack: Stack) -> &'static str {
    ItemInfo::get(stack.item).map_or("", |i| i.name)
}

/// Whether a stack is a potion of any kind: drunk, splash or lingering.
pub fn is_potion(stack: Stack) -> bool {
    matches!(
        item_name(stack),
        "minecraft:potion" | "minecraft:splash_potion" | "minecraft:lingering_potion"
    )
}

/// Whether an item does anything in a brewing stand's ingredient slot.
pub fn is_ingredient(stack: Stack) -> bool {
    let short = item_name(stack).trim_start_matches("minecraft:");
    matches!(
        short,
        "redstone" | "glowstone_dust" | "gunpowder" | "dragon_breath"
    ) || MIXES.iter().any(|&(_, i, _)| i == short)
}

/// What `ingredient` brews `bottle` into, if anything.
pub fn brew(ingredient: Stack, bottle: Stack) -> Option<Stack> {
    if ingredient.is_empty() || !is_potion(bottle) {
        return None;
    }
    let kind = item_name(bottle);
    let ingredient = item_name(ingredient).trim_start_matches("minecraft:");
    // Gunpowder makes a potion splash; dragon's breath makes a splash potion linger.
    let becomes = |item: &str| Some(ItemInfo::id_of(item)?).map(|id| Stack { item: id, ..bottle });
    match (ingredient, kind) {
        ("gunpowder", "minecraft:potion") => return becomes("minecraft:splash_potion"),
        ("dragon_breath", "minecraft:splash_potion") => {
            return becomes("minecraft:lingering_potion")
        }
        ("gunpowder" | "dragon_breath", _) => return None,
        _ => {}
    }
    let current = name(bottle.potion?)?.trim_start_matches("minecraft:");
    let result = MIXES
        .iter()
        .find(|&&(from, i, _)| from == current && i == ingredient)
        .map(|&(_, _, to)| to.to_owned())
        .or_else(|| match ingredient {
            "redstone" if EXTENDABLE.contains(&current) => Some(format!("long_{current}")),
            "glowstone_dust" if AMPLIFIABLE.contains(&current) => Some(format!("strong_{current}")),
            _ => None,
        })?;
    Some(Stack {
        potion: Some(id(&result)?),
        ..bottle
    })
}

/// A potion item of the given kind holding potion `name`.
pub fn stack(item: &str, name: &str) -> Stack {
    let mut s = Stack::new(ItemInfo::id_of(item).unwrap_or(0), 1);
    s.potion = id(name);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn potion(name: &str) -> Stack {
        stack("minecraft:potion", name)
    }

    fn ingredient(name: &str) -> Stack {
        Stack::new(ItemInfo::id_of(&format!("minecraft:{name}")).unwrap(), 1)
    }

    #[test]
    fn every_potion_name_is_known() {
        for (from, i, to) in MIXES {
            assert!(id(from).is_some() && id(to).is_some(), "{from} {to}");
            assert!(ItemInfo::id_of(&format!("minecraft:{i}")).is_some(), "{i}");
        }
        for p in EXTENDABLE {
            assert!(id(&format!("long_{p}")).is_some(), "long_{p}");
        }
        for p in AMPLIFIABLE {
            assert!(id(&format!("strong_{p}")).is_some(), "strong_{p}");
        }
    }

    #[test]
    fn brewing_a_strength_potion_step_by_step() {
        let awkward = brew(ingredient("nether_wart"), potion("water")).unwrap();
        assert_eq!(awkward, potion("awkward"));
        let strength = brew(ingredient("blaze_powder"), awkward).unwrap();
        assert_eq!(strength, potion("strength"));
        let strong = brew(ingredient("glowstone_dust"), strength).unwrap();
        assert_eq!(strong, potion("strong_strength"));
        assert_eq!(effects(strong.potion.unwrap())[0].amplifier, 1);
        assert_eq!(
            brew(ingredient("redstone"), strength).unwrap(),
            potion("long_strength")
        );
        // Long strength cannot also be made strong.
        assert!(brew(ingredient("glowstone_dust"), potion("long_strength")).is_none());
        let splash = brew(ingredient("gunpowder"), strong).unwrap();
        assert_eq!(splash, stack("minecraft:splash_potion", "strong_strength"));
        let lingering = brew(ingredient("dragon_breath"), splash).unwrap();
        assert_eq!(
            lingering,
            stack("minecraft:lingering_potion", "strong_strength")
        );
        assert!(brew(ingredient("dirt"), potion("water")).is_none());
        assert!(is_ingredient(ingredient("nether_wart")));
        assert!(!is_ingredient(ingredient("dirt")));
    }

    #[test]
    fn potions_survive_saving() {
        let p = stack("minecraft:splash_potion", "long_swiftness");
        assert_eq!(Stack::from_nbt(&p.to_nbt()), p);
        assert_ne!(p, stack("minecraft:splash_potion", "swiftness"));
    }
}
