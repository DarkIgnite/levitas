//! Placing, using and breaking blocks in the world, neighbour updates, and scheduled
//! block ticks. The rules themselves live in [`crate::blocks`].

use std::collections::VecDeque;

use levitas_nbt::{Compound, Tag};
use levitas_registry::{collision_boxes, sound_id, BlockInfo};

use super::*;
use crate::blocks::place::{place, PlaceContext};
use crate::blocks::{self, offset, prop, with, with_bool, Dir};

/// The most blocks one change may update in a chain, as a guard against loops.
const MAX_UPDATES: usize = 4096;
const SOUND_CATEGORY_BLOCKS: i32 = 4;
/// Ticks a button stays pressed.
const WOODEN_BUTTON_TICKS: u64 = 30;
const STONE_BUTTON_TICKS: u64 = 20;

/// A block update due at a given game tick (vanilla's "block ticks").
pub struct ScheduledTick {
    due: u64,
    pos: BlockPos,
    /// The block (any of its states) the tick is for; skipped if it changed meanwhile.
    block: u16,
}

impl World {
    pub(super) fn use_item_on(
        &mut self,
        id: i32,
        hand: i32,
        pos: BlockPos,
        face: i32,
        cursor: [f32; 3],
        sequence: i32,
    ) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let slot = p.hand_slot(hand);
        let stack = p.inventory.slots[slot];
        let (alive, survival) = (p.alive, p.survival());
        let (yaw, pitch, sneaking) = (p.yaw, p.pitch, p.sneaking);
        let Some(face) = Dir::from_face(face).filter(|_| alive) else {
            return self.ack(id, sequence);
        };

        // Using a container or a door works unless sneaking with something in hand.
        let clicked = self.block(pos);
        let may_use = hand == 0 && !(sneaking && !stack.is_empty());
        if may_use && (self.open_container(id, pos, clicked) || self.interact(id, pos, clicked)) {
            return self.ack(id, sequence);
        }

        if !stack.is_empty() && self.use_tool_on_block(id, slot, pos, face, clicked) {
            return self.ack(id, sequence);
        }

        let block_state = ItemInfo::get(stack.item).map_or(block::AIR, |i| i.block);
        if !stack.is_empty() && block_state != block::AIR {
            let ctx = PlaceContext {
                clicked: pos,
                face,
                cursor,
                yaw,
                pitch,
                sneaking,
            };
            let placed = place(block_state, &ctx, &mut |q| self.block(q));
            match placed {
                Some(blocks) if !self.obstructed(&blocks) => {
                    for &(q, state) in &blocks {
                        self.set_block(q, state);
                        if let Some(entity) = BlockEntity::for_block(state) {
                            self.block_entities.insert(q, entity);
                        }
                    }
                    for &(q, state) in &blocks {
                        self.update_neighbors(q);
                        self.check_falling(q, state);
                    }
                    if survival {
                        let p = self.players.get_mut(&id).expect("player exists");
                        p.inventory.take(slot, 1);
                        let update = p.inventory.set_slot_packet(slot);
                        self.send(id, update);
                    }
                }
                // Undo whatever the client predicted around the click.
                _ => self.resync_around(id, offset(pos, face)),
            }
        }
        self.ack(id, sequence);
    }

    /// Whether any of the blocks would overlap a player.
    fn obstructed(&self, blocks: &[(BlockPos, u16)]) -> bool {
        blocks.iter().any(|&(pos, state)| {
            collision_boxes(state).iter().any(|b| {
                let min = [
                    f64::from(pos.x) + f64::from(b[0]),
                    f64::from(pos.y) + f64::from(b[1]),
                    f64::from(pos.z) + f64::from(b[2]),
                ];
                let max = [
                    f64::from(pos.x) + f64::from(b[3]),
                    f64::from(pos.y) + f64::from(b[4]),
                    f64::from(pos.z) + f64::from(b[5]),
                ];
                self.players.values().filter(|p| p.alive).any(|p| {
                    let lo = [p.pos[0] - 0.3, p.pos[1], p.pos[2] - 0.3];
                    let hi = [p.pos[0] + 0.3, p.pos[1] + 1.8, p.pos[2] + 0.3];
                    (0..3).all(|i| min[i] < hi[i] && lo[i] < max[i])
                })
            })
        })
    }

    /// Sends the real blocks at and around `center` to a player whose prediction failed.
    fn resync_around(&mut self, id: i32, center: BlockPos) {
        let positions: Vec<BlockPos> = std::iter::once(center)
            .chain(Dir::ALL.map(|d| offset(center, d)))
            .collect();
        for pos in positions {
            let state = self.block(pos);
            self.send(id, block_update(pos, state));
        }
    }

    /// Opens, closes, flips or presses a block. Returns whether it was something to use.
    fn interact(&mut self, id: i32, pos: BlockPos, state: u16) -> bool {
        if self.use_bed(id, pos, state) {
            return true;
        }
        if !blocks::toggles(state) {
            return false;
        }
        let name = blocks::name(state);
        if block_has_tag(state, "minecraft:buttons") {
            if prop(state, "powered") != Some("true") {
                let delay = if block_has_tag(state, "minecraft:wooden_buttons") {
                    WOODEN_BUTTON_TICKS
                } else {
                    STONE_BUTTON_TICKS
                };
                self.set_block_updating(pos, with_bool(state, "powered", true));
                self.schedule(pos, state, delay);
                self.play_sound(&button_sound(name, true), pos, Some(id));
            }
            return true;
        }
        if name == "minecraft:lever" {
            let on = prop(state, "powered") != Some("true");
            self.set_block_updating(pos, with_bool(state, "powered", on));
            self.play_sound_pitched(
                "minecraft:block.lever.click",
                pos,
                Some(id),
                if on { 0.6 } else { 0.5 },
            );
            return true;
        }
        // Doors keep their open state on the lower half; the upper half follows it.
        let pos = if blocks::is_door(state) && prop(state, "half") == Some("upper") {
            offset(pos, Dir::Down)
        } else {
            pos
        };
        let state = self.block(pos);
        let opening = prop(state, "open") != Some("true");
        let mut new = with_bool(state, "open", opening);
        if block_has_tag(state, "minecraft:fence_gates") && opening {
            // A gate swings away from whoever opens it.
            let player = Dir::from_yaw(self.players[&id].yaw);
            if prop(state, "facing") == Some(player.opposite().name()) {
                new = with(new, "facing", player.name());
            }
        }
        self.set_block_updating(pos, new);
        self.play_sound(&toggle_sound(state, opening), pos, Some(id));
        true
    }

    /// Sets a block and lets its neighbours react.
    pub(super) fn set_block_updating(&mut self, pos: BlockPos, state: u16) {
        self.set_block(pos, state);
        self.update_neighbors(pos);
        self.check_falling(pos, state);
    }

    /// Re-evaluates the blocks around `origin` after it changed, and so on outwards for as
    /// long as blocks keep changing. Blocks that lose their support pop off with drops.
    pub(super) fn update_neighbors(&mut self, origin: BlockPos) {
        let mut queue: VecDeque<BlockPos> = Dir::ALL.map(|d| offset(origin, d)).into();
        let mut budget = MAX_UPDATES;
        while let Some(pos) = queue.pop_front() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            // Updates do not load chunks or reach outside the world.
            if !(MIN_Y..=MAX_Y).contains(&pos.y) || !self.chunks.contains_key(&pos.chunk()) {
                continue;
            }
            let old = self.block(pos);
            let new = blocks::updated(old, pos, &mut |q| self.block(q));
            if new == old {
                self.check_falling(pos, old);
                continue;
            }
            if is_air(new) {
                self.pop_block(pos, old);
            } else {
                self.set_block(pos, new);
            }
            self.check_falling(pos, new);
            queue.extend(Dir::ALL.map(|d| offset(pos, d)));
        }
    }

    pub(super) fn check_falling(&mut self, pos: BlockPos, state: u16) {
        if !self.chunks.contains_key(&pos.chunk()) {
            return;
        }
        let can_fall = if blocks::is_gravity_block(state) {
            pos.y > MIN_Y && blocks::can_fall_through(self.block(offset(pos, Dir::Down)))
        } else if blocks::is_stalactite(state) {
            pos.y > MIN_Y && !blocks::stalactite_supported(pos, &mut |q| self.block(q))
        } else {
            false
        };
        if can_fall {
            let first = BlockInfo::of(state).first_state;
            if !self
                .scheduled
                .iter()
                .any(|t| t.pos == pos && t.block == first)
            {
                self.schedule(pos, state, 2);
            }
        }
    }

    /// A block knocked off by an update: it disappears and drops its loot.
    pub(super) fn pop_block(&mut self, pos: BlockPos, state: u16) {
        self.set_block(pos, block::AIR);
        self.remove_block_entity(pos);
        self.broadcast(
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(LEVEL_EVENT_BLOCK_BREAK)
                .position(pos)
                .i32(i32::from(state))
                .bool(false),
            None,
        );
        let drops = self.loot.block_drops(&mut LootContext {
            state,
            tool: 0,
            rng: &mut self.rng,
            entity: Default::default(),
        });
        for (item, count) in drops {
            self.drop_at_block(pos, Stack::new(item, count));
        }
    }

    pub(super) fn schedule(&mut self, pos: BlockPos, state: u16, delay: u64) {
        self.scheduled.push(ScheduledTick {
            due: self.tick + delay,
            pos,
            block: BlockInfo::of(state).first_state,
        });
    }

    pub(super) fn tick_scheduled(&mut self) {
        let now = self.tick;
        let (due, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut self.scheduled)
            .into_iter()
            .partition(|t| t.due <= now);
        self.scheduled = pending;
        for tick in due {
            if !self.chunks.contains_key(&tick.pos.chunk()) {
                continue;
            }
            let state = self.block(tick.pos);
            if BlockInfo::of(state).first_state != tick.block {
                continue;
            }
            if block_has_tag(state, "minecraft:buttons") && prop(state, "powered") == Some("true") {
                self.set_block_updating(tick.pos, with_bool(state, "powered", false));
                self.play_sound(&button_sound(blocks::name(state), false), tick.pos, None);
            }
            let can_fall = if blocks::is_gravity_block(state) {
                tick.pos.y > MIN_Y
                    && blocks::can_fall_through(self.block(offset(tick.pos, Dir::Down)))
            } else if blocks::is_stalactite(state) {
                tick.pos.y > MIN_Y
                    && !blocks::stalactite_supported(tick.pos, &mut |q| self.block(q))
            } else {
                false
            };
            if can_fall {
                self.spawn_falling_block(tick.pos, state);
            }
            if crate::blocks::fire::is_fire(state) {
                self.fire_tick(tick.pos, state);
            }
        }
    }

    /// Scheduled ticks in a chunk, in the vanilla `block_ticks` form.
    pub(super) fn scheduled_ticks_in(&self, cx: i32, cz: i32) -> Vec<Compound> {
        self.scheduled
            .iter()
            .filter(|t| t.pos.chunk() == (cx, cz))
            .map(|t| {
                Compound::new()
                    .with("i", BlockInfo::of(t.block).name)
                    .with("x", t.pos.x)
                    .with("y", t.pos.y)
                    .with("z", t.pos.z)
                    .with("t", t.due.saturating_sub(self.tick) as i32)
                    .with("p", 0)
            })
            .collect()
    }

    /// Restores scheduled ticks saved with a chunk.
    pub(super) fn load_scheduled_ticks(&mut self, ticks: &[Tag]) {
        for tick in ticks.iter().filter_map(Tag::as_compound) {
            let (Some(name), Some(x), Some(y), Some(z)) =
                (tick.str("i"), tick.int("x"), tick.int("y"), tick.int("z"))
            else {
                continue;
            };
            let Some(info) = BlockInfo::by_name(name) else {
                continue;
            };
            self.scheduled.push(ScheduledTick {
                due: self.tick + tick.int("t").unwrap_or(0).max(0) as u64,
                pos: BlockPos::new(x as i32, y as i32, z as i32),
                block: info.first_state,
            });
        }
    }

    /// Forgets scheduled ticks of an unloaded chunk (they were saved with it).
    pub(super) fn drop_scheduled_ticks(&mut self, cx: i32, cz: i32) {
        self.scheduled.retain(|t| t.pos.chunk() != (cx, cz));
    }

    pub(super) fn play_sound(&mut self, name: &str, pos: BlockPos, except: Option<i32>) {
        let pitch = self.rng.f64() as f32 * 0.1 + 0.9;
        self.play_sound_pitched(name, pos, except, pitch);
    }

    /// Plays a sound at a block for everyone but `except` (who already heard it, having
    /// predicted the action).
    fn play_sound_pitched(&mut self, name: &str, pos: BlockPos, except: Option<i32>, pitch: f32) {
        let centre = [pos.x, pos.y, pos.z].map(|v| f64::from(v) + 0.5);
        self.play_sound_at(name, centre, 1.0, pitch, except);
    }

    /// Plays a block sound at a point for everyone but `except`.
    pub(super) fn play_sound_at(
        &mut self,
        name: &str,
        at: [f64; 3],
        volume: f32,
        pitch: f32,
        except: Option<i32>,
    ) {
        let Some(sound) = sound_id(name) else {
            return;
        };
        let seed = self.rng.next_u64() as i64;
        let fixed = |v: f64| (v * 8.0) as i32; // positions are sent in 1/8 blocks
        self.broadcast(
            Writer::packet(clientbound::SOUND)
                .varint(sound + 1)
                .varint(SOUND_CATEGORY_BLOCKS)
                .i32(fixed(at[0]))
                .i32(fixed(at[1]))
                .i32(fixed(at[2]))
                .f32(volume)
                .f32(pitch)
                .i64(seed),
            except,
        );
    }
}

/// The wood (or metal) family a door, trapdoor, gate or button sounds like.
fn sound_family(name: &str) -> &'static str {
    if name.contains("bamboo") {
        "bamboo_wood"
    } else if name.contains("cherry") {
        "cherry_wood"
    } else if name.contains("crimson") || name.contains("warped") {
        "nether_wood"
    } else if name.contains("copper") {
        "copper"
    } else if name.contains("iron") {
        "iron"
    } else {
        "wooden"
    }
}

fn toggle_sound(state: u16, opening: bool) -> String {
    let name = blocks::name(state);
    let family = sound_family(name);
    let action = if opening { "open" } else { "close" };
    let kind = if blocks::is_door(state) {
        format!("{family}_door")
    } else if block_has_tag(state, "minecraft:trapdoors") {
        format!("{family}_trapdoor")
    } else if family == "wooden" {
        "fence_gate".to_owned()
    } else {
        format!("{family}_fence_gate")
    };
    format!("minecraft:block.{kind}.{action}")
}

fn button_sound(name: &str, on: bool) -> String {
    let family = match sound_family(name) {
        "wooden" if name.contains("stone") || name.contains("blackstone") => "stone",
        family => family,
    };
    let action = if on { "click_on" } else { "click_off" };
    format!("minecraft:block.{family}_button.{action}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_exist() {
        let door = BlockInfo::by_name("minecraft:cherry_door")
            .unwrap()
            .default_state;
        assert!(sound_id(&toggle_sound(door, true)).is_some());
        let gate = BlockInfo::by_name("minecraft:oak_fence_gate")
            .unwrap()
            .default_state;
        assert!(sound_id(&toggle_sound(gate, false)).is_some());
        let trapdoor = BlockInfo::by_name("minecraft:warped_trapdoor")
            .unwrap()
            .default_state;
        assert!(sound_id(&toggle_sound(trapdoor, true)).is_some());
        for button in [
            "minecraft:stone_button",
            "minecraft:oak_button",
            "minecraft:polished_blackstone_button",
        ] {
            assert!(sound_id(&button_sound(button, true)).is_some(), "{button}");
        }
    }
}
