//! Huge mushrooms grown from small ones with bone meal (a 40% chance). The shapes are
//! ported from Pumpkin's `huge_brown_mushroom` and `huge_red_mushroom` features
//! (github.com/Pumpkin-MC/Pumpkin, GPL-3.0, crates/pumpkin-world/src/generation/feature/
//! features/), adapted to Levitas's world: a stem 4 to 6 blocks tall (1 in 12 twice
//! that), under a flat 7x7 brown cap without corners or a red dome of 3x3 sides.

use super::*;
use crate::blocks::{self, offset, with_bool, Dir};

/// How often bone meal grows a huge mushroom.
const GROW_CHANCE: f64 = 0.4;
/// The brown cap's and red dome's reach from the stem.
const BROWN_RADIUS: i32 = 3;
const RED_RADIUS: i32 = 2;

/// A mushroom block with its six outside faces set.
fn mushroom_block(name: &str, faces: [bool; 6]) -> Option<u16> {
    let mut state = BlockInfo::by_name(name)?.default_state;
    for (face, on) in ["up", "down", "north", "south", "west", "east"]
        .into_iter()
        .zip(faces)
    {
        state = with_bool(state, face, on);
    }
    Some(state)
}

impl World {
    /// Bone meal on a small mushroom: it grows huge 40% of the time, if there is room
    /// and it stands on soil that takes one. The bone meal is used either way.
    pub(super) fn bone_meal_mushroom(&mut self, pos: BlockPos, state: u16) {
        if self.rng.f64() < GROW_CHANCE {
            self.grow_huge_mushroom(pos, state);
        }
    }

    /// Grows the small mushroom at `pos` into a huge one; returns whether it did.
    pub(super) fn grow_huge_mushroom(&mut self, pos: BlockPos, state: u16) -> bool {
        let red = blocks::name(state) == "minecraft:red_mushroom";
        let height = {
            let h = 4 + self.rng.below(3) as i32;
            if self.rng.below(12) == 0 {
                h * 2
            } else {
                h
            }
        };
        if pos.y < MIN_Y + 1 || pos.y + height + 1 > MAX_Y {
            return false;
        }
        let soil = self.block(offset(pos, Dir::Down));
        let tag = if red {
            "minecraft:huge_red_mushroom_can_place_on"
        } else {
            "minecraft:huge_brown_mushroom_can_place_on"
        };
        if !block_has_tag(soil, tag) {
            return false;
        }
        // Room: air or leaves for the stem and where the cap goes (the mushroom itself
        // is replaced).
        for dy in 0..=height {
            let radius = if red {
                if dy >= height - 3 {
                    RED_RADIUS
                } else {
                    0
                }
            } else if dy <= 3 {
                0
            } else {
                BROWN_RADIUS
            };
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let at = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                    if at == pos {
                        continue;
                    }
                    let s = self.block(at);
                    if !is_air(s) && !block_has_tag(s, "minecraft:leaves") {
                        return false;
                    }
                }
            }
        }
        let mut blocks_to_place = Vec::new();
        if red {
            let r = RED_RADIUS;
            let k = r - 2;
            for i in (height - 3)..=height {
                let j = if i < height { r } else { r - 1 };
                for l in -j..=j {
                    for m in -j..=j {
                        let on_x_edge = l == -j || l == j;
                        let on_z_edge = m == -j || m == j;
                        if i < height && on_x_edge == on_z_edge {
                            continue;
                        }
                        let faces = [i >= height - 1, false, m < -k, m > k, l < -k, l > k];
                        if let Some(s) = mushroom_block("minecraft:red_mushroom_block", faces) {
                            blocks_to_place
                                .push((BlockPos::new(pos.x + l, pos.y + i, pos.z + m), s));
                        }
                    }
                }
            }
        } else {
            let r = BROWN_RADIUS;
            let cap_y = pos.y + height;
            for j in -r..=r {
                for k in -r..=r {
                    let on_x_edge = j == -r || j == r;
                    let on_z_edge = k == -r || k == r;
                    if on_x_edge && on_z_edge {
                        continue;
                    }
                    let faces = [
                        true,
                        false,
                        k == -r || (on_x_edge && k == 1 - r),
                        k == r || (on_x_edge && k == r - 1),
                        j == -r || (on_z_edge && j == 1 - r),
                        j == r || (on_z_edge && j == r - 1),
                    ];
                    if let Some(s) = mushroom_block("minecraft:brown_mushroom_block", faces) {
                        blocks_to_place.push((BlockPos::new(pos.x + j, cap_y, pos.z + k), s));
                    }
                }
            }
        }
        if let Some(stem) = mushroom_block(
            "minecraft:mushroom_stem",
            [false, false, true, true, true, true],
        ) {
            for i in 0..height {
                blocks_to_place.push((BlockPos::new(pos.x, pos.y + i, pos.z), stem));
            }
        }
        for (at, s) in blocks_to_place {
            let here = self.block(at);
            if at == pos
                || is_air(here)
                || block_has_tag(here, "minecraft:replaceable_by_mushrooms")
            {
                self.set_block_updating(at, s);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bone_meal_grows_both_huge_mushrooms() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-mushroom-{}-{}",
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
        let ground = (MIN_Y..MAX_Y)
            .rev()
            .find(|&y| !is_air(w.block(BlockPos::new(8, y, 8))))
            .unwrap();
        for (x, name, cap) in [
            (
                4,
                "minecraft:brown_mushroom",
                "minecraft:brown_mushroom_block",
            ),
            (12, "minecraft:red_mushroom", "minecraft:red_mushroom_block"),
        ] {
            let soil = BlockPos::new(x, ground, 8);
            w.set_block(
                soil,
                BlockInfo::by_name("minecraft:mycelium")
                    .unwrap()
                    .default_state,
            );
            let pos = offset(soil, Dir::Up);
            let small = BlockInfo::by_name(name).unwrap().default_state;
            w.set_block(pos, small);
            assert!(w.grow_huge_mushroom(pos, small));
            assert_eq!(blocks::name(w.block(pos)), "minecraft:mushroom_stem");
            let capped = (pos.y..pos.y + 14).any(|y| {
                (-3..=3).any(|d| blocks::name(w.block(BlockPos::new(x + d, y, 8))) == cap)
            });
            assert!(capped, "{name} has no cap");
        }
        // Not on stone.
        let pos = BlockPos::new(8, ground + 1, 2);
        w.set_block(offset(pos, Dir::Down), block::STONE);
        let small = BlockInfo::by_name("minecraft:red_mushroom")
            .unwrap()
            .default_state;
        w.set_block(pos, small);
        assert!(!w.grow_huge_mushroom(pos, small));
    }
}
