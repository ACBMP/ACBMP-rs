//! Soft-body cloth (`Cloth` -> `ClothState.SoftBodyLODs[i]` = `ClothLOD`).
//!
//! A ClothLOD is a particle system in the visual mesh's space: particles
//! (`VertexPos`) skinned to `MeshBones` by `BoneIndices`/`BoneWeights`
//! (4 per particle), held together by distance `Constraints`
//! (`SquareRestLength`), and a visual mesh whose vertices follow particle
//! triangles (`VisualVertexMappings`: three particle indices, barycentric
//! weights and an offset along the triangle normal). `VertexSkinFactors`
//! blend each simulated particle toward its skinned position.

use acb_world::{Archive, MeshBone};
use anvil_fastload::{Obj, Value};
use glam::{Vec3, Vec4};

#[derive(Debug, Clone)]
pub struct Constraint {
    pub a: u16,
    pub b: u16,
    pub rest: f32,
    pub contractibility: f32,
}

#[derive(Debug, Clone)]
pub struct VertexMapping {
    pub skinned: bool,
    pub offset: f32,
    pub indices: [u16; 3],
    pub weights: [f32; 3],
}

#[derive(Debug, Clone, Default)]
pub struct ClothSettings {
    pub gravity: Vec3,
    pub air_friction: f32,
    pub iterations: u32,
    pub enable_physics: bool,
    pub min_skin_factor: f32,
    pub max_skin_factor: f32,
}

#[derive(Debug, Clone)]
pub struct ClothLod {
    pub uid: u32,
    pub visual_mesh: u32,
    pub settings: ClothSettings,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub flags: Vec<u8>,
    pub radius: Vec<f32>,
    pub rigidity: Vec<f32>,
    pub skin_factor: Vec<f32>,
    pub bone_indices: Vec<[u8; 4]>,
    pub bone_weights: Vec<Vec4>,
    pub bones: Vec<MeshBone>,
    /// Particle triangles (collision / normals).
    pub indices: Vec<u16>,
    pub constraints: Vec<Constraint>,
    pub mappings: Vec<VertexMapping>,
    pub visual_indices: Vec<u16>,
    /// D3D convention (V down), per mapping.
    pub visual_uvs: Vec<[f32; 2]>,
}

fn f32s(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn u16s(b: &[u8]) -> Vec<u16> {
    b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
}

fn f32_of(archive: &Archive, o: &Obj, n: &str) -> f32 {
    archive.field(o, n).and_then(Value::as_f32s).and_then(|v| v.first().copied()).unwrap_or(0.0)
}

impl ClothLod {
    /// The first ClothLOD with particles of a `Cloth` root.
    pub fn load(archive: &Archive, cloth: u32) -> Option<ClothLod> {
        let root = archive.root(cloth).ok()?;
        let state = archive.field(&root.obj, "SoftBodyStates")?.items().first()?.as_obj()?;
        archive
            .field(state, "SoftBodyLODs")?
            .items()
            .iter()
            .filter_map(Value::as_obj)
            .find_map(|l| Self::from_obj(archive, l).filter(|c| !c.positions.is_empty()))
    }

    pub fn from_obj(archive: &Archive, o: &Obj) -> Option<ClothLod> {
        let bytes = |n: &str| archive.field(o, n).and_then(Value::as_bytes).unwrap_or_default();
        let vec3s = |n: &str| f32s(bytes(n)).chunks_exact(3).map(Vec3::from_slice).collect::<Vec<_>>();
        let num = |o: &Obj, n: &str| archive.field(o, n).and_then(Value::as_u32).unwrap_or(0);
        let settings = archive
            .field(o, "Settings")
            .and_then(Value::target_id)
            .and_then(|s| archive.root(s).ok())
            .map(|s| {
                let s = &s.obj;
                ClothSettings {
                    gravity: archive.field(s, "Gravity").and_then(Value::as_f32s).map(|g| Vec3::from_slice(&g)).unwrap_or(Vec3::Z * -10.0),
                    air_friction: f32_of(archive, s, "AirFrictionFactor"),
                    iterations: num(s, "ConstraintIterationCount"),
                    enable_physics: num(s, "EnablePhysics") != 0,
                    min_skin_factor: f32_of(archive, s, "MinSkinFactor"),
                    max_skin_factor: f32_of(archive, s, "MaxSkinFactor"),
                }
            })
            .unwrap_or_default();
        let constraints = archive
            .field(o, "Constraints")
            .map(Value::items)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_obj)
            .map(|c| Constraint {
                a: num(c, "VertexIndex1") as u16,
                b: num(c, "VertexIndex2") as u16,
                rest: f32_of(archive, c, "SquareRestLength").max(0.0).sqrt(),
                contractibility: f32_of(archive, c, "Contractibility"),
            })
            .collect();
        let mappings = archive
            .field(o, "VisualVertexMappings")
            .map(Value::items)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_obj)
            .map(|m| {
                let idx = u16s(archive.field(m, "Indices").and_then(Value::as_bytes).unwrap_or_default());
                let w = f32s(archive.field(m, "Weights").and_then(Value::as_bytes).unwrap_or_default());
                VertexMapping {
                    skinned: num(m, "UseVisualSkinning") != 0,
                    offset: f32_of(archive, m, "Offset"),
                    indices: std::array::from_fn(|i| idx.get(i).copied().unwrap_or(0)),
                    weights: std::array::from_fn(|i| w.get(i).copied().unwrap_or(0.0)),
                }
            })
            .collect();
        Some(ClothLod {
            uid: o.id,
            visual_mesh: archive.field(o, "VisualMesh").and_then(Value::target_id).unwrap_or(0),
            settings,
            positions: vec3s("VertexPos"),
            normals: vec3s("VertexNormals"),
            flags: bytes("VertexFlags").to_vec(),
            radius: f32s(bytes("VertexSphereRadius")),
            rigidity: f32s(bytes("VertexRigidities")),
            skin_factor: f32s(bytes("VertexSkinFactors")),
            bone_indices: bytes("BoneIndices").chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect(),
            bone_weights: f32s(bytes("BoneWeights")).chunks_exact(4).map(Vec4::from_slice).collect(),
            bones: archive.mesh_bones(archive.field(o, "MeshBones")),
            indices: u16s(bytes("Indices")),
            constraints,
            mappings,
            visual_indices: u16s(bytes("VisualIndices")),
            visual_uvs: f32s(bytes("VisualTextureCoords")).chunks_exact(2).map(|c| [c[0], c[1]]).collect(),
        })
    }

    /// For each vertex of the visual mesh, the mapping that drives it: the
    /// mesh's vertex order (and seam splits) differ from the mapping list,
    /// so vertices are matched by bind-pose position and UV. `None` for
    /// vertices with no mapping within 5 mm (they stay skinned).
    pub fn match_visual(&self, mesh: &acb_world::SkinnedMesh) -> Vec<Option<usize>> {
        let vis = self.visual_positions(&self.positions);
        mesh.positions
            .iter()
            .zip(&mesh.uvs)
            .enumerate()
            .map(|(i, (p, uv))| {
                let p = Vec3::from(*p);
                if vis.get(i).is_some_and(|v| v.distance(p) < 0.001) {
                    return Some(i);
                }
                let duv = |k: usize| self.visual_uvs.get(k).map_or(0.0, |t| (t[0] - uv[0]).abs() + (t[1] - uv[1]).abs());
                (0..vis.len())
                    .filter(|&k| vis[k].distance(p) < 0.005)
                    .min_by(|&a, &b| (duv(a), vis[a].distance(p)).partial_cmp(&(duv(b), vis[b].distance(p))).unwrap())
            })
            .collect()
    }

    /// Visual vertex positions from particle positions `p`.
    pub fn visual_positions(&self, p: &[Vec3]) -> Vec<Vec3> {
        self.mappings
            .iter()
            .map(|m| {
                // 0xffff: no particle (single- or two-particle mappings)
                let q = m.indices.map(|i| p.get(i as usize).copied());
                let mut v: Vec3 = q.iter().zip(m.weights).filter_map(|(x, w)| x.map(|x| x * w)).sum();
                if let [Some(a), Some(b), Some(c)] = q {
                    v += (b - a).cross(c - a).normalize_or_zero() * m.offset;
                }
                v
            })
            .collect()
    }
}

/// Per-instance simulation state of one ClothLOD, in world space.
#[derive(Debug, Clone, Default)]
pub struct ClothSim {
    pub pos: Vec<Vec3>,
    prev: Vec<Vec3>,
    /// Unspent time below one step.
    pending: f32,
}

/// Fixed simulation step.
const STEP: f32 = 1.0 / 60.0;

impl ClothSim {
    /// Advances by `dt` toward the skinned particle positions `target`
    /// (world space, this frame). Pinned particles (flag 1, skin factor 1)
    /// follow the skin; the others integrate with gravity and air friction,
    /// are pulled toward the skin by their skin factor and keep their
    /// constraint lengths.
    pub fn step(&mut self, lod: &ClothLod, target: &[Vec3], dt: f32) {
        let n = target.len();
        if self.pos.len() != n || !self.pos.iter().all(|p| p.is_finite()) {
            self.pos = target.to_vec();
            self.prev = target.to_vec();
            self.pending = 0.0;
            return;
        }
        let s = &lod.settings;
        if !s.enable_physics {
            self.pos.copy_from_slice(target);
            self.prev.copy_from_slice(target);
            return;
        }
        self.pending += dt.min(0.1);
        let steps = (self.pending / STEP) as usize;
        self.pending -= steps as f32 * STEP;
        let damp = (1.0 - s.air_friction * 0.05).clamp(0.0, 1.0);
        let pin = |i: usize| lod.skin_factor.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0);
        for _ in 0..steps {
            for i in 0..n {
                let k = pin(i);
                if k >= 1.0 {
                    self.prev[i] = target[i];
                    self.pos[i] = target[i];
                    continue;
                }
                let v = (self.pos[i] - self.prev[i]) * damp;
                self.prev[i] = self.pos[i];
                let free = self.pos[i] + v + s.gravity * STEP * STEP;
                // painted skin factor: share of the skinned position kept
                self.pos[i] = free.lerp(target[i], k * k * 0.5);
            }
            for _ in 0..s.iterations.max(1) {
                for c in &lod.constraints {
                    let (a, b) = (c.a as usize, c.b as usize);
                    let (wa, wb) = (1.0 - pin(a), 1.0 - pin(b));
                    if wa + wb <= 0.0 {
                        continue;
                    }
                    let d = self.pos[b] - self.pos[a];
                    let len = d.length();
                    if len < 1e-6 {
                        continue;
                    }
                    let corr = d * ((len - c.rest) / (len * (wa + wb)));
                    self.pos[a] += corr * wa;
                    self.pos[b] -= corr * wb;
                }
            }
            // stand-in for body collision: stay within reach of the skin
            for i in 0..n {
                let reach = 0.05 + 0.6 * (1.0 - pin(i));
                let d = self.pos[i] - target[i];
                if d.length() > reach {
                    self.pos[i] = target[i] + d.normalize() * reach;
                }
            }
        }
    }
}
