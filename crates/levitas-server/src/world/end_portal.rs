//! Eyes of ender, End portals and getting to and from the End, following the Minecraft
//! Wiki.
//!
//! An eye of ender thrown in the overworld floats towards the nearest stronghold for
//! four seconds, then breaks (one time in five it drops instead). Put in the twelve
//! frames of a stronghold's portal room, facing in, eyes open the portal. Stepping in
//! takes a player to the End's obsidian platform; the End's own portal sends them
//! back to their spawn point.

use levitas_registry::entity_type;

use super::dimension::Dimension;
use super::travel::Via;
use super::*;
use crate::blocks::{self, prop, with, Dir};

/// Where players arrive in the End: standing on a 5 x 5 obsidian platform.
const END_PLATFORM: [i32; 3] = [100, 49, 0];
const EYE_LIFETIME: u32 = 80;
const EYE_SPEED: f64 = 0.4;
const LEVEL_EVENT_EYE_BREAKS: i32 = 2003;
const LEVEL_EVENT_FRAME_FILLED: i32 = 1503;
const LEVEL_EVENT_END_PORTAL_OPENED: i32 = 1038;

pub struct EyeOfEnder {
    pos: [f64; 3],
    /// The stronghold it is drawn to.
    target: [f64; 3],
    age: u32,
}

fn eye_spawn(id: i32, eye: &EyeOfEnder) -> Writer {
    Writer::packet(clientbound::ADD_ENTITY)
        .varint(id)
        .uuid(id as u128)
        .varint(entity_type::EYE_OF_ENDER)
        .f64(eye.pos[0])
        .f64(eye.pos[1])
        .f64(eye.pos[2])
        .u8(0)
        .u8(0)
        .u8(0)
        .varint(0)
        .i16(0)
        .i16(0)
        .i16(0)
}

impl World {
    /// Throws the eye of ender player `id` holds in `hand`. Returns whether it held one.
    pub(super) fn throw_eye(&mut self, id: i32, hand: i32) -> bool {
        let Some(p) = self.players.get(&id) else {
            return false;
        };
        let slot = p.hand_slot(hand);
        let stack = p.inventory.slots[slot];
        if ItemInfo::get(stack.item).is_none_or(|i| i.name != "minecraft:ender_eye") {
            return false;
        }
        if !p.alive || stack.is_empty() || self.dimension != Dimension::Overworld {
            return true;
        }
        let (eye, _, _) = p.eye_ray();
        let survival = p.survival();
        let stronghold = match self
            .terrain
            .stronghold_near(eye[0].floor() as i32, eye[2].floor() as i32)
        {
            Ok(Some(found)) => found,
            Ok(None) => return true,
            Err(()) => {
                // The first search takes a moment; the eye stays in hand meanwhile.
                self.send(
                    id,
                    system_chat("The eye of ender is still sensing... try again"),
                );
                return true;
            }
        };
        let thrown = EyeOfEnder {
            pos: eye,
            target: [
                f64::from(stronghold[0]) + 0.5,
                f64::from(stronghold[1]),
                f64::from(stronghold[2]) + 0.5,
            ],
            age: 0,
        };
        let eye_id = next_entity_id();
        self.broadcast(eye_spawn(eye_id, &thrown), None);
        self.eyes.insert(eye_id, thrown);
        self.play_sound_at("minecraft:entity.ender_eye.launch", eye, 0.5, 0.4, None);
        if survival {
            let p = self.players.get_mut(&id).expect("player exists");
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        true
    }

    /// Eyes of ender in flight, for a player who just arrived.
    pub(super) fn eye_packets(&self) -> Vec<Writer> {
        self.eyes.iter().map(|(&id, e)| eye_spawn(id, e)).collect()
    }

    /// Eyes float up and towards their stronghold, then break or drop.
    pub(super) fn tick_eyes(&mut self) {
        let ids: Vec<i32> = self.eyes.keys().copied().collect();
        for id in ids {
            let Some(mut eye) = self.eyes.remove(&id) else {
                continue;
            };
            eye.age += 1;
            if eye.age > EYE_LIFETIME {
                self.broadcast(items::remove_entity(id), None);
                if self.rng.below(5) == 0 {
                    if let Some(item) = ItemInfo::id_of("minecraft:ender_eye") {
                        self.spawn_dropped(Stack::new(item, 1), eye.pos, [0.0; 3]);
                    }
                } else {
                    let at = BlockPos::new(
                        eye.pos[0].floor() as i32,
                        eye.pos[1].floor() as i32,
                        eye.pos[2].floor() as i32,
                    );
                    self.broadcast(
                        Writer::packet(clientbound::LEVEL_EVENT)
                            .i32(LEVEL_EVENT_EYE_BREAKS)
                            .position(at)
                            .i32(0)
                            .bool(false),
                        None,
                    );
                    self.play_sound_at("minecraft:entity.ender_eye.death", eye.pos, 1.0, 1.0, None);
                }
                continue;
            }
            // Far away, it heads for a point 12 blocks along and 8 up; close, straight in.
            let (dx, dz) = (eye.target[0] - eye.pos[0], eye.target[2] - eye.pos[2]);
            let horizontal = dx.hypot(dz);
            let goal_y = if horizontal > 12.0 {
                eye.pos[1] + 8.0
            } else {
                eye.target[1]
            };
            let speed = EYE_SPEED.min(horizontal);
            let step = if horizontal > 1e-6 {
                [dx / horizontal * speed, dz / horizontal * speed]
            } else {
                [0.0, 0.0]
            };
            let rise = ((goal_y - eye.pos[1]) * 0.1).clamp(-0.2, 0.2);
            eye.pos = [
                eye.pos[0] + step[0],
                eye.pos[1] + rise,
                eye.pos[2] + step[1],
            ];
            self.broadcast(
                Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                    .varint(id)
                    .f64(eye.pos[0])
                    .f64(eye.pos[1])
                    .f64(eye.pos[2])
                    .f64(step[0])
                    .f64(rise)
                    .f64(step[1])
                    .f32(0.0)
                    .f32(0.0)
                    .bool(false),
                None,
            );
            self.eyes.insert(id, eye);
        }
    }

    /// An eye of ender put in an End portal frame. Returns whether one went in.
    pub(super) fn fill_frame(&mut self, id: i32, slot: usize, pos: BlockPos) -> bool {
        let state = self.block(pos);
        if blocks::name(state) != "minecraft:end_portal_frame" || prop(state, "eye") == Some("true")
        {
            return false;
        }
        let Some(p) = self.players.get_mut(&id) else {
            return false;
        };
        if ItemInfo::get(p.inventory.slots[slot].item)
            .is_none_or(|i| i.name != "minecraft:ender_eye")
        {
            return false;
        }
        if p.survival() {
            p.inventory.take(slot, 1);
            let update = p.inventory.set_slot_packet(slot);
            self.send(id, update);
        }
        self.set_block(pos, with(state, "eye", "true"));
        self.broadcast(
            Writer::packet(clientbound::LEVEL_EVENT)
                .i32(LEVEL_EVENT_FRAME_FILLED)
                .position(pos)
                .i32(0)
                .bool(false),
            None,
        );
        self.try_open_end_portal(pos);
        true
    }

    /// Opens the portal if the frame at `pos` completes a ring of twelve frames, all
    /// with eyes and facing in, around a 3 x 3 opening.
    pub(super) fn try_open_end_portal(&mut self, pos: BlockPos) -> bool {
        for cx in pos.x - 2..=pos.x + 2 {
            for cz in pos.z - 2..=pos.z + 2 {
                let centre = BlockPos::new(cx, pos.y, cz);
                if self.end_frame_ring(centre) {
                    let portal = BlockInfo::by_name("minecraft:end_portal")
                        .expect("End portals exist")
                        .default_state;
                    for x in cx - 1..=cx + 1 {
                        for z in cz - 1..=cz + 1 {
                            self.set_block(BlockPos::new(x, pos.y, z), portal);
                        }
                    }
                    self.broadcast(
                        Writer::packet(clientbound::LEVEL_EVENT)
                            .i32(LEVEL_EVENT_END_PORTAL_OPENED)
                            .position(centre)
                            .i32(0)
                            .bool(true), // heard everywhere
                        None,
                    );
                    return true;
                }
            }
        }
        false
    }

    fn end_frame_ring(&mut self, centre: BlockPos) -> bool {
        let sides = [
            (Dir::North, Dir::South),
            (Dir::South, Dir::North),
            (Dir::West, Dir::East),
            (Dir::East, Dir::West),
        ];
        sides.into_iter().all(|(side, inwards)| {
            let s = side.step();
            (-1..=1).all(|along| {
                let (x, z) = if s[0] == 0 {
                    (centre.x + along, centre.z + 2 * s[2])
                } else {
                    (centre.x + 2 * s[0], centre.z + along)
                };
                let state = self.block(BlockPos::new(x, centre.y, z));
                blocks::name(state) == "minecraft:end_portal_frame"
                    && prop(state, "eye") == Some("true")
                    && blocks::facing(state) == Some(inwards)
            })
        })
    }

    /// The End's arrival platform, built (or cleared) for each arrival.
    pub(super) fn end_platform(&mut self) -> [f64; 3] {
        let [x0, y0, z0] = END_PLATFORM;
        let obsidian = BlockInfo::by_name("minecraft:obsidian")
            .expect("obsidian exists")
            .default_state;
        for x in x0 - 2..=x0 + 2 {
            for z in z0 - 2..=z0 + 2 {
                self.set_block(BlockPos::new(x, y0 - 1, z), obsidian);
                for y in y0..y0 + 3 {
                    self.set_block(BlockPos::new(x, y, z), block::AIR);
                }
            }
        }
        [f64::from(x0) + 0.5, f64::from(y0), f64::from(z0) + 0.5]
    }

    /// Players stepping into an End portal go through at once.
    pub(super) fn tick_end_portals(&mut self) {
        let entering: Vec<i32> = self
            .players
            .iter()
            .filter(|(_, p)| p.alive && p.portal_cooldown == 0)
            .filter(|(_, p)| {
                let feet = BlockPos::new(
                    p.pos[0].floor() as i32,
                    p.pos[1].floor() as i32,
                    p.pos[2].floor() as i32,
                );
                self.loaded_block(feet)
                    .is_some_and(|s| blocks::name(s) == "minecraft:end_portal")
            })
            .map(|(&id, _)| id)
            .collect();
        for id in entering {
            let to = if self.dimension == Dimension::End {
                Dimension::Overworld
            } else {
                Dimension::End
            };
            self.hand_over(id, to, None, Via::EndPortal);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Profile;

    fn temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "levitas-end-{}-{}",
            std::process::id(),
            next_entity_id()
        ))
    }

    fn flat() -> NewWorld {
        NewWorld {
            kind: WorldKind::Flat,
            seed: Some(0),
        }
    }

    fn alice(world: &mut World, at: [f64; 3]) -> i32 {
        let (out, _frames) = mpsc::channel(100_000);
        let id = next_entity_id();
        let profile = Profile {
            name: "Alice".to_owned(),
            uuid: 1,
        };
        world.players.insert(id, Player::new(profile, out, at));
        id
    }

    #[test]
    fn twelve_eyes_facing_in_open_the_portal() {
        let (mut w, _) = World::new(temp_dir(), flat());
        w.chunk(0, 0);
        let frame = BlockInfo::by_name("minecraft:end_portal_frame")
            .unwrap()
            .default_state;
        let y = -60;
        let (cx, cz) = (8, 8);
        let mut last = None;
        for (side, inwards) in [
            (Dir::North, "south"),
            (Dir::South, "north"),
            (Dir::West, "east"),
            (Dir::East, "west"),
        ] {
            let s = side.step();
            for along in -1..=1 {
                let (x, z) = if s[0] == 0 {
                    (cx + along, cz + 2 * s[2])
                } else {
                    (cx + 2 * s[0], cz + along)
                };
                let pos = BlockPos::new(x, y, z);
                let facing = with(frame, "facing", inwards);
                let filled = last.is_some();
                w.set_block(
                    pos,
                    with(facing, "eye", if filled { "true" } else { "false" }),
                );
                if !filled {
                    last = Some(pos);
                }
            }
        }
        let id = alice(&mut w, [8.5, -60.0, 4.5]);
        let slot = w.players[&id].hand_slot(0);
        let eye = ItemInfo::id_of("minecraft:ender_eye").unwrap();
        w.players.get_mut(&id).unwrap().inventory.slots[slot] = Stack::new(eye, 1);
        assert!(w.fill_frame(id, slot, last.unwrap()));
        assert_eq!(
            blocks::name(w.block(BlockPos::new(cx, y, cz))),
            "minecraft:end_portal"
        );
    }

    #[tokio::test]
    async fn end_portals_take_players_to_the_platform() {
        let dir = temp_dir();
        let (mut overworld, _) = World::new(dir.clone(), flat());
        let (mut end, _) = World::new_in(dir, flat(), Dimension::End);
        let (tx, mut rx) = mpsc::channel(16);
        overworld.links.insert(Dimension::End, tx);
        overworld.chunk(0, 0);
        let portal = BlockInfo::by_name("minecraft:end_portal")
            .unwrap()
            .default_state;
        overworld.set_block(BlockPos::new(3, -60, 3), portal);
        let id = alice(&mut overworld, [3.5, -60.0, 3.5]);
        overworld.tick_end_portals();
        let Some(Event::Arrive(arrival)) = rx.recv().await else {
            panic!("no arrival");
        };
        end.arrive(*arrival);
        assert_eq!(end.players[&id].pos, [100.5, 49.0, 0.5]);
        assert_eq!(
            blocks::name(end.block(BlockPos::new(100, 48, 0))),
            "minecraft:obsidian"
        );
    }
}
