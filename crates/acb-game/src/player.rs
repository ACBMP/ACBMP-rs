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
    /// Seconds left in which a fall does not grab holds (after a back
    /// eject, so the wall just left is not caught again).
    pub catch_block: f32,
    /// This tick's touchdown after a fall: height (m), damage (% of max
    /// life, retail `HealthSettings`) and landing type (reset every tick;
    /// the match takes the damage off its health, fatal = dead).
    pub landing: Option<(f32, u32, jump::LandingType)>,
    /// In a hiding spot (haystack): the rules' [`rules::Stance::Hidden`]
    /// (set from `hide` while fully inside).
    pub hidden: bool,
    /// Entering, inside or leaving a hiding spot (`hide.rs`).
    pub hide: Option<crate::hide::Hiding>,
    /// Blended in a crowd group ([`rules::blended`], set by the world step).
    pub blended: bool,
    /// Horizontal speed over the last tick, m/s (the rules' running test).
    pub moved: f32,
}

/// Low profile stops at a drop deeper than this instead of walking off
/// (retail `xx_l_ledge_lookdown_*`, `xx_l_beam_edge_stop`); the jump
/// button then lowers onto the wall below (`Climb::pulldown`) or steps
/// off. Retail: `HumanGround::TestMoveToEdgeStopOnHumanGroundInterfaceCommand_
/// UnbalanceOnEdge` enters EdgeStop only for a narrow edge whose gap is
/// deeper than 2 m (and pull-down clearance); shallower drops are walked off.
pub const LEDGE_STOP: f32 = 2.0;

impl Player {
    pub fn new(body: usize, pos: Vec3, yaw: f32, loco: Locomotion) -> Player {
        Player { body, pos, yaw, loco, vel: Vec3::ZERO, fall_from: pos.z, flight: None, kill: None, climb: None, approach: None, prev_pos: pos, catch_block: 0.0, landing: None, hidden: false, hide: None, blended: false, moved: 0.0 }
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
        } else if self.loco.airborne() || self.flight.is_some() {
            rules::Stance::Air
        } else if self.hidden {
            rules::Stance::Hidden
        } else {
            rules::Stance::Ground
        };
        rules::Actor { pos: self.pos, yaw: self.yaw, stance, speed: self.moved }
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

/// Body `b` as player-side rules see it: `past` (a player's actor from
/// the lag-compensation history) when given, else its present placement
/// with the player's `stance`; anyone being killed or stunned now is
/// [`rules::Stance::Down`] either way.
pub fn seen_actor(b: &Body, past: Option<&rules::Actor>, stance: Option<rules::Stance>) -> rules::Actor {
    let mut a = match past {
        Some(a) => *a,
        None => {
            let mut a = body_actor(b);
            if let Some(s) = stance {
                a.stance = s;
            }
            a
        }
    };
    if b.scripted.is_some() {
        a.stance = rules::Stance::Down;
    }
    a
}

/// A walk-in towards a kill's start point.
#[derive(Clone)]
pub struct PlayerApproach {
    /// Victim body.
    pub victim: usize,
    /// Seconds since it began.
    pub t: f32,
}

/// The kill set ([`kill::WALK`], ...) for a granted situation.
pub fn kill_kind(situation: rules::Situation, act: rules::Act) -> usize {
    use rules::Situation as S;
    match (act, situation) {
        (rules::Act::Stun, _) => kill::STUN,
        (_, S::Ground2Ground) => kill::WALK,
        (_, S::Run2Ground) => kill::RUN,
        (_, S::AirDrop2Ground) => kill::AIR,
        (_, S::Drop2Ground) => kill::DROP,
        (_, S::Ground2Haystack) => kill::TO_HAY,
        (_, S::Haystack2Ground) => kill::HAY_GRAB,
    }
}

/// Gravity of the closing leap of air and drop kills, m/s^2.
const LEAP_GRAVITY: f32 = 9.81;

/// Start the paired kill (or stun) of body `vi` with set `kind`: the
/// victim plays its clip in the shared frame, the player blends into the
/// assassin's. Kinds that start away from the clip's start point (run,
/// air, drop, haystack) first close the gap: the player leaps (air, drop:
/// ballistic) or lunges there during a lead time while the victim holds
/// its clip's first frame.
#[allow(clippy::too_many_arguments)]
fn start_paired(p: &mut Player, bodies: &mut [Body], kills: &kill::KillClips, idle: &acb_anim::pose::Clip, vi: usize, kind: usize, dies: bool, time: f32) {
    let victim = body_actor(&bodies[vi]);
    let stun = kind == kill::STUN;
    // a kill plays in the victim's frame; a stun in the player's, with the
    // victim's frame ahead of it
    let plan = if stun {
        let side = kill::STUN + kill::side(p.pos, victim.pos, victim.yaw);
        let (at, yaw) = rules::stun_placement(&p.actor(), kill::STUN_SPACING);
        Some((side, p.entity(), Mat4::from_rotation_translation(Quat::from_rotation_z(yaw), at)))
    } else {
        kills.plan(kind, p.pos, victim.pos, victim.yaw).map(|(side, frame)| (side, frame, frame))
    };
    let Some((side, frame, victim_frame)) = plan else { return };
    let lead = if matches!(kind, kill::WALK | kill::STUN) {
        None
    } else {
        let (goal, goal_yaw) = kills.start_point(side, frame, idle);
        let gap = (goal - p.pos).truncate().length();
        let drop = (p.pos.z - goal.z).max(0.0);
        let leap = matches!(kind, kill::AIR | kill::DROP);
        let t = if leap {
            (2.0 * drop / LEAP_GRAVITY).sqrt().max(gap / 8.0).clamp(0.2, 1.0)
        } else {
            (gap / p.moved.max(rules::RUN_SPEED)).clamp(0.1, 0.4)
        };
        let t = (t * crate::TICK_HZ as f32).round() / crate::TICK_HZ as f32;
        Some(Lead { time: t, from: p.pos, from_yaw: p.yaw, to: goal, to_yaw: goal_yaw, leap })
    };
    if crate::trace() {
        eprintln!(
            "kill: start set {side} body {vi} at {:.2} {:.2} {:.2} victim d {:.2} lead {:.2}",
            p.pos.x,
            p.pos.y,
            p.pos.z,
            (victim.pos - p.pos).truncate().length(),
            lead.as_ref().map_or(0.0, |l| l.time)
        );
    }
    let v = &mut bodies[vi];
    v.agent = None;
    let start = time + lead.as_ref().map_or(0.0, |l| l.time);
    v.scripted = Some(Scripted { clip: kills.sets[side].1.clone(), frame: victim_frame, start, dies });
    let from_pose = p.loco.last_pose().clone();
    p.flight = None;
    p.climb = None;
    p.hidden = false;
    p.hide = None;
    p.vel = Vec3::ZERO;
    p.kill = Some(PlayerKill { side, frame, t: 0.0, from_pose, from_world: p.entity(), lead });
}

/// The closing move before a kill clip starts.
#[derive(Clone)]
pub struct Lead {
    /// Seconds (whole ticks).
    pub time: f32,
    from: Vec3,
    from_yaw: f32,
    to: Vec3,
    to_yaw: f32,
    /// Ballistic (air and drop kills) rather than a straight lunge.
    leap: bool,
}

impl Lead {
    /// Position and yaw at `t` seconds in.
    fn at(&self, t: f32) -> (Vec3, f32) {
        let s = (t / self.time).clamp(0.0, 1.0);
        let mut pos = self.from.lerp(self.to, s);
        if self.leap {
            let tt = t.clamp(0.0, self.time);
            let v0 = (self.to.z - self.from.z + 0.5 * LEAP_GRAVITY * self.time * self.time) / self.time;
            pos.z = self.from.z + v0 * tt - 0.5 * LEAP_GRAVITY * tt * tt;
        }
        let dy = (self.to_yaw - self.from_yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        (pos, self.from_yaw + dy * s)
    }
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
    /// Closing move first (run, air, drop, haystack kills).
    lead: Option<Lead>,
}

impl PlayerKill {
    /// Index into [`kill::KillClips::sets`].
    pub fn set(&self) -> usize {
        self.side
    }
}

/// Blend-in time of a kill from wherever the player was.
const KILL_BLEND: f32 = 0.25;

/// How close the player gets to other people, metres.
const PERSONAL: f32 = 0.6;

/// Seconds a back eject keeps the fall from grabbing holds: the length of
/// retail's untargeted push-off (`xx_h_rebound_footl_tr_fall`, 0.467 s).
const EJECT_NO_CATCH: f32 = 0.467;

/// One tick of player control: kills, climbing, locomotion (input -> gait
/// and facing, clip-driven pose and root motion with wall sliding, floor
/// snapping, falls and jumps), then the body's pose and placement.
pub(crate) fn drive(
    p: &mut Player,
    bodies: &mut [Body],
    clipset: &Clipset,
    ground: Option<&Ground>,
    input: &PlayerInput,
    rules: &rules::Rules,
    relation: &dyn Fn(usize) -> rules::Relation,
    seen: &dyn Fn(usize, &Body) -> rules::Actor,
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
    // being killed or stunned: the victim clip, no control; a stun (or a
    // free-play kill) gets up at its end
    if let Some(sc) = &bodies[me].scripted {
        let ct = (time - sc.start).clamp(0.0, sc.clip.length);
        let pose = sc.clip.sample(sk, ct);
        let world = sc.frame * sc.clip.root_motion(ct);
        let ended = time - sc.start >= sc.clip.length;
        let free = ended && !sc.dies;
        let neutral = clips.idle();
        let neutral = kill::reference(neutral, 0.0).unwrap_or((Vec3::Z * 0.96, 66f32.to_radians()));
        if let Some((rp, ry)) = kill::reference(&sc.clip, ct) {
            let (pos, yaw) = kill::reroot(sc.frame, rp, ry, neutral);
            p.pos = pos.with_z(sc.frame.w_axis.z);
            p.yaw = crate::locomotion::norm_yaw(yaw);
        }
        p.kill = None;
        p.approach = None;
        p.climb = None;
        p.flight = None;
        p.hide = None;
        p.hidden = false;
        p.vel = Vec3::ZERO;
        if free {
            bodies[me].scripted = None;
            p.loco.resume(pose.clone());
        }
        bodies[me].show(world, pose);
        return;
    }
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
                .map(|(i, b)| (i, seen(i, b), relation(i)));
            match rules::best_victim(act, &me_actor, candidates, rules, kills.spacing()) {
                Some((vi, grant)) => {
                    if crate::trace() {
                        eprintln!("kill: {act:?} body {vi} granted {grant:?}");
                    }
                    if grant.approach {
                        p.approach = Some(PlayerApproach { victim: vi, t: 0.0 });
                    } else {
                        let kind = kill_kind(grant.situation, act);
                        start_paired(p, bodies, kills, clips.idle(), vi, kind, act == rules::Act::Kill && !rules.free_play, time);
                    }
                }
                None if crate::trace() => {
                    // why the nearest one was refused
                    let why = bodies
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != me)
                        .map(|(i, b)| (i, seen(i, b)))
                        .min_by(|a, b| a.1.pos.distance(p.pos).total_cmp(&b.1.pos.distance(p.pos)))
                        .map(|(i, v)| (i, rules::check(act, &me_actor, &v, relation(i), rules, kills.spacing())));
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
        let goal = kills.plan(kill::WALK, p.pos, victim.pos, victim.yaw).map(|(side, frame)| kills.start_point(side, frame, clips.idle()));
        let step = match goal {
            Some(_) if bodies[vi].scripted.is_some() || victim.stance != rules::Stance::Ground => rules::Approach::Abort,
            // the contract went away during the walk-in
            Some(_) if !matches!(relation(vi), rules::Relation::Target | rules::Relation::Mutual) && !(relation(vi) == rules::Relation::Npc && (rules.allow_killing_npcs || rules.free_play)) => rules::Approach::Abort,
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
                start_paired(p, bodies, kills, clips.idle(), vi, kill::WALK, !rules.free_play, time);
            }
            rules::Approach::Abort => p.approach = None,
        }
    }
    let input = &input;
    if let (Some(kill), Some(kills)) = (&mut p.kill, &clipset.kills) {
        let clip = &kills.sets[kill.side].0;
        kill.t += dt;
        let lead_time = kill.lead.as_ref().map_or(0.0, |l| l.time);
        if let Some(lead) = kill.lead.as_ref().filter(|l| kill.t < l.time - 1e-4) {
            // closing in: pose eases into the clip's first frame
            let (pos, yaw) = lead.at(kill.t);
            p.pos = pos;
            p.yaw = crate::locomotion::norm_yaw(yaw);
            let mut pose = kill.from_pose.clone();
            acb_anim::pose::blend(&mut pose, &clip.sample(sk, 0.0), (kill.t / lead.time).min(1.0));
            bodies[me].show(p.entity(), pose);
            return;
        }
        let ct = kill.t - lead_time;
        let t = ct.min(clip.length);
        let mut pose = clip.sample(sk, t);
        let w = if kill.lead.is_some() { 1.0 } else { (kill.t / KILL_BLEND).min(1.0) };
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
            p.yaw = crate::locomotion::norm_yaw(yaw);
        }
        let mut world = kill.frame * clip.root_motion(t);
        if w < 1.0 {
            let (_, r0, t0) = kill.from_world.to_scale_rotation_translation();
            let (_, r1, t1) = world.to_scale_rotation_translation();
            world = Mat4::from_rotation_translation(r0.slerp(r1, w), t0.lerp(t1, w));
        }
        if ct >= clip.length {
            p.kill = None;
            p.loco.resume(pose.clone());
        }
        bodies[me].show(world, pose);
        return;
    }
    // in a hiding spot: its clips, any move input leaves
    if let (Some(h), Some(hide)) = (&mut p.hide, &clipset.hide) {
        let fwd = Vec2::new(input.cam_yaw.cos(), input.cam_yaw.sin());
        let right = Vec2::new(fwd.y, -fwd.x);
        let mut dir = Vec2::ZERO;
        if input.forward { dir += fwd; }
        if input.back { dir -= fwd; }
        if input.right { dir += right; }
        if input.left { dir -= right; }
        let leave = (dir != Vec2::ZERO).then(|| dir.normalize());
        let (pose, world, pos, yaw, out) = h.update(hide, sk, leave, dt);
        p.pos = pos;
        p.yaw = yaw;
        p.hidden = h.hidden();
        p.vel = Vec3::ZERO;
        if out {
            if crate::trace() {
                eprintln!("hide: out of spot {} at {:.2} {:.2} {:.2}", h.spot, p.pos.x, p.pos.y, p.pos.z);
            }
            p.hide = None;
            p.hidden = false;
            p.fall_from = p.pos.z;
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
        let (pose, outcome) = cl.update(cc, sk, ground, want, input.jump, input.run, dt);
        p.pos = cl.pos;
        p.yaw = crate::locomotion::norm_yaw(cl.yaw);
        if crate::trace() && (time * 4.0).floor() != ((time - dt) * 4.0).floor() {
            eprintln!("t {:.2} pos {:.2} {:.2} {:.2} yaw {:.0} {}", time, p.pos.x, p.pos.y, p.pos.z, p.yaw.to_degrees(), cl.describe());
        }
        match outcome {
            Outcome::Climbing => {}
            Outcome::Stand(pos, yaw) => {
                p.climb = None;
                p.pos = pos;
                p.yaw = crate::locomotion::norm_yaw(yaw);
                p.vel = Vec3::ZERO;
                p.loco.resume(pose.clone());
            }
            Outcome::Drop(v) | Outcome::Eject(v) => {
                if matches!(outcome, Outcome::Eject(_)) {
                    p.catch_block = EJECT_NO_CATCH;
                    if crate::trace() {
                        eprintln!("eject at {:.2?} v {:.2?}", p.pos.to_array(), v.to_array());
                    }
                }
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
    // into a hiding spot: running into one, or falling onto it
    if let Some(hide) = &clipset.hide {
        let air = p.loco.airborne() && p.flight.is_none();
        let high = input.run || input.sprint;
        if let Some(i) = crate::hide::entry(&hide.spots, p.pos, dir.normalize_or_zero(), high, air, p.vel.z) {
            if crate::trace() {
                eprintln!("hide: enter spot {i} from {:.2} {:.2} {:.2}{}", p.pos.x, p.pos.y, p.pos.z, if air { " (air)" } else { "" });
            }
            let pose = p.loco.last_pose().clone();
            p.hide = Some(crate::hide::Hiding::enter(hide, i, p.pos, p.yaw, air, pose.clone(), bodies[me].world));
            p.loco.resume(pose);
            p.vel = Vec3::ZERO;
            p.approach = None;
            p.fall_from = p.pos.z;
            let input = PlayerInput { kill: false, stun: false, ..*input };
            return drive(p, bodies, clipset, ground, &input, rules, relation, seen, time, 0.0);
        }
    }
    // balancing on a beam (railing, narrow wall top): beam clip set, moves
    // kept on its centre line, no climb entry
    let beam = match ground {
        Some(g) if p.flight.is_none() && !p.loco.airborne() => g.beam(p.pos),
        _ => None,
    };
    p.loco.set_beam(clips, sk, beam.is_some());
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
                p.loco.set_beam(clips, sk, ground.beam(at).is_some());
                p.loco.land(0.0, dir != Vec2::ZERO);
            }
        } else if p.loco.airborne() {
            p.vel.z -= jump::GRAVITY * dt;
            p.catch_block = (p.catch_block - dt).max(0.0);
            // fall height counts from the highest point (retail
            // ComputeFallHeight: InAirData start/peak z - z)
            p.fall_from = p.fall_from.max(p.pos.z);
            // falling past a hold: grab it (not right after letting go)
            let fwd = Vec2::new(-p.yaw.sin(), p.yaw.cos());
            if p.vel.z < -1.0 && clipset.climbs.is_some() && p.fall_from - p.pos.z > 0.3 && p.catch_block <= 0.0 {
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
                    p.loco.set_beam(clips, sk, ground.beam(at).is_some());
                    let height = p.fall_from - f;
                    let (damage, kind) = jump::landing(height);
                    if crate::trace() {
                        eprintln!("land: fall {height:.2} m {kind:?} damage {damage}% at {:.2?}", at.to_array());
                    }
                    p.landing = Some((height, damage, kind));
                    p.loco.land(height, dir != Vec2::ZERO);
                }
                _ => at.z = z,
            }
            p.pos = at;
        } else {
            let running = p.loco.speed >= 1.5 && dir != Vec2::ZERO;
            let fwd = Vec2::new(-p.yaw.sin(), p.yaw.cos());
            // freerun: up onto a ledge ahead
            let up = running.then(|| jump::target(ground, clips, p.pos, fwd, false)).flatten();
            let (to, ledge) = ground.move_on(p.pos, jump::balance_on(beam, delta));
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
            // Retail's beam state (HumanNarrowObjectBeam) has no wall-run or
            // vertical-stance climb transition: off the beam first.
            if clipset.climbs.is_some() && up.is_none() && dir != Vec2::ZERO && beam.is_none() {
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
            // low profile at a drop (a roof edge, a beam end): stop at the
            // edge (a beam end at any drop); the jump button lowers onto the
            // wall below or steps off
            let deep = |g: &Ground| match g.floor_below(to + Vec3::Z * 0.1) {
                Some(f) => p.pos.z - f > LEDGE_STOP,
                None => true,
            };
            if ledge && !input.run && up.is_none() && len > 1e-4 && (beam.is_some() || deep(ground)) {
                let along = delta / len;
                if input.jump {
                    let hang = clipset.climbs.as_ref().and_then(|_| ground.ledge_hold(p.pos, along));
                    if let Some((hold, point)) = hang {
                        let stance = ground.hang_stance(point, hold.wall);
                        let c = Climb::pulldown(hold, point, stance, p.pos, p.yaw, p.loco.last_pose().clone());
                        if crate::trace() {
                            eprintln!("climb: {} from {:.2} {:.2} {:.2}", c.describe(), p.pos.x, p.pos.y, p.pos.z);
                        }
                        p.climb = Some(c);
                        p.vel = Vec3::ZERO;
                        return;
                    }
                } else {
                    p.loco.blocked(dt);
                    if crate::trace() && (time * 4.0).floor() != ((time - dt) * 4.0).floor() {
                        eprintln!("ledge stop at {:.2} {:.2} {:.2}", p.pos.x, p.pos.y, p.pos.z);
                    }
                    let world = p.entity() * bodies[me].model;
                    bodies[me].show(world, step.pose);
                    return;
                }
            }
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
