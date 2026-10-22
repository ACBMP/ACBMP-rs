//! Brotherhood MP gameplay, without a renderer: map collision and freerun
//! guidance, player locomotion, freerun jumps, climbing, kills and the
//! ambient crowd, advanced by a fixed 60 Hz tick.
//!
//! [`World::step`] takes one [`PlayerInput`] per player and advances
//! everything by [`DT`]; nothing reads a frame time. Each [`Body`] keeps
//! its placement and local pose after the last two ticks, so a client
//! draws between them ([`Body::interpolated`]) at any frame rate.

pub mod climb;
pub mod crowd;
pub mod ground;
pub mod jump;
pub mod kill;
pub mod locomotion;
pub mod action;
pub mod player;
pub mod rules;
pub mod state;

use std::sync::{Arc, OnceLock};

use acb_anim::character::Character;
use acb_anim::pose::Clip;
use acb_anim::BuiltinTracks;
use acb_world::Archive;
use glam::{Mat4, Quat, Vec3};

use climb::ClimbClips;
use crowd::{Agent, Nav};
use ground::Ground;
use kill::KillClips;
use locomotion::{Clips, Locomotion};
use player::Player;

/// Simulation rate and step.
pub const TICK_HZ: u32 = 60;
pub const DT: f32 = 1.0 / 60.0;

/// Local bone transforms (translation, rotation) in skeleton order.
pub type Pose = Vec<(Vec3, Quat)>;

/// `ACBMP_TRACE` is set: print player state and decisions.
pub fn trace() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("ACBMP_TRACE").is_some())
}

/// `ACBMP_TRACE=pose`: print the player's pose change every tick.
pub fn trace_pose() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("ACBMP_TRACE").as_deref() == Ok("pose"))
}

/// One player's controls for one tick. Plain data, so it can be recorded
/// or sent over the network as is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerInput {
    /// Movement keys, relative to `cam_yaw` (on a wall: up, down, left,
    /// right).
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    /// Camera heading the keys are relative to, radians (0 = +X, pi/2 = +Y).
    pub cam_yaw: f32,
    pub run: bool,
    pub sprint: bool,
    /// Not used yet: freerun jumps start by themselves while running.
    pub jump: bool,
    pub kill: bool,
    pub stun: bool,
}

/// A clip played once in a given frame and held at its end (--pair, kill
/// victims) instead of everything else.
#[derive(Clone)]
pub struct Scripted {
    pub clip: Clip,
    pub frame: Mat4,
    /// Simulated time it started at.
    pub start: f32,
}

/// A character in the world.
pub struct Body {
    pub character: Arc<Character>,
    pub spawn: Mat4,
    /// Entity transform (a player's body follows [`Player::entity`] after
    /// each tick).
    pub entity: Mat4,
    /// Skeleton model space -> entity space.
    pub model: Mat4,
    phase: f32,
    /// Clip time of the previous tick.
    clip_time: f32,
    epoch: u32,
    /// Navmesh wanderer (crowd) instead of the looping clip.
    pub agent: Option<Agent>,
    pub scripted: Option<Scripted>,
    /// Skeleton model space -> world and the local pose after the last
    /// tick, and after the one before.
    pub world: Mat4,
    pub pose: Pose,
    pub prev_world: Mat4,
    pub prev_pose: Pose,
}

impl Body {
    fn new(character: Arc<Character>, spawn: Mat4, entity: Mat4, model: Mat4, phase: f32, agent: Option<Agent>) -> Body {
        let pose = character.skeleton.bind_pose();
        let world = entity * model;
        Body {
            character,
            spawn,
            entity,
            model,
            phase,
            clip_time: phase,
            epoch: 0,
            agent,
            scripted: None,
            world,
            prev_world: world,
            prev_pose: pose.clone(),
            pose,
        }
    }

    /// Sets this tick's placement and pose.
    fn show(&mut self, world: Mat4, pose: Pose) {
        self.world = world;
        self.pose = pose;
    }

    /// Placement and pose between the last two ticks (`alpha` 0 = the
    /// previous tick, 1 = the last one).
    pub fn interpolated(&self, alpha: f32) -> (Mat4, Pose) {
        if alpha >= 1.0 {
            return (self.world, self.pose.clone());
        }
        let (s0, r0, t0) = self.prev_world.to_scale_rotation_translation();
        let (s1, r1, t1) = self.world.to_scale_rotation_translation();
        let world = Mat4::from_scale_rotation_translation(s0.lerp(s1, alpha), r0.slerp(r1, alpha), t0.lerp(t1, alpha));
        let mut pose = self.prev_pose.clone();
        acb_anim::pose::blend(&mut pose, &self.pose, alpha);
        (world, pose)
    }
}

/// Clips the player controller uses.
pub struct Clipset {
    pub loco: Option<Clips>,
    pub kills: Option<KillClips>,
    pub climbs: Option<ClimbClips>,
}

/// The crowd: the navmesh and the clips wanderers blend.
pub struct Crowd {
    pub nav: Nav,
    idle: Clip,
    walk: Clip,
}

/// What to put in a [`World`].
pub struct Setup<'a> {
    /// Characters with the names they were loaded by (personas, for their
    /// kill sets), placed round-robin on the standard player spawns.
    pub characters: Vec<(String, Arc<Character>)>,
    /// How many spawns get a character (default: one per character).
    pub count: Option<usize>,
    /// Clip the non-player characters loop.
    pub anim: String,
    /// Looping characters follow root motion and return to their spawn
    /// after this many seconds; 0 animates in place.
    pub wander: f32,
    /// The first character is a player.
    pub play: bool,
    /// NPCs wandering the crowd lanes or navmesh.
    pub crowd: usize,
    /// Two clips (assassin, victim) the first two characters play in the
    /// first spawn's frame.
    pub pair: Vec<String>,
    /// Player start: world x,y,z[,yaw degrees, 0 = +Y, 90 = -X].
    pub at: Option<Vec<f32>>,
    /// The engine's builtin tracks (root motion).
    pub builtins: Option<&'a BuiltinTracks>,
}

pub struct World {
    pub ground: Option<Ground>,
    pub crowd: Option<Crowd>,
    pub clips: Clipset,
    /// The looping clip of non-player characters.
    pub clip: Option<Clip>,
    pub wander: f32,
    pub bodies: Vec<Body>,
    /// Players drive bodies `0..players.len()`.
    pub players: Vec<Player>,
    /// Simulated seconds (sum of [`DT`]) and ticks since the start.
    pub time: f32,
    pub tick: u64,
}

/// An Entity or EntityBuilder by name.
pub fn load_character(archive: &Archive, name: &str) -> anyhow::Result<Character> {
    let c = match by_name(archive, "EntityBuilder", name) {
        Some(b) => Character::load_built(archive, b),
        None => by_name(archive, "Entity", name).and_then(|e| Character::load(archive, e)),
    };
    c.ok_or_else(|| anyhow::anyhow!("{name}: no such character"))
}

fn by_name(archive: &Archive, ty: &str, name: &str) -> Option<u32> {
    archive.uids_of(ty).into_iter().find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(name)))
}

impl World {
    /// Places the characters (and with `play` loads the player's clips and
    /// the map collision).
    pub fn load(archive: &Archive, setup: &Setup) -> anyhow::Result<World> {
        let mut w = World {
            ground: None,
            crowd: None,
            clips: Clipset { loco: None, kills: None, climbs: None },
            clip: None,
            wander: setup.wander,
            bodies: Vec::new(),
            players: Vec::new(),
            time: 0.0,
            tick: 0,
        };
        let characters = &setup.characters;
        if characters.is_empty() {
            return Ok(w);
        }
        let builtins = setup.builtins;
        let load_clip = |name: &str| by_name(archive, "Animation", name).and_then(|a| Clip::load_with(archive, a, builtins));
        // a stride half (`*_footl`) loops together with its other half
        let clip = load_clip(&setup.anim).map(|c| match acb_anim::pose::stride_partner(&setup.anim).and_then(|n| load_clip(&n)) {
            Some(other) => c.looped_with(&other),
            None => c,
        });
        if clip.is_none() {
            eprintln!("{}: animation not found, bind pose", setup.anim);
        }
        let loco = if setup.play { Some(Clips::load(archive, builtins)?) } else { None };
        let kills = if setup.play { KillClips::load(archive, builtins, &characters[0].0) } else { None };
        let climbs = if setup.play { Some(ClimbClips::load(archive, builtins)?) } else { None };
        if setup.play && kills.is_none() {
            eprintln!("{}: no ground kill clips", characters[0].0);
        }
        let spawns: Vec<_> = archive.player_spawns().into_iter().filter(|s| s.kind == 0).collect();
        let count = setup.count.unwrap_or(characters.len()).min(spawns.len());
        for (i, spawn) in spawns.iter().take(count).enumerate() {
            let character = characters[i % characters.len()].1.clone();
            let spawn = Mat4::from_cols_array(&spawn.matrix);
            // clips that animate Reference place the pelvis in entity space
            let root_bone = character.skeleton.bones.first().map(|b| b.id);
            let clip_i = if i == 0 && loco.is_some() { loco.as_ref().map(Clips::idle) } else { clip.as_ref() };
            let model = match (clip_i, root_bone) {
                (Some(clip), Some(id)) if clip.drives(id) => Mat4::IDENTITY,
                _ => character.model_to_entity(),
            };
            let phase = i as f32 * 0.37;
            w.bodies.push(Body::new(character, spawn, spawn, model, phase, None));
        }
        eprintln!("{} characters on {} standard spawns", w.bodies.len(), spawns.len());
        if trace() {
            for (i, b) in w.bodies.iter().enumerate() {
                let (p, f) = (b.spawn.w_axis, b.spawn.y_axis);
                eprintln!("body {i} spawn {:.2} {:.2} {:.2} facing {:.2} {:.2}", p.x, p.y, p.z, f.x, f.y);
            }
        }
        if trace() {
            for (i, a) in w.bodies.iter().enumerate() {
                let p = a.spawn.w_axis;
                let f = a.spawn.y_axis;
                eprintln!("actor {i} at {:.2} {:.2} {:.2} facing {:.2} {:.2}", p.x, p.y, p.z, f.x, f.y);
            }
        }
        if let [ass, victim] = setup.pair.as_slice() {
            for (i, name) in [ass, victim].into_iter().enumerate() {
                let clip = by_name(archive, "Animation", name)
                    .and_then(|a| Clip::load_with(archive, a, builtins))
                    .ok_or_else(|| anyhow::anyhow!("{name}: animation not found"))?;
                let shared = w.bodies[0].spawn;
                if let Some(a) = w.bodies.get_mut(i) {
                    (a.spawn, a.entity, a.model) = (shared, shared, Mat4::IDENTITY);
                    a.scripted = Some(Scripted { clip, frame: shared, start: 0.0 });
                }
            }
        }
        if setup.crowd > 0 {
            let load = |name: &str| {
                by_name(archive, "Animation", name)
                    .and_then(|a| Clip::load_with(archive, a, builtins))
                    .ok_or_else(|| anyhow::anyhow!("{name}: animation not found"))
            };
            let nav = Nav::load(archive).ok_or_else(|| anyhow::anyhow!("--crowd: the map has no navmesh"))?;
            // the walk cycle is two half strides (left foot, right foot)
            let walk = load("xx_l_walk_hipm_footl")?.then(&load("xx_l_walk_hipm_footr")?);
            let crowd = Crowd { nav, idle: load("xx_l_wait_hipm_footl")?, walk };
            for i in 0..setup.crowd {
                let character = characters[(count + i) % characters.len()].1.clone();
                let root_bone = character.skeleton.bones.first().map(|b| b.id);
                let model = match root_bone {
                    Some(id) if crowd.walk.drives(id) => Mat4::IDENTITY,
                    _ => character.model_to_entity(),
                };
                let agent = Agent::new(&crowd.nav, i as u64 + 1);
                let entity = agent.entity();
                w.bodies.push(Body::new(character, entity, entity, model, 0.0, Some(agent)));
            }
            eprintln!("crowd: {} NPCs, {} lanes, {} navmesh triangles", setup.crowd, crowd.nav.flows(), crowd.nav.triangles());
            w.crowd = Some(crowd);
        }
        // the player starts on the first spawn, facing along it (+Y)
        if let (Some(clips), Some(body)) = (&loco, w.bodies.first()) {
            let m = Mat4::from_cols_array(&spawns[0].matrix);
            let facing = m.y_axis.truncate().normalize_or_zero();
            let yaw = (-facing.x).atan2(facing.y);
            let (at, yaw) = match setup.at.as_deref() {
                Some([x, y, z]) => (Vec3::new(*x, *y, *z), yaw),
                // facing as a compass-free angle: 0 = +Y, 90 = -X
                Some([x, y, z, deg]) => (Vec3::new(*x, *y, *z), deg.to_radians()),
                _ => (m.w_axis.truncate(), yaw),
            };
            w.players.push(Player::new(0, at, yaw, Locomotion::new(clips, &body.character.skeleton)));
        }
        if loco.is_some() {
            let t = std::time::Instant::now();
            let g = Ground::load(archive)?;
            eprintln!("ground: {} triangles in {:.1}s", g.triangles(), t.elapsed().as_secs_f64());
            w.ground = Some(g);
        }
        if let (Some(p), Some(g)) = (w.players.first_mut(), &w.ground) {
            match g.floor_below(p.pos + Vec3::Z * 1.0) {
                // --at well above the floor: start falling from there
                Some(z) if setup.at.is_some() && p.pos.z - z > 1.5 => {
                    p.loco.start_fall();
                    p.fall_from = p.pos.z;
                }
                Some(z) => p.pos.z = z,
                None => {}
            }
            p.prev_pos = p.pos;
            if trace() {
                for (spot, face) in g.mount_spots(p.pos, 5) {
                    eprintln!("mount spot {:.2} {:.2} {:.2} facing {:.2} {:.2}", spot.x, spot.y, spot.z, face.x, face.y);
                }
            }
        }
        w.clip = clip;
        w.clips = Clipset { loco, kills, climbs };
        Ok(w)
    }

    /// The simulated time the next [`World::step`] runs at.
    pub fn next_time(&self) -> f32 {
        self.time + DT
    }

    /// Advances one tick; `inputs[i]` drives `players[i]` (missing: no
    /// input).
    pub fn step(&mut self, inputs: &[PlayerInput]) {
        for b in &mut self.bodies {
            b.prev_world = b.world;
            b.prev_pose.clone_from(&b.pose);
        }
        for p in &mut self.players {
            p.prev_pos = p.pos;
        }
        self.time += DT;
        self.tick += 1;
        let t = self.time;
        for (i, p) in self.players.iter_mut().enumerate() {
            let input = inputs.get(i).copied().unwrap_or_default();
            // ACBMP_TRACE=pose: per-tick pose change of the player (summed
            // bone rotation, radians), to spot clips that snap or reset
            let before = trace_pose().then(|| p.loco.last_pose().clone());
            player::drive(p, &mut self.bodies, &self.clips, self.ground.as_ref(), &input, t, DT);
            if let Some(before) = before {
                let jump: f32 = before.iter().zip(p.loco.last_pose()).map(|((_, a), (_, b))| a.angle_between(*b)).sum();
                eprintln!("pose t {t:.3} speed {:.2} change {jump:.2}{}", p.loco.speed, if p.loco.on_beam() { " beam" } else { "" });
            }
            // where others see it (crowd separation, targeting)
            if let Some(b) = self.bodies.get_mut(p.body) {
                b.entity = p.entity();
            }
        }
        let wander = self.wander;
        let skip = self.players.len();
        // everyone's position, for the crowd's separation (2 m buckets)
        let mut near: std::collections::HashMap<(i32, i32), Vec<Vec3>> = Default::default();
        let bucket = |p: Vec3| ((p.x / 2.0).floor() as i32, (p.y / 2.0).floor() as i32);
        for p in self.bodies.iter().map(|a| a.entity.w_axis.truncate()) {
            near.entry(bucket(p)).or_default().push(p);
        }
        for a in self.bodies.iter_mut().skip(skip) {
            let c = a.character.clone();
            if let (Some(agent), Some(crowd)) = (&mut a.agent, &self.crowd) {
                let (bx, by) = bucket(agent.pos);
                let others: Vec<Vec3> =
                    (-1..=1).flat_map(|x| (-1..=1).map(move |y| (bx + x, by + y))).filter_map(|k| near.get(&k)).flatten().copied().collect();
                let pose = agent.update(&crowd.nav, &crowd.idle, &crowd.walk, &c.skeleton, &others, DT);
                a.entity = agent.entity();
                let world = a.entity * a.model;
                a.show(world, pose);
                continue;
            }
            if let Some(sc) = &a.scripted {
                let ct = (t - sc.start).clamp(0.0, sc.clip.length);
                let pose = sc.clip.sample(&c.skeleton, ct);
                let world = sc.frame * sc.clip.root_motion(ct);
                a.show(world, pose);
                continue;
            }
            let local = match &self.clip {
                Some(clip) if clip.length > 0.0 => {
                    let ct = (t + a.phase) % clip.length;
                    if wander > 0.0 {
                        // back to the spawn every `wander` seconds
                        let epoch = ((t + a.phase) / wander) as u32;
                        if epoch != a.epoch {
                            a.epoch = epoch;
                            a.entity = a.spawn;
                        } else {
                            a.entity *= clip.root_delta(a.clip_time, ct);
                        }
                    }
                    a.clip_time = ct;
                    clip.sample(&c.skeleton, ct)
                }
                _ => c.skeleton.bind_pose(),
            };
            let world = a.entity * a.model;
            a.show(world, local);
        }
        // nothing to interpolate from on the first tick
        if self.tick == 1 {
            for b in &mut self.bodies {
                b.prev_world = b.world;
                b.prev_pose.clone_from(&b.pose);
            }
            for p in &mut self.players {
                p.prev_pos = p.pos;
            }
        }
    }
}
