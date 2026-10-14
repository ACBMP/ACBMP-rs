//! ACB map content on top of the forge and FastLoad layers.
//!
//! [`Archive`] indexes every root object of a forge by its object id (uid) and
//! decodes objects on demand. The helpers below turn decoded objects into
//! engine-ready data: entity instances, static meshes and textures. Layouts
//! follow the ACB map editor's `visual.py`, which checked them against the
//! retail maps' collision shapes and textures.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use anvil_fastload::{Codec, Obj, Root, Value};
use anvil_forge::{DataFile, Forge};
use anvil_schema::Schema;

pub mod collision;
pub mod guidance;
pub mod mesh;
pub mod navmesh;
pub mod shading;
pub mod skin;
pub mod texture;

pub use collision::{Collider, CollisionMesh};
pub use guidance::GuidanceEdge;
pub use mesh::StaticMesh;
pub use navmesh::{CrowdFlow, NavGraph, NavMesh};
pub use skin::{MeshBone, SkinnedMesh, SkinnedSubmesh};
pub use shading::{MaterialOverride, MaterialParams, ParamValue, Permutation, ShaderParam, TemplateShaders};
pub use texture::{PixelFormat, Texture};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Forge(#[from] anvil_forge::Error),
    #[error(transparent)]
    Decode(#[from] anvil_fastload::Error),
    #[error("{0}")]
    Missing(String),
}

/// Where a root object lives.
#[derive(Debug, Clone)]
pub struct Loc {
    /// Index into the archive's forges.
    pub forge: usize,
    pub entry: usize,
    pub sub: usize,
    pub type_hash: u32,
    pub name: String,
}

pub struct Archive<'s> {
    schema: &'s Schema,
    codec: Codec<'s>,
    /// (type hash, property name) -> property name hash, resolved against
    /// the type's own property chain: the name table alone is ambiguous.
    field_hashes: RefCell<HashMap<(u32, String), Option<u32>>>,
    forges: Vec<RefCell<Forge>>,
    index: HashMap<u32, Loc>,
    /// uids in archive order
    order: Vec<u32>,
    entries: RefCell<HashMap<(usize, usize), Rc<DataFile>>>,
    roots: RefCell<HashMap<u32, Rc<Root>>>,
}

const ENTRY_CACHE: usize = 256;

/// Object id of a root payload: two status bytes after the import table.
fn payload_uid(payload: &[u8]) -> Option<u32> {
    let pre = if payload.len() >= 8 && payload[0] == 1 {
        12 * i32::from_le_bytes(payload[4..8].try_into().unwrap()).max(0) as usize + 7
    } else {
        0
    };
    payload.get(pre + 2..pre + 6).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

impl<'s> Archive<'s> {
    pub fn open(path: impl AsRef<Path>, schema: &'s Schema) -> Result<Self, Error> {
        Self::open_many(&[path.as_ref()], schema)
    }

    /// Several forges as one object space, the way the game mounts a map on
    /// top of `DataPC.forge`. Earlier forges win when a uid repeats.
    pub fn open_many(paths: &[&Path], schema: &'s Schema) -> Result<Self, Error> {
        let mut index = HashMap::new();
        let mut order = Vec::new();
        let mut forges = Vec::new();
        for (fi, path) in paths.iter().enumerate() {
            let mut forge = Forge::open(path)?;
            let entries = forge.entries().to_vec();
            for e in entries.iter().filter(|e| !e.is_metafile()) {
                let df = DataFile::parse(&forge.read(e)?)?;
                for o in &df.objects {
                    let Some(uid) = payload_uid(&o.payload) else { continue };
                    // first copy wins, as in the engine's loader
                    if let std::collections::hash_map::Entry::Vacant(v) = index.entry(uid) {
                        order.push(uid);
                        v.insert(Loc {
                            forge: fi,
                            entry: e.index,
                            sub: o.index,
                            type_hash: o.type_hash,
                            name: o.name.clone(),
                        });
                    }
                }
            }
            forges.push(RefCell::new(forge));
        }
        Ok(Self {
            schema,
            codec: Codec::new(schema),
            field_hashes: RefCell::default(),
            forges,
            index,
            order,
            entries: RefCell::default(),
            roots: RefCell::default(),
        })
    }

    pub fn schema(&self) -> &'s Schema {
        self.schema
    }

    pub fn loc(&self, uid: u32) -> Option<&Loc> {
        self.index.get(&uid)
    }

    pub fn type_name(&self, uid: u32) -> Option<String> {
        self.loc(uid).map(|l| self.schema.name_or_hash(l.type_hash))
    }

    /// uids of every root of the named type in one forge, in archive order.
    pub fn uids_of_in(&self, type_name: &str, forge: usize) -> Vec<u32> {
        let Some(h) = self.schema.hash_of(type_name) else { return Vec::new() };
        self.order.iter().copied().filter(|u| self.index[u].type_hash == h && self.index[u].forge == forge).collect()
    }

    /// uids of every root of the named type, in archive order.
    pub fn uids_of(&self, type_name: &str) -> Vec<u32> {
        let Some(h) = self.schema.hash_of(type_name) else { return Vec::new() };
        self.order.iter().copied().filter(|u| self.index[u].type_hash == h).collect()
    }

    fn entry(&self, fi: usize, idx: usize) -> Result<Rc<DataFile>, Error> {
        if let Some(df) = self.entries.borrow().get(&(fi, idx)) {
            return Ok(df.clone());
        }
        let mut forge = self.forges[fi].borrow_mut();
        let e = forge.entries()[idx].clone();
        let df = Rc::new(DataFile::parse(&forge.read(&e)?)?);
        let mut cache = self.entries.borrow_mut();
        if cache.len() >= ENTRY_CACHE {
            cache.clear();
        }
        cache.insert((fi, idx), df.clone());
        Ok(df)
    }

    /// Decoded root object by uid (cached).
    pub fn root(&self, uid: u32) -> Result<Rc<Root>, Error> {
        if let Some(r) = self.roots.borrow().get(&uid) {
            return Ok(r.clone());
        }
        let loc = self.index.get(&uid).ok_or_else(|| Error::Missing(format!("object {uid:#010x}")))?;
        let df = self.entry(loc.forge, loc.entry)?;
        let root = Rc::new(self.codec.decode(&df.objects[loc.sub].payload)?);
        self.roots.borrow_mut().insert(uid, root.clone());
        Ok(root)
    }

    /// Field of a decoded object by property name.
    pub fn field<'o>(&self, o: &'o Obj, name: &str) -> Option<&'o Value> {
        let key = (o.type_hash, name.to_owned());
        let cached = self.field_hashes.borrow().get(&key).copied();
        let h = match cached {
            Some(h) => h,
            None => {
                let h = self
                    .schema
                    .property_chain(o.type_hash)
                    .into_iter()
                    .find(|p| self.schema.name(p.name_hash) == Some(name))
                    .map(|p| p.name_hash);
                self.field_hashes.borrow_mut().insert(key, h);
                h
            }
        };
        o.field(h?)
    }

    pub fn is_type(&self, o: &Obj, type_name: &str) -> bool {
        self.schema.hash_of(type_name) == Some(o.type_hash)
    }
}

#[derive(Debug, Clone)]
pub struct Material {
    /// TextureMap uid of TextureSet slot 0.
    pub diffuse: Option<u32>,
    pub alpha_test: bool,
    pub alpha_ref: u8,
    pub two_sided: bool,
    /// 0 = opaque; other values are blended modes not decoded yet.
    pub blend_mode: u32,
    pub disabled: bool,
}

/// One placed instance of a mesh.
#[derive(Debug, Clone)]
pub struct Instance {
    pub entity: u32,
    pub mesh: u32,
    /// The entity is an out-of-bounds wall: invisible in play, shown only
    /// as the boundary effect.
    pub out_of_bounds: bool,
    /// Row-major, translation in the last row (D3D row vectors); read as
    /// column-major this is the column-vector matrix.
    pub matrix: [f32; 16],
}

/// A player spawn point.
#[derive(Debug, Clone)]
pub struct Spawn {
    pub entity: u32,
    pub kind: u32,
    pub team: u32,
    /// Same layout as [`Instance::matrix`].
    pub matrix: [f32; 16],
}

impl Archive<'_> {
    /// LOD0 mesh behind a Visual's target (a LODSelector or a Mesh).
    pub fn lod0_mesh(&self, mut target: u32) -> Option<u32> {
        for _ in 0..4 {
            match self.type_name(target)?.as_str() {
                "Mesh" => return Some(target),
                "LODSelector" => {
                    let root = self.root(target).ok()?;
                    let first = self.field(&root.obj, "LODDescs")?.items().first()?.as_obj()?;
                    target = self.field(first, "Object")?.target_id()?;
                }
                _ => return None,
            }
        }
        None
    }

    /// Every Visual of every Entity root and every entity inlined in an
    /// EntityGroup, resolved to LOD0 meshes.
    pub fn instances(&self) -> Vec<Instance> {
        let mut out = Vec::new();
        let mut visit = |e: &Obj| {
            let Some(m) = self.field(e, "GlobalMatrix").and_then(Value::as_f32s) else { return };
            let Ok(matrix) = <[f32; 16]>::try_from(m) else { return };
            let Some(comps) = self.field(e, "Components") else { return };
            let out_of_bounds =
                comps.items().iter().filter_map(Value::as_obj).any(|c| self.is_type(c, "OutOfBoundsComponent"));
            for c in comps.items().iter().filter_map(Value::as_obj) {
                if !self.is_type(c, "Visual") {
                    continue;
                }
                let target = self.field(c, "Object").and_then(Value::target_id);
                if let Some(mesh) = target.and_then(|t| self.lod0_mesh(t)) {
                    out.push(Instance { entity: e.id, mesh, out_of_bounds, matrix });
                }
            }
        };
        // entities placed by the map itself (forge 0), not library content
        for uid in self.uids_of_in("Entity", 0) {
            if let Ok(r) = self.root(uid) {
                visit(&r.obj);
            }
        }
        // groups nest: every Entity object inside one
        let entity = self.schema.hash_of("Entity");
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
        let mut seen = std::collections::HashSet::new();
        out.retain(|i| seen.insert((i.entity, i.mesh, i.matrix.map(f32::to_bits))));
        out
    }

    /// Player spawn points placed by the map (`MultiSpawnPlayerComponent`
    /// roots): SpawnType 0 standard, 1 tutorial, 2 team, 3 chest. Facing is
    /// local +Y.
    pub fn player_spawns(&self) -> Vec<Spawn> {
        let mut out = Vec::new();
        for uid in self.uids_of_in("Entity", 0) {
            let Ok(r) = self.root(uid) else { continue };
            let Some(Ok(matrix)) = self.field(&r.obj, "GlobalMatrix").and_then(Value::as_f32s).map(<[f32; 16]>::try_from)
            else {
                continue;
            };
            let comps = self.field(&r.obj, "Components").map(Value::items).unwrap_or_default();
            for c in comps.iter().filter_map(Value::as_obj).filter(|c| self.is_type(c, "MultiSpawnPlayerComponent")) {
                let num = |n: &str| self.field(c, n).and_then(Value::as_u32).unwrap_or(0);
                out.push(Spawn { entity: uid, kind: num("SpawnType"), team: num("TeamIndex"), matrix });
            }
        }
        out
    }

    /// Render state and diffuse texture of a Material.
    pub fn material(&self, uid: u32) -> Option<Material> {
        let m = self.root(uid).ok()?;
        let o = &m.obj;
        if !self.is_type(o, "Material") {
            return None;
        }
        let flag = |n: &str| self.field(o, n).and_then(Value::as_bool).unwrap_or(false);
        Some(Material {
            diffuse: self.material_texture(uid, 0),
            alpha_test: flag("AlphaTestEnabled"),
            alpha_ref: self.field(o, "AlphaTestValue").and_then(Value::as_u32).unwrap_or(128) as u8,
            two_sided: flag("TwoSided"),
            blend_mode: self.field(o, "BlendMode").and_then(Value::as_u32).unwrap_or(0),
            disabled: flag("MaterialDisabled"),
        })
    }

    /// TextureMap uid in a Material's TextureSet slot (0 diffuse, 1 normal,
    /// 2 specular).
    pub fn material_texture(&self, material: u32, slot: usize) -> Option<u32> {
        let m = self.root(material).ok()?;
        let set = self.field(&m.obj, "TextureSet")?.target_id()?;
        let set = self.root(set).ok()?;
        let tex = self.field(&set.obj, "Maps")?.items().get(slot)?.target_id()?;
        (self.type_name(tex)? == "TextureMap").then_some(tex)
    }
}

/// ACB's schema with its nameless placeholders filled from Revelations'
/// (`ACB_MP.schema` + `ACR_MP.schema` in `dir`): the layout retail ACB data
/// actually uses.
pub fn load_schema(dir: &Path) -> Result<Schema, anvil_schema::Error> {
    let acb = Schema::load(dir.join("ACB_MP.schema"))?;
    let acr = Schema::load(dir.join("ACR_MP.schema"))?;
    Ok(acb.with_placeholders_filled(&acr))
}

/// A map forge mounted over the core `DataPC.forge` next to it, which holds
/// the shared materials, texture sets and templates maps refer to.
pub fn open_map<'s>(map: &Path, schema: &'s Schema) -> Result<Archive<'s>, Error> {
    let core = map.with_file_name("DataPC.forge");
    let paths: Vec<&Path> = if core.exists() && core != map { vec![map, &core] } else { vec![map] };
    Archive::open_many(&paths, schema)
}
