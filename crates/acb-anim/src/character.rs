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

use std::collections::HashMap;

use acb_world::{Archive, SkinnedMesh};
use anvil_fastload::{Obj, Value};
use glam::{Mat4, Vec3, Vec4};

use crate::pose::{Bone, Skeleton};

#[derive(Debug, Clone)]
pub struct CharacterPart {
    pub mesh_uid: u32,
    pub mesh: SkinnedMesh,
    /// Material per submesh after overrides.
    pub materials: Vec<u32>,
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
        let bone_index: HashMap<u32, usize> = skeleton.bones.iter().enumerate().map(|(i, b)| (b.id, i)).collect();

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

        let mut parts = Vec::new();
        for v in comps.iter().filter(|c| archive.is_type(c, "Visual")) {
            let Some(mesh_uid) = archive.field(v, "Object").and_then(Value::target_id) else { continue };
            let Ok(Some(mesh)) = archive.skinned_mesh(mesh_uid) else { continue };
            let mut local = overrides.clone();
            if let Some(infos) = archive
                .field(v, "InstanceData")
                .and_then(Value::as_obj)
                .and_then(|d| archive.field(d, "MaterialInfos"))
            {
                for i in infos.items().iter().filter_map(Value::as_obj) {
                    let get = |n: &str| archive.field(i, n).and_then(Value::target_id).filter(|u| *u != 0);
                    if let (Some(a), Some(b)) = (get("MeshMaterial"), get("InstanceMaterial")) {
                        local.entry(a).or_insert(b);
                    }
                }
            }
            let materials = mesh.submeshes.iter().map(|s| local.get(&s.material).copied().unwrap_or(s.material)).collect();
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
            parts.push(CharacterPart { mesh_uid, mesh, materials, bone_map, inverse_bind });
        }
        Some(Character { entity, skeleton, parts })
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
