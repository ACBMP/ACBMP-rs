//! Player collision: the map's Havok shapes (`MeshShape`, `BoxShape`)
//! bucketed in a 2D grid, split into walkable (floor) and steep (wall)
//! faces.

use std::collections::HashMap;

use acb_world::{Archive, CollisionMesh};

use crate::climb::{Dir, Hold};
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
    /// Freerun guidance edges with an upward face (ledges): ends, up
    /// face normal.
    ledges: Vec<(Vec3, Vec3)>,
    ledge_grid: HashMap<(i32, i32), Vec<u32>>,
    /// Guidance edges along walls, for climbing.
    holds: Vec<Hold>,
    hold_grid: HashMap<(i32, i32), Vec<u32>>,
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

    /// A ledge to jump up onto from `pos` running along `fwd` (unit): the
    /// nearest guidance edge ahead within `reach` metres whose top is `min`
    /// to `max` above the feet and has floor just behind it. Returns the
    /// landing point on top.
    pub fn ledge_ahead(&self, pos: Vec3, fwd: Vec2, reach: f32, min: f32, max: f32) -> Option<Vec3> {
        let probe = pos.truncate() + fwd * reach * 0.5;
        let (c0, c1) = (cell(probe - Vec2::splat(reach)), cell(probe + Vec2::splat(reach)));
        let mut best: Option<(f32, Vec3)> = None;
        for x in c0.0..=c1.0 {
            for y in c0.1..=c1.1 {
                for &i in self.ledge_grid.get(&(x, y)).into_iter().flatten() {
                    let (a, b) = self.ledges[i as usize];
                    // closest point of the edge to the line ahead
                    let ab = b - a;
                    let t = if ab.length_squared() > 1e-6 { ((pos - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                    let q = a + ab * t;
                    let d = (q - pos).truncate();
                    let (dist, dz) = (d.length(), q.z - pos.z);
                    if dist < 0.3 || dist > reach || dz < min || dz > max || d.normalize().dot(fwd) < 0.8 {
                        continue;
                    }
                    // somewhere to stand just past the edge
                    let land = q + (fwd * 0.45).extend(0.0);
                    let Some(z) = self.floor_below(land + Vec3::Z * 0.5) else { continue };
                    if (z - q.z).abs() > 0.3 {
                        continue;
                    }
                    if best.is_none_or(|(bd, _)| dist < bd) {
                        best = Some((dist, land.with_z(z)));
                    }
                }
            }
        }
        best.map(|b| b.1)
    }

    fn holds_near(&self, p: Vec3, r: f32) -> impl Iterator<Item = &Hold> + '_ {
        let (c0, c1) = (cell(p.truncate() - Vec2::splat(r)), cell(p.truncate() + Vec2::splat(r)));
        let mut ids: Vec<u32> =
            (c0.0..=c1.0).flat_map(|x| (c0.1..=c1.1).map(move |y| (x, y))).filter_map(|k| self.hold_grid.get(&k)).flatten().copied().collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().map(|i| &self.holds[i as usize])
    }

    /// A hold to start climbing from feet at `pos` facing `fwd`: a wall
    /// edge facing us within 0.9 m, 1.3-2.6 m up.
    pub fn mount_hold(&self, pos: Vec3, fwd: Vec2) -> Option<Hold> {
        self.holds_near(pos, 1.5)
            .filter(|h| h.wall.dot(fwd) < -0.7)
            .filter_map(|h| {
                let q = h.nearest(pos);
                let d = (q - pos).truncate();
                let dz = q.z - pos.z;
                (d.length() < 0.9 && d.dot(fwd) > 0.0 && (1.3..=2.6).contains(&dz)).then_some((dz, *h))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
    }

    /// A hold to grab while falling at `pos` facing `fwd`: a wall edge
    /// facing us within 0.7 m ahead, 1.3-2.2 m above the feet.
    pub fn catch_hold(&self, pos: Vec3, fwd: Vec2) -> Option<Hold> {
        self.holds_near(pos, 1.5)
            .filter(|h| h.wall.dot(fwd) < -0.5)
            .filter_map(|h| {
                let q = h.nearest(pos);
                let d = (q - pos).truncate();
                let dz = q.z - pos.z;
                (d.length() < 0.7 && d.dot(fwd) > -0.1 && (1.3..=2.2).contains(&dz)).then_some((d.length(), *h))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
    }

    /// The next hold from hands at `hand` on a wall facing `n`, moving
    /// `dir`: an edge on the same wall plane, 0.5-2 m away that way.
    pub fn next_hold(&self, hand: Vec3, n: Vec2, dir: Dir) -> Option<(Hold, Vec3)> {
        // facing f = -n, the character's right is (f.y, -f.x)
        let right = Vec2::new(-n.y, n.x);
        let probe = match dir {
            Dir::Up => hand + Vec3::Z * 1.2,
            Dir::Down => hand - Vec3::Z * 1.2,
            Dir::Left => hand - (right * 1.2).extend(0.0),
            Dir::Right => hand + (right * 1.2).extend(0.0),
        };
        self.holds_near(probe, 2.0)
            .filter(|h| h.wall.dot(n) > 0.8)
            .filter_map(|h| {
                let q = h.nearest(probe);
                let d = q - hand;
                let (lat, up, out) = (d.truncate().dot(right), d.z, d.truncate().dot(n));
                let ok = out.abs() < 0.35
                    && match dir {
                        Dir::Up => (0.5..=2.0).contains(&up) && lat.abs() < 0.5,
                        Dir::Down => (-2.0..=-0.5).contains(&up) && lat.abs() < 0.5,
                        Dir::Left => (-2.0..=-0.5).contains(&lat) && up.abs() < 0.5,
                        Dir::Right => (0.5..=2.0).contains(&lat) && up.abs() < 0.5,
                    };
                ok.then_some((q.distance(probe), *h, q))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| (x.1, x.2))
    }

    /// Debug: standing spots from which a hold can be mounted, nearest
    /// `near` first (spot, facing).
    pub fn mount_spots(&self, near: Vec3, count: usize) -> Vec<(Vec3, Vec2)> {
        let mut out: Vec<(f32, Vec3, Vec2)> = self
            .holds
            .iter()
            .filter_map(|h| {
                let q = (h.a + h.b) / 2.0;
                let stand = q + (h.wall * 0.6).extend(0.0);
                let z = self.floor_below(stand.with_z(q.z - 1.2))?;
                let dz = q.z - z;
                // and another hold above to climb to
                let hand = q;
                let stacked = self.next_hold(hand, h.wall, Dir::Up).is_some();
                ((1.4..=2.5).contains(&dz) && stacked).then(|| (stand.with_z(z).distance(near), stand.with_z(z), -h.wall))
            })
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().take(count).map(|x| (x.1, x.2)).collect()
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
