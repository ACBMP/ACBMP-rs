//! Map collision: the Havok shapes behind entities' `RigidBody` components.
//!
//! Static scenery carries an `InertComponent` (or another component) whose
//! `RigidBody.Shape` points at a shape root; a `MultiInertComponent` holds an
//! inline `MultiMeshShape` listing other entities and their shapes. `MeshShape` is a plain
//! triangle soup in the entity's frame: `Vertices` f32x3, `IndicesTriangles`
//! u16x3 and `IndicesMaterial` one u8 per triangle into `Materials`
//! (PhysicMaterial roots such as `Stone_Dirt`). The MOPP code is Havok's
//! acceleration tree over the same triangles and is not needed.

use anvil_fastload::{Obj, Value};

use crate::{Archive, Error};

/// One rigid body placed in the map.
#[derive(Debug, Clone)]
pub struct Collider {
    pub entity: u32,
    /// Type hash of the component holding the rigid body.
    pub component: u32,
    /// Shape root uid (or inline object id).
    pub shape: u32,
    pub layer: u32,
    pub fixed: bool,
    /// The entity is an out-of-bounds wall.
    pub out_of_bounds: bool,
    /// `InertComponent` gameplay flags.
    pub is_ground: bool,
    pub is_roof: bool,
    /// Same layout as [`crate::Instance::matrix`].
    pub matrix: [f32; 16],
}

/// Triangles of a shape in its owner's frame.
#[derive(Debug, Clone, Default)]
pub struct CollisionMesh {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// Per triangle: PhysicMaterial uid (0 when unset).
    pub materials: Vec<u32>,
}

impl CollisionMesh {
    fn append(&mut self, other: CollisionMesh, m: Option<&[f32; 16]>) {
        let base = self.vertices.len() as u32;
        self.vertices.extend(other.vertices.into_iter().map(|v| match m {
            Some(m) => xform(m, v),
            None => v,
        }));
        self.triangles.extend(other.triangles.into_iter().map(|t| t.map(|i| i + base)));
        self.materials.extend(other.materials);
    }
}

/// Row-vector transform: `v * M` with the translation in the last row.
pub fn xform(m: &[f32; 16], [x, y, z]: [f32; 3]) -> [f32; 3] {
    [
        x * m[0] + y * m[4] + z * m[8] + m[12],
        x * m[1] + y * m[5] + z * m[9] + m[13],
        x * m[2] + y * m[6] + z * m[10] + m[14],
    ]
}

impl Archive<'_> {
    /// Every rigid body on the map's own entities (and those inlined in
    /// EntityGroups).
    pub fn colliders(&self) -> Vec<Collider> {
        let mut out = Vec::new();
        let mut visit = |e: &Obj| {
            let Some(Ok(matrix)) = self.field(e, "GlobalMatrix").and_then(Value::as_f32s).map(<[f32; 16]>::try_from)
            else {
                return;
            };
            let comps = self.field(e, "Components").map(Value::items).unwrap_or_default();
            let out_of_bounds =
                comps.iter().filter_map(Value::as_obj).any(|c| self.is_type(c, "OutOfBoundsComponent"));
            for c in comps.iter().filter_map(Value::as_obj) {
                let flag = |o: &Obj, n: &str| self.field(o, n).and_then(Value::as_bool).unwrap_or(false);
                // MultiInertComponent: an inline MultiMeshShape merging other
                // entities' shapes, each placed by its entity's matrix
                if let Some(multi) = self.field(c, "Shape").and_then(Value::as_obj) {
                    let list = |n: &str| -> Vec<u32> {
                        self.field(multi, n).map(Value::items).unwrap_or_default().iter().map(|v| v.target_id().unwrap_or(0)).collect()
                    };
                    for (ent, shape) in list("ContainedEntities").into_iter().zip(list("ContainedShapes")) {
                        let Some(Ok(m)) = self
                            .root(ent)
                            .ok()
                            .and_then(|r| self.field(&r.obj, "GlobalMatrix").and_then(Value::as_f32s))
                            .map(<[f32; 16]>::try_from)
                        else {
                            continue;
                        };
                        out.push(Collider {
                            entity: ent,
                            component: c.type_hash,
                            shape,
                            layer: 1,
                            fixed: true,
                            out_of_bounds: false,
                            is_ground: false,
                            is_roof: false,
                            matrix: m,
                        });
                    }
                    continue;
                }
                let Some(rb) = self.field(c, "RigidBody").and_then(Value::as_obj) else { continue };
                let Some(shape) = self.field(rb, "Shape").and_then(Value::target_id) else { continue };
                let layer = self
                    .field(rb, "FilterInfo")
                    .and_then(Value::as_obj)
                    .and_then(|f| self.field(f, "Layer"))
                    .and_then(Value::as_u32)
                    .unwrap_or(0);
                out.push(Collider {
                    entity: e.id,
                    component: c.type_hash,
                    shape,
                    layer,
                    fixed: flag(rb, "IsFixed"),
                    out_of_bounds,
                    is_ground: flag(c, "IsGround"),
                    is_roof: flag(c, "IsRoof"),
                    matrix,
                });
            }
        };
        for uid in self.uids_of_in("Entity", 0) {
            if let Ok(r) = self.root(uid) {
                visit(&r.obj);
            }
        }
        // groups nest: every Entity object inside one
        let entity = self.schema().hash_of("Entity");
        for uid in self.uids_of_in("EntityGroup", 0) {
            if let Ok(r) = self.root(uid) {
                visit(&r.obj);
                r.obj.walk(&mut |o| {
                    if Some(o.type_hash) == entity {
                        visit(o)
                    }
                });
            }
        }
        // merged entities (InertComponent.IsMerged) are also listed by a
        // MultiMeshShape: keep one collider per entity and shape, the
        // entity's own component first
        let multi = self.schema().hash_of("MultiInertComponent");
        out.sort_by_key(|c| Some(c.component) == multi);
        let mut seen = std::collections::HashSet::new();
        out.retain(|c| seen.insert((c.entity, c.shape)));
        out
    }

    /// Triangles of a shape root; `Ok(None)` for shape types not decoded.
    pub fn collision_mesh(&self, uid: u32) -> Result<Option<CollisionMesh>, Error> {
        let root = self.root(uid)?;
        self.shape_mesh(&root.obj)
    }

    fn shape_mesh(&self, o: &Obj) -> Result<Option<CollisionMesh>, Error> {
        let name = self.schema().name_or_hash(o.type_hash);
        match name.as_str() {
            "MeshShape" => Ok(Some(self.mesh_shape(o)?)),
            "BoxShape" => Ok(self.box_shape(o)),
            _ => Ok(None),
        }
    }

    /// `HalfExtents` around the origin of `Matrix` (row vectors).
    fn box_shape(&self, o: &Obj) -> Option<CollisionMesh> {
        let h = self.field(o, "HalfExtents")?.as_f32s()?;
        let m: [f32; 16] = self.field(o, "Matrix")?.as_f32s()?.try_into().ok()?;
        let vertices = (0..8)
            .map(|i| {
                let s = |bit: usize, e: f32| if i >> bit & 1 == 1 { e } else { -e };
                xform(&m, [s(0, h[0]), s(1, h[1]), s(2, h[2])])
            })
            .collect();
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let triangles: Vec<[u32; 3]> = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
        let mat = self.field(o, "Material").and_then(Value::target_id).unwrap_or(0);
        Some(CollisionMesh { vertices, materials: vec![mat; triangles.len()], triangles })
    }

    fn mesh_shape(&self, o: &Obj) -> Result<CollisionMesh, Error> {
        let bytes = |n: &str| self.field(o, n).and_then(Value::as_bytes).unwrap_or_default();
        let vertices = bytes("Vertices")
            .chunks_exact(12)
            .map(|c| std::array::from_fn(|i| f32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().unwrap())))
            .collect::<Vec<[f32; 3]>>();
        let idx = bytes("IndicesTriangles");
        let triangles: Vec<[u32; 3]> = idx
            .chunks_exact(6)
            .map(|c| std::array::from_fn(|i| u16::from_le_bytes([c[i * 2], c[i * 2 + 1]]) as u32))
            .collect();
        if let Some(bad) = triangles.iter().flatten().find(|&&i| i as usize >= vertices.len()) {
            return Err(Error::Missing(format!("MeshShape {:#010x}: index {bad} of {}", o.id, vertices.len())));
        }
        let mats: Vec<u32> = self
            .field(o, "Materials")
            .map(Value::items)
            .unwrap_or_default()
            .iter()
            .map(|m| m.target_id().unwrap_or(0))
            .collect();
        let per_tri = bytes("IndicesMaterial");
        let materials =
            (0..triangles.len()).map(|i| per_tri.get(i).and_then(|&m| mats.get(m as usize)).copied().unwrap_or(0)).collect();
        Ok(CollisionMesh { vertices, triangles, materials })
    }

    /// Every collider's triangles in world space, one soup. Returns the
    /// uids of shape types it could not decode, with counts.
    pub fn world_collision(&self) -> (CollisionMesh, Vec<(String, usize)>) {
        let mut world = CollisionMesh::default();
        let mut skipped: std::collections::BTreeMap<String, usize> = Default::default();
        let mut cache: std::collections::HashMap<u32, Option<CollisionMesh>> = Default::default();
        for c in self.colliders() {
            let mesh = cache.entry(c.shape).or_insert_with(|| self.collision_mesh(c.shape).ok().flatten());
            match mesh {
                Some(m) => world.append(m.clone(), Some(&c.matrix)),
                None => *skipped.entry(self.type_name(c.shape).unwrap_or_else(|| "?".into())).or_default() += 1,
            }
        }
        (world, skipped.into_iter().collect())
    }
}
