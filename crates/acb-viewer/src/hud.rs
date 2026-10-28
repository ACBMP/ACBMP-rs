//! On-screen match HUD: the `ClientNet::hud` line as text at the top of the
//! screen, plus a compass arrow to the nearest enemy VIP for Escort hunters.
//!
//! Render-only and dependency-free: a 5x7 bitmap font baked in code, every
//! lit run of glyph pixels one quad, all quads untextured vertex-coloured
//! triangles in one small overlay pass. Captures (which render offscreen
//! through `acb_render::render_to_rgba`) get the same triangles rasterised
//! on the CPU.

use acb_render::wgpu;

/// A triangle-list vertex in pixel coordinates (y down), sRGB colour.
#[derive(Clone, Copy)]
pub struct Vert {
    pub pos: [f32; 2],
    pub color: [f32; 4],
}

const TEXT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BACK: [f32; 4] = [0.0, 0.0, 0.0, 0.55];
const ARROW: [f32; 4] = [1.0, 0.78, 0.2, 1.0];
const RING: [f32; 4] = [1.0, 1.0, 1.0, 0.35];
const MARK: [f32; 4] = [0.85, 0.9, 1.0, 0.95];
const TARGET: [f32; 4] = [0.95, 0.15, 0.1, 1.0];
const WARN_BACK: [f32; 4] = [0.6, 0.0, 0.0, 0.7];

/// Glyph rows top to bottom, `#` lit. Lowercase is drawn as uppercase,
/// anything missing as `?`.
#[rustfmt::skip]
const FONT: &[(char, [&str; 7])] = &[
    (' ', [".....", ".....", ".....", ".....", ".....", ".....", "....."]),
    ('A', [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"]),
    ('B', ["####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."]),
    ('C', [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."]),
    ('D', ["####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####."]),
    ('E', ["#####", "#....", "#....", "####.", "#....", "#....", "#####"]),
    ('F', ["#####", "#....", "#....", "####.", "#....", "#....", "#...."]),
    ('G', [".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".####"]),
    ('H', ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"]),
    ('I', [".###.", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."]),
    ('J', ["..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##.."]),
    ('K', ["#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"]),
    ('L', ["#....", "#....", "#....", "#....", "#....", "#....", "#####"]),
    ('M', ["#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"]),
    ('N', ["#...#", "#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#"]),
    ('O', [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
    ('P', ["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."]),
    ('Q', [".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#"]),
    ('R', ["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"]),
    ('S', [".####", "#....", "#....", ".###.", "....#", "....#", "####."]),
    ('T', ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."]),
    ('U', ["#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
    ('V', ["#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#.."]),
    ('W', ["#...#", "#...#", "#...#", "#.#.#", "#.#.#", "#.#.#", ".#.#."]),
    ('X', ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"]),
    ('Y', ["#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#.."]),
    ('Z', ["#####", "....#", "...#.", "..#..", ".#...", "#....", "#####"]),
    ('0', [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."]),
    ('1', ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."]),
    ('2', [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"]),
    ('3', ["#####", "...#.", "..#..", "...#.", "....#", "#...#", ".###."]),
    ('4', ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."]),
    ('5', ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."]),
    ('6', ["..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###."]),
    ('7', ["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."]),
    ('8', [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."]),
    ('9', [".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##.."]),
    ('.', [".....", ".....", ".....", ".....", ".....", ".##..", ".##.."]),
    (',', [".....", ".....", ".....", ".....", ".##..", "..#..", ".#..."]),
    (':', [".....", ".##..", ".##..", ".....", ".##..", ".##..", "....."]),
    ('|', ["..#..", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."]),
    ('%', ["##...", "##..#", "...#.", "..#..", ".#...", "#..##", "...##"]),
    ('/', [".....", "....#", "...#.", "..#..", ".#...", "#....", "....."]),
    ('[', [".###.", ".#...", ".#...", ".#...", ".#...", ".#...", ".###."]),
    (']', [".###.", "...#.", "...#.", "...#.", "...#.", "...#.", ".###."]),
    ('(', ["...#.", "..#..", ".#...", ".#...", ".#...", "..#..", "...#."]),
    (')', [".#...", "..#..", "...#.", "...#.", "...#.", "..#..", ".#..."]),
    ('-', [".....", ".....", ".....", "#####", ".....", ".....", "....."]),
    ('+', [".....", "..#..", "..#..", "#####", "..#..", "..#..", "....."]),
    ('=', [".....", ".....", "#####", ".....", "#####", ".....", "....."]),
    ('_', [".....", ".....", ".....", ".....", ".....", ".....", "#####"]),
    ('!', ["..#..", "..#..", "..#..", "..#..", "..#..", ".....", "..#.."]),
    ('\'', ["..#..", "..#..", ".#...", ".....", ".....", ".....", "....."]),
    ('?', [".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#.."]),
    ('<', ["...#.", "..#..", ".#...", "#....", ".#...", "..#..", "...#."]),
    ('>', [".#...", "..#..", "...#.", "....#", "...#.", "..#..", ".#..."]),
];

fn glyph(c: char) -> &'static [&'static str; 7] {
    let c = c.to_ascii_uppercase();
    FONT.iter().find(|g| g.0 == c).or_else(|| FONT.iter().find(|g| g.0 == '?')).map(|g| &g.1).unwrap()
}

fn quad(v: &mut Vec<Vert>, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]) {
    for pos in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
        v.push(Vert { pos, color });
    }
}

fn tri(v: &mut Vec<Vert>, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: [f32; 4]) {
    v.extend([a, b, c].map(|pos| Vert { pos, color }));
}

/// Greedy wrap of the ` | `-separated HUD fields into lines of at most
/// `max` characters (a single longer field is cut).
fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for field in text.split(" | ") {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 3 + field.chars().count() <= max => {
                *l += " | ";
                *l += field;
            }
            _ => lines.push(field.chars().take(max).collect()),
        }
    }
    lines
}

/// One line of bitmap text at `x`, `y` (top left), pixel scale `s`.
fn label(v: &mut Vec<Vert>, x: f32, y: f32, s: f32, line: &str, color: [f32; 4]) {
    for (i, c) in line.chars().enumerate() {
        let gx = x + i as f32 * 6.0 * s;
        for (r, row) in glyph(c).iter().enumerate() {
            // one quad per horizontal run of lit pixels
            let row = row.as_bytes();
            let mut col = 0;
            while col < 5 {
                if row[col] != b'#' {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < 5 && row[col] == b'#' {
                    col += 1;
                }
                let y0 = y + r as f32 * s;
                quad(v, gx + start as f32 * s, y0, gx + col as f32 * s, y0 + s, color);
            }
        }
    }
}

/// What a compass mark on the Assassinate strip stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A potential target (any other living player).
    Potential,
    /// The player being acquired (look in progress).
    Acquiring,
    /// The acquired target (the contract).
    Target,
}

/// The Assassinate widgets: compass strip marks (bearing as for the Escort
/// arrow: radians, CCW from straight ahead, so positive is left), the
/// acquire progress 0..1 when a look is in progress, and the "being
/// acquired" warning with its blink phase (from the sim tick, so the same
/// at any frame rate).
#[derive(Clone, Debug, Default)]
pub struct Assassin {
    pub marks: Vec<(f32, Mark)>,
    pub acquiring: Option<f32>,
    pub warned: bool,
    pub blink_on: bool,
}

/// Triangles for the HUD `text` across the top of a `w` x `h` screen and,
/// when `bearing` is set (radians, counter-clockwise from straight ahead =
/// to the left), a compass arrow under it; `assassin` adds the compass
/// strip, the acquire bar and the warning.
pub fn build(text: &str, bearing: Option<f32>, assassin: Option<&Assassin>, w: u32, h: u32) -> Vec<Vert> {
    let (w, h) = (w as f32, h as f32);
    let s = (w / 640.0).round().clamp(1.0, 4.0);
    let adv = 6.0 * s;
    let line_h = 10.0 * s;
    let max = (((w - 16.0) / adv) as usize).max(8);
    let mut v = Vec::new();
    let mut y = 8.0;
    for line in wrap(text, max) {
        let n = line.chars().count() as f32;
        let x = ((w - n * adv) / 2.0).round();
        quad(&mut v, x - 2.0 * s, y - 2.0 * s, x + n * adv + 1.0 * s, y + 9.0 * s, BACK);
        label(&mut v, x, y, s, &line, TEXT);
        y += line_h;
    }
    if let Some(a) = assassin {
        y = assassin_widgets(&mut v, a, w, h, y, s);
    }
    if let Some(b) = bearing {
        let r = 14.0 * s;
        let (cx, cy) = (w / 2.0, y + r + 4.0 * s);
        // ring of 32 thin segments
        let n = 32;
        let at = |a: f32, rad: f32| [cx + rad * a.cos(), cy + rad * a.sin()];
        for i in 0..n {
            let (a0, a1) = (i as f32 / n as f32 * std::f32::consts::TAU, (i + 1) as f32 / n as f32 * std::f32::consts::TAU);
            let (o0, o1, i0, i1) = (at(a0, r), at(a1, r), at(a0, r - s), at(a1, r - s));
            tri(&mut v, o0, o1, i1, RING);
            tri(&mut v, o0, i1, i0, RING);
        }
        // screen direction (y down): ahead is up, positive bearing is left
        let d = [-b.sin(), -b.cos()];
        let p = [d[1], -d[0]];
        let pt = |f: f32, side: f32| [cx + d[0] * f * r + p[0] * side * r, cy + d[1] * f * r + p[1] * side * r];
        tri(&mut v, pt(0.85, 0.0), pt(-0.45, 0.4), pt(-0.2, 0.0), ARROW);
        tri(&mut v, pt(0.85, 0.0), pt(-0.2, 0.0), pt(-0.45, -0.4), ARROW);
    }
    v
}

/// The Assassinate compass strip (+-180 deg, ahead in the middle), the
/// acquire bar under it and the blinking warning in the lower third.
/// Returns the y under the strip and bar.
fn assassin_widgets(v: &mut Vec<Vert>, a: &Assassin, w: f32, h: f32, y: f32, s: f32) -> f32 {
    use std::f32::consts::PI;
    let half = (110.0 * s).min(w / 2.0 - 8.0);
    let cx = w / 2.0;
    let (top, bot) = (y + 2.0 * s, y + 12.0 * s);
    let mid = (top + bot) / 2.0;
    quad(v, cx - half - 2.0 * s, top - 2.0 * s, cx + half + 2.0 * s, bot + 2.0 * s, BACK);
    // baseline, ahead notch, quarter ticks (left, right, behind at the ends)
    quad(v, cx - half, mid - 0.5 * s, cx + half, mid + 0.5 * s, RING);
    for q in [-1.0f32, -0.5, 0.0, 0.5, 1.0] {
        let x = cx + q * half;
        let (t0, t1) = if q == 0.0 { (top, bot) } else { (mid - 2.0 * s, mid + 2.0 * s) };
        quad(v, x - 0.5 * s, t0, x + 0.5 * s, t1, RING);
    }
    let x_of = |b: f32| cx - (b / PI).clamp(-1.0, 1.0) * half;
    // potential targets first so the acquiring / target marks draw on top
    let mut marks = a.marks.clone();
    marks.sort_by_key(|m| m.1 as u8);
    for (b, m) in marks {
        let x = x_of(b);
        match m {
            Mark::Potential => quad(v, x - 1.0 * s, top + 1.0 * s, x + 1.0 * s, bot - 1.0 * s, MARK),
            Mark::Acquiring => {
                tri(v, [x, bot], [x - 3.0 * s, top], [x + 3.0 * s, top], ARROW);
            }
            Mark::Target => {
                // diamond
                let r = 4.5 * s;
                tri(v, [x, mid - r], [x + r, mid], [x, mid + r], TARGET);
                tri(v, [x, mid - r], [x, mid + r], [x - r, mid], TARGET);
            }
        }
    }
    let mut y = bot + 4.0 * s;
    if let Some(p) = a.acquiring {
        let p = p.clamp(0.0, 1.0);
        let (x0, x1) = (cx - half * 0.6, cx + half * 0.6);
        quad(v, x0 - 1.0 * s, y - 1.0 * s, x1 + 1.0 * s, y + 5.0 * s, BACK);
        quad(v, x0, y, x0 + (x1 - x0) * p, y + 4.0 * s, ARROW);
        y += 7.0 * s;
    }
    if a.warned && a.blink_on {
        let msg = "BEING ACQUIRED!";
        let n = msg.len() as f32;
        let x = ((w - n * 6.0 * s * 2.0) / 2.0).round();
        let wy = (h * 0.68).round();
        quad(v, x - 4.0 * s, wy - 4.0 * s, x + n * 12.0 * s + 2.0 * s, wy + 18.0 * s, WARN_BACK);
        label(v, x, wy, s * 2.0, msg, TEXT);
    }
    y
}

/// Alpha-blend the triangles into an RGBA8 (sRGB) image, for captures.
pub fn rasterize(v: &[Vert], rgba: &mut [u8], w: u32, h: u32) {
    for t in v.chunks_exact(3) {
        let [a, b, c] = [t[0].pos, t[1].pos, t[2].pos];
        let edge = |p: [f32; 2], q: [f32; 2], x: f32, y: f32| (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0]);
        let area = edge(a, b, c[0], c[1]);
        if area.abs() < 1e-6 {
            continue;
        }
        let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as u32;
        let x1 = (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as u32).min(w);
        let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as u32;
        let y1 = (a[1].max(b[1]).max(c[1]).ceil().max(0.0) as u32).min(h);
        let col = t[0].color;
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let e = [edge(b, c, px, py), edge(c, a, px, py), edge(a, b, px, py)];
                // top-left-free inclusive test on the triangle's own winding
                if e.iter().all(|&e| e * area.signum() >= 0.0) {
                    let i = ((y * w + x) * 4) as usize;
                    for k in 0..3 {
                        let d = rgba[i + k] as f32 / 255.0;
                        rgba[i + k] = ((d + (col[k] - d) * col[3]) * 255.0).round() as u8;
                    }
                }
            }
        }
    }
}

/// The overlay pass: one pipeline, one growable vertex buffer, no bindings.
pub struct Overlay {
    pipeline: wgpu::RenderPipeline,
    buf: wgpu::Buffer,
    cap: u64,
    srgb: bool,
}

const SHADER: &str = r#"
struct Out { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> Out {
    return Out(vec4<f32>(pos, 0.0, 1.0), color);
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> { return i.color; }
"#;

const STRIDE: u64 = 24;

impl Overlay {
    pub fn new(d: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("hud"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("hud"), bind_group_layouts: &[], immediate_size: 0 });
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
        let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: STRIDE, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs })],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let cap = 4096;
        Self { pipeline, buf: Self::buffer(d, cap), cap, srgb: format.is_srgb() }
    }

    fn buffer(d: &wgpu::Device, verts: u64) -> wgpu::Buffer {
        d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud"),
            size: verts * STRIDE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Draw `v` (pixel coordinates on a `w` x `h` target) over `view`.
    pub fn draw(&mut self, d: &wgpu::Device, q: &wgpu::Queue, enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, v: &[Vert], w: u32, h: u32) {
        if v.is_empty() {
            return;
        }
        if v.len() as u64 > self.cap {
            self.cap = (v.len() as u64).next_power_of_two();
            self.buf = Self::buffer(d, self.cap);
        }
        let lin = |c: f32| if self.srgb { c.powf(2.2) } else { c };
        let mut bytes = Vec::with_capacity(v.len() * STRIDE as usize);
        for x in v {
            let ndc = [x.pos[0] / w as f32 * 2.0 - 1.0, 1.0 - x.pos[1] / h as f32 * 2.0];
            let c = [lin(x.color[0]), lin(x.color[1]), lin(x.color[2]), x.color[3]];
            for f in ndc.into_iter().chain(c) {
                bytes.extend_from_slice(&f.to_ne_bytes());
            }
        }
        q.write_buffer(&self.buf, 0, &bytes);
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hud"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.buf.slice(..bytes.len() as u64));
        pass.draw(0..v.len() as u32, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_rows_are_five_wide() {
        for (c, rows) in FONT {
            assert!(rows.iter().all(|r| r.len() == 5), "{c}");
        }
    }

    #[test]
    fn wraps_on_fields() {
        assert_eq!(wrap("aaaa | bbbb | cc", 11), vec!["aaaa | bbbb", "cc"]);
        assert_eq!(wrap("abcdef", 4), vec!["abcd"]);
    }

    #[test]
    fn arrow_points_ahead_and_left() {
        let tip = |b| {
            let v = build("x", Some(b), None, 640, 480);
            v[v.len() - 6].pos
        };
        let (c, ahead, left) = (320.0, tip(0.0), tip(std::f32::consts::FRAC_PI_2));
        assert!((ahead[0] - c).abs() < 0.01 && ahead[1] < 40.0);
        assert!(left[0] < c - 5.0);
    }

    #[test]
    fn strip_marks_follow_bearing() {
        let col = |v: &[Vert], c: [f32; 4]| v.iter().filter(|x| x.color == c).map(|x| x.pos[0]).sum::<f32>() / v.iter().filter(|x| x.color == c).count() as f32;
        let a = Assassin { marks: vec![(0.0, Mark::Target), (std::f32::consts::FRAC_PI_2, Mark::Potential)], acquiring: Some(0.5), warned: true, blink_on: true };
        let v = build("x", None, Some(&a), 640, 480);
        assert!((col(&v, TARGET) - 320.0).abs() < 0.01);
        assert!(col(&v, MARK) < 300.0);
        assert!(v.iter().any(|x| x.color == WARN_BACK));
        let off = build("x", None, Some(&Assassin { blink_on: false, ..a }), 640, 480);
        assert!(!off.iter().any(|x| x.color == WARN_BACK));
    }
}
