//! Cascaded sun shadows for the retail `LightingKey 0x80` pixel shaders.
//!
//! The shaders take the vertex shader's light-space position
//! (`g_WorldToLightProjVS`: three rows of the light's view matrix), map it
//! into each cascade with `g_ShadowScales{X,Y,Z,W}` / `g_ShadowOffsets*`
//! (atlas u, v and depth, all 0..1), use the last cascade of X, Y, Z, W
//! whose u and v lie inside `g_ShadowCascadesRanges{Min,Max}` (.y/.z/.w for
//! Y/Z/W; X is the fallback), sample the 4x1 atlas `g_ShadowSampler` at
//! u * 0.25 + k * 0.25 with four noise-rotated Poisson taps (radius
//! `g_ShadowNoiseScale` texels of `g_ShadowMapSize`), count `depth <= texel`
//! as lit, add `sat(dist^2 * g_ShadowFadeParams.x + .y)` and multiply by a
//! cloud texture. So W is the finest cascade and X the widest.

use glam::{Mat4, Vec2, Vec3};
use wgpu::util::DeviceExt;

use crate::{Camera, GpuMesh, MaterialRef, Renderer, Scene, DEPTH_FORMAT};

/// Texels per cascade side.
pub const CASCADE_SIZE: u32 = 1024;
/// View distances where the cascades end: W, Z, Y, X.
const SPLITS: [f32; 4] = [8.0, 25.0, 70.0, 200.0];
/// How far beyond a cascade's box towards the sun casters are rendered.
const CASTER_REACH: f32 = 150.0;
/// How far beyond a cascade's box away from the sun receivers can be.
const RECEIVER_REACH: f32 = 400.0;
/// Margin inside a cascade (atlas uv) before the next one takes over.
const MARGIN: f32 = 0.02;

/// This frame's shadow constants.
#[derive(Debug, Clone, Copy)]
pub struct ShadowFrame {
    pub light_view: Mat4,
    /// Per cascade X, Y, Z, W: v8 * scale + offset = (u, v, depth).
    pub scales: [Vec3; 4],
    pub offsets: [Vec3; 4],
    pub atlas_size: Vec2,
    /// `sat(dist^2 * x + y)` fades shadows out.
    pub fade: [f32; 2],
    pub noise_scale: f32,
    pub margin: f32,
}

pub(crate) struct Shadows {
    pub atlas: wgpu::TextureView,
    atlas_tex: wgpu::Texture,
    depth: wgpu::TextureView,
    pub noise: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    uniforms: Vec<wgpu::Buffer>,
    bind_groups: Vec<wgpu::BindGroup>,
}

const SHADER: &str = r#"
@group(0) @binding(0) var<uniform> view_proj: mat4x4<f32>;
struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
};
@vertex fn vs(v: VsIn) -> @builtin(position) vec4<f32> {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    return view_proj * (model * vec4<f32>(v.pos, 1.0));
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(p.z, 0.0, 0.0, 1.0);
}
"#;

impl Shadows {
    pub fn new(r: &Renderer) -> Shadows {
        let d = &r.device;
        let format = r.shadow_format;
        let size = wgpu::Extent3d { width: CASCADE_SIZE * 4, height: CASCADE_SIZE, depth_or_array_layers: 1 };
        let tex = |format, usage| {
            d.create_texture(&wgpu::TextureDescriptor {
                label: Some("shadow atlas"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&Default::default())
        };
        let atlas_tex = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow atlas"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let atlas = atlas_tex.create_view(&Default::default());
        let depth = tex(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);

        // per-pixel rotations for the Poisson taps: (r, g) = 0.5 + 0.5 (cos, sin)
        let mut seed = 0x2545_f491u32;
        let noise: Vec<u8> = (0..64 * 64)
            .flat_map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let a = (seed as f32 / u32::MAX as f32) * std::f32::consts::TAU;
                [((0.5 + 0.5 * a.cos()) * 255.0) as u8, ((0.5 + 0.5 * a.sin()) * 255.0) as u8, 128, 255]
            })
            .collect();
        let noise = d
            .create_texture_with_data(
                &r.queue,
                &wgpu::TextureDescriptor {
                    label: Some("shadow noise"),
                    size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &noise,
            )
            .create_view(&Default::default());

        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniforms: Vec<wgpu::Buffer> = (0..4)
            .map(|_| {
                d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("shadow cascade"),
                    contents: bytemuck::cast_slice(&Mat4::IDENTITY.to_cols_array()),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                })
            })
            .collect();
        let bind_groups = uniforms
            .iter()
            .map(|b| {
                d.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("shadow cascade"),
                    layout: &layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: b.as_entire_binding() }],
                })
            })
            .collect();
        let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x3];
        let instance_attrs = wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4];
        let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<crate::Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex_attrs,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attrs,
                    }),
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            // both faces cast: thin scenery has no back side
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                // receivers compare against the nearest caster: push it back
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Shadows { atlas, atlas_tex, depth, noise, pipeline, uniforms, bind_groups }
    }

    /// Cascades for `camera` with the sun towards `sun_dir`: constants for
    /// the shaders and each cascade's view-projection (X, Y, Z, W).
    pub fn plan(camera: &Camera, sun_dir: Vec3) -> (ShadowFrame, [Mat4; 4]) {
        let dir = sun_dir.normalize_or(Vec3::Z);
        let up = if dir.z.abs() > 0.95 { Vec3::Y } else { Vec3::Z };
        let light_view = glam::camera::rh::view::look_at_mat4(Vec3::ZERO, -dir, up);
        let fwd = (camera.target - camera.eye).normalize_or(Vec3::Y);
        let right = fwd.cross(Vec3::Z).normalize_or(Vec3::X);
        let cup = right.cross(fwd);
        let ty = (camera.fov_y / 2.0).tan();
        let tx = ty * camera.aspect;
        let corners = |d: f32| {
            [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].map(|(sx, sy)| camera.eye + (fwd + right * (sx * tx) + cup * (sy * ty)) * d)
        };
        let mut scales = [Vec3::ZERO; 4];
        let mut offsets = [Vec3::ZERO; 4];
        let mut view_projs = [Mat4::IDENTITY; 4];
        // shader slots X, Y, Z, W take splits widest first
        for (slot, k) in [3usize, 2, 1, 0].into_iter().enumerate() {
            let near = if k == 0 { crate::NEAR } else { SPLITS[k - 1] };
            let pts: Vec<Vec3> = corners(near).into_iter().chain(corners(SPLITS[k])).map(|p| light_view.transform_point3(p)).collect();
            let center = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
            let radius = pts.iter().map(|p| p.distance(center)).fold(0.0f32, f32::max).max(1.0);
            // snap to texels so the shadows do not crawl as the camera moves
            let texel = 2.0 * radius / CASCADE_SIZE as f32;
            let (cx, cy) = ((center.x / texel).floor() * texel, (center.y / texel).floor() * texel);
            let (l, r, b, t) = (cx - radius, cx + radius, cy - radius, cy + radius);
            // view z is negative ahead; distances along the light's view.
            // The shaders pick a cascade by u, v alone, so its depth range
            // must also hold every receiver behind the box, not just the
            // camera slice.
            let (n, f) = (-(center.z + radius) - CASTER_REACH, -(center.z - radius) + RECEIVER_REACH);
            view_projs[slot] = glam::camera::rh::proj::directx::orthographic(l, r, b, t, n, f) * light_view;
            scales[slot] = Vec3::new(1.0 / (r - l), -1.0 / (t - b), 1.0 / (n - f));
            offsets[slot] = Vec3::new(-l / (r - l), t / (t - b), n / (n - f));
        }
        let (s, e) = (SPLITS[3] * 0.8, SPLITS[3]);
        let frame = ShadowFrame {
            light_view,
            scales,
            offsets,
            atlas_size: Vec2::new((CASCADE_SIZE * 4) as f32, CASCADE_SIZE as f32),
            fade: [1.0 / (e * e - s * s), -s * s / (e * e - s * s)],
            noise_scale: 1.2,
            margin: MARGIN,
        };
        (frame, view_projs)
    }

    /// Renders the casters of `scene` into the atlas.
    pub fn render(&self, r: &Renderer, enc: &mut wgpu::CommandEncoder, scene: &Scene, view_projs: &[Mat4; 4]) {
        for (u, m) in self.uniforms.iter().zip(view_projs) {
            r.queue.write_buffer(u, 0, bytemuck::cast_slice(&m.to_cols_array()));
        }
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.atlas,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        for (slot, bg) in self.bind_groups.iter().enumerate() {
            let s = CASCADE_SIZE as f32;
            pass.set_viewport(slot as f32 * s, 0.0, s, s, 0.0, 1.0);
            pass.set_bind_group(0, bg, &[]);
            for (i, m) in scene.meshes.iter().enumerate() {
                if scene.sky.is_some_and(|s| s.0 == i) {
                    continue;
                }
                draw_casters(&mut pass, scene, m);
            }
        }
    }
}

fn draw_casters(pass: &mut wgpu::RenderPass, scene: &Scene, m: &GpuMesh) {
    let mut bound = false;
    for (range, mat) in &m.submeshes {
        // opaque surfaces only: blended and alpha-tested cards (foliage)
        // would cast solid quads
        let solid = match *mat {
            MaterialRef::Basic(i) => !scene.materials[i].blended && !scene.materials[i].alpha_test,
            MaterialRef::Retail(i) => !scene.retail_materials[i].blended && !scene.retail_materials[i].alpha_test,
        };
        if !solid {
            continue;
        }
        if !bound {
            pass.set_vertex_buffer(0, m.vertices.slice(..));
            pass.set_vertex_buffer(1, m.instances.slice(..));
            pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
            bound = true;
        }
        pass.draw_indexed(range.clone(), 0, 0..m.instance_count);
    }
}

impl Shadows {
    /// Debug: the atlas as an 8-bit grey PGM (R32Float only).
    pub fn dump(&self, r: &Renderer, path: &str) {
        if r.shadow_format != wgpu::TextureFormat::R32Float {
            return;
        }
        let (w, h) = (CASCADE_SIZE * 4, CASCADE_SIZE);
        let buf = r.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (w * h * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = r.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            self.atlas_tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        r.queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        r.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let data = buf.slice(..).get_mapped_range().expect("mapped");
        let grey: Vec<u8> = data.chunks_exact(4).map(|c| (f32::from_le_bytes(c.try_into().unwrap()).clamp(0.0, 1.0) * 255.0) as u8).collect();
        let mut out = format!("P5 {w} {h} 255\n").into_bytes();
        out.extend_from_slice(&grey);
        std::fs::write(path, out).ok();
    }
}

/// The shaders' `ShadowFrame` rows for the cascades X, Y, Z, W.
pub fn cascade_rows(f: &ShadowFrame, i: usize) -> ([f32; 4], [f32; 4]) {
    (f.scales[i].extend(0.0).to_array(), f.offsets[i].extend(0.0).to_array())
}

