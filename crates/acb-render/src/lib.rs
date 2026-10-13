//! wgpu renderer for ACB maps.
//!
//! [`Scene::load`] uploads every static mesh instance of an [`Archive`]:
//! one vertex/index buffer per unique Mesh, one instance buffer of entity
//! matrices per Mesh, one instanced draw per submesh. Materials bind the
//! diffuse texture in its native BC format. Shading is a placeholder until
//! the retail SM3 shaders are translated.

use std::collections::HashMap;
use std::ops::Range;

use acb_world::{Archive, PixelFormat, Texture};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

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
            required_limits: wgpu::Limits::default(),
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
        let proj = glam::camera::rh::proj::directx::perspective(self.fov_y, self.aspect, 0.25, 8000.0);
        proj * view
    }
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    instances: wgpu::Buffer,
    instance_count: u32,
    submeshes: Vec<(Range<u32>, usize)>,
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
}

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
}

pub struct Scene {
    meshes: Vec<GpuMesh>,
    materials: Vec<GpuMaterial>,
    /// World-space bounds of every instance origin.
    pub bounds: (Vec3, Vec3),
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
        Self { device, queue, frame_buf, frame_bg, material_layout, sampler, pipelines }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    fn upload_texture(&self, t: &Texture) -> Option<wgpu::Texture> {
        let (format, block_bytes) = match t.format {
            PixelFormat::Bgra8 => (wgpu::TextureFormat::Bgra8UnormSrgb, 4),
            PixelFormat::Bc1 => (wgpu::TextureFormat::Bc1RgbaUnormSrgb, 8),
            PixelFormat::Bc2 => (wgpu::TextureFormat::Bc2RgbaUnormSrgb, 16),
            PixelFormat::Bc3 => (wgpu::TextureFormat::Bc3RgbaUnormSrgb, 16),
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

    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        scene: &Scene,
        camera: &Camera,
    ) {
        let fog = [0.62, 0.68, 0.74];
        let sun = Vec3::new(0.4, -0.5, 0.75).normalize();
        let frame = FrameUniform {
            view_proj: camera.view_proj().to_cols_array(),
            eye: camera.eye.extend(1.0).to_array(),
            sun_dir: sun.extend(0.0).to_array(),
            fog: [fog[0], fog[1], fog[2], 0.0006],
        };
        self.queue.write_buffer(&self.frame_buf, 0, bytemuck::bytes_of(&frame));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main"),
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
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, &self.frame_bg, &[]);
        for blended_pass in [false, true] {
            for m in &scene.meshes {
                pass.set_vertex_buffer(0, m.vertices.slice(..));
                pass.set_vertex_buffer(1, m.instances.slice(..));
                pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                for (range, mat) in &m.submeshes {
                    let mat = &scene.materials[*mat];
                    if mat.blended != blended_pass {
                        continue;
                    }
                    let kind = if mat.blended { 2 } else { mat.alpha_test as usize };
                    pass.set_pipeline(&self.pipelines[kind][mat.two_sided as usize]);
                    pass.set_bind_group(1, &mat.bind_group, &[]);
                    pass.draw_indexed(range.clone(), 0, 0..m.instance_count);
                }
            }
        }
    }
}

impl Scene {
    pub fn load(r: &Renderer, archive: &Archive) -> anyhow::Result<(Self, LoadStats)> {
        let mut stats = LoadStats::default();
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

        let mut meshes = Vec::new();
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
            for s in &mesh.submeshes {
                if s.index_count == 0 {
                    continue;
                }
                if archive.material(s.material).is_some_and(|m| m.disabled) {
                    stats.disabled_submeshes += 1;
                    continue;
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
                                    let gpu = r.upload_texture(&tex)?;
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
                submeshes.push((s.first_index..s.first_index + s.index_count, slot));
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
            meshes.push(GpuMesh {
                vertices: init("vertices", bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX),
                indices: init("indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX),
                instances: init("instances", bytemuck::cast_slice(mats), wgpu::BufferUsages::VERTEX),
                instance_count: mats.len() as u32,
                submeshes,
            });
        }
        stats.materials = materials.len();
        Ok((Self { meshes, materials, bounds: (lo, hi) }, stats))
    }
}

/// Renders one frame offscreen and returns tightly packed RGBA8 pixels.
pub fn render_to_rgba(r: &Renderer, scene: &Scene, camera: &Camera, width: u32, height: u32) -> Vec<u8> {
    let d = r.device();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
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
    let depth = target(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
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
    out
}
