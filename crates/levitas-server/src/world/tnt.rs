//! Primed TNT: lit by flint and steel, fire or another explosion, it hops, falls, and
//! explodes with power 4 when its fuse runs out (4 seconds, or half a second to a
//! second and a half when another explosion lit it), as the Minecraft Wiki describes.

use levitas_registry::entity_type;

use super::*;

/// Ticks a lit TNT block burns before it explodes.
pub const FUSE: u32 = 80;
const POWER: f32 = 4.0;
const GRAVITY: f64 = 0.04;
const DRAG: f64 = 0.98;
const HALF_WIDTH: f64 = 0.49;
const HEIGHT: f64 = 0.98;
const DATA_FUSE: u8 = 8;
const SERIALIZER_INT: i32 = 1;

pub struct PrimedTnt {
    pos: [f64; 3],
    vel: [f64; 3],
    fuse: u32,
    sent_pos: [f64; 3],
    age: u32,
}

impl PrimedTnt {
    /// Adds speed, as an explosion's push does.
    pub fn push(&mut self, by: [f64; 3]) {
        for (v, p) in self.vel.iter_mut().zip(by) {
            *v += p;
        }
    }

    pub fn pos(&self) -> [f64; 3] {
        self.pos
    }
}

/// The packets that show a primed TNT to a player.
pub fn spawn_packets(id: i32, tnt: &PrimedTnt) -> [Writer; 2] {
    let vel = |v: f64| (v.clamp(-3.9, 3.9) * 8000.0) as i16;
    [
        Writer::packet(clientbound::ADD_ENTITY)
            .varint(id)
            .uuid(id as u128)
            .varint(entity_type::TNT)
            .f64(tnt.pos[0])
            .f64(tnt.pos[1])
            .f64(tnt.pos[2])
            .u8(0)
            .u8(0)
            .u8(0)
            .varint(0)
            .i16(vel(tnt.vel[0]))
            .i16(vel(tnt.vel[1]))
            .i16(vel(tnt.vel[2])),
        Writer::packet(clientbound::SET_ENTITY_DATA)
            .varint(id)
            .u8(DATA_FUSE)
            .varint(SERIALIZER_INT)
            .varint(tnt.fuse as i32)
            .u8(0xFF),
    ]
}

impl World {
    /// Lights the TNT block that was at `pos` (already removed): it hops a little in a
    /// random direction and explodes after `fuse` ticks.
    pub(super) fn prime_tnt(&mut self, pos: BlockPos, fuse: u32) {
        let angle = self.rng.f64() * std::f64::consts::TAU;
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let tnt = PrimedTnt {
            pos: at,
            vel: [-angle.sin() * 0.02, 0.2, -angle.cos() * 0.02],
            fuse,
            sent_pos: at,
            age: 0,
        };
        let id = next_entity_id();
        for packet in spawn_packets(id, &tnt) {
            self.broadcast(packet, None);
        }
        self.play_sound("minecraft:entity.tnt.primed", pos, None);
        self.tnt.insert(id, tnt);
    }

    /// TNT lit by an explosion goes off sooner, after half a second to a second and a half.
    pub(super) fn prime_tnt_by_explosion(&mut self, pos: BlockPos) {
        let fuse = FUSE / 8 + self.rng.below(FUSE / 4);
        self.prime_tnt(pos, fuse);
    }

    pub(super) fn tick_tnt(&mut self) {
        let ids: Vec<i32> = self.tnt.keys().copied().collect();
        for id in ids {
            let mut tnt = self.tnt.remove(&id).expect("listed");
            let cell = BlockPos::new(
                tnt.pos[0].floor() as i32,
                tnt.pos[1].floor() as i32,
                tnt.pos[2].floor() as i32,
            );
            if !self.chunks.contains_key(&cell.chunk()) {
                self.tnt.insert(id, tnt);
                continue; // frozen while nobody is near
            }
            tnt.age += 1;
            tnt.vel[1] -= GRAVITY;
            let dy = self.clip_move(tnt.pos, HALF_WIDTH, HEIGHT, 1, tnt.vel[1]);
            let on_ground = tnt.vel[1] < 0.0 && dy > tnt.vel[1] + 1e-9;
            if (dy - tnt.vel[1]).abs() > 1e-9 {
                tnt.vel[1] = 0.0;
            }
            tnt.pos[1] += dy;
            for axis in [0, 2] {
                let d = self.clip_move(tnt.pos, HALF_WIDTH, HEIGHT, axis, tnt.vel[axis]);
                if (d - tnt.vel[axis]).abs() > 1e-9 {
                    tnt.vel[axis] = 0.0;
                }
                tnt.pos[axis] += d;
            }
            tnt.vel = tnt.vel.map(|v| v * DRAG);
            if on_ground {
                tnt.vel[0] *= 0.7;
                tnt.vel[2] *= 0.7;
                tnt.vel[1] *= -0.5;
            }

            tnt.fuse = tnt.fuse.saturating_sub(1);
            if tnt.fuse == 0 || tnt.pos[1] < f64::from(MIN_Y - 64) {
                self.broadcast(items::remove_entity(id), None);
                if tnt.fuse == 0 {
                    let centre = [tnt.pos[0], tnt.pos[1] + HEIGHT / 16.0, tnt.pos[2]];
                    self.explode(centre, POWER, "TNT");
                }
                continue;
            }

            // Clients move TNT themselves; correct them now and then.
            let moved: f64 = (0..3).map(|i| (tnt.pos[i] - tnt.sent_pos[i]).powi(2)).sum();
            if tnt.age.is_multiple_of(5) && moved > 0.01 {
                tnt.sent_pos = tnt.pos;
                self.broadcast(
                    Writer::packet(clientbound::ENTITY_POSITION_SYNC)
                        .varint(id)
                        .f64(tnt.pos[0])
                        .f64(tnt.pos[1])
                        .f64(tnt.pos[2])
                        .f64(tnt.vel[0])
                        .f64(tnt.vel[1])
                        .f64(tnt.vel[2])
                        .f32(0.0)
                        .f32(0.0)
                        .bool(on_ground),
                    None,
                );
            }
            self.tnt.insert(id, tnt);
        }
    }
}
