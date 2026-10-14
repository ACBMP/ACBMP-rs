//! Player collision: the map's Havok shapes (`MeshShape`, `BoxShape`)
//! bucketed in a 2D grid, split into walkable (floor) and steep (wall)
//! faces.

use std::collections::HashMap;

use acb_world::{Archive, CollisionMesh};
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
}

fn cell(p: Vec2) -> (i32, i32) {
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
        let mut g = Ground { tris: Vec::new(), floor: HashMap::new(), wall: HashMap::new() };
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

    pub fn triangles(&self) -> usize {
        self.tris.len()
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
}
