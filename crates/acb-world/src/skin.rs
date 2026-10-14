//! Skinned mesh decoding (vertex format 0, 32 bytes).
//!
//! Layout: short4n position (w = 32767; the shaders scale xyz by 10) |
//! ubyte4 normal | ubyte4 tangent | ubyte4 binormal ((b - 127) / 127) |
//! short2n uv (scaled by 16) | ubyte4 bone indices | ubyte4n weights.
//!
//! Bone indices address a per-draw hardware palette (`g_BoneMatrixArray`,
//! 42 float4x3 at c120): `InstancingData[i].HwBoneToMeshBoneTable` maps
//! palette slots to `Mesh.Bones`, whose `BoneID`s name skeleton bones.

use anvil_fastload::Value;

use crate::{Archive, Error};

/// Palette size of the skinned vertex shaders (126 registers / 3).
pub const PALETTE_SIZE: usize = 42;

#[derive(Debug, Clone, Default)]
pub struct SkinnedMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// D3D convention: V down.
    pub uvs: Vec<[f32; 2]>,
    /// Hardware palette slots of the vertex's draw.
    pub joints: Vec<[u8; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub submeshes: Vec<SkinnedSubmesh>,
    pub bones: Vec<MeshBone>,
    pub format: u32,
    pub stride: usize,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct SkinnedSubmesh {
    pub first_index: u32,
    pub index_count: u32,
    /// Vertex range the submesh's indices fall in.
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub material: u32,
    /// Palette slot -> index into `bones`.
    pub palette: Vec<u16>,
}

#[derive(Debug, Clone, Copy)]
pub struct MeshBone {
    /// Skeleton bone id.
    pub id: u32,
    /// `PackedInitialMatrixInverse`: J0..K2 then TransA, TransB.
    pub packed: ([u16; 6], u32, u32),
}

impl Archive<'_> {
    /// Decoded skinned mesh; `Ok(None)` when the mesh is not format 0.
    pub fn skinned_mesh(&self, uid: u32) -> Result<Option<SkinnedMesh>, Error> {
        let root = self.root(uid)?;
        let o = &root.obj;
        let missing = |what: &str| Error::Missing(format!("mesh {uid:#010x}: {what}"));
        let Some(cm) = self.field(o, "CompiledMesh").and_then(Value::as_obj) else {
            return Ok(None);
        };
        let md = self.field(cm, "MeshData").and_then(Value::as_obj).ok_or_else(|| missing("MeshData"))?;
        let get = |n: &str| self.field(md, n).ok_or_else(|| missing(n));
        let num = |o: &anvil_fastload::Obj, n: &str| self.field(o, n).and_then(Value::as_u32).unwrap_or(0);

        let format = get("VertexFormat")?.as_u32().unwrap_or(u32::MAX);
        let stride = get("VertexStride")?.as_u32().unwrap_or(0) as usize;
        if (format, stride) != (0, 32) {
            return Ok(None);
        }
        let vb = get("VertexBufferData")?.as_bytes().unwrap_or_default();
        let ib = get("IndexBufferData")?.as_bytes().unwrap_or_default();
        let idx32 = get("IsIndexBuffer32bit")?.as_bool().unwrap_or(false);
        let n = vb.len() / stride;
        if n == 0 {
            return Ok(None);
        }

        let s16 = |b: &[u8], i: usize| i16::from_le_bytes([b[i], b[i + 1]]) as f32;
        let snorm = |b: u8| (b as f32 - 127.0) / 127.0;
        let mut m = SkinnedMesh { format, stride, raw: vb[..n * stride].to_vec(), ..Default::default() };
        for v in vb.chunks_exact(stride) {
            let s = 10.0 / 32767.0;
            m.positions.push([s16(v, 0) * s, s16(v, 2) * s, s16(v, 4) * s]);
            m.normals.push([snorm(v[8]), snorm(v[9]), snorm(v[10])]);
            m.uvs.push([s16(v, 20) * 16.0 / 32767.0, s16(v, 22) * 16.0 / 32767.0]);
            m.joints.push([v[24], v[25], v[26], v[27]]);
            m.weights.push(std::array::from_fn(|i| v[28 + i] as f32 / 255.0));
        }

        let index: Vec<u32> = if idx32 {
            ib.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
        } else {
            ib.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect()
        };

        for b in self.field(o, "Bones").map(Value::items).unwrap_or_default() {
            let Some(b) = b.as_obj() else { continue };
            let p = self.field(b, "PackedInitialMatrixInverse").and_then(Value::as_obj);
            let pn = |n: &str| p.map(|p| num(p, n)).unwrap_or(0);
            m.bones.push(MeshBone {
                id: num(b, "BoneID"),
                packed: (
                    ["J0", "J1", "J2", "K0", "K1", "K2"].map(|n| pn(n) as u16),
                    pn("TransA"),
                    pn("TransB"),
                ),
            });
        }

        let materials: Vec<u32> = self
            .field(o, "CompiledMeshMaterials")
            .map(|v| v.items().iter().map(|r| r.target_id().unwrap_or(0)).collect())
            .unwrap_or_default();
        // InstancingData[i] and CompiledMeshMaterials[i] belong to primitive i
        // (the palette sizes match each primitive's highest bone index;
        // SubMeshIndex is something else)
        let per_prim: Vec<(u32, Vec<u16>)> = self
            .field(cm, "InstancingData")
            .map(Value::items)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let palette = d
                    .as_obj()
                    .map(|d| {
                        let nb = num(d, "NumBones") as usize;
                        let table = self.field(d, "HwBoneToMeshBoneTable").and_then(Value::as_bytes).unwrap_or_default();
                        table.iter().take(nb).map(|&b| b as u16).collect()
                    })
                    .unwrap_or_default();
                (materials.get(i).copied().unwrap_or(0), palette)
            })
            .collect();

        for (pi, p) in get("StandardPrimitives")?.items().iter().enumerate() {
            let Some(p) = p.as_obj() else { continue };
            if num(p, "Type") != 1 {
                continue;
            }
            let (start, count) = (num(p, "StartIndex") as usize, num(p, "PrimitiveCount") as usize);
            let first = m.indices.len() as u32;
            for t in index.get(start..start + 3 * count).unwrap_or_default().chunks_exact(3) {
                if t.iter().all(|&i| (i as usize) < n) && t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                    m.indices.extend_from_slice(t);
                }
            }
            let (material, palette) = per_prim.get(pi).cloned().unwrap_or_default();
            m.submeshes.push(SkinnedSubmesh {
                first_index: first,
                index_count: m.indices.len() as u32 - first,
                first_vertex: num(p, "MinIndex"),
                vertex_count: num(p, "NumVertices"),
                material,
                palette,
            });
        }
        Ok(Some(m))
    }
}
