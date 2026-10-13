//! Binary glTF export of a map's static geometry, for checking decoded data
//! in Blender or any glTF viewer.

use std::collections::HashMap;
use std::path::Path;

use acb_world::{Archive, StaticMesh};
use serde_json::{json, Value as Json};

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;

#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<Json>,
    accessors: Vec<Json>,
}

impl Builder {
    fn push(&mut self, bytes: &[u8], target: u32) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        self.views.push(json!({
            "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len(), "target": target
        }));
        self.bin.extend_from_slice(bytes);
        self.views.len() - 1
    }

    fn vec_accessor<const N: usize>(&mut self, data: &[[f32; N]], bounds: bool) -> usize {
        let bytes: Vec<u8> = data.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.push(&bytes, ARRAY_BUFFER);
        let kind = ["SCALAR", "VEC2", "VEC3", "VEC4"][N - 1];
        let mut a = json!({
            "bufferView": view, "componentType": FLOAT, "count": data.len(), "type": kind,
        });
        if bounds {
            let mut lo = [f32::MAX; N];
            let mut hi = [f32::MIN; N];
            for v in data {
                for k in 0..N {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            a["min"] = json!(lo.to_vec());
            a["max"] = json!(hi.to_vec());
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn index_accessor(&mut self, idx: &[u32]) -> usize {
        let bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = self.push(&bytes, ELEMENT_ARRAY_BUFFER);
        self.accessors.push(json!({
            "bufferView": view, "componentType": UNSIGNED_INT, "count": idx.len(), "type": "SCALAR"
        }));
        self.accessors.len() - 1
    }
}

pub struct Stats {
    pub instances: usize,
    pub meshes: usize,
    pub skipped_meshes: usize,
    pub triangles: usize,
}

pub fn export(archive: &Archive, out: &Path) -> anyhow::Result<Stats> {
    let mut b = Builder::default();
    let mut meshes: Vec<Json> = Vec::new();
    let mut mesh_index: HashMap<u32, Option<usize>> = HashMap::new();
    let mut materials: Vec<Json> = Vec::new();
    let mut material_index: HashMap<u32, usize> = HashMap::new();
    let mut nodes: Vec<Json> = vec![json!({
        "name": "ACB world (Z up)",
        // Z-up to glTF's Y-up: -90 degrees about X
        "rotation": [-std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2],
        "children": [],
    })];
    let mut stats = Stats { instances: 0, meshes: 0, skipped_meshes: 0, triangles: 0 };

    for inst in archive.instances() {
        let slot = match mesh_index.get(&inst.mesh) {
            Some(s) => *s,
            None => {
                let s = match archive.static_mesh(inst.mesh)? {
                    Some(m) if !m.indices.is_empty() => {
                        stats.triangles += m.indices.len() / 3;
                        let name = archive.loc(inst.mesh).map(|l| l.name.clone()).unwrap_or_default();
                        let prims = primitives(&mut b, &m, &mut materials, &mut material_index, archive);
                        meshes.push(json!({ "name": name, "primitives": prims }));
                        stats.meshes += 1;
                        Some(meshes.len() - 1)
                    }
                    _ => {
                        stats.skipped_meshes += 1;
                        None
                    }
                };
                mesh_index.insert(inst.mesh, s);
                s
            }
        };
        let Some(mesh) = slot else { continue };
        nodes.push(json!({
            "name": format!("{:#010x}", inst.entity),
            "mesh": mesh,
            "matrix": inst.matrix,
        }));
        let child = nodes.len() - 1;
        nodes[0]["children"].as_array_mut().unwrap().push(json!(child));
        stats.instances += 1;
    }

    while b.bin.len() % 4 != 0 {
        b.bin.push(0);
    }
    let doc = json!({
        "asset": { "version": "2.0", "generator": "acbmp-rs" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": b.accessors,
        "bufferViews": b.views,
        "buffers": [{ "byteLength": b.bin.len() }],
    });
    write_glb(out, &serde_json::to_vec(&doc)?, &b.bin)?;
    Ok(stats)
}

fn primitives(
    b: &mut Builder,
    m: &StaticMesh,
    materials: &mut Vec<Json>,
    material_index: &mut HashMap<u32, usize>,
    archive: &Archive,
) -> Vec<Json> {
    let pos = b.vec_accessor(&m.positions, true);
    let normals: Vec<[f32; 3]> = m
        .normals
        .iter()
        .map(|n| {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if l > 1e-6 { [n[0] / l, n[1] / l, n[2] / l] } else { [0.0, 0.0, 1.0] }
        })
        .collect();
    let nrm = b.vec_accessor(&normals, false);
    // glTF's UV origin is top-left like D3D's, so V needs no flip
    let uv = b.vec_accessor(&m.uvs, false);
    m.submeshes
        .iter()
        .filter(|s| s.index_count > 0)
        .map(|s| {
            let range = s.first_index as usize..(s.first_index + s.index_count) as usize;
            let idx = b.index_accessor(&m.indices[range]);
            let mat = *material_index.entry(s.material).or_insert_with(|| {
                let name = archive.loc(s.material).map(|l| l.name.clone()).unwrap_or("none".into());
                materials.push(json!({ "name": name, "doubleSided": false }));
                materials.len() - 1
            });
            json!({
                "attributes": { "POSITION": pos, "NORMAL": nrm, "TEXCOORD_0": uv },
                "indices": idx,
                "material": mat,
            })
        })
        .collect()
}

fn write_glb(path: &Path, json: &[u8], bin: &[u8]) -> std::io::Result<()> {
    let mut json = json.to_vec();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    std::fs::write(path, out)
}
