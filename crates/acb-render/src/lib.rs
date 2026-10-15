//! wgpu renderer for ACB maps.
//!
//! [`Scene::load`] uploads every static mesh instance of an [`Archive`]:
//! one vertex/index buffer per unique Mesh, one instance buffer of entity
//! matrices per Mesh, one instanced draw per submesh. Materials bind the
//! diffuse texture in its native BC format.
//!
//! With [`ShadingMode::Retail`] materials whose template and vertex format
//! translate are drawn by the game's own shaders ([`retail`]); everything
//! else keeps the placeholder `basic.wgsl` shading.
//!
//! Skinned meshes ([`Scene::add_skinned`]) are retail-only; the caller poses
//! them with per-bone skin matrices ([`Scene::set_skinned_pose`]).

mod retail;

use std::collections::HashMap;
use std::ops::Range;

use acb_world::{Archive, MaterialOverride, PixelFormat, SkinnedMesh, Texture};

/// Cache identity of a material override ([`MaterialOverride::key`]).
type MaterialKey = (Option<u32>, Vec<(u32, [u32; 4])>);
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

pub use retail::{Environment, RetailStats};
pub use wgpu;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    color: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view_proj: [f32; 16],
    eye: [f32; 4],
    sun_dir: [f32; 4],
    fog: [f32; 4],
}

/// Device with the features the renderer needs, no surface.
pub async fn headless_device() -> anyhow::Result<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        })
        .await?;
    request_device(&adapter).await
}

pub async fn request_device(adapter: &wgpu::Adapter) -> anyhow::Result<(wgpu::Device, wgpu::Queue)> {
    Ok(adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("acb"),
            required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
            // the retail path feeds instance matrices through locations 24..31
            required_limits: wgpu::Limits {
                max_vertex_attributes: adapter.limits().max_vertex_attributes.min(32),
                ..wgpu::Limits::default()
            },
            ..Default::default()
        })
        .await?)
}

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub fov_y: f32,
    pub aspect: f32,
}

impl Camera {
    pub fn view_proj(&self) -> Mat4 {
        let view = glam::camera::rh::view::look_at_mat4(self.eye, self.target, Vec3::Z);
        // wgpu clip space is D3D's: depth 0..1
        let proj = glam::camera::rh::proj::directx::perspective(self.fov_y, self.aspect, NEAR, FAR);
        proj * view
    }
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    instances: wgpu::Buffer,
    instance_count: u32,
    submeshes: Vec<(Range<u32>, MaterialRef)>,
    /// Undecoded vertex buffer for the retail shaders.
    raw_vertices: Option<wgpu::Buffer>,
    /// Retail per-instance rows (WVP, World), rewritten each frame.
    retail_instances: Option<(wgpu::Buffer, Vec<Mat4>)>,
}

/// A posed skinned mesh: one draw per submesh, each with its own palette.
struct GpuSkinned {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    instances: wgpu::Buffer,
    world: Mat4,
    /// Per mesh bone: mesh space -> model space.
    skin: Vec<Mat4>,
    draws: Vec<SkinnedDraw>,
}

struct SkinnedDraw {
    range: Range<u32>,
    material: usize,
    /// Palette slot -> mesh bone.
    palette: Vec<u16>,
    frame: wgpu::Buffer,
    consts: wgpu::BindGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaterialRef {
    Basic(usize),
    Retail(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShadingMode {
    #[default]
    Placeholder,
    Retail,
}

struct GpuMaterial {
    bind_group: wgpu::BindGroup,
    alpha_test: bool,
    two_sided: bool,
    /// Alpha blended (decals, foliage cards): drawn after opaque geometry.
    blended: bool,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    frame_buf: wgpu::Buffer,
    frame_bg: wgpu::BindGroup,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// [opaque, alpha test, blended] x [culled, two-sided]
    pipelines: [[wgpu::RenderPipeline; 2]; 3],
    color_format: wgpu::TextureFormat,
    /// Opaque depth -> inverse view distance, for shaders that read the
    /// scene depth (`g_DepthSampler`: water, soft particles).
    depth_blit: wgpu::RenderPipeline,
    depth_blit_layout: wgpu::BindGroupLayout,
    depth_copy: std::cell::RefCell<Option<((u32, u32), wgpu::TextureView)>>,
}

/// Camera clip planes (see [`Camera::view_proj`]).
pub const NEAR: f32 = 0.25;
pub const FAR: f32 = 8000.0;
/// Format of the scene-depth copy: 1 / view distance fits half floats.
const DEPTH_COPY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

#[derive(Debug, Default)]
pub struct LoadStats {
    pub meshes: usize,
    pub skipped_meshes: usize,
    pub instances: usize,
    pub triangles: usize,
    pub materials: usize,
    pub textures: usize,
    pub missing_textures: usize,
    pub disabled_submeshes: usize,
    /// Material count per BlendMode value.
    pub blend_modes: std::collections::BTreeMap<u32, usize>,
    pub retail: Option<RetailStats>,
}

pub struct Scene {
    meshes: Vec<GpuMesh>,
    materials: Vec<GpuMaterial>,
    retail: Option<retail::Retail>,
    retail_materials: Vec<retail::Material>,
    /// (material, TextureSet override, vertex format) -> retail material,
    /// None when it fell back.
    retail_slots: HashMap<(u32, MaterialKey, u32), Option<usize>>,
    /// Textures uploaded unconverted for the retail shaders.
    gamma_views: HashMap<u32, Option<wgpu::TextureView>>,
    skinned: Vec<GpuSkinned>,
    pub environment: Environment,
    /// World-space bounds of every instance origin.
    pub bounds: (Vec3, Vec3),
    /// The sky dome mesh (LayeredSky layer), kept centred on the eye and
    /// scaled out behind everything else.
    sky: Option<(usize, f32)>,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("basic"),
            source: wgpu::ShaderSource::Wgsl(include_str!("basic.wgsl").into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buf.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("basic"),
            bind_group_layouts: &[Some(&frame_layout), Some(&material_layout)],
            immediate_size: 0,
        });

        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 7 => Unorm8x4];
        let instance_attrs =
            wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4];
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vertex_attrs,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attrs,
            }),
        ];
        let pipeline = |fs: &str, cull: Option<wgpu::Face>, blended: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
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
                    depth_write_enabled: Some(!blended),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    // pull decals in front of the surface they lie on
                    bias: if blended {
                        wgpu::DepthBiasState { constant: -2, slope_scale: -1.0, clamp: 0.0 }
                    } else {
                        Default::default()
                    },
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: blended.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let back = Some(wgpu::Face::Back);
        let pipelines = [
            [pipeline("fs_opaque", back, false), pipeline("fs_opaque", None, false)],
            [pipeline("fs_alpha_test", back, false), pipeline("fs_alpha_test", None, false)],
            [pipeline("fs_blended", back, true), pipeline("fs_blended", None, true)],
        ];
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let depth_blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth blit"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let blit_src = format!(
            r#"
@group(0) @binding(0) var src: texture_depth_2d;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}}
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {{
    let d = textureLoad(src, vec2<i32>(pos.xy), 0);
    // 0..1 depth of a directx-style perspective -> 1 / view distance
    return vec4<f32>(({far} - d * ({far} - {near})) / ({near} * {far}), 0.0, 0.0, 1.0);
}}
"#,
            near = NEAR,
            far = FAR
        );
        let blit_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth blit"),
            source: wgpu::ShaderSource::Wgsl(blit_src.into()),
        });
        let blit_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth blit"),
            bind_group_layouts: &[Some(&depth_blit_layout)],
            immediate_size: 0,
        });
        let depth_blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth blit"),
            layout: Some(&blit_pl),
            vertex: wgpu::VertexState {
                module: &blit_module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_module,
                entry_point: Some("fs"),
                targets: &[Some(DEPTH_COPY_FORMAT.into())],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            device,
            queue,
            frame_buf,
            frame_bg,
            material_layout,
            sampler,
            pipelines,
            color_format,
            depth_blit,
            depth_blit_layout,
            depth_copy: Default::default(),
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// `srgb`: sample linear values (placeholder shading); the retail
    /// shaders work on the stored gamma values, like D3D9 without SRGBTEXTURE.
    fn upload_texture(&self, t: &Texture, srgb: bool) -> Option<wgpu::Texture> {
        use wgpu::TextureFormat as F;
        let (format, block_bytes) = match (t.format, srgb) {
            (PixelFormat::Bgra8, true) => (F::Bgra8UnormSrgb, 4),
            (PixelFormat::Bc1, true) => (F::Bc1RgbaUnormSrgb, 8),
            (PixelFormat::Bc2, true) => (F::Bc2RgbaUnormSrgb, 16),
            (PixelFormat::Bc3, true) => (F::Bc3RgbaUnormSrgb, 16),
            (PixelFormat::Bgra8, false) => (F::Bgra8Unorm, 4),
            (PixelFormat::Bc1, false) => (F::Bc1RgbaUnorm, 8),
            (PixelFormat::Bc2, false) => (F::Bc2RgbaUnorm, 16),
            (PixelFormat::Bc3, false) => (F::Bc3RgbaUnorm, 16),
        };
        let block = if t.format == PixelFormat::Bgra8 { 1 } else { 4 };
        if t.width % block != 0 || t.height % block != 0 {
            return None;
        }
        // a BC chain stops being addressable once a level is smaller than a block
        let mut mips = 0;
        let (mut w, mut h) = (t.width, t.height);
        while mips < t.mips && (block == 1 || (w % 4 == 0 && h % 4 == 0) || mips == 0) {
            mips += 1;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (mut w, mut h, mut offset) = (t.width, t.height, 0usize);
        for level in 0..mips {
            let (bw, bh) = (w.div_ceil(block), h.div_ceil(block));
            let size = (bw * bh * block_bytes) as usize;
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: level,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &t.data[offset..offset + size],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bw * block_bytes),
                    rows_per_image: Some(bh),
                },
                wgpu::Extent3d { width: bw * block, height: bh * block, depth_or_array_layers: 1 },
            );
            offset += size;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        Some(tex)
    }

    fn material(
        &self,
        view: &wgpu::TextureView,
        alpha_ref: f32,
        alpha_test: bool,
        two_sided: bool,
        blended: bool,
    ) -> GpuMaterial {
        let params = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[alpha_ref, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: params.as_entire_binding() },
            ],
        });
        GpuMaterial { bind_group, alpha_test, two_sided, blended }
    }

    fn white_texture(&self) -> wgpu::Texture {
        self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("white"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[200, 200, 200, 255],
        )
    }

    /// The scene-depth copy for this frame size (recreated on resize).
    fn depth_copy_view(&self, size: (u32, u32)) -> wgpu::TextureView {
        let mut slot = self.depth_copy.borrow_mut();
        if let Some((s, v)) = &*slot {
            if *s == size {
                return v.clone();
            }
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene depth copy"),
            size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_COPY_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        *slot = Some((size, view.clone()));
        view
    }

    /// Draws `scene`: opaque and alpha-tested geometry, then a copy of the
    /// depth for shaders that read it, then blended geometry. `depth` must
    /// allow texture binding; `size` is the target's.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        scene: &Scene,
        camera: &Camera,
    ) {
        let env = &scene.environment;
        let fog = env.fog_color.to_array();
        let sun = env.sun_dir;
        let frame = FrameUniform {
            view_proj: camera.view_proj().to_cols_array(),
            eye: camera.eye.extend(1.0).to_array(),
            sun_dir: sun.extend(0.0).to_array(),
            fog: [fog[0], fog[1], fog[2], 0.0006],
        };
        self.queue.write_buffer(&self.frame_buf, 0, bytemuck::bytes_of(&frame));
        let fv = retail::FrameView::new(camera, size);
        if let Some(retail) = &scene.retail {
            retail.update_frame(self, env, &fv);
            let view_proj = camera.view_proj();
            for (i, m) in scene.meshes.iter().enumerate() {
                if let Some((buf, worlds)) = &m.retail_instances {
                    let sky = scene.sky.filter(|s| s.0 == i).map(|s| [Mat4::from_scale_rotation_translation(Vec3::splat(s.1), glam::Quat::IDENTITY, camera.eye)]);
                    let worlds = match &sky {
                        Some(w) => &w[..],
                        None => &worlds[..],
                    };
                    let rows = retail::instance_rows(view_proj, worlds);
                    self.queue.write_buffer(buf, 0, bytemuck::cast_slice(&rows));
                }
            }
            for s in &scene.skinned {
                let rows = retail::instance_rows(view_proj, &[s.world]);
                self.queue.write_buffer(&s.instances, 0, bytemuck::cast_slice(&rows));
                for d in &s.draws {
                    let palette: Vec<Mat4> =
                        d.palette.iter().map(|&b| s.skin.get(b as usize).copied().unwrap_or(Mat4::IDENTITY)).collect();
                    let mat = &scene.retail_materials[d.material];
                    retail.write_skinned_frame(self, mat, &d.frame, env, &fv, &palette);
                }
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("opaque"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: fog[0] as f64,
                            g: fog[1] as f64,
                            b: fog[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            self.draw(&mut pass, scene, false, &HashMap::new());
        }

        // scene depth for the blended pass
        let copy = self.depth_copy_view(size);
        {
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("depth blit"),
                layout: &self.depth_blit_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(depth) }],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("depth copy"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &copy,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.depth_blit);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        // blended materials that read the depth get it bound this frame
        let mut depth_bound: HashMap<usize, wgpu::BindGroup> = HashMap::new();
        if let Some(retail) = &scene.retail {
            for (i, m) in scene.retail_materials.iter().enumerate().filter(|(_, m)| m.blended) {
                if let Some(bg) = retail.textures_with_depth(self, m, &copy) {
                    depth_bound.insert(i, bg);
                }
            }
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blended"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        self.draw(&mut pass, scene, true, &depth_bound);
    }

    /// One pass's draws: the opaque or the blended materials.
    fn draw(&self, pass: &mut wgpu::RenderPass, scene: &Scene, blended_pass: bool, depth_bound: &HashMap<usize, wgpu::BindGroup>) {
        for m in &scene.meshes {
            pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
            for (range, mat) in &m.submeshes {
                match *mat {
                    MaterialRef::Basic(i) => {
                        let mat = &scene.materials[i];
                        if mat.blended != blended_pass {
                            continue;
                        }
                        let kind = if mat.blended { 2 } else { mat.alpha_test as usize };
                        pass.set_pipeline(&self.pipelines[kind][mat.two_sided as usize]);
                        pass.set_bind_group(0, &self.frame_bg, &[]);
                        pass.set_bind_group(1, &mat.bind_group, &[]);
                        pass.set_vertex_buffer(0, m.vertices.slice(..));
                        pass.set_vertex_buffer(1, m.instances.slice(..));
                    }
                    MaterialRef::Retail(i) => {
                        let (Some(retail), Some(raw), Some((inst, _))) = (&scene.retail, &m.raw_vertices, &m.retail_instances)
                        else {
                            continue;
                        };
                        let mat = &scene.retail_materials[i];
                        if mat.blended != blended_pass {
                            continue;
                        }
                        pass.set_pipeline(retail.pipeline(mat));
                        pass.set_bind_group(0, &mat.consts, &[]);
                        pass.set_bind_group(1, depth_bound.get(&i).unwrap_or(&mat.textures), &[]);
                        pass.set_vertex_buffer(0, raw.slice(..));
                        pass.set_vertex_buffer(1, inst.slice(..));
                    }
                }
                pass.draw_indexed(range.clone(), 0, 0..m.instance_count);
            }
        }
        let Some(retail) = &scene.retail else { return };
        for s in &scene.skinned {
            pass.set_index_buffer(s.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_vertex_buffer(0, s.vertices.slice(..));
            pass.set_vertex_buffer(1, s.instances.slice(..));
            for d in &s.draws {
                let mat = &scene.retail_materials[d.material];
                if mat.blended != blended_pass {
                    continue;
                }
                pass.set_pipeline(retail.pipeline(mat));
                pass.set_bind_group(0, &d.consts, &[]);
                pass.set_bind_group(1, depth_bound.get(&d.material).unwrap_or(&mat.textures), &[]);
                pass.draw_indexed(d.range.clone(), 0, 0..1);
            }
        }
    }
}

impl Scene {
    pub fn load(r: &Renderer, archive: &Archive, shading: ShadingMode) -> anyhow::Result<(Self, LoadStats)> {
        let mut stats = LoadStats::default();
        let mut retail = (shading == ShadingMode::Retail).then(|| retail::Retail::new(r));
        // the map's own fog and world lightmap (its first, daytime one)
        let mut environment = Environment::default();
        let mut sky_layer: Option<(u32, u32)> = None;
        if let Some(a) = archive.ambiance() {
            sky_layer = a.sky_layers.first().copied().filter(|_| std::env::var_os("ACBMP_NO_SKY").is_none());
            if a.time > 0.0 {
                environment.time_of_day = a.time;
            }
            if a.fog_enabled && a.fog_far > a.fog_near {
                environment.fog_color = Vec3::from(a.fog_color);
                environment.fog_start = a.fog_near;
                environment.fog_range = a.fog_far - a.fog_near;
                environment.fog_max = (a.fog_density * a.fog_intensity).clamp(0.0, 1.0);
            }
            let lm = a.lightmaps.first().filter(|_| std::env::var_os("ACBMP_NO_LIGHTMAP").is_none());
            if let (Some(lm), Some(retail)) = (lm, retail.as_mut()) {
                let view = archive.texture(lm.texture).ok().flatten().and_then(|t| r.upload_texture(&t, false));
                if let Some(tex) = view {
                    retail.world_lightmap = Some(tex.create_view(&Default::default()));
                    environment.lightmap = Some(retail::Lightmap {
                        uv_scale: lm.uv_scale,
                        z_min: lm.z_min,
                        z_range: lm.z_max - lm.z_min,
                        normal_offset: 0.5,
                        fade: 0.5,
                        intensity: 2.0,
                        weight: 3.0,
                    });
                }
            }
        }
        let mut retail_materials = Vec::new();
        let mut retail_slots: HashMap<(u32, MaterialKey, u32), Option<usize>> = HashMap::new();
        let mut gamma_views: HashMap<u32, Option<wgpu::TextureView>> = HashMap::new();
        let white = r.white_texture().create_view(&Default::default());
        let mut materials = vec![r.material(&white, 0.5, false, false, false)];
        let mut material_slots: HashMap<u32, usize> = HashMap::from([(0, 0)]);
        let mut texture_views: HashMap<u32, Option<wgpu::TextureView>> = HashMap::new();

        // group instance matrices by mesh
        let mut by_mesh: HashMap<u32, Vec<[f32; 16]>> = HashMap::new();
        let mut order = Vec::new();
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for inst in archive.instances().into_iter().filter(|i| !i.out_of_bounds) {
            let t = Vec3::new(inst.matrix[12], inst.matrix[13], inst.matrix[14]);
            lo = lo.min(t);
            hi = hi.max(t);
            by_mesh.entry(inst.mesh).or_insert_with(|| {
                order.push(inst.mesh);
                Vec::new()
            });
            by_mesh.get_mut(&inst.mesh).unwrap().push(inst.matrix);
        }
        // the sky dome: one instance, placed per frame
        if let Some((mesh, _)) = sky_layer.filter(|(m, _)| !by_mesh.contains_key(m)) {
            order.push(mesh);
            by_mesh.insert(mesh, vec![Mat4::IDENTITY.to_cols_array()]);
        }
        let sky_uid = sky_layer.map(|l| l.0);

        let mut meshes = Vec::new();
        let mut sky = None;
        for uid in order {
            let Some(mesh) = archive.static_mesh(uid)? else {
                stats.skipped_meshes += 1;
                continue;
            };
            if mesh.indices.is_empty() {
                stats.skipped_meshes += 1;
                continue;
            }
            let mut submeshes = Vec::new();
            let is_sky = Some(uid) == sky_uid;
            if is_sky {
                let radius = mesh.positions.iter().map(|p| Vec3::from(*p).length()).fold(0.0f32, f32::max);
                sky = Some((meshes.len(), if radius > 0.0 { 0.9 * FAR / radius } else { 1.0 }));
            }
            for s in &mesh.submeshes {
                if s.index_count == 0 {
                    continue;
                }
                // a sky layer draws with its MaterialOverride
                let s = &acb_world::mesh::Submesh {
                    material: sky_layer.filter(|l| is_sky && l.1 != 0).map_or(s.material, |l| l.1),
                    ..s.clone()
                };
                if archive.material(s.material).is_some_and(|m| m.disabled) {
                    stats.disabled_submeshes += 1;
                    continue;
                }
                if let Some(retail) = &mut retail {
                    let slot = retail_slot(
                        r,
                        archive,
                        retail,
                        &mut retail_materials,
                        &mut retail_slots,
                        &mut gamma_views,
                        s.material,
                        &MaterialOverride::default(),
                        (mesh.format, mesh.stride),
                    );
                    if let Some(slot) = slot {
                        submeshes.push((s.first_index..s.first_index + s.index_count, MaterialRef::Retail(slot)));
                        continue;
                    }
                }
                let slot = match material_slots.get(&s.material) {
                    Some(&i) => i,
                    None => {
                        let info = archive.material(s.material);
                        let view = info.as_ref().and_then(|i| i.diffuse).and_then(|t| {
                            texture_views
                                .entry(t)
                                .or_insert_with(|| {
                                    let tex = archive.texture(t).ok().flatten()?;
                                    let gpu = r.upload_texture(&tex, true)?;
                                    stats.textures += 1;
                                    Some(gpu.create_view(&Default::default()))
                                })
                                .clone()
                        });
                        if view.is_none() {
                            stats.missing_textures += 1;
                        }
                        let (alpha_test, alpha_ref, two_sided, blended) = info
                            .as_ref()
                            .map(|i| (i.alpha_test, i.alpha_ref as f32 / 255.0, i.two_sided, i.blend_mode != 0))
                            .unwrap_or((false, 0.5, false, false));
                        *stats.blend_modes.entry(info.as_ref().map_or(0, |i| i.blend_mode)).or_default() += 1;
                        materials.push(r.material(
                            view.as_ref().unwrap_or(&white),
                            alpha_ref,
                            alpha_test,
                            two_sided,
                            blended,
                        ));
                        material_slots.insert(s.material, materials.len() - 1);
                        materials.len() - 1
                    }
                };
                submeshes.push((s.first_index..s.first_index + s.index_count, MaterialRef::Basic(slot)));
            }
            let vertices: Vec<Vertex> = (0..mesh.positions.len())
                .map(|i| Vertex {
                    pos: mesh.positions[i],
                    normal: mesh.normals[i],
                    uv: mesh.uvs[i],
                    color: mesh.colors.as_ref().map_or([255; 4], |c| c[i]),
                })
                .collect();
            let mats = &by_mesh[&uid];
            let init = |label: &str, contents: &[u8], usage| {
                r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
            };
            stats.meshes += 1;
            stats.instances += mats.len();
            stats.triangles += mesh.indices.len() / 3 * mats.len();
            let any_retail = submeshes.iter().any(|(_, m)| matches!(m, MaterialRef::Retail(_)));
            let (raw_vertices, retail_instances) = if any_retail {
                let worlds: Vec<Mat4> = mats.iter().map(Mat4::from_cols_array).collect();
                let buf = r.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("retail instances"),
                    size: retail::INSTANCE_STRIDE * worlds.len() as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                (Some(init("raw vertices", &mesh.raw, wgpu::BufferUsages::VERTEX)), Some((buf, worlds)))
            } else {
                (None, None)
            };
            meshes.push(GpuMesh {
                raw_vertices,
                retail_instances,
                vertices: init("vertices", bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX),
                indices: init("indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX),
                instances: init("instances", bytemuck::cast_slice(mats), wgpu::BufferUsages::VERTEX),
                instance_count: mats.len() as u32,
                submeshes,
            });
        }
        stats.materials = materials.len();
        stats.retail = retail.as_mut().map(|r| {
            r.stats.programs = r.programs.len();
            std::mem::take(&mut r.stats)
        });
        let scene = Self {
            meshes,
            materials,
            retail,
            retail_materials,
            retail_slots,
            gamma_views,
            skinned: Vec::new(),
            environment,
            bounds: (lo, hi),
            sky,
        };
        Ok((scene, stats))
    }

    /// A scene with nothing in it yet, drawn with the retail shaders.
    pub fn empty(r: &Renderer) -> Self {
        let white = r.white_texture().create_view(&Default::default());
        Self {
            meshes: Vec::new(),
            materials: vec![r.material(&white, 0.5, false, false, false)],
            retail: Some(retail::Retail::new(r)),
            retail_materials: Vec::new(),
            retail_slots: HashMap::new(),
            gamma_views: HashMap::new(),
            skinned: Vec::new(),
            environment: Environment::default(),
            bounds: (Vec3::ZERO, Vec3::ZERO),
            sky: None,
        }
    }

    /// Adds a skinned mesh drawn with `materials` (one per submesh, each
    /// with an optional TextureSet override); returns
    /// its handle for [`Scene::set_skinned_pose`], or `None` without retail
    /// shading. Submeshes whose material does not translate are skipped.
    pub fn add_skinned(
        &mut self,
        r: &Renderer,
        archive: &Archive,
        mesh: &SkinnedMesh,
        materials: &[(u32, MaterialOverride)],
    ) -> Option<usize> {
        let retail = self.retail.as_mut()?;
        let mut draws = Vec::new();
        for (s, (material, ov)) in mesh.submeshes.iter().zip(materials) {
            let material = *material;
            if s.index_count == 0 || archive.material(material).is_some_and(|m| m.disabled) {
                continue;
            }
            let Some(slot) = retail_slot(
                r,
                archive,
                retail,
                &mut self.retail_materials,
                &mut self.retail_slots,
                &mut self.gamma_views,
                material,
                ov,
                (mesh.format, mesh.stride),
            ) else {
                continue;
            };
            let (frame, consts) = retail.skinned_consts(r, &self.retail_materials[slot]);
            draws.push(SkinnedDraw {
                range: s.first_index..s.first_index + s.index_count,
                material: slot,
                palette: s.palette.clone(),
                frame,
                consts,
            });
        }
        let init = |label: &str, contents: &[u8], usage| {
            r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        self.skinned.push(GpuSkinned {
            vertices: init("skinned vertices", &mesh.raw, wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST),
            indices: init("skinned indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX),
            instances: r.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("skinned instance"),
                size: retail::INSTANCE_STRIDE,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            world: Mat4::IDENTITY,
            skin: vec![Mat4::IDENTITY; mesh.bones.len()],
            draws,
        });
        Some(self.skinned.len() - 1)
    }

    /// Replaces a skinned mesh's vertex buffer (same layout and size), e.g.
    /// a simulated cloth posed on the CPU.
    pub fn set_skinned_vertices(&self, r: &Renderer, handle: usize, raw: &[u8]) {
        if let Some(s) = self.skinned.get(handle) {
            if raw.len() as u64 <= s.vertices.size() {
                r.queue.write_buffer(&s.vertices, 0, raw);
            }
        }
    }

    /// Places a skinned mesh: `world` = model -> world, `skin` = per mesh
    /// bone, mesh space -> model space.
    pub fn set_skinned_pose(&mut self, handle: usize, world: Mat4, skin: &[Mat4]) {
        if let Some(s) = self.skinned.get_mut(handle) {
            s.world = world;
            s.skin.clear();
            s.skin.extend_from_slice(skin);
        }
    }

    /// Retail material statistics so far.
    pub fn retail_stats(&self) -> Option<&RetailStats> {
        self.retail.as_ref().map(|r| &r.stats)
    }
}

/// The retail material slot of `material` drawn with a vertex format,
/// translating its program on first use.
#[allow(clippy::too_many_arguments)]
fn retail_slot(
    r: &Renderer,
    archive: &Archive,
    retail: &mut retail::Retail,
    materials: &mut Vec<retail::Material>,
    slots: &mut HashMap<(u32, MaterialKey, u32), Option<usize>>,
    gamma_views: &mut HashMap<u32, Option<wgpu::TextureView>>,
    material: u32,
    ov: &MaterialOverride,
    (format, stride): (u32, usize),
) -> Option<usize> {
    *slots.entry((material, ov.key(), format)).or_insert_with(|| {
        let mut load = |t: u32| {
            gamma_views
                .entry(t)
                .or_insert_with(|| {
                    let tex = archive.texture(t).ok().flatten()?;
                    let gpu = r.upload_texture(&tex, false)?;
                    Some(gpu.create_view(&Default::default()))
                })
                .clone()
        };
        let m = retail.material(r, archive, material, ov, (format, stride), &mut load)?;
        materials.push(m);
        Some(materials.len() - 1)
    })
}

/// Renders one frame offscreen and returns tightly packed RGBA8 pixels.
pub fn render_to_rgba(r: &Renderer, scene: &Scene, camera: &Camera, width: u32, height: u32) -> Vec<u8> {
    let d = r.device();
    // the renderer's own target format (a window's may be BGRA)
    let format = r.color_format;
    let bgra = matches!(format, wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm);
    let target = |format, usage| {
        d.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = target(format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth = target(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = d.create_command_encoder(&Default::default());
    r.render(
        &mut enc,
        &color.create_view(&Default::default()),
        &depth.create_view(&Default::default()),
        (width, height),
        scene,
        camera,
    );
    enc.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    r.queue().submit([enc.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    d.poll(wgpu::PollType::wait_indefinitely()).ok();
    let data = slice.get_mapped_range().expect("readback mapped");
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        out.extend_from_slice(&data[y * row as usize..y * row as usize + width as usize * 4]);
    }
    for p in out.chunks_exact_mut(4) {
        if bgra {
            p.swap(0, 2);
        }
        // shaders leave height masks etc. in alpha; the screen ignores it
        p[3] = 255;
    }
    out
}
