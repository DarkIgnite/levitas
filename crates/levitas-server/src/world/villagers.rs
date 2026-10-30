//! Villagers and trading, following the Minecraft Wiki.
//!
//! An unemployed villager takes the profession of an unclaimed workstation near it and
//! offers that profession's novice trades. Trading earns it experience; at 10, 70, 150
//! and 250 it climbs to apprentice, journeyman, expert and master, each level adding
//! two trades picked from that level's list. Trades sell out after their uses and are
//! restocked twice a day while the villager has its workstation. A villager that never
//! traded loses its profession with its workstation. Trades are part of the game code,
//! not of the generated data, so their table is written out here from the wiki.

use levitas_nbt::{Compound, Tag};
use levitas_registry::{menu, villager_profession, villager_type, TagRegistry};
use serde_json::Value;

use super::containers::{View, MAX_WINDOW_ID};
use super::mobs::{Mob, MobKind};
use super::*;
use crate::click::Trade;
use crate::enchanting;

/// Entity data: the villager's type, profession and level. In 26.3 ageable mobs have an
/// age lock at 17, so this sits at 19; the serializers run ... 15 optional block state,
/// 16 particle, 17 particles, 18 villager data.
const DATA_VILLAGER: u8 = 19;
const SERIALIZER_VILLAGER_DATA: i32 = 18;
/// Experience a villager needs for each level above novice.
const LEVEL_XP: [i32; 5] = [0, 10, 70, 150, 250];
/// How far an unemployed villager looks for a workstation, sideways and up or down.
const SITE_REACH: i32 = 16;
const SITE_REACH_UP: i32 = 4;
/// Ticks between a villager's looks around for a workstation.
const SITE_CHECK_TICKS: u64 = 100;
/// Times of day villagers restock at their workstations: morning and afternoon.
const RESTOCK_TIMES: [i64; 2] = [2000, 9000];

/// What a villager does for a living.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profession {
    None,
    Nitwit,
    Armorer,
    Butcher,
    Cartographer,
    Cleric,
    Farmer,
    Fisherman,
    Fletcher,
    Leatherworker,
    Librarian,
    Mason,
    Shepherd,
    Toolsmith,
    Weaponsmith,
}

impl Profession {
    const WORKERS: [Profession; 13] = [
        Profession::Armorer,
        Profession::Butcher,
        Profession::Cartographer,
        Profession::Cleric,
        Profession::Farmer,
        Profession::Fisherman,
        Profession::Fletcher,
        Profession::Leatherworker,
        Profession::Librarian,
        Profession::Mason,
        Profession::Shepherd,
        Profession::Toolsmith,
        Profession::Weaponsmith,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Profession::None => "minecraft:none",
            Profession::Nitwit => "minecraft:nitwit",
            Profession::Armorer => "minecraft:armorer",
            Profession::Butcher => "minecraft:butcher",
            Profession::Cartographer => "minecraft:cartographer",
            Profession::Cleric => "minecraft:cleric",
            Profession::Farmer => "minecraft:farmer",
            Profession::Fisherman => "minecraft:fisherman",
            Profession::Fletcher => "minecraft:fletcher",
            Profession::Leatherworker => "minecraft:leatherworker",
            Profession::Librarian => "minecraft:librarian",
            Profession::Mason => "minecraft:mason",
            Profession::Shepherd => "minecraft:shepherd",
            Profession::Toolsmith => "minecraft:toolsmith",
            Profession::Weaponsmith => "minecraft:weaponsmith",
        }
    }

    pub fn from_name(name: &str) -> Profession {
        [Profession::None, Profession::Nitwit]
            .into_iter()
            .chain(Profession::WORKERS)
            .find(|p| p.name() == name)
            .unwrap_or(Profession::None)
    }

    /// The block a villager of this profession works at.
    fn workstation(self) -> &'static str {
        match self {
            Profession::Armorer => "minecraft:blast_furnace",
            Profession::Butcher => "minecraft:smoker",
            Profession::Cartographer => "minecraft:cartography_table",
            Profession::Cleric => "minecraft:brewing_stand",
            Profession::Farmer => "minecraft:composter",
            Profession::Fisherman => "minecraft:barrel",
            Profession::Fletcher => "minecraft:fletching_table",
            Profession::Leatherworker => "minecraft:cauldron",
            Profession::Librarian => "minecraft:lectern",
            Profession::Mason => "minecraft:stonecutter",
            Profession::Shepherd => "minecraft:loom",
            Profession::Toolsmith => "minecraft:smithing_table",
            Profession::Weaponsmith => "minecraft:grindstone",
            Profession::None | Profession::Nitwit => "",
        }
    }

    /// The profession a workstation block gives.
    fn of_workstation(block: &str) -> Option<Profession> {
        // Any filled cauldron counts too.
        let block = if block.ends_with("_cauldron") {
            "minecraft:cauldron"
        } else {
            block
        };
        Profession::WORKERS
            .into_iter()
            .find(|p| p.workstation() == block)
    }

    fn trades(self) -> bool {
        !matches!(self, Profession::None | Profession::Nitwit)
    }

    fn title(self) -> &'static str {
        match self {
            Profession::Armorer => "Armorer",
            Profession::Butcher => "Butcher",
            Profession::Cartographer => "Cartographer",
            Profession::Cleric => "Cleric",
            Profession::Farmer => "Farmer",
            Profession::Fisherman => "Fisherman",
            Profession::Fletcher => "Fletcher",
            Profession::Leatherworker => "Leatherworker",
            Profession::Librarian => "Librarian",
            Profession::Mason => "Mason",
            Profession::Shepherd => "Shepherd",
            Profession::Toolsmith => "Toolsmith",
            Profession::Weaponsmith => "Weaponsmith",
            Profession::None | Profession::Nitwit => "Villager",
        }
    }
}

/// One trade on offer.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub buy: Stack,
    pub buy_b: Stack,
    pub sell: Stack,
    pub uses: i32,
    pub max_uses: i32,
    /// Experience the villager earns per trade.
    pub xp: i32,
    pub price_multiplier: f32,
}

impl Offer {
    fn to_nbt(&self) -> Compound {
        let mut nbt = Compound::new()
            .with("buy", self.buy.to_nbt())
            .with("sell", self.sell.to_nbt())
            .with("uses", self.uses)
            .with("maxUses", self.max_uses)
            .with("xp", self.xp)
            .with("rewardExp", true)
            .with("priceMultiplier", self.price_multiplier)
            .with("specialPrice", 0)
            .with("demand", 0);
        if !self.buy_b.is_empty() {
            nbt.insert("buyB", self.buy_b.to_nbt());
        }
        nbt
    }

    fn from_nbt(nbt: &Compound) -> Option<Offer> {
        let stack = |key: &str| nbt.compound(key).map_or(Stack::EMPTY, Stack::from_nbt);
        let offer = Offer {
            buy: stack("buy"),
            buy_b: stack("buyB"),
            sell: stack("sell"),
            uses: nbt.int("uses").unwrap_or(0) as i32,
            max_uses: nbt.int("maxUses").unwrap_or(12) as i32,
            xp: nbt.int("xp").unwrap_or(1) as i32,
            price_multiplier: nbt.float("priceMultiplier").unwrap_or(0.05) as f32,
        };
        (!offer.buy.is_empty() && !offer.sell.is_empty()).then_some(offer)
    }

    pub fn sold_out(&self) -> bool {
        self.uses >= self.max_uses
    }
}

pub struct Villager {
    /// The biome kind it comes from (`minecraft:plains`, ...), which sets its clothes.
    pub kind: &'static str,
    pub profession: Profession,
    pub level: i32,
    pub xp: i32,
    pub offers: Vec<Offer>,
    /// The workstation it claimed.
    pub job_site: Option<BlockPos>,
    /// The player trading with it, who alone may.
    pub customer: Option<i32>,
    next_site_check: u64,
}

impl Villager {
    pub fn new(kind: &'static str) -> Self {
        Villager {
            kind,
            profession: Profession::None,
            level: 1,
            xp: 0,
            offers: Vec::new(),
            job_site: None,
            customer: None,
            next_site_check: 0,
        }
    }

    pub fn write_nbt(&self, nbt: &mut Compound) {
        nbt.insert(
            "VillagerData",
            Compound::new()
                .with("type", self.kind)
                .with("profession", self.profession.name())
                .with("level", self.level),
        );
        nbt.insert("Xp", self.xp);
        nbt.insert(
            "Offers",
            Compound::new().with(
                "Recipes",
                self.offers
                    .iter()
                    .map(|o| Tag::Compound(o.to_nbt()))
                    .collect::<Vec<_>>(),
            ),
        );
        if let Some(site) = self.job_site {
            nbt.insert(
                "Brain",
                Compound::new().with(
                    "memories",
                    Compound::new().with(
                        "minecraft:job_site",
                        Compound::new().with(
                            "value",
                            Compound::new()
                                .with("pos", vec![site.x, site.y, site.z])
                                .with("dimension", "minecraft:overworld"),
                        ),
                    ),
                ),
            );
        }
    }

    pub fn read_nbt(nbt: &Compound) -> Self {
        let data = nbt.compound("VillagerData");
        let kind = data
            .and_then(|d| d.str("type"))
            .and_then(|t| villager_type::NAMES.iter().find(|&&n| n == t))
            .copied()
            .unwrap_or("minecraft:plains");
        let mut v = Villager::new(kind);
        v.profession = Profession::from_name(data.and_then(|d| d.str("profession")).unwrap_or(""));
        v.level = data.and_then(|d| d.int("level")).unwrap_or(1).clamp(1, 5) as i32;
        v.xp = nbt.int("Xp").unwrap_or(0) as i32;
        v.offers = nbt
            .compound("Offers")
            .and_then(|o| o.list("Recipes"))
            .unwrap_or_default()
            .iter()
            .filter_map(Tag::as_compound)
            .filter_map(Offer::from_nbt)
            .collect();
        v.job_site = nbt
            .compound("Brain")
            .and_then(|b| b.compound("memories"))
            .and_then(|m| m.compound("minecraft:job_site"))
            .and_then(|j| j.compound("value"))
            .and_then(|v| v.int_array("pos"))
            .and_then(|p| match p {
                &[x, y, z] => Some(BlockPos::new(x, y, z)),
                _ => None,
            });
        v
    }

    /// The entity data that dresses the villager for its kind, profession and level.
    pub fn data_packet(&self, id: i32) -> Writer {
        let index =
            |names: &[&str], name: &str| names.iter().position(|&n| n == name).unwrap_or(0) as i32;
        Writer::packet(clientbound::SET_ENTITY_DATA)
            .varint(id)
            .u8(DATA_VILLAGER)
            .varint(SERIALIZER_VILLAGER_DATA)
            .varint(index(villager_type::NAMES, self.kind))
            .varint(index(villager_profession::NAMES, self.profession.name()))
            .varint(self.level)
            .u8(0xFF)
    }

    /// Adds two trades of the villager's current level.
    fn add_trades(&mut self, rng: &mut Rng) {
        let mut pool: Vec<&Spec> = table(self.profession)
            .iter()
            .filter(|s| s.level == self.level)
            .collect();
        for _ in 0..2 {
            if pool.is_empty() {
                break;
            }
            let spec = pool.swap_remove(rng.below(pool.len() as u32) as usize);
            if let Some(offer) = spec.offer(rng) {
                self.offers.push(offer);
            }
        }
    }

    fn restock(&mut self) {
        for offer in &mut self.offers {
            offer.uses = 0;
        }
    }

    /// The trades as the trading screen's click handling sees them.
    pub fn trades(&self) -> Vec<Trade> {
        self.offers
            .iter()
            .map(|o| Trade {
                buy: o.buy,
                buy_b: o.buy_b,
                sell: o.sell,
                left: o.max_uses - o.uses,
            })
            .collect()
    }
}

/// Fixed prices and amounts, or an enchanted item or book priced by its enchantments.
#[derive(Clone, Copy)]
enum Kind {
    Plain,
    /// An enchanted copy of `sell`, costing this many emeralds plus the enchanting level.
    Enchanted(i32),
    /// An enchanted book for emeralds and a book.
    Book,
}

/// A line of the trade table.
struct Spec {
    level: i32,
    buy: (&'static str, i32),
    buy_b: (&'static str, i32),
    sell: (&'static str, i32),
    max_uses: i32,
    xp: i32,
    kind: Kind,
}

const fn t(
    level: i32,
    buy: (&'static str, i32),
    sell: (&'static str, i32),
    max_uses: i32,
    xp: i32,
) -> Spec {
    Spec {
        level,
        buy,
        buy_b: ("", 0),
        sell,
        max_uses,
        xp,
        kind: Kind::Plain,
    }
}

/// A trade taking a second item along with the emeralds.
const fn t2(
    level: i32,
    buy: (&'static str, i32),
    buy_b: (&'static str, i32),
    sell: (&'static str, i32),
    max_uses: i32,
    xp: i32,
) -> Spec {
    Spec {
        level,
        buy,
        buy_b,
        sell,
        max_uses,
        xp,
        kind: Kind::Plain,
    }
}

/// An enchanted item for `base` emeralds plus its enchanting level.
const fn ench(level: i32, base: i32, sell: &'static str, xp: i32) -> Spec {
    Spec {
        level,
        buy: ("emerald", base),
        buy_b: ("", 0),
        sell: (sell, 1),
        max_uses: 3,
        xp,
        kind: Kind::Enchanted(base),
    }
}

const fn book(level: i32, xp: i32) -> Spec {
    Spec {
        level,
        buy: ("emerald", 1),
        buy_b: ("book", 1),
        sell: ("enchanted_book", 1),
        max_uses: 12,
        xp,
        kind: Kind::Book,
    }
}

const E: &str = "emerald";

const FARMER: &[Spec] = &[
    t(1, ("wheat", 20), (E, 1), 16, 2),
    t(1, ("potato", 26), (E, 1), 16, 2),
    t(1, ("carrot", 22), (E, 1), 16, 2),
    t(1, ("beetroot", 15), (E, 1), 16, 2),
    t(1, (E, 1), ("bread", 6), 16, 1),
    t(2, ("pumpkin", 6), (E, 1), 12, 10),
    t(2, (E, 1), ("pumpkin_pie", 4), 12, 5),
    t(2, (E, 1), ("apple", 4), 16, 5),
    t(3, (E, 3), ("cookie", 18), 12, 10),
    t(3, ("melon", 4), (E, 1), 12, 20),
    t(4, (E, 1), ("cake", 1), 12, 15),
    t(4, (E, 1), ("suspicious_stew", 1), 12, 15),
    t(5, (E, 3), ("golden_carrot", 3), 12, 30),
    t(5, (E, 4), ("glistering_melon_slice", 3), 12, 30),
];

const FISHERMAN: &[Spec] = &[
    t(1, ("string", 20), (E, 1), 16, 2),
    t(1, ("coal", 10), (E, 1), 16, 2),
    t2(1, (E, 1), ("cod", 6), ("cooked_cod", 6), 16, 1),
    t(1, (E, 3), ("cod_bucket", 1), 16, 1),
    t(2, ("cod", 15), (E, 1), 16, 10),
    t2(2, (E, 1), ("salmon", 6), ("cooked_salmon", 6), 16, 5),
    t(2, (E, 2), ("campfire", 1), 12, 5),
    t(3, ("salmon", 13), (E, 1), 16, 20),
    ench(3, 3, "fishing_rod", 10),
    t(4, ("tropical_fish", 6), (E, 1), 12, 30),
    t(5, ("pufferfish", 4), (E, 1), 12, 30),
    t(5, ("oak_boat", 1), (E, 1), 12, 30),
];

const SHEPHERD: &[Spec] = &[
    t(1, ("white_wool", 18), (E, 1), 16, 2),
    t(1, ("brown_wool", 18), (E, 1), 16, 2),
    t(1, ("black_wool", 18), (E, 1), 16, 2),
    t(1, ("gray_wool", 18), (E, 1), 16, 2),
    t(1, (E, 2), ("shears", 1), 12, 1),
    t(2, ("white_dye", 12), (E, 1), 16, 10),
    t(2, ("gray_dye", 12), (E, 1), 16, 10),
    t(2, ("black_dye", 12), (E, 1), 16, 10),
    t(2, ("light_blue_dye", 12), (E, 1), 16, 10),
    t(2, ("lime_dye", 12), (E, 1), 16, 10),
    t(2, (E, 1), ("white_wool", 1), 16, 5),
    t(2, (E, 1), ("white_carpet", 4), 16, 5),
    t(3, ("light_gray_dye", 12), (E, 1), 16, 20),
    t(3, ("orange_dye", 12), (E, 1), 16, 20),
    t(3, ("red_dye", 12), (E, 1), 16, 20),
    t(3, ("pink_dye", 12), (E, 1), 16, 20),
    t(3, (E, 3), ("white_bed", 1), 12, 10),
    t(4, ("brown_dye", 12), (E, 1), 16, 30),
    t(4, ("purple_dye", 12), (E, 1), 16, 30),
    t(4, ("blue_dye", 12), (E, 1), 16, 30),
    t(4, ("green_dye", 12), (E, 1), 16, 30),
    t(4, ("magenta_dye", 12), (E, 1), 16, 30),
    t(4, ("cyan_dye", 12), (E, 1), 16, 30),
    t(4, (E, 3), ("white_banner", 1), 12, 15),
    t(5, (E, 2), ("painting", 3), 12, 30),
];

const FLETCHER: &[Spec] = &[
    t(1, ("stick", 32), (E, 1), 16, 2),
    t(1, (E, 1), ("arrow", 16), 12, 1),
    t2(1, (E, 1), ("gravel", 10), ("flint", 10), 12, 1),
    t(2, ("flint", 26), (E, 1), 12, 10),
    t(2, (E, 2), ("bow", 1), 12, 5),
    t(3, ("string", 14), (E, 1), 16, 20),
    t(3, (E, 3), ("crossbow", 1), 12, 10),
    t(4, ("feather", 24), (E, 1), 16, 30),
    ench(4, 2, "bow", 15),
    t(5, ("tripwire_hook", 8), (E, 1), 12, 30),
    ench(5, 3, "crossbow", 15),
];

const LIBRARIAN: &[Spec] = &[
    t(1, ("paper", 24), (E, 1), 16, 2),
    book(1, 1),
    t(1, (E, 9), ("bookshelf", 1), 12, 1),
    t(2, ("book", 4), (E, 1), 12, 10),
    book(2, 5),
    t(2, (E, 1), ("lantern", 1), 12, 5),
    t(3, ("ink_sac", 5), (E, 1), 12, 20),
    book(3, 10),
    t(3, (E, 1), ("glass", 4), 12, 10),
    t(4, ("writable_book", 2), (E, 1), 12, 30),
    book(4, 15),
    t(4, (E, 5), ("clock", 1), 12, 15),
    t(4, (E, 4), ("compass", 1), 12, 15),
    t(5, (E, 20), ("name_tag", 1), 12, 30),
];

const CARTOGRAPHER: &[Spec] = &[
    t(1, ("paper", 24), (E, 1), 16, 2),
    t(1, (E, 7), ("map", 1), 12, 1),
    t(2, ("glass_pane", 11), (E, 1), 16, 10),
    t(3, ("compass", 1), (E, 1), 12, 20),
    t(4, (E, 7), ("item_frame", 1), 12, 15),
    t(4, (E, 3), ("white_banner", 1), 12, 15),
    t(5, (E, 8), ("globe_banner_pattern", 1), 12, 30),
];

const CLERIC: &[Spec] = &[
    t(1, ("rotten_flesh", 32), (E, 1), 16, 2),
    t(1, (E, 1), ("redstone", 2), 12, 1),
    t(2, ("gold_ingot", 3), (E, 1), 12, 10),
    t(2, (E, 1), ("lapis_lazuli", 1), 12, 5),
    t(3, ("rabbit_foot", 2), (E, 1), 12, 20),
    t(3, (E, 4), ("glowstone", 1), 12, 10),
    t(4, ("turtle_scute", 4), (E, 1), 12, 30),
    t(4, ("glass_bottle", 9), (E, 1), 12, 30),
    t(4, (E, 5), ("ender_pearl", 1), 12, 15),
    t(5, ("nether_wart", 22), (E, 1), 12, 30),
    t(5, (E, 3), ("experience_bottle", 1), 12, 30),
];

const ARMORER: &[Spec] = &[
    t(1, ("coal", 15), (E, 1), 16, 2),
    t(1, (E, 7), ("iron_leggings", 1), 12, 1),
    t(1, (E, 4), ("iron_boots", 1), 12, 1),
    t(1, (E, 5), ("iron_helmet", 1), 12, 1),
    t(1, (E, 9), ("iron_chestplate", 1), 12, 1),
    t(2, ("iron_ingot", 4), (E, 1), 12, 10),
    t(2, (E, 36), ("bell", 1), 12, 5),
    t(2, (E, 1), ("chainmail_boots", 1), 12, 5),
    t(2, (E, 3), ("chainmail_leggings", 1), 12, 5),
    t(3, ("lava_bucket", 1), (E, 1), 12, 20),
    t(3, ("diamond", 1), (E, 1), 12, 20),
    t(3, (E, 1), ("chainmail_helmet", 1), 12, 10),
    t(3, (E, 4), ("chainmail_chestplate", 1), 12, 10),
    t(3, (E, 5), ("shield", 1), 12, 10),
    ench(4, 14, "diamond_leggings", 15),
    ench(4, 8, "diamond_boots", 15),
    ench(5, 8, "diamond_helmet", 30),
    ench(5, 16, "diamond_chestplate", 30),
];

const WEAPONSMITH: &[Spec] = &[
    t(1, ("coal", 15), (E, 1), 16, 2),
    t(1, (E, 3), ("iron_axe", 1), 12, 1),
    ench(1, 2, "iron_sword", 1),
    t(2, ("iron_ingot", 4), (E, 1), 12, 10),
    t(2, (E, 36), ("bell", 1), 12, 5),
    t(3, ("flint", 24), (E, 1), 12, 20),
    t(4, ("diamond", 1), (E, 1), 12, 30),
    ench(4, 12, "diamond_axe", 15),
    ench(5, 8, "diamond_sword", 30),
];

const TOOLSMITH: &[Spec] = &[
    t(1, ("coal", 15), (E, 1), 16, 2),
    t(1, (E, 1), ("stone_axe", 1), 12, 1),
    t(1, (E, 1), ("stone_shovel", 1), 12, 1),
    t(1, (E, 1), ("stone_pickaxe", 1), 12, 1),
    t(1, (E, 1), ("stone_hoe", 1), 12, 1),
    t(2, ("iron_ingot", 4), (E, 1), 12, 10),
    t(2, (E, 36), ("bell", 1), 12, 5),
    t(3, ("flint", 30), (E, 1), 12, 20),
    ench(3, 1, "iron_axe", 10),
    ench(3, 2, "iron_shovel", 10),
    ench(3, 3, "iron_pickaxe", 10),
    t(3, (E, 4), ("diamond_hoe", 1), 3, 10),
    t(4, ("diamond", 1), (E, 1), 12, 30),
    ench(4, 12, "diamond_axe", 15),
    ench(4, 5, "diamond_shovel", 15),
    ench(5, 13, "diamond_pickaxe", 30),
];

const BUTCHER: &[Spec] = &[
    t(1, ("chicken", 14), (E, 1), 16, 2),
    t(1, ("porkchop", 7), (E, 1), 16, 2),
    t(1, ("rabbit", 4), (E, 1), 16, 2),
    t(1, (E, 1), ("rabbit_stew", 1), 12, 1),
    t(2, ("coal", 15), (E, 1), 16, 2),
    t(2, (E, 1), ("cooked_porkchop", 5), 16, 5),
    t(2, (E, 1), ("cooked_chicken", 8), 16, 5),
    t(3, ("mutton", 7), (E, 1), 16, 20),
    t(3, ("beef", 10), (E, 1), 16, 20),
    t(4, ("dried_kelp_block", 10), (E, 1), 12, 30),
    t(5, ("sweet_berries", 10), (E, 1), 12, 30),
];

const LEATHERWORKER: &[Spec] = &[
    t(1, ("leather", 6), (E, 1), 16, 2),
    t(1, (E, 3), ("leather_leggings", 1), 12, 1),
    t(1, (E, 7), ("leather_chestplate", 1), 12, 1),
    t(2, ("flint", 26), (E, 1), 12, 10),
    t(2, (E, 5), ("leather_helmet", 1), 12, 5),
    t(2, (E, 4), ("leather_boots", 1), 12, 5),
    t(3, ("rabbit_hide", 9), (E, 1), 12, 20),
    t(3, (E, 7), ("leather_chestplate", 1), 12, 10),
    t(4, ("turtle_scute", 4), (E, 1), 12, 30),
    t(4, (E, 6), ("leather_horse_armor", 1), 12, 15),
    t(5, (E, 6), ("saddle", 1), 12, 30),
    t(5, (E, 5), ("leather_helmet", 1), 12, 30),
];

const MASON: &[Spec] = &[
    t(1, ("clay_ball", 10), (E, 1), 16, 2),
    t(1, (E, 1), ("brick", 10), 16, 1),
    t(2, ("stone", 20), (E, 1), 16, 10),
    t(2, (E, 1), ("chiseled_stone_bricks", 4), 16, 5),
    t(3, ("granite", 16), (E, 1), 16, 20),
    t(3, ("andesite", 16), (E, 1), 16, 20),
    t(3, ("diorite", 16), (E, 1), 16, 20),
    t(3, (E, 1), ("polished_andesite", 4), 16, 10),
    t(3, (E, 1), ("polished_diorite", 4), 16, 10),
    t(3, (E, 1), ("polished_granite", 4), 16, 10),
    t(4, ("quartz", 12), (E, 1), 12, 30),
    t(4, (E, 1), ("white_terracotta", 1), 12, 15),
    t(4, (E, 1), ("white_glazed_terracotta", 1), 12, 15),
    t(5, (E, 1), ("quartz_pillar", 1), 12, 30),
    t(5, (E, 1), ("quartz_block", 1), 12, 30),
];

fn table(profession: Profession) -> &'static [Spec] {
    match profession {
        Profession::Farmer => FARMER,
        Profession::Fisherman => FISHERMAN,
        Profession::Shepherd => SHEPHERD,
        Profession::Fletcher => FLETCHER,
        Profession::Librarian => LIBRARIAN,
        Profession::Cartographer => CARTOGRAPHER,
        Profession::Cleric => CLERIC,
        Profession::Armorer => ARMORER,
        Profession::Weaponsmith => WEAPONSMITH,
        Profession::Toolsmith => TOOLSMITH,
        Profession::Butcher => BUTCHER,
        Profession::Leatherworker => LEATHERWORKER,
        Profession::Mason => MASON,
        Profession::None | Profession::Nitwit => &[],
    }
}

fn item(name: &str, count: i32) -> Option<Stack> {
    if name.is_empty() {
        return Some(Stack::EMPTY);
    }
    let id = ItemInfo::id_of(&format!("minecraft:{name}"))?;
    Some(Stack::new(id, count))
}

impl Spec {
    fn offer(&self, rng: &mut Rng) -> Option<Offer> {
        let mut buy = item(self.buy.0, self.buy.1)?;
        let mut buy_b = item(self.buy_b.0, self.buy_b.1)?;
        let mut sell = item(self.sell.0, self.sell.1)?;
        let mut multiplier = 0.05;
        match self.kind {
            Kind::Plain => {}
            Kind::Enchanted(base) => {
                let level = 5 + rng.below(15);
                for (id, l) in enchanting::select(rng, sell.item, level) {
                    sell.enchants.set(id, l);
                }
                buy.count = (base + level as i32).min(64);
                multiplier = 0.2;
            }
            Kind::Book => {
                let options = Value::String("#minecraft:tradeable".to_owned());
                let (id, level) = enchanting::random_for_loot(rng, buy_b.item, &options)?;
                sell.enchants.set(id, level);
                let level = level as i32;
                let mut cost = 2 + rng.below((5 + level * 10) as u32) as i32 + 3 * level;
                let treasure = TagRegistry::get("minecraft:enchantment")
                    .is_some_and(|t| t.members("minecraft:treasure").contains(&i64::from(id)));
                if treasure {
                    cost *= 2;
                }
                buy.count = cost.min(64);
                multiplier = 0.2;
                buy_b = buy_b.with_count(1);
            }
        }
        Some(Offer {
            buy,
            buy_b,
            sell,
            uses: 0,
            max_uses: self.max_uses,
            xp: self.xp,
            price_multiplier: multiplier,
        })
    }
}

/// The villager kind of a biome: its climate decides how villagers there dress.
pub fn kind_for_biome(biome: &str) -> &'static str {
    let b = biome.trim_start_matches("minecraft:");
    if b.contains("desert") || b.contains("badlands") {
        "minecraft:desert"
    } else if b.contains("jungle") {
        "minecraft:jungle"
    } else if b.contains("savanna") {
        "minecraft:savanna"
    } else if b.contains("snowy") || b.contains("frozen") || b.contains("ice") {
        "minecraft:snow"
    } else if b.contains("swamp") {
        "minecraft:swamp"
    } else if b.contains("taiga") || b.contains("grove") {
        "minecraft:taiga"
    } else {
        "minecraft:plains"
    }
}

impl World {
    /// Villagers' daily life: finding work, losing it, and restocking.
    pub(super) fn tick_villager(&mut self, id: i32, mob: &mut Mob) {
        let tick = self.tick;
        let time = self.day_time.rem_euclid(24_000);
        let pos = mob.pos;
        let adult = mob.age >= 0;
        let Some(v) = mob.villager.as_mut() else {
            return;
        };
        if RESTOCK_TIMES.contains(&time) && v.job_site.is_some() {
            v.restock();
        }
        if tick < v.next_site_check {
            return;
        }
        v.next_site_check = tick + SITE_CHECK_TICKS;
        // A workstation that is gone takes the job of a villager that never traded.
        if let Some(site) = v.job_site {
            let name = BlockInfo::of(self.block(site)).name;
            if Profession::of_workstation(name) != Some(v.profession) {
                v.job_site = None;
                if v.xp == 0 && v.level == 1 {
                    v.profession = Profession::None;
                    v.offers.clear();
                }
            }
        }
        if !adult || v.job_site.is_some() || v.profession == Profession::Nitwit {
            return;
        }
        let wanted = v.profession;
        let claimed: HashSet<BlockPos> = self
            .mobs
            .values()
            .filter_map(|m| m.villager.as_ref()?.job_site)
            .collect();
        let feet = BlockPos::new(
            pos[0].floor() as i32,
            pos[1].floor() as i32,
            pos[2].floor() as i32,
        );
        let mut best: Option<(i32, BlockPos, Profession)> = None;
        for dy in -SITE_REACH_UP..=SITE_REACH_UP {
            for dx in -SITE_REACH..=SITE_REACH {
                for dz in -SITE_REACH..=SITE_REACH {
                    let q = BlockPos::new(feet.x + dx, feet.y + dy, feet.z + dz);
                    let Some(state) = self.loaded_state(q) else {
                        continue;
                    };
                    let Some(job) = Profession::of_workstation(BlockInfo::of(state).name) else {
                        continue;
                    };
                    // One already working sticks to its trade.
                    if (wanted != Profession::None && job != wanted) || claimed.contains(&q) {
                        continue;
                    }
                    let d = dx * dx + dy * dy + dz * dz;
                    if best.is_none_or(|(b, _, _)| d < b) {
                        best = Some((d, q, job));
                    }
                }
            }
        }
        let Some((_, site, job)) = best else {
            return;
        };
        let v = mob.villager.as_mut().expect("checked above");
        v.job_site = Some(site);
        if v.profession == Profession::None {
            v.profession = job;
            v.add_trades(&mut self.rng);
            let sound = format!(
                "minecraft:entity.villager.work_{}",
                job.name().trim_start_matches("minecraft:")
            );
            self.play_sound_at(&sound, pos, 1.0, 1.0, None);
        }
        if let Some(v) = mob.villager.as_ref() {
            let packet = v.data_packet(id);
            self.send_to_viewers(mob, packet);
        }
    }

    /// A player right-clicks a villager: the trading screen opens if it has trades.
    pub(super) fn open_trading(&mut self, id: i32, target: i32) -> bool {
        let Some(mob) = self.mobs.get_mut(&target) else {
            return false;
        };
        let (adult, pos) = (mob.age >= 0, mob.pos);
        let Some(v) = mob.villager.as_mut() else {
            return false;
        };
        let busy = v.customer.is_some_and(|c| c != id);
        if !adult || !v.profession.trades() || v.offers.is_empty() || busy {
            self.play_sound_at("minecraft:entity.villager.no", pos, 1.0, 1.0, None);
            return true;
        }
        v.customer = Some(id);
        let title = v.profession.title();
        self.close_container(id);
        let p = self.players.get_mut(&id).expect("player exists");
        p.next_window = p.next_window % MAX_WINDOW_ID + 1;
        let window = p.next_window;
        p.open = Some(Open {
            window,
            view: View::Merchant(target),
            grid: [Stack::EMPTY; 9],
            result: Stack::EMPTY,
            offers: Default::default(),
            choice: None,
            rename: None,
        });
        self.send(
            id,
            Writer::packet(clientbound::OPEN_SCREEN)
                .varint(window)
                .varint(menu::MERCHANT)
                .text(title),
        );
        self.send_offers(id, target);
        self.resync_window(id, window, View::Merchant(target));
        true
    }

    /// The villager's trades, its level and experience, for the trading screen.
    pub(super) fn send_offers(&mut self, id: i32, target: i32) {
        let Some(window) = self
            .players
            .get(&id)
            .and_then(|p| p.open.as_ref())
            .map(|o| o.window)
        else {
            return;
        };
        let Some(v) = self.mobs.get(&target).and_then(|m| m.villager.as_ref()) else {
            return;
        };
        let cost = |packet: Writer, s: Stack| {
            // An item cost: the item, how many, and no required components.
            packet.varint(s.item).varint(s.count).varint(0)
        };
        let mut packet = Writer::packet(clientbound::MERCHANT_OFFERS)
            .varint(window)
            .varint(v.offers.len() as i32);
        for o in &v.offers {
            packet = cost(packet, o.buy);
            packet = crate::inventory::write_stack(packet, o.sell);
            packet = if o.buy_b.is_empty() {
                packet.bool(false)
            } else {
                cost(packet.bool(true), o.buy_b)
            };
            packet = packet
                .bool(o.sold_out())
                .i32(o.uses)
                .i32(o.max_uses)
                .i32(o.xp)
                .i32(0) // special price
                .f32(o.price_multiplier)
                .i32(0); // demand
        }
        packet = packet
            .varint(v.level)
            .varint(v.xp)
            .bool(true) // a regular villager, with levels
            .bool(true); // which restocks
        self.send(id, packet);
    }

    /// The player picked a trade: what they paid in before goes back, and the payment
    /// for the new one comes out of their inventory.
    pub(super) fn select_trade(&mut self, id: i32, index: i32) {
        let Some(p) = self.players.get_mut(&id) else {
            return;
        };
        let Some(open) = p.open.as_mut() else {
            return;
        };
        let View::Merchant(target) = open.view else {
            return;
        };
        let window = open.window;
        let Some(offer) = self
            .mobs
            .get(&target)
            .and_then(|m| m.villager.as_ref())
            .and_then(|v| v.offers.get(usize::try_from(index).ok()?))
            .cloned()
        else {
            return;
        };
        open.choice = usize::try_from(index).ok();
        let returned = [open.grid[0], open.grid[1]];
        open.grid[0] = Stack::EMPTY;
        open.grid[1] = Stack::EMPTY;
        for stack in returned.into_iter().filter(|s| !s.is_empty()) {
            let (_, over) = p.inventory.add(stack);
            if over > 0 {
                let open = p.open.as_mut().expect("checked above");
                open.grid[0] = stack.with_count(over);
            }
        }
        // Pull the payment out of the inventory, up to a full stack of each.
        for (slot, cost) in [(0, offer.buy), (1, offer.buy_b)] {
            if cost.is_empty() {
                continue;
            }
            let open = p.open.as_ref().expect("checked above");
            let mut have = open.grid[slot];
            if !have.is_empty() && !have.same_kind(&cost.with_count(have.count)) {
                continue;
            }
            for s in 0..crate::inventory::SLOTS {
                if !(9..45).contains(&s) {
                    continue;
                }
                let stack = p.inventory.slots[s];
                if stack.item != cost.item || stack.potion != cost.potion {
                    continue;
                }
                let room = stack.max_stack() - have.count;
                let take = room.min(stack.count);
                if take <= 0 {
                    break;
                }
                have = stack.with_count(have.count + take);
                p.inventory.slots[s] = stack.with_count(stack.count - take);
            }
            p.open.as_mut().expect("checked above").grid[slot] = have;
        }
        // The result shows as soon as the payment covers it.
        let open = p.open.as_mut().expect("checked above");
        let trade = Trade {
            buy: offer.buy,
            buy_b: offer.buy_b,
            sell: offer.sell,
            left: offer.max_uses - offer.uses,
        };
        open.grid[2] = if trade.paid_by(open.grid[0], open.grid[1]) {
            offer.sell
        } else {
            Stack::EMPTY
        };
        self.resync_window(id, window, View::Merchant(target));
        let p = self.players.get_mut(&id).expect("player exists");
        let content = p.inventory.content_packet();
        self.send(id, content);
    }

    /// Trades went through: the villager counts the uses and its experience, the player
    /// gets a little experience, and the villager may climb a level.
    pub(super) fn traded(&mut self, id: i32, target: i32, index: usize, times: i32) {
        let mut player_xp = 0;
        let mut levelled = false;
        let pos = {
            let Some(mob) = self.mobs.get_mut(&target) else {
                return;
            };
            let Some(v) = mob.villager.as_mut() else {
                return;
            };
            let Some(offer) = v.offers.get_mut(index) else {
                return;
            };
            offer.uses += times;
            v.xp += offer.xp * times;
            for _ in 0..times {
                player_xp += 3 + self.rng.below(4) as i32;
            }
            while v.level < 5 && v.xp >= LEVEL_XP[v.level as usize] {
                v.level += 1;
                v.add_trades(&mut self.rng);
                player_xp += 5;
                levelled = true;
            }
            mob.pos
        };
        self.play_sound_at("minecraft:entity.villager.yes", pos, 1.0, 1.0, None);
        if levelled {
            if let Some(mob) = self.mobs.remove(&target) {
                if let Some(v) = mob.villager.as_ref() {
                    let packet = v.data_packet(target);
                    self.send_to_viewers(&mob, packet);
                }
                self.mobs.insert(target, mob);
            }
            self.play_sound_at("minecraft:entity.villager.celebrate", pos, 1.0, 1.0, None);
        }
        let at = self.players.get(&id).map(|p| p.pos);
        if let Some(at) = at {
            self.spawn_xp(at, player_xp);
        }
        self.send_offers(id, target);
    }

    /// The player closed the trading screen.
    pub(super) fn stop_trading(&mut self, id: i32, target: i32) {
        if let Some(v) = self.mobs.get_mut(&target).and_then(|m| m.villager.as_mut()) {
            if v.customer == Some(id) {
                v.customer = None;
            }
        }
    }

    /// Spawns the villagers, and any other mobs Levitas knows, a structure placed.
    pub(super) fn spawn_structure_mobs(&mut self, entities: &[Compound]) {
        for nbt in entities {
            if let Some(mob) = Mob::read(nbt, &mut self.rng) {
                self.mobs.insert(next_entity_id(), mob);
            }
        }
    }
}

/// A new villager for `/summon` and spawn eggs: a novice of no profession, dressed for
/// `biome`.
pub fn summoned(pos: [f64; 3], biome: &str, rng: &mut Rng) -> Mob {
    let mut mob = Mob::new(MobKind::Villager, pos, rng);
    if let Some(v) = mob.villager.as_mut() {
        v.kind = kind_for_biome(biome);
    }
    mob
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn stack(name: &str, count: i32) -> Stack {
        item(name, count).unwrap()
    }

    #[test]
    fn a_villager_takes_a_job_and_trades() {
        let dir = std::env::temp_dir().join(format!("levitas-villager-{}", std::process::id()));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let lectern = BlockPos::new(4, -60, 4);
        w.set_block(
            lectern,
            BlockInfo::by_name("minecraft:lectern")
                .unwrap()
                .default_state,
        );
        let vid = next_entity_id();
        let villager = Mob::new(MobKind::Villager, [1.5, -60.0, 1.5], &mut w.rng);
        w.mobs.insert(vid, villager);
        let mut m = w.mobs.remove(&vid).unwrap();
        w.tick_villager(vid, &mut m);
        w.mobs.insert(vid, m);
        let v = w.mobs[&vid].villager.as_ref().unwrap();
        assert_eq!(v.profession, Profession::Librarian);
        assert_eq!(v.job_site, Some(lectern));
        assert_eq!(v.offers.len(), 2);

        // A plain trade to pay for: 24 paper for an emerald.
        let paper_trade = Offer {
            buy: stack("paper", 24),
            buy_b: Stack::EMPTY,
            sell: stack("emerald", 1),
            uses: 0,
            max_uses: 16,
            xp: 2,
            price_multiplier: 0.05,
        };
        w.mobs
            .get_mut(&vid)
            .unwrap()
            .villager
            .as_mut()
            .unwrap()
            .offers = vec![paper_trade];

        let (out, frames) = mpsc::channel(100_000);
        std::mem::forget(frames);
        let alice = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
            properties: Vec::new(),
        };
        w.players
            .insert(alice, Player::new(profile, out, [2.5, -60.0, 1.5]));
        w.players.get_mut(&alice).unwrap().inventory.slots[9] = stack("paper", 30);

        assert!(w.open_trading(alice, vid));
        w.select_trade(alice, 0);
        let open = w.players[&alice].open.as_ref().unwrap();
        assert_eq!(open.grid[0], stack("paper", 30), "payment pulled in");
        let window = open.window;
        w.container_click(alice, window, 2, 0, 0, Vec::new(), Stack::EMPTY);
        assert_eq!(w.players[&alice].inventory.carried, stack("emerald", 1));
        assert_eq!(
            w.players[&alice].open.as_ref().unwrap().grid[0],
            stack("paper", 6)
        );
        let v = w.mobs[&vid].villager.as_ref().unwrap();
        assert_eq!((v.offers[0].uses, v.xp), (1, 2));
        assert_eq!(v.customer, Some(alice));
        w.close_container(alice);
        assert_eq!(w.mobs[&vid].villager.as_ref().unwrap().customer, None);

        // Its lectern gone, a villager that traded keeps its profession.
        w.set_block(lectern, levitas_registry::block::AIR);
        let mut m = w.mobs.remove(&vid).unwrap();
        m.villager.as_mut().unwrap().next_site_check = 0;
        w.tick_villager(vid, &mut m);
        let v = m.villager.as_ref().unwrap();
        assert_eq!((v.profession, v.job_site), (Profession::Librarian, None));
    }

    #[test]
    fn every_trade_names_real_items() {
        let mut rng = Rng::new(3);
        for p in Profession::WORKERS {
            assert!(!table(p).is_empty(), "{p:?}");
            for level in 1..=5 {
                assert!(
                    table(p).iter().any(|s| s.level == level),
                    "{p:?} has no level {level} trades"
                );
            }
            for spec in table(p) {
                assert!(spec.offer(&mut rng).is_some(), "{p:?}: {:?}", spec.sell);
            }
            assert!(Profession::of_workstation(p.workstation()) == Some(p));
        }
        assert_eq!(
            Profession::of_workstation("minecraft:water_cauldron"),
            Some(Profession::Leatherworker)
        );
    }

    /// The entity data entry: index 19, serializer 18, then type, profession and level
    /// as registry IDs (a taiga librarian of level 3 here).
    #[test]
    fn villager_data_is_written_as_26_3_reads_it() {
        let mut v = Villager::new("minecraft:taiga");
        v.profession = Profession::Librarian;
        v.level = 3;
        let frame = v.data_packet(5).into_frame();
        let taiga = villager_type::NAMES
            .iter()
            .position(|&n| n == "minecraft:taiga");
        let librarian = villager_profession::NAMES
            .iter()
            .position(|&n| n == "minecraft:librarian");
        let tail = [
            19,
            18,
            taiga.unwrap() as u8,
            librarian.unwrap() as u8,
            3,
            0xFF,
        ];
        assert!(frame.ends_with(&tail), "{frame:?}");
    }

    #[test]
    fn villagers_keep_their_trades_through_saving() {
        let mut rng = Rng::new(9);
        let mut v = Villager::new("minecraft:taiga");
        v.profession = Profession::Librarian;
        v.add_trades(&mut rng);
        v.offers[0].uses = 3;
        v.job_site = Some(BlockPos::new(1, 64, -2));
        v.xp = 12;
        let mut nbt = Compound::new();
        v.write_nbt(&mut nbt);
        let back = Villager::read_nbt(&nbt);
        assert_eq!(back.kind, "minecraft:taiga");
        assert_eq!(back.profession, Profession::Librarian);
        assert_eq!(back.offers, v.offers);
        assert_eq!(back.job_site, v.job_site);
        assert_eq!(back.xp, 12);
    }
}
