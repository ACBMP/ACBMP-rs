//! Characters: an Entity's skinned Visuals bound to its skeletons.
//!
//! An `Entity` lists `Visual` components (`Object` = a Mesh) and one
//! `SkeletonComponent` (`MainSkeleton` + `AddonSkeletons`). Addon skeletons
//! (skirt, hood, weapons, head) hang off the main one: an addon bone whose
//! `BoneID` already exists is that bone. A `MaterialOverrider` component and
//! each Visual's `MaterialInfos` swap the meshes' placeholder `*_Empty`
//! materials for the real ones.
//!
//! Mesh bones carry `PackedInitialMatrixInverse`, the inverse bind matrix:
//! J and K are i16/32767 vectors, the rotation's columns are
//! (J x K, J, K), and TransA | TransB << 32 holds three signed 21-bit
//! translation fields in 1/65536 m. Mesh space is the skeleton's model
//! space turned 90 degrees about Z, so a character's bind pose renders
//! turned too; checked by every bone's bind origin sitting on the centroid
//! of the vertices it carries.
//!
//! Visuals point at a Mesh or at a `LODSelector`, whose `LODDescs[0]`
//! holds the nearest LOD (a null object there draws nothing up close).
//!
//! `EntityBuilder`s (`CU_*`, `AC2MP_IDxx_MainTemplate_*`) and the
//! `BuildTable`s they nest patch a template Entity: each `BuildColumn` has a
//! `Pass` and a property path, `Rows[0]` holds a value per `ColumnID` as a
//! dynamic property. Applied here: pass 5 with an empty path adds the value
//! (a GraphicObject becomes a Visual, a BuildTable is applied in turn), and
//! a material TextureSet column (`MaterialPropertyPathNodeSolver` node, then
//! `TextureSet`) gives shared materials such as `UA_Ezio_Head` their maps.
//! Other columns (anim/sound sets, voices) are ignored.

use std::collections::HashMap;

use acb_world::{Archive, SkinnedMesh};
use anvil_fastload::{Obj, Value};
use glam::{Mat4, Vec3, Vec4};

use crate::pose::{Bone, Skeleton};

#[derive(Debug, Clone)]
pub struct CharacterPart {
    pub mesh_uid: u32,
    pub mesh: SkinnedMesh,
    /// Material per submesh after overrides, with the TextureSet an
    /// EntityBuilder assigns it.
    pub materials: Vec<(u32, Option<u32>)>,
    /// Mesh bone -> skeleton bone; None only when no bone of the mesh maps.
    pub bone_map: Vec<Option<usize>>,
    /// Per mesh bone: mesh space -> bone space at bind time.
    pub inverse_bind: Vec<Mat4>,
}

#[derive(Debug, Clone)]
pub struct Character {
    pub entity: u32,
    pub skeleton: Skeleton,
    pub parts: Vec<CharacterPart>,
    /// The Entity's MaterialOverrider: placeholder -> real material.
    overrides: HashMap<u32, u32>,
    /// Builder-assigned TextureSets per material.
    texture_sets: HashMap<u32, u32>,
}

/// Decodes a `PackedOrthonormalMatrix` (see the module docs).
pub fn unpack_inverse_bind(j: [u16; 6], trans_a: u32, trans_b: u32) -> Mat4 {
    let s = |v: u16| v as i16 as f32 / 32767.0;
    let jv = Vec3::new(s(j[0]), s(j[1]), s(j[2]));
    let k = Vec3::new(s(j[3]), s(j[4]), s(j[5]));
    let x = jv.cross(k);
    let y = jv;
    let packed = trans_a as u64 | (trans_b as u64) << 32;
    let field = |i: u32| {
        let v = ((packed >> (21 * i)) & 0x1f_ffff) as i32;
        (if v >= 1 << 20 { v - (1 << 21) } else { v }) as f32 / 65536.0
    };
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), k.extend(0.0), Vec4::new(field(0), field(1), field(2), 1.0))
}

impl Skeleton {
    /// Appends an addon skeleton's bones; bones whose id already exists are
    /// shared, other roots attach to bone 0.
    pub fn attach(&mut self, addon: &Skeleton) {
        let start = self.bones.len();
        let remap: Vec<usize> = addon
            .bones
            .iter()
            .scan(start, |next, b| {
                Some(match self.bones[..start].iter().position(|m| m.id == b.id) {
                    Some(i) => i,
                    None => {
                        *next += 1;
                        *next - 1
                    }
                })
            })
            .collect();
        for (ai, b) in addon.bones.iter().enumerate() {
            if remap[ai] >= start {
                self.bones.push(Bone { parent: Some(b.parent.map(|p| remap[p]).unwrap_or(0)), ..b.clone() });
            }
        }
    }
}

fn components<'a>(archive: &Archive, o: &'a Obj) -> Vec<&'a Obj> {
    archive.field(o, "Components").map(Value::items).unwrap_or_default().iter().filter_map(Value::as_obj).collect()
}

impl Character {
    pub fn load(archive: &Archive, entity: u32) -> Option<Character> {
        let root = archive.root(entity).ok()?;
        let comps = components(archive, &root.obj);

        let sc = comps.iter().find(|c| archive.is_type(c, "SkeletonComponent"))?;
        let mut skeleton = Skeleton::load(archive, archive.field(sc, "MainSkeleton")?.target_id()?)?;
        for a in archive.field(sc, "AddonSkeletons").map(Value::items).unwrap_or_default() {
            if let Some(s) = a.target_id().and_then(|u| Skeleton::load(archive, u)) {
                skeleton.attach(&s);
            }
        }

        let mut overrides: HashMap<u32, u32> = HashMap::new();
        for c in comps.iter().filter(|c| archive.is_type(c, "MaterialOverrider")) {
            for d in archive.field(c, "MaterialOverrides").map(Value::items).unwrap_or_default() {
                let Some(d) = d.as_obj() else { continue };
                let get = |n: &str| archive.field(d, n).and_then(Value::target_id).filter(|u| *u != 0);
                if let (Some(a), Some(b)) = (get("MaterialToReplace"), get("NewMaterial")) {
                    overrides.insert(a, b);
                }
            }
        }

        let mut c = Character { entity, skeleton, parts: Vec::new(), overrides, texture_sets: HashMap::new() };
        for v in comps.iter().filter(|c| archive.is_type(c, "Visual")) {
            let Some(object) = archive.field(v, "Object").and_then(Value::target_id) else { continue };
            let mut local = HashMap::new();
            if let Some(infos) = archive
                .field(v, "InstanceData")
                .and_then(Value::as_obj)
                .and_then(|d| archive.field(d, "MaterialInfos"))
            {
                for i in infos.items().iter().filter_map(Value::as_obj) {
                    let get = |n: &str| archive.field(i, n).and_then(Value::target_id).filter(|u| *u != 0);
                    if let (Some(a), Some(b)) = (get("MeshMaterial"), get("InstanceMaterial")) {
                        local.insert(a, b);
                    }
                }
            }
            c.add_visual(archive, object, &local);
        }
        Some(c)
    }

    /// Adds a Visual's object (a Mesh, or a LODSelector's nearest LOD) if it
    /// is a skinned mesh; `instance_materials` are the Visual's own
    /// MaterialInfos, weaker than the Entity's MaterialOverrider.
    pub fn add_visual(&mut self, archive: &Archive, object: u32, instance_materials: &HashMap<u32, u32>) {
        let mesh_uid = match archive.type_name(object).as_deref() {
            Some("LODSelector") => {
                let Ok(root) = archive.root(object) else { return };
                let lod0 = archive.field(&root.obj, "LODDescs").map(Value::items).unwrap_or_default().first().and_then(Value::as_obj);
                match lod0.and_then(|d| archive.field(d, "Object")).and_then(Value::target_id).filter(|u| *u != 0) {
                    Some(m) => m,
                    None => return,
                }
            }
            _ => object,
        };
        // a Cloth state's visual is usually also added as a plain part
        if self.parts.iter().any(|p| p.mesh_uid == mesh_uid) {
            return;
        }
        let Ok(Some(mesh)) = archive.skinned_mesh(mesh_uid) else { return };
        let bone_index: HashMap<u32, usize> = self.skeleton.bones.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
        let materials = mesh
            .submeshes
            .iter()
            .map(|s| {
                let m = self.overrides.get(&s.material).or(instance_materials.get(&s.material)).copied().unwrap_or(s.material);
                (m, self.texture_sets.get(&m).copied())
            })
            .collect();
        let mut bone_map: Vec<Option<usize>> = mesh.bones.iter().map(|b| bone_index.get(&b.id).copied()).collect();
        let mut inverse_bind: Vec<Mat4> =
            mesh.bones.iter().map(|b| unpack_inverse_bind(b.packed.0, b.packed.1, b.packed.2)).collect();
        // Bones no listed skeleton has (MA_Ezio_LOD0's eyes) ride rigidly
        // on the mapped bone nearest their bind origin.
        let origin = |m: &Mat4| m.inverse().w_axis.truncate();
        for i in 0..bone_map.len() {
            if bone_map[i].is_some() {
                continue;
            }
            let o = origin(&inverse_bind[i]);
            if let Some(j) = (0..bone_map.len())
                .filter(|&j| bone_map[j].is_some())
                .min_by(|&a, &b| origin(&inverse_bind[a]).distance(o).total_cmp(&origin(&inverse_bind[b]).distance(o)))
            {
                bone_map[i] = bone_map[j];
                inverse_bind[i] = inverse_bind[j];
            }
        }
        self.parts.push(CharacterPart { mesh_uid, mesh, materials, bone_map, inverse_bind });
    }

    /// Builds an `EntityBuilder`'s template Entity and applies the builder.
    pub fn load_built(archive: &Archive, builder: u32) -> Option<Character> {
        let root = archive.root(builder).ok()?;
        if !archive.is_type(&root.obj, "EntityBuilder") {
            return None;
        }
        let mut c = Character::load(archive, archive.field(&root.obj, "Template")?.target_id()?)?;
        c.apply_table(archive, &root.obj, 0);
        // the persona's default outfit and colours (Custom1..3, Colors;
        // names are irregular, e.g. ID17_Cutom3_ext); the game picks these
        // tables by the player's customization, nothing in the data links them
        let name = archive.loc(builder).map(|l| l.name.to_ascii_lowercase()).unwrap_or_default();
        if let Some(id) = name.strip_prefix("ac2mp_").and_then(|n| n.split('_').next()) {
            let prefix = format!("ac2mp_{id}_");
            let mut tables: Vec<(String, u32)> = archive
                .uids_of("BuildTableExt")
                .into_iter()
                .filter_map(|u| Some((archive.loc(u)?.name.to_ascii_lowercase(), u)))
                .filter(|(n, _)| n.starts_with(&prefix))
                .collect();
            tables.sort();
            for (_, t) in tables {
                c.apply_build_table(archive, t);
            }
        }
        Some(c)
    }

    /// Applies the first row (the default choice) of a BuildTable or
    /// BuildTableExt, e.g. a persona's `AC2MP_IDxx_Custom1_ext` outfit.
    pub fn apply_build_table(&mut self, archive: &Archive, table: u32) {
        if let Ok(t) = archive.root(table) {
            self.apply_table(archive, &t.obj, 0);
        }
    }

    fn apply_table(&mut self, archive: &Archive, table: &Obj, depth: usize) {
        const TEXTURE_SET: u32 = 0xd70e_6670; // CRC32("TextureSet")
        if depth > 8 {
            return;
        }
        let row = archive.field(table, "Rows").map(Value::items).unwrap_or_default().first().and_then(Value::as_obj);
        let values: HashMap<u32, &Value> =
            row.and_then(|r| r.dyn_props.as_ref()).into_iter().flatten().map(|d| (d.name_hash, &d.value)).collect();
        let (mut adds, mut sets) = (Vec::new(), Vec::new());
        for col in archive.field(table, "Columns").map(Value::items).unwrap_or_default().iter().filter_map(Value::as_obj) {
            let Some(id) = archive.field(col, "ColumnID").and_then(Value::as_u32) else { continue };
            let Some(value) = values.get(&id).and_then(|v| v.target_id()).filter(|u| *u != 0) else { continue };
            let pass = archive.field(col, "Pass").and_then(Value::as_u32);
            let nodes = archive
                .field(col, "TargetProperty")
                .and_then(Value::as_obj)
                .and_then(|p| archive.field(p, "PathNodes"))
                .map(Value::items)
                .unwrap_or_default();
            match (pass, nodes) {
                (Some(5), []) => adds.push((archive.type_name(value).unwrap_or_default(), value)),
                (_, [first, second]) => {
                    let material = first
                        .as_obj()
                        .and_then(|n| archive.field(n, "NodeSolver"))
                        .and_then(Value::as_obj)
                        .filter(|s| archive.is_type(s, "MaterialPropertyPathNodeSolver"))
                        .and_then(|s| archive.field(s, "TargetMaterial"))
                        .and_then(Value::target_id);
                    let property = second.as_obj().and_then(|n| archive.field(n, "TargetPropertyID")).and_then(Value::as_u32);
                    if let (Some(material), Some(TEXTURE_SET)) = (material, property) {
                        sets.push((material, value));
                    }
                }
                _ => {}
            }
        }
        // add-on skeletons first: the parts added next bind to their bones
        adds.sort_by_key(|(t, _)| t != "Skeleton");
        for (ty, value) in adds {
            match ty.as_str() {
                "Skeleton" => {
                    if let Some(s) = Skeleton::load(archive, value) {
                        self.skeleton.attach(&s);
                    }
                }
                "BuildTable" | "BuildTableExt" => self.apply_build_table(archive, value),
                "Mesh" | "LODSelector" => self.add_visual(archive, value, &HashMap::new()),
                // soft-body cloth: its first state's visual, skinned only (no simulation yet)
                "Cloth" => {
                    let visual = archive.root(value).ok().and_then(|r| {
                        let state = archive.field(&r.obj, "SoftBodyStates")?.items().first()?.as_obj()?;
                        archive.field(state, "GraphicObject")?.target_id()
                    });
                    if let Some(v) = visual.filter(|u| *u != 0) {
                        self.add_visual(archive, v, &HashMap::new());
                    }
                }
                _ => {}
            }
        }
        for (material, set) in sets {
            self.texture_sets.insert(material, set);
            for m in self.parts.iter_mut().flat_map(|p| p.materials.iter_mut()).filter(|m| m.0 == material) {
                m.1 = Some(set);
            }
        }
    }

    /// Skinning matrices (mesh space -> model space) of every mesh bone of
    /// every part, for skeleton bone matrices `globals`.
    pub fn skin_matrices(&self, globals: &[Mat4]) -> Vec<Vec<Mat4>> {
        self.parts
            .iter()
            .map(|p| {
                p.bone_map
                    .iter()
                    .zip(&p.inverse_bind)
                    .map(|(b, inv)| b.and_then(|b| globals.get(b)).map(|g| *g * *inv).unwrap_or(Mat4::IDENTITY))
                    .collect()
            })
            .collect()
    }
}
