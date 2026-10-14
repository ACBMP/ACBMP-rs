//! Fly-through viewer for ACB maps.
//!
//! WASD move, Space/C up/down, hold right mouse to look, Shift for speed,
//! mouse wheel changes the base speed, Escape quits. `--character` puts
//! MP personas on the map's standard player spawns, playing `--anim`.
//! `--play` hands the first one to the keyboard: WASD walk relative to the
//! camera, Shift runs, Space sprints, right mouse orbits, the wheel zooms.

mod climb;
mod crowd;
mod ground;
mod kill;
mod locomotion;

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use acb_anim::character::Character;
use acb_anim::cloth::ClothSim;
use acb_anim::pose::Clip;
use acb_anim::BuiltinTracks;
use acb_render::{wgpu, Camera, Renderer, Scene, DEPTH_FORMAT};
use clap::Parser;
use glam::{Mat4, Quat, Vec2, Vec3};
use climb::{Climb, ClimbClips, Dir, Outcome};
use crowd::{Agent, Nav};
use ground::Ground;
use kill::KillClips;
use locomotion::{Clips, Gait, Locomotion};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

#[derive(Parser)]
#[command(about = "Fly through an ACB map")]
struct Args {
    /// Map forge, e.g. multi/DataPC_AC2MP_SanDonato.forge
    map: PathBuf,
    /// Directory holding ACB_MP.schema and ACR_MP.schema
    #[arg(long, env = "ACBMP_SCHEMAS")]
    schemas: PathBuf,
    /// EntityBuilder or Entity to place on the player spawns, round-robin
    /// (repeatable), e.g. AC2MP_ID07_MainTemplate_Player
    #[arg(long = "character")]
    characters: Vec<String>,
    /// How many spawns get a character (default: one per --character)
    #[arg(long)]
    count: Option<usize>,
    /// Animation the characters loop
    #[arg(long, default_value = "xx_l_walk_hipm_footl")]
    anim: String,
    /// Render one frame to this PNG after --capture-after seconds and quit
    #[arg(long)]
    capture: Option<PathBuf>,
    #[arg(long, default_value_t = 3.0)]
    capture_after: f32,
    /// ACBMP.exe, for the engine's builtin animation tracks (root motion);
    /// default: the one next to the map's `multi` directory
    #[arg(long, env = "ACBMP_EXE")]
    exe: Option<PathBuf>,
    /// Characters follow root motion and return to their spawn after this
    /// many seconds (no collision yet); 0 animates in place
    #[arg(long, default_value_t = 6.0)]
    wander: f32,
    /// Drive the first character (third-person, needs --character)
    #[arg(long)]
    play: bool,
    /// Keys held from the start, for --capture runs (w, a, s, d, shift,
    /// space)
    #[arg(long, value_delimiter = ',')]
    hold: Vec<String>,
    /// Add this many NPCs wandering the navmesh (round-robin over
    /// --character)
    #[arg(long, default_value_t = 0)]
    crowd: usize,
    /// Debug paired animations: the first two characters share the first
    /// spawn's transform and play these two clips (assassin,victim)
    #[arg(long, value_delimiter = ',')]
    pair: Vec<String>,
    /// Start the --play character here (world x,y,z) instead of its spawn
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    at: Option<Vec<f32>>,
    /// Scripted keys over simulated time, e.g. "0=w,shift 2= 2.5=d": from
    /// each time on exactly these keys are held (replaces --hold)
    #[arg(long)]
    input: Option<String>,
}

fn parse_keys(list: &str) -> anyhow::Result<HashSet<KeyCode>> {
    list.split(',')
        .filter(|k| !k.is_empty())
        .map(|k| {
            Ok(match k.to_ascii_lowercase().as_str() {
                "w" => KeyCode::KeyW,
                "a" => KeyCode::KeyA,
                "s" => KeyCode::KeyS,
                "d" => KeyCode::KeyD,
                "shift" => KeyCode::ShiftLeft,
                "space" => KeyCode::Space,
                "e" => KeyCode::KeyE,
                other => anyhow::bail!("unknown key {other}"),
            })
        })
        .collect()
}

/// `--input`: (time, keys) in time order.
fn parse_input(script: &str) -> anyhow::Result<Vec<(f32, HashSet<KeyCode>)>> {
    let mut out = Vec::new();
    for item in script.split_whitespace() {
        let (t, keys) = item.split_once('=').ok_or_else(|| anyhow::anyhow!("--input: {item}: want time=keys"))?;
        out.push((t.parse::<f32>()?, parse_keys(keys)?));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(out)
}

/// Keyboard-driven character (actor 0) and its follow camera.
struct Player {
    pos: Vec3,
    /// Facing: entity +Y is (-sin yaw, cos yaw).
    yaw: f32,
    loco: Locomotion,
    /// Velocity while airborne, m/s.
    vel: Vec3,
    /// Height the current fall started at.
    fall_from: f32,
    kill: Option<PlayerKill>,
    climb: Option<Climb>,
    cam_yaw: f32,
    cam_pitch: f32,
    cam_dist: f32,
}

impl Player {
    fn entity(&self) -> Mat4 {
        Mat4::from_rotation_translation(Quat::from_rotation_z(self.yaw), self.pos)
    }

    fn look(&self) -> Vec3 {
        Vec3::new(self.cam_yaw.cos() * self.cam_pitch.cos(), self.cam_yaw.sin() * self.cam_pitch.cos(), self.cam_pitch.sin())
    }

    fn camera_eye_target(&self, ground: Option<&Ground>) -> (Vec3, Vec3) {
        let target = self.pos + Vec3::Z * 1.6;
        let mut dist = self.cam_dist;
        // pull in rather than look through a wall behind the player
        let back = -self.look();
        // pull in in front of whatever is between the player and the eye
        // (keeping 0.25 m off it)
        // (keeping 0.25 m off it; geometry hugging the player is ignored)
        const SKIP: f32 = 0.3;
        if let Some(f) = ground.and_then(|g| g.ray(target + back * SKIP, back * (dist + 0.25 - SKIP))) {
            dist = (SKIP + (dist + 0.25 - SKIP) * f - 0.25).max(0.6);
        }
        (target + back * dist, target)
    }
}

/// A character placed in the scene.
struct Actor {
    character: Rc<Character>,
    handles: Vec<Option<usize>>,
    spawn: Mat4,
    /// Entity transform, advanced by root motion.
    entity: Mat4,
    /// Skeleton model space -> entity space.
    model: Mat4,
    phase: f32,
    /// Clip time of the previous frame.
    clip_time: f32,
    epoch: u32,
    /// One simulation per `character.cloths`.
    cloth: Vec<ClothSim>,
    /// Navmesh wanderer (--crowd) instead of the looping clip.
    agent: Option<Agent>,
    /// A clip played once in a given frame and held at its end (--pair,
    /// kill victims) instead of everything else.
    scripted: Option<Scripted>,
}

struct Scripted {
    clip: Clip,
    frame: Mat4,
    /// Simulated time it started at.
    start: f32,
}

/// The player's kill in progress.
struct PlayerKill {
    side: usize,
    frame: Mat4,
    t: f32,
    /// Pose and placement it blends in from.
    from_pose: Vec<(Vec3, Quat)>,
    from_world: Mat4,
}

/// Blend-in time of a kill from wherever the player was.
const KILL_BLEND: f32 = 0.25;

/// --crowd: the navmesh and the clips wanderers blend.
struct Crowd {
    nav: Nav,
    idle: Clip,
    walk: Clip,
}

/// Skins every part of an actor at `world` and steps its cloth by `dt`:
/// cloth parts are posed on the CPU and drawn with identity skinning.
fn pose_actor(scene: &mut Scene, r: &Renderer, a: &mut Actor, globals: &[Mat4], world: Mat4, dt: f32) {
    let c = &a.character;
    let mut skins = c.skin_matrices(globals);
    let to_model = world.inverse();
    for (ci, sim) in a.cloth.iter_mut().enumerate() {
        let Some(part) = c.cloths[ci].part else { continue };
        let target: Vec<Vec3> = c.cloth_targets(ci, globals).into_iter().map(|p| world.transform_point3(p)).collect();
        let capsules: Vec<(Vec3, Vec3, f32)> = c
            .cloth_capsules(ci, globals)
            .into_iter()
            .map(|(a, b, r)| (world.transform_point3(a), world.transform_point3(b), r))
            .collect();
        sim.step(&c.cloths[ci].lod, &target, &capsules, dt);
        let local: Vec<Vec3> = sim.pos.iter().map(|p| to_model.transform_point3(*p)).collect();
        if let (Some(raw), Some(h)) = (c.cloth_vertices(ci, &local, &skins[part]), a.handles[part]) {
            scene.set_skinned_vertices(r, h, &raw);
            skins[part].iter_mut().for_each(|m| *m = Mat4::IDENTITY);
        }
    }
    for (h, skin) in a.handles.iter().zip(skins) {
        if let Some(h) = h {
            scene.set_skinned_pose(*h, world, &skin);
        }
    }
}

struct Fly {
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    speed: f32,
}

impl Fly {
    fn forward(&self) -> Vec3 {
        Vec3::new(self.yaw.cos() * self.pitch.cos(), self.yaw.sin() * self.pitch.cos(), self.pitch.sin())
    }
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    renderer: Renderer,
    scene: Scene,
    actors: Vec<Actor>,
    clip: Option<Clip>,
    loco: Option<Clips>,
    ground: Option<Ground>,
    crowd: Option<Crowd>,
    kills: Option<KillClips>,
    climbs: Option<ClimbClips>,
}

struct App {
    args: Args,
    gfx: Option<Gfx>,
    fly: Fly,
    player: Option<Player>,
    keys: HashSet<KeyCode>,
    looking: bool,
    last: Instant,
    frames: u32,
    fps_time: Instant,
    start: Instant,
    /// `--input` events not yet applied.
    script: Vec<(f32, HashSet<KeyCode>)>,
    /// Kill and climb clips between loading and `Gfx`.
    kills: Option<KillClips>,
    climbs: Option<ClimbClips>,
    /// Simulated seconds since start (fixed 60 Hz steps in --capture runs).
    sim_time: f32,
}

fn depth_view(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

impl App {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        let window = Arc::new(event_loop.create_window(
            Window::default_attributes()
                .with_title(format!("acb-viewer: {}", self.args.map.display()))
                .with_inner_size(winit::dpi::LogicalSize::new(1600, 900)),
        )?);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let (device, queue) = pollster::block_on(acb_render::request_device(&adapter))?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        let depth = depth_view(&device, config.width, config.height);
        let renderer = Renderer::new(device, queue, format);

        let t0 = Instant::now();
        let schema = Box::leak(Box::new(acb_world::load_schema(&self.args.schemas)?));
        let archive = acb_world::open_map(&self.args.map, schema)?;
        let (mut scene, s) = Scene::load(&renderer, &archive, acb_render::ShadingMode::Retail)?;
        eprintln!(
            "{} meshes, {} instances, {} triangles, {} textures, loaded in {:.1}s",
            s.meshes, s.instances, s.triangles, s.textures, t0.elapsed().as_secs_f64()
        );
        let (actors, clip, loco, crowd) = self.place_characters(&renderer, &archive, &mut scene)?;
        let ground = if loco.is_some() {
            let t = Instant::now();
            let g = Ground::load(&archive)?;
            eprintln!("ground: {} triangles in {:.1}s", g.triangles(), t.elapsed().as_secs_f64());
            Some(g)
        } else {
            None
        };
        if let (Some(p), Some(g)) = (&mut self.player, &ground) {
            if let Some(z) = g.floor_below(p.pos + Vec3::Z * 1.0) {
                p.pos.z = z;
            }
            if std::env::var_os("ACBMP_TRACE").is_some() {
                for (spot, face) in g.mount_spots(p.pos, 5) {
                    eprintln!("mount spot {:.2} {:.2} {:.2} facing {:.2} {:.2}", spot.x, spot.y, spot.z, face.x, face.y);
                }
            }
        }
        let kills = self.kills.take();
        let climbs = self.climbs.take();
        self.gfx = Some(Gfx { window, surface, config, depth, renderer, scene, actors, clip, loco, ground, crowd, kills, climbs });
        self.start = Instant::now();
        self.sim_time = 0.0;
        Ok(())
    }

    fn place_characters(
        &mut self,
        r: &Renderer,
        archive: &acb_world::Archive,
        scene: &mut Scene,
    ) -> anyhow::Result<(Vec<Actor>, Option<Clip>, Option<Clips>, Option<Crowd>)> {
        if self.args.characters.is_empty() {
            return Ok((Vec::new(), None, None, None));
        }
        let by_name = |ty: &str, name: &str| {
            archive.uids_of(ty).into_iter().find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(name)))
        };
        let mut characters = Vec::new();
        for name in &self.args.characters {
            let c = match by_name("EntityBuilder", name) {
                Some(b) => Character::load_built(archive, b),
                None => by_name("Entity", name).and_then(|e| Character::load(archive, e)),
            };
            characters.push(Rc::new(c.ok_or_else(|| anyhow::anyhow!("{name}: no such character"))?));
        }
        let exe = self.args.exe.clone().or_else(|| {
            let p = self.args.map.parent()?.parent()?.join("ACBMP.exe");
            p.exists().then_some(p)
        });
        let builtins = exe.as_ref().and_then(|p| BuiltinTracks::from_executable(&std::fs::read(p).ok()?));
        if builtins.is_none() {
            eprintln!("no ACBMP.exe builtin tracks (--exe): no root motion");
        }
        let clip = by_name("Animation", &self.args.anim).and_then(|a| Clip::load_with(archive, a, builtins.as_ref()));
        if clip.is_none() {
            eprintln!("{}: animation not found, bind pose", self.args.anim);
        }
        let loco = if self.args.play {
            Some(Clips::load(archive, builtins.as_ref())?)
        } else {
            None
        };
        self.kills = if self.args.play { KillClips::load(archive, builtins.as_ref(), &self.args.characters[0]) } else { None };
        self.climbs = if self.args.play { Some(ClimbClips::load(archive, builtins.as_ref())?) } else { None };
        if self.args.play && self.kills.is_none() {
            eprintln!("{}: no ground kill clips", self.args.characters[0]);
        }
        let spawns: Vec<_> = archive.player_spawns().into_iter().filter(|s| s.kind == 0).collect();
        let count = self.args.count.unwrap_or(characters.len()).min(spawns.len());
        let mut actors = Vec::new();
        for (i, spawn) in spawns.iter().take(count).enumerate() {
            let character = characters[i % characters.len()].clone();
            let handles = character.parts.iter().map(|p| scene.add_skinned(r, archive, &p.mesh, &p.materials)).collect();
            let spawn = Mat4::from_cols_array(&spawn.matrix);
            // clips that animate Reference place the pelvis in entity space
            let root_bone = character.skeleton.bones.first().map(|b| b.id);
            let clip_i = if i == 0 && loco.is_some() { loco.as_ref().map(Clips::idle) } else { clip.as_ref() };
            let model = match (clip_i, root_bone) {
                (Some(clip), Some(id)) if clip.drives(id) => Mat4::IDENTITY,
                _ => character.model_to_entity(),
            };
            let phase = i as f32 * 0.37;
            let cloth = vec![ClothSim::default(); character.cloths.len()];
            actors.push(Actor { character, handles, spawn, entity: spawn, model, phase, clip_time: phase, epoch: 0, cloth, agent: None, scripted: None });
        }
        eprintln!("{} characters on {} standard spawns", actors.len(), spawns.len());
        if std::env::var_os("ACBMP_TRACE").is_some() {
            for (i, a) in actors.iter().enumerate() {
                let p = a.spawn.w_axis;
                let f = a.spawn.y_axis;
                eprintln!("actor {i} at {:.2} {:.2} {:.2} facing {:.2} {:.2}", p.x, p.y, p.z, f.x, f.y);
            }
        }
        if let [ass, victim] = self.args.pair.as_slice() {
            for (i, name) in [ass, victim].into_iter().enumerate() {
                let clip = by_name("Animation", name)
                    .and_then(|a| Clip::load_with(archive, a, builtins.as_ref()))
                    .ok_or_else(|| anyhow::anyhow!("{name}: animation not found"))?;
                let shared = actors[0].spawn;
                if let Some(a) = actors.get_mut(i) {
                    (a.spawn, a.entity, a.model) = (shared, shared, Mat4::IDENTITY);
                    a.scripted = Some(Scripted { clip, frame: shared, start: 0.0 });
                }
            }
        }
        let crowd = if self.args.crowd > 0 {
            let load = |name: &str| {
                by_name("Animation", name)
                    .and_then(|a| Clip::load_with(archive, a, builtins.as_ref()))
                    .ok_or_else(|| anyhow::anyhow!("{name}: animation not found"))
            };
            let nav = Nav::load(archive).ok_or_else(|| anyhow::anyhow!("--crowd: the map has no navmesh"))?;
            let crowd = Crowd { nav, idle: load("xx_l_wait_hipm_footl")?, walk: load("xx_l_walk_hipm_footl")? };
            for i in 0..self.args.crowd {
                let character = characters[(count + i) % characters.len()].clone();
                let handles = character.parts.iter().map(|p| scene.add_skinned(r, archive, &p.mesh, &p.materials)).collect();
                let root_bone = character.skeleton.bones.first().map(|b| b.id);
                let model = match root_bone {
                    Some(id) if crowd.walk.drives(id) => Mat4::IDENTITY,
                    _ => character.model_to_entity(),
                };
                let agent = Agent::new(&crowd.nav, i as u64 + 1);
                let entity = agent.entity();
                let cloth = vec![ClothSim::default(); character.cloths.len()];
                actors.push(Actor { character, handles, spawn: entity, entity, model, phase: 0.0, clip_time: 0.0, epoch: 0, cloth, agent: Some(agent), scripted: None });
            }
            eprintln!(
                "crowd: {} NPCs, {} lanes, {} navmesh triangles",
                self.args.crowd,
                crowd.nav.flows(),
                crowd.nav.triangles()
            );
            Some(crowd)
        } else {
            None
        };
        // start behind the first one, looking along its facing (+Y)
        if let Some(spawn) = spawns.first().filter(|_| !actors.is_empty()) {
            let m = Mat4::from_cols_array(&spawn.matrix);
            let facing = m.y_axis.truncate().normalize_or_zero();
            let at = m.w_axis.truncate();
            self.fly.pos = at - facing * 4.0 + Vec3::Z * 1.8;
            self.fly.yaw = facing.y.atan2(facing.x);
            self.fly.pitch = -0.15;
            self.fly.speed = 4.0;
            if let (Some(clips), Some(actor)) = (&loco, actors.first()) {
                let yaw = (-facing.x).atan2(facing.y);
                let at = match self.args.at.as_deref() {
                    Some([x, y, z]) => Vec3::new(*x, *y, *z),
                    _ => at,
                };
                self.player = Some(Player {
                    pos: at,
                    yaw,
                    loco: Locomotion::new(clips, &actor.character.skeleton),
                    vel: Vec3::ZERO,
                    fall_from: at.z,
                    kill: None,
                    climb: None,
                    cam_yaw: self.fly.yaw,
                    cam_pitch: -0.25,
                    cam_dist: 3.5,
                });
            }
        }
        Ok((actors, clip, loco, crowd))
    }

    fn animate(&mut self, dt: f32) {
        self.sim_time += dt;
        while self.script.first().is_some_and(|e| e.0 <= self.sim_time) {
            self.keys = self.script.remove(0).1;
        }
        self.drive_player(dt);
        let Some(g) = &mut self.gfx else { return };
        let t = self.sim_time;
        let wander = self.args.wander;
        let skip = usize::from(self.player.is_some());
        // everyone's position, for the crowd's separation (2 m buckets)
        let mut near: std::collections::HashMap<(i32, i32), Vec<Vec3>> = Default::default();
        let bucket = |p: Vec3| ((p.x / 2.0).floor() as i32, (p.y / 2.0).floor() as i32);
        for p in g.actors.iter().map(|a| a.entity.w_axis.truncate()).chain(self.player.as_ref().map(|p| p.pos)) {
            near.entry(bucket(p)).or_default().push(p);
        }
        for a in g.actors.iter_mut().skip(skip) {
            let c = &a.character;
            if let (Some(agent), Some(crowd)) = (&mut a.agent, &g.crowd) {
                let (bx, by) = bucket(agent.pos);
                let others: Vec<Vec3> =
                    (-1..=1).flat_map(|x| (-1..=1).map(move |y| (bx + x, by + y))).filter_map(|k| near.get(&k)).flatten().copied().collect();
                let pose = agent.update(&crowd.nav, &crowd.idle, &crowd.walk, &c.skeleton, &others, dt);
                a.entity = agent.entity();
                let globals = c.skeleton.globals(&pose);
                let world = a.entity * a.model;
                pose_actor(&mut g.scene, &g.renderer, a, &globals, world, dt);
                continue;
            }
            if let Some(sc) = &a.scripted {
                let ct = (t - sc.start).clamp(0.0, sc.clip.length);
                let pose = sc.clip.sample(&c.skeleton, ct);
                let globals = c.skeleton.globals(&pose);
                let world = sc.frame * sc.clip.root_motion(ct);
                pose_actor(&mut g.scene, &g.renderer, a, &globals, world, dt);
                continue;
            }
            let local = match &g.clip {
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
            let globals = c.skeleton.globals(&local);
            let world = a.entity * a.model;
            pose_actor(&mut g.scene, &g.renderer, a, &globals, world, dt);
        }
    }

    /// Player locomotion: input -> gait and facing, clip-driven pose and
    /// root motion with wall sliding and floor snapping.
    fn drive_player(&mut self, dt: f32) {
        let (Some(p), Some(g)) = (&mut self.player, &mut self.gfx) else { return };
        let Some(clips) = &g.loco else { return };
        if g.actors.is_empty() {
            return;
        }
        let k = |c| self.keys.contains(&c);
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
                        v.scripted = Some(Scripted { clip: kills.sets[side].1.clone(), frame, start: self.sim_time });
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
                    p.pos = to;
                }
            }
        } else {
            p.pos += delta.extend(0.0);
        }

        // ACBMP_TRACE=1: player state four times a second (simulated time)
        if std::env::var_os("ACBMP_TRACE").is_some() && (self.sim_time * 4.0).floor() != ((self.sim_time - dt) * 4.0).floor() {
            eprintln!(
                "t {:.2} pos {:.2} {:.2} {:.2} yaw {:.0} speed {:.2}{}",
                self.sim_time,
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

    fn update(&mut self, dt: f32) {
        if self.player.is_some() {
            return;
        }
        let k = |c| self.keys.contains(&c);
        let fwd = self.fly.forward();
        let right = fwd.cross(Vec3::Z).normalize_or_zero();
        let mut dir = Vec3::ZERO;
        if k(KeyCode::KeyW) { dir += fwd; }
        if k(KeyCode::KeyS) { dir -= fwd; }
        if k(KeyCode::KeyD) { dir += right; }
        if k(KeyCode::KeyA) { dir -= right; }
        if k(KeyCode::Space) { dir += Vec3::Z; }
        if k(KeyCode::KeyC) { dir -= Vec3::Z; }
        let boost = if k(KeyCode::ShiftLeft) { 5.0 } else { 1.0 };
        self.fly.pos += dir.normalize_or_zero() * self.fly.speed * boost * dt;
    }

    fn camera(&self, g: &Gfx) -> Camera {
        let (eye, target) = match &self.player {
            Some(p) => p.camera_eye_target(g.ground.as_ref()),
            None => (self.fly.pos, self.fly.pos + self.fly.forward()),
        };
        Camera { eye, target, fov_y: 60f32.to_radians(), aspect: g.config.width as f32 / g.config.height as f32 }
    }

    /// --capture: steps the simulation at a fixed 60 Hz without presenting
    /// (a hidden window may never get a frame), then writes one frame.
    fn capture_step(&mut self, path: &std::path::Path) {
        const DT: f32 = 1.0 / 60.0;
        self.update(DT);
        self.animate(DT);
        if self.sim_time < self.args.capture_after {
            return;
        }
        let Some(g) = &self.gfx else { return };
        let camera = self.camera(g);
        let rgba = acb_render::render_to_rgba(&g.renderer, &g.scene, &camera, g.config.width, g.config.height);
        match write_png(path, g.config.width, g.config.height, &rgba) {
            Ok(()) => eprintln!("wrote {}", path.display()),
            Err(e) => eprintln!("capture failed: {e:#}"),
        }
        eprintln!("camera eye {:.2?} target {:.2?}", camera.eye.to_array(), camera.target.to_array());
        if let Some(p) = &self.player {
            eprintln!("player at {:.2} {:.2} {:.2} yaw {:.0} speed {:.2}", p.pos.x, p.pos.y, p.pos.z, p.yaw.to_degrees(), p.loco.speed);
        }
        std::process::exit(0);
    }

    fn draw(&mut self) {
        let Some(g) = &self.gfx else { return };
        let camera = self.camera(g);
        let Some(g) = &mut self.gfx else { return };
        let frame = match g.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                g.surface.configure(g.renderer.device(), &g.config);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = g.renderer.device().create_command_encoder(&Default::default());
        g.renderer.render(&mut enc, &view, &g.depth, &g.scene, &camera);
        g.renderer.queue().submit([enc.finish()]);
        g.renderer.queue().present(frame);

        self.frames += 1;
        let t = self.fps_time.elapsed().as_secs_f32();
        if t >= 1.0 {
            let pos = self.player.as_ref().map_or(self.fly.pos, |p| p.pos);
            let mut status = format!(
                "acb-viewer: {:.0} fps  pos {:.1} {:.1} {:.1}",
                self.frames as f32 / t,
                pos.x,
                pos.y,
                pos.z
            );
            if let Some(p) = &self.player {
                status += &format!("  yaw {:.0}  speed {:.2}", p.yaw.to_degrees(), p.loco.speed);
            }
            g.window.set_title(&status);
            if std::env::var_os("ACB_VIEWER_LOG").is_some() {
                eprintln!("{status}");
            }
            self.frames = 0;
            self.fps_time = Instant::now();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_none() {
            if let Err(e) = self.init(event_loop) {
                eprintln!("init failed: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gfx {
                    g.config.width = size.width.max(1);
                    g.config.height = size.height.max(1);
                    g.surface.configure(g.renderer.device(), &g.config);
                    g.depth = depth_view(g.renderer.device(), g.config.width, g.config.height);
                }
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape {
                        event_loop.exit();
                    }
                    if event.state == ElementState::Pressed {
                        self.keys.insert(code);
                    } else {
                        self.keys.remove(&code);
                    }
                }
            }
            WindowEvent::MouseInput { button: MouseButton::Right, state, .. } => {
                self.looking = state == ElementState::Pressed;
                if let Some(g) = &self.gfx {
                    let mode = if self.looking { CursorGrabMode::Locked } else { CursorGrabMode::None };
                    if g.window.set_cursor_grab(mode).is_err() && self.looking {
                        let _ = g.window.set_cursor_grab(CursorGrabMode::Confined);
                    }
                    g.window.set_cursor_visible(!self.looking);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                if let Some(p) = &mut self.player {
                    p.cam_dist = (p.cam_dist / 1.15f32.powf(d)).clamp(1.0, 20.0);
                } else {
                    self.fly.speed = (self.fly.speed * 1.2f32.powf(d)).clamp(1.0, 500.0);
                }
            }
            WindowEvent::RedrawRequested if self.args.capture.is_some() => {}
            WindowEvent::RedrawRequested => {
                let dt = self.last.elapsed().as_secs_f32().min(0.1);
                self.last = Instant::now();
                self.update(dt);
                self.animate(dt);
                self.draw();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if self.looking {
                if let Some(p) = &mut self.player {
                    p.cam_yaw -= delta.0 as f32 * 0.003;
                    p.cam_pitch = (p.cam_pitch - delta.1 as f32 * 0.003).clamp(-1.3, 1.0);
                } else {
                    self.fly.yaw -= delta.0 as f32 * 0.003;
                    self.fly.pitch = (self.fly.pitch - delta.1 as f32 * 0.003).clamp(-1.5, 1.5);
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(path) = self.args.capture.clone() {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
            if self.gfx.is_some() {
                self.capture_step(&path);
            }
            return;
        }
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }
}

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

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let hold = parse_keys(&args.hold.join(","))?;
    let script = args.input.as_deref().map(parse_input).transpose()?.unwrap_or_default();
    let event_loop = EventLoop::new()?;
    let mut app = App {
        args,
        gfx: None,
        fly: Fly { pos: Vec3::new(-20.0, -60.0, 12.0), yaw: 1.2, pitch: -0.1, speed: 12.0 },
        player: None,
        keys: hold,
        script,
        looking: false,
        last: Instant::now(),
        frames: 0,
        fps_time: Instant::now(),
        start: Instant::now(),
        sim_time: 0.0,
        kills: None,
        climbs: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
