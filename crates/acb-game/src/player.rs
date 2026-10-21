//! A player-driven character: per-tick input -> kills, climbing,
//! locomotion, falls and freerun jumps (`jump.rs`).

use glam::{Mat4, Quat, Vec2, Vec3};

use crate::climb::{Climb, Dir, Outcome};
use crate::ground::Ground;
use crate::locomotion::{Gait, Locomotion};
use crate::{jump, kill, rules, Body, Clipset, PlayerInput, Scripted};

/// A character under player control and its movement state.
#[derive(Clone)]
pub struct Player {
    /// The body it drives (`World::bodies`).
    pub body: usize,
    pub pos: Vec3,
    /// Facing: entity +Y is (-sin yaw, cos yaw).
    pub yaw: f32,
    pub loco: Locomotion,
    /// Velocity while airborne, m/s.
    pub vel: Vec3,
    /// Height the current fall started at.
    pub fall_from: f32,
    /// A targeted freerun jump in flight (`jump.rs`).
    pub flight: Option<jump::Flight>,
    pub kill: Option<PlayerKill>,
    pub climb: Option<Climb>,
    /// Walking in to a kill (`rules::approach`).
    pub approach: Option<PlayerApproach>,
    /// `pos` before the last tick, for render interpolation.
    pub prev_pos: Vec3,
}

impl Player {
    pub fn new(body: usize, pos: Vec3, yaw: f32, loco: Locomotion) -> Player {
        Player { body, pos, yaw, loco, vel: Vec3::ZERO, fall_from: pos.z, flight: None, kill: None, climb: None, approach: None, prev_pos: pos }
    }

    pub fn entity(&self) -> Mat4 {
        Mat4::from_rotation_translation(Quat::from_rotation_z(self.yaw), self.pos)
    }

    /// Position between the last two ticks (`alpha` 0 = previous, 1 = last).
    pub fn pos_at(&self, alpha: f32) -> Vec3 {
        if alpha >= 1.0 {
            self.pos
        } else {
            self.prev_pos.lerp(self.pos, alpha)
        }
    }
}

impl Player {
    /// The player as the rules see it.
    pub fn actor(&self) -> rules::Actor {
        let stance = if self.kill.is_some() {
            rules::Stance::Down
        } else if self.climb.is_some() {
            rules::Stance::Climb
        } else if self.loco.airborne() {
            rules::Stance::Air
        } else {
            rules::Stance::Ground
        };
        rules::Actor { pos: self.pos, yaw: self.yaw, stance, speed: self.loco.speed }
    }
}

/// A non-player body as the rules see it (crowd NPCs walk the ground;
/// one playing a scripted clip is being killed or stunned).
pub fn body_actor(b: &Body) -> rules::Actor {
    let (_, rot, pos) = b.entity.to_scale_rotation_translation();
    let yaw = rot.to_euler(glam::EulerRot::ZXY).0;
    let stance = if b.scripted.is_some() { rules::Stance::Down } else { rules::Stance::Ground };
    rules::Actor { pos, yaw, stance, speed: 0.0 }
}

/// A walk-in towards a kill's start point.
#[derive(Clone)]
pub struct PlayerApproach {
    /// Victim body.
    pub victim: usize,
    /// Seconds since it began.
    pub t: f32,
}

/// Start the paired kill (or stun) of body `vi`: the victim plays its
/// clip in the shared frame, the player blends into the assassin's.
fn start_paired(p: &mut Player, bodies: &mut [Body], kills: &kill::KillClips, vi: usize, stun: bool, time: f32) {
    let victim = body_actor(&bodies[vi]);
    // a kill plays in the victim's frame; a stun in the player's, with the
    // victim's frame ahead of it
    let plan = if stun {
        let side = 2 + kill::side(p.pos, victim.pos, victim.yaw);
        let (at, yaw) = rules::stun_placement(&p.actor(), kill::STUN_SPACING);
        Some((side, p.entity(), Mat4::from_rotation_translation(Quat::from_rotation_z(yaw), at)))
    } else {
        kills.plan(p.pos, victim.pos, victim.yaw).map(|(side, frame)| (side, frame, frame))
    };
    let Some((side, frame, victim_frame)) = plan else { return };
    if crate::trace() {
        eprintln!("kill: start set {side} body {vi} at {:.2} {:.2} victim d {:.2}", p.pos.x, p.pos.y, (victim.pos - p.pos).truncate().length());
    }
    let v = &mut bodies[vi];
    v.agent = None;
    v.model = Mat4::IDENTITY;
    v.scripted = Some(Scripted { clip: kills.sets[side].1.clone(), frame: victim_frame, start: time });
    let from_pose = p.loco.last_pose().clone();
    p.kill = Some(PlayerKill { side, frame, t: 0.0, from_pose, from_world: p.entity() });
}

/// The player's kill in progress.
#[derive(Clone)]
pub struct PlayerKill {
    side: usize,
    frame: Mat4,
    t: f32,
    /// Pose and placement it blends in from.
    from_pose: Vec<(Vec3, Quat)>,
    from_world: Mat4,
}

/// Blend-in time of a kill from wherever the player was.
const KILL_BLEND: f32 = 0.25;

/// How close the player gets to other people, metres.
const PERSONAL: f32 = 0.6;

/// One tick of player control: kills, climbing, locomotion (input -> gait
/// and facing, clip-driven pose and root motion with wall sliding, floor
/// snapping, falls and jumps), then the body's pose and placement.
pub(crate) fn drive(
    p: &mut Player,
    bodies: &mut [Body],
    clipset: &Clipset,
    ground: Option<&Ground>,
    input: &PlayerInput,
    time: f32,
    dt: f32,
) {
    let Some(clips) = &clipset.loco else { return };
    if bodies.is_empty() {
        return;
    }
    let me = p.body;
    let c = bodies[me].character.clone();
    let sk = &c.skeleton;
    // E: kill, Q: stun the nearest valid victim (rules.rs decides who and
    // from where); a walking kill walks in to the kill's start point first
    let mut input = *input;
    if (input.kill || input.stun) && p.kill.is_none() && p.approach.is_none() {
        if let Some(kills) = &clipset.kills {
            let act = if input.stun { rules::Act::Stun } else { rules::Act::Kill };
            let me_actor = p.actor();
            let candidates = bodies
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != me)
                .map(|(i, b)| (i, body_actor(b), rules::Relation::Npc));
            match rules::best_victim(act, &me_actor, candidates, &rules::Rules::default(), kills.spacing()) {
                Some((vi, grant)) => {
                    if crate::trace() {
                        eprintln!("kill: {act:?} body {vi} granted {grant:?}");
                    }
                    if grant.situation != rules::Situation::Ground2Ground {
                        // only the ground pairs are loaded so far
                    } else if grant.approach {
                        p.approach = Some(PlayerApproach { victim: vi, t: 0.0 });
                    } else {
                        start_paired(p, bodies, kills, vi, act == rules::Act::Stun, time);
                    }
                }
                None if crate::trace() => {
                    // why the nearest one was refused
                    let why = bodies
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != me)
                        .map(|(i, b)| (i, body_actor(b)))
                        .min_by(|a, b| a.1.pos.distance(p.pos).total_cmp(&b.1.pos.distance(p.pos)))
                        .map(|(i, v)| (i, rules::check(act, &me_actor, &v, rules::Relation::Npc, &rules::Rules::default(), kills.spacing())));
                    eprintln!("kill: {act:?} refused, nearest {why:?}");
                }
                None => {}
            }
        }
    }
    // the walk-in: steer the locomotion at the kill's start point
    if let (Some(ap), Some(kills)) = (&mut p.approach, &clipset.kills) {
        ap.t += dt;
        let vi = ap.victim;
        let victim = body_actor(&bodies[vi]);
        let goal = kills.plan(p.pos, victim.pos, victim.yaw).map(|(side, frame)| kills.start_point(side, frame, clips.idle()));
        let step = match goal {
            Some(_) if bodies[vi].scripted.is_some() || victim.stance != rules::Stance::Ground => rules::Approach::Abort,
            Some((goal, _)) => rules::approach(p.pos, goal, victim.pos, ap.t, kills.spacing() + rules::DIST_GAP),
            None => rules::Approach::Abort,
        };
        if crate::trace() {
            let g = goal.map(|g| g.0).unwrap_or_default();
            eprintln!(
                "kill: approach t {:.3} pos {:.2} {:.2} goal {:.2} {:.2} victim d {:.2} -> {step:?}",
                ap.t,
                p.pos.x,
                p.pos.y,
                g.x,
                g.y,
                (victim.pos - p.pos).truncate().length()
            );
        }
        match step {
            rules::Approach::Move { heading, run, .. } => {
                input = PlayerInput { forward: true, cam_yaw: heading, run, ..PlayerInput::default() };
            }
            rules::Approach::Arrived => {
                p.approach = None;
                start_paired(p, bodies, kills, vi, false, time);
            }
            rules::Approach::Abort => p.approach = None,
        }
    }
    let input = &input;
    if let (Some(kill), Some(kills)) = (&mut p.kill, &clipset.kills) {
        let clip = &kills.sets[kill.side].0;
        kill.t += dt;
        let t = kill.t.min(clip.length);
        let mut pose = clip.sample(sk, t);
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
        if kill.t >= clip.length {
            p.kill = None;
            p.loco.resume(pose.clone());
        }
        bodies[me].show(world, pose);
        return;
    }
    // on a wall: climb
    if let (Some(cl), Some(cc), Some(ground)) = (&mut p.climb, &clipset.climbs, ground) {
        let want = if input.forward {
            Some(Dir::Up)
        } else if input.back {
            Some(Dir::Down)
        } else if input.left {
            Some(Dir::Left)
        } else if input.right {
            Some(Dir::Right)
        } else {
            None
        };
        let (pose, outcome) = cl.update(cc, sk, ground, want, dt);
        p.pos = cl.pos;
        p.yaw = cl.yaw;
        if crate::trace() && (time * 4.0).floor() != ((time - dt) * 4.0).floor() {
            eprintln!("t {:.2} pos {:.2} {:.2} {:.2} yaw {:.0} {}", time, p.pos.x, p.pos.y, p.pos.z, p.yaw.to_degrees(), cl.describe());
        }
        match outcome {
            Outcome::Climbing => {}
            Outcome::Stand(pos, yaw) => {
                p.climb = None;
                p.pos = pos;
                p.yaw = yaw;
                p.vel = Vec3::ZERO;
                p.loco.resume(pose.clone());
            }
            Outcome::Drop(v) => {
                p.climb = None;
                p.loco.resume(pose.clone());
                p.loco.start_fall();
                p.vel = v;
                p.fall_from = p.pos.z;
            }
        }
        bodies[me].show(p.entity(), pose);
        return;
    }
    let fwd = Vec2::new(input.cam_yaw.cos(), input.cam_yaw.sin());
    let right = Vec2::new(fwd.y, -fwd.x);
    let mut dir = Vec2::ZERO;
    if input.forward { dir += fwd; }
    if input.back { dir -= fwd; }
    if input.right { dir += right; }
    if input.left { dir -= right; }
    let gait = if input.sprint {
        Gait::Sprint
    } else if input.run {
        Gait::Run
    } else {
        Gait::Walk
    };
    let step = p.loco.update(clips, sk, dir.normalize_or_zero(), gait, &mut p.yaw, dt);
    let delta = step.delta;
    if let Some(ground) = ground {
        if let Some(f) = &mut p.flight {
            // a targeted jump: on its path, down on the target
            let (at, done) = f.step(dt);
            p.pos = at;
            if done {
                if crate::trace() {
                    eprintln!("landed at {:.2?} after {:.3} s", at.to_array(), f.t);
                }
                p.flight = None;
                p.loco.land(0.0, dir != Vec2::ZERO);
            }
        } else if p.loco.airborne() {
            p.vel.z -= jump::GRAVITY * dt;
            // falling past a hold: grab it (not right after letting go)
            let fwd = Vec2::new(-p.yaw.sin(), p.yaw.cos());
            if p.vel.z < -1.0 && clipset.climbs.is_some() && p.fall_from - p.pos.z > 0.3 {
                if let Some(hold) = ground.catch_hold(p.pos, fwd) {
                    let stance = ground.hang_stance(hold.nearest(p.pos), hold.wall);
                    p.climb = Some(Climb::catch(hold, stance, p.pos, p.loco.last_pose().clone()));
                    p.loco.resume(p.loco.last_pose().clone());
                    p.vel = Vec3::ZERO;
                    return;
                }
            }
            let mut at = ground.slide(p.pos, p.vel.truncate() * dt);
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
            let up = running.then(|| jump::target(ground, clips, p.pos, fwd, false)).flatten();
            let (to, ledge) = ground.move_on(p.pos, delta);
            // pressing into a wall: ease out of the cycle instead of
            // running on the spot (the run lean would put the head through)
            let len = delta.length();
            let blocked = len > 1e-4 && (to - p.pos).truncate().dot(delta) / (len * len) < 0.3;
            if up.is_none() && blocked {
                p.loco.blocked(dt);
            }
            // onto the wall (retail entry order): a wall run when running at
            // a wall in high profile, else climbing straight on from a foot
            // hold when pushing into it. Neither needs the move to be
            // blocked this tick: the easing out of a blocked cycle stops the
            // feet, so a blocked test never fires again after first contact.
            if clipset.climbs.is_some() && up.is_none() && dir != Vec2::ZERO {
                let pushing = dir.normalize_or_zero().dot(fwd) > 0.7;
                let vel = delta / dt.max(1e-4);
                let start = if input.run && p.loco.speed >= 1.5 && pushing {
                    ground.wall_ahead(p.pos, fwd, vel).map(|(hit, n)| {
                        let target = ground.wall_run_hold(hit, n, p.pos.z);
                        Climb::wall_run(target, hit, n, p.pos, p.yaw, p.loco.last_pose().clone())
                    })
                } else {
                    None
                };
                let start = start.or_else(|| {
                    pushing
                        .then(|| ground.climb_entry(p.pos, fwd))
                        .flatten()
                        .map(|(hold, point)| Climb::mount(hold, point, p.pos, p.yaw, p.loco.last_pose().clone()))
                });
                if let Some(c) = start {
                    if crate::trace() {
                        eprintln!("climb: {} from {:.2} {:.2} {:.2}", c.describe(), p.pos.x, p.pos.y, p.pos.z);
                    }
                    p.climb = Some(c);
                    p.vel = Vec3::ZERO;
                    return;
                }
            }
            // freerun: over a gap instead of dropping into it
            let why = if up.is_some() { "up" } else { "gap" };
            let across = (ledge && running && up.is_none()).then(|| jump::target(ground, clips, p.pos, fwd, true)).flatten();
            if let Some((target, flight, narrow)) = up.or(across) {
                jump::take_off(p, clips, sk, target, flight, if narrow { "narrow top" } else { why });
            } else {
                if ledge {
                    p.loco.start_fall();
                    p.vel = (delta / dt.max(1e-4)).extend(0.0);
                    p.fall_from = p.pos.z;
                }
                // shoulder past people rather than through them (walls win)
                let mut push = Vec2::ZERO;
                for a in bodies.iter().enumerate().filter(|(i, a)| *i != me && a.scripted.is_none()).map(|x| x.1) {
                    let o = a.entity.w_axis.truncate();
                    let d = (to - o).truncate();
                    let len = d.length();
                    if len > 1e-3 && len < PERSONAL && (to.z - o.z).abs() < 1.5 {
                        push += d / len * (PERSONAL - len);
                    }
                }
                p.pos = if push == Vec2::ZERO { to } else { ground.move_on(to, push).0 };
            }
        }
    } else {
        p.pos += delta.extend(0.0);
    }

    // ACBMP_TRACE=1: player state four times a second (simulated time)
    if crate::trace() && (time * 4.0).floor() != ((time - dt) * 4.0).floor() {
        eprintln!(
            "t {:.2} pos {:.2} {:.2} {:.2} yaw {:.0} speed {:.2}{}",
            time,
            p.pos.x,
            p.pos.y,
            p.pos.z,
            p.yaw.to_degrees(),
            p.loco.speed,
            if p.loco.airborne() { " AIR" } else { "" }
        );
    }
    let world = p.entity() * bodies[me].model;
    bodies[me].show(world, step.pose);
}
