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

pub mod mesh;
pub mod texture;

pub use mesh::StaticMesh;
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
    field_hashes: HashMap<&'s str, u32>,
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
        let field_hashes = schema.names.iter().map(|(h, n)| (n.as_str(), *h)).collect();
        Ok(Self {
            schema,
            codec: Codec::new(schema),
            field_hashes,
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
        o.field(*self.field_hashes.get(name)?)
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
        for uid in self.uids_of_in("EntityGroup", 0) {
            if let Ok(r) = self.root(uid) {
                visit(&r.obj);
                if let Some(children) = self.field(&r.obj, "Entities") {
                    for c in children.items().iter().filter_map(Value::as_obj) {
                        visit(c);
                    }
                }
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
