//! Wall climbing and hanging on freerun guidance edges.
//!
//! Getting onto a wall follows the retail entry rules (NOTES.md, "Climbing:
//! retail entry"): running in high profile at a wall starts a wall run
//! (`HumanJumpDetector::DoWallingRaycastTest`: a 1.5 m ray at 1.3 m, facing
//! and clearance tests) that grabs the lowest edge within reach (up to
//! 4.3 m) and hangs from it; pushing into a wall with a foot hold and a
//! hand hold 1.2 m above it climbs straight on
//! (`Human::CheckForClimbEntryFromVerticalStance`).
//!
//! On the wall the character is in one of three stances, each with its own
//! pose and hand placement relative to the entity (measured from the
//! clips' hand bones): `Climb` (`xx_climb_wait_1m`, feet on a lower hold),
//! `HangWall` (`xx_h_hangwall_wait`, feet against the wall) and `HangFree`
//! (`xx_h_hangfree_wait`, nothing below). Every move plays a short sequence
//! of the game's clips and carries the entity from its start to its target
//! in proportion to the clips' own root progress (horizontal, vertical and
//! yaw separately), so any hold spacing works: climb moves between holds,
//! shimmies along an edge, inner and outer corner turns, climb <-> hang,
//! passover onto the top, steps down to the ground and drops.

use std::collections::HashMap;

use acb_anim::pose::{Clip, Skeleton};
use acb_anim::BuiltinTracks;
use acb_world::Archive;
use glam::{Mat4, Quat, Vec2, Vec3};

use crate::ground::{cell, Ground};
use crate::Pose;

/// A guidance edge as a climbing surface.
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    pub a: Vec3,
    pub b: Vec3,
    /// Outward horizontal normal of the wall below the edge.
    pub wall: Vec2,
    /// The edge has walkable ground on top.
    pub top: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stance {
    Climb,
    HangWall,
    HangFree,
}

impl Stance {
    /// Hands relative to the entity (forward, up), from the wait poses'
    /// hand bones (`xx_climb_wait_1m` 0.44 / 1.14, `xx_h_hangwall_wait`
    /// 0.45 / 1.0, `xx_h_hangfree_wait` -0.04 / 2.30).
    pub fn hands(self) -> (f32, f32) {
        match self {
            Stance::Climb => (0.44, 1.14),
            Stance::HangWall => (0.45, 1.0),
            Stance::HangFree => (-0.04, 2.30),
        }
    }

    fn wait(self) -> &'static str {
        match self {
            Stance::Climb => "xx_climb_wait_1m",
            Stance::HangWall => "xx_h_hangwall_wait",
            Stance::HangFree => "xx_h_hangfree_wait",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// Every clip the climber plays.
const CLIPS: &[&str] = &[
    "xx_climb_wait_1m",
    "xx_h_hangwall_wait",
    "xx_h_hangfree_wait",
    "xx_h_wait_hipm_footl_tr_climbing_1m",
    // wall run
    "xx_h_impultionstraight_footl_to_wallingfront_step1_footr",
    "xx_h_wallingfront_entry_footl_a",
    "xx_h_wallingfront_step1_footr",
    "xx_h_wallingfront_step1_footr_tr_hangwall_251cm_000cm_a",
    "xx_h_wallingfront_step1_footr_tr_hangwall_430cm_000cm_a",
    // hang
    "xx_h_hangwall_strafe_left_050cm_open",
    "xx_h_hangwall_strafe_left_050cm_close",
    "xx_h_hangwall_strafe_right_050cm_open",
    "xx_h_hangwall_strafe_right_050cm_close",
    "xx_h_hangfree_strafe_left_050cm_open",
    "xx_h_hangfree_strafe_left_050cm_close",
    "xx_h_hangfree_strafe_right_050cm_open",
    "xx_h_hangfree_strafe_right_050cm_close",
    "xx_h_hangwall_corner_left_090_in",
    "xx_h_hangwall_corner_left_090_out",
    "xx_h_hangwall_corner_right_090_in",
    "xx_h_hangwall_corner_right_090_out",
    "xx_h_hangfree_corner_left_090_in",
    "xx_h_hangfree_corner_left_090_out",
    "xx_h_hangfree_corner_right_090_in",
    "xx_h_hangfree_corner_right_090_out",
    "xx_h_hangwall_tr_passover_handr",
    "xx_h_passover_handr_030cm",
    "xx_h_hangwall_tr_fall_a",
    "xx_h_hangfree_tr_fall_a",
    "xx_h_hangwall_d_climb_1m",
    "xx_h_climbing_climb1m_tr_hangwall_down_2_a",
    "xx_h_climbing_climb1m_tr_hangwall_down_2_b",
    "xx_h_climbing_climb1m_tr_hangwall_down_2_c",
    "xx_l_climb_1m_u_hangwall",
    // back to the ground
    "xx_l_climb_1m_tr_h_wait_hipm_footl_a",
    "xx_l_climb_1m_to_groundentry_left",
    "xx_l_climb_1m_to_groundentry_left_tr_h_wait_footl",
    "xx_l_climb_1m_to_groundentry_right",
    "xx_l_climb_1m_to_groundentry_right_tr_h_wait_footr",
];

pub struct ClimbClips {
    clips: HashMap<String, Clip>,
    /// Idle Reference bone (position, yaw), for standing up again.
    neutral: (Vec3, f32),
}

impl ClimbClips {
    pub fn load(archive: &Archive, builtins: Option<&BuiltinTracks>) -> anyhow::Result<ClimbClips> {
        let uids = archive.uids_of("Animation");
        let load = |name: &str| {
            uids.iter()
                .copied()
                .find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(name)))
                .and_then(|u| Clip::load_with(archive, u, builtins))
                .ok_or_else(|| anyhow::anyhow!("{name}: animation not found"))
        };
        let mut clips = HashMap::new();
        let mut names: Vec<String> = CLIPS.iter().map(|s| s.to_string()).collect();
        for (dir, hand) in [("u", "lhand"), ("d", "lhand"), ("l", "lhand"), ("r", "rhand")] {
            for n in [2, 3] {
                names.push(format!("xx_h_climb_1m_tr_climb_{dir}_{hand}_{n}"));
                names.push(format!("xx_h_climb_{dir}_{hand}_{n}"));
                names.push(format!("xx_h_climb_{dir}_{hand}_{n}_tr_climb_1m"));
            }
        }
        for n in [2, 3] {
            for s in ["a", "b", "c"] {
                names.push(format!("xx_h_climbing_hangwall_tr_climb1m_up_l_hand_{n}_{s}"));
            }
        }
        for n in &names {
            clips.insert(n.clone(), load(n)?);
        }
        let idle = load("xx_l_wait_hipm_footl")?;
        let neutral = crate::kill::reference(&idle, 0.0).unwrap_or((Vec3::Z * 0.96, 66f32.to_radians()));
        Ok(ClimbClips { clips, neutral })
    }

    fn get(&self, name: &str) -> &Clip {
        &self.clips[name]
    }
}

/// What happens when a move's clips are done.
#[derive(Clone, Copy, Debug)]
enum Then {
    /// On the wall at `hold` in `stance`.
    Hold(Stance, Hold),
    /// Let go: fall with this velocity.
    Drop(Vec3),
    /// Back on the ground (stand up where the last pose stands).
    Stand,
}

/// A sequence of clips played back to back while the entity is carried
/// from `from` to `to` (and turned from `yaw0` to `yaw1`).
#[derive(Clone)]
struct Seq {
    clips: Vec<&'static str>,
    names: Vec<String>,
    stage: usize,
    t: f32,
    from: Vec3,
    to: Vec3,
    yaw0: f32,
    yaw1: f32,
    then: Then,
}

#[derive(Clone)]
enum Phase {
    Wait(f32),
    /// Grabbed a hold while falling: glide onto it.
    Catch(f32),
    Seq(Seq),
}

#[derive(Clone)]
pub struct Climb {
    pub hold: Hold,
    pub stance: Stance,
    /// Entity position and facing (yaw, entity +Y = (-sin, cos)).
    pub pos: Vec3,
    pub yaw: f32,
    phase: Phase,
    from: Vec3,
    /// Pose blended out of over the first [`FADE`] seconds.
    fade: Option<(Pose, f32)>,
    /// What the last move was, for the trace.
    pub last: &'static str,
    /// Time the next hold wait ignores the stick for (a mount settling
    /// into the wall before the first move, as retail).
    settle: f32,
}

const FADE: f32 = 0.15;

/// How long the hold wait after a standing mount lets the pose settle
/// before a held stick starts the first move.
const MOUNT_SETTLE: f32 = 0.3;

/// Facing into a wall with outward normal `n`.
pub fn yaw_into(n: Vec2) -> f32 {
    n.x.atan2(-n.y)
}

/// The character's right on a wall facing `n` (it faces -n).
pub fn right_of(n: Vec2) -> Vec2 {
    Vec2::new(-n.y, n.x)
}

/// Entity position that puts the hands on `hold_point` of a wall facing
/// `n` in `stance`.
pub fn entity_for(hold_point: Vec3, n: Vec2, stance: Stance) -> Vec3 {
    let (f, u) = stance.hands();
    hold_point + (n * f).extend(0.0) - Vec3::Z * u
}

fn hands_of(pos: Vec3, n: Vec2, stance: Stance) -> Vec3 {
    let (f, u) = stance.hands();
    pos - (n * f).extend(0.0) + Vec3::Z * u
}

/// Shortest signed angle from `a` to `b`.
fn turn(a: f32, b: f32) -> f32 {
    let mut d = (b - a) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    } else if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

impl Hold {
    /// The point of the edge nearest `p` horizontally (kept 5% off the
    /// ends).
    pub fn nearest(&self, p: Vec3) -> Vec3 {
        self.at(self.param(p).clamp(0.05, 0.95))
    }

    /// Edge parameter (0 at `a`, 1 at `b`) of the point nearest `p`
    /// horizontally, unclamped.
    pub fn param(&self, p: Vec3) -> f32 {
        let ab = (self.b - self.a).truncate();
        if ab.length_squared() > 1e-6 {
            (p - self.a).truncate().dot(ab) / ab.length_squared()
        } else {
            0.5
        }
    }

    pub fn at(&self, t: f32) -> Vec3 {
        self.a + (self.b - self.a) * t
    }

    pub fn length(&self) -> f32 {
        (self.b - self.a).truncate().length()
    }
}

/// Per-axis progress of a clip sequence: how far the root has got, 0..1,
/// horizontally, vertically and in yaw; an axis the clips do not move
/// along progresses with time.
fn seq_progress(clips: &[&Clip], stage: usize, t: f32) -> (f32, f32, f32) {
    let ends: Vec<(f32, f32, f32, f32)> = clips
        .iter()
        .map(|c| {
            let m = c.root_motion(c.length);
            let (_, r, p) = m.to_scale_rotation_translation();
            (p.truncate().length(), p.z.abs(), r.to_euler(glam::EulerRot::ZXY).0.abs(), c.length)
        })
        .collect();
    let total = ends.iter().fold((0.0, 0.0, 0.0, 0.0), |s, e| (s.0 + e.0, s.1 + e.1, s.2 + e.2, s.3 + e.3));
    let (mut h, mut z, mut y, mut tt) = (0.0, 0.0, 0.0, 0.0);
    for (i, (c, e)) in clips.iter().zip(&ends).enumerate() {
        if i < stage {
            (h, z, y, tt) = (h + e.0, z + e.1, y + e.2, tt + e.3);
            continue;
        }
        if i == stage {
            let tc = t.clamp(0.0, c.length);
            let (_, r, p) = c.root_motion(tc).to_scale_rotation_translation();
            let end = c.root_motion(c.length).w_axis.truncate();
            let fh = if e.0 > 1e-3 { (p.truncate().dot(end.truncate()) / (e.0 * e.0)).clamp(0.0, 1.0) } else { 0.0 };
            let fz = if e.1 > 1e-3 { (p.z / end.z).clamp(0.0, 1.0) } else { 0.0 };
            let fy = if e.2 > 1e-3 { (r.to_euler(glam::EulerRot::ZXY).0.abs() / e.2).clamp(0.0, 1.0) } else { 0.0 };
            (h, z, y, tt) = (h + e.0 * fh, z + e.1 * fz, y + e.2 * fy, tt + tc);
        }
        break;
    }
    let time = if total.3 > 0.0 { (tt / total.3).min(1.0) } else { 1.0 };
    let f = |v: f32, tot: f32| if tot > 0.02 { (v / tot).min(1.0) } else { time };
    (f(h, total.0), f(z, total.1), f(y, total.2))
}

pub enum Outcome {
    Climbing,
    /// Let go: fall from the current position with this velocity.
    Drop(Vec3),
    /// Standing again at this entity position and facing.
    Stand(Vec3, f32),
}

impl Climb {
    fn new(hold: Hold, stance: Stance, pos: Vec3, yaw: f32, phase: Phase, fade: Option<Pose>) -> Climb {
        Climb { hold, stance, pos, yaw, phase, from: pos, fade: fade.map(|p| (p, 0.0)), last: "enter", settle: 0.0 }
    }

    /// Climbs straight onto the wall from standing at `pos` (retail
    /// vertical-stance entry): `hold` is the hand hold.
    pub fn mount(hold: Hold, point: Vec3, pos: Vec3, yaw: f32, from: Pose) -> Climb {
        let to = entity_for(point, hold.wall, Stance::Climb);
        let seq = Seq::new(&["xx_h_wait_hipm_footl_tr_climbing_1m"], pos, to, yaw, yaw_into(hold.wall), Then::Hold(Stance::Climb, hold));
        let mut c = Climb::new(hold, Stance::Climb, pos, yaw, Phase::Seq(seq), Some(from));
        c.last = "mount";
        c.settle = MOUNT_SETTLE;
        c
    }

    /// Runs up the wall at `wall` (a point on it, outward normal `n`) from
    /// feet at `pos` and grabs `hold` at `point`, or comes back down when
    /// there is none.
    pub fn wall_run(target: Option<(Hold, Vec3)>, wall: Vec3, n: Vec2, pos: Vec3, yaw: f32, from: Pose) -> Climb {
        let run = ["xx_h_impultionstraight_footl_to_wallingfront_step1_footr", "xx_h_wallingfront_entry_footl_a", "xx_h_wallingfront_step1_footr"];
        let yaw1 = yaw_into(n);
        let (seq, hold) = match target {
            Some((hold, point)) => {
                let to = entity_for(point, hold.wall, Stance::HangWall);
                // the 251 cm grab ends with the hands 2.51 m above the
                // feet, the 430 cm one rises 1.8 m more
                let grab = if point.z - pos.z > 2.9 {
                    "xx_h_wallingfront_step1_footr_tr_hangwall_430cm_000cm_a"
                } else {
                    "xx_h_wallingfront_step1_footr_tr_hangwall_251cm_000cm_a"
                };
                let clips = [run[0], run[1], run[2], grab];
                (Seq::new(&clips, pos, to, yaw, yaw_into(hold.wall), Then::Hold(Stance::HangWall, hold)), hold)
            }
            None => {
                // up 1.5 m against the wall, then push off it
                let to = (wall + (n * 0.5).extend(0.0)).with_z(pos.z + 1.5);
                let hold = Hold { a: wall, b: wall, wall: n, top: false };
                (Seq::new(&run, pos, to, yaw, yaw1, Then::Drop((n * 1.5).extend(0.0))), hold)
            }
        };
        let mut c = Climb::new(hold, Stance::HangWall, pos, yaw, Phase::Seq(seq), Some(from));
        c.last = if target.is_some() { "wall run" } else { "wall run (nothing to grab)" };
        c
    }

    /// Grabs `hold` while falling past it at `pos`.
    pub fn catch(hold: Hold, stance: Stance, pos: Vec3, from: Pose) -> Climb {
        let yaw = yaw_into(hold.wall);
        let mut c = Climb::new(hold, stance, pos, yaw, Phase::Catch(0.0), Some(from));
        c.last = "catch";
        c
    }

    /// Short state description for traces.
    pub fn describe(&self) -> String {
        let phase = match &self.phase {
            Phase::Wait(_) => "wait".to_string(),
            Phase::Catch(_) => "catch".to_string(),
            Phase::Seq(s) => format!("{} [{}]", self.last, s.clips[s.stage]),
        };
        format!("CLIMB {:?} {phase}", self.stance)
    }

    /// Advances one tick; `want` is the stick direction on the wall.
    pub fn update(&mut self, clips: &ClimbClips, sk: &Skeleton, ground: &Ground, want: Option<Dir>, dt: f32) -> (Pose, Outcome) {
        let (pose, outcome) = self.step(clips, sk, ground, want, dt);
        let pose = match &mut self.fade {
            Some((from, t)) => {
                *t += dt;
                let w = (*t / FADE).min(1.0);
                let mut p = from.clone();
                acb_anim::pose::blend(&mut p, &pose, w);
                if w >= 1.0 {
                    self.fade = None;
                }
                p
            }
            None => pose,
        };
        (pose, outcome)
    }

    fn step(&mut self, clips: &ClimbClips, sk: &Skeleton, ground: &Ground, want: Option<Dir>, dt: f32) -> (Pose, Outcome) {
        let n = self.hold.wall;
        match &mut self.phase {
            Phase::Catch(t) => {
                const GLIDE: f32 = 0.15;
                *t += dt;
                let target = entity_for(self.hold.nearest(self.from), n, self.stance);
                let k = (*t / GLIDE).min(1.0);
                self.pos = self.from.lerp(target, k * k * (3.0 - 2.0 * k));
                if *t >= GLIDE {
                    self.pos = target;
                    self.phase = Phase::Wait(0.0);
                }
                (clips.get(self.stance.wait()).sample(sk, 0.0), Outcome::Climbing)
            }
            Phase::Wait(t) => {
                *t += dt;
                let wait = clips.get(self.stance.wait());
                let pose = wait.sample(sk, *t % wait.length.max(1e-3));
                let settled = *t >= self.settle;
                if let Some(dir) = want.filter(|_| settled) {
                    if let Some((seq, name)) = self.plan(ground, dir) {
                        self.last = name;
                        self.settle = 0.0;
                        self.phase = Phase::Seq(seq);
                    }
                }
                (pose, Outcome::Climbing)
            }
            Phase::Seq(s) => {
                s.t += dt;
                let list: Vec<&Clip> = s.names.iter().map(|c| clips.get(c)).collect();
                let c = list[s.stage];
                let (fh, fz, fy) = seq_progress(&list, s.stage, s.t);
                let h = s.from.truncate().lerp(s.to.truncate(), fh);
                self.pos = h.extend(s.from.z + (s.to.z - s.from.z) * fz);
                self.yaw = s.yaw0 + turn(s.yaw0, s.yaw1) * fy;
                let pose = c.sample(sk, s.t.min(c.length));
                if s.t < c.length {
                    return (pose, Outcome::Climbing);
                }
                if s.stage + 1 < s.clips.len() {
                    s.t -= c.length;
                    s.stage += 1;
                    return (pose, Outcome::Climbing);
                }
                self.pos = s.to;
                self.yaw = s.yaw1;
                match s.then {
                    Then::Hold(stance, hold) => {
                        self.hold = hold;
                        self.stance = stance;
                        self.yaw = yaw_into(hold.wall);
                        self.phase = Phase::Wait(0.0);
                        (pose, Outcome::Climbing)
                    }
                    Then::Drop(v) => (pose, Outcome::Drop(v)),
                    Then::Stand => {
                        // stand where the last pose's Reference stands
                        let frame = Mat4::from_rotation_translation(Quat::from_rotation_z(self.yaw), self.pos);
                        let (pos, yaw) = match crate::kill::reference(c, c.length) {
                            Some((rp, ry)) => crate::kill::reroot(frame, rp, ry, clips.neutral),
                            None => (self.pos, self.yaw),
                        };
                        (pose, Outcome::Stand(pos.with_z(s.to.z), yaw))
                    }
                }
            }
        }
    }

    /// The move for stick direction `dir` from the current hold, if any.
    fn plan(&self, ground: &Ground, dir: Dir) -> Option<(Seq, &'static str)> {
        let n = self.hold.wall;
        let hand = hands_of(self.pos, n, self.stance);
        let yaw = self.yaw;
        let seq = |clips: &[&'static str], to: Vec3, yaw1: f32, then: Then| Seq::new(clips, self.pos, to, yaw, yaw1, then);
        let hold_at = |stance: Stance, hold: Hold, point: Vec3| {
            (entity_for(point, hold.wall, stance), yaw_into(hold.wall), Then::Hold(stance, hold))
        };
        let d = [Dir::Up, Dir::Down, Dir::Left, Dir::Right].iter().position(|x| *x == dir).unwrap();
        let (dname, hname) = [("u", "lhand"), ("d", "lhand"), ("l", "lhand"), ("r", "rhand")][d];
        let triple = |dist: f32| -> [&'static str; 3] {
            let k = if dist > 1.5 { 3 } else { 2 };
            [
                leak(format!("xx_h_climb_1m_tr_climb_{dname}_{hname}_{k}")),
                leak(format!("xx_h_climb_{dname}_{hname}_{k}")),
                leak(format!("xx_h_climb_{dname}_{hname}_{k}_tr_climb_1m")),
            ]
        };
        match (self.stance, dir) {
            (Stance::Climb, _) => {
                if let Some((hold, point)) = ground.next_hold(hand, n, dir) {
                    // up onto a ledge: hang from it
                    if dir == Dir::Up && hold.top {
                        let (to, y1, then) = hold_at(Stance::HangWall, hold, point);
                        return Some((seq(&["xx_l_climb_1m_u_hangwall"], to, y1, then), "climb to hang"));
                    }
                    // down onto a hold with nothing below it for the feet:
                    // hang from it against the wall
                    if dir == Dir::Down && ground.next_hold(point, hold.wall, Dir::Down).is_none() {
                        let (to, y1, then) = hold_at(Stance::HangWall, hold, point);
                        let clips = [
                            "xx_h_climbing_climb1m_tr_hangwall_down_2_a",
                            "xx_h_climbing_climb1m_tr_hangwall_down_2_b",
                            "xx_h_climbing_climb1m_tr_hangwall_down_2_c",
                        ];
                        return Some((seq(&clips, to, y1, then), "climb down to hang"));
                    }
                    let (to, y1, then) = hold_at(Stance::Climb, hold, point);
                    return Some((seq(&triple(to.distance(self.pos)), to, y1, then), "climb"));
                }
                match dir {
                    // the hands are on the top edge already: hang from it
                    Dir::Up if self.hold.top => {
                        let (to, y1, then) = hold_at(Stance::HangWall, self.hold, self.hold.nearest(hand));
                        Some((seq(&["xx_l_climb_1m_u_hangwall"], to, y1, then), "climb to hang"))
                    }
                    Dir::Down => match ground.floor_below(self.pos + Vec3::Z * 0.2) {
                        // close above the ground: step down
                        Some(f) if self.pos.z - f < 1.0 => {
                            let to = (self.pos + (n * 0.3).extend(0.0)).with_z(f);
                            Some((seq(&["xx_l_climb_1m_tr_h_wait_hipm_footl_a"], to, yaw, Then::Stand), "step down"))
                        }
                        _ => Some((seq(&[], self.pos, yaw, Then::Drop((n * 1.0).extend(0.0))), "let go")),
                    },
                    Dir::Left | Dir::Right => {
                        // at the bottom: step off sideways
                        let side = right_of(n) * if dir == Dir::Left { -1.0 } else { 1.0 };
                        let spot = self.pos + (side * 1.0 + n * 0.5).extend(0.0);
                        let f = ground.floor_below(spot + Vec3::Z * 0.3)?;
                        (self.pos.z - f < 1.0 && ground.sweep_wall(self.pos + Vec3::Z * 0.5, side * 1.0, 0.3).is_none()).then(|| {
                            let clips: [&'static str; 2] = if dir == Dir::Left {
                                ["xx_l_climb_1m_to_groundentry_left", "xx_l_climb_1m_to_groundentry_left_tr_h_wait_footl"]
                            } else {
                                ["xx_l_climb_1m_to_groundentry_right", "xx_l_climb_1m_to_groundentry_right_tr_h_wait_footr"]
                            };
                            (seq(&clips, spot.with_z(f), yaw, Then::Stand), "step off")
                        })
                    }
                    _ => None,
                }
            }
            (hang, Dir::Up) => {
                // over the top onto the ground behind the edge
                if self.hold.top {
                    let spot = self.hold.nearest(hand) - (n * 0.7).extend(0.0);
                    if let Some(f) = ground.floor_below(spot + Vec3::Z * 0.5).filter(|f| (f - spot.z).abs() < 0.5) {
                        let clips = ["xx_h_hangwall_tr_passover_handr", "xx_h_passover_handr_030cm"];
                        return Some((seq(&clips, spot.with_z(f), yaw, Then::Stand), "passover"));
                    }
                }
                if hang == Stance::HangWall {
                    let (hold, point) = ground.next_hold(hand, n, Dir::Up)?;
                    let (to, y1, then) = hold_at(Stance::Climb, hold, point);
                    let k = if to.distance(self.pos) > 1.5 { 3 } else { 2 };
                    let clips: Vec<&'static str> =
                        ["a", "b", "c"].iter().map(|s| leak(format!("xx_h_climbing_hangwall_tr_climb1m_up_l_hand_{k}_{s}"))).collect();
                    return Some((seq(&clips, to, y1, then), "hang to climb"));
                }
                None
            }
            (hang, Dir::Down) => {
                if hang == Stance::HangWall {
                    if let Some((hold, point)) = ground.next_hold(hand, n, Dir::Down) {
                        let (to, y1, then) = hold_at(Stance::Climb, hold, point);
                        return Some((seq(&["xx_h_hangwall_d_climb_1m"], to, y1, then), "hang down to climb"));
                    }
                }
                // let go
                let (clip, to) = if hang == Stance::HangWall {
                    ("xx_h_hangwall_tr_fall_a", self.pos + (n * 0.3).extend(0.0) - Vec3::Z * 1.0)
                } else {
                    ("xx_h_hangfree_tr_fall_a", self.pos)
                };
                Some((seq(&[clip], to, yaw, Then::Drop((n * 1.0).extend(0.0))), "drop"))
            }
            (hang, side) => {
                let sign = if side == Dir::Left { -1.0 } else { 1.0 };
                let wall = if hang == Stance::HangWall { "hangwall" } else { "hangfree" };
                let sname = if side == Dir::Left { "left" } else { "right" };
                match ground.shimmy(&self.hold, hand, sign) {
                    Some(Shimmy::Along(hold, point)) => {
                        let (to, y1, then) = hold_at(hang, hold, point);
                        let clips = [
                            leak(format!("xx_h_{wall}_strafe_{sname}_050cm_open")),
                            leak(format!("xx_h_{wall}_strafe_{sname}_050cm_close")),
                        ];
                        Some((seq(&clips, to, y1, then), "shimmy"))
                    }
                    Some(Shimmy::Corner(hold, point, outer)) => {
                        let (to, y1, then) = hold_at(hang, hold, point);
                        let kind = if outer { "out" } else { "in" };
                        let clip = leak(format!("xx_h_{wall}_corner_{sname}_090_{kind}"));
                        Some((seq(&[clip], to, y1, then), if outer { "outer corner" } else { "inner corner" }))
                    }
                    None => None,
                }
            }
        }
    }
}

/// Clip names live as long as the program (a few dozen distinct ones).
fn leak(s: String) -> &'static str {
    use std::sync::Mutex;
    static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut names = NAMES.lock().unwrap();
    if let Some(n) = names.iter().find(|n| **n == s) {
        return n;
    }
    let n: &'static str = Box::leak(s.into_boxed_str());
    names.push(n);
    n
}

impl Seq {
    fn new(clips: &[&'static str], from: Vec3, to: Vec3, yaw0: f32, yaw1: f32, then: Then) -> Seq {
        // an empty sequence ends at once (on the wait pose)
        let clips: Vec<&'static str> = if clips.is_empty() { vec!["xx_h_hangwall_wait"] } else { clips.to_vec() };
        let empty = clips == ["xx_h_hangwall_wait"];
        Seq {
            names: clips.iter().map(|s| s.to_string()).collect(),
            clips,
            stage: 0,
            t: if empty { f32::INFINITY } else { 0.0 },
            from,
            to,
            yaw0,
            yaw1,
            then,
        }
    }
}

/// Where a shimmy goes.
#[derive(Clone, Copy, Debug)]
pub enum Shimmy {
    /// Along the edge (or one continuing it) to this point.
    Along(Hold, Vec3),
    /// Round a 90 degree corner onto another edge: outer (convex) or
    /// inner.
    Corner(Hold, Vec3, bool),
}

/// One shimmy step (an open + close strafe pair), metres.
pub const SHIMMY_STEP: f32 = 1.0;

/// Retail walling test constants (`HumanJumpDetector::DoWallingRaycastTest`,
/// `CalculateWallingRaycastDirDepth`).
pub const WALLING_RAY_HEIGHT: f32 = 1.3;
pub const WALLING_RAY_DEPTH: f32 = 1.5;

/// Largest angle between the move direction and the wall's inward normal
/// a wall run accepts at `speed` m/s: 45 degrees when nearly still, else
/// from 0.5 rad at 1 m/s down to 1 - cos 50 = 0.357 rad at 6 m/s.
pub fn walling_tolerance(speed: f32) -> f32 {
    if speed <= 0.35 {
        return std::f32::consts::FRAC_PI_4;
    }
    let (a, b) = (0.5, 50f32.to_radians().cos());
    let f = if speed < 1.0 {
        a
    } else if speed > 6.0 {
        b
    } else {
        a + (b - a) * (speed - 1.0) / 5.0
    };
    1.0 - f
}

/// Hold height above the feet a wall run reaches (the 251 cm and 430 cm
/// grabs; lower edges are climbed or jumped onto).
pub const WALL_RUN_REACH: (f32, f32) = (1.6, 4.4);

/// Hold queries on the map's guidance edges.
impl Ground {
    fn holds_near(&self, p: Vec3, r: f32) -> impl Iterator<Item = &Hold> + '_ {
        let (c0, c1) = (cell(p.truncate() - Vec2::splat(r)), cell(p.truncate() + Vec2::splat(r)));
        let mut ids: Vec<u32> =
            (c0.0..=c1.0).flat_map(|x| (c0.1..=c1.1).map(move |y| (x, y))).filter_map(|k| self.hold_grid.get(&k)).flatten().copied().collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().map(|i| &self.holds[i as usize])
    }

    /// Holds with an edge point within `r` of `p` horizontally (debugging,
    /// tests).
    pub fn holds_around(&self, p: Vec3, r: f32) -> Vec<Hold> {
        self.holds_near(p, r).filter(|h| (h.nearest(p) - p).truncate().length() < r).copied().collect()
    }

    /// Retail wall-run test from feet at `pos` facing `fwd` moving at
    /// `vel` (m/s): a horizontal ray 1.5 m ahead at 1.3 m must hit a wall
    /// that faces the move closely enough ([`walling_tolerance`]) with
    /// nothing in the way at knee height. Returns the hit and the wall's
    /// outward normal.
    pub fn wall_ahead(&self, pos: Vec3, fwd: Vec2, vel: Vec2) -> Option<(Vec3, Vec2)> {
        let origin = pos + Vec3::Z * WALLING_RAY_HEIGHT;
        let (f, n) = self.sweep_wall(origin, fwd * WALLING_RAY_DEPTH, 0.0)?;
        let hit = origin + (fwd * WALLING_RAY_DEPTH * f).extend(0.0);
        let speed = vel.length();
        let against = if speed > 0.35 { vel / speed } else { fwd };
        if against.dot(-n).clamp(-1.0, 1.0).acos() > walling_tolerance(speed) {
            return None;
        }
        // knee-high clearance up to half a metre off the wall
        let short = (hit.truncate() + n * 0.5) - origin.truncate();
        if short.dot(fwd) > 0.0 && self.sweep_wall(origin - Vec3::Z * 1.0, short, 0.0).is_some() {
            return None;
        }
        Some((hit, n))
    }

    /// The hold a wall run up the wall at `hit` (outward normal `n`) from
    /// feet at height `feet` grabs: the lowest edge on that wall within
    /// half a metre sideways, [`WALL_RUN_REACH`] above the feet.
    pub fn wall_run_hold(&self, hit: Vec3, n: Vec2, feet: f32) -> Option<(Hold, Vec3)> {
        let right = right_of(n);
        self.holds_near(hit, 1.0)
            .filter(|h| h.wall.dot(n) > 0.7)
            .filter_map(|h| {
                let q = h.nearest(hit);
                let d = (q - hit).truncate();
                let dz = q.z - feet;
                (d.dot(right).abs() < 0.5 && d.dot(n).abs() < 0.4 && (WALL_RUN_REACH.0..=WALL_RUN_REACH.1).contains(&dz)).then_some((dz, *h, q))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| (x.1, x.2))
    }

    /// Retail vertical-stance climb entry from feet at `pos` facing `fwd`:
    /// a foot hold 0.95 m ahead within 0.6 m of the feet's height (searched
    /// at 0, +0.6 and -0.6 m) on a wall facing us, and a hand hold 1.2 m
    /// above it. Returns the hand hold and its point.
    pub fn climb_entry(&self, pos: Vec3, fwd: Vec2) -> Option<(Hold, Vec3)> {
        let side = Vec2::new(fwd.y, -fwd.x);
        let ahead = pos + (fwd * 0.95).extend(0.0);
        let facing = |h: &Hold| h.wall.dot(fwd) < -std::f32::consts::FRAC_1_SQRT_2;
        // inside the entry box: 0.75 m either side, 0..1.2 m ahead,
        // -0.9..2.1 m up
        let in_box = |q: Vec3| {
            let d = q - pos;
            d.truncate().dot(side).abs() <= 0.75 && (0.0..=1.2).contains(&d.truncate().dot(fwd)) && (-0.9..=2.1).contains(&d.z)
        };
        for dz in [0.0, 0.6, -0.6] {
            let probe = ahead + Vec3::Z * dz;
            let feet = self
                .holds_near(probe, 1.0)
                .filter(|h| facing(h))
                .map(|h| (h.nearest(probe), *h))
                .filter(|(q, _)| in_box(*q) && q.distance(probe) < 0.75)
                .min_by(|a, b| a.0.distance(probe).total_cmp(&b.0.distance(probe)));
            let Some((fq, fh)) = feet else { continue };
            let up = fq + Vec3::Z * 1.2;
            let hands = self
                .holds_near(up, 1.0)
                .filter(|h| h.wall.dot(fh.wall) > 0.8)
                .map(|h| (h.nearest(up), *h))
                .filter(|(q, _)| q.distance(up) < 0.3)
                .min_by(|a, b| a.0.distance(up).total_cmp(&b.0.distance(up)));
            if let Some((hq, hh)) = hands {
                return Some((hh, hq));
            }
        }
        None
    }

    /// Hanging from `point` on a wall facing `n`: against the wall when
    /// there is wall below the hands for the feet, else free.
    pub fn hang_stance(&self, point: Vec3, n: Vec2) -> Stance {
        let o = point + (n * 0.6).extend(0.0) - Vec3::Z * 0.9;
        if self.sweep_wall(o, -n * 1.0, 0.0).is_some() {
            Stance::HangWall
        } else {
            Stance::HangFree
        }
    }

    /// A hold to grab while falling at `pos` facing `fwd`: a wall edge
    /// facing us within 0.7 m ahead, 1.3-2.2 m above the feet.
    pub fn catch_hold(&self, pos: Vec3, fwd: Vec2) -> Option<Hold> {
        self.holds_near(pos, 1.5)
            .filter(|h| h.wall.dot(fwd) < -0.5)
            .filter_map(|h| {
                let q = h.nearest(pos);
                let d = (q - pos).truncate();
                let dz = q.z - pos.z;
                (d.length() < 0.7 && d.dot(fwd) > -0.1 && (1.3..=2.2).contains(&dz)).then_some((d.length(), *h))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
    }

    /// The next hold from hands at `hand` on a wall facing `n`, moving
    /// `dir`: an edge on the same wall plane, 0.5-2 m away that way.
    pub fn next_hold(&self, hand: Vec3, n: Vec2, dir: Dir) -> Option<(Hold, Vec3)> {
        let right = right_of(n);
        let probe = match dir {
            Dir::Up => hand + Vec3::Z * 1.2,
            Dir::Down => hand - Vec3::Z * 1.2,
            Dir::Left => hand - (right * 1.2).extend(0.0),
            Dir::Right => hand + (right * 1.2).extend(0.0),
        };
        self.holds_near(probe, 2.0)
            .filter(|h| h.wall.dot(n) > 0.8)
            .filter_map(|h| {
                let q = h.nearest(probe);
                let d = q - hand;
                let (lat, up, out) = (d.truncate().dot(right), d.z, d.truncate().dot(n));
                let ok = out.abs() < 0.35
                    && match dir {
                        Dir::Up => (0.5..=2.0).contains(&up) && lat.abs() < 0.5,
                        Dir::Down => (-2.0..=-0.5).contains(&up) && lat.abs() < 0.5,
                        Dir::Left => (-2.0..=-0.5).contains(&lat) && up.abs() < 0.5,
                        Dir::Right => (0.5..=2.0).contains(&lat) && up.abs() < 0.5,
                    };
                ok.then_some((q.distance(probe), *h, q))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| (x.1, x.2))
    }

    /// A shimmy step from hands at `hand` on `hold` towards the
    /// character's right (`sign` 1) or left (-1).
    pub fn shimmy(&self, hold: &Hold, hand: Vec3, sign: f32) -> Option<Shimmy> {
        shimmy_on(hold, hand, sign, &self.holds_near(hand, SHIMMY_STEP + 1.0).copied().collect::<Vec<_>>())
    }

    /// Debug: standing spots from which a hold can be climbed straight
    /// onto, nearest `near` first (spot, facing).
    pub fn mount_spots(&self, near: Vec3, count: usize) -> Vec<(Vec3, Vec2)> {
        let mut out: Vec<(f32, Vec3, Vec2)> = self
            .holds
            .iter()
            .filter_map(|h| {
                let q = (h.a + h.b) / 2.0;
                let stand = q + (h.wall * 0.95).extend(0.0);
                let z = self.floor_below(stand.with_z(q.z + 0.7))?;
                let feet = stand.with_z(z);
                self.climb_entry(feet, -h.wall).map(|_| (feet.distance(near), feet, -h.wall))
            })
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().take(count).map(|x| (x.1, x.2)).collect()
    }
}

/// [`Ground::shimmy`] over a given set of nearby holds.
pub fn shimmy_on(hold: &Hold, hand: Vec3, sign: f32, near: &[Hold]) -> Option<Shimmy> {
    let n = hold.wall;
    let dir = right_of(n) * sign;
    let len = hold.length();
    if len < 1e-3 {
        return None;
    }
    let e = (hold.b - hold.a).truncate() / len;
    // edge parameter per metre the way we move
    let s = if e.dot(dir) >= 0.0 { 1.0 / len } else { -1.0 / len };
    let t = hold.param(hand);
    // keep the hands 0.25 m off the end
    let margin = 0.25 / len;
    let end_t = if s > 0.0 { 1.0 - margin } else { margin };
    let room = (end_t - t) / s;
    if room >= 0.3 {
        let step = room.min(SHIMMY_STEP);
        return Some(Shimmy::Along(*hold, hold.at(t + s * step)));
    }
    let end = if s > 0.0 { hold.b } else { hold.a };
    // an edge continuing this one along the same wall
    let ahead = end + (dir * 0.4).extend(0.0);
    if let Some((h, q)) = near
        .iter()
        .filter(|h| h.wall.dot(n) > 0.95 && !same(h, hold))
        .map(|h| (h, h.nearest(ahead)))
        .filter(|(_, q)| q.distance(ahead) < 0.3)
        .min_by(|a, b| a.1.distance(ahead).total_cmp(&b.1.distance(ahead)))
    {
        return Some(Shimmy::Along(*h, q));
    }
    // a corner: an edge starting near this end on a wall turned 90 degrees
    near.iter()
        .filter(|h| !same(h, hold) && h.wall.dot(n).abs() < 0.3 && h.length() > 0.6)
        .filter_map(|h| {
            let (near_end, far) = if h.a.distance(end) < h.b.distance(end) { (h.a, h.b) } else { (h.b, h.a) };
            let gap = near_end.distance(end);
            (gap < 0.4 && (near_end.z - end.z).abs() < 0.3).then(|| {
                let outer = h.wall.dot(dir) > 0.0;
                let into = (far - near_end).normalize_or_zero();
                (gap, Shimmy::Corner(*h, near_end + into * 0.35, outer))
            })
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|x| x.1)
}

fn same(a: &Hold, b: &Hold) -> bool {
    a.a.distance(b.a) < 1e-4 && a.b.distance(b.b) < 1e-4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entity_faces_the_wall_with_its_hands_on_the_hold() {
        for stance in [Stance::Climb, Stance::HangWall, Stance::HangFree] {
            for n in [Vec2::X, -Vec2::X, Vec2::Y, Vec2::new(0.6, -0.8)] {
                let yaw = yaw_into(n);
                let facing = Vec2::new(-yaw.sin(), yaw.cos());
                assert!(facing.dot(-n) > 0.999, "{n:?}");
                let hold = Vec3::new(3.0, 4.0, 5.0);
                let e = entity_for(hold, n, stance);
                let (f, u) = stance.hands();
                let hands = e + (facing * f).extend(0.0) + Vec3::Z * u;
                assert!(hands.distance(hold) < 1e-5);
                assert!(hands_of(e, n, stance).distance(hold) < 1e-5);
            }
        }
    }

    #[test]
    fn the_right_hand_side_matches_the_facing() {
        // facing +Y (wall normal -Y) the right is +X
        assert!(right_of(-Vec2::Y).distance(Vec2::X) < 1e-6);
        // facing -X (normal +X) the right is +Y
        assert!(right_of(Vec2::X).distance(Vec2::Y) < 1e-6);
    }

    #[test]
    fn walling_tolerance_follows_the_retail_speed_curve() {
        assert!((walling_tolerance(0.0) - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert!((walling_tolerance(0.5) - 0.5).abs() < 1e-6);
        assert!((walling_tolerance(1.0) - 0.5).abs() < 1e-6);
        assert!((walling_tolerance(6.0) - (1.0 - 50f32.to_radians().cos())).abs() < 1e-6);
        assert!((walling_tolerance(10.0) - walling_tolerance(6.0)).abs() < 1e-6);
        assert!(walling_tolerance(3.0) < walling_tolerance(2.0));
    }

    #[test]
    fn turns_take_the_short_way_round() {
        assert!((turn(3.0, -3.0) - (std::f32::consts::TAU - 6.0)).abs() < 1e-5);
        assert!((turn(0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((turn(-3.0, 3.0) + (std::f32::consts::TAU - 6.0)).abs() < 1e-5);
    }

    // a wall along x facing -y (we face +y), its top edge 4 m up from x 0
    // to 5, and a wall facing +x from (5, 0) to (5, 5) (outer corner at x 5)
    fn south_wall() -> Hold {
        Hold { a: Vec3::new(0.0, 0.0, 4.0), b: Vec3::new(5.0, 0.0, 4.0), wall: -Vec2::Y, top: true }
    }

    #[test]
    fn shimmy_moves_along_the_edge_and_stops_short_of_its_end() {
        let h = south_wall();
        match shimmy_on(&h, Vec3::new(2.0, 0.0, 4.0), 1.0, &[h]) {
            Some(Shimmy::Along(_, p)) => assert!(p.distance(Vec3::new(3.0, 0.0, 4.0)) < 1e-4, "{p:?}"),
            other => panic!("{other:?}"),
        }
        // near the end: a shorter step
        match shimmy_on(&h, Vec3::new(4.2, 0.0, 4.0), 1.0, &[h]) {
            Some(Shimmy::Along(_, p)) => assert!((p.x - 4.75).abs() < 1e-4, "{p:?}"),
            other => panic!("{other:?}"),
        }
        // at the end with nothing beyond
        assert!(shimmy_on(&h, Vec3::new(4.75, 0.0, 4.0), 1.0, &[h]).is_none());
        // left goes the other way
        match shimmy_on(&h, Vec3::new(2.0, 0.0, 4.0), -1.0, &[h]) {
            Some(Shimmy::Along(_, p)) => assert!((p.x - 1.0).abs() < 1e-4),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn shimmy_turns_outer_and_inner_corners() {
        let h = south_wall();
        let east = Hold { a: Vec3::new(5.0, 0.0, 4.0), b: Vec3::new(5.0, 5.0, 4.0), wall: Vec2::X, top: true };
        match shimmy_on(&h, Vec3::new(4.75, 0.0, 4.0), 1.0, &[h, east]) {
            Some(Shimmy::Corner(c, p, outer)) => {
                assert!(outer);
                assert_eq!(c.wall, Vec2::X);
                assert!(p.distance(Vec3::new(5.0, 0.35, 4.0)) < 1e-4, "{p:?}");
                // outer corner to the right turns left (+90)
                assert!((turn(yaw_into(h.wall), yaw_into(c.wall)) - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
            }
            other => panic!("{other:?}"),
        }
        // a wall coming towards us at x 5 (facing -x): inner corner
        let inner = Hold { a: Vec3::new(5.0, 0.0, 4.0), b: Vec3::new(5.0, -5.0, 4.0), wall: -Vec2::X, top: true };
        match shimmy_on(&h, Vec3::new(4.75, 0.0, 4.0), 1.0, &[h, inner]) {
            Some(Shimmy::Corner(c, _, outer)) => {
                assert!(!outer);
                // inner corner to the right turns right (-90)
                assert!((turn(yaw_into(h.wall), yaw_into(c.wall)) + std::f32::consts::FRAC_PI_2).abs() < 1e-4);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn shimmy_continues_onto_a_collinear_edge() {
        let h = south_wall();
        let next = Hold { a: Vec3::new(5.1, 0.0, 4.0), b: Vec3::new(8.0, 0.0, 4.0), wall: -Vec2::Y, top: true };
        match shimmy_on(&h, Vec3::new(4.75, 0.0, 4.0), 1.0, &[h, next]) {
            Some(Shimmy::Along(c, p)) => {
                assert!(same(&c, &next));
                assert!(p.x > 5.1);
            }
            other => panic!("{other:?}"),
        }
    }
}
