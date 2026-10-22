//! Player locomotion from the generic `xx_` clip set: a speed-blended cycle
//! (idle, walk, run, sprint) plus one-shot clips for stopping and turning
//! in place, joined by short cross-fades.
//!
//! The one-shots carry their own root motion: `*_to_h_waitturn_*` turns
//! the root 90/180 degrees over its middle 0.1 s but keeps its pose in the
//! starting frame (the Reference bone turns instead), so the heading changes
//! when it ends; `runstop`/`sprintstop` brake along +Y. Their `_tr_h_wait_*` follow-ups settle into the high
//! profile wait pose in place.

use acb_anim::pose::{blend, Clip, Skeleton};
use acb_anim::BuiltinTracks;
use acb_world::Archive;
use glam::{Mat4, Quat, Vec2, Vec3};

pub use crate::Pose;

/// Cross-fade length when switching between the cycle and a one-shot.
const FADE: f32 = 0.15;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gait {
    Walk,
    Run,
    Sprint,
}

impl Gait {
    fn speed(self) -> f32 {
        match self {
            Gait::Walk => 1.0,
            Gait::Run => 2.0,
            Gait::Sprint => 3.0,
        }
    }
}

#[derive(Clone, Copy)]
enum Id {
    Idle,
    Walk,
    Run,
    Sprint,
    /// [footl, footr] then settle
    RunStop(usize),
    RunStopSettle(usize),
    SprintStop(usize),
    /// left 90, right 90, left 180, right 180
    Turn(usize),
    TurnSettle(usize),
    /// walk, run: into the fall pose (held on its last frame)
    Fall(usize),
    /// from the wait into walk, jog (ending on the right foot)
    Start(usize),
    /// foot: 180 turn at a run (no root yaw: the heading flips when it
    /// ends) and the jog out of it
    RunTurn(usize),
    RunTurnOut(usize),
    /// jump kind (see [`JUMPS`]), foot: run into the air / air onto a surface
    Takeoff(usize, usize),
    JumpLand(usize, usize),
    /// soft/hard/damage x (to wait, to jog)
    Land(usize),
    /// Balancing on a beam (retail `HumanNarrowObjectBeam`): the crouched
    /// wait and walk/jog/sprint cycles, setting off (walk, jog), the jog
    /// stop and its settle per foot, the landing onto it and the 180 turn
    /// on the spot (footl, footr) with its settle. The wait and the starts
    /// come per foot (the landing and the right-foot stop end on the right).
    BeamIdle(usize),
    BeamWalk,
    BeamRun,
    BeamSprint,
    /// foot, jog
    BeamStart(usize, usize),
    BeamStop(usize),
    BeamStopSettle(usize),
    BeamLand,
    BeamTurn(usize),
    BeamTurnSettle(usize),
}

/// The clips of retail's beam state (`xx_l_beam_*` / `xx_h_beam_*`).
pub struct BeamClips {
    pub idle: [Clip; 2],
    pub walk: Clip,
    pub run: Clip,
    pub sprint: Clip,
    pub start: [[Clip; 2]; 2],
    pub stop: [Clip; 2],
    pub stop_settle: [Clip; 2],
    pub land: Clip,
    pub turn: [Clip; 2],
    pub turn_settle: [Clip; 2],
}

pub struct Clips {
    idle: Clip,
    walk: Clip,
    run: Clip,
    sprint: Clip,
    run_stop: [Clip; 2],
    run_stop_settle: [Clip; 2],
    sprint_stop: [Clip; 2],
    turn: [Clip; 4],
    turn_settle: [Clip; 4],
    fall: [Clip; 2],
    /// soft, hard, damage (still / input held), damage roll, fatal
    land: [Clip; 8],
    start: [Clip; 2],
    run_turn: [Clip; 2],
    run_turn_out: [Clip; 2],
    takeoff: Vec<[Clip; 2]>,
    jump_land: Vec<[Clip; 2]>,
    /// None when the archive lacks them: beams use the ground set.
    beam: Option<BeamClips>,
}

/// Jump kinds: direction and distance class of `xx_h_run_{dir}_{dist}_foot*_to_air`
/// (takeoff) / `xx_h_air_{dir}_{dist}_foot*_to_freestep[_down]` (second half
/// of the flight, onto the feet). Retail times a targeted jump as the two
/// clips back to back (`jump.rs`).
pub const JUMPS: [(&str, &str); 8] = [
    ("up", "050cm"),
    ("up", "300cm"),
    ("front", "050cm"),
    ("front", "300cm"),
    ("front", "550cm"),
    ("down", "050cm"),
    ("down", "300cm"),
    ("down", "550cm"),
];

impl Clips {
    pub fn load(archive: &Archive, builtins: Option<&BuiltinTracks>) -> anyhow::Result<Clips> {
        let uids = archive.uids_of("Animation");
        let load = |name: &str| {
            uids.iter()
                .copied()
                .find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(name)))
                .and_then(|u| Clip::load_with(archive, u, builtins))
                .ok_or_else(|| anyhow::anyhow!("{name}: animation not found"))
        };
        let pair = |f: &dyn Fn(&str) -> String| -> anyhow::Result<[Clip; 2]> { Ok([load(&f("l"))?, load(&f("r"))?]) };
        let turns = [("left", "090"), ("right", "090"), ("left", "180"), ("right", "180")];
        let turn = |i: usize| load(&format!("xx_l_wait_hipm_footl_to_l_waitturn_{}_{}_footl", turns[i].0, turns[i].1));
        // left turns end on the left foot, right turns on the right
        let settle = |i: usize| {
            let end = if turns[i].0 == "left" { "l" } else { "r" };
            load(&format!("xx_l_waitturn_{}_{}_footl_tr_l_wait_hipm_foot{end}", turns[i].0, turns[i].1))
        };
        // a cycle is two half strides: `_footl` ends where `_footr` starts
        let stride = |stem: &str| -> anyhow::Result<Clip> { Ok(load(&format!("{stem}_footl"))?.then(&load(&format!("{stem}_footr"))?)) };
        let beam = (|| -> anyhow::Result<BeamClips> {
            Ok(BeamClips {
                idle: pair(&|f| format!("xx_l_beam_crouchwait_foot{f}"))?,
                walk: stride("xx_l_beam_crouchwalk")?,
                run: stride("xx_h_beam_crouchjog")?,
                sprint: stride("xx_h_beam_crouchsprint")?,
                start: [
                    [load("xx_l_beam_crouchwait_footl_tr_crouchwalk_footr")?, load("xx_h_beam_crouchwait_footl_tr_crouchjog_footr")?],
                    [load("xx_l_beam_crouchwait_footr_tr_crouchwalk_footl")?, load("xx_h_beam_crouchwait_footr_tr_crouchjog_footl")?],
                ],
                stop: pair(&|f| format!("xx_h_beam_crouchjog_stop_foot{f}"))?,
                // (sic: retail's right-foot settle is spelt "couchwait")
                stop_settle: [
                    load("xx_h_beam_crouchjog_stop_footl_tr_crouchwait_footl")?,
                    load("xx_h_beam_crouchjog_stop_footr_tr_couchwait_footr")?,
                ],
                land: load("xx_h_beam_landing_straight_soft_tr_crouchwait_footr_a")?,
                turn: pair(&|f| format!("xx_l_beam_crouchwait_foot{f}_turn180"))?,
                turn_settle: [load("xx_l_beam_crouchwait_footl_turn180_tr_footr")?, load("xx_l_beam_crouchwait_footr_turn180_tr_footl")?],
            })
        })();
        if let Err(e) = &beam {
            if crate::trace() {
                eprintln!("beam clips: {e}");
            }
        }
        Ok(Clips {
            beam: beam.ok(),
            idle: load("xx_l_wait_hipm_footl")?,
            walk: stride("xx_l_walk_hipm")?,
            run: stride("xx_h_run_hipm")?,
            sprint: stride("xx_h_sprint_hipm")?,
            run_stop: pair(&|f| format!("xx_h_runstop_foot{f}"))?,
            run_stop_settle: [
                load("xx_h_runstop_footl_tr_h_wait_hipm_footr")?,
                load("xx_h_runstop_footr_tr_h_wait_hipm_footl")?,
            ],
            sprint_stop: pair(&|f| format!("xx_h_sprintstop_foot{f}"))?,
            turn: [turn(0)?, turn(1)?, turn(2)?, turn(3)?],
            turn_settle: [settle(0)?, settle(1)?, settle(2)?, settle(3)?],
            start: [load("xx_l_wait_footl_tr_l_walk_footr")?, load("xx_l_wait_footl_tr_h_jog_footr")?],
            run_turn: [load("xx_h_runturn180_footl")?, load("xx_h_runturn180_footr")?],
            run_turn_out: [load("xx_h_runturn180_footl_tr_jog_hipm_footl")?, load("xx_h_runturn180_footr_tr_jog_hipm_footr")?],
            takeoff: JUMPS
                .iter()
                .map(|(d, n)| Ok([load(&format!("xx_h_run_{d}_{n}_footl_to_air"))?, load(&format!("xx_h_run_{d}_{n}_footr_to_air"))?]))
                .collect::<anyhow::Result<_>>()?,
            jump_land: JUMPS
                .iter()
                .map(|(d, n)| {
                    let to = if *d == "down" { "freestep_down" } else { "freestep" };
                    Ok([load(&format!("xx_h_air_{d}_{n}_footl_to_{to}"))?, load(&format!("xx_h_air_{d}_{n}_footr_to_{to}"))?])
                })
                .collect::<anyhow::Result<_>>()?,
            fall: [load("xx_l_walklowfall_footl_tr_fall")?, load("xx_h_runlowfall_footl_tr_fall")?],
            land: [
                load("xx_h_landing_forward_soft_footl_tr_h_wait_footl_a")?,
                load("xx_h_landing_forward_soft_footl_tr_h_jog_footr_a")?,
                load("xx_h_landing_forward_hard_footl_tr_h_wait_footl_a")?,
                load("xx_h_landing_forward_hard_footl_tr_h_jog_footr_a")?,
                load("xx_h_landing_damage_footl_tr_h_wait_footr")?,
                load("xx_h_landing_damage_footl_tr_h_jog_footl")?,
                load("xx_h_landing_damage_footl_roll")?,
                load("xx_h_landing_death_back")?,
            ],
        })
    }

    /// The beam set, if loaded.
    pub fn beam(&self) -> Option<&BeamClips> {
        self.beam.as_ref()
    }

    pub fn idle(&self) -> &Clip {
        &self.idle
    }

    /// Takeoff and air clip of jump `kind` ([`JUMPS`]) off `foot` (0 left).
    pub fn jump(&self, kind: usize, foot: usize) -> (&Clip, &Clip) {
        (&self.takeoff[kind][foot], &self.jump_land[kind][foot])
    }

    fn get(&self, id: Id) -> &Clip {
        match id {
            Id::Idle => &self.idle,
            Id::Walk => &self.walk,
            Id::Run => &self.run,
            Id::Sprint => &self.sprint,
            Id::RunStop(f) => &self.run_stop[f],
            Id::RunStopSettle(f) => &self.run_stop_settle[f],
            Id::SprintStop(f) => &self.sprint_stop[f],
            Id::Turn(i) => &self.turn[i],
            Id::TurnSettle(i) => &self.turn_settle[i],
            Id::Fall(i) => &self.fall[i],
            Id::Start(i) => &self.start[i],
            Id::RunTurn(f) => &self.run_turn[f],
            Id::RunTurnOut(f) => &self.run_turn_out[f],
            Id::Takeoff(k, f) => &self.takeoff[k][f],
            Id::JumpLand(k, f) => &self.jump_land[k][f],
            Id::Land(i) => &self.land[i],
            beam => {
                let Some(b) = &self.beam else {
                    // unreachable: beam ids are only used with the set loaded
                    return &self.idle;
                };
                match beam {
                    Id::BeamIdle(f) => &b.idle[f],
                    Id::BeamWalk => &b.walk,
                    Id::BeamRun => &b.run,
                    Id::BeamSprint => &b.sprint,
                    Id::BeamStart(f, j) => &b.start[f][j],
                    Id::BeamStop(f) => &b.stop[f],
                    Id::BeamStopSettle(f) => &b.stop_settle[f],
                    Id::BeamLand => &b.land,
                    Id::BeamTurn(f) => &b.turn[f],
                    Id::BeamTurnSettle(f) => &b.turn_settle[f],
                    _ => &self.idle,
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Cancel {
    /// Stops give way to new input at once.
    OnInput,
    /// Starts give way to releasing the input.
    OnRelease,
    /// Turns and landings play out.
    Never,
}

/// A one-shot sequence of clips played back to back.
#[derive(Clone)]
struct Action {
    queue: Vec<Id>,
    t: f32,
    cancel: Cancel,
    /// The controller keeps turning the facing toward the input.
    steer: bool,
    /// Cycle speed to continue with when it ends (at the best-matching
    /// phase); the second value is unused.
    end: (f32, f32),
}

impl Action {
    fn new(queue: Vec<Id>, cancel: Cancel) -> Self {
        Action { queue, t: 0.0, cancel, steer: false, end: (0.0, 0.0) }
    }
}

#[derive(Clone)]
pub struct Locomotion {
    /// 0 idle, 1 walk, 2 run, 3 sprint; eases toward the input.
    pub speed: f32,
    /// Shared normalized phase of the cycles (full strides: left foot, then
    /// right).
    phase: f32,
    idle_time: f32,
    action: Option<Action>,
    /// Pose faded out from and the fade time left.
    fade: Option<(Pose, f32)>,
    /// Falling: the clip into the fall pose and its time.
    air: Option<(Id, f32)>,
    /// The landing a jump in progress ends with.
    jump_land: Option<Id>,
    /// Standing on a beam (and the beam set is loaded): the cycles,
    /// starts, stops and turns come from [`BeamClips`].
    beam: bool,
    /// Foot of the beam wait (0 left): the one nearest the pose an action
    /// ended in.
    beam_foot: usize,
    last: Pose,
}

/// Result of one update.
pub struct Step {
    /// Displacement in world XY.
    pub delta: Vec2,
    pub pose: Pose,
}

fn wrap_angle(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

fn yaw_of(m: &Mat4) -> f32 {
    let (_, r, _) = m.to_scale_rotation_translation();
    r.to_euler(glam::EulerRot::ZXY).0
}

/// Summed rotation difference of two poses, radians.
fn pose_distance(a: &[(Vec3, Quat)], b: &[(Vec3, Quat)]) -> f32 {
    a.iter().zip(b).map(|((_, p), (_, q))| p.angle_between(*q)).sum()
}

impl Locomotion {
    pub fn new(clips: &Clips, skeleton: &Skeleton) -> Self {
        Locomotion { speed: 0.0, phase: 0.0, idle_time: 0.0, action: None, fade: None, air: None, jump_land: None, beam: false, beam_foot: 0, last: clips.idle.sample(skeleton, 0.0) }
    }

    fn start(&mut self, action: Action) {
        self.fade = Some((self.last.clone(), FADE));
        self.action = Some(action);
    }

    /// Back in control after a scripted clip that ended in `pose`.
    pub fn resume(&mut self, pose: Pose) {
        self.action = None;
        self.air = None;
        self.speed = 0.0;
        self.fade = Some((pose.clone(), FADE));
        self.last = pose;
    }

    /// On a beam or off it (called every grounded tick before
    /// [`Locomotion::update`], and before [`Locomotion::land`]). A change
    /// cross-fades between the ground and beam sets at the same phase and
    /// drops a start or stop in progress.
    pub fn set_beam(&mut self, clips: &Clips, sk: &Skeleton, on: bool) {
        let on = on && clips.beam.is_some();
        if on == self.beam {
            return;
        }
        self.beam = on;
        if self.air.is_some() {
            return;
        }
        if self.action.as_ref().is_some_and(|a| a.cancel != Cancel::Never) {
            let a = self.action.take().unwrap();
            if a.end.0 > 0.0 {
                self.speed = self.speed.max(a.end.0.min(1.0));
            }
        }
        if on {
            self.beam_foot = nearest_foot(clips, sk, &self.last);
        }
        self.fade = Some((self.last.clone(), FADE));
    }

    pub fn on_beam(&self) -> bool {
        self.beam
    }

    /// Cycle clips of the current set: idle, walk, run, sprint.
    fn cycles(&self) -> [Id; 4] {
        if self.beam {
            [Id::BeamIdle(self.beam_foot), Id::BeamWalk, Id::BeamRun, Id::BeamSprint]
        } else {
            [Id::Idle, Id::Walk, Id::Run, Id::Sprint]
        }
    }

    /// The pose shown last frame.
    pub fn last_pose(&self) -> &Pose {
        &self.last
    }

    pub fn airborne(&self) -> bool {
        self.air.is_some()
    }

    /// Left the ground: the caller moves the character ballistically until
    /// [`Locomotion::land`].
    pub fn start_fall(&mut self) {
        if self.air.is_none() {
            self.air = Some((Id::Fall(usize::from(self.speed > 1.5)), 0.0));
            self.action = None;
            self.fade = Some((self.last.clone(), FADE));
        }
    }

    /// Took off on a freerun jump of kind `kind` ([`JUMPS`]); the caller
    /// flies the character until [`Locomotion::land`]. Returns the foot
    /// it takes off from (0 left).
    pub fn start_jump(&mut self, clips: &Clips, sk: &Skeleton, kind: usize) -> usize {
        let f = (0..2)
            .min_by(|&a, &b| {
                let d = |f| pose_distance(&self.last, &clips.get(Id::Takeoff(kind, f)).sample(sk, 0.0));
                d(a).total_cmp(&d(b))
            })
            .unwrap_or(0);
        self.air = Some((Id::Takeoff(kind, f), 0.0));
        self.jump_land = Some(Id::JumpLand(kind, f));
        self.action = None;
        self.fade = Some((self.last.clone(), FADE * 0.5));
        f
    }

    /// Touched down after falling `height` metres; `moving`: input held.
    pub fn land(&mut self, height: f32, moving: bool) {
        if self.air.take().is_none() {
            return;
        }
        if self.beam && !moving {
            // onto the beam crouch (from a jump or a fall alike)
            self.jump_land = None;
            self.speed = 0.0;
            self.idle_time = 0.0;
            self.start(Action::new(vec![Id::BeamLand], Cancel::OnInput));
            return;
        }
        if self.jump_land.take().is_some() {
            // a planned jump played its air clip in flight: on into the
            // cycle (no landing displacement: narrow tops stay under foot)
            self.speed = if moving { self.speed.max(2.0) } else { 0.0 };
            self.fade = Some((self.last.clone(), FADE));
            return;
        }
        // retail landing type by fall height (`jump::landing`): safe ->
        // soft / hard forward landing, small / heavy damage -> the damage
        // landing, or its roll with input held, fatal -> death
        let clip = match crate::jump::landing(height).1 {
            crate::jump::LandingType::Safe if height < crate::jump::HARD_LANDING => usize::from(moving),
            crate::jump::LandingType::Safe => 2 + usize::from(moving),
            crate::jump::LandingType::Fatal => 7,
            _ if moving => 6,
            _ => 4,
        };
        self.speed = if moving && clip != 7 { 2.0 } else { 0.0 };
        let mut a = Action::new(vec![Id::Land(clip)], Cancel::Never);
        a.end = (self.speed, 0.5);
        self.start(a);
    }

    /// The character ran into something: drop toward idle.
    pub fn blocked(&mut self, dt: f32) {
        if self.action.is_none() {
            self.speed = (self.speed - 6.0 * dt).max(0.0);
        }
    }

    /// `dir`: wanted world direction (zero when no input); `yaw` is the
    /// character's facing and is turned by the controller or the clips.
    pub fn update(&mut self, clips: &Clips, sk: &Skeleton, dir: Vec2, gait: Gait, yaw: &mut f32, dt: f32) -> Step {
        let want = (dir != Vec2::ZERO).then(|| (-dir.x).atan2(dir.y));
        if let Some((id, t)) = &mut self.air {
            *t += dt;
            // takeoff, then the air clip of the same jump
            if let (Id::Takeoff(..), Some(land)) = (*id, self.jump_land) {
                let len = clips.get(*id).length;
                if *t > len {
                    *id = land;
                    *t -= len;
                }
            }
            let c = clips.get(*id);
            let pose = c.sample(sk, t.min(c.length));
            let pose = self.faded(pose, dt);
            return Step { delta: Vec2::ZERO, pose };
        }
        let cancel = self.action.as_ref().map(|a| a.cancel);
        if (cancel == Some(Cancel::OnInput) && want.is_some()) || (cancel == Some(Cancel::OnRelease) && want.is_none()) {
            self.action = None;
            self.fade = Some((self.last.clone(), FADE));
        }
        if self.action.is_none() {
            self.maybe_start_action(clips, sk, want, gait, *yaw);
        }
        if let (Some(a), Some(w)) = (&self.action, want) {
            if a.steer {
                let turn = 7.0 * dt;
                *yaw = wrap_angle(*yaw + wrap_angle(w - *yaw).clamp(-turn, turn));
            }
        }
        let (delta, pose) = match self.action.is_some() {
            true => self.play_action(clips, sk, yaw, dt),
            false => self.play_cycle(clips, sk, want, gait, yaw, dt),
        };
        let pose = self.faded(pose, dt);
        Step { delta, pose }
    }

    /// Applies the running cross-fade and remembers the result.
    fn faded(&mut self, pose: Pose, dt: f32) -> Pose {
        let pose = match &mut self.fade {
            Some((from, left)) => {
                *left -= dt;
                let mut out = from.clone();
                blend(&mut out, &pose, (1.0 - *left / FADE).clamp(0.0, 1.0));
                if *left <= 0.0 {
                    self.fade = None;
                }
                out
            }
            None => pose,
        };
        self.last = pose.clone();
        pose
    }

    fn maybe_start_action(&mut self, clips: &Clips, sk: &Skeleton, want: Option<f32>, gait: Gait, yaw: f32) {
        match want {
            // released while running: brake on the foot the pose is on
            None if self.speed >= 1.5 => {
                let sprint = self.speed >= 2.5 && !self.beam;
                let beam = self.beam;
                let first = |f| match (beam, sprint) {
                    (true, _) => Id::BeamStop(f),
                    (false, true) => Id::SprintStop(f),
                    (false, false) => Id::RunStop(f),
                };
                let f = (0..2)
                    .min_by(|&a, &b| {
                        let d = |f| pose_distance(&self.last, &clips.get(first(f)).sample(sk, 0.0));
                        d(a).total_cmp(&d(b))
                    })
                    .unwrap();
                let queue = match (beam, sprint) {
                    (true, _) => vec![first(f), Id::BeamStopSettle(f)],
                    (false, true) => vec![first(f)],
                    (false, false) => vec![first(f), Id::RunStopSettle(f)],
                };
                self.speed = 0.0;
                self.idle_time = 0.0;
                self.start(Action::new(queue, Cancel::OnInput));
            }
            // running and asked to go back the way we came: run turn
            // (no running turn on a beam: the move steers round)
            Some(w) if self.speed >= 1.5 && !self.beam && wrap_angle(w - yaw).abs() > 135f32.to_radians() => {
                let f = (0..2)
                    .min_by(|&a, &b| {
                        let d = |f| pose_distance(&self.last, &clips.get(Id::RunTurn(f)).sample(sk, 0.0));
                        d(a).total_cmp(&d(b))
                    })
                    .unwrap();
                let mut a = Action::new(vec![Id::RunTurn(f), Id::RunTurnOut(f)], Cancel::Never);
                a.end = (1.6, 0.0);
                self.start(a);
            }
            // standing and asked to face well away: turn on the spot
            Some(w) if self.speed < 0.1 => {
                let d = wrap_angle(w - yaw);
                if self.beam && d.abs() > 135f32.to_radians() {
                    // about turn in the crouch, on the foot the pose is on
                    let f = (0..2)
                        .min_by(|&a, &b| {
                            let d = |f| pose_distance(&self.last, &clips.get(Id::BeamTurn(f)).sample(sk, 0.0));
                            d(a).total_cmp(&d(b))
                        })
                        .unwrap();
                    self.start(Action::new(vec![Id::BeamTurn(f), Id::BeamTurnSettle(f)], Cancel::Never));
                } else if !self.beam && d.abs() > 60f32.to_radians() {
                    let i = match (d > 0.0, d.abs() > 135f32.to_radians()) {
                        (true, false) => 0,
                        (false, false) => 1,
                        (true, true) => 2,
                        (false, true) => 3,
                    };
                    self.start(Action::new(vec![Id::Turn(i), Id::TurnSettle(i)], Cancel::Never));
                } else {
                    // set off: into the walk, or a jog that blends into the run
                    let jog = gait != Gait::Walk;
                    let id = if self.beam { Id::BeamStart(self.beam_foot, usize::from(jog)) } else { Id::Start(usize::from(jog)) };
                    let mut a = Action::new(vec![id], Cancel::OnRelease);
                    a.steer = true;
                    a.end = (if jog { 1.6 } else { 1.0 }, 0.5);
                    self.start(a);
                }
            }
            _ => {}
        }
    }

    /// Advances the one-shot queue; root motion turns `yaw`. Returns the
    /// world displacement and the pose.
    fn play_action(&mut self, clips: &Clips, sk: &Skeleton, yaw: &mut f32, dt: f32) -> (Vec2, Pose) {
        let a = self.action.as_mut().unwrap();
        let mut left = dt;
        let mut moved = Vec2::ZERO;
        loop {
            let id = a.queue[0];
            let clip = clips.get(id);
            let t1 = (a.t + left).min(clip.length);
            let d = clip.root_delta(a.t, t1);
            moved += (Quat::from_rotation_z(*yaw) * d.w_axis.truncate()).truncate();
            // a `_to_` turn keeps its pose in the frame it started in: the
            // entity takes the new heading only once it ends
            if !matches!(id, Id::Turn(_)) {
                *yaw = wrap_angle(*yaw + yaw_of(&d));
            }
            left -= t1 - a.t;
            a.t = t1;
            if a.t < clip.length || a.queue.len() == 1 {
                break;
            }
            if matches!(id, Id::Turn(_)) {
                *yaw = wrap_angle(*yaw + yaw_of(&clip.root_motion(clip.length)));
            }
            if matches!(id, Id::RunTurn(_)) {
                *yaw = wrap_angle(*yaw + std::f32::consts::PI);
            }
            a.queue.remove(0);
            a.t = 0.0;
        }
        let clip = clips.get(a.queue[0]);
        let pose = clip.sample(sk, a.t);
        if a.t >= clip.length {
            self.speed = a.end.0;
            if self.beam {
                self.beam_foot = nearest_foot(clips, sk, &pose);
            }
            self.phase = if self.speed > 0.0 { matching_phase(clips, sk, self.cycles(), self.speed, &pose) } else { 0.0 };
            self.action = None;
            self.fade = Some((pose.clone(), FADE));
        }
        (moved, pose)
    }

    fn play_cycle(&mut self, clips: &Clips, sk: &Skeleton, want: Option<f32>, gait: Gait, yaw: &mut f32, dt: f32) -> (Vec2, Pose) {
        let target = if want.is_some() { gait.speed() } else { 0.0 };
        let ease = 2.5 * dt;
        self.speed += (target - self.speed).clamp(-ease, ease);
        if let Some(w) = want {
            let turn = 7.0 * dt;
            *yaw = wrap_angle(*yaw + wrap_angle(w - *yaw).clamp(-turn, turn));
        }
        let s = self.speed;
        let ids = self.cycles();
        let cycles = [(ids[1], w_of(s, 1.0)), (ids[2], w_of(s, 2.0)), (ids[3], w_of(s, 3.0))];
        let moving: f32 = cycles.iter().map(|c| c.1).sum();
        let p0 = self.phase;
        if moving > 1e-3 {
            let rate: f32 = cycles.iter().map(|&(id, w)| w / clips.get(id).length).sum::<f32>() / moving;
            self.phase = (self.phase + dt * rate).fract();
        }
        self.idle_time = (self.idle_time + dt) % clips.get(ids[0]).length.max(1e-3);
        let mut step = Vec3::ZERO;
        for (id, w) in cycles.into_iter().filter(|c| c.1 > 0.0) {
            let c = clips.get(id);
            step += c.root_step(p0 * c.length, self.phase * c.length) * w;
        }
        let pose = cycle_pose(clips, sk, ids, s, self.phase, self.idle_time);
        ((Quat::from_rotation_z(*yaw) * step).truncate(), pose)
    }
}

/// The cycle blend at `speed` and normalized `phase`: neighbouring cycles
/// by weight (idle|walk|run, walk|run|sprint).
fn cycle_pose(clips: &Clips, sk: &Skeleton, ids: [Id; 4], speed: f32, phase: f32, idle_time: f32) -> Pose {
    let (lo, hi, w) = match speed {
        s if s <= 1.0 => (0, 1, s),
        s if s <= 2.0 => (1, 2, s - 1.0),
        s => (2, 3, s - 2.0),
    };
    let sample = |i: usize| {
        let c = clips.get(ids[i]);
        c.sample(sk, if i == 0 { idle_time } else { phase * c.length })
    };
    let mut pose = sample(lo);
    if w > 0.0 {
        blend(&mut pose, &sample(hi), w);
    }
    pose
}

/// Cycle phase whose pose at `speed` is nearest `pose` (64 samples over the
/// full two-foot stride).
fn matching_phase(clips: &Clips, sk: &Skeleton, ids: [Id; 4], speed: f32, pose: &[(Vec3, Quat)]) -> f32 {
    (0..64)
        .map(|i| i as f32 / 64.0)
        .min_by(|&a, &b| {
            let d = |ph| pose_distance(pose, &cycle_pose(clips, sk, ids, speed, ph, 0.0));
            d(a).total_cmp(&d(b))
        })
        .unwrap_or(0.0)
}


/// The beam wait (left or right foot) nearest `pose` (0 without the set).
fn nearest_foot(clips: &Clips, sk: &Skeleton, pose: &[(Vec3, Quat)]) -> usize {
    let Some(b) = &clips.beam else { return 0 };
    let d = |f: usize| pose_distance(pose, &b.idle[f].sample(sk, 0.0));
    usize::from(d(1) < d(0))
}

/// Tent weight of the cycle centred on speed `at`.
fn w_of(s: f32, at: f32) -> f32 {
    (1.0 - (s - at).abs()).max(0.0)
}
