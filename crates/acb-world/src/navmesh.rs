//! Navigation meshes (`NavMeshManager` roots, one per tile).
//!
//! Each manager holds `NavigationMeshes`: `VertexList` (f32x4, world space),
//! `TriangleVertex` (u16 x3 per triangle) and per-triangle
//! `NeighborTriangleIndex` (u16 x3, within the same NavMesh). Links across
//! meshes and tiles (`MetaLinks`, `DirectConnectionSets`) are not decoded;
//! [`NavGraph`] instead joins triangles that share an edge in space.
//! Neighbour values 0xffe2..0xffe4 (-30..-28) mark border edges.

use std::collections::HashMap;

use anvil_fastload::Value;

use crate::Archive;

#[derive(Debug, Clone)]
pub struct NavMesh {
    pub manager: u32,
    pub source_entity: u32,
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u16; 3]>,
    /// Per triangle edge (v0v1, v1v2, v2v0): neighbour in this mesh, or
    /// `u16::MAX`.
    pub neighbors: Vec<[u16; 3]>,
    pub active: bool,
}

/// All triangles of a map's navmeshes in one adjacency graph.
#[derive(Debug, Clone, Default)]
pub struct NavGraph {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// Per triangle: neighbours and the point to cross into each (the
    /// middle of the shared stretch of edge).
    pub links: Vec<Vec<(u32, [f32; 3])>>,
}

impl Archive<'_> {
    /// Every NavMesh of the map's NavMeshManagers.
    pub fn navmeshes(&self) -> Vec<NavMesh> {
        let mut out = Vec::new();
        for uid in self.uids_of_in("NavMeshManager", 0) {
            let Ok(r) = self.root(uid) else { continue };
            for m in self.field(&r.obj, "NavigationMeshes").map(Value::items).unwrap_or_default().iter().filter_map(Value::as_obj) {
                let bytes = |n: &str| self.field(m, n).and_then(Value::as_bytes).unwrap_or_default();
                let vertices = bytes("VertexList")
                    .chunks_exact(16)
                    .map(|c| std::array::from_fn(|i| f32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().unwrap())))
                    .collect();
                let u16s = |b: &[u8]| b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>();
                let tv = u16s(bytes("TriangleVertex"));
                let triangles = tv.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
                let neighbors = self
                    .field(m, "Triangles")
                    .map(Value::items)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_obj)
                    .map(|t| {
                        let n = u16s(self.field(t, "NeighborTriangleIndex").and_then(Value::as_bytes).unwrap_or_default());
                        std::array::from_fn(|i| n.get(i).copied().unwrap_or(u16::MAX))
                    })
                    .collect();
                out.push(NavMesh {
                    manager: uid,
                    source_entity: self.field(m, "SourceEntityHandle").and_then(Value::target_id).unwrap_or(0),
                    vertices,
                    triangles,
                    neighbors,
                    active: self.field(m, "Active").and_then(Value::as_bool).unwrap_or(true),
                });
            }
        }
        out
    }

    /// The map's active navmeshes as one graph; vertices within 2 cm are
    /// welded so edges shared across meshes and tiles connect.
    pub fn nav_graph(&self) -> NavGraph {
        let mut g = NavGraph::default();
        let mut weld: HashMap<[i32; 3], u32> = HashMap::new();
        let key = |v: [f32; 3]| v.map(|x| (x * 50.0).round() as i32);
        for m in self.navmeshes().into_iter().filter(|m| m.active) {
            let ids: Vec<u32> = m
                .vertices
                .iter()
                .map(|v| {
                    *weld.entry(key(*v)).or_insert_with(|| {
                        g.vertices.push(*v);
                        g.vertices.len() as u32 - 1
                    })
                })
                .collect();
            for t in &m.triangles {
                if t.iter().all(|&i| (i as usize) < ids.len()) {
                    g.triangles.push(t.map(|i| ids[i as usize]));
                }
            }
        }
        // adjacency by shared (welded) edges
        let mut edges: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        for (ti, t) in g.triangles.iter().enumerate() {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                edges.entry((a.min(b), a.max(b))).or_default().push(ti as u32);
            }
        }
        g.links = vec![Vec::new(); g.triangles.len()];
        for (&(a, b), list) in edges.iter().filter(|(_, l)| l.len() == 2) {
            let (va, vb) = (g.vertices[a as usize], g.vertices[b as usize]);
            let mid = std::array::from_fn(|i| (va[i] + vb[i]) / 2.0);
            g.links[list[0] as usize].push((list[1], mid));
            g.links[list[1] as usize].push((list[0], mid));
        }
        g.join_t_junctions();
        g
    }
}

impl NavGraph {
    fn edge(&self, t: usize, e: usize) -> ([f32; 3], [f32; 3]) {
        let tri = self.triangles[t];
        (self.vertices[tri[e] as usize], self.vertices[tri[(e + 1) % 3] as usize])
    }

    /// Meshes meet at T-junctions (an edge of one spans several of the
    /// other), so edges without a welded neighbour that are collinear
    /// within 5 cm, overlap by 10 cm and differ by under 0.4 m in height
    /// are joined too.
    fn join_t_junctions(&mut self) {
        const CELL: f32 = 2.0;
        let mut welded = vec![[false; 3]; self.triangles.len()];
        for (t, tri) in self.triangles.iter().enumerate() {
            for e in 0..3 {
                let (a, b) = (tri[e], tri[(e + 1) % 3]);
                welded[t][e] = self.links[t].iter().any(|(o, _)| {
                    let ot = self.triangles[*o as usize];
                    ot.contains(&a) && ot.contains(&b)
                });
            }
        }
        let open: Vec<(usize, usize)> =
            (0..self.triangles.len()).flat_map(|t| (0..3).map(move |e| (t, e))).filter(|&(t, e)| !welded[t][e]).collect();
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (k, &(t, e)) in open.iter().enumerate() {
            let (a, b) = self.edge(t, e);
            let (x0, x1) = ((a[0].min(b[0]) / CELL).floor() as i32, (a[0].max(b[0]) / CELL).floor() as i32);
            let (y0, y1) = ((a[1].min(b[1]) / CELL).floor() as i32, (a[1].max(b[1]) / CELL).floor() as i32);
            for x in x0..=x1 {
                for y in y0..=y1 {
                    grid.entry((x, y)).or_default().push(k);
                }
            }
        }
        let mut pairs = std::collections::HashSet::new();
        for list in grid.values() {
            for (i, &ka) in list.iter().enumerate() {
                for &kb in &list[i + 1..] {
                    let ((ta, ea), (tb, eb)) = (open[ka], open[kb]);
                    if ta == tb || !pairs.insert((ka.min(kb), ka.max(kb))) {
                        continue;
                    }
                    let (a0, a1) = self.edge(ta, ea);
                    let (b0, b1) = self.edge(tb, eb);
                    if let Some(mid) = overlap(a0, a1, b0, b1) {
                        self.links[ta].push((tb as u32, mid));
                        self.links[tb].push((ta as u32, mid));
                    }
                }
            }
        }
    }
}

/// Where two edges lie on one line (5 cm, 0.4 m in height) and overlap
/// by at least 10 cm: the middle of the overlap.
fn overlap(a0: [f32; 3], a1: [f32; 3], b0: [f32; 3], b1: [f32; 3]) -> Option<[f32; 3]> {
    let d = [a1[0] - a0[0], a1[1] - a0[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 1e-4 {
        return None;
    }
    let u = [d[0] / len, d[1] / len];
    let off = |p: [f32; 3]| ((p[0] - a0[0]) * u[0] + (p[1] - a0[1]) * u[1], (p[0] - a0[0]) * -u[1] + (p[1] - a0[1]) * u[0]);
    let ((s0, n0), (s1, n1)) = (off(b0), off(b1));
    if n0.abs() > 0.05 || n1.abs() > 0.05 {
        return None;
    }
    let (lo, hi) = (s0.min(s1).max(0.0), s0.max(s1).min(len));
    if hi - lo < 0.1 {
        return None;
    }
    let m = (lo + hi) / 2.0;
    let za = a0[2] + (a1[2] - a0[2]) * m / len;
    let zb = if (s1 - s0).abs() > 1e-4 { b0[2] + (b1[2] - b0[2]) * (m - s0) / (s1 - s0) } else { b0[2] };
    ((za - zb).abs() < 0.4).then(|| [a0[0] + u[0] * m, a0[1] + u[1] * m, (za + zb) / 2.0])
}
