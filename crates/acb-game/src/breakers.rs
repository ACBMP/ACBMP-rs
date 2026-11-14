//! Chase breakers: the map's closing doors, falling platforms and lifts
//! (data: [`acb_world::breakers`]).
//!
//! Doors and platforms are a trigger zone and a scene. The trigger
//! (retail `TriggerComponentSettings::UpdateOverlappedEntities` 0080c990,
//! `OnUpdate` 007d5c00, `OnCondition` 007d5c80) watches who is in its
//! zone; a seed list turns the scene on, and the scene's timeline turns
//! components on and off: a door's blocker comes on (it is off in the map
//! data, so a door stands open), a platform's collision and guidance go
//! off. The scene ends by turning the trigger back on.
//!
//! - a door fires when someone who was in its zone leaves it
//!   (`WatchType` EXIT) and its condition holds for them: not in the Walk
//!   or WaitLow actor state and not in conflict. So running through shuts
//!   it; walking through does not.
//! - a platform fires 1.0 s after someone enters its zone (`OnEnter`), or
//!   0.8 s after one of the trigger's 0.5 s updates finds someone in it
//!   (`OnUpdate`), whichever is first.
//!
//! A lift (`ElevatorComponent`) takes a character on the ground moving
//! inside its detection sphere (`IsInDetectionZone` 011510c2, tested from
//! the HumanGround Move state, 00fceeac) unless it is in use: the rider is
//! put on the lift's start point facing it (`EnterElevatorRaise`
//! 00fc60e6) and plays the ride clips; the lift (`ActivateElevator`
//! 0111f7e0, `UpdateAfter` 01132804) plays its up animation, then its
//! rewind, and only then takes someone again.
//!
//! Ours:
//! - the zones test a point [`ENTITY_HEIGHT`] above the feet. Retail tests
//!   the character entity's position; a lift's sphere (1.2 m around a
//!   point 1.5 m up) does not reach the floor its start point is on, so
//!   that position is not the feet. We take the idle's pelvis height.
//! - triggers watch players only (no crowd, no VIPs).
//! - a trigger stops watching from the tick a seed fires until its scene
//!   turns it back on, and starts over with nobody inside.
//! - the trigger's update phase is the world tick (every 30th tick).
//! - Walk = on the ground slower than [`crate::rules::RUN_SPEED`], WaitLow
//!   = standing; in conflict = in a kill or stun.
//! - a lift wants high profile (run or sprint held): what sends the
//!   HumanGround interface command `ActivateElevator` was not read.
//! - a lift takes a runner from up to [`LIFT_AHEAD`] before its sphere
//!   (before our climb would start on the lift's base).
//! - the ride clips are picked by their root motion (the action to clip
//!   table was not read); always the front entry, the side entries
//!   (`..._positionl` / `r`) are not used.
//! - the lift's landed counterweight (inactive in the map data) never
//!   collides and does no damage (`SendCounterWeightDamage` 01167d20).

use acb_anim::pose::{Clip, Skeleton};
use acb_anim::BuiltinTracks;
use acb_world::breakers::{self as data, Breaker, Condition, Lift, ZoneShape};
use acb_world::Archive;
use glam::{Mat4, Quat, Vec2, Vec3};

use crate::ground::Ground;
use crate::{Pose, TICK_HZ};

/// Height above the feet of the point zones test (ours, see the module
/// doc): the idle's pelvis (Reference bone) height.
pub const ENTITY_HEIGHT: f32 = 0.96;
/// How far before its detection sphere a lift takes a runner heading for
/// it (ours): as far as a wall ahead is seen from. The lift stands on a
/// 0.96 m base under its sphere, and our climb onto a wall starts from
/// where the wall is first seen, so without this the runner would climb
/// the base before reaching the sphere.
pub const LIFT_AHEAD: f32 = crate::climb::WALLING_RAY_DEPTH;
/// Blend from the pose before into the ride (ours, as hiding).
const BLEND: f32 = 0.2;

fn ticks(seconds: f32) -> u32 {
    (seconds * TICK_HZ as f32).round().max(0.0) as u32
}

/// A trigger + scene breaker, ready to tick.
pub struct Scene {
    pub data: Breaker,
    /// World to the zone owner's frame.
    to_zone: Mat4,
    /// (tick into the scene, ground group, on), in time order.
    switches: Vec<(u32, u16, bool)>,
    /// The scene's ground groups and their state in the map data.
    groups: Vec<(u16, bool)>,
    /// Ticks from the scene's start to its end (the trigger is back on).
    length: u32,
    /// Ticks between the trigger's periodic updates.
    every: u32,
}

/// What changes in a trigger + scene breaker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneState {
    /// Players in the zone at the last pass (bit per player).
    inside: u64,
    /// The condition seeds fired since the last entry (`FireOnlyOnce`).
    fired: bool,
    /// Ticks until the scene starts (a seed's delay).
    pending: Option<u32>,
    /// Ticks played of the scene.
    playing: Option<u32>,
    /// The scene has played: its groups are where its last switches left
    /// them, not where the map data has them.
    played: bool,
}

impl SceneState {
    /// Seconds into the scene, while it plays.
    pub fn time(&self) -> Option<f32> {
        self.playing.map(|k| k as f32 / TICK_HZ as f32)
    }
}

/// A player as the triggers see it.
#[derive(Clone, Copy, Debug)]
pub struct Watched {
    /// Feet.
    pub pos: Vec3,
    /// Actor state Walk / WaitLow.
    pub walking: bool,
    pub idle: bool,
    pub conflict: bool,
}

fn holds(c: &Condition, w: &Watched) -> bool {
    match c {
        Condition::All(l) => l.iter().all(|c| holds(c, w)),
        Condition::Any(l) => l.iter().any(|c| holds(c, w)),
        Condition::Not(c) => !holds(c, w),
        Condition::ActorState(data::ACTOR_STATE_WALK) => w.walking,
        Condition::ActorState(data::ACTOR_STATE_WAIT_LOW) => w.idle,
        Condition::InConflict => w.conflict,
        // states and conditions we do not model never hold
        Condition::ActorState(_) | Condition::Other(_) => false,
    }
}

impl Scene {
    fn new(b: Breaker, ground: &Ground, trigger: u32) -> Scene {
        let mut switches = Vec::new();
        let mut groups: Vec<(u16, bool)> = Vec::new();
        for t in &b.toggles {
            if let Some(g) = ground.group_of(t.entity, t.component) {
                switches.push((ticks(t.time), g, t.on));
                if !groups.iter().any(|x| x.0 == g) {
                    groups.push((g, ground.group_on(g)));
                }
            }
        }
        switches.sort_by_key(|s| s.0);
        // the scene's last word: its trigger back on
        let rearm = b.toggles.iter().filter(|t| t.on && t.entity == b.group && t.component == trigger).map(|t| t.time).fold(b.length, f32::max);
        Scene {
            to_zone: Mat4::from_cols_array(&b.matrix).inverse(),
            switches,
            groups,
            length: ticks(rearm).max(1),
            every: ticks(b.update_frequency).max(1),
            data: b,
        }
    }

    /// Is a world point in the trigger's zone (retail
    /// `TriggerZone::IsGlobalPositionInZone` 0080be40)?
    pub fn in_zone(&self, p: Vec3) -> bool {
        let l = self.to_zone.transform_point3(p);
        match self.data.zone {
            ZoneShape::Sphere { center, radius } => (l - Vec3::from(center)).length_squared() < radius * radius,
            ZoneShape::Box { center, half } => {
                let d = (l - Vec3::from(center)).abs();
                d.x <= half[0] && d.y <= half[1] && d.z <= half[2]
            }
        }
    }

    /// One tick of the trigger and the scene.
    fn tick(&self, st: &mut SceneState, tick: u64, players: &[Watched]) {
        if let Some(k) = st.playing {
            if k + 1 >= self.length {
                *st = SceneState { played: true, ..Default::default() };
            } else {
                st.playing = Some(k + 1);
            }
            return;
        }
        if let Some(d) = st.pending {
            if d <= 1 {
                st.pending = None;
                st.playing = Some(0);
            } else {
                st.pending = Some(d - 1);
            }
            return;
        }
        let b = &self.data;
        let mut now = 0u64;
        for (i, w) in players.iter().enumerate().take(64) {
            if self.in_zone(w.pos + Vec3::Z * ENTITY_HEIGHT) {
                now |= 1 << i;
            }
        }
        let (entered, exited) = (now & !st.inside, st.inside & !now);
        st.inside = now;
        let mut fire: Option<f32> = None;
        let mut seed = |d: Option<f32>| {
            if let Some(d) = d {
                fire = Some(fire.map_or(d, |f: f32| f.min(d)));
            }
        };
        if entered != 0 {
            if b.reset_condition_on_enter {
                st.fired = false;
            }
            seed(b.on_enter);
        }
        if exited != 0 {
            seed(b.on_exit);
        }
        if now != 0 && tick % self.every as u64 == 0 {
            seed(b.on_update);
        }
        if b.on_condition.is_some() {
            let who = match b.watch {
                data::WATCH_EXIT => exited,
                data::WATCH_ENTER => entered,
                data::WATCH_INSIDE => now,
                _ => 0,
            };
            for (i, w) in players.iter().enumerate().take(64) {
                if who & (1 << i) == 0 || (b.fire_only_once && st.fired) {
                    continue;
                }
                if b.condition.as_ref().is_none_or(|c| holds(c, w)) {
                    st.fired = true;
                    seed(b.on_condition);
                }
            }
        }
        match fire.map(ticks) {
            Some(0) => st.playing = Some(0),
            Some(d) => st.pending = Some(d),
            None => {}
        }
    }

    /// Is a ground group on in this state?
    fn group_on(&self, st: &SceneState, group: u16, initial: bool) -> bool {
        let at = match (st.playing, st.played) {
            (Some(k), _) => k,
            (None, true) => u32::MAX,
            (None, false) => return initial,
        };
        // an earlier play left the group where its last switch put it
        let before = if st.played { self.switches.iter().rev().find(|s| s.1 == group).map_or(initial, |s| s.2) } else { initial };
        self.switches.iter().take_while(|s| s.0 <= at).filter(|s| s.1 == group).last().map_or(before, |s| s.2)
    }
}

/// A lift, ready to take riders.
pub struct LiftSpot {
    pub data: Lift,
    /// Centre of the detection sphere.
    pub detect: Vec3,
    /// The way a rider faces to be taken (and rides).
    pub dir: Vec2,
    /// Where the rider starts.
    pub start: Vec3,
    /// Ticks from a ride's start until the lift takes someone again: its
    /// up animation plus its rewind.
    pub reload: u32,
}

/// The map's breakers (static).
pub struct Breakers {
    pub scenes: Vec<Scene>,
    pub lifts: Vec<LiftSpot>,
}

/// What changes in the breakers, tick by tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub scenes: Vec<SceneState>,
    /// Per lift: ticks until it takes someone again (0: ready).
    pub lifts: Vec<u32>,
}

fn rotate(m: &[f32; 16], [x, y, z]: [f32; 3]) -> Vec3 {
    Vec3::new(x * m[0] + y * m[4] + z * m[8], x * m[1] + y * m[5] + z * m[9], x * m[2] + y * m[6] + z * m[10])
}

impl Breakers {
    /// The map's breakers; `ground` names the collision groups their
    /// scenes switch.
    pub fn load(archive: &Archive, ground: &Ground, builtins: Option<&BuiltinTracks>) -> Breakers {
        let trigger = archive.schema().hash_of("TriggerComponent").unwrap_or(0);
        let scenes = archive.breakers().into_iter().map(|b| Scene::new(b, ground, trigger)).collect();
        let mut lifts = Vec::new();
        for l in archive.lifts() {
            // `Animations`: wait, up, wait at the top, rewind
            let length = |k: usize| l.animations.get(k).and_then(|&a| Clip::load_with(archive, a, builtins)).map(|c| c.length);
            let (Some(up), Some(rewind)) = (length(1), length(3)) else {
                eprintln!("lift {}: animations not found", l.name);
                continue;
            };
            let dir = rotate(&l.matrix, l.detection_dir).truncate().normalize_or_zero();
            if dir == Vec2::ZERO {
                continue;
            }
            lifts.push(LiftSpot {
                detect: Vec3::from(acb_world::collision::xform(&l.matrix, l.detection_offset)),
                start: Vec3::from(acb_world::collision::xform(&l.matrix, l.assassin_offset)),
                dir,
                reload: ticks(up + rewind),
                data: l,
            });
        }
        Breakers { scenes, lifts }
    }

    /// Everything open, standing and ready.
    pub fn state(&self) -> State {
        State { scenes: vec![SceneState::default(); self.scenes.len()], lifts: vec![0; self.lifts.len()] }
    }

    /// One tick: the triggers watch `players`, scenes and lift reloads
    /// run on.
    pub fn tick(&self, st: &mut State, tick: u64, players: &[Watched]) {
        for (scene, s) in self.scenes.iter().zip(&mut st.scenes) {
            scene.tick(s, tick, players);
        }
        for l in &mut st.lifts {
            *l = l.saturating_sub(1);
        }
    }

    /// Puts the collision groups where the scenes have them.
    pub fn apply(&self, st: &State, ground: &mut Ground) {
        for (scene, s) in self.scenes.iter().zip(&st.scenes) {
            for &(g, initial) in &scene.groups {
                ground.set_group(g, scene.group_on(s, g, initial));
            }
        }
    }

    /// The lift that takes a character at `pos` (feet) facing `facing`
    /// now (retail `ElevatorComponent::IsInDetectionZone`): ready, the
    /// entity point inside the sphere, and facing within `MaxAngle` of the
    /// detection direction. Ours: the sphere is also reached from up to
    /// [`LIFT_AHEAD`] before it along the facing.
    pub fn lift_for(&self, st: &State, pos: Vec3, facing: Vec2) -> Option<usize> {
        let p = pos + Vec3::Z * ENTITY_HEIGHT;
        self.lifts.iter().enumerate().position(|(k, l)| {
            let to = l.detect - p;
            let near = to - facing.extend(0.0) * to.truncate().dot(facing).clamp(0.0, LIFT_AHEAD);
            st.lifts.get(k).copied().unwrap_or(0) == 0
                && near.length_squared() < l.data.activation_radius * l.data.activation_radius
                && facing != Vec2::ZERO
                && l.dir.angle_to(facing).abs() <= l.data.max_angle
        })
    }

    /// Starts a ride on lift `k`.
    pub fn ride(&self, st: &mut State, k: usize, pose: Pose, world: Mat4) -> Ride {
        let l = &self.lifts[k];
        st.lifts[k] = l.reload;
        let yaw = (-l.dir.x).atan2(l.dir.y);
        Ride { lift: k, height: l.data.height.min(3) as usize, phase: 0, t: 0.0, frame: Mat4::from_rotation_translation(Quat::from_rotation_z(yaw), l.start), from_pose: pose, from_world: world }
    }
}

/// The rider's clips: onto the rope, up, off at the top. Picked by root
/// motion (ours, see the module doc): `..._positionf` 0.75 m forward and
/// 3 m up, `..._Up0800` 4 m and `..._Up1200` 8 m up, `..._out` 1.1 m
/// forward and 1.6 m up; the 16 m and 20 m lifts ride the `xx_lift_*`
/// cycles (12.2 m, 14.2 m).
#[derive(Clone)]
pub struct LiftClips {
    pub position: Clip,
    /// By `ElevatorHeight`.
    pub up: [Option<Clip>; 4],
    pub out: Clip,
}

impl LiftClips {
    pub fn load(archive: &Archive, builtins: Option<&BuiltinTracks>) -> Option<LiftClips> {
        let load = |n: &str| {
            archive
                .uids_of("Animation")
                .into_iter()
                .find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(n)))
                .and_then(|a| Clip::load_with(archive, a, builtins))
        };
        Some(LiftClips {
            position: load("IDxx_ChaseBreaker_Lift_positionf")?,
            up: [load("IDxx_ChaseBreaker_Lift_Up0800"), load("IDxx_ChaseBreaker_Lift_Up1200"), load("xx_lift_1600_cycle"), load("xx_lift_2000_cycle")],
            out: load("IDxx_ChaseBreaker_Lift_out")?,
        })
    }
}

/// A player riding a lift (retail states ElevatorRaiseStart, RaiseUp,
/// RaiseExit; unkillable throughout, `EnterHumanElevator` 00f791e4).
#[derive(Clone)]
pub struct Ride {
    pub lift: usize,
    height: usize,
    /// 0 onto the rope, 1 up, 2 off at the top.
    phase: u8,
    t: f32,
    /// Where the current clip started.
    frame: Mat4,
    from_pose: Pose,
    from_world: Mat4,
}

impl Ride {
    fn clip<'a>(&self, c: &'a LiftClips) -> Option<&'a Clip> {
        match self.phase {
            0 => Some(&c.position),
            1 => c.up[self.height].as_ref(),
            _ => Some(&c.out),
        }
    }

    /// One tick: the pose, its world placement, the logical position /
    /// yaw, and whether the ride is over (the rider is in the air above
    /// the roof).
    pub fn update(&mut self, clips: &LiftClips, sk: &Skeleton, dt: f32) -> (Pose, Mat4, Vec3, f32, bool) {
        self.t += dt;
        let mut total = self.t;
        while self.phase < 2 {
            let len = self.clip(clips).map_or(0.0, |c| c.length);
            if self.t < len {
                break;
            }
            if let Some(c) = self.clip(clips) {
                self.frame *= c.root_motion(c.length);
            }
            self.t -= len;
            self.phase += 1;
            total = f32::MAX;
        }
        let clip = self.clip(clips).unwrap_or(&clips.out);
        let ct = self.t.min(clip.length);
        let mut pose = clip.sample(sk, ct);
        let mut world = self.frame * clip.root_motion(ct);
        let (_, r, at) = world.to_scale_rotation_translation();
        if self.phase == 0 && total < BLEND {
            let w = total / BLEND;
            let mut from = self.from_pose.clone();
            acb_anim::pose::blend(&mut from, &pose, w);
            pose = from;
            let (_, r0, t0) = self.from_world.to_scale_rotation_translation();
            world = Mat4::from_rotation_translation(r0.slerp(r, w), t0.lerp(at, w));
        }
        let out = self.phase == 2 && self.t >= clip.length;
        (pose, world, at, r.to_euler(glam::EulerRot::ZXY).0, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn door() -> Scene {
        let b = Breaker {
            group: 1,
            name: "door".into(),
            kind: Some(data::KIND_DOORS),
            matrix: Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)).to_cols_array(),
            zone: ZoneShape::Box { center: [0.0, 0.0, 1.2], half: [0.33, 1.2, 1.5] },
            watch: data::WATCH_EXIT,
            update_frequency: 0.5,
            on_enter: None,
            on_update: None,
            on_exit: None,
            on_condition: Some(0.0),
            condition: Some(Condition::All(vec![
                Condition::Not(Box::new(Condition::ActorState(data::ACTOR_STATE_WALK))),
                Condition::Not(Box::new(Condition::ActorState(data::ACTOR_STATE_WAIT_LOW))),
                Condition::Not(Box::new(Condition::InConflict)),
            ])),
            fire_only_once: true,
            reset_condition_on_enter: true,
            toggles: Vec::new(),
            moves: Vec::new(),
            length: 3.6,
        };
        Scene { to_zone: Mat4::from_cols_array(&b.matrix).inverse(), switches: vec![(0, 1, true), (ticks(3.5), 1, false)], groups: vec![(1, false)], length: ticks(3.6), every: 30, data: b }
    }

    fn at(x: f32, walking: bool) -> Watched {
        Watched { pos: Vec3::new(x, 0.0, 0.0), walking, idle: false, conflict: false }
    }

    #[test]
    fn a_door_shuts_behind_a_runner_only() {
        let s = door();
        // walking through: stays open
        let mut st = SceneState::default();
        for (tick, x) in [9.0, 10.0, 11.0].into_iter().enumerate() {
            s.tick(&mut st, tick as u64, &[at(x, true)]);
            assert!(!s.group_on(&st, 1, false));
        }
        // running through: shut from the tick the runner is out, for 3.5 s,
        // and the trigger is back 0.1 s later
        let mut st = SceneState::default();
        s.tick(&mut st, 0, &[at(9.0, false)]);
        s.tick(&mut st, 1, &[at(10.0, false)]);
        assert!(st.playing.is_none() && st.inside == 1);
        s.tick(&mut st, 2, &[at(11.0, false)]);
        assert_eq!(st.playing, Some(0));
        assert!(s.group_on(&st, 1, false));
        let mut shut = 1;
        let mut tick = 3;
        while st.playing.is_some() {
            s.tick(&mut st, tick, &[at(11.0, false)]);
            shut += u32::from(s.group_on(&st, 1, false));
            tick += 1;
        }
        assert_eq!(shut, ticks(3.5));
        assert_eq!(tick - 3, ticks(3.6) as u64);
        assert!(!s.group_on(&st, 1, false) && st.inside == 0 && !st.fired);
        // someone in a kill does not shut it
        let mut st = SceneState::default();
        s.tick(&mut st, 0, &[Watched { conflict: true, ..at(10.0, false) }]);
        s.tick(&mut st, 1, &[Watched { conflict: true, ..at(11.0, false) }]);
        assert!(st.playing.is_none());
    }

    #[test]
    fn a_platform_falls_a_second_after_someone_is_on() {
        let mut s = door();
        s.data.watch = data::WATCH_INSIDE;
        s.data.on_condition = None;
        s.data.condition = None;
        s.data.on_enter = Some(1.0);
        s.data.on_update = Some(0.8);
        s.switches = vec![(0, 1, false), (ticks(4.5), 1, true)];
        s.groups = vec![(1, true)];
        s.length = ticks(4.7);
        // on at tick 31: the enter seed (1.0 s) is first
        let mut st = SceneState::default();
        s.tick(&mut st, 31, &[at(10.0, true)]);
        assert_eq!(st.pending, Some(60));
        // on at one of the trigger's updates: its seed (0.8 s) is first
        let mut st = SceneState::default();
        s.tick(&mut st, 30, &[at(10.0, true)]);
        assert_eq!(st.pending, Some(48));
        let mut tick = 31;
        while st.pending.is_some() {
            assert!(s.group_on(&st, 1, true));
            s.tick(&mut st, tick, &[]);
            tick += 1;
        }
        assert_eq!(tick - 31, 48);
        let mut gone = 0;
        while st.playing.is_some() {
            gone += u32::from(!s.group_on(&st, 1, true));
            s.tick(&mut st, tick, &[]);
            tick += 1;
        }
        assert_eq!(gone, ticks(4.5));
        assert!(s.group_on(&st, 1, true));
    }
}
