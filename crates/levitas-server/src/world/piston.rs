//! Pistons and sticky pistons: pushing up to 12 blocks, breaking the fragile ones in
//! the way, and pulling one block back when sticky.
//!
//! Like vanilla, a piston answers power on any side but its front, and also power at
//! the block above it. Clients animate the move themselves from a block event, which
//! therefore goes out before the blocks change; the moving blocks sit as moving
//! pistons for two ticks before landing.

use levitas_registry::STATE_BLOCK;

use super::*;
use crate::blocks::piston::{is_piston, is_sticky, reaction, Reaction, PUSH_LIMIT};
use crate::blocks::{self, offset, prop, with, with_bool, Dir};

/// Ticks a piston takes to move.
const MOVE_TICKS: u64 = 2;
const EVENT_EXTEND: u8 = 0;
const EVENT_RETRACT: u8 = 1;

/// Blocks on their way, landing when `due`.
pub struct Landing {
    due: u64,
    pos: BlockPos,
    state: u16,
}

fn moving_piston(facing: Dir, sticky: bool) -> u16 {
    let base =
        BlockInfo::by_name("minecraft:moving_piston").map_or(block::AIR, |b| b.default_state);
    with(
        with(base, "facing", facing.name()),
        "type",
        if sticky { "sticky" } else { "normal" },
    )
}

fn piston_head(facing: Dir, sticky: bool) -> u16 {
    let base = BlockInfo::by_name("minecraft:piston_head").map_or(block::AIR, |b| b.default_state);
    with(
        with(with_bool(base, "short", false), "facing", facing.name()),
        "type",
        if sticky { "sticky" } else { "normal" },
    )
}

impl World {
    /// Whether a piston at `pos` facing `facing` is powered: from any side but its
    /// front, or at the block above it.
    fn piston_powered(&self, pos: BlockPos, facing: Dir) -> bool {
        let sides = Dir::ALL
            .into_iter()
            .filter(|&d| d != facing)
            .any(|d| self.power_from(offset(pos, d), d.opposite(), true) > 0);
        sides || self.power_at(offset(pos, Dir::Up)) > 0
    }

    /// A piston next to a change: it extends when powered, retracts when not.
    pub(super) fn react_piston(&mut self, pos: BlockPos, state: u16) {
        if self.landings.iter().any(|l| l.pos == pos) {
            return; // still moving
        }
        let Some(facing) = blocks::facing(state) else {
            return;
        };
        let extended = prop(state, "extended") == Some("true");
        let powered = self.piston_powered(pos, facing);
        if powered && !extended {
            self.extend(pos, state, facing);
        } else if !powered && extended {
            self.retract(pos, state, facing);
        }
    }

    /// The blocks a push moves (nearest first) and breaks, or `None` if it is stuck.
    fn push_plan(&self, from: BlockPos, facing: Dir) -> Option<(Vec<BlockPos>, Option<BlockPos>)> {
        let mut moved = Vec::new();
        let mut at = offset(from, facing);
        loop {
            let state = self.loaded_block(at)?;
            match reaction(state) {
                Reaction::Empty => return Some((moved, None)),
                Reaction::Destroy => return Some((moved, Some(at))),
                Reaction::Block => return None,
                Reaction::Push => {
                    if moved.len() == PUSH_LIMIT || !(MIN_Y..=MAX_Y).contains(&at.y) {
                        return None;
                    }
                    moved.push(at);
                    at = offset(at, facing);
                }
            }
        }
    }

    fn extend(&mut self, pos: BlockPos, state: u16, facing: Dir) {
        let Some((moved, broken)) = self.push_plan(pos, facing) else {
            return;
        };
        let end = moved
            .last()
            .map_or(offset(pos, facing), |&last| offset(last, facing));
        if !(MIN_Y..=MAX_Y).contains(&end.y) {
            return;
        }
        self.piston_event(pos, state, EVENT_EXTEND, facing);
        if let Some(broken) = broken {
            let old = self.block(broken);
            self.pop_block(broken, old);
        }
        let states: Vec<u16> = moved.iter().map(|&p| self.block(p)).collect();
        for &p in &moved {
            self.set_block(p, block::AIR);
        }
        let due = self.tick + MOVE_TICKS;
        for (&p, &s) in moved.iter().zip(&states).rev() {
            let to = offset(p, facing);
            self.set_block(to, moving_piston(facing, false));
            self.landings.push(Landing {
                due,
                pos: to,
                state: s,
            });
        }
        let sticky = is_sticky(state);
        let head = offset(pos, facing);
        self.set_block(head, moving_piston(facing, sticky));
        self.landings.push(Landing {
            due,
            pos: head,
            state: piston_head(facing, sticky),
        });
        self.set_block(pos, with_bool(state, "extended", true));
    }

    fn retract(&mut self, pos: BlockPos, state: u16, facing: Dir) {
        let sticky = is_sticky(state);
        self.piston_event(pos, state, EVENT_RETRACT, facing);
        let head = offset(pos, facing);
        if blocks::name(self.block(head)) == "minecraft:piston_head" {
            self.set_block(head, block::AIR);
        }
        let due = self.tick + MOVE_TICKS;
        let retracted = with_bool(state, "extended", false);
        self.set_block(pos, moving_piston(facing, sticky));
        self.landings.push(Landing {
            due,
            pos,
            state: retracted,
        });
        if !sticky {
            return;
        }
        // A sticky piston pulls the block in front of its head back with it.
        let pulled_from = offset(head, facing);
        let Some(pulled) = self.loaded_block(pulled_from) else {
            return;
        };
        if reaction(pulled) == Reaction::Push {
            self.set_block(pulled_from, block::AIR);
            self.set_block(head, moving_piston(facing, false));
            self.landings.push(Landing {
                due,
                pos: head,
                state: pulled,
            });
        }
    }

    /// Tells clients to animate a piston. Sent before the blocks change, since clients
    /// work the move out from the blocks they still see.
    fn piston_event(&mut self, pos: BlockPos, state: u16, action: u8, facing: Dir) {
        self.broadcast(
            Writer::packet(clientbound::BLOCK_EVENT)
                .position(pos)
                .u8(action)
                .u8(facing.face() as u8)
                .varint(i32::from(STATE_BLOCK[usize::from(state)])),
            None,
        );
    }

    /// Moving blocks land once their piston has finished moving.
    pub(super) fn tick_pistons(&mut self) {
        if self.landings.is_empty() {
            return;
        }
        let now = self.tick;
        let (done, waiting): (Vec<Landing>, Vec<Landing>) = std::mem::take(&mut self.landings)
            .into_iter()
            .partition(|l| l.due <= now);
        self.landings = waiting;
        for l in &done {
            if blocks::name(self.block(l.pos)) == "minecraft:moving_piston" {
                self.set_block(l.pos, l.state);
            }
        }
        for l in &done {
            self.update_neighbors(l.pos);
            self.check_falling(l.pos, l.state);
        }
        // A piston that finished moving looks at its power again.
        for l in done {
            if is_piston(l.state) {
                self.redstone_changed(l.pos);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let dir = std::env::temp_dir().join(format!(
            "levitas-piston-{}-{}",
            std::process::id(),
            next_entity_id()
        ));
        let (mut world, _) = World::new(
            dir,
            NewWorld {
                kind: WorldKind::Flat,
                seed: Some(0),
            },
        );
        for cx in -1..=1 {
            for cz in -1..=1 {
                world.chunk(cx, cz);
            }
        }
        world
    }

    fn state(name: &str) -> u16 {
        BlockInfo::by_name(name).unwrap().default_state
    }

    fn settle(w: &mut World, ticks: u64) {
        for _ in 0..ticks {
            w.tick += 1;
            w.tick_scheduled();
            w.tick_pistons();
            w.tick_redstone();
        }
    }

    #[test]
    fn pistons_push_rows_and_sticky_ones_pull_back() {
        let mut w = world();
        let y = -60;
        let piston = BlockPos::new(0, y, 0);
        w.set_block(
            piston,
            with(state("minecraft:sticky_piston"), "facing", "east"),
        );
        for x in 1..=3 {
            w.set_block(BlockPos::new(x, y, 0), block::STONE);
        }
        let torch = BlockPos::new(4, y, 0);
        w.set_block(torch, state("minecraft:torch"));
        settle(&mut w, 2);

        let lever = BlockPos::new(-1, y, 0);
        let on = with_bool(
            with(
                with(state("minecraft:lever"), "face", "wall"),
                "facing",
                "west",
            ),
            "powered",
            true,
        );
        w.set_block(lever, on);
        settle(&mut w, 4);
        assert_eq!(prop(w.block(piston), "extended"), Some("true"));
        assert_eq!(
            blocks::name(w.block(BlockPos::new(1, y, 0))),
            "minecraft:piston_head"
        );
        for x in 2..=4 {
            assert_eq!(w.block(BlockPos::new(x, y, 0)), block::STONE, "x = {x}");
        }
        assert!(!w.items.is_empty(), "the torch broke and dropped");

        w.set_block(lever, with_bool(on, "powered", false));
        settle(&mut w, 4);
        assert_eq!(prop(w.block(piston), "extended"), Some("false"));
        assert_eq!(w.block(BlockPos::new(1, y, 0)), block::STONE, "pulled back");
        assert!(is_air(w.block(BlockPos::new(2, y, 0))));
    }

    #[test]
    fn pistons_cannot_push_obsidian_or_more_than_twelve() {
        let mut w = world();
        let y = -60;
        let piston = BlockPos::new(0, y, 0);
        w.set_block(piston, with(state("minecraft:piston"), "facing", "east"));
        w.set_block(BlockPos::new(1, y, 0), state("minecraft:obsidian"));
        w.set_block(
            BlockPos::new(0, y + 1, 0),
            state("minecraft:redstone_block"),
        );
        settle(&mut w, 4);
        assert_eq!(prop(w.block(piston), "extended"), Some("false"));

        // Thirteen blocks are one too many; twelve go.
        let row = |w: &mut World, n: i32| {
            for x in 1..=13 {
                let s = if x <= n { block::STONE } else { block::AIR };
                w.set_block(BlockPos::new(x, y, 3), s);
            }
        };
        let second = BlockPos::new(0, y, 3);
        w.set_block(second, with(state("minecraft:piston"), "facing", "east"));
        row(&mut w, 13);
        w.set_block(
            BlockPos::new(0, y + 1, 3),
            state("minecraft:redstone_block"),
        );
        settle(&mut w, 4);
        assert_eq!(prop(w.block(second), "extended"), Some("false"));
        // Like vanilla, the piston only looks again when something next to it changes.
        row(&mut w, 12);
        w.set_block(BlockPos::new(0, y + 1, 3), block::AIR);
        settle(&mut w, 2);
        w.set_block(
            BlockPos::new(0, y + 1, 3),
            state("minecraft:redstone_block"),
        );
        settle(&mut w, 4);
        assert_eq!(prop(w.block(second), "extended"), Some("true"));
        assert_eq!(w.block(BlockPos::new(13, y, 3)), block::STONE);
    }
}
