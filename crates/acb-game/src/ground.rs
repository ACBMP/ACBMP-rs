//! Player collision: the map's Havok shapes (`MeshShape`, `BoxShape`)
//! bucketed in a 2D grid, split into walkable (floor) and steep (wall)
//! faces, plus the freerun guidance edges (ledges for jumps, holds for
//! climbing; their queries live in `jump.rs` and `climb.rs`).

use std::collections::HashMap;

use acb_world::{Archive, CollisionMesh};

use crate::climb::Hold;
use crate::human::in_air::StepContact;
use glam::{Mat4, Vec2, Vec3};

const CELL: f32 = 2.0;
/// Faces with a normal at least this upright are floor, the rest walls.
const WALKABLE_NZ: f32 = 0.6;
/// `CollisionFilterInfo.Layer` of camera-only shapes (`*_CamCol_*`).
const CAMERA_LAYER: u32 = 18;

pub struct Ground {
    tris: Vec<[Vec3; 3]>,
    /// Per triangle: its collision material is `AIType_StepOff` (2): no
    /// standing on it (retail InAir StepOff, [`crate::human::in_air::step_off`]).
    no_stand: Vec<bool>,
    floor: HashMap<(i32, i32), Vec<u32>>,
    wall: HashMap<(i32, i32), Vec<u32>>,
    /// Freerun guidance edges with an upward face (ledges): ends.
    pub(crate) ledges: Vec<(Vec3, Vec3)>,
    pub(crate) ledge_grid: HashMap<(i32, i32), Vec<u32>>,
    /// Guidance edges along walls, for climbing.
    pub(crate) holds: Vec<Hold>,
    pub(crate) hold_grid: HashMap<(i32, i32), Vec<u32>>,
    /// Recent [`Ground::pilotis`] answers by their exact inputs (a pure
    /// query; a client's prediction replays the same falling ticks after
    /// every snapshot).
    pub(crate) pilotis_memo: std::sync::Mutex<std::collections::VecDeque<([u32; 5], Option<Vec3>)>>,
}

/// Retail player body (`BipedComponent` of AC2MP_Generic_PlayerEntity):
/// capsule radius, height (m) and mass (kg). Walls are swept at the radius.
pub const BODY_RADIUS: f32 = 0.4;
pub const BODY_HEIGHT: f32 = 1.8;
pub const BODY_MASS: f32 = 65.0;

/// Height a character steps up or down without falling, metres.
pub const STEP: f32 = 0.6;

/// `CollisionMaterial::AIType_StepOff`: the material forbids standing.
pub const AI_TYPE_STEP_OFF: u32 = 2;

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
        let mut g = Ground { tris: Vec::new(), no_stand: Vec::new(), floor: HashMap::new(), wall: HashMap::new(), ledges: Vec::new(), ledge_grid: HashMap::new(), holds: Vec::new(), hold_grid: HashMap::new(), pilotis_memo: Default::default() };
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
        let mut step_off: HashMap<u32, bool> = HashMap::new();
        for c in archive.colliders() {
            // camera-only proxies (haystack CamCol) and loose debris
            if c.layer == CAMERA_LAYER || !c.fixed {
                continue;
            }
            let shape = shapes.entry(c.shape).or_insert_with(|| archive.collision_mesh(c.shape).ok().flatten());
            let Some(shape) = shape else { continue };
            let m = Mat4::from_cols_array(&c.matrix);
            for (k, t) in shape.triangles.iter().enumerate() {
                let mat = shape.materials.get(k).copied().unwrap_or(0);
                let no_stand = mat != 0
                    && *step_off.entry(mat).or_insert_with(|| archive.material_ai_type(mat) == Some(AI_TYPE_STEP_OFF));
                g.add(t.map(|i| m.transform_point3(Vec3::from(shape.vertices[i as usize]))), no_stand);
            }
        }
        Ok(g)
    }

    fn add(&mut self, v: [Vec3; 3], no_stand: bool) {
        let n = (v[1] - v[0]).cross(v[2] - v[0]);
        let len = n.length();
        if len < 1e-8 {
            return;
        }
        let grid = if (n.z / len).abs() >= WALKABLE_NZ { &mut self.floor } else { &mut self.wall };
        let id = self.tris.len() as u32;
        self.tris.push(v);
        self.no_stand.push(no_stand);
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

    /// Contacts of the feet sphere (radius [`BODY_RADIUS`], resting on
    /// `feet`) with no-standing triangles, for
    /// [`crate::human::in_air::step_off`]. Default-material contacts are
    /// left out: our landing takes those (retail: a default contact steeper
    /// than 45 deg alone never steps off either).
    pub fn step_contacts(&self, feet: Vec3) -> Vec<StepContact> {
        let c = feet + Vec3::Z * BODY_RADIUS;
        let (lo, hi) = (cell(c.truncate() - Vec2::splat(BODY_RADIUS)), cell(c.truncate() + Vec2::splat(BODY_RADIUS)));
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                for grid in [&self.floor, &self.wall] {
                    for &i in grid.get(&(x, y)).into_iter().flatten() {
                        if !self.no_stand[i as usize] || !seen.insert(i) {
                            continue;
                        }
                        let t = &self.tris[i as usize];
                        let q = closest_on_tri(c, t);
                        let d = c - q;
                        if d.length_squared() > BODY_RADIUS * BODY_RADIUS {
                            continue;
                        }
                        let mut tri = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
                        if tri.dot(d) < 0.0 {
                            tri = -tri;
                        }
                        let normal = d.try_normalize().unwrap_or(tri);
                        out.push(StepContact { tri, normal, no_stand: true, character: false });
                    }
                }
            }
        }
        out
    }

    /// The no-standing (StepOff material) triangles.
    pub fn no_stand_triangles(&self) -> impl Iterator<Item = [Vec3; 3]> + '_ {
        self.tris.iter().zip(&self.no_stand).filter(|(_, n)| **n).map(|(t, _)| *t)
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
        self.floor_hit(p).map(|h| h.0)
    }

    /// [`Self::floor_below`] plus the floor's unit normal (pointing up).
    pub fn floor_hit(&self, p: Vec3) -> Option<(f32, Vec3)> {
        let ids = self.floor.get(&cell(p.truncate()))?;
        ids.iter()
            .filter_map(|&i| ray_tri(p, -Vec3::Z, &self.tris[i as usize]).map(|t| (t, i)))
            .filter(|(t, _)| *t >= 0.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(t, i)| {
                let [a, b, c] = self.tris[i as usize];
                let n = (b - a).cross(c - a).normalize_or_zero();
                (p.z - t, if n.z < 0.0 { -n } else { n })
            })
    }

    /// Is an upright box free of collision? `centre`, `dir` (unit,
    /// horizontal: the box's first axis; the second is across it, the third
    /// up) and `half` extents along those axes (retail
    /// `Human::CheckClearanceBoxGlobal`, used by
    /// [`crate::human::ground::fall_box`]). Exact triangle / box overlap
    /// (separating axes) against the floor and wall triangles.
    pub fn box_clear(&self, centre: Vec3, dir: Vec2, half: Vec3) -> bool {
        let ax = [dir.extend(0.0), Vec3::new(-dir.y, dir.x, 0.0), Vec3::Z];
        let r = half.x.hypot(half.y);
        let (lo, hi) = (cell(centre.truncate() - Vec2::splat(r)), cell(centre.truncate() + Vec2::splat(r)));
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                for map in [&self.floor, &self.wall] {
                    for &i in map.get(&(x, y)).into_iter().flatten() {
                        let t = self.tris[i as usize].map(|v| {
                            let d = v - centre;
                            Vec3::new(d.dot(ax[0]), d.dot(ax[1]), d.dot(ax[2]))
                        });
                        if tri_box_overlap(&t, half) {
                            return false;
                        }
                    }
                }
            }
        }
        true
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
        self.slide_hit(pos, delta).0
    }

    /// [`Self::slide`], also returning the first wall contact's normal
    /// (horizontal, out of the wall; the sweeps are at 0.55 and 1.3 m, above
    /// retail's 0.5 m obstacle contact height).
    pub fn slide_hit(&self, pos: Vec3, delta: Vec2) -> (Vec3, Option<Vec2>) {
        let blocked = |from: Vec3, d: Vec2| {
            [0.55, 1.3]
                .into_iter()
                .filter_map(|h| self.sweep_wall(from + Vec3::Z * h, d, BODY_RADIUS))
                .min_by(|a, b| a.0.total_cmp(&b.0))
        };
        let mut at = pos;
        let mut d = delta;
        let mut hit = None;
        for _ in 0..2 {
            match blocked(at, d) {
                None => {
                    at += d.extend(0.0);
                    break;
                }
                Some((f, n)) => {
                    hit = hit.or(Some(n));
                    at += (d * f).extend(0.0);
                    let rest = d * (1.0 - f);
                    d = rest - n * rest.dot(n);
                }
            }
        }
        (at, hit)
    }

    /// Moves a character standing at `pos` by `delta`: slides along walls
    /// and snaps to the floor within a step. Returns the new position and
    /// whether it left a ledge (floor more than a step below: start
    /// falling, position not snapped). Moves onto no floor at all are
    /// refused.
    pub fn move_on(&self, pos: Vec3, delta: Vec2) -> (Vec3, bool) {
        self.move_on_edge(pos, delta, STEP)
    }

    /// [`Self::move_on`] stepping down less than `down` metres (the player:
    /// [`crate::human::ground::FALL_OFF_EDGE`]); steps up stay at [`STEP`].
    pub fn move_on_edge(&self, pos: Vec3, delta: Vec2, down: f32) -> (Vec3, bool) {
        let at = self.slide(pos, delta);
        match self.floor_below(at + Vec3::Z * STEP) {
            Some(z) if at.z - z < down => (at.with_z(z), false),
            Some(_) => (at, true),
            None => (pos, false),
        }
    }
}

/// Does triangle `t` (in box space) overlap the box `[-half, half]`?
/// Separating axis test: the 3 box axes, the triangle normal and the 9 edge
/// cross products.
fn tri_box_overlap(t: &[Vec3; 3], half: Vec3) -> bool {
    let e = [t[1] - t[0], t[2] - t[1], t[0] - t[2]];
    let mut axes = vec![Vec3::X, Vec3::Y, Vec3::Z, e[0].cross(e[1])];
    for b in [Vec3::X, Vec3::Y, Vec3::Z] {
        axes.extend(e.iter().map(|e| b.cross(*e)));
    }
    axes.into_iter().filter(|a| a.length_squared() > 1e-12).all(|a| {
        let p = t.map(|v| v.dot(a));
        let r = half.x * a.x.abs() + half.y * a.y.abs() + half.z * a.z.abs();
        p[0].min(p[1]).min(p[2]) <= r && p[0].max(p[1]).max(p[2]) >= -r
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tri_box_overlap_separates() {
        let h = Vec3::new(0.25, 0.3, 1.0);
        let t = |z: f32, x: f32| [Vec3::new(x, -1.0, z), Vec3::new(x, 1.0, z), Vec3::new(x + 1.0, 0.0, z)];
        assert!(tri_box_overlap(&t(0.0, -0.5), h));
        assert!(!tri_box_overlap(&t(1.1, -0.5), h));
        assert!(!tri_box_overlap(&t(0.0, 0.3), h));
        // a wall across the box
        let w = [Vec3::new(0.1, -2.0, -2.0), Vec3::new(0.1, 2.0, -2.0), Vec3::new(0.1, 0.0, 3.0)];
        assert!(tri_box_overlap(&w, h));
    }
}

/// Closest point of triangle `t` to `p` (Ericson, Real-Time Collision
/// Detection 5.1.5).
fn closest_on_tri(p: Vec3, [a, b, c]: &[Vec3; 3]) -> Vec3 {
    let (ab, ac, ap) = (*b - *a, *c - *a, p - *a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return *a;
    }
    let bp = p - *b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return *b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return *a + ab * (d1 / (d1 - d3));
    }
    let cp = p - *c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return *c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return *a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return *b + (*c - *b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    *a + ab * (vb * denom) + ac * (vc * denom)
}
