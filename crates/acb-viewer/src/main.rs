//! Fly-through viewer for ACB maps.
//!
//! WASD move, Space/C up/down, hold right mouse to look, Shift for speed,
//! mouse wheel changes the base speed, Escape quits. `--character` puts
//! MP personas on the map's standard player spawns, playing `--anim`.
//! `--play` hands the first one to the keyboard: WASD walk relative to the
//! camera, Shift runs, Space sprints, right mouse orbits, the wheel zooms.
//!
//! Gameplay is `acb_game::World`, stepped at a fixed 60 Hz: each frame runs
//! the ticks that are due (keys sampled per tick, presses between ticks
//! count once) and draws the characters interpolated between the last two.
//!
//! `--connect host:port` joins an `acbmp serve` match: the own player is
//! predicted locally and reconciled with the server, the others are drawn
//! ~100 ms behind the newest server tick (`acb_net::client`).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use acb_anim::character::Character;
use acb_anim::cloth::ClothSim;
use acb_anim::BuiltinTracks;
use acb_game::ground::Ground;
use acb_game::{PlayerInput, Setup, World, TICK_HZ};
use acb_render::{wgpu, Camera, Renderer, Scene, DEPTH_FORMAT};
use clap::Parser;
use glam::{Mat4, Vec3};
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
    /// Start the --play character here (world x,y,z[,yaw degrees, 0 = +Y,
    /// 90 = -X]) instead of its spawn
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    at: Option<Vec<f32>>,
    /// Scripted keys over simulated time, e.g. "0=w,shift 2= 2.5=d": from
    /// each time on exactly these keys are held (replaces --hold)
    #[arg(long)]
    input: Option<String>,
    /// --capture runs: simulated render rate (frames of 1/N s fed to the
    /// 60 Hz tick; default 60), to check frame-rate independence
    #[arg(long)]
    fps: Option<f64>,
    /// Join an `acbmp serve` match at host:port (plays the first
    /// --character; --input times are match time)
    #[arg(long)]
    connect: Option<String>,
    /// Player name sent to the server
    #[arg(long, default_value = "player")]
    name: String,
    /// Simulated network on this client, e.g. loss=5%,delay=50ms,jitter=10ms
    #[arg(long, default_value = "")]
    netsim: String,
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
                "q" => KeyCode::KeyQ,
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

/// A body's GPU side: one skinned instance per character part, and the
/// cloth simulations (visual only, stepped per frame).
struct Skin {
    handles: Vec<Option<usize>>,
    /// One simulation per `character.cloths`.
    cloth: Vec<ClothSim>,
}

/// Skins every part of a character at `world` and steps its cloth by `dt`:
/// cloth parts are posed on the CPU and drawn with identity skinning.
fn pose_skin(scene: &mut Scene, r: &Renderer, c: &Character, skin: &mut Skin, globals: &[Mat4], world: Mat4, dt: f32) {
    let mut skins = c.skin_matrices(globals);
    let to_model = world.inverse();
    for (ci, sim) in skin.cloth.iter_mut().enumerate() {
        let Some(part) = c.cloths[ci].part else { continue };
        let target: Vec<Vec3> = c.cloth_targets(ci, globals).into_iter().map(|p| world.transform_point3(p)).collect();
        let capsules: Vec<(Vec3, Vec3, f32)> = c
            .cloth_capsules(ci, globals)
            .into_iter()
            .map(|(a, b, r)| (world.transform_point3(a), world.transform_point3(b), r))
            .collect();
        sim.step(&c.cloths[ci].lod, &target, &capsules, dt);
        let local: Vec<Vec3> = sim.pos.iter().map(|p| to_model.transform_point3(*p)).collect();
        if let (Some(raw), Some(h)) = (c.cloth_vertices(ci, &local, &skins[part]), skin.handles[part]) {
            scene.set_skinned_vertices(r, h, &raw);
            skins[part].iter_mut().for_each(|m| *m = Mat4::IDENTITY);
        }
    }
    for (h, m) in skin.handles.iter().zip(skins) {
        if let Some(h) = h {
            scene.set_skinned_pose(*h, world, &m);
        }
    }
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
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

/// The --play follow camera.
struct Orbit {
    yaw: f32,
    pitch: f32,
    dist: f32,
}

impl Orbit {
    fn look(&self) -> Vec3 {
        Vec3::new(self.yaw.cos() * self.pitch.cos(), self.yaw.sin() * self.pitch.cos(), self.pitch.sin())
    }

    /// Eye and target following a character standing at `pos`.
    fn eye_target(&self, pos: Vec3, ground: Option<&Ground>) -> (Vec3, Vec3) {
        let target = pos + Vec3::Z * 1.6;
        let mut dist = self.dist;
        let back = -self.look();
        // pull in in front of whatever is between the player and the eye
        // (keeping 0.25 m off it; geometry hugging the player is ignored)
        const SKIP: f32 = 0.3;
        if let Some(f) = ground.and_then(|g| g.ray(target + back * SKIP, back * (dist + 0.25 - SKIP))) {
            dist = (SKIP + (dist + 0.25 - SKIP) * f - 0.25).max(0.6);
        }
        (target + back * dist, target)
    }
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    renderer: Renderer,
    scene: Scene,
    /// Per `world.bodies`.
    skins: Vec<Skin>,
}

struct App {
    args: Args,
    gfx: Option<Gfx>,
    world: Option<World>,
    fly: Fly,
    orbit: Orbit,
    /// Keys held, and keys pressed since the last tick (a tap shorter than
    /// a tick still reaches one).
    keys: HashSet<KeyCode>,
    tapped: HashSet<KeyCode>,
    looking: bool,
    last: Instant,
    frames: u32,
    fps_time: Instant,
    /// `--input` events not yet applied.
    script: Vec<(f32, HashSet<KeyCode>)>,
    /// Seconds of frames shown (sum of frame times; ticks follow it).
    render_time: f64,
    /// --capture: frames rendered.
    capture_frames: u64,
    /// Where this frame is between the last two ticks.
    alpha: f32,
    /// --connect: the match client.
    net: Option<acb_net::client::Client>,
    /// --capture with --connect: when the first frame ran (frames are
    /// paced to real time).
    net_t0: Option<Instant>,
    /// --connect: the match start, until the world is built from it.
    connect: Option<acb_net::client::Start>,
}

/// Controls from held and tapped keys and the camera yaw.
fn input_from(keys: &HashSet<KeyCode>, tapped: &HashSet<KeyCode>, cam_yaw: f32) -> PlayerInput {
    let k = |c| keys.contains(&c) || tapped.contains(&c);
    PlayerInput {
        forward: k(KeyCode::KeyW),
        back: k(KeyCode::KeyS),
        left: k(KeyCode::KeyA),
        right: k(KeyCode::KeyD),
        cam_yaw,
        run: k(KeyCode::ShiftLeft) || k(KeyCode::ShiftRight),
        sprint: k(KeyCode::Space),
        jump: false,
        kill: k(KeyCode::KeyE),
        stun: k(KeyCode::KeyQ),
    }
}

impl App {
    /// --connect: joins the match first (the world is built from the
    /// server's slots), then loads.
    fn connect_and_init(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        if let Some(addr) = self.args.connect.clone() {
            let map = self.args.map.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let character = self.args.characters.first().cloned().ok_or_else(|| anyhow::anyhow!("--connect needs --character"))?;
            let cfg = acb_net::client::ClientConfig { server: addr, name: self.args.name.clone(), character, map, netsim: acb_net::link::NetSim::parse(&self.args.netsim)? };
            let (client, start) = acb_net::client::Client::connect(&cfg, std::time::Duration::from_secs(300))?;
            self.net = Some(client);
            self.connect = Some(start);
        }
        self.init(event_loop)
    }

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
        let world = match self.connect.take() {
            Some(start) => {
                let world = acb_net::game::load_world(&archive, &start.slots, self.builtins().as_ref())?;
                eprintln!("net: match world with {} players, playing slot {}", world.players.len(), start.slot);
                world
            }
            None => self.load_world(&archive)?,
        };
        let skins = world
            .bodies
            .iter()
            .map(|b| Skin {
                handles: b.character.parts.iter().map(|p| scene.add_skinned(&renderer, &archive, &p.mesh, &p.materials)).collect(),
                cloth: vec![ClothSim::default(); b.character.cloths.len()],
            })
            .collect();
        // start behind the own one, looking along its facing (+Y)
        if let Some(b) = world.players.get(self.me()).and_then(|p| world.bodies.get(p.body)).or(world.bodies.first()) {
            let mut facing = b.spawn.y_axis.truncate().normalize_or_zero();
            let mut at = b.spawn.w_axis.truncate();
            if let Some(p) = world.players.get(self.me()) {
                facing = Vec3::new(-p.yaw.sin(), p.yaw.cos(), 0.0);
                at = p.pos;
            }
            self.fly.pos = at - facing * 4.0 + Vec3::Z * 1.8;
            self.fly.yaw = facing.y.atan2(facing.x);
            self.fly.pitch = -0.15;
            self.fly.speed = 4.0;
            self.orbit = Orbit { yaw: self.fly.yaw, pitch: -0.25, dist: 3.5 };
        }
        self.world = Some(world);
        self.gfx = Some(Gfx { window, surface, config, depth, renderer, scene, skins });
        self.render_time = 0.0;
        Ok(())
    }

    fn load_world(&self, archive: &acb_world::Archive) -> anyhow::Result<World> {
        let mut characters = Vec::new();
        for name in &self.args.characters {
            characters.push((name.clone(), Arc::new(acb_game::load_character(archive, name)?)));
        }
        let builtins = if characters.is_empty() { None } else { self.builtins() };
        let setup = Setup {
            characters,
            count: self.args.count,
            anim: self.args.anim.clone(),
            wander: self.args.wander,
            play: self.args.play,
            crowd: self.args.crowd,
            pair: self.args.pair.clone(),
            at: self.args.at.clone(),
            builtins: builtins.as_ref(),
        };
        World::load(archive, &setup)
    }

    fn builtins(&self) -> Option<BuiltinTracks> {
        let exe = self.args.exe.clone().or_else(|| {
            let p = self.args.map.parent()?.parent()?.join("ACBMP.exe");
            p.exists().then_some(p)
        });
        let b = exe.as_ref().and_then(|p| BuiltinTracks::from_executable(&std::fs::read(p).ok()?));
        if b.is_none() {
            eprintln!("no ACBMP.exe builtin tracks (--exe): no root motion");
        }
        b
    }

    /// This tick's controls from the keys and the camera.
    fn input(&self) -> PlayerInput {
        input_from(&self.keys, &self.tapped, self.orbit.yaw)
    }

    /// The player this viewer drives.
    fn me(&self) -> usize {
        self.net.as_ref().map_or(0, |n| n.slot)
    }

    /// Runs the ticks due by `render_time` and returns how far the frame
    /// is between the last two (1 = on the last tick).
    fn run_ticks(&mut self) -> f32 {
        let Some(mut world) = self.world.take() else { return 1.0 };
        if let Some(mut net) = self.net.take() {
            let (script, keys, tapped, yaw) = (&mut self.script, &mut self.keys, &mut self.tapped, self.orbit.yaw);
            let alpha = net.advance(&mut world, |tick| {
                let time = tick as f32 / TICK_HZ as f32;
                while script.first().is_some_and(|e| e.0 <= time) {
                    *keys = script.remove(0).1;
                }
                let i = input_from(keys, tapped, yaw);
                tapped.clear();
                i
            });
            self.net = Some(net);
            self.world = Some(world);
            return alpha;
        }
        let hz = TICK_HZ as f64;
        let due = (self.render_time * hz - 1e-6).ceil().max(0.0) as u64;
        while world.tick < due {
            let next = world.next_time();
            while self.script.first().is_some_and(|e| e.0 <= next) {
                self.keys = self.script.remove(0).1;
            }
            world.step(&[self.input()]);
            self.tapped.clear();
        }
        let behind = world.tick as f64 - self.render_time * hz;
        self.world = Some(world);
        let alpha = (1.0 - behind) as f32;
        // frames on a tick show it exactly
        if alpha > 1.0 - 1e-4 {
            1.0
        } else {
            alpha.max(0.0)
        }
    }

    /// Advances the simulation to `render_time` and poses every body for
    /// this frame (`dt` steps the cloth).
    fn animate(&mut self, dt: f32) {
        let alpha = self.run_ticks();
        let (Some(g), Some(w)) = (&mut self.gfx, &self.world) else { return };
        for (i, (b, skin)) in w.bodies.iter().zip(&mut g.skins).enumerate() {
            // remote players: their server ticks, ~100 ms behind
            let remote = self.net.as_ref().and_then(|n| w.players.iter().position(|p| p.body == i).and_then(|j| n.remote_view(j)));
            let (world, pose) = remote.unwrap_or_else(|| b.interpolated(alpha));
            let globals = b.character.skeleton.globals(&pose);
            pose_skin(&mut g.scene, &g.renderer, &b.character, skin, &globals, world, dt);
        }
        self.alpha = alpha;
    }

    fn update(&mut self, dt: f32) {
        if self.player_pos().is_some() {
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

    /// The --play character's position as drawn this frame.
    fn player_pos(&self) -> Option<Vec3> {
        self.world.as_ref()?.players.get(self.me()).map(|p| p.pos_at(self.alpha))
    }

    fn camera(&self, g: &Gfx) -> Camera {
        let (eye, target) = match self.player_pos() {
            Some(pos) => self.orbit.eye_target(pos, self.world.as_ref().and_then(|w| w.ground.as_ref())),
            None => (self.fly.pos, self.fly.pos + self.fly.forward()),
        };
        Camera { eye, target, fov_y: 60f32.to_radians(), aspect: g.config.width as f32 / g.config.height as f32 }
    }

    /// --capture: steps frames at --fps (default 60) without presenting (a
    /// hidden window may never get a frame), then writes one frame.
    fn capture_step(&mut self, path: &std::path::Path) {
        let fps = self.args.fps.unwrap_or(TICK_HZ as f64);
        let dt = (1.0 / fps) as f32;
        self.capture_frames += 1;
        self.render_time = self.capture_frames as f64 / fps;
        if self.net.is_some() {
            // the server runs in real time: frames at --fps real rate
            let t0 = *self.net_t0.get_or_insert_with(Instant::now);
            let due = t0 + std::time::Duration::from_secs_f64(self.capture_frames as f64 / fps);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        self.update(dt);
        self.animate(dt);
        let over = self.net.as_ref().and_then(|n| n.over.clone());
        if over.is_none() && self.world.as_ref().is_some_and(|w| w.time < self.args.capture_after) {
            return;
        }
        if let Some(reason) = over {
            eprintln!("net: match over: {reason}");
        }
        let Some(g) = &self.gfx else { return };
        let camera = self.camera(g);
        let rgba = acb_render::render_to_rgba(&g.renderer, &g.scene, &camera, g.config.width, g.config.height);
        match write_png(path, g.config.width, g.config.height, &rgba) {
            Ok(()) => eprintln!("wrote {}", path.display()),
            Err(e) => eprintln!("capture failed: {e:#}"),
        }
        eprintln!("camera eye {:.2?} target {:.2?}", camera.eye.to_array(), camera.target.to_array());
        if let Some(p) = self.world.as_ref().and_then(|w| w.players.get(self.me())) {
            eprintln!("player at {:.2} {:.2} {:.2} yaw {:.0} speed {:.2}", p.pos.x, p.pos.y, p.pos.z, p.yaw.to_degrees(), p.loco.speed);
        }
        if let (Some(n), Some(w)) = (&mut self.net, &self.world) {
            for (j, p) in w.players.iter().enumerate().filter(|(j, _)| *j != n.slot) {
                let drawn = n.remote_view(j).map(|v| v.0.w_axis.truncate()).unwrap_or(p.pos);
                eprintln!("net: remote player {j} drawn at {:.2} {:.2} {:.2} (tick {:.1}), predicted {:.2} {:.2} {:.2}", drawn.x, drawn.y, drawn.z, n.remote_tick(), p.pos.x, p.pos.y, p.pos.z);
            }
            let s = n.stats;
            let l = n.link_stats();
            eprintln!(
                "net: tick {} confirmed {} rtt {:.0} ms; {} snapshots, {} checked ({} exact, {} desync), {} corrections, {} rollbacks, {} rejected; sent {} recv {} dropped {}",
                w.tick, n.confirmed_tick, n.rtt * 1000.0, s.snapshots, s.checked, s.exact, s.desyncs, s.corrections, s.rollbacks, s.rejected, l.sent, l.recv, l.dropped
            );
            n.leave();
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
        g.renderer.render(&mut enc, &view, &g.depth, (g.config.width, g.config.height), &g.scene, &camera);
        g.renderer.queue().submit([enc.finish()]);
        g.renderer.queue().present(frame);

        self.frames += 1;
        let t = self.fps_time.elapsed().as_secs_f32();
        if t >= 1.0 {
            let player = self.world.as_ref().and_then(|w| w.players.first());
            let pos = player.map_or(self.fly.pos, |p| p.pos);
            let mut status = format!(
                "acb-viewer: {:.0} fps  pos {:.1} {:.1} {:.1}",
                self.frames as f32 / t,
                pos.x,
                pos.y,
                pos.z
            );
            if let Some(p) = player {
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
            if let Err(e) = self.connect_and_init(event_loop) {
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
                        self.tapped.insert(code);
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
                if self.player_pos().is_some() {
                    self.orbit.dist = (self.orbit.dist / 1.15f32.powf(d)).clamp(1.0, 20.0);
                } else {
                    self.fly.speed = (self.fly.speed * 1.2f32.powf(d)).clamp(1.0, 500.0);
                }
            }
            WindowEvent::RedrawRequested if self.args.capture.is_some() => {}
            WindowEvent::RedrawRequested => {
                let dt = self.last.elapsed().as_secs_f32().min(0.1);
                self.last = Instant::now();
                self.render_time += dt as f64;
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
                if self.player_pos().is_some() {
                    self.orbit.yaw -= delta.0 as f32 * 0.003;
                    self.orbit.pitch = (self.orbit.pitch - delta.1 as f32 * 0.003).clamp(-1.3, 1.0);
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
        world: None,
        fly: Fly { pos: Vec3::new(-20.0, -60.0, 12.0), yaw: 1.2, pitch: -0.1, speed: 12.0 },
        orbit: Orbit { yaw: 0.0, pitch: -0.25, dist: 3.5 },
        keys: hold,
        tapped: HashSet::new(),
        script,
        looking: false,
        last: Instant::now(),
        frames: 0,
        fps_time: Instant::now(),
        render_time: 0.0,
        capture_frames: 0,
        alpha: 1.0,
        net: None,
        net_t0: None,
        connect: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
