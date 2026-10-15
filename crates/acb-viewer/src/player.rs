//! The keyboard-driven character (`--play`): input, locomotion, falls,
//! freerun jumps, climbing and kills, and its follow camera.

use std::collections::HashSet;

use glam::{Mat4, Quat, Vec2, Vec3};
use winit::keyboard::KeyCode;

use crate::climb::{Climb, Dir, Outcome};
use crate::ground::Ground;
use crate::locomotion::{self, Gait, Locomotion};
use crate::{kill, pose_actor, Gfx, Scripted};

/// Keyboard-driven character (actor 0) and its follow camera.
pub(crate) struct Player {
    pub(crate) pos: Vec3,
    /// Facing: entity +Y is (-sin yaw, cos yaw).
    pub(crate) yaw: f32,
    pub(crate) loco: Locomotion,
    /// Velocity while airborne, m/s.
    pub(crate) vel: Vec3,
    /// Height the current fall started at.
    pub(crate) fall_from: f32,
    pub(crate) kill: Option<PlayerKill>,
    pub(crate) climb: Option<Climb>,
    pub(crate) cam_yaw: f32,
    pub(crate) cam_pitch: f32,
    pub(crate) cam_dist: f32,
}

impl Player {
    pub(crate) fn entity(&self) -> Mat4 {
        Mat4::from_rotation_translation(Quat::from_rotation_z(self.yaw), self.pos)
    }

    pub(crate) fn look(&self) -> Vec3 {
        Vec3::new(self.cam_yaw.cos() * self.cam_pitch.cos(), self.cam_yaw.sin() * self.cam_pitch.cos(), self.cam_pitch.sin())
    }

    pub(crate) fn camera_eye_target(&self, ground: Option<&Ground>) -> (Vec3, Vec3) {
        let target = self.pos + Vec3::Z * 1.6;
        let mut dist = self.cam_dist;
        let back = -self.look();
        // pull in in front of whatever is between the player and the eye
        // (keeping 0.25 m off it; geometry hugging the player is ignored)
        const SKIP: f32 = 0.3;
        if let Some(f) = ground.and_then(|g| g.ray(target + back * SKIP, back * (dist + 0.25 - SKIP))) {
            dist = (SKIP + (dist + 0.25 - SKIP) * f - 0.25).max(0.6);
        }
        (target + back * dist, target)
    }
}

/// The player's kill in progress.
pub(crate) struct PlayerKill {
    side: usize,
    frame: Mat4,
    t: f32,
    /// Pose and placement it blends in from.
    from_pose: Vec<(Vec3, Quat)>,
    from_world: Mat4,
}

/// Blend-in time of a kill from wherever the player was.
const KILL_BLEND: f32 = 0.25;

/// Player control: kills, climbing, locomotion (input -> gait and facing,
/// clip-driven pose and root motion with wall sliding, floor snapping,
/// falls and jumps), then posing actor 0.
pub(crate) fn drive(p: &mut Player, g: &mut Gfx, keys: &HashSet<KeyCode>, sim_time: f32, dt: f32) {
    let Some(clips) = &g.loco else { return };
    if g.actors.is_empty() {
        return;
    }
    let k = |c| keys.contains(&c);
    // E: kill the nearest person in reach ahead
    if k(KeyCode::KeyE) && p.kill.is_none() && !p.loco.airborne() {
        if let Some(kills) = &g.kills {
            let fwd = Vec2::new(-p.yaw.sin(), p.yaw.cos());
            let victim = g
                .actors
                .iter()
                .enumerate()
                .skip(1)
                .filter(|(_, a)| a.scripted.is_none())
                .map(|(i, a)| (i, a.entity.w_axis.truncate()))
                .filter(|(_, v)| {
                    let d = (*v - p.pos).truncate();
                    d.length() < 2.5 && (v.z - p.pos.z).abs() < 1.0 && d.normalize_or_zero().dot(fwd) > 0.5
                })
                .min_by(|a, b| a.1.distance(p.pos).total_cmp(&b.1.distance(p.pos)));
            if let Some((vi, vpos)) = victim {
                let (_, rot, _) = g.actors[vi].entity.to_scale_rotation_translation();
                let vyaw = rot.to_euler(glam::EulerRot::ZXY).0;
                if let Some((side, frame)) = kills.plan(p.pos, vpos, vyaw) {
                    let v = &mut g.actors[vi];
                    v.agent = None;
                    v.model = Mat4::IDENTITY;
                    v.scripted = Some(Scripted { clip: kills.sets[side].1.clone(), frame, start: sim_time });
                    let from_pose = p.loco.last_pose().clone();
                    p.kill = Some(PlayerKill { side, frame, t: 0.0, from_pose, from_world: p.entity() });
                }
            }
        }
    }
    if let (Some(kill), Some(kills)) = (&mut p.kill, &g.kills) {
        let clip = &kills.sets[kill.side].0;
        kill.t += dt;
        let t = kill.t.min(clip.length);
        let c = g.actors[0].character.clone();
        let mut pose = clip.sample(&c.skeleton, t);
        let w = (kill.t / KILL_BLEND).min(1.0);
        if w < 1.0 {
            let mut from = kill.from_pose.clone();
            acb_anim::pose::blend(&mut from, &pose, w);
            pose = from;
        }
        // the camera follows the pelvis; the hand-off re-roots there
        let neutral = kill::reference(clips.idle(), 0.0).unwrap_or((Vec3::Z * 0.96, 66f32.to_radians()));
        if let Some((rp, ry)) = kill::reference(clip, t) {
            let (pos, yaw) = kill::reroot(kill.frame, rp, ry, neutral);
            p.pos = pos.with_z(kill.frame.w_axis.z);
            p.yaw = yaw;
        }
        let mut world = kill.frame * clip.root_motion(t);
        if w < 1.0 {
            let (_, r0, t0) = kill.from_world.to_scale_rotation_translation();
            let (_, r1, t1) = world.to_scale_rotation_translation();
            world = Mat4::from_rotation_translation(r0.slerp(r1, w), t0.lerp(t1, w));
        }
        let globals = c.skeleton.globals(&pose);
        if kill.t >= clip.length {
            p.kill = None;
            p.loco.resume(pose);
        }
        if let Some(actor) = g.actors.first_mut() {
            pose_actor(&mut g.scene, &g.renderer, actor, &globals, world, dt);
        }
        return;
    }
    // on a wall: climb
    if let (Some(cl), Some(cc), Some(ground)) = (&mut p.climb, &g.climbs, &g.ground) {
        let want = if k(KeyCode::KeyW) {
            Some(Dir::Up)
        } else if k(KeyCode::KeyS) {
            Some(Dir::Down)
        } else if k(KeyCode::KeyA) {
            Some(Dir::Left)
        } else if k(KeyCode::KeyD) {
            Some(Dir::Right)
        } else {
            None
        };
        let c = g.actors[0].character.clone();
        let find = |hand: Vec3, n: Vec2, d: Dir| ground.next_hold(hand, n, d);
        let (pose, outcome) = cl.update(cc, &c.skeleton, want, &find, dt);
        p.pos = cl.pos;
        p.yaw = cl.yaw;
        let wall = cl.hold.wall;
        match outcome {
            Outcome::Climbing => {}
            Outcome::TopOut(spot) => {
                p.climb = None;
                p.loco.resume(pose.clone());
                if let Some(z) = ground.floor_below(spot + Vec3::Z * 1.0) {
                    let target = spot.with_z(z);
                    let d = target - p.pos;
                    let t = 0.45;
                    p.vel = (d.truncate() / t).extend((d.z + 0.05 + 0.5 * GRAVITY * t * t) / t);
                    p.fall_from = target.z;
                    p.loco.start_jump(clips, &c.skeleton, 0);
                }
            }
            Outcome::Drop => {
                p.climb = None;
                p.loco.resume(pose.clone());
                p.loco.start_fall();
                p.vel = (wall * 1.0).extend(0.0);
                p.fall_from = p.pos.z;
            }
        }
        let world = p.entity();
        let globals = c.skeleton.globals(&pose);
        if let Some(actor) = g.actors.first_mut() {
            pose_actor(&mut g.scene, &g.renderer, actor, &globals, world, dt);
        }
        return;
    }
    let actor = &g.actors[0];
    let fwd = Vec2::new(p.cam_yaw.cos(), p.cam_yaw.sin());
    let right = Vec2::new(fwd.y, -fwd.x);
    let mut dir = Vec2::ZERO;
    if k(KeyCode::KeyW) { dir += fwd; }
    if k(KeyCode::KeyS) { dir -= fwd; }
    if k(KeyCode::KeyD) { dir += right; }
    if k(KeyCode::KeyA) { dir -= right; }
    let gait = if k(KeyCode::Space) {
        Gait::Sprint
    } else if k(KeyCode::ShiftLeft) || k(KeyCode::ShiftRight) {
        Gait::Run
    } else {
        Gait::Walk
    };
    let step = p.loco.update(clips, &actor.character.skeleton, dir.normalize_or_zero(), gait, &mut p.yaw, dt);
    let delta = step.delta;
    if let Some(ground) = &g.ground {
        if p.loco.airborne() {
            p.vel.z -= GRAVITY * dt;
            // falling past a hold: grab it (not right after letting go)
            let fwd = Vec2::new(-p.yaw.sin(), p.yaw.cos());
            if p.vel.z < -1.0 && g.climbs.is_some() && p.fall_from - p.pos.z > 0.3 {
                if let Some(hold) = ground.catch_hold(p.pos, fwd) {
                    p.climb = Some(Climb::catch(hold, p.pos));
                    p.loco.resume(p.loco.last_pose().clone());
                    p.vel = Vec3::ZERO;
                    return;
                }
            }
            let mut at = slide(ground, p.pos, p.vel.truncate() * dt);
            let z = p.pos.z + p.vel.z * dt;
            match ground.floor_below(at + Vec3::Z * 0.1) {
                Some(f) if f >= z => {
                    at.z = f;
                    p.vel = Vec3::ZERO;
                    p.loco.land(p.fall_from - f, dir != Vec2::ZERO);
                }
                _ => at.z = z,
            }
            p.pos = at;
        } else {
            let running = p.loco.speed >= 1.5 && dir != Vec2::ZERO;
            let fwd = Vec2::new(-p.yaw.sin(), p.yaw.cos());
            // freerun: up onto a ledge ahead
            let up = running.then(|| ground.ledge_ahead(p.pos, fwd, 2.0, 0.4, 2.2)).flatten();
            let (to, ledge) = move_on(ground, p.pos, delta);
            // pressing into a wall: ease out of the cycle instead of
            // running on the spot (the run lean would put the head through)
            let len = delta.length();
            let blocked = len > 1e-4 && (to - p.pos).truncate().dot(delta) / (len * len) < 0.3;
            if up.is_none() && blocked {
                p.loco.blocked(dt);
            }
            // walked into a wall with a hold above: climb it
            if blocked && dir != Vec2::ZERO && g.climbs.is_some() {
                if let Some(hold) = ground.mount_hold(p.pos, fwd) {
                    p.climb = Some(Climb::mount(hold, p.pos));
                    p.vel = Vec3::ZERO;
                    return;
                }
            }
            // freerun: over a gap instead of dropping into it
            let across = (ledge && running).then(|| jump_target(ground, p.pos, fwd)).flatten();
            if let Some(target) = up.or(across) {
                let d = target - p.pos;
                let horiz = d.truncate().length();
                let kind = match (d.z, horiz) {
                    (z, _) if z > 0.3 => usize::from(z > 1.2),          // up 050 / 300
                    (z, h) if z < -1.0 => 5 + usize::from(h > 2.5) + usize::from(h > 4.0), // down
                    (_, h) => 2 + usize::from(h > 2.5) + usize::from(h > 4.0), // front
                };
                // a parabola that comes down onto the target at `t`
                // (descending there, so it clears the edge before it)
                let t = (horiz / 6.0).clamp(0.4, 0.8);
                p.vel = (d.truncate() / t).extend((d.z + 0.05 + 0.5 * GRAVITY * t * t) / t);
                p.fall_from = p.pos.z.max(target.z);
                p.loco.start_jump(clips, &actor.character.skeleton, kind);
                if std::env::var_os("ACBMP_TRACE").is_some() {
                    eprintln!(
                        "jump {} from {:.2?} to {:.2?} ({}) in {t:.2}s",
                        locomotion::JUMPS[kind].0,
                        p.pos.to_array(),
                        target.to_array(),
                        if up.is_some() { "ledge" } else { "gap" }
                    );
                }
            } else {
                if ledge {
                    p.loco.start_fall();
                    p.vel = (delta / dt.max(1e-4)).extend(0.0);
                    p.fall_from = p.pos.z;
                }
                // shoulder past people rather than through them (walls win)
                let mut push = Vec2::ZERO;
                for a in g.actors.iter().skip(1).filter(|a| a.scripted.is_none()) {
                    let o = a.entity.w_axis.truncate();
                    let d = (to - o).truncate();
                    let len = d.length();
                    if len > 1e-3 && len < PERSONAL && (to.z - o.z).abs() < 1.5 {
                        push += d / len * (PERSONAL - len);
                    }
                }
                p.pos = if push == Vec2::ZERO { to } else { move_on(ground, to, push).0 };
            }
        }
    } else {
        p.pos += delta.extend(0.0);
    }

    // ACBMP_TRACE=1: player state four times a second (simulated time)
    if std::env::var_os("ACBMP_TRACE").is_some() && (sim_time * 4.0).floor() != ((sim_time - dt) * 4.0).floor() {
        eprintln!(
            "t {:.2} pos {:.2} {:.2} {:.2} yaw {:.0} speed {:.2}{}",
            sim_time,
            p.pos.x,
            p.pos.y,
            p.pos.z,
            p.yaw.to_degrees(),
            p.loco.speed,
            if p.loco.airborne() { " AIR" } else { "" }
        );
    }
    let c = &actor.character;
    let pose = step.pose;
    let globals = c.skeleton.globals(&pose);
    let world = p.entity() * actor.model;
    if let Some(actor) = g.actors.first_mut() {
        pose_actor(&mut g.scene, &g.renderer, actor, &globals, world, dt);
    }
}

/// How close the player gets to other people, metres.
const PERSONAL: f32 = 0.6;

/// Gravity, m/s^2 (ClothSettings use the same 10).
const GRAVITY: f32 = 10.0;

/// Horizontal move with wall sliding, swept at knee and chest height (so
/// steps and curbs pass).
fn slide(ground: &Ground, pos: Vec3, delta: Vec2) -> Vec3 {
    const RADIUS: f32 = 0.3;
    let blocked = |from: Vec3, d: Vec2| {
        [0.55, 1.3]
            .into_iter()
            .filter_map(|h| ground.sweep_wall(from + Vec3::Z * h, d, RADIUS))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    };
    let mut at = pos;
    let mut d = delta;
    for _ in 0..2 {
        match blocked(at, d) {
            None => {
                at += d.extend(0.0);
                break;
            }
            Some((f, n)) => {
                at += (d * f).extend(0.0);
                let rest = d * (1.0 - f);
                d = rest - n * rest.dot(n);
            }
        }
    }
    at
}

/// Where a running jump off a drop at `pos` along `fwd` lands: the first
/// floor 1.5 to 5.5 m ahead no more than 0.6 m higher and 5 m lower, with
/// headroom on the way.
fn jump_target(ground: &Ground, pos: Vec3, fwd: Vec2) -> Option<Vec3> {
    (6..=22).map(|i| i as f32 * 0.25).find_map(|d| {
        let q = pos + (fwd * d).extend(0.0);
        let z = ground.floor_below(q + Vec3::Z * 0.6)?;
        if z < pos.z - 5.0 {
            return None;
        }
        // nothing in the way at chest height
        ground.sweep_wall(pos + Vec3::Z * 1.3, fwd * d, 0.3).is_none().then(|| q.with_z(z))
    })
}

/// Moves a character standing at `pos` by `delta`: slides along walls and
/// snaps to the floor within a step. Returns the new position and whether
/// it left a ledge (floor more than a step below: start falling, position
/// not snapped). Moves onto no floor at all are refused.
fn move_on(ground: &Ground, pos: Vec3, delta: Vec2) -> (Vec3, bool) {
    const STEP: f32 = 0.6;
    let at = slide(ground, pos, delta);
    match ground.floor_below(at + Vec3::Z * STEP) {
        Some(z) if z >= at.z - STEP => (at.with_z(z), false),
        Some(_) => (at, true),
        None => (pos, false),
    }
}
