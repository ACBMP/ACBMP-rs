//! Materials drawn with the game's own compiled shaders.
//!
//! A material's main forward pass uses its template's vertex permutation
//! `(MeshData.VertexFormat << 16) | 1` (pass 1) and the lighting pixel
//! permutation `(ShaderKey 1, LightingKey 1)`: ambient cube plus one direct
//! light, fog and the world lightmap. See NOTES.md for the key layouts.
//!
//! Constants come from three places:
//! - per program (frame): engine globals, filled each frame by CTAB name
//!   (`g_EyePosition`, `g_AmbientCube`, `g_DirectLights`, `g_Fog*`, ...);
//! - per material: the template parameters listed by the permutation,
//!   from the Material's values (`Layer0_UVScale`, `Layer0_Color`, ...);
//! - per instance: `g_WorldViewProj` / `g_World` rows, read from an
//!   instance vertex buffer so a mesh stays one instanced draw.
//!
//! The shaders read the raw vertex buffer: SHORT4 position and UBYTE4
//! normal/tangent are integer attributes, the texcoord is SHORT2N and the
//! vertex colour is a D3DCOLOR.
//!
//! Skinned (format 0) draws index `g_BoneMatrixArray` relatively, which the
//! translation reads from the frame block, so each skinned submesh owns its
//! frame buffer: the program's globals plus its bone palette.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use acb_world::{Archive, MaterialOverride, MaterialParams, ParamValue, Permutation};
use d3d9_shader::parse::{RegisterSet, SamplerType};
use d3d9_shader::wgsl::{self, InputKind, Options};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::{Renderer, DEPTH_FORMAT};

/// `GfxVertexShaderKey` word0 of the main lit forward pass.
pub const MAIN_PASS: u32 = 1;
/// One direct light, no omni/spot lights, no shadows.
pub const LIGHTING_KEY: u32 = 1;
/// Instance attribute locations: WVP rows, then World rows.
const INSTANCE_LOC: u32 = 24;
/// Pixel material register holding the alpha-test reference.
const ALPHA_REF_REG: u32 = wgsl::PS_CONSTS as u32 - 1;
/// Bytes per instance: 4 WVP rows + 4 World rows.
pub const INSTANCE_STRIDE: u64 = 128;
/// `g_BoneMatrixArray`: float4x3 rows from c120, 42 bones.
const BONE_REG: u32 = 120;

/// Frame lighting, in the shaders' gamma space.
#[derive(Debug, Clone, Copy)]
pub struct Environment {
    /// Direction towards the sun.
    pub sun_dir: Vec3,
    pub sun_color: Vec3,
    /// +X, -X, +Y, -Y, +Z, -Z.
    pub ambient: [Vec3; 6],
    pub fog_color: Vec3,
    pub fog_start: f32,
    pub fog_range: f32,
    pub fog_max: f32,
    /// World lightmap constants, when one is bound.
    pub lightmap: Option<Lightmap>,
}

/// `g_WorldLightmap*` values for a world lightmap (see the lighting PS:
/// uv = (pos.xy + n.xy * normal_offset) * uv_scale + 0.5; the texel alpha
/// is the baked ground height z_min + a * z_range, and the lightmap fades
/// out at `fade` per metre above or below it; its rgb * intensity replaces
/// the ambient by sat(dot(albedo * lm, weight))).
#[derive(Debug, Clone, Copy)]
pub struct Lightmap {
    pub uv_scale: f32,
    pub z_min: f32,
    pub z_range: f32,
    pub normal_offset: f32,
    pub fade: f32,
    pub intensity: f32,
    pub weight: f32,
}

impl Default for Environment {
    fn default() -> Self {
        let side = Vec3::new(0.36, 0.37, 0.40);
        Self {
            sun_dir: Vec3::new(0.4, -0.5, 0.75).normalize(),
            sun_color: Vec3::new(1.05, 0.98, 0.86),
            ambient: [side, side, side, side, Vec3::new(0.46, 0.52, 0.62), Vec3::new(0.26, 0.24, 0.21)],
            fog_color: Vec3::new(0.62, 0.68, 0.74),
            fog_start: 60.0,
            fog_range: 900.0,
            fog_max: 0.7,
            lightmap: None,
        }
    }
}

/// Vertex attributes of the static formats, by shader input location.
fn vertex_attributes(format: u32, stride: usize) -> Option<Vec<wgpu::VertexAttribute>> {
    use wgpu::VertexFormat as F;
    let attr = |loc, format, offset| wgpu::VertexAttribute { format, offset, shader_location: loc };
    Some(match (format, stride) {
        (4, 20) => vec![attr(0, F::Sint16x4, 0), attr(3, F::Uint8x4, 8), attr(6, F::Uint8x4, 12), attr(8, F::Snorm16x2, 16)],
        (3, 24) => vec![
            attr(0, F::Sint16x4, 0),
            attr(3, F::Uint8x4, 8),
            attr(6, F::Uint8x4, 12),
            attr(4, F::Unorm8x4Bgra, 16),
            attr(8, F::Snorm16x2, 20),
        ],
        // skinned: SHORT4N position, UBYTE4 palette slots, UBYTE4N weights
        (0, 32) => vec![
            attr(0, F::Snorm16x4, 0),
            attr(3, F::Uint8x4, 8),
            attr(6, F::Uint8x4, 12),
            attr(7, F::Uint8x4, 16),
            attr(8, F::Snorm16x2, 20),
            attr(2, F::Uint8x4, 24),
            attr(1, F::Unorm8x4, 28),
        ],
        _ => return None,
    })
}

fn input_kind(f: wgpu::VertexFormat) -> InputKind {
    use wgpu::VertexFormat as F;
    match f {
        F::Sint16x4 | F::Sint16x2 | F::Sint8x4 | F::Sint32x4 => InputKind::Sint,
        F::Uint8x4 | F::Uint16x2 | F::Uint16x4 | F::Uint32x4 => InputKind::Uint,
        _ => InputKind::Float,
    }
}

#[derive(Debug, Clone)]
struct Global {
    name: String,
    set: RegisterSet,
    reg: u32,
    count: u32,
}

fn globals(p: &d3d9_shader::Program) -> Vec<Global> {
    p.constants
        .iter()
        .filter(|c| c.name.starts_with("g_"))
        .map(|c| Global { name: c.name.clone(), set: c.set, reg: c.index, count: c.count })
        .collect()
}

/// A `Consts` uniform block: c[n] vec4<f32>, i[16] vec4<i32>, b[16] vec4<u32>.
#[derive(Clone)]
struct ConstBlock {
    c: Vec<[f32; 4]>,
    i: [[i32; 4]; 16],
    b: [[u32; 4]; 16],
}

impl ConstBlock {
    fn new(n: usize) -> Self {
        Self { c: vec![[0.0; 4]; n], i: [[0; 4]; 16], b: [[0; 4]; 16] }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.c.len() * 16 + 512);
        out.extend_from_slice(bytemuck::cast_slice(&self.c));
        out.extend_from_slice(bytemuck::cast_slice(&self.i));
        out.extend_from_slice(bytemuck::cast_slice(&self.b));
        out
    }

    fn set(&mut self, reg: u32, v: [f32; 4]) {
        if let Some(c) = self.c.get_mut(reg as usize) {
            *c = v;
        }
    }
}

/// Per-frame camera facts the engine globals are made of.
pub struct FrameView {
    pub eye: Vec3,
    pub forward: Vec3,
    /// World -> view (right-handed, looking down -Z).
    pub view: Mat4,
    pub tan_half: (f32, f32),
    pub size: (u32, u32),
}

impl FrameView {
    pub fn new(camera: &crate::Camera, size: (u32, u32)) -> FrameView {
        let view = glam::camera::rh::view::look_at_mat4(camera.eye, camera.target, Vec3::Z);
        let ty = (camera.fov_y / 2.0).tan();
        FrameView {
            eye: camera.eye,
            forward: (camera.target - camera.eye).normalize_or_zero(),
            view,
            tan_half: (ty * camera.aspect, ty),
            size,
        }
    }
}

fn fill_globals(block: &mut ConstBlock, globals: &[Global], env: &Environment, fv: &FrameView) -> BTreeSet<String> {
    let eye = fv.eye;
    let mut unknown = BTreeSet::new();
    let v = |x: Vec3, w: f32| [x.x, x.y, x.z, w];
    for g in globals {
        match g.set {
            RegisterSet::Float4 => {}
            RegisterSet::Bool => {
                // deconstruction, depth-texture shadows: off
                if let Some(b) = block.b.get_mut(g.reg as usize) {
                    *b = [0; 4];
                }
                continue;
            }
            _ => continue,
        }
        let rows: Vec<[f32; 4]> = match g.name.as_str() {
            // fed per instance
            "g_WorldViewProj" | "g_World" => continue,
            "g_EyePosition" => vec![v(eye, 1.0)],
            // the pixel shader kills on the plane distance; keep everything
            "g_ClipPlane" => vec![[0.0, 0.0, 0.0, -1.0]],
            "g_FogColor" => vec![v(env.fog_color, 1.0)],
            "g_FogParams" => vec![[env.fog_start, 1.0 / env.fog_range, env.fog_max, 0.0]],
            "g_AmbientCube" => env.ambient.iter().map(|a| v(*a, 1.0)).collect(),
            "g_DirectLights" => vec![v(env.sun_dir, 0.0), v(env.sun_color, 1.0)],
            // without a lightmap .y = 0 removes its contribution
            "g_WorldLightmapParameters" => match env.lightmap {
                Some(l) => vec![[l.normal_offset, l.z_min, l.fade, l.z_range]],
                None => vec![[0.0; 4]],
            },
            "g_WorldLightmapUVParameters" => match env.lightmap {
                Some(l) => vec![[l.intensity, l.weight, l.uv_scale, l.uv_scale]],
                None => vec![[0.0, 0.0, 1.0, 1.0]],
            },
            // mip by distance: start, range
            "g_WorldLightmapZOffsetParameters" => vec![[0.0, 60.0, 0.0, 0.0]],
            "g_DeconstructionRanges" => vec![[1.0e6, 1.0e6 + 1.0, 0.0, 0.0]],
            "g_EyeDirection" => vec![v(fv.forward, 0.0)],
            // world -> view with z = distance ahead (the shaders compare it
            // with the depth sampler's linearized value)
            "g_WorldToView" => {
                let m = fv.view;
                vec![m.row(0).to_array(), m.row(1).to_array(), (-m.row(2)).to_array()]
            }
            // g_DepthSampler holds 1 / distance (see the renderer's depth
            // copy), so distance = w / (d + z) with z = 0, w = 1
            "g_ReverseProjectionParams" => vec![[fv.tan_half.0, fv.tan_half.1, 0.0, 1.0]],
            "g_VPosToUV" => vec![[1.0 / fv.size.0.max(1) as f32, 1.0 / fv.size.1.max(1) as f32, 0.0, 0.0]],
            _ => {
                unknown.insert(g.name.clone());
                continue;
            }
        };
        for (k, r) in rows.into_iter().take(g.count as usize).enumerate() {
            block.set(g.reg + k as u32, r);
        }
    }
    unknown
}

/// Translated main-pass program of one template + vertex format.
pub(crate) struct Program {
    /// [culled, two-sided]
    pipelines: [wgpu::RenderPipeline; 2],
    tex_layout: wgpu::BindGroupLayout,
    vs_frame: wgpu::Buffer,
    ps_frame: wgpu::Buffer,
    vs_globals: Vec<Global>,
    ps_globals: Vec<Global>,
    /// (stage base binding, register, type, CTAB name)
    samplers: Vec<(u32, u32, SamplerType, String)>,
    vs_params: Vec<acb_world::ShaderParam>,
    ps_params: Vec<acb_world::ShaderParam>,
}

pub(crate) struct Material {
    pub program: usize,
    pub consts: wgpu::BindGroup,
    vs_mat: wgpu::Buffer,
    ps_mat: wgpu::Buffer,
    pub textures: wgpu::BindGroup,
    pub two_sided: bool,
    pub blended: bool,
    /// Texture views by binding, and the binding of `g_DepthSampler` if
    /// the program reads the scene depth.
    views: Vec<(u32, wgpu::TextureView)>,
    depth_binding: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ProgramKey {
    template: u32,
    format: u32,
    alpha_test: bool,
    blended: bool,
}

/// Why a material fell back to the placeholder shading.
#[derive(Debug, Default)]
pub struct RetailStats {
    pub materials: usize,
    pub programs: usize,
    pub fallbacks: BTreeMap<String, usize>,
    pub unknown_globals: BTreeSet<String>,
}

pub(crate) struct Defaults {
    white: wgpu::TextureView,
    black: wgpu::TextureView,
    flat_normal: wgpu::TextureView,
    cube: wgpu::TextureView,
    volume: wgpu::TextureView,
}

impl Defaults {
    pub fn new(r: &Renderer) -> Self {
        let make = |dim: wgpu::TextureDimension, layers: u32, view: wgpu::TextureViewDimension, rgba: [u8; 4]| {
            let tex = r.device.create_texture_with_data(
                &r.queue,
                &wgpu::TextureDescriptor {
                    label: Some("default"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: dim,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &rgba.repeat(layers as usize),
            );
            tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(view), ..Default::default() })
        };
        use wgpu::{TextureDimension as D, TextureViewDimension as V};
        Self {
            white: make(D::D2, 1, V::D2, [255; 4]),
            black: make(D::D2, 1, V::D2, [0, 0, 0, 255]),
            flat_normal: make(D::D2, 1, V::D2, [128, 128, 128, 255]),
            cube: make(D::D2, 6, V::Cube, [255; 4]),
            volume: make(D::D3, 1, V::D3, [255; 4]),
        }
    }
}

pub(crate) struct Retail {
    pub programs: Vec<Program>,
    keys: HashMap<ProgramKey, Option<usize>>,
    consts_layout: wgpu::BindGroupLayout,
    defaults: Defaults,
    pub stats: RetailStats,
    /// Bound to `g_WorldLightMapSampler`.
    pub world_lightmap: Option<wgpu::TextureView>,
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Retail {
    pub fn new(r: &Renderer) -> Self {
        use wgpu::ShaderStages as S;
        let consts_layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("retail consts"),
            entries: &[
                uniform_entry(0, S::VERTEX),
                uniform_entry(1, S::FRAGMENT),
                uniform_entry(2, S::VERTEX),
                uniform_entry(3, S::FRAGMENT),
            ],
        });
        Self {
            programs: Vec::new(),
            keys: HashMap::new(),
            consts_layout,
            defaults: Defaults::new(r),
            stats: RetailStats::default(),
            world_lightmap: None,
        }
    }

    fn fallback(&mut self, why: impl Into<String>) -> Option<Material> {
        *self.stats.fallbacks.entry(why.into()).or_default() += 1;
        None
    }

    /// Retail material for a submesh, or `None` to use the placeholder.
    pub fn material(
        &mut self,
        r: &Renderer,
        archive: &Archive,
        uid: u32,
        ov: &MaterialOverride,
        (format, stride): (u32, usize),
        textures: &mut dyn FnMut(u32) -> Option<wgpu::TextureView>,
    ) -> Option<Material> {
        let Some(info) = archive.material(uid) else { return self.fallback("no material") };
        let Some(params) = archive.material_params_overridden(uid, ov) else { return self.fallback("no template") };
        let key = ProgramKey {
            template: params.template,
            format,
            alpha_test: info.alpha_test,
            blended: info.blend_mode != 0,
        };
        // ACBMP_DEBUG_MATERIALS=1: print every retail material's template and values
        if std::env::var_os("ACBMP_DEBUG_MATERIALS").is_some() {
            let n = |u: u32| archive.loc(u).map(|l| l.name.clone()).unwrap_or_default();
            let mut v: Vec<_> = params.values.iter().map(|(k, v)| format!("{k:#010x}={v:?}")).collect();
            v.sort();
            eprintln!("material {} blend {} template {} format {}: {}", n(uid), info.blend_mode, n(key.template), key.format, v.join(" "));
        }
        let program = match self.keys.get(&key) {
            Some(p) => *p,
            None => {
                let p = match self.program(r, archive, key, stride) {
                    Ok((p, unknown)) => {
                        self.stats.unknown_globals.extend(unknown);
                        self.programs.push(p);
                        Some(self.programs.len() - 1)
                    }
                    Err(e) => {
                        let tname = archive.loc(key.template).map(|l| l.name.clone()).unwrap_or_default();
                        eprintln!("retail: {tname} format {}: {e}", key.format);
                        *self.stats.fallbacks.entry(format!("program: {e}")).or_default() += 1;
                        None
                    }
                };
                self.keys.insert(key, p);
                p
            }
        };
        let Some(pi) = program else { return self.fallback("no program") };
        self.stats.materials += 1;
        Some(self.bind_material(r, pi, &params, info.alpha_ref as f32 / 255.0, info.two_sided, key.blended, textures))
    }

    fn program(
        &self,
        r: &Renderer,
        archive: &Archive,
        key: ProgramKey,
        stride: usize,
    ) -> anyhow::Result<(Program, BTreeSet<String>)> {
        let shaders = archive.template_shaders(key.template).ok_or_else(|| anyhow::anyhow!("no compiled template"))?;
        let vs_perm: &Permutation = shaders
            .vertex((key.format << 16) | MAIN_PASS)
            .ok_or_else(|| anyhow::anyhow!("no vertex permutation"))?;
        let ps_perm = shaders
            .lighting(MAIN_PASS, LIGHTING_KEY)
            .or_else(|| shaders.pixel(MAIN_PASS).cloned())
            .ok_or_else(|| anyhow::anyhow!("no pixel permutation"))?;
        let attrs = vertex_attributes(key.format, stride).ok_or_else(|| anyhow::anyhow!("vertex format"))?;

        let vs_prog = d3d9_shader::parse(&vs_perm.code)?;
        let ps_prog = d3d9_shader::parse(&ps_perm.code)?;
        let mut instance_consts = BTreeMap::new();
        for c in &vs_prog.constants {
            let base = match c.name.as_str() {
                "g_WorldViewProj" => INSTANCE_LOC,
                "g_World" => INSTANCE_LOC + 4,
                _ => continue,
            };
            for k in 0..c.count.min(4) {
                instance_consts.insert(c.index + k, base + k);
            }
        }
        let material_regs = |params: &[acb_world::ShaderParam]| -> BTreeSet<u32> {
            params.iter().filter(|p| !p.is_sampler()).flat_map(|p| p.register..p.register + p.count).collect()
        };
        let vs_opts = Options {
            input_kinds: attrs.iter().map(|a| (a.shader_location, input_kind(a.format))).collect(),
            material_consts: material_regs(&vs_perm.params),
            instance_consts,
            ..Default::default()
        };
        let vs = wgsl::translate(&vs_prog, &vs_opts)?;
        for i in &vs.inputs {
            if !attrs.iter().any(|a| a.shader_location == i.location) {
                anyhow::bail!("vertex input {:?}{} not in format {}", i.usage, i.index, key.format);
            }
        }
        let ps_opts = Options {
            available_varyings: Some(vs.outputs.iter().map(|o| (wgsl::usage_key(o.usage), o.index)).collect()),
            material_consts: material_regs(&ps_perm.params),
            color_outputs: Some(1),
            alpha_test: key.alpha_test.then_some(ALPHA_REF_REG),
            srgb_target: r.color_format.is_srgb(),
            ..Default::default()
        };
        let ps = wgsl::translate(&ps_prog, &ps_opts)?;

        let scope = r.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = |src: &str| {
            r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
        };
        let (vs_mod, ps_mod) = (module(&vs.wgsl), module(&ps.wgsl));

        let mut samplers = Vec::new();
        let mut tex_entries = Vec::new();
        for (prog, list, base, vis) in [
            (&vs_prog, &vs.samplers, wgsl::VS_SAMPLER_BASE, wgpu::ShaderStages::VERTEX),
            (&ps_prog, &ps.samplers, 0, wgpu::ShaderStages::FRAGMENT),
        ] {
            for &(reg, ty) in list.iter() {
                let name = prog
                    .constants
                    .iter()
                    .find(|c| c.set == RegisterSet::Sampler && c.index == reg)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                let dim = match ty {
                    SamplerType::Cube => wgpu::TextureViewDimension::Cube,
                    SamplerType::Volume => wgpu::TextureViewDimension::D3,
                    _ => wgpu::TextureViewDimension::D2,
                };
                tex_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: base + 2 * reg,
                    visibility: vis,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: dim,
                        multisampled: false,
                    },
                    count: None,
                });
                tex_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: base + 2 * reg + 1,
                    visibility: vis,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                });
                samplers.push((base, reg, ty, name));
            }
        }
        let tex_layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("retail textures"),
            entries: &tex_entries,
        });
        let layout = r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("retail"),
            bind_group_layouts: &[Some(&self.consts_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let instance_attrs: Vec<wgpu::VertexAttribute> = (0..8)
            .map(|k| wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16 * k as u64,
                shader_location: INSTANCE_LOC + k,
            })
            .collect();
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: stride as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attrs,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_STRIDE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attrs,
            }),
        ];
        let pipeline = |cull: Option<wgpu::Face>| {
            r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("retail"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &vs_mod,
                    entry_point: Some(vs.entry),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(!key.blended),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: if key.blended {
                        wgpu::DepthBiasState { constant: -2, slope_scale: -1.0, clamp: 0.0 }
                    } else {
                        Default::default()
                    },
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &ps_mod,
                    entry_point: Some(ps.entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: r.color_format,
                        blend: key.blended.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = [pipeline(Some(wgpu::Face::Back)), pipeline(None)];
        if let Some(e) = pollster::block_on(scope.pop()) {
            anyhow::bail!("pipeline: {e}");
        }

        let frame_buffer = |n: usize| {
            r.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("retail frame"),
                size: ConstBlock::new(n).bytes().len() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let (vs_globals, ps_globals) = (globals(&vs_prog), globals(&ps_prog));
        let env = Environment::default();
        let fv = FrameView { eye: Vec3::ZERO, forward: Vec3::Y, view: Mat4::IDENTITY, tan_half: (1.0, 1.0), size: (1, 1) };
        let mut unknown = fill_globals(&mut ConstBlock::new(wgsl::VS_CONSTS), &vs_globals, &env, &fv);
        unknown.extend(fill_globals(&mut ConstBlock::new(wgsl::PS_CONSTS), &ps_globals, &env, &fv));
        Ok((Program {
            pipelines,
            tex_layout,
            vs_frame: frame_buffer(wgsl::VS_CONSTS),
            ps_frame: frame_buffer(wgsl::PS_CONSTS),
            vs_globals,
            ps_globals,
            samplers,
            vs_params: vs_perm.params.clone(),
            ps_params: ps_perm.params.clone(),
        }, unknown))
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_material(
        &self,
        r: &Renderer,
        pi: usize,
        params: &MaterialParams,
        alpha_ref: f32,
        two_sided: bool,
        blended: bool,
        textures: &mut dyn FnMut(u32) -> Option<wgpu::TextureView>,
    ) -> Material {
        let p = &self.programs[pi];
        let block = |n: usize, list: &[acb_world::ShaderParam], extra: Option<(u32, f32)>| {
            let mut b = ConstBlock::new(n);
            for sp in list.iter().filter(|sp| !sp.is_sampler()) {
                if let Some(ParamValue::Vector(v)) = params.values.get(&sp.property) {
                    b.set(sp.register, *v);
                }
            }
            if let Some((reg, v)) = extra {
                b.set(reg, [v; 4]);
            }
            r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("retail material"),
                contents: &b.bytes(),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let vs_mat = block(wgsl::VS_CONSTS, &p.vs_params, None);
        let ps_mat = block(wgsl::PS_CONSTS, &p.ps_params, Some((ALPHA_REF_REG, alpha_ref)));
        let consts = r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("retail consts"),
            layout: &self.consts_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: p.vs_frame.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: p.ps_frame.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: vs_mat.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: ps_mat.as_entire_binding() },
            ],
        });

        let mut views = Vec::new();
        let mut depth_binding = None;
        for (base, reg, ty, name) in &p.samplers {
            let list = if *base == 0 { &p.ps_params } else { &p.vs_params };
            let tex = list
                .iter()
                .find(|sp| sp.is_sampler() && sp.register == *reg)
                .and_then(|sp| match params.values.get(&sp.property) {
                    Some(ParamValue::Texture(Some(t))) => Some(*t),
                    _ => None,
                });
            let view = match ty {
                SamplerType::Cube => self.defaults.cube.clone(),
                SamplerType::Volume => self.defaults.volume.clone(),
                _ if name == "g_WorldLightMapSampler" && self.world_lightmap.is_some() => {
                    self.world_lightmap.clone().unwrap()
                }
                _ => match tex.and_then(&mut *textures) {
                    Some(v) => v,
                    None if name.contains("Normal") => self.defaults.flat_normal.clone(),
                    // unset emissive glows nothing (CHR_Detail_DoNotUse: Layer0emisive)
                    None if name.to_ascii_lowercase().contains("emis") => self.defaults.black.clone(),
                    None => self.defaults.white.clone(),
                },
            };
            if name == "g_DepthSampler" {
                depth_binding = Some(base + 2 * reg);
            }
            views.push((base + 2 * reg, view));
        }
        let mut entries = Vec::new();
        for (binding, view) in &views {
            entries.push(wgpu::BindGroupEntry { binding: *binding, resource: wgpu::BindingResource::TextureView(view) });
            entries.push(wgpu::BindGroupEntry {
                binding: binding + 1,
                resource: wgpu::BindingResource::Sampler(&r.sampler),
            });
        }
        let textures = r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("retail textures"),
            layout: &p.tex_layout,
            entries: &entries,
        });
        Material { program: pi, consts, vs_mat, ps_mat, textures, two_sided, blended, views, depth_binding }
    }

    /// Uploads this frame's engine globals for every program.
    pub fn update_frame(&self, r: &Renderer, env: &Environment, fv: &FrameView) {
        for p in &self.programs {
            let mut vs = ConstBlock::new(wgsl::VS_CONSTS);
            let mut ps = ConstBlock::new(wgsl::PS_CONSTS);
            fill_globals(&mut vs, &p.vs_globals, env, fv);
            fill_globals(&mut ps, &p.ps_globals, env, fv);
            r.queue.write_buffer(&p.vs_frame, 0, &vs.bytes());
            r.queue.write_buffer(&p.ps_frame, 0, &ps.bytes());
        }
    }

    /// For a material that reads `g_DepthSampler`: its textures with the
    /// scene-depth copy bound there.
    pub fn textures_with_depth(&self, r: &Renderer, m: &Material, depth: &wgpu::TextureView) -> Option<wgpu::BindGroup> {
        let slot = m.depth_binding?;
        let mut entries = Vec::new();
        for (binding, view) in &m.views {
            let view = if *binding == slot { depth } else { view };
            entries.push(wgpu::BindGroupEntry { binding: *binding, resource: wgpu::BindingResource::TextureView(view) });
            entries.push(wgpu::BindGroupEntry { binding: binding + 1, resource: wgpu::BindingResource::Sampler(&r.sampler) });
        }
        Some(r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("retail textures + depth"),
            layout: &self.programs[m.program].tex_layout,
            entries: &entries,
        }))
    }

    /// A skinned draw's own VS frame buffer and consts bind group.
    pub fn skinned_consts(&self, r: &Renderer, m: &Material) -> (wgpu::Buffer, wgpu::BindGroup) {
        let p = &self.programs[m.program];
        let buf = r.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("retail skinned frame"),
            size: ConstBlock::new(wgsl::VS_CONSTS).bytes().len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("retail skinned consts"),
            layout: &self.consts_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: p.ps_frame.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: m.vs_mat.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: m.ps_mat.as_entire_binding() },
            ],
        });
        (buf, bg)
    }

    /// Fills a skinned draw's frame buffer: globals, then `palette` (mesh
    /// space -> model space per palette slot) as float4x3 rows from c120.
    pub fn write_skinned_frame(&self, r: &Renderer, m: &Material, buf: &wgpu::Buffer, env: &Environment, fv: &FrameView, palette: &[Mat4]) {
        let mut vs = ConstBlock::new(wgsl::VS_CONSTS);
        fill_globals(&mut vs, &self.programs[m.program].vs_globals, env, fv);
        for (k, b) in palette.iter().take(acb_world::skin::PALETTE_SIZE).enumerate() {
            for row in 0..3 {
                vs.set(BONE_REG + 3 * k as u32 + row as u32, b.row(row).to_array());
            }
        }
        r.queue.write_buffer(buf, 0, &vs.bytes());
    }

    pub fn pipeline(&self, m: &Material) -> &wgpu::RenderPipeline {
        &self.programs[m.program].pipelines[m.two_sided as usize]
    }
}

/// Per-instance rows: WVP rows then World rows (`c[k]` = row k of the
/// column-vector matrix).
pub fn instance_rows(view_proj: Mat4, worlds: &[Mat4]) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(worlds.len() * 8);
    for w in worlds {
        let wvp = view_proj * *w;
        for m in [wvp, *w] {
            for k in 0..4 {
                out.push(m.row(k).to_array());
            }
        }
    }
    out
}
