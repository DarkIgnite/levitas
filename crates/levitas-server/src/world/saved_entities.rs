//! Arrows, thrown tridents, experience orbs and falling blocks, saved with the chunk
//! they are in (the `entities/` region files), as vanilla saves them.

use levitas_nbt::Compound;

use super::*;

/// A saved entity's `Pos`, `Motion` or similar triple.
pub(super) fn triple_of(nbt: &levitas_nbt::Compound, key: &str) -> Option<[f64; 3]> {
    let v: Vec<f64> = nbt
        .list(key)?
        .iter()
        .filter_map(levitas_nbt::Tag::as_f64)
        .collect();
    Some([*v.first()?, *v.get(1)?, *v.get(2)?])
}

pub(super) fn doubles(v: [f64; 3]) -> Vec<levitas_nbt::Tag> {
    v.iter().map(|&x| levitas_nbt::Tag::Double(x)).collect()
}

pub(super) fn uuid_ints(u: u128) -> Vec<i32> {
    vec![
        (u >> 96) as i32,
        (u >> 64) as i32,
        (u >> 32) as i32,
        u as i32,
    ]
}

pub(super) fn uuid_of(ints: &[i32]) -> Option<u128> {
    let &[a, b, c, d] = ints else {
        return None;
    };
    Some(
        (u128::from(a as u32) << 96)
            | (u128::from(b as u32) << 64)
            | (u128::from(c as u32) << 32)
            | u128::from(d as u32),
    )
}

impl World {
    /// The saved forms of the arrows, orbs and falling blocks in chunk (`cx`, `cz`).
    pub(super) fn saved_extras(&self, cx: i32, cz: i32) -> Vec<Compound> {
        let mut out: Vec<Compound> = self
            .arrows
            .values()
            .filter(|a| a.chunk() == (cx, cz) && !a.returning)
            .map(|a| {
                let owner = self.players.get(&a.shooter).map(|p| p.profile.uuid);
                a.to_nbt(owner)
            })
            .collect();
        out.extend(
            self.orbs
                .values()
                .filter(|o| o.chunk() == (cx, cz))
                .map(experience::XpOrb::to_nbt),
        );
        out.extend(
            self.falling_blocks
                .values()
                .filter(|f| f.chunk() == (cx, cz))
                .map(falling::FallingBlock::to_nbt),
        );
        out
    }

    /// Chunks holding any of them.
    pub(super) fn extra_chunks(&self) -> Vec<(i32, i32)> {
        self.arrows
            .values()
            .map(arrows::Arrow::chunk)
            .chain(self.orbs.values().map(experience::XpOrb::chunk))
            .chain(
                self.falling_blocks
                    .values()
                    .map(falling::FallingBlock::chunk),
            )
            .collect()
    }

    /// Takes them out of the world with their chunk. Returns whether there were any.
    pub(super) fn unload_extras(&mut self, cx: i32, cz: i32) -> bool {
        let arrows: Vec<i32> = self
            .arrows
            .iter()
            .filter(|(_, a)| a.chunk() == (cx, cz) && !a.returning)
            .map(|(&id, _)| id)
            .collect();
        let orbs: Vec<i32> = self
            .orbs
            .iter()
            .filter(|(_, o)| o.chunk() == (cx, cz))
            .map(|(&id, _)| id)
            .collect();
        let falling: Vec<i32> = self
            .falling_blocks
            .iter()
            .filter(|(_, f)| f.chunk() == (cx, cz))
            .map(|(&id, _)| id)
            .collect();
        let any = !(arrows.is_empty() && orbs.is_empty() && falling.is_empty());
        for id in arrows {
            self.arrows.remove(&id);
            self.broadcast(items::remove_entity(id), None);
        }
        for id in orbs {
            self.orbs.remove(&id);
            self.broadcast(items::remove_entity(id), None);
        }
        for id in falling {
            self.falling_blocks.remove(&id);
            self.broadcast(items::remove_entity(id), None);
        }
        any
    }

    /// Brings one back from a loaded chunk's entities. Returns whether it was one.
    pub(super) fn load_extra(&mut self, nbt: &Compound) -> bool {
        let owner = nbt.int_array("Owner").and_then(uuid_of).and_then(|u| {
            self.players
                .iter()
                .find(|(_, p)| p.profile.uuid == u)
                .map(|(&id, _)| id)
        });
        if let Some(arrow) = arrows::Arrow::from_nbt(nbt, owner) {
            let id = next_entity_id();
            for p in arrows::spawn_saved(id, &arrow) {
                self.broadcast(p, None);
            }
            self.mob_chunks.insert(arrow.chunk());
            self.arrows.insert(id, arrow);
            return true;
        }
        if let Some(orb) = experience::XpOrb::from_nbt(nbt) {
            let id = next_entity_id();
            for p in experience::spawn_saved(id, &orb) {
                self.broadcast(p, None);
            }
            self.mob_chunks.insert(orb.chunk());
            self.orbs.insert(id, orb);
            return true;
        }
        if let Some(f) = falling::FallingBlock::from_nbt(nbt) {
            // Shown by `track_ground_entities`.
            self.mob_chunks.insert(f.chunk());
            self.falling_blocks.insert(next_entity_id(), f);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_orbs_and_falling_blocks_are_saved_with_their_chunk() {
        let dir = std::env::temp_dir().join(format!(
            "levitas-extras-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let new_world = || {
            World::new(
                dir.clone(),
                NewWorld {
                    kind: WorldKind::Flat,
                    seed: Some(0),
                },
            )
            .0
        };
        let mut w = new_world();
        w.chunk(0, 0);
        // An arrow stuck in the ground, an orb and a falling block, all in chunk 0,0.
        w.shoot_arrow(
            0,
            false,
            arrows::Shot {
                from: [3.5, -59.5, 3.5],
                dir: [0.0, -1.0, 0.0],
                speed: 1.0,
                base_damage: 2.0,
                ..Default::default()
            },
        );
        for _ in 0..5 {
            w.tick_arrows();
        }
        w.spawn_xp([6.5, -60.0, 6.5], 7);
        let sand = BlockInfo::by_name("minecraft:sand").unwrap().default_state;
        let above = BlockPos::new(9, -50, 9);
        w.set_block(above, sand);
        w.spawn_falling_block(above, sand);
        assert_eq!(w.arrows.len(), 1);
        assert!(!w.orbs.is_empty());
        assert_eq!(w.falling_blocks.len(), 1);
        w.save_mobs();

        let mut w = new_world();
        w.chunk(0, 0);
        assert_eq!(w.arrows.len(), 1);
        let arrow = w.arrows.values().next().unwrap();
        assert!(arrow.stuck.is_some(), "still stuck in the ground");
        let xp: i32 = w
            .orbs
            .values()
            .map(|o| o.to_nbt().int("Value").unwrap() as i32)
            .sum();
        assert_eq!(xp, 7);
        assert_eq!(w.falling_blocks.values().next().unwrap().state, sand);
    }
}
