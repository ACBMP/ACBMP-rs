//! Material parameters and compiled shader permutations.
//!
//! A `MaterialTemplate` names its tweakable inputs in `EditableProperties`;
//! a property's id is the CRC-32 of its `PropertyName` (`Layer0_Diffuse` ->
//! 0x5D09D951). A `Material` stores its values as DynamicProperties keyed by
//! that id: textures as `TextureSelector`s, colours and scales as raw
//! floats. Missing values fall back to the template operator the property is
//! linked to (`ConstantVectorValue.Value` etc.).
//!
//! Each compiled shader permutation lists the properties it reads in
//! `Parameters` (id, register, register count, type: 0/1 = float constant,
//! 2 = sampler). Lighting permutations have no list; they share registers
//! with the plain pixel permutation of the same `ShaderKey`.

use std::collections::HashMap;
use std::rc::Rc;

use anvil_fastload::{Obj, Value};

use crate::Archive;

/// CRC-32 (IEEE), the hash of material property names.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Vector([f32; 4]),
    /// TextureMap uid, `None` when the selector resolves to nothing.
    Texture(Option<u32>),
}

#[derive(Debug, Clone)]
pub struct MaterialParams {
    pub template: u32,
    /// By property id.
    pub values: HashMap<u32, ParamValue>,
}

#[derive(Debug, Clone, Copy)]
pub struct ShaderParam {
    pub property: u32,
    pub register: u32,
    pub count: u32,
    /// 0/1 float constant, 2 sampler.
    pub kind: u32,
}

impl ShaderParam {
    pub fn is_sampler(&self) -> bool {
        self.kind == 2
    }
}

#[derive(Debug, Clone)]
pub struct Permutation {
    pub key: u32,
    /// Lighting permutations only.
    pub lighting_key: Option<u32>,
    pub code: Rc<[u8]>,
    pub params: Vec<ShaderParam>,
}

#[derive(Debug, Clone, Default)]
pub struct TemplateShaders {
    pub vertex: Vec<Permutation>,
    pub pixel: Vec<Permutation>,
    pub lighting: Vec<Permutation>,
}

impl TemplateShaders {
    pub fn vertex(&self, key: u32) -> Option<&Permutation> {
        self.vertex.iter().find(|p| p.key == key)
    }

    pub fn pixel(&self, key: u32) -> Option<&Permutation> {
        self.pixel.iter().find(|p| p.key == key)
    }

    /// Lighting pixel shader with the register layout of `pixel(key)`.
    pub fn lighting(&self, key: u32, lighting_key: u32) -> Option<Permutation> {
        let p = self.lighting.iter().find(|p| p.key == key && p.lighting_key == Some(lighting_key))?;
        let params = self.pixel(key).map(|b| b.params.clone()).unwrap_or_default();
        Some(Permutation { params, ..p.clone() })
    }
}

fn vector(v: &Value) -> Option<[f32; 4]> {
    match v {
        Value::Raw(b) if b.len() == 1 => Some([b[0] as f32, 0.0, 0.0, 0.0]),
        _ => {
            let f = v.as_f32s()?;
            Some(match f.len() {
                0 => return None,
                1 => [f[0], f[0], f[0], f[0]],
                n => std::array::from_fn(|i| if i < n { f[i] } else { 0.0 }),
            })
        }
    }
}

impl Archive<'_> {
    /// Texture a `TextureSelector` resolves to for a material whose own
    /// TextureSet is `material_set` (`TextureSelector::GetTextureMap`:
    /// method 0 = the material's set, 1 = an explicit set, 2 = an explicit
    /// texture; sets are indexed by `MapType`).
    fn selector_texture(&self, sel: &Obj, material_set: Option<u32>) -> Option<u32> {
        let num = |n: &str| self.field(sel, n).and_then(Value::as_u32).unwrap_or(0);
        let explicit = self.field(sel, "Texture").and_then(Value::target_id);
        let set = match num("TextureSpecificationMethod") {
            0 => material_set,
            1 => self.field(sel, "TextureSet").and_then(Value::target_id),
            2 => return explicit,
            _ => return None,
        };
        let from_set = set.and_then(|s| {
            let r = self.root(s).ok()?;
            self.field(&r.obj, "Maps")?.items().get(num("MapType") as usize)?.target_id()
        });
        from_set.or(explicit).filter(|t| self.type_name(*t).as_deref() == Some("TextureMap"))
    }

    fn param_value(&self, v: &Value, material_set: Option<u32>) -> Option<ParamValue> {
        match v.as_obj() {
            Some(o) if self.is_type(o, "TextureSelector") => {
                Some(ParamValue::Texture(self.selector_texture(o, material_set)))
            }
            Some(_) => None,
            None => vector(v).map(ParamValue::Vector),
        }
    }

    /// Template and parameter values of a Material.
    pub fn material_params(&self, uid: u32) -> Option<MaterialParams> {
        let m = self.root(uid).ok()?;
        let o = &m.obj;
        if !self.is_type(o, "Material") {
            return None;
        }
        let template = self.field(o, "MaterialTemplate")?.target_id()?;
        let set = self.field(o, "TextureSet").and_then(Value::target_id);
        let mut values = HashMap::new();

        // template defaults from the operators the properties are linked to
        if let Ok(t) = self.root(template) {
            let ops: HashMap<u32, &Obj> = self
                .field(&t.obj, "MaterialDefinition")
                .and_then(Value::as_obj)
                .and_then(|d| self.field(d, "Operators"))
                .map(|v| v.items().iter().filter_map(Value::as_obj).map(|o| (o.id, o)).collect())
                .unwrap_or_default();
            for p in self.field(&t.obj, "EditableProperties").map(Value::items).unwrap_or_default() {
                let Some(p) = p.as_obj() else { continue };
                let Some(name) = self.field(p, "PropertyName").and_then(Value::as_bytes) else { continue };
                let Some(op) = self.field(p, "LinkedOperator").and_then(Value::target_id).and_then(|id| ops.get(&id))
                else {
                    continue;
                };
                let default = self
                    .field(op, "Value")
                    .and_then(|v| self.param_value(v, set))
                    .or_else(|| self.field(op, "Texture").and_then(|v| self.param_value(v, set)));
                if let Some(v) = default {
                    values.insert(crc32(name), v);
                }
            }
        }
        for d in o.dyn_props.iter().flatten() {
            if let Some(v) = self.param_value(&d.value, set) {
                values.insert(d.name_hash, v);
            }
        }
        Some(MaterialParams { template, values })
    }

    /// Compiled permutations of a MaterialTemplate (primary platform).
    pub fn template_shaders(&self, uid: u32) -> Option<TemplateShaders> {
        let t = self.root(uid).ok()?;
        let ct = self.field(&t.obj, "CompiledTemplatePrimary")?.as_obj()?;
        let list = |name: &str, lighting: bool| -> Vec<Permutation> {
            let num = |o: &Obj, n: &str| self.field(o, n).and_then(Value::as_u32).unwrap_or(0);
            self.field(ct, name)
                .map(Value::items)
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_obj)
                .map(|p| Permutation {
                    key: num(p, "ShaderKey"),
                    lighting_key: lighting.then(|| num(p, "LightingKey")),
                    code: self.field(p, "CompiledCode").and_then(Value::as_bytes).unwrap_or_default().into(),
                    params: self
                        .field(p, "Parameters")
                        .map(Value::items)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(Value::as_obj)
                        .map(|q| ShaderParam {
                            property: num(q, "PropertyID"),
                            register: num(q, "ConstantIndex"),
                            count: num(q, "NbVectors").max(1),
                            kind: num(q, "Type"),
                        })
                        .collect(),
                })
                .collect()
        };
        Some(TemplateShaders {
            vertex: list("VertexShaderPermutations", false),
            pixel: list("PixelShaderPermutations", false),
            lighting: list("LightingPermutations", true),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn property_ids() {
        assert_eq!(super::crc32(b"Layer0_Diffuse"), 0x5D09D951);
        assert_eq!(super::crc32(b"Layer0_UVScale"), 0xE1E639A5);
    }
}
