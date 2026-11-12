//! Chase breakers: closing doors, falling platforms and poles, lifts.
//!
//! A door or platform breaker is an `EntityGroup` with a `TriggerComponent`
//! (a zone watching characters) and a `Scene` (inactive at rest). The
//! trigger's event seeds turn the Scene on; the Scene is a small timeline
//! of clips between checkpoints: `ActivateClip` turns a component type of
//! an entity on or off (the door's blocker `InertComponent`, the platform's
//! collision and guidance), `BlendPosClip` moves an entity (door leaves,
//! the platform dropping), `NothingClip` waits. Nothing here is specific to
//! doors or platforms: every map's breakers are read the same way.
//!
//! A lift is an entity with an `ElevatorComponent` (where it is taken from,
//! how high it goes) and an `AnimComponent` whose four animations are
//! wait, up, wait at the top and rewind, in that order (retail
//! `ElevatorComponent::ActivateElevator` plays 1, `UpdateAfter` then 3,
//! then 0).

use std::collections::HashMap;

use anvil_fastload::{Obj, Value};

use crate::Archive;

/// `TriggerComponentSettings::WatchType`: what the additional condition's
/// events fire on.
pub const WATCH_INSIDE: u32 = 0;
pub const WATCH_ENTER: u32 = 2;
pub const WATCH_EXIT: u32 = 3;

/// `ChaseBreakerEvent::Type`.
pub const KIND_DOORS: u32 = 0;
pub const KIND_BEAM_POLE: u32 = 1;
pub const KIND_LIFT: u32 = 2;
pub const KIND_CORNER: u32 = 3;
pub const KIND_PLATFORM: u32 = 4;

/// `ActorStateID`s the door conditions name.
pub const ACTOR_STATE_WALK: u32 = 58;
pub const ACTOR_STATE_WAIT_LOW: u32 = 158;

/// A trigger zone in its owner's frame.
#[derive(Debug, Clone)]
pub enum ZoneShape {
    Sphere { center: [f32; 3], radius: f32 },
    Box { center: [f32; 3], half: [f32; 3] },
}

/// `IRuleCondition` tree of a trigger's `AdditionalCondition`.
#[derive(Debug, Clone)]
pub enum Condition {
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    /// `AC2EventCondition`: the character is in this `ActorStateID`.
    ActorState(u32),
    /// `AC2MPInConflictCondition`.
    InConflict,
    /// A condition type not decoded (type hash).
    Other(u32),
}

/// An `ActivateClip`: `component` (type hash) of `entity` goes on or off
/// `time` seconds into the scene.
#[derive(Debug, Clone)]
pub struct Toggle {
    pub time: f32,
    pub entity: u32,
    pub component: u32,
    pub on: bool,
}

/// A `BlendPosClip`: `entity` moves by `translate` and turns by `rotate`
/// (quaternion xyzw), both relative to where it is (`TargetType`
/// RelativeWorld), over `duration` seconds from `start`.
#[derive(Debug, Clone)]
pub struct Move {
    pub start: f32,
    pub duration: f32,
    pub entity: u32,
    pub translate: Option<[f32; 3]>,
    pub rotate: Option<[f32; 4]>,
    /// `BlendPosClip::TargetType`: 0 Absolute, 1 RelativeWorld, 2 RelativeLocal.
    pub target: u32,
}

/// A trigger + scene chase breaker.
#[derive(Debug, Clone)]
pub struct Breaker {
    pub group: u32,
    pub name: String,
    /// `ChaseBreakerEvent::Type` of the trigger's Activate event.
    pub kind: Option<u32>,
    /// The zone owner's `GlobalMatrix` (row vectors).
    pub matrix: [f32; 16],
    pub zone: ZoneShape,
    pub watch: u32,
    /// Seconds between the trigger's periodic updates (`OnUpdate`).
    pub update_frequency: f32,
    /// Delay (s) of the seed turning the Scene on in each seed list; `None`:
    /// that list does not start the scene.
    pub on_enter: Option<f32>,
    pub on_update: Option<f32>,
    pub on_exit: Option<f32>,
    pub on_condition: Option<f32>,
    /// `AdditionalCondition` when `UseAdditionalCondition` is set.
    pub condition: Option<Condition>,
    pub fire_only_once: bool,
    pub reset_condition_on_enter: bool,
    pub toggles: Vec<Toggle>,
    pub moves: Vec<Move>,
    /// Seconds from the scene's start to its last checkpoint.
    pub length: f32,
}

/// An entity with an `ElevatorComponent`.
#[derive(Debug, Clone)]
pub struct Lift {
    pub entity: u32,
    pub name: String,
    pub matrix: [f32; 16],
    pub use_only_once: bool,
    pub activation_radius: f32,
    /// Entity space.
    pub detection_dir: [f32; 3],
    pub detection_offset: [f32; 3],
    pub max_angle: f32,
    /// `ElevatorHeight`: 0 = 8 m, 1 = 12 m, 2 = 16 m, 3 = 20 m.
    pub height: u32,
    /// Where the rider starts, entity space.
    pub assassin_offset: [f32; 3],
    pub counter_weight_offset: [f32; 3],
    pub counter_weight_damage_radius: f32,
    /// The prop's `AnimComponent::Animations` (wait, up, wait top, rewind).
    pub animations: Vec<u32>,
}

impl Lift {
    /// Metres the lift raises its rider.
    pub fn rise(&self) -> f32 {
        8.0 + 4.0 * self.height.min(3) as f32
    }
}

fn vec3(v: Option<&Value>) -> Option<[f32; 3]> {
    let f = v?.as_f32s()?;
    Some([*f.first()?, *f.get(1)?, *f.get(2)?])
}

impl Archive<'_> {
    fn u32_of(&self, o: &Obj, n: &str) -> u32 {
        self.field(o, n).and_then(Value::as_u32).unwrap_or(0)
    }

    fn flag_of(&self, o: &Obj, n: &str) -> bool {
        self.field(o, n).and_then(Value::as_bool).unwrap_or(false)
    }

    fn components<'o>(&self, e: &'o Obj) -> impl Iterator<Item = &'o Obj> + 'o {
        self.field(e, "Components").map(Value::items).unwrap_or_default().iter().filter_map(Value::as_obj)
    }

    fn condition(&self, o: &Obj) -> Condition {
        let c = match self.schema().name_or_hash(o.type_hash).as_str() {
            "BooleanRuleCondition" => {
                let subs: Vec<Condition> = self
                    .field(o, "SubConditions")
                    .map(Value::items)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_obj)
                    .filter(|s| self.field(s, "Enabled").and_then(Value::as_bool).unwrap_or(true))
                    .map(|s| self.condition(s))
                    .collect();
                // BooleanOperation: 0 BO_And, 1 BO_Or
                if self.u32_of(o, "Operator") == 1 {
                    Condition::Any(subs)
                } else {
                    Condition::All(subs)
                }
            }
            "AC2EventCondition" => Condition::ActorState(self.u32_of(o, "ActorState")),
            "AC2MPInConflictCondition" => Condition::InConflict,
            _ => Condition::Other(o.type_hash),
        };
        if self.flag_of(o, "Negation") {
            Condition::Not(Box::new(c))
        } else {
            c
        }
    }

    /// Delay of the seed in `list` that turns the Scene of `group` on, and
    /// the `ChaseBreakerEvent` type of an Activate seed in it.
    fn seeds(&self, settings: &Obj, list: &str, group: u32, scene: u32, kind: &mut Option<u32>) -> Option<f32> {
        let mut start = None;
        for s in self.field(settings, list).map(Value::items).unwrap_or_default().iter().filter_map(Value::as_obj) {
            match self.schema().name_or_hash(s.type_hash).as_str() {
                "ActivateEventSeed" => {
                    let Some(t) = self.field(s, "EventTemplate").and_then(Value::as_obj) else { continue };
                    let target = self.field(s, "EventTargetEntity").and_then(Value::target_id);
                    // ActivateEventAction: 0 TurnOn
                    if self.u32_of(t, "Action") == 0 && self.u32_of(t, "AffectedComponentType") == scene && target.is_none_or(|e| e == group) {
                        start = Some(self.f32_of(s, "Delay"));
                    }
                }
                // ChaseBreakerAction: 1 Activate
                "ChaseBreakerEventSeed" if self.u32_of(s, "Action") == 1 => *kind = Some(self.u32_of(s, "Type")),
                _ => {}
            }
        }
        start
    }

    /// The scene's clips as timed toggles and moves, and its length.
    ///
    /// A clip runs from its `StartPoint` checkpoint (plus its start delay)
    /// and a checkpoint is reached when every clip ending on it is done. A
    /// checkpoint no clip ends on floats: its clips are placed to end on
    /// their end points (San Donato's platform comes back up 0.1 s before
    /// the checkpoint that turns its collision on again).
    fn scene(&self, scene: &Obj) -> (Vec<Toggle>, Vec<Move>, f32) {
        struct ClipAt<'o> {
            obj: &'o Obj,
            entity: Option<u32>,
            start: u32,
            end: u32,
            delay: f32,
            dur: f32,
        }
        let mut clips = Vec::new();
        for actor in self.field(scene, "Actors").map(Value::items).unwrap_or_default().iter().filter_map(Value::as_obj) {
            let entity = self.field(actor, "ControlledEntity").and_then(Value::target_id);
            for c in self.field(actor, "Clips").map(Value::items).unwrap_or_default().iter().filter_map(Value::as_obj) {
                let point = |n: &str| self.field(c, n).and_then(Value::target_id).unwrap_or(0);
                let dur = match self.schema().name_or_hash(c.type_hash).as_str() {
                    "BlendPosClip" => self.f32_of(c, "BlendDuration"),
                    _ => self.f32_of(c, "PlayTimeMin"),
                };
                clips.push(ClipAt { obj: c, entity, start: point("StartPoint"), end: point("EndPoint"), delay: self.f32_of(c, "StartDelayMin"), dur });
            }
        }
        let first = self.field(scene, "StartCheckpoint").and_then(Value::as_obj).map_or(0, |o| o.id);
        let mut points: Vec<u32> = vec![first];
        points.extend(self.field(scene, "AdditionalCheckPoints").map(Value::items).unwrap_or_default().iter().filter_map(Value::as_obj).map(|o| o.id));
        let floating = |p: u32| p != first && !clips.iter().any(|c| c.end == p);
        let mut at: HashMap<u32, f32> = HashMap::from([(first, 0.0)]);
        // fixed checkpoints: the latest clip ending there
        for _ in 0..points.len() {
            for &p in &points {
                if p == first || floating(p) {
                    continue;
                }
                let pre: Vec<&ClipAt> = clips.iter().filter(|c| c.end == p && !floating(c.start)).collect();
                if pre.iter().all(|c| at.contains_key(&c.start)) {
                    let t = pre.iter().map(|c| at[&c.start] + c.delay + c.dur).fold(0.0, f32::max);
                    at.insert(p, t);
                }
            }
        }
        for &p in &points {
            if floating(p) {
                let t = clips.iter().filter(|c| c.start == p).filter_map(|c| Some(at.get(&c.end)? - c.delay - c.dur)).fold(f32::INFINITY, f32::min);
                if t.is_finite() {
                    at.insert(p, t.max(0.0));
                }
            }
        }
        let (mut toggles, mut moves) = (Vec::new(), Vec::new());
        for c in &clips {
            let Some(&t0) = at.get(&c.start) else { continue };
            let start = t0 + c.delay;
            match self.schema().name_or_hash(c.obj.type_hash).as_str() {
                "ActivateClip" => {
                    let (Some(action), Some(entity)) = (self.field(c.obj, "Action").and_then(Value::as_obj), self.field(c.obj, "Entity").and_then(Value::target_id)) else {
                        continue;
                    };
                    // ActivateEventAction: 0 TurnOn, 1 TurnOff (2 Switch: not used by the maps)
                    let on = match self.u32_of(action, "Action") {
                        0 => true,
                        1 => false,
                        _ => continue,
                    };
                    toggles.push(Toggle { time: start, entity, component: self.u32_of(action, "AffectedComponentType"), on });
                }
                "BlendPosClip" => {
                    let Some(entity) = c.entity else { continue };
                    let translate = vec3(self.field(c.obj, "TargetPosition")).filter(|_| self.flag_of(c.obj, "BlendPosition"));
                    let rotate = self
                        .field(c.obj, "TargetRotation")
                        .and_then(Value::as_f32s)
                        .and_then(|q| <[f32; 4]>::try_from(q).ok())
                        .filter(|_| self.flag_of(c.obj, "BlendRotation"));
                    moves.push(Move { start, duration: c.dur, entity, translate, rotate, target: self.u32_of(c.obj, "TargetType") });
                }
                _ => {}
            }
        }
        toggles.sort_by(|a, b| a.time.total_cmp(&b.time));
        moves.sort_by(|a, b| a.start.total_cmp(&b.start));
        let length = at.values().copied().fold(0.0, f32::max);
        (toggles, moves, length)
    }

    fn breaker_of(&self, group: &Obj, name: String) -> Option<Breaker> {
        let scene = self.components(group).find(|c| self.is_type(c, "Scene"))?;
        let trigger = self.components(group).find(|c| self.is_type(c, "TriggerComponent"))?;
        let settings = self.field(trigger, "Settings").and_then(Value::as_obj)?;
        let zone = self.field(settings, "Zone").and_then(Value::as_obj)?;
        // TriggerZoneType: 1 Sphere, 3 Box (2 Plane: no breaker uses it)
        let shape = match self.u32_of(zone, "ZoneType") {
            1 => ZoneShape::Sphere { center: vec3(self.field(zone, "SphereLocalCenter"))?, radius: self.f32_of(zone, "SphereRadius") },
            3 => ZoneShape::Box { center: vec3(self.field(zone, "BoxLocalCenter"))?, half: vec3(self.field(zone, "BoxHalfExtents"))? },
            _ => return None,
        };
        let matrix: [f32; 16] = self.field(group, "GlobalMatrix").and_then(Value::as_f32s)?.try_into().ok()?;
        let mut kind = None;
        let scene_type = scene.type_hash;
        let mut seeds = |list: &str| self.seeds(settings, list, group.id, scene_type, &mut kind);
        let (on_enter, on_update, on_exit, on_condition) = (seeds("OnEnter"), seeds("OnUpdate"), seeds("OnExit"), seeds("OnCondition"));
        let condition = self
            .flag_of(settings, "UseAdditionalCondition")
            .then(|| self.field(settings, "AdditionalCondition").and_then(Value::as_obj).map(|c| self.condition(c)))
            .flatten();
        let (toggles, moves, length) = self.scene(scene);
        Some(Breaker {
            group: group.id,
            name,
            kind,
            matrix,
            zone: shape,
            watch: self.u32_of(settings, "WatchType"),
            update_frequency: self.f32_of(settings, "UpdateFrequency"),
            on_enter,
            on_update,
            on_exit,
            on_condition,
            condition,
            fire_only_once: self.flag_of(settings, "FireOnlyOnce"),
            reset_condition_on_enter: self.flag_of(settings, "ResetConditionOnEnter"),
            toggles,
            moves,
            length,
        })
    }

    /// Every trigger + scene group of the map (doors, platforms, poles).
    pub fn breakers(&self) -> Vec<Breaker> {
        let mut out = Vec::new();
        for uid in self.uids_of_in("EntityGroup", 0) {
            let Ok(r) = self.root(uid) else { continue };
            let name = self.loc(uid).map(|l| l.name.clone()).unwrap_or_default();
            if let Some(b) = self.breaker_of(&r.obj, name).filter(|b| b.on_enter.or(b.on_update).or(b.on_exit).or(b.on_condition).is_some()) {
                out.push(b);
            }
        }
        out
    }

    fn lift_of(&self, e: &Obj, name: String) -> Option<Lift> {
        let c = self.components(e).find(|c| self.is_type(c, "ElevatorComponent"))?;
        let matrix: [f32; 16] = self.field(e, "GlobalMatrix").and_then(Value::as_f32s)?.try_into().ok()?;
        let animations = self
            .components(e)
            .find(|c| self.is_type(c, "AnimComponent"))
            .map(|a| self.field(a, "Animations").map(Value::items).unwrap_or_default().iter().map(|v| v.target_id().unwrap_or(0)).collect())
            .unwrap_or_default();
        let v = |n: &str| vec3(self.field(c, n)).unwrap_or_default();
        Some(Lift {
            entity: e.id,
            name,
            matrix,
            use_only_once: self.flag_of(c, "UseOnlyOnce"),
            activation_radius: self.f32_of(c, "ActivationRadius"),
            detection_dir: v("DetectionZoneDir"),
            detection_offset: v("DetectionZoneOffset"),
            max_angle: self.f32_of(c, "MaxAngleToDetectElevator"),
            height: self.u32_of(c, "ElevatorHeight"),
            assassin_offset: v("AssassinOffsetOnStart"),
            counter_weight_offset: v("CounterWeightOffset"),
            counter_weight_damage_radius: self.f32_of(c, "CounterWeightDamageRadius"),
            animations,
        })
    }

    /// Every lift of the map (entities with an `ElevatorComponent`, on
    /// their own or inside a group).
    pub fn lifts(&self) -> Vec<Lift> {
        let mut out = Vec::new();
        for uid in self.uids_of_in("Entity", 0) {
            let Ok(r) = self.root(uid) else { continue };
            let name = self.loc(uid).map(|l| l.name.clone()).unwrap_or_default();
            out.extend(self.lift_of(&r.obj, name));
        }
        let entity = self.schema().hash_of("Entity");
        for uid in self.uids_of_in("EntityGroup", 0) {
            let Ok(r) = self.root(uid) else { continue };
            let name = self.loc(uid).map(|l| l.name.clone()).unwrap_or_default();
            r.obj.walk(&mut |o| {
                if Some(o.type_hash) == entity {
                    out.extend(self.lift_of(o, name.clone()));
                }
            });
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|l| seen.insert(l.entity));
        out
    }
}
