//! Static mesh decoding.
//!
//! Static vertex formats (`MeshData.VertexFormat`, stride):
//! - 4 (20 bytes): short4 position | ubyte4 normal | ubyte4 tangent | short2 uv
//! - 3 (24 bytes): as 4 with a d3dcolor before the uv
//!
//! Position = xyz * |w| / 2^18 in the entity's frame (w's sign carries the
//! tangent handedness). Normal = (b - 127.5) / 127.5. UV = short / 2048 with
//! V pointing down (D3D). Format 0 (32 bytes) is skinned and not handled yet.

use anvil_fastload::Value;

use crate::{Archive, Error};

const POS_SCALE: f32 = 1.0 / (1 << 18) as f32;
const UV_SCALE: f32 = 1.0 / 2048.0;
/// `StandardPrimitive.Type` of a triangle list, the only type in retail maps.
const TRIANGLE_LIST: u32 = 1;

#[derive(Debug, Clone, Default)]
pub struct StaticMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// D3D convention: V down.
    pub uvs: Vec<[f32; 2]>,
    /// Optional per-vertex RGBA (format 3).
    pub colors: Option<Vec<[u8; 4]>>,
    pub indices: Vec<u32>,
    pub submeshes: Vec<Submesh>,
}

#[derive(Debug, Clone)]
pub struct Submesh {
    /// Range into `indices`.
    pub first_index: u32,
    pub index_count: u32,
    /// Material uid, 0 when unset.
    pub material: u32,
}

impl Archive<'_> {
    /// Decoded static mesh; `Ok(None)` for skinned or unknown vertex formats.
    pub fn static_mesh(&self, uid: u32) -> Result<Option<StaticMesh>, Error> {
        let root = self.root(uid)?;
        let o = &root.obj;
        let missing = |what: &str| Error::Missing(format!("mesh {uid:#010x}: {what}"));
        let Some(cm) = self.field(o, "CompiledMesh").and_then(Value::as_obj) else {
            return Ok(None);
        };
        let md = self.field(cm, "MeshData").and_then(Value::as_obj).ok_or_else(|| missing("MeshData"))?;
        let get = |n: &str| self.field(md, n).ok_or_else(|| missing(n));

        let format = get("VertexFormat")?.as_u32().unwrap_or(u32::MAX);
        let stride = get("VertexStride")?.as_u32().unwrap_or(0) as usize;
        let has_color = match (format, stride) {
            (4, 20) => false,
            (3, 24) => true,
            _ => return Ok(None),
        };
        let vb = get("VertexBufferData")?.as_bytes().unwrap_or_default();
        let ib = get("IndexBufferData")?.as_bytes().unwrap_or_default();
        let idx32 = get("IsIndexBuffer32bit")?.as_bool().unwrap_or(false);
        let n = vb.len() / stride;
        if n == 0 {
            return Ok(None);
        }

        let s16 = |b: &[u8], i: usize| i16::from_le_bytes([b[i], b[i + 1]]) as f32;
        let unorm = |b: u8| (b as f32 - 127.5) / 127.5;
        let mut m = StaticMesh {
            colors: has_color.then(|| Vec::with_capacity(n)),
            ..Default::default()
        };
        for v in vb.chunks_exact(stride) {
            let s = s16(v, 6).abs() * POS_SCALE;
            m.positions.push([s16(v, 0) * s, s16(v, 2) * s, s16(v, 4) * s]);
            m.normals.push([unorm(v[8]), unorm(v[9]), unorm(v[10])]);
            if let Some(c) = &mut m.colors {
                c.push([v[16], v[17], v[18], v[19]]);
            }
            m.uvs.push([s16(v, stride - 4) * UV_SCALE, s16(v, stride - 2) * UV_SCALE]);
        }

        let index: Vec<u32> = if idx32 {
            ib.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
        } else {
            ib.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect()
        };

        let materials: Vec<u32> = self
            .field(o, "CompiledMeshMaterials")
            .map(|v| v.items().iter().map(|r| r.target_id().unwrap_or(0)).collect())
            .unwrap_or_default();

        for (pi, p) in get("StandardPrimitives")?.items().iter().enumerate() {
            let Some(p) = p.as_obj() else { continue };
            let pf = |n: &str| self.field(p, n).and_then(Value::as_u32).unwrap_or(0) as usize;
            if pf("Type") as u32 != TRIANGLE_LIST {
                continue;
            }
            let (start, count) = (pf("StartIndex"), pf("PrimitiveCount"));
            let first = m.indices.len() as u32;
            for t in index.get(start..start + 3 * count).unwrap_or_default().chunks_exact(3) {
                let (a, b, c) = (t[0], t[1], t[2]);
                if a as usize >= n || b as usize >= n || c as usize >= n || a == b || b == c || a == c {
                    continue;
                }
                m.indices.extend_from_slice(&[a, b, c]);
            }
            m.submeshes.push(Submesh {
                first_index: first,
                index_count: m.indices.len() as u32 - first,
                material: materials.get(pi).copied().unwrap_or(0),
            });
        }
        Ok(Some(m))
    }
}
