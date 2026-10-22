//! Player collision: the map's Havok shapes (`MeshShape`, `BoxShape`)
//! bucketed in a 2D grid, split into walkable (floor) and steep (wall)
//! faces, plus the freerun guidance edges (ledges for jumps, holds for
//! climbing; their queries live in `jump.rs` and `climb.rs`).

use std::collections::HashMap;

use acb_world::{Archive, CollisionMesh};

use crate::climb::Hold;
use glam::{Mat4, Vec2, Vec3};

const CELL: f32 = 2.0;
/// Faces with a normal at least this upright are floor, the rest walls.
const WALKABLE_NZ: f32 = 0.6;
/// `CollisionFilterInfo.Layer` of camera-only shapes (`*_CamCol_*`).
const CAMERA_LAYER: u32 = 18;

pub struct Ground {
    tris: Vec<[Vec3; 3]>,
    floor: HashMap<(i32, i32), Vec<u32>>,
    wall: HashMap<(i32, i32), Vec<u32>>,
    /// Freerun guidance edges with an upward face (ledges): ends.
    pub(crate) ledges: Vec<(Vec3, Vec3)>,
    pub(crate) ledge_grid: HashMap<(i32, i32), Vec<u32>>,
    /// Guidance edges along walls, for climbing.
    pub(crate) holds: Vec<Hold>,
    pub(crate) hold_grid: HashMap<(i32, i32), Vec<u32>>,
}

/// Retail player body (`BipedComponent` of AC2MP_Generic_PlayerEntity):
/// capsule radius, height (m) and mass (kg). Walls are swept at the radius.
pub const BODY_RADIUS: f32 = 0.4;
pub const BODY_HEIGHT: f32 = 1.8;
pub const BODY_MASS: f32 = 65.0;

/// Height a character steps up or down without falling, metres.
pub const STEP: f32 = 0.6;

pub(crate) fn cell(p: Vec2) -> (i32, i32) {
    ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32)
}

/// Ray parameter of the hit of `o + t*d` with triangle `t`, either side.
fn ray_tri(o: Vec3, d: Vec3, [a, b, c]: &[Vec3; 3]) -> Option<f32> {
    let (e1, e2) = (*b - *a, *c - *a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let s = o - *a;
    let u = s.dot(p) / det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) / det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(e2.dot(q) / det)
}

impl Ground {
    pub fn load(archive: &Archive) -> anyhow::Result<Ground> {
        let mut g = Ground { tris: Vec::new(), floor: HashMap::new(), wall: HashMap::new(), ledges: Vec::new(), ledge_grid: HashMap::new(), holds: Vec::new(), hold_grid: HashMap::new() };
        for e in archive.guidance_edges() {
            let wall = e.normals.iter().map(|n| Vec3::from(*n)).find(|n| n.z.abs() < 0.5 && n.truncate().length() > 0.5);
            if let Some(w) = wall {
                let (a, b) = (Vec3::from(e.a), Vec3::from(e.b));
                let id = g.holds.len() as u32;
                g.holds.push(Hold { a, b, wall: w.truncate().normalize(), top: e.normals.iter().any(|n| n[2] > 0.7) });
                let (lo, hi) = (cell(a.truncate().min(b.truncate())), cell(a.truncate().max(b.truncate())));
                for x in lo.0..=hi.0 {
                    for y in lo.1..=hi.1 {
                        g.hold_grid.entry((x, y)).or_default().push(id);
                    }
                }
            }
            // keep edges with a walkable face on top
            if !e.normals.iter().any(|n| n[2] > 0.7) {
                continue;
            }
            let (a, b) = (Vec3::from(e.a), Vec3::from(e.b));
            let id = g.ledges.len() as u32;
            g.ledges.push((a, b));
            let (lo, hi) = (cell(a.truncate().min(b.truncate())), cell(a.truncate().max(b.truncate())));
            for x in lo.0..=hi.0 {
                for y in lo.1..=hi.1 {
                    g.ledge_grid.entry((x, y)).or_default().push(id);
                }
            }
        }
        let mut shapes: HashMap<u32, Option<CollisionMesh>> = HashMap::new();
        for c in archive.colliders() {
            // camera-only proxies (haystack CamCol) and loose debris
            if c.layer == CAMERA_LAYER || !c.fixed {
                continue;
            }
            let shape = shapes.entry(c.shape).or_insert_with(|| archive.collision_mesh(c.shape).ok().flatten());
            let Some(shape) = shape else { continue };
            let m = Mat4::from_cols_array(&c.matrix);
            for t in &shape.triangles {
                g.add(t.map(|i| m.transform_point3(Vec3::from(shape.vertices[i as usize]))));
            }
        }
        Ok(g)
    }

    fn add(&mut self, v: [Vec3; 3]) {
        let n = (v[1] - v[0]).cross(v[2] - v[0]);
        let len = n.length();
        if len < 1e-8 {
            return;
        }
        let grid = if (n.z / len).abs() >= WALKABLE_NZ { &mut self.floor } else { &mut self.wall };
        let id = self.tris.len() as u32;
        self.tris.push(v);
        let lo = cell(v[0].truncate().min(v[1].truncate()).min(v[2].truncate()));
        let hi = cell(v[0].truncate().max(v[1].truncate()).max(v[2].truncate()));
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                grid.entry((x, y)).or_default().push(id);
            }
        }
    }

    /// Nearest hit of the segment `o + t*d`, t in 0..1, with any collision
    /// triangle (floors and walls).
    pub fn ray(&self, o: Vec3, d: Vec3) -> Option<f32> {
        let (lo, hi) = (cell(o.truncate().min((o + d).truncate())), cell(o.truncate().max((o + d).truncate())));
        let mut best: Option<f32> = None;
        let mut seen = std::collections::HashSet::new();
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                for grid in [&self.floor, &self.wall] {
                    for &i in grid.get(&(x, y)).into_iter().flatten() {
                        if !seen.insert(i) {
                            continue;
                        }
                        if let Some(t) = ray_tri(o, d, &self.tris[i as usize]).filter(|t| (0.0..=1.0).contains(t)) {
                            if best.is_none_or(|b| t < b) {
                                best = Some(t);
                            }
                        }
                    }
                }
            }
        }
        best
    }

    pub fn triangles(&self) -> usize {
        self.tris.len()
    }

    /// The walkable (floor) triangles.
    pub fn floor_triangles(&self) -> impl Iterator<Item = [Vec3; 3]> + '_ {
        let mut ids: Vec<u32> = self.floor.values().flatten().copied().collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().map(|i| self.tris[i as usize])
    }

    /// Highest floor at `p.xy` no higher than `p.z`.
    pub fn floor_below(&self, p: Vec3) -> Option<f32> {
        let ids = self.floor.get(&cell(p.truncate()))?;
        ids.iter()
            .filter_map(|&i| ray_tri(p, -Vec3::Z, &self.tris[i as usize]))
            .filter(|t| *t >= 0.0)
            .min_by(f32::total_cmp)
            .map(|t| p.z - t)
    }

    /// First wall hit moving horizontally from `from` by `delta` (plus
    /// `radius`): the fraction of the move and the wall's unit normal.
    pub fn sweep_wall(&self, from: Vec3, delta: Vec2, radius: f32) -> Option<(f32, Vec2)> {
        let len = delta.length();
        if len < 1e-6 {
            return None;
        }
        let dir = (delta / len).extend(0.0);
        let reach = len + radius;
        let end = from + dir * reach;
        let (lo, hi) = (cell(from.truncate().min(end.truncate())), cell(from.truncate().max(end.truncate())));
        let mut best: Option<(f32, u32)> = None;
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                for &i in self.wall.get(&(x, y)).into_iter().flatten() {
                    if let Some(t) = ray_tri(from, dir, &self.tris[i as usize]).filter(|t| (0.0..reach).contains(t)) {
                        if best.is_none_or(|(b, _)| t < b) {
                            best = Some((t, i));
                        }
                    }
                }
            }
        }
        let (t, i) = best?;
        let [a, b, c] = self.tris[i as usize];
        let mut n = (b - a).cross(c - a).truncate().normalize_or_zero();
        if n.dot(dir.truncate()) > 0.0 {
            n = -n;
        }
        Some((((t - radius) / len).max(0.0), n))
    }

    /// Horizontal move with wall sliding, swept at knee and chest height (so
    /// steps and curbs pass).
    pub fn slide(&self, pos: Vec3, delta: Vec2) -> Vec3 {
        let blocked = |from: Vec3, d: Vec2| {
            [0.55, 1.3]
                .into_iter()
                .filter_map(|h| self.sweep_wall(from + Vec3::Z * h, d, BODY_RADIUS))
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

    /// Moves a character standing at `pos` by `delta`: slides along walls
    /// and snaps to the floor within a step. Returns the new position and
    /// whether it left a ledge (floor more than a step below: start
    /// falling, position not snapped). Moves onto no floor at all are
    /// refused.
    pub fn move_on(&self, pos: Vec3, delta: Vec2) -> (Vec3, bool) {
        let at = self.slide(pos, delta);
        match self.floor_below(at + Vec3::Z * STEP) {
            Some(z) if z >= at.z - STEP => (at.with_z(z), false),
            Some(_) => (at, true),
            None => (pos, false),
        }
    }
}
