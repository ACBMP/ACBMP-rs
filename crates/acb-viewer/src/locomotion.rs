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

pub type Pose = Vec<(Vec3, Quat)>;

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
    /// soft/hard/damage x (to wait, to jog)
    Land(usize),
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
    land: [Clip; 6],
}

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
        let turn = |i: usize| load(&format!("xx_h_wait_hipm_footl_to_h_waitturn_{}_{}_footl", turns[i].0, turns[i].1));
        // left turns end on the left foot, right turns on the right
        let settle = |i: usize| {
            let end = if turns[i].0 == "left" { "l" } else { "r" };
            load(&format!("xx_h_waitturn_{}_{}_footl_tr_h_wait_hipm_foot{end}", turns[i].0, turns[i].1))
        };
        Ok(Clips {
            idle: load("xx_l_wait_hipm_footl")?,
            walk: load("xx_l_walk_hipm_footl")?,
            run: load("xx_h_run_hipm_footl")?,
            sprint: load("xx_h_sprint_hipm_footl")?,
            run_stop: pair(&|f| format!("xx_h_runstop_foot{f}"))?,
            run_stop_settle: [
                load("xx_h_runstop_footl_tr_h_wait_hipm_footr")?,
                load("xx_h_runstop_footr_tr_h_wait_hipm_footl")?,
            ],
            sprint_stop: pair(&|f| format!("xx_h_sprintstop_foot{f}"))?,
            turn: [turn(0)?, turn(1)?, turn(2)?, turn(3)?],
            turn_settle: [settle(0)?, settle(1)?, settle(2)?, settle(3)?],
            fall: [load("xx_l_walklowfall_footl_tr_fall")?, load("xx_h_runlowfall_footl_tr_fall")?],
            land: [
                load("xx_h_landing_forward_soft_footl_tr_h_wait_footl_a")?,
                load("xx_h_landing_forward_soft_footl_tr_h_jog_footr_a")?,
                load("xx_h_landing_forward_hard_footl_tr_h_wait_footl_a")?,
                load("xx_h_landing_forward_hard_footl_tr_h_jog_footr_a")?,
                load("xx_h_landing_damage_footl_tr_h_wait_footr")?,
                load("xx_h_landing_damage_footl_tr_h_jog_footl")?,
            ],
        })
    }

    pub fn idle(&self) -> &Clip {
        &self.idle
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
            Id::Land(i) => &self.land[i],
        }
    }
}

/// A one-shot sequence of clips played back to back.
struct Action {
    queue: Vec<Id>,
    t: f32,
    /// Stops give way to new input at once; turns finish first.
    interruptible: bool,
}

pub struct Locomotion {
    /// 0 idle, 1 walk, 2 run, 3 sprint; eases toward the input.
    pub speed: f32,
    /// Shared normalized phase of the (left-foot-first) cycles.
    phase: f32,
    idle_time: f32,
    action: Option<Action>,
    /// Pose faded out from and the fade time left.
    fade: Option<(Pose, f32)>,
    /// Falling: the clip into the fall pose and its time.
    air: Option<(Id, f32)>,
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
        Locomotion { speed: 0.0, phase: 0.0, idle_time: 0.0, action: None, fade: None, air: None, last: clips.idle.sample(skeleton, 0.0) }
    }

    fn start(&mut self, action: Action) {
        self.fade = Some((self.last.clone(), FADE));
        self.action = Some(action);
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

    /// Touched down after falling `height` metres; `moving`: input held.
    pub fn land(&mut self, height: f32, moving: bool) {
        if self.air.take().is_none() {
            return;
        }
        let severity = if height < 3.0 {
            0
        } else if height < 6.0 {
            1
        } else {
            2
        };
        self.speed = if moving { 2.0 } else { 0.0 };
        self.phase = 0.0;
        self.start(Action { queue: vec![Id::Land(severity * 2 + usize::from(moving))], t: 0.0, interruptible: false });
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
            let c = clips.get(*id);
            let pose = c.sample(sk, t.min(c.length));
            let pose = self.faded(pose, dt);
            return Step { delta: Vec2::ZERO, pose };
        }
        if self.action.as_ref().is_some_and(|a| a.interruptible) && want.is_some() {
            self.action = None;
            self.fade = Some((self.last.clone(), FADE));
        }
        if self.action.is_none() {
            self.maybe_start_action(clips, sk, want, *yaw);
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

    fn maybe_start_action(&mut self, clips: &Clips, sk: &Skeleton, want: Option<f32>, yaw: f32) {
        match want {
            // released while running: brake on the foot the pose is on
            None if self.speed >= 1.5 => {
                let sprint = self.speed >= 2.5;
                let first = |f| if sprint { Id::SprintStop(f) } else { Id::RunStop(f) };
                let f = (0..2)
                    .min_by(|&a, &b| {
                        let d = |f| pose_distance(&self.last, &clips.get(first(f)).sample(sk, 0.0));
                        d(a).total_cmp(&d(b))
                    })
                    .unwrap();
                let queue = if sprint { vec![first(f)] } else { vec![first(f), Id::RunStopSettle(f)] };
                self.speed = 0.0;
                self.idle_time = 0.0;
                self.start(Action { queue, t: 0.0, interruptible: true });
            }
            // standing and asked to face well away: turn on the spot
            Some(w) if self.speed < 0.1 => {
                let d = wrap_angle(w - yaw);
                if d.abs() > 60f32.to_radians() {
                    let i = match (d > 0.0, d.abs() > 135f32.to_radians()) {
                        (true, false) => 0,
                        (false, false) => 1,
                        (true, true) => 2,
                        (false, true) => 3,
                    };
                    self.start(Action { queue: vec![Id::Turn(i), Id::TurnSettle(i)], t: 0.0, interruptible: false });
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
            a.queue.remove(0);
            a.t = 0.0;
        }
        let clip = clips.get(a.queue[0]);
        let pose = clip.sample(sk, a.t);
        if a.t >= clip.length {
            self.action = None;
            self.fade = Some((pose.clone(), FADE));
            self.phase = 0.0;
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
        // neighbouring cycles by weight: (idle|walk|run, walk|run|sprint)
        let s = self.speed;
        let (lo, hi, w) = match s {
            s if s <= 1.0 => (Id::Idle, Id::Walk, s),
            s if s <= 2.0 => (Id::Walk, Id::Run, s - 1.0),
            s => (Id::Run, Id::Sprint, s - 2.0),
        };
        let cycles = [(Id::Walk, w_of(s, 1.0)), (Id::Run, w_of(s, 2.0)), (Id::Sprint, w_of(s, 3.0))];
        let moving: f32 = cycles.iter().map(|c| c.1).sum();
        let p0 = self.phase;
        if moving > 1e-3 {
            let rate: f32 = cycles.iter().map(|&(id, w)| w / clips.get(id).length).sum::<f32>() / moving;
            self.phase = (self.phase + dt * rate).fract();
        }
        self.idle_time = (self.idle_time + dt) % clips.idle.length.max(1e-3);
        let mut step = Vec3::ZERO;
        for (id, w) in cycles.into_iter().filter(|c| c.1 > 0.0) {
            let c = clips.get(id);
            step += c.root_step(p0 * c.length, self.phase * c.length) * w;
        }
        let sample = |id: Id| match id {
            Id::Idle => clips.idle.sample(sk, self.idle_time),
            id => {
                let c = clips.get(id);
                c.sample(sk, self.phase * c.length)
            }
        };
        let mut pose = sample(lo);
        if w > 0.0 {
            blend(&mut pose, &sample(hi), w);
        }
        ((Quat::from_rotation_z(*yaw) * step).truncate(), pose)
    }
}

/// Tent weight of the cycle centred on speed `at`.
fn w_of(s: f32, at: f32) -> f32 {
    (1.0 - (s - at).abs()).max(0.0)
}
