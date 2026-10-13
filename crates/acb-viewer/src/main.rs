//! Fly-through viewer for ACB maps.
//!
//! WASD move, Space/C up/down, hold right mouse to look, Shift for speed,
//! mouse wheel changes the base speed, Escape quits.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use acb_render::{wgpu, Camera, Renderer, Scene, DEPTH_FORMAT};
use clap::Parser;
use glam::Vec3;
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
}

struct App {
    args: Args,
    gfx: Option<Gfx>,
    fly: Fly,
    keys: HashSet<KeyCode>,
    looking: bool,
    last: Instant,
    frames: u32,
    fps_time: Instant,
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
        let (scene, s) = Scene::load(&renderer, &archive)?;
        eprintln!(
            "{} meshes, {} instances, {} triangles, {} textures, loaded in {:.1}s",
            s.meshes, s.instances, s.triangles, s.textures, t0.elapsed().as_secs_f64()
        );
        self.gfx = Some(Gfx { window, surface, config, depth, renderer, scene });
        Ok(())
    }

    fn update(&mut self, dt: f32) {
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

    fn draw(&mut self) {
        let Some(g) = &mut self.gfx else { return };
        let frame = match g.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                g.surface.configure(g.renderer.device(), &g.config);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let camera = Camera {
            eye: self.fly.pos,
            target: self.fly.pos + self.fly.forward(),
            fov_y: 60f32.to_radians(),
            aspect: g.config.width as f32 / g.config.height as f32,
        };
        let mut enc = g.renderer.device().create_command_encoder(&Default::default());
        g.renderer.render(&mut enc, &view, &g.depth, &g.scene, &camera);
        g.renderer.queue().submit([enc.finish()]);
        g.renderer.queue().present(frame);

        self.frames += 1;
        let t = self.fps_time.elapsed().as_secs_f32();
        if t >= 1.0 {
            let status = format!(
                "acb-viewer: {:.0} fps  pos {:.0} {:.0} {:.0}",
                self.frames as f32 / t,
                self.fly.pos.x,
                self.fly.pos.y,
                self.fly.pos.z
            );
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
            WindowEvent::KeyboardInput { event, .. } => {
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
                self.fly.speed = (self.fly.speed * 1.2f32.powf(d)).clamp(1.0, 500.0);
            }
            WindowEvent::RedrawRequested => {
                let dt = self.last.elapsed().as_secs_f32().min(0.1);
                self.last = Instant::now();
                self.update(dt);
                self.draw();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if self.looking {
                self.fly.yaw -= delta.0 as f32 * 0.003;
                self.fly.pitch = (self.fly.pitch - delta.1 as f32 * 0.003).clamp(-1.5, 1.5);
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let event_loop = EventLoop::new()?;
    let mut app = App {
        args,
        gfx: None,
        fly: Fly { pos: Vec3::new(-20.0, -60.0, 12.0), yaw: 1.2, pitch: -0.1, speed: 12.0 },
        keys: HashSet::new(),
        looking: false,
        last: Instant::now(),
        frames: 0,
        fps_time: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
