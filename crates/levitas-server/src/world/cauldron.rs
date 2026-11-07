//! Using things on cauldrons, as the Minecraft Wiki describes it: buckets fill and
//! empty them (water, lava and powder snow), bottles take and give a level of water,
//! and water washes the dye off leather armor, a layer off a banner and the colour off
//! a shulker box, a level at a time.

use super::*;
use crate::blocks::{self, prop, with};

fn level(state: u16) -> i32 {
    prop(state, "level")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0)
}

fn block_state(name: &str) -> Option<u16> {
    BlockInfo::by_name(name).map(|b| b.default_state)
}

impl World {
    /// Uses the held item on a cauldron. Returns whether something happened.
    pub(super) fn use_cauldron(&mut self, id: i32, slot: usize, pos: BlockPos, state: u16) -> bool {
        let name = blocks::name(state);
        if !(name == "minecraft:cauldron" || name.ends_with("_cauldron")) {
            return false;
        }
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let held = p.inventory.slots[slot];
        let held_name = ItemInfo::get(held.item).map_or("", |i| i.name);
        let water = name == "minecraft:water_cauldron";
        let lvl = level(state);
        // (new cauldron, what the hand gets back, sound, statistic)
        let outcome: Option<(u16, Option<Stack>, &str, Option<i32>)> = match (name, held_name) {
            (_, "minecraft:water_bucket") if name != "minecraft:lava_cauldron" => {
                block_state("minecraft:water_cauldron").map(|c| {
                    (
                        with(c, "level", "3"),
                        self.item("minecraft:bucket"),
                        "minecraft:item.bucket.empty",
                        Some(levitas_registry::custom_stat::FILL_CAULDRON),
                    )
                })
            }
            ("minecraft:cauldron", "minecraft:lava_bucket") => {
                block_state("minecraft:lava_cauldron").map(|c| {
                    (
                        c,
                        self.item("minecraft:bucket"),
                        "minecraft:item.bucket.empty_lava",
                        None,
                    )
                })
            }
            ("minecraft:cauldron", "minecraft:powder_snow_bucket") => {
                block_state("minecraft:powder_snow_cauldron").map(|c| {
                    (
                        with(c, "level", "3"),
                        self.item("minecraft:bucket"),
                        "minecraft:item.bucket.empty_powder_snow",
                        None,
                    )
                })
            }
            ("minecraft:water_cauldron", "minecraft:bucket") if lvl == 3 => {
                block_state("minecraft:cauldron").map(|c| {
                    (
                        c,
                        self.item("minecraft:water_bucket"),
                        "minecraft:item.bucket.fill",
                        None,
                    )
                })
            }
            ("minecraft:lava_cauldron", "minecraft:bucket") => block_state("minecraft:cauldron")
                .map(|c| {
                    (
                        c,
                        self.item("minecraft:lava_bucket"),
                        "minecraft:item.bucket.fill_lava",
                        None,
                    )
                }),
            ("minecraft:powder_snow_cauldron", "minecraft:bucket") if lvl == 3 => {
                block_state("minecraft:cauldron").map(|c| {
                    (
                        c,
                        self.item("minecraft:powder_snow_bucket"),
                        "minecraft:item.bucket.fill_powder_snow",
                        None,
                    )
                })
            }
            ("minecraft:water_cauldron", "minecraft:glass_bottle") => {
                let water_bottle = self.item("minecraft:potion").map(|mut s| {
                    s.potion = crate::potions::id("minecraft:water");
                    s
                });
                Some((
                    self.lower(state),
                    water_bottle,
                    "minecraft:item.bottle.fill",
                    Some(levitas_registry::custom_stat::USE_CAULDRON),
                ))
            }
            (_, "minecraft:potion")
                if held.potion == crate::potions::id("minecraft:water")
                    && (name == "minecraft:cauldron" || (water && lvl < 3)) =>
            {
                let next = if water {
                    with(state, "level", &(lvl + 1).to_string())
                } else {
                    with(
                        block_state("minecraft:water_cauldron").unwrap_or(state),
                        "level",
                        "1",
                    )
                };
                Some((
                    next,
                    self.item("minecraft:glass_bottle"),
                    "minecraft:item.bottle.empty",
                    Some(levitas_registry::custom_stat::USE_CAULDRON),
                ))
            }
            // Washing.
            _ if water && held.color.is_some() => Some((
                self.lower(state),
                Some(Stack {
                    color: None,
                    ..held.with_count(1)
                }),
                "minecraft:block.water.ambient",
                Some(levitas_registry::custom_stat::CLEAN_ARMOR),
            )),
            _ if water && held.patterns.is_some() => {
                let mut layers = crate::banners::layers(held.patterns);
                layers.pop();
                Some((
                    self.lower(state),
                    Some(Stack {
                        patterns: crate::banners::intern(layers),
                        ..held.with_count(1)
                    }),
                    "minecraft:block.water.ambient",
                    Some(levitas_registry::custom_stat::CLEAN_BANNER),
                ))
            }
            (_, shulker) if water && shulker.ends_with("_shulker_box") => {
                let plain = self.item("minecraft:shulker_box").map(|s| Stack {
                    item: s.item,
                    ..held
                });
                Some((
                    self.lower(state),
                    plain,
                    "minecraft:block.water.ambient",
                    Some(levitas_registry::custom_stat::CLEAN_SHULKER_BOX),
                ))
            }
            _ => None,
        };
        let Some((next, back, sound, stat)) = outcome else {
            return false;
        };
        self.set_block_updating(pos, next);
        self.redstone_dirty.push(pos);
        self.play_sound(sound, pos, None);
        if let Some(stat) = stat {
            self.stat(id, levitas_registry::stat_type::CUSTOM, stat, 1);
        }
        let p = self.players.get_mut(&id).expect("checked");
        if !p.survival() {
            return true;
        }
        let washing = matches!(stat, Some(s) if s == levitas_registry::custom_stat::CLEAN_ARMOR || s == levitas_registry::custom_stat::CLEAN_BANNER || s == levitas_registry::custom_stat::CLEAN_SHULKER_BOX);
        let back = back.unwrap_or(Stack::EMPTY);
        if washing && held.count == 1 {
            // A washed stack of one stays in the hand, washed.
            p.inventory.slots[slot] = back;
        } else {
            p.inventory.take(slot, 1);
            if p.inventory.slots[slot].is_empty() {
                p.inventory.slots[slot] = back;
            } else if !back.is_empty() {
                let (changed, left) = p.inventory.add(back);
                let updates: Vec<Writer> = changed
                    .into_iter()
                    .map(|s| p.inventory.set_slot_packet(s))
                    .collect();
                let at = p.pos;
                for u in updates {
                    self.send(id, u);
                }
                if left > 0 {
                    self.scatter_item(at, back);
                }
            }
        }
        let p = self.players.get_mut(&id).expect("checked");
        let update = p.inventory.set_slot_packet(slot);
        self.send(id, update);
        true
    }

    /// A water cauldron one level lower (empty below level 1).
    fn lower(&self, state: u16) -> u16 {
        let lvl = level(state);
        if lvl <= 1 {
            block_state("minecraft:cauldron").unwrap_or(state)
        } else {
            with(state, "level", &(lvl - 1).to_string())
        }
    }

    fn item(&self, name: &str) -> Option<Stack> {
        ItemInfo::id_of(name).map(|i| Stack::new(i, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn world() -> (World, i32) {
        let dir = std::env::temp_dir().join(format!(
            "levitas-cauldron-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut w, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        w.chunk(0, 0);
        let (out, frames) = tokio::sync::mpsc::channel(100_000);
        std::mem::forget(frames);
        let id = next_entity_id();
        let profile = Profile {
            name: "Dyer".to_owned(),
            uuid: 8,
            properties: Vec::new(),
        };
        let mut p = Player::new(profile, out, [2.5, -60.0, 2.5]);
        p.mode = GAME_MODE_SURVIVAL;
        w.players.insert(id, p);
        (w, id)
    }

    fn stack(name: &str) -> Stack {
        Stack::new(ItemInfo::id_of(name).unwrap(), 1)
    }

    #[test]
    fn looms_layer_banners_that_copy_onto_shields_and_wash_off() {
        // The loom: a red creeper face needs the creeper pattern item; a stripe needs none.
        let banner = stack("minecraft:white_banner");
        let dye = stack("minecraft:red_dye");
        assert!(!crate::banners::selectable(Stack::EMPTY).is_empty());
        assert!(!crate::banners::selectable(stack("minecraft:creeper_banner_pattern")).is_empty());
        let striped = crate::banners::loom_result(banner, dye, Stack::EMPTY, Some(0));
        assert_eq!(crate::banners::layers(striped.patterns).len(), 1);
        assert_eq!(Stack::from_nbt(&striped.to_nbt()), striped);

        // Copied onto a blank banner of the same colour, and onto a shield.
        let r = crate::recipes::Recipes::load();
        let mut g = vec![Stack::EMPTY; 9];
        g[0] = striped;
        g[1] = banner;
        assert_eq!(r.craft(&g, 3).patterns, striped.patterns);
        g[1] = stack("minecraft:shield");
        let shield = r.craft(&g, 3);
        assert_eq!(shield.base_color, Some(0));
        assert_eq!(shield.patterns, striped.patterns);

        // A water cauldron: filled by a bucket, washing the layer off a level at a time.
        let (mut w, id) = world();
        let pos = BlockPos::new(3, -60, 3);
        let cauldron = BlockInfo::by_name("minecraft:cauldron")
            .unwrap()
            .default_state;
        w.set_block(pos, cauldron);
        let slot = w.players[&id].hand_slot(0);
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = stack("minecraft:water_bucket");
        assert!(w.use_cauldron(id, slot, pos, cauldron));
        assert_eq!(prop(w.block(pos), "level"), Some("3"));
        assert_eq!(
            w.players[&id].inventory.slots[slot].item,
            stack("minecraft:bucket").item
        );
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = striped;
        let full = w.block(pos);
        assert!(w.use_cauldron(id, slot, pos, full));
        assert!(w.players[&id].inventory.slots[slot].patterns.is_none());
        assert_eq!(prop(w.block(pos), "level"), Some("2"));
        // A bottle takes a level of water.
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = stack("minecraft:glass_bottle");
        let two = w.block(pos);
        assert!(w.use_cauldron(id, slot, pos, two));
        assert_eq!(prop(w.block(pos), "level"), Some("1"));
        assert!(w.players[&id].inventory.slots[slot].potion.is_some());

        // A placed banner keeps its layers, on disk too.
        let bpos = BlockPos::new(5, -60, 5);
        let mut entity = BlockEntity::for_block(
            BlockInfo::by_name("minecraft:white_banner")
                .unwrap()
                .default_state,
        )
        .unwrap();
        if let BlockEntity::Banner(b) = &mut entity {
            b.patterns = striped.patterns;
        }
        let saved = entity.to_nbt(bpos);
        let (_, back) = BlockEntity::from_nbt(&saved).unwrap();
        assert!(matches!(back, BlockEntity::Banner(b) if b.patterns == striped.patterns));
    }
}
