//! Hostile mobs: hunting players, spawning in the dark, burning in daylight and
//! despawning far from players.
//!
//! Behaviour follows the Minecraft Wiki: monsters spawn where the block light is 0 and
//! the sky light (dimmed at night) is low, at least 24 blocks from any player, up to 70
//! around each player. They despawn at once beyond 128 blocks and now and then beyond 32.
//! Zombies burn in sunlight; spiders only hunt in the dark; creepers hiss for 1.5
//! seconds next to their target and explode.

use levitas_registry::collision_boxes;

use super::arrows::Shot;
use super::mobs::{Mob, MobKind};
use super::*;
use crate::blocks::fluid::{fluid_at, Fluid};
use crate::blocks::{sturdy, Dir};
use crate::world::survival::Damage;

/// Most monsters around one player.
const MONSTER_CAP: usize = 70;
/// Monsters spawn this many chunks around players, whatever the view distance.
const MOB_SPAWN_CHUNKS: i32 = 8;
/// Monsters never spawn this close to a player.
const SPAWN_DISTANCE: f64 = 24.0;
/// Spawn attempts per player per tick.
const SPAWN_ATTEMPTS: usize = 3;
/// Ticks between two melee attacks.
const ATTACK_TICKS: u32 = 20;
/// Ticks a creeper hisses before exploding, and how far its target may get away.
const FUSE_TICKS: u32 = 30;
const FUSE_CANCEL_DISTANCE: f64 = 7.0;
const CREEPER_POWER: f32 = 3.0;
const KNOCKBACK: f64 = 0.4;
/// Skeletons: how close they come, how far they shoot, ticks between shots, and how
/// fast and how wildly (on Normal difficulty) their arrows fly.
const SKELETON_KEEP_DISTANCE: f64 = 12.0;
const SKELETON_RANGE: f64 = 15.0;
const SKELETON_SHOT_TICKS: u32 = 40;
const SKELETON_ARROW_SPEED: f64 = 1.6;
const SKELETON_INACCURACY: f64 = 6.0;

const DATA_FLAGS: u8 = 0;
const ON_FIRE: u8 = 0x01;
const DATA_CREEPER_SWELL: u8 = 16;
const SERIALIZER_BYTE: i32 = 0;
const SERIALIZER_INT: i32 = 1;

impl World {
    /// How much the night (and rain) dims sky light: 0 at noon up to 11 at midnight.
    pub(super) fn sky_darken(&self) -> i32 {
        // The sun's angle as the client computes it from the time of day.
        let d = ((self.day_time.rem_euclid(24_000) as f64) / 24_000.0 - 0.25).rem_euclid(1.0);
        let e = 0.5 - (d * std::f64::consts::PI).cos() / 2.0;
        let angle = (d * 2.0 + e) / 3.0;
        let mut light = 1.0 - ((angle * std::f64::consts::TAU).cos() * 2.0 + 0.5).clamp(0.0, 1.0);
        light = 1.0 - light;
        light *= 1.0 - f64::from(self.weather.rain_level) * 5.0 / 16.0;
        light *= 1.0 - f64::from(self.weather.rain_level * self.weather.thunder_level) * 5.0 / 16.0;
        ((1.0 - light) * 11.0) as i32
    }

    /// Sky light and block light at a block, if its chunk is loaded.
    pub(super) fn light_at(&self, pos: BlockPos) -> Option<(u8, u8)> {
        let (sky, block) = self.raw_light_at(pos)?;
        Some((if self.dimension.has_sky() { sky } else { 0 }, block))
    }

    fn raw_light_at(&self, pos: BlockPos) -> Option<(u8, u8)> {
        if !(MIN_Y..=MAX_Y).contains(&pos.y) {
            return Some((15, 0));
        }
        let chunk = self.chunks.get(&pos.chunk())?;
        let section = ((pos.y - MIN_Y) >> 4) as usize + 1;
        let (x, y, z) = (
            (pos.x & 15) as usize,
            (pos.y & 15) as usize,
            (pos.z & 15) as usize,
        );
        Some((
            chunk.light.sky[section].get(x, y, z),
            chunk.light.block[section].get(x, y, z),
        ))
    }

    /// The light level a mob sees at `pos`: block light, or sky light dimmed by night.
    pub(super) fn light_level(&self, pos: BlockPos) -> i32 {
        let (sky, block) = self.light_at(pos).unwrap_or((0, 0));
        (i32::from(sky) - self.sky_darken()).max(i32::from(block))
    }

    /// A monster picks and chases its target, and attacks when close. Returns whether
    /// the monster is gone (a creeper that exploded).
    pub(super) fn hunt(&mut self, id: i32, mob: &mut Mob) -> bool {
        mob.attack_cooldown = mob.attack_cooldown.saturating_sub(1);
        if mob.kind == MobKind::Enderman {
            self.enderman_senses(id, mob);
        }
        // Zombified piglins and endermen leave players alone unless angered.
        if matches!(mob.kind, MobKind::ZombifiedPiglin | MobKind::Enderman) {
            mob.anger = mob.anger.saturating_sub(1);
            if mob.anger == 0 {
                mob.hunting = None;
                return false;
            }
        }
        let range = mob.kind.follow_range();
        let head = BlockPos::new(
            mob.pos[0].floor() as i32,
            (mob.pos[1] + mob.size().1 * 0.85).floor() as i32,
            mob.pos[2].floor() as i32,
        );
        let still_valid = mob
            .hunting
            .and_then(|pid| self.players.get(&pid))
            .is_some_and(|p| {
                p.alive && p.survival() && horizontal_distance(p.pos, mob.pos) <= range
            });
        if !still_valid {
            mob.hunting = None;
        }
        // Spiders only turn hostile in the dark (unless something provoked them).
        let hostile = mob.kind != MobKind::Spider || self.light_level(head) <= 7;
        if mob.hunting.is_none() && hostile {
            mob.hunting = self
                .players
                .iter()
                .filter(|(_, p)| p.alive && p.survival())
                .map(|(&pid, p)| (pid, horizontal_distance(p.pos, mob.pos)))
                .filter(|&(_, d)| d <= range)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(pid, _)| pid);
        }
        let Some(pid) = mob.hunting else {
            if mob.fuse.take().is_some() {
                self.send_to_viewers(mob, creeper_swell(id, false));
            }
            return false;
        };
        let target = self.players[&pid].pos;
        let distance = horizontal_distance(target, mob.pos);
        let dy = target[1] - mob.pos[1];
        mob.head_yaw = yaw_towards(mob.pos, target);
        if mob.kind == MobKind::Spider {
            // Spiders climb over whatever is in the way.
            mob.target = Some(([target[0], target[2]], 20));
        } else if mob.kind == MobKind::MagmaCube || mob.kind.flies() {
            // Magma cubes hop towards their prey, ghasts and blazes fly.
        } else {
            let goal = BlockPos::new(
                target[0].floor() as i32,
                (target[1] + 0.01).floor() as i32,
                target[2].floor() as i32,
            );
            self.follow_path(mob, goal);
        }

        if mob.kind == MobKind::Creeper {
            if mob.fuse.is_none() && distance < 3.0 && dy.abs() < 2.0 {
                mob.fuse = Some(0);
                self.send_to_viewers(mob, creeper_swell(id, true));
                self.play_sound_at("minecraft:entity.creeper.primed", mob.pos, 1.0, 0.5, None);
            } else if mob.fuse.is_some() && distance > FUSE_CANCEL_DISTANCE {
                mob.fuse = None;
                self.send_to_viewers(mob, creeper_swell(id, false));
            }
            if let Some(fuse) = mob.fuse.as_mut() {
                mob.target = None; // stands still while hissing
                *fuse += 1;
                if *fuse >= FUSE_TICKS {
                    self.send_to_viewers(mob, items::remove_entity(id));
                    self.explode(mob.pos, CREEPER_POWER, "Creeper");
                    return true;
                }
            }
            return false;
        }

        if mob.kind == MobKind::Skeleton {
            self.shoot_at(id, mob, target, distance);
            return false;
        }
        if mob.kind == MobKind::Ghast {
            self.ghast_attack(id, mob, target);
            return false;
        }
        // An enderman far from its prey now and then teleports closer.
        if mob.kind == MobKind::Enderman && distance > 12.0 && self.rng.below(40) == 0 {
            self.teleport_enderman(mob, target, 8.0);
        }
        if mob.kind == MobKind::Blaze && distance > 2.0 {
            self.blaze_attack(id, mob, target, distance);
            return false;
        }

        let (width, _) = mob.size();
        let reach = ((width * 2.0).powi(2) + 0.6).sqrt();
        if distance <= reach && dy.abs() < 1.5 && mob.attack_cooldown == 0 {
            mob.attack_cooldown = ATTACK_TICKS;
            self.send_to_viewers(mob, Writer::packet(clientbound::ANIMATE).varint(id).u8(0));
            let hurt = self.damage_from(
                pid,
                mob.attack_damage(),
                Damage::Mob(mob.kind.display()),
                Some(id),
            );
            // A blocked blow pushes nobody.
            if hurt {
                let yaw = f64::from(yaw_towards(mob.pos, target)).to_radians();
                self.push_player(pid, [-yaw.sin() * KNOCKBACK, 0.36, yaw.cos() * KNOCKBACK]);
            }
        }
        false
    }

    /// An enderman turns on a player who looks it in the eyes (unless they wear a carved
    /// pumpkin), and teleports away from water.
    fn enderman_senses(&mut self, id: i32, mob: &mut Mob) {
        let head = [mob.pos[0], mob.pos[1] + 2.55, mob.pos[2]];
        let feet = BlockPos::new(
            mob.pos[0].floor() as i32,
            mob.pos[1].floor() as i32,
            mob.pos[2].floor() as i32,
        );
        let wet = self
            .loaded_block_at(feet)
            .and_then(fluid_at)
            .is_some_and(|f| f.fluid == Fluid::Water)
            || (self.weather.is_raining()
                && self.dimension.has_sky()
                && self.light_at(feet).is_some_and(|(sky, _)| sky == 15));
        if wet {
            if self.tick.is_multiple_of(10) {
                self.hurt_mob(id, mob, 1.0, "minecraft:drown");
            }
            let at = mob.pos;
            self.teleport_enderman(mob, at, 32.0);
        }
        if mob.anger > 0 {
            return;
        }
        let pumpkin = ItemInfo::id_of("minecraft:carved_pumpkin");
        let staring = self
            .players
            .iter()
            .filter(|(_, p)| p.alive && p.survival())
            .filter(|(_, p)| Some(p.inventory.slots[5].item) != pumpkin)
            .find(|(_, p)| {
                let (eye, look, _) = p.eye_ray();
                let to = [0, 1, 2].map(|i| head[i] - eye[i]);
                let distance = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
                if distance > 64.0 || distance < 1e-6 {
                    return false;
                }
                let dot: f64 = (0..3).map(|i| look[i] * to[i] / distance).sum();
                dot > 1.0 - 0.025 / distance && self.line_of_sight(eye, head)
            })
            .map(|(&pid, _)| pid);
        if let Some(pid) = staring {
            mob.anger = super::mobs::ENDERMAN_ANGER;
            mob.hunting = Some(pid);
            self.play_sound_at("minecraft:entity.enderman.stare", mob.pos, 2.5, 1.0, None);
            self.send_to_viewers(
                mob,
                Writer::packet(clientbound::SET_ENTITY_DATA)
                    .varint(id)
                    .u8(17) // screaming
                    .varint(8) // boolean
                    .bool(true)
                    .u8(0xFF),
            );
        }
    }

    /// A ghast that sees its target charges for a second, crying out halfway, then
    /// spits a fireball, and waits two seconds before charging again.
    fn ghast_attack(&mut self, id: i32, mob: &mut Mob, target: [f64; 3]) {
        let (_, height) = mob.size();
        let eye = [mob.pos[0], mob.pos[1] + height / 2.0, mob.pos[2]];
        let chest = [target[0], target[1] + 0.9, target[2]];
        mob.yaw = yaw_towards(mob.pos, target);
        let sees = self.line_of_sight(eye, [target[0], target[1] + 1.62, target[2]]);
        if !sees {
            if mob.charge > 0 {
                mob.charge -= 1;
            }
        } else {
            mob.charge += 1;
        }
        if mob.charge == 10 {
            self.play_sound_at("minecraft:entity.ghast.warn", mob.pos, 10.0, 1.0, None);
            self.send_to_viewers(mob, ghast_charging(id, true));
        }
        if mob.charge >= 20 {
            let dir = [0, 1, 2].map(|i| chest[i] - eye[i]);
            let horizontal = dir[0].hypot(dir[2]).max(1e-9);
            // From the front of the ghast's face.
            let from = [
                eye[0] + dir[0] / horizontal * 4.0 * 0.5,
                eye[1],
                eye[2] + dir[2] / horizontal * 4.0 * 0.5,
            ];
            self.shoot_fireball(id, "Ghast", from, dir, true);
            self.play_sound_at("minecraft:entity.ghast.shoot", mob.pos, 10.0, 1.0, None);
            self.send_to_viewers(mob, ghast_charging(id, false));
            mob.charge = -40;
        }
    }

    /// A blaze in range glows for three seconds, then throws three small fireballs a
    /// third of a second apart, and rests for five seconds.
    fn blaze_attack(&mut self, id: i32, mob: &mut Mob, target: [f64; 3], distance: f64) {
        let eye = [mob.pos[0], mob.pos[1] + 1.5, mob.pos[2]];
        let sees = self.line_of_sight(eye, [target[0], target[1] + 1.62, target[2]]);
        mob.charge -= 1;
        if mob.charge > 0 || !sees {
            return;
        }
        match mob.volley {
            0 => {
                mob.volley = 3;
                mob.charge = 60;
                self.send_to_viewers(mob, blaze_glowing(id, true));
            }
            left => {
                let spread = distance.sqrt() * 0.5 * 0.1;
                let mut dir =
                    [0, 1, 2].map(|i| target[i] + if i == 1 { 0.9 } else { 0.0 } - eye[i]);
                for v in &mut dir {
                    *v += (self.rng.f64() * 2.0 - 1.0) * spread * 4.0;
                }
                self.shoot_fireball(id, "Blaze", eye, dir, false);
                self.play_sound_at("minecraft:entity.blaze.shoot", mob.pos, 1.0, 1.0, None);
                mob.volley = left - 1;
                if mob.volley == 0 {
                    mob.charge = 100;
                    self.send_to_viewers(mob, blaze_glowing(id, false));
                } else {
                    mob.charge = 6;
                }
            }
        }
    }

    /// A skeleton keeps its distance and shoots when it can see its target.
    fn shoot_at(&mut self, id: i32, mob: &mut Mob, target: [f64; 3], distance: f64) {
        if distance <= SKELETON_KEEP_DISTANCE {
            mob.target = None; // stand and shoot
            mob.yaw = mob.head_yaw;
        }
        let eye = [mob.pos[0], mob.pos[1] + mob.size().1 * 0.85, mob.pos[2]];
        let chest = [target[0], target[1] + 0.6, target[2]];
        if distance > SKELETON_RANGE
            || mob.attack_cooldown > 0
            || !self.line_of_sight(eye, [target[0], target[1] + 1.62, target[2]])
        {
            return;
        }
        // Aimed a little high, more the farther the target, to make up for the drop.
        let (dx, dy, dz) = (chest[0] - eye[0], chest[1] - eye[1], chest[2] - eye[2]);
        let lift = dx.hypot(dz) * 0.2;
        self.shoot_arrow(
            id,
            false,
            Shot {
                from: eye,
                dir: [dx, dy + lift, dz],
                speed: SKELETON_ARROW_SPEED,
                inaccuracy: SKELETON_INACCURACY,
                critical: false,
                base_damage: 2.0,
                pickup: false,
                flaming: false,
            },
        );
        mob.attack_cooldown = SKELETON_SHOT_TICKS;
    }

    /// Whether nothing solid is between two points.
    pub(super) fn line_of_sight(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let steps = (length / 0.25).ceil() as usize;
        (1..steps).all(|i| {
            let t = i as f64 / steps as f64;
            !self.point_in_block([from[0] + d[0] * t, from[1] + d[1] * t, from[2] + d[2] * t])
        })
    }

    /// Gives a player a push (knockback); the client moves itself.
    pub(super) fn push_player(&mut self, id: i32, velocity: [f64; 3]) {
        let v = |x: f64| (x.clamp(-3.9, 3.9) * 8000.0) as i16;
        self.send(
            id,
            Writer::packet(clientbound::SET_ENTITY_MOTION)
                .varint(id)
                .i16(v(velocity[0]))
                .i16(v(velocity[1]))
                .i16(v(velocity[2])),
        );
    }

    /// Sets a mob alight for `ticks`, as fire aspect and flaming arrows do, showing the
    /// flames to players if it was not burning yet.
    pub(super) fn ignite_mob(&mut self, id: i32, ticks: i32) {
        let Some(mut mob) = self.mobs.remove(&id) else {
            return;
        };
        let was_burning = mob.fire_ticks > 0;
        mob.fire_ticks = mob.fire_ticks.max(ticks.max(0) as u32);
        if !was_burning && mob.fire_ticks > 0 {
            self.send_to_viewers(
                &mob,
                Writer::packet(clientbound::SET_ENTITY_DATA)
                    .varint(id)
                    .u8(DATA_FLAGS)
                    .varint(SERIALIZER_BYTE)
                    .u8(ON_FIRE)
                    .u8(0xFF),
            );
        }
        self.mobs.insert(id, mob);
    }

    /// Zombies catch fire in direct sunlight and burn until they find shade or water.
    pub(super) fn burn_in_daylight(&mut self, id: i32, mob: &mut Mob) {
        let was_burning = mob.fire_ticks > 0;
        let feet = BlockPos::new(
            mob.pos[0].floor() as i32,
            mob.pos[1].floor() as i32,
            mob.pos[2].floor() as i32,
        );
        let in_water = self
            .chunks
            .get(&feet.chunk())
            .filter(|_| (MIN_Y..=MAX_Y).contains(&feet.y))
            .map(|c| c.get((feet.x & 15) as usize, feet.y, (feet.z & 15) as usize))
            .and_then(fluid_at)
            .is_some_and(|f| f.fluid == Fluid::Water);
        let in_lava = self
            .chunks
            .get(&feet.chunk())
            .filter(|_| (MIN_Y..=MAX_Y).contains(&feet.y))
            .map(|c| c.get((feet.x & 15) as usize, feet.y, (feet.z & 15) as usize))
            .and_then(fluid_at)
            .is_some_and(|f| f.fluid == Fluid::Lava);
        if in_lava && mob.kind.fire_immune() {
            mob.fire_ticks = 0;
        } else if in_lava {
            // Lava burns everything for 15 seconds, and hurts twice a second.
            mob.fire_ticks = mob.fire_ticks.max(300);
            if self.tick.is_multiple_of(10) {
                self.hurt_mob(id, mob, 4.0, "minecraft:lava");
            }
        } else if in_water {
            mob.fire_ticks = 0;
        } else if mob.kind.burns_in_daylight() && !self.weather.is_raining() {
            let head = BlockPos::new(
                feet.x,
                (mob.pos[1] + mob.size().1 * 0.85).floor() as i32,
                feet.z,
            );
            let under_sky = self.light_at(head).is_some_and(|(sky, _)| sky == 15);
            let level = f64::from(self.light_level(head).clamp(0, 15)) / 15.0;
            let brightness = level / (4.0 - 3.0 * level);
            if under_sky && brightness > 0.5 && self.rng.f64() * 30.0 < (brightness - 0.4) * 2.0 {
                mob.fire_ticks = 160;
            }
        }
        if mob.fire_ticks > 0 {
            mob.fire_ticks -= 1;
            if mob.fire_ticks.is_multiple_of(20) {
                self.hurt_mob(id, mob, 1.0, "minecraft:on_fire");
            }
        }
        if was_burning != (mob.fire_ticks > 0) {
            let flags = if mob.fire_ticks > 0 { ON_FIRE } else { 0 };
            self.send_to_viewers(
                mob,
                Writer::packet(clientbound::SET_ENTITY_DATA)
                    .varint(id)
                    .u8(DATA_FLAGS)
                    .varint(SERIALIZER_BYTE)
                    .u8(flags)
                    .u8(0xFF),
            );
        }
    }

    /// Damage from the world (fire, explosions) rather than a player.
    pub(super) fn hurt_mob(&mut self, id: i32, mob: &mut Mob, amount: f32, damage_type: &str) {
        mob.health -= amount;
        let kind = SyncedRegistry::get("minecraft:damage_type")
            .and_then(|r| r.id_of(damage_type))
            .unwrap_or(0);
        self.send_to_viewers(
            mob,
            Writer::packet(clientbound::DAMAGE_EVENT)
                .varint(id)
                .varint(kind)
                .varint(0)
                .varint(0)
                .bool(false),
        );
        if mob.health > 0.0 {
            let sound = format!("minecraft:entity.{}.hurt", mob.kind.sound());
            self.play_sound_at(&sound, mob.pos, 1.0, 1.0, None);
        }
    }

    /// Whether a monster is too far from every player to stay: always beyond 128
    /// blocks, now and then beyond 32.
    pub(super) fn despawns(&mut self, mob: &Mob) -> bool {
        if !mob.kind.is_monster() {
            return false;
        }
        let nearest = self
            .players
            .values()
            .map(|p| {
                let d: f64 = (0..3).map(|i| (p.pos[i] - mob.pos[i]).powi(2)).sum();
                d.sqrt()
            })
            .fold(f64::INFINITY, f64::min);
        nearest > 128.0 || (nearest > 32.0 && self.rng.below(800) == 0)
    }

    /// Whether monsters are close enough to a bed to keep players awake.
    pub(super) fn monsters_near(&self, pos: BlockPos) -> bool {
        self.mobs.values().any(|m| {
            m.kind.is_monster()
                && m.alive()
                && (m.pos[0] - f64::from(pos.x)).abs() < 8.0
                && (m.pos[2] - f64::from(pos.z)).abs() < 8.0
                && (m.pos[1] - f64::from(pos.y)).abs() < 5.0
        })
    }

    /// Tries to spawn monster groups in the dark around each player.
    pub(super) fn tick_monster_spawning(&mut self) {
        let players: Vec<([f64; 3], (i32, i32))> = self
            .players
            .values()
            .filter(|p| p.alive)
            .map(|p| (p.pos, p.center))
            .collect();
        for (pos, center) in players {
            let near = self
                .mobs
                .values()
                .filter(|m| m.kind.is_monster() && horizontal_distance(m.pos, pos) <= 128.0)
                .count();
            if near >= MONSTER_CAP {
                continue;
            }
            for _ in 0..SPAWN_ATTEMPTS {
                self.try_spawn_group(center);
            }
        }
    }

    fn try_spawn_group(&mut self, center: (i32, i32)) {
        let span = MOB_SPAWN_CHUNKS as u32 * 2 + 1;
        let cx = center.0 - MOB_SPAWN_CHUNKS + self.rng.below(span) as i32;
        let cz = center.1 - MOB_SPAWN_CHUNKS + self.rng.below(span) as i32;
        let x = cx * 16 + self.rng.below(16) as i32;
        let z = cz * 16 + self.rng.below(16) as i32;
        let Some(top) = self.top_block(x, z) else {
            return;
        };
        let bottom = self.dimension.span().min_y();
        if top < bottom {
            return;
        }
        let y = bottom + self.rng.below((top - bottom + 2) as u32) as i32;
        let Some(biome) = self.biome_at(x, y, z) else {
            return;
        };
        let Some((kind, size)) = self.biome_spawns.pick_monster(&biome, &mut self.rng) else {
            return;
        };
        // Ghasts turn up only one time in twenty.
        if kind == MobKind::Ghast && self.rng.below(20) != 0 {
            return;
        }
        let (mut gx, mut gz) = (x, z);
        for _ in 0..size {
            gx += self.rng.below(11) as i32 - 5;
            gz += self.rng.below(11) as i32 - 5;
            let pos = BlockPos::new(gx, y, gz);
            let at = [f64::from(gx) + 0.5, f64::from(y), f64::from(gz) + 0.5];
            // A spider needs more room than the two blocks a zombie needs.
            if self.monster_can_spawn(pos) && self.mob_fits(kind, at) {
                let mob = Mob::new(kind, at, &mut self.rng);
                self.mobs.insert(next_entity_id(), mob);
            }
        }
    }

    /// The y of the highest block in a loaded column that is not air.
    fn top_block(&self, x: i32, z: i32) -> Option<i32> {
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
        (MIN_Y..=MAX_Y)
            .rev()
            .find(|&y| !is_air(chunk.get(lx, y, lz)))
    }

    /// Whether a monster may appear standing at `pos`: on a solid block mobs can spawn on,
    /// with room for its body, far enough from players, and dark enough.
    fn monster_can_spawn(&mut self, pos: BlockPos) -> bool {
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let too_close = self.players.values().any(|p| {
            let d: f64 = (0..3).map(|i| (p.pos[i] - centre[i]).powi(2)).sum();
            d.sqrt() < SPAWN_DISTANCE
        });
        if too_close {
            return false;
        }
        let block = |w: &World, dy: i32| w.loaded_block_at(BlockPos::new(pos.x, pos.y + dy, pos.z));
        let (Some(below), Some(feet), Some(head)) =
            (block(self, -1), block(self, 0), block(self, 1))
        else {
            return false;
        };
        let open = |s: u16| collision_boxes(s).is_empty() && fluid_at(s).is_none();
        let ground_ok = sturdy(below, Dir::Up)
            && below != block::BEDROCK
            && !block_has_tag(below, "minecraft:leaves")
            && !block_has_tag(below, "minecraft:impermeable");
        if !ground_ok || !open(feet) || !open(head) {
            return false;
        }
        // Nether monsters do not care how dark it is.
        if !self.dimension.has_sky() {
            return true;
        }
        let Some((sky, light)) = self.light_at(pos) else {
            return false;
        };
        if light > 0 || u32::from(sky) > self.rng.below(32) {
            return false;
        }
        let level = (i32::from(sky) - self.sky_darken()).max(0);
        level <= self.rng.below(8) as i32
    }

    fn loaded_block_at(&self, pos: BlockPos) -> Option<u16> {
        if !(MIN_Y..=MAX_Y).contains(&pos.y) {
            return None;
        }
        let chunk = self.chunks.get(&pos.chunk())?;
        Some(chunk.get((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize))
    }
}

fn horizontal_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// The yaw (degrees, Minecraft's convention) that faces from `from` towards `to`.
fn yaw_towards(from: [f64; 3], to: [f64; 3]) -> f32 {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    (-dx.atan2(dz)).to_degrees() as f32
}

/// A ghast's face opening or closing as it charges a fireball.
fn ghast_charging(id: i32, charging: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(16)
        .varint(8) // boolean
        .bool(charging)
        .u8(0xFF)
}

/// A blaze glowing as it gets ready to throw fireballs.
fn blaze_glowing(id: i32, glowing: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(16)
        .varint(SERIALIZER_BYTE)
        .u8(u8::from(glowing))
        .u8(0xFF)
}

/// A creeper starting or stopping to swell.
fn creeper_swell(id: i32, swelling: bool) -> Writer {
    Writer::packet(clientbound::SET_ENTITY_DATA)
        .varint(id)
        .u8(DATA_CREEPER_SWELL)
        .varint(SERIALIZER_INT)
        .varint(if swelling { 1 } else { -1 })
        .u8(0xFF)
}
