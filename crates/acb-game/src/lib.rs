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
pub mod hide;
pub mod human;
pub mod jump;
pub mod kill;
pub mod locomotion;
pub mod manhunt;
pub mod mode;
pub mod abilities;
pub mod action;
pub mod bonus;
pub mod player;
pub mod rounds;
pub mod rules;
pub mod state;
pub mod vip;

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

/// `ACBMP_TRACE` is set (and is not `csv:<path>`): print player state and decisions.
pub fn trace() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("ACBMP_TRACE").is_some() && trace_csv().is_none())
}

/// CSV columns written by `ACBMP_TRACE=csv:<path>`, identical to `acb-trace record`
/// (retail captures). Units and axes: findings/trace-format.md.
pub const TRACE_CSV_HEADER: &str = "tick,t,x,y,z,yaw_deg,vx,vy,vz,hspeed,state,retail_state,anim,anim_t";

type CsvOut = std::sync::Mutex<std::io::BufWriter<std::fs::File>>;

/// `ACBMP_TRACE=csv:<path>`: the per-tick CSV writer (header already written).
pub fn trace_csv() -> Option<&'static CsvOut> {
    use std::io::Write;
    static CSV: OnceLock<Option<CsvOut>> = OnceLock::new();
    CSV.get_or_init(|| {
        let v = std::env::var("ACBMP_TRACE").ok()?;
        let path = v.strip_prefix("csv:")?;
        let f = std::fs::File::create(path).map_err(|e| eprintln!("ACBMP_TRACE: {path}: {e}")).ok()?;
        let mut f = std::io::BufWriter::new(f);
        writeln!(f, "{TRACE_CSV_HEADER}").ok()?;
        Some(std::sync::Mutex::new(f))
    })
    .as_ref()
}

/// One `ACBMP_TRACE=csv` row for a player (velocity = this tick's displacement / DT).
fn trace_csv_row(tick: u64, time: f32, p: &player::Player) {
    use std::io::Write;
    let Some(csv) = trace_csv() else { return };
    let v = (p.pos - p.prev_pos) / DT;
    let (state, retail) = p.trace_state();
    let row = format!(
        "{tick},{time:.4},{:.4},{:.4},{:.4},{:.2},{:.4},{:.4},{:.4},{:.4},{state},{retail},,",
        p.pos.x,
        p.pos.y,
        p.pos.z,
        p.yaw.to_degrees(),
        v.x,
        v.y,
        v.z,
        v.truncate().length()
    );
    if let Ok(mut f) = csv.lock() {
        let _ = writeln!(f, "{row}");
        let _ = f.flush();
    }
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
    /// Retail Legs (viewer Space): sprint, and the freerun jump button
    /// (held or pressed within 0.5 s, `human::jump`); standing, a straight
    /// jump.
    pub sprint: bool,
    /// Empty hand (viewer F): wall jumps while climbing, ledge pulldown /
    /// step-off at a low-profile edge stop.
    pub jump: bool,
    pub kill: bool,
    pub stun: bool,
    /// Assassinate lock (viewer R): a press locks the highlighted player as
    /// the acquisition candidate (no line of sight needed), another unlocks.
    pub lock: bool,
    /// Assassinate release (viewer Y, retail CancelWithY): a press drops the
    /// current contract.
    pub release: bool,
    /// Ticks between this input's tick and the tick the client drew the
    /// other players at (lag compensation; the sim rewinds at most
    /// [`rules::MAX_REWIND`]).
    pub view_lag: u8,
}

/// A clip played once in a given frame and held at its end (--pair, kill
/// victims) instead of everything else.
#[derive(Clone)]
pub struct Scripted {
    pub clip: Clip,
    pub frame: Mat4,
    /// Simulated time it started at.
    pub start: f32,
    /// A kill victim in a match: stays down at the end (until the match
    /// respawns it); otherwise a player victim gets up again.
    pub dies: bool,
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
    /// Hiding spots and their clips (`hide.rs`).
    pub hide: Option<hide::Hide>,
}

/// The crowd: the navmesh and the clips wanderers blend.
pub struct Crowd {
    pub nav: Nav,
    idle: Clip,
    walk: Clip,
}

impl Crowd {
    /// The navmesh and the crowd's idle and walk clips.
    pub fn load(archive: &Archive, builtins: Option<&BuiltinTracks>) -> anyhow::Result<Crowd> {
        let load = |name: &str| {
            by_name(archive, "Animation", name)
                .and_then(|a| Clip::load_with(archive, a, builtins))
                .ok_or_else(|| anyhow::anyhow!("{name}: animation not found"))
        };
        let nav = Nav::load(archive).ok_or_else(|| anyhow::anyhow!("the map has no navmesh"))?;
        // the walk cycle is two half strides (left foot, right foot)
        let walk = load("xx_l_walk_hipm_footl")?.then(&load("xx_l_walk_hipm_footr")?);
        Ok(Crowd { nav, idle: load("xx_l_wait_hipm_footl")?, walk })
    }

    /// Walking speed of the walk cycle's root motion, m/s.
    pub fn pace(&self) -> f32 {
        self.walk.root_motion(self.walk.length).w_axis.truncate().length() / self.walk.length.max(1e-3)
    }
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
    /// A running match (contracts, scores, respawns); `None`: free play.
    pub mode: Option<mode::Match>,
    /// Every player as the rules saw it at the end of each of the last
    /// [`rules::HISTORY`] ticks (oldest first): lag compensation checks a
    /// kill against where the killer saw its victim.
    pub seen: std::collections::VecDeque<Vec<rules::Actor>>,
    /// Escort VIP ways per attacking team (static per map; not saved).
    pub vip_ways: Vec<(rules::Team, Arc<vip::WaySet>)>,
    /// Escort team spawns (TeamIndex, position, yaw); team `t` starts every
    /// round on TeamIndex `t + 1` (our reading, as [`World::escort`]).
    pub team_spawns: Vec<(u32, Vec3, f32)>,
}

/// An Entity or EntityBuilder by name.
pub fn load_character(archive: &Archive, name: &str) -> anyhow::Result<Character> {
    let c = match by_name(archive, "EntityBuilder", name) {
        Some(b) => Character::load_built(archive, b),
        None => by_name(archive, "Entity", name).and_then(|e| Character::load(archive, e)),
    };
    c.ok_or_else(|| anyhow::anyhow!("{name}: no such character"))
}

/// Crowd NPCs on a map unless asked otherwise (viewer `--crowd`, server
/// `--crowd`). PLACEHOLDER (ours): retail's count is not traced (the crowd
/// follows the players' skins, `SkinInCrowdInfo`, filled from the network);
/// 60 costs 0.38 ms per sim tick on San Donato (`examples/tickbench.rs`).
pub const DEFAULT_CROWD: usize = 60;

/// The crowd's characters: every MP persona's NPC builder
/// (`AC2MP_IDxx_MainTemplate_NPC`, 14 in DataPC: ID07..ID27), sorted by
/// name so every peer builds the same crowd. The crowd is made of the
/// personas' lookalikes (retail ties it to the players' skins:
/// `GBrick_PlayerReady::SetSkinsAndCrowdReadyCallback` 013c2ed8; ours uses
/// all of them). Builders that fail to load are skipped.
pub fn crowd_characters(archive: &Archive) -> Vec<(String, Arc<Character>)> {
    let mut names: Vec<String> = archive
        .uids_of("EntityBuilder")
        .into_iter()
        .filter_map(|u| archive.loc(u).map(|l| l.name.clone()))
        .filter(|n| n.starts_with("AC2MP_ID") && n.ends_with("_MainTemplate_NPC"))
        .collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter_map(|n| match load_character(archive, &n) {
            Ok(c) => Some((n, Arc::new(c))),
            Err(e) => {
                eprintln!("crowd: {e}");
                None
            }
        })
        .collect()
}

fn by_name(archive: &Archive, ty: &str, name: &str) -> Option<u32> {
    archive.uids_of(ty).into_iter().find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(name)))
}

impl World {
    /// Restarts every crowd NPC from `seed` (NPC `i` from `seed + i + 1`;
    /// seed 0 is what [`World::load`] builds). Server and clients of a
    /// match call this with the same seed, so they build the same crowd.
    pub fn seed_crowd(&mut self, seed: u64) {
        let Some(crowd) = &self.crowd else { return };
        let mut i = 0u64;
        for b in self.bodies.iter_mut().filter(|b| b.agent.is_some()) {
            let agent = Agent::new(&crowd.nav, seed.wrapping_add(i + 1));
            i += 1;
            b.entity = agent.entity();
            b.spawn = b.entity;
            let world = b.entity * b.model;
            (b.world, b.prev_world) = (world, world);
            b.agent = Some(agent);
        }
    }

    /// Places the characters (and with `play` loads the player's clips and
    /// the map collision).
    pub fn load(archive: &Archive, setup: &Setup) -> anyhow::Result<World> {
        let mut w = World {
            ground: None,
            crowd: None,
            clips: Clipset { loco: None, kills: None, climbs: None, hide: None },
            clip: None,
            wander: setup.wander,
            bodies: Vec::new(),
            players: Vec::new(),
            time: 0.0,
            tick: 0,
            mode: None,
            seen: Default::default(),
            vip_ways: Vec::new(),
            team_spawns: Vec::new(),
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
                    a.scripted = Some(Scripted { clip, frame: shared, start: 0.0, dies: false });
                }
            }
        }
        if setup.crowd > 0 {
            let crowd = Crowd::load(archive, builtins)?;
            // every persona's lookalike, round-robin (else the players' characters)
            let pool = crowd_characters(archive);
            let pool = if pool.is_empty() { characters.clone() } else { pool };
            for i in 0..setup.crowd {
                let character = pool[i % pool.len()].1.clone();
                let root_bone = character.skeleton.bones.first().map(|b| b.id);
                let model = match root_bone {
                    Some(id) if crowd.walk.drives(id) => Mat4::IDENTITY,
                    _ => character.model_to_entity(),
                };
                let agent = Agent::new(&crowd.nav, i as u64 + 1);
                let entity = agent.entity();
                w.bodies.push(Body::new(character, entity, entity, model, 0.0, Some(agent)));
            }
            eprintln!("crowd: {} NPCs of {} characters, {} lanes, {} navmesh triangles", setup.crowd, pool.len(), crowd.nav.flows(), crowd.nav.triangles());
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
                Some([x, y, z, deg, ..]) => (Vec3::new(*x, *y, *z), deg.to_radians()),
                _ => (m.w_axis.truncate(), yaw),
            };
            w.players.push(Player::new(0, at, yaw, Locomotion::new(clips, &body.character.skeleton)));
        }
        // further x,y,z,yaw groups place bodies 1, 2, ... (scripted scenes)
        if let Some(rest) = setup.at.as_deref().filter(|a| a.len() > 4).map(|a| &a[4..]) {
            for (k, c) in rest.chunks_exact(4).enumerate() {
                if let Some(b) = w.bodies.get_mut(k + 1) {
                    let m = Mat4::from_rotation_translation(glam::Quat::from_rotation_z(c[3].to_radians()), Vec3::new(c[0], c[1], c[2]));
                    (b.spawn, b.entity) = (m, m);
                }
            }
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
        let hide = if setup.play {
            let spots = hide::spots(archive);
            match hide::HideClips::load(archive, builtins) {
                Some(clips) => {
                    let seats = spots.iter().filter(|s| s.seat.is_some()).count();
                    eprintln!("hide: {} haystacks", spots.len() - seats);
                    eprintln!("hide: {seats} bench seats{}", if clips.sit.is_some() { "" } else { " (sit clips not found)" });
                    if trace() {
                        for (i, s) in spots.iter().enumerate() {
                            let c = s.center();
                            match s.seat {
                                Some(t) => eprintln!("hide spot {i} at {:.2} {:.2} {:.2} seat {t} yaw {:.0}", c.x, c.y, c.z, hide::seat_yaw(s).to_degrees()),
                                None => eprintln!("hide spot {i} at {:.2} {:.2} {:.2} kind {}", c.x, c.y, c.z, s.kind),
                            }
                        }
                    }
                    Some(hide::Hide { spots, clips })
                }
                None => {
                    eprintln!("hide: haystack clips not found");
                    None
                }
            }
        } else {
            None
        };
        w.clips = Clipset { loco, kills, climbs, hide };
        Ok(w)
    }

    /// Starts an Escort (teamvip) match: adds [`vip::NB_VIPS`] VIP bodies
    /// (crowd walkers; our reading: lookalikes of the loaded personas, as
    /// retail's VIP skins ship no data we can map), generates the VIP ways
    /// for each team as the hunters (Escort teams use team spawn TeamIndex
    /// team + 1, our reading) and loads the crowd clips if there is no
    /// crowd. The VIPs spawn on the first tick.
    pub fn escort(&mut self, archive: &Archive, builtins: Option<&BuiltinTracks>, mut teams: rules::Teams, config: mode::MatchConfig, spawns: Vec<(Vec3, f32)>) -> anyhow::Result<()> {
        if self.crowd.is_none() {
            self.crowd = Some(Crowd::load(archive, builtins)?);
        }
        let Some(crowd) = &self.crowd else { unreachable!() };
        let team_spawns = vip::team_spawns(archive);
        let mut all: Vec<rules::Team> = teams.team.clone();
        all.push(teams.attacking_team);
        all.sort_unstable();
        all.dedup();
        for t in all {
            if self.vip_ways.iter().any(|(k, _)| *k == t) {
                continue;
            }
            let avoid: Vec<Vec3> = team_spawns.iter().filter(|s| s.0 == t as u32 + 1).map(|s| s.1).collect();
            let ways = vip::ways_for(&crowd.nav, &avoid);
            eprintln!("escort: team {t} hunting: {} VIP ways", ways.len());
            self.vip_ways.push((t, Arc::new(vip::WaySet::new(ways))));
        }
        let start = self.ways_of(teams.attacking_team).and_then(|w| w.fwd.first().map(|w| w.start())).unwrap_or(Vec3::ZERO);
        let mut bodies = Vec::new();
        for i in 0..vip::NB_VIPS {
            let Some(src) = self.bodies.get(i % self.bodies.len().max(1)) else { anyhow::bail!("escort: no character for the VIPs") };
            let character = src.character.clone();
            let root_bone = character.skeleton.bones.first().map(|b| b.id);
            let model = match root_bone {
                Some(id) if crowd.walk.drives(id) => Mat4::IDENTITY,
                _ => character.model_to_entity(),
            };
            let entity = Mat4::from_translation(start);
            bodies.push(self.bodies.len());
            self.bodies.push(Body::new(character, entity, entity, model, 0.0, None));
        }
        teams.escorted = bodies.clone();
        teams.vips.clear();
        self.team_spawns = archive
            .player_spawns()
            .into_iter()
            .filter(|s| s.kind == 2)
            .map(|s| {
                let m = Mat4::from_cols_array(&s.matrix);
                let f = m.y_axis.truncate();
                (s.team, m.w_axis.truncate(), (-f.x).atan2(f.y))
            })
            .collect();
        let team = teams.team.clone();
        self.mode = Some(mode::Match { vips: Some(vip::Vips::new(bodies)), ..mode::Match::escort(teams, config, spawns) });
        self.place_teams(&team);
        Ok(())
    }

    /// Starts a Manhunt (catsmice) match: Escort's teams, rounds and team
    /// spawn groups, with no VIPs ([`rules::TeamGame::Manhunt`]: the
    /// attacking team hunts the other team's players).
    /// Loads the crowd (its navmesh steers the test bots) when the map has one.
    pub fn manhunt(&mut self, archive: &Archive, builtins: Option<&BuiltinTracks>, mut teams: rules::Teams, config: mode::MatchConfig, spawns: Vec<(Vec3, f32)>) {
        if self.crowd.is_none() {
            self.crowd = Crowd::load(archive, builtins).ok();
        }
        teams.game = rules::TeamGame::Manhunt;
        teams.vips.clear();
        teams.escorted.clear();
        self.team_spawns = archive
            .player_spawns()
            .into_iter()
            .filter(|s| s.kind == 2)
            .map(|s| {
                let m = Mat4::from_cols_array(&s.matrix);
                let f = m.y_axis.truncate();
                (s.team, m.w_axis.truncate(), (-f.x).atan2(f.y))
            })
            .collect();
        let team = teams.team.clone();
        self.mode = Some(mode::Match::escort(teams, config, spawns));
        self.place_teams(&team);
    }

    /// Escort: puts every player on its team's spawn group (the k-th
    /// player of team `t` on the k-th spawn of TeamIndex `t + 1`; a team
    /// larger than its group shares the spawns round robin).
    fn place_teams(&mut self, team: &[rules::Team]) {
        let mut placed = Vec::new();
        for (p, &t) in team.iter().enumerate().take(self.players.len()) {
            let k = team[..p].iter().filter(|&&o| o == t).count();
            let group: Vec<(Vec3, f32)> = self.team_spawns.iter().filter(|s| s.0 == t as u32 + 1).map(|s| (s.1, s.2)).collect();
            if let Some(&(at, yaw)) = group.get(k % group.len().max(1)) {
                placed.push((p, at, yaw));
            }
        }
        for (p, at, yaw) in placed {
            self.place_player(p, at, yaw);
        }
    }

    /// VIP ways with `team` hunting (after [`World::escort`]).
    pub fn ways_of(&self, team: rules::Team) -> Option<Arc<vip::WaySet>> {
        self.vip_ways.iter().find(|(t, _)| *t == team).map(|(_, w)| w.clone())
    }

    /// Escort: steps the VIP cycle after the match tick: kills of this
    /// tick, timers, (re)spawns, walking; keeps [`rules::Teams::vips`] to
    /// the targetable ones.
    fn step_vips(&mut self, kills: &[(usize, usize)]) {
        let Some(m) = &mut self.mode else { return };
        // intro / recap: VIPs neither spawn nor walk (play starts a fresh cycle)
        let playing = m.playing();
        let mode::Mode::Escort(teams) = &mut m.mode else { return };
        let Some(v) = &mut m.vips else { return };
        if !playing {
            v.events.clear();
            teams.vips.clear();
            return;
        }
        let (Some(crowd), Some(ways)) = (&self.crowd, self.vip_ways.iter().find(|(t, _)| *t == teams.attacking_team).map(|(_, w)| w.clone())) else { return };
        v.events.clear();
        for &(killer, body) in kills {
            v.killed(body, killer);
        }
        let hunters: Vec<Vec3> = self.players.iter().enumerate().filter(|(i, _)| teams.team.get(*i).is_some_and(|&t| t == teams.attacking_team)).map(|(_, p)| p.pos).collect();
        for s in v.tick(&ways, &hunters) {
            let b = &mut self.bodies[v.bodies[s]];
            b.scripted = None;
            b.agent = None;
        }
        v.walk(&crowd.nav, &ways, crowd.pace(), crowd.walk.length, DT);
        teams.vips = v.targets();
        if trace() {
            for e in &v.events {
                eprintln!("vip: t {:.3} {e:?}", self.time);
            }
            if self.tick % 60 == 0 {
                for (s, slot) in v.slots.iter().enumerate() {
                    if let Some(x) = slot {
                        let p = x.walker.pos;
                        eprintln!("vip {s} body {} way {}{} at {:.2} {:.2} {:.2} next {} checkpoints {}", v.bodies[s], x.way, if x.back { " back" } else { "" }, p.x, p.y, p.z, x.walker.next, x.checkpoints);
                    }
                }
            }
        }
        let events = v.events.clone();
        m.on_vip_events(&events);
        if trace() {
            for e in m.events.iter().filter(|e| matches!(e, mode::MatchEvent::TeamScore { .. } | mode::MatchEvent::Round(_))) {
                eprintln!("escort: t {:.3} {e:?} teams {:?}", self.time, m.rounds.as_ref().map(|r| &r.team_score));
            }
        }
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
        // the match: who may kill / stun whom this tick, who is in control
        let n_bodies = self.bodies.len();
        let n_players = self.players.len();
        let (act_rules, relations, in_control) = match &mut self.mode {
            Some(m) => {
                m.events.clear();
                let rel: Vec<Vec<rules::Relation>> = (0..self.players.len()).map(|i| (0..n_bodies).map(|j| m.relation(i, j)).collect()).collect();
                let ctl: Vec<bool> = (0..self.players.len()).map(|i| m.alive(i) && !m.over).collect();
                (m.rules(), Some(rel), ctl)
            }
            None => (rules::Rules::default(), None, vec![true; self.players.len()]),
        };
        let mut actions = Vec::new();
        // Escort: VIPs hunters may kill this tick, and who killed which
        let vip_targets: Vec<usize> = match self.mode.as_ref().map(|m| &m.mode) {
            Some(mode::Mode::Escort(t)) => t.vips.clone(),
            _ => Vec::new(),
        };
        let mut vip_kills = Vec::new();
        // crowd NPCs killed this tick, by player
        let mut npc_kills = Vec::new();
        let stances: Vec<rules::Stance> = self.players.iter().map(|p| p.actor().stance).collect();
        // ground move cycle time scale: chase boost, Sprint Boost, knives
        let body_pos: Vec<Vec3> = self.bodies.iter().map(|b| b.entity.w_axis.truncate()).collect();
        for (i, p) in self.players.iter_mut().enumerate() {
            p.time_scale = match &self.mode {
                Some(m) if in_control[i] => {
                    let x = inputs.get(i).copied().unwrap_or_default();
                    let moving = x.forward || x.back || x.left || x.right;
                    let facing = glam::Vec2::new(-p.yaw.sin(), p.yaw.cos());
                    m.time_scale(i, x.run || x.sprint, moving, p.pos, facing, &body_pos)
                }
                _ => 1.0,
            };
        }
        for (i, p) in self.players.iter_mut().enumerate() {
            let input = if in_control[i] { inputs.get(i).copied().unwrap_or_default() } else { PlayerInput { cam_yaw: inputs.get(i).map_or(0.0, |x| x.cam_yaw), ..Default::default() } };
            let relation = |j: usize| relations.as_ref().map_or(rules::Relation::Npc, |r| r[i][j]);
            let rewind = rules::rewind(input.view_lag, self.seen.len());
            let past = self.seen.len().checked_sub(rewind).filter(|_| rewind > 0).and_then(|k| self.seen.get(k));
            let seen = |j: usize, b: &Body| player::seen_actor(b, past.and_then(|h| h.get(j)), stances.get(j).copied());
            let was_scripted: Vec<bool> = if relations.is_some() { self.bodies.iter().map(|b| b.scripted.is_some()).collect() } else { Vec::new() };
            let was_attacking = p.kill.is_some();
            // ACBMP_TRACE=pose: per-tick pose change of the player (summed
            // bone rotation, radians), to spot clips that snap or reset
            let before = trace_pose().then(|| p.loco.last_pose().clone());
            p.landing = None;
            player::drive(p, &mut self.bodies, &self.clips, self.ground.as_ref(), &input, &act_rules, &relation, &seen, t, DT);
            if relations.is_some() && p.kill.is_some() && !was_attacking {
                if let Some(v) = (0..n_players).find(|&j| j != p.body && !was_scripted[j] && self.bodies[j].scripted.is_some()) {
                    actions.push((i, v, p.kill.as_ref().map_or(kill::WALK, |k| k.set())));
                } else if let Some(v) = vip_targets.iter().copied().find(|&j| !was_scripted[j] && self.bodies[j].scripted.is_some()) {
                    vip_kills.push((i, v));
                } else if (n_players..n_bodies).any(|j| !vip_targets.contains(&j) && !was_scripted[j] && self.bodies[j].scripted.is_some()) {
                    npc_kills.push(i);
                }
            }
            if let Some(before) = before {
                let jump: f32 = before.iter().zip(p.loco.last_pose()).map(|((_, a), (_, b))| a.angle_between(*b)).sum();
                eprintln!("pose t {t:.3} speed {:.2} change {jump:.2}{}", p.loco.speed, if p.loco.on_beam() { " beam" } else { "" });
            }
            p.moved = (p.pos - p.prev_pos).truncate().length() / DT;
            // where others see it (crowd separation, targeting)
            if let Some(b) = self.bodies.get_mut(p.body) {
                b.entity = p.entity();
            }
        }
        if let Some(p) = self.players.first() {
            trace_csv_row(self.tick, t, p);
        }
        // blending: walking among a crowd group (NPCs, persona lookalikes)
        let npcs: Vec<rules::Member> = self
            .bodies
            .iter()
            .skip(n_players)
            .filter(|b| b.scripted.is_none())
            .map(|b| {
                let f = b.entity.y_axis;
                rules::Member { pos: b.entity.w_axis.truncate(), yaw: (-f.x).atan2(f.y) }
            })
            .collect();
        let polygons = if npcs.len() >= rules::BLEND_MIN_GROUP { rules::blend_polygons(&npcs) } else { Vec::new() };
        for p in &mut self.players {
            // on a bench: blended (retail benches are blend spots)
            p.blended = match &p.hide {
                Some(h) => h.seated(),
                None => rules::blended(&p.actor(), &polygons),
            };
        }
        if let Some(m) = &mut self.mode {
            for (k, v, set) in actions {
                m.on_action(k, v, set);
            }
            for k in npc_kills {
                m.on_npc_kill(k);
            }
            for (i, p) in self.players.iter().enumerate() {
                if let Some((_, damage, _)) = p.landing {
                    m.on_fall(i, damage);
                }
            }
            let pos: Vec<Vec3> = self.players.iter().map(|p| p.pos).collect();
            let who: Vec<mode::Presence> = self
                .players
                .iter()
                .enumerate()
                .map(|(i, p)| mode::Presence { pos: p.pos, hidden: p.hidden, blended: p.blended, running: p.moved >= rules::RUN_SPEED && p.kill.is_none(), yaw: p.yaw, speed: p.moved, in_air: p.loco.airborne() || p.flight.is_some(), look: inputs.get(i).map_or(0.0, |x| x.cam_yaw), lock: inputs.get(i).is_some_and(|x| x.lock), release: inputs.get(i).is_some_and(|x| x.release), conflict: p.kill.is_some() })
                .collect();
            let ground = self.ground.as_ref();
            // head to head (retail GameBrick::CanSee); no collision: clear
            let los = |a: usize, b: usize| {
                let (o, t) = (pos[a] + Vec3::Z * rules::TARGET_HEIGHT, pos[b] + Vec3::Z * rules::TARGET_HEIGHT);
                ground.is_none_or(|g| g.ray(o, t - o).is_none())
            };
            // Assassinate acquisition (ValidateTargetForCameraWithNavAndRaycast
            // 0x0130d596): any of three rays to the target's body is clear
            let sight = |a: usize, b: usize| {
                let o = pos[a] + Vec3::Z * rules::TARGET_HEIGHT;
                rules::LOOK_RAY_HEIGHTS.iter().any(|&z| {
                    let t = pos[b] + Vec3::Z * z.max(0.1);
                    ground.is_none_or(|g| g.ray(o, t - o).is_none())
                })
            };
            let back: Vec<(usize, Vec3, f32)> =
                m.tick_with(&who, &los, &sight).into_iter().map(|(p, s)| { let (at, yaw) = m.spawns.get(s).copied().unwrap_or((pos[p], 0.0)); (p, at, yaw) }).collect();
            // Escort: every round's play starts on the team spawns
            let play_start = m.events.iter().any(|e| matches!(e, mode::MatchEvent::Round(rounds::RoundEvent::Phase { phase: rounds::Phase::Play, .. })));
            let teams = match &m.mode {
                mode::Mode::Escort(teams) if play_start => Some(teams.team.clone()),
                _ => None,
            };
            for (p, at, yaw) in back {
                respawn(&mut self.players[p], &mut self.bodies);
                self.place_player(p, at, yaw);
            }
            if let Some(team) = teams {
                self.place_teams(&team);
            }
        }
        self.step_vips(&vip_kills);
        self.seen.push_back(self.players.iter().map(|p| p.actor()).collect());
        while self.seen.len() > rules::HISTORY {
            self.seen.pop_front();
        }
        let wander = self.wander;
        let skip = self.players.len();
        // everyone's position, for the crowd's separation (2 m buckets)
        let mut near: std::collections::HashMap<(i32, i32), Vec<Vec3>> = Default::default();
        let bucket = |p: Vec3| ((p.x / 2.0).floor() as i32, (p.y / 2.0).floor() as i32);
        for p in self.bodies.iter().map(|a| a.entity.w_axis.truncate()) {
            near.entry(bucket(p)).or_default().push(p);
        }
        let vips = self.mode.as_ref().and_then(|m| m.vips.as_ref());
        for (i, a) in self.bodies.iter_mut().enumerate().skip(skip) {
            let c = a.character.clone();
            // a living VIP walks its way (a dead one plays its kill clip)
            if let (Some(v), Some(crowd)) = (vips, &self.crowd) {
                if let Some(x) = v.slot_of(i).and_then(|s| v.slots[s].as_ref()) {
                    a.entity = Mat4::from_rotation_translation(Quat::from_rotation_z(x.walker.yaw), x.walker.pos);
                    let pose = crowd.walk.sample(&c.skeleton, x.phase * crowd.walk.length);
                    let world = a.entity * a.model;
                    a.show(world, pose);
                    continue;
                }
            }
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

/// Puts player `p` back in control: its victim clip and actions dropped.
fn respawn(p: &mut player::Player, bodies: &mut [Body]) {
    let b = &mut bodies[p.body];
    b.scripted = None;
    p.kill = None;
    p.approach = None;
    p.climb = None;
    p.flight = None;
    p.hide = None;
    p.hidden = false;
    p.vel = Vec3::ZERO;
    p.loco.resume(b.pose.clone());
}
