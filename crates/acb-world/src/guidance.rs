//! Freerun guidance: the edges characters grab, hang from and run along
//! (`GuidanceSystem` components and their capsule/cylinder/barrel kin).
//!
//! `CompressPoints` are u16 x3, (v - 32768) * 5 mm in entity space (they
//! fill the `Partitioner` box: a tower head's 6.09 m spans 1211 steps). Each `GuidanceObject`
//! joins `Index0` -> `Index1`; `DecN4Normal0`/`1` are four i16 / 511 each:
//! the faces meeting at the edge (on a roof ledge: up, then the wall's
//! outward normal). `SubType` 1 = plain edge (others: beams, ropes,
//! ladders, boats; see the schema's `GuidanceObjectSubType*`).

use anvil_fastload::{Obj, Value};

use crate::collision::xform;
use crate::Archive;

/// Quantization step of `CompressPoints`.
const POINT_STEP: f32 = 0.005;

#[derive(Debug, Clone)]
pub struct GuidanceEdge {
    pub entity: u32,
    pub a: [f32; 3],
    pub b: [f32; 3],
    /// World-space normals of the two faces meeting at the edge.
    pub normals: [[f32; 3]; 2],
    pub sub_type: u32,
}

fn rotate(m: &[f32; 16], [x, y, z]: [f32; 3]) -> [f32; 3] {
    [x * m[0] + y * m[4] + z * m[8], x * m[1] + y * m[5] + z * m[9], x * m[2] + y * m[6] + z * m[10]]
}

impl Archive<'_> {
    fn guidance_of(&self, e: &Obj, m: &[f32; 16], out: &mut Vec<GuidanceEdge>) {
        for c in self.field(e, "Components").map(Value::items).unwrap_or_default().iter().filter_map(Value::as_obj) {
            let Some(objects) = self.field(c, "GuidanceObjects") else { continue };
            let raw = self.field(c, "CompressPoints").and_then(Value::as_bytes).unwrap_or_default();
            let points: Vec<[f32; 3]> = raw
                .chunks_exact(6)
                .map(|p| {
                    let q = |i: usize| (u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]) as f32 - 32768.0) * POINT_STEP;
                    xform(m, std::array::from_fn(q))
                })
                .collect();
            for o in objects.items().iter().filter_map(Value::as_obj) {
                let num = |n: &str| self.field(o, n).and_then(Value::as_u32).unwrap_or(0);
                if num("Valid") == 0 {
                    continue;
                }
                let (Some(&a), Some(&b)) = (points.get(num("Index0") as usize), points.get(num("Index1") as usize)) else {
                    continue;
                };
                let normal = |n: &str| {
                    let b = self.field(o, n).and_then(Value::as_bytes).unwrap_or_default();
                    let s = |i: usize| b.get(i * 2..i * 2 + 2).map_or(0.0, |c| i16::from_le_bytes([c[0], c[1]]) as f32 / 511.0);
                    rotate(m, [s(0), s(1), s(2)])
                };
                out.push(GuidanceEdge {
                    entity: e.id,
                    a,
                    b,
                    normals: [normal("DecN4Normal0"), normal("DecN4Normal1")],
                    sub_type: num("SubType"),
                });
            }
        }
    }

    /// Every guidance edge on the map's entities, in world space.
    pub fn guidance_edges(&self) -> Vec<GuidanceEdge> {
        let mut out = Vec::new();
        let mut visit = |e: &Obj| {
            if let Some(Ok(m)) = self.field(e, "GlobalMatrix").and_then(Value::as_f32s).map(<[f32; 16]>::try_from) {
                self.guidance_of(e, &m, &mut out);
            }
        };
        for uid in self.uids_of_in("Entity", 0) {
            if let Ok(r) = self.root(uid) {
                visit(&r.obj);
            }
        }
        let entity = self.schema().hash_of("Entity");
        let mut seen = std::collections::HashSet::new();
        for uid in self.uids_of_in("EntityGroup", 0) {
            if let Ok(r) = self.root(uid) {
                r.obj.walk(&mut |o| {
                    if Some(o.type_hash) == entity && seen.insert(o.id) {
                        visit(o)
                    }
                });
            }
        }
        out
    }
}
