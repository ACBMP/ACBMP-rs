//! Snapshots of the simulation's mutable state, for network rewind
//! (restore an acknowledged tick, re-simulate the inputs after it), and
//! setting up more than one player.
//!
//! [`Saved`] holds everything [`World::step`] changes: the clock, every
//! [`Player`] (locomotion, kill, climb, airborne velocity) and every body's
//! placement, pose and scripted clip. The static parts (collision, clips,
//! characters) are shared and not copied. Any new per-tick state added to
//! `World` must be added here too, or a rewind will not restore it.
//!
//! Cost: a save clones each body's two local poses and each player's
//! locomotion poses (about 100 bones each, a few KB per character), plus a
//! deep copy of the clip of any body playing a scripted clip (kill
//! victims).

use glam::{Mat4, Vec3};

use crate::crowd::Agent;
use crate::locomotion::Locomotion;
use crate::player::Player;
use crate::{Pose, Scripted, World};

/// The state of a [`World`] after one tick.
#[derive(Clone)]
pub struct Saved {
    pub tick: u64,
    pub time: f32,
    players: Vec<Player>,
    bodies: Vec<BodyState>,
    mode: Option<crate::mode::Match>,
    seen: std::collections::VecDeque<Vec<crate::rules::Actor>>,
}

#[derive(Clone)]
struct BodyState {
    entity: Mat4,
    model: Mat4,
    phase: f32,
    clip_time: f32,
    epoch: u32,
    agent: Option<Agent>,
    scripted: Option<Scripted>,
    world: Mat4,
    pose: Pose,
    prev_world: Mat4,
    prev_pose: Pose,
}

impl World {
    /// Saves the mutable state (see [`Saved`]).
    pub fn save(&self) -> Saved {
        Saved {
            tick: self.tick,
            time: self.time,
            players: self.players.clone(),
            mode: self.mode.clone(),
            seen: self.seen.clone(),
            bodies: self
                .bodies
                .iter()
                .map(|b| BodyState {
                    entity: b.entity,
                    model: b.model,
                    phase: b.phase,
                    clip_time: b.clip_time,
                    epoch: b.epoch,
                    agent: b.agent.clone(),
                    scripted: b.scripted.clone(),
                    world: b.world,
                    pose: b.pose.clone(),
                    prev_world: b.prev_world,
                    prev_pose: b.prev_pose.clone(),
                })
                .collect(),
        }
    }

    /// Puts the world back to `saved` (taken from this world, or from one
    /// loaded with the same [`crate::Setup`]).
    pub fn restore(&mut self, saved: &Saved) {
        assert_eq!(saved.bodies.len(), self.bodies.len(), "restore: saved from a different world");
        self.tick = saved.tick;
        self.time = saved.time;
        self.players.clone_from(&saved.players);
        self.mode.clone_from(&saved.mode);
        self.seen.clone_from(&saved.seen);
        for (b, s) in self.bodies.iter_mut().zip(&saved.bodies) {
            b.entity = s.entity;
            b.model = s.model;
            b.phase = s.phase;
            b.clip_time = s.clip_time;
            b.epoch = s.epoch;
            b.agent.clone_from(&s.agent);
            b.scripted.clone_from(&s.scripted);
            b.world = s.world;
            b.pose.clone_from(&s.pose);
            b.prev_world = s.prev_world;
            b.prev_pose.clone_from(&s.prev_pose);
        }
    }

    /// Hands bodies `0..n` to players (a [`crate::Setup`] with `play` makes
    /// only the first one a player), each standing on its spawn, facing
    /// along it. Needs the player clips (`play`).
    pub fn add_players(&mut self, n: usize) {
        let Some(clips) = &self.clips.loco else { return };
        let n = n.min(self.bodies.len());
        let root_drives = |b: &crate::Body| b.character.skeleton.bones.first().is_some_and(|bone| clips.idle().drives(bone.id));
        let mut placed = Vec::new();
        for i in self.players.len()..n {
            let b = &mut self.bodies[i];
            if root_drives(b) {
                b.model = Mat4::IDENTITY;
            } else {
                b.model = b.character.model_to_entity();
            }
            let facing = b.spawn.y_axis.truncate().normalize_or_zero();
            let yaw = (-facing.x).atan2(facing.y);
            let at = b.spawn.w_axis.truncate();
            let loco = Locomotion::new(clips, &b.character.skeleton);
            self.players.push(Player::new(i, at, yaw, loco));
            placed.push((i, at, yaw));
        }
        for (i, at, yaw) in placed {
            self.place_player(i, at, yaw);
        }
    }

    /// Puts player `i` at `pos` facing `yaw` (entity +Y = (-sin, cos)),
    /// on the floor below it.
    pub fn place_player(&mut self, i: usize, pos: Vec3, yaw: f32) {
        let Some(p) = self.players.get_mut(i) else { return };
        p.pos = pos;
        p.yaw = yaw;
        if let Some(z) = self.ground.as_ref().and_then(|g| g.floor_below(pos + Vec3::Z * 1.0)) {
            p.pos.z = z;
        }
        p.prev_pos = p.pos;
        p.fall_from = p.pos.z;
        let entity = p.entity();
        if let Some(b) = self.bodies.get_mut(p.body) {
            b.entity = entity;
            b.world = entity * b.model;
            b.prev_world = b.world;
        }
    }
}
