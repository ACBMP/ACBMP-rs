//! Decoder for Anvil "FastLoad" object payloads (the sub-objects of a `.data`
//! container), driven by the reflection schema.
//!
//! Rules follow anvilforge-py's `fastload.py`, which took them from the Mac
//! build's `FastLoadSerializer` and checked them against ~20k retail objects:
//!
//! - A property is on disk iff `flags & 0x2000000`; BOOLs are one byte.
//! - Root: optional import-table pre-header, `status(1) flag(1) id(4) hash(4)`.
//! - Pointer: status byte. 3 = null; 1/2/5 = link + id(4); 0/4 = inline object
//!   (`[flag(1) if ManagedObject-derived] id(4) hash(4) props`).
//! - OBJECT: `[flag(1) if managed] id(4) hash(4) props`; BASE_OBJECT has no flag.
//! - REFERENCE: `tag(1) extra(1) id(4)`, tag 0 = object inlined (`hash(4) props`).
//! - HANDLE: `tag(1) id(4)`. STRING/LSTRING: `u32 len` then `len + 1` units.
//! - Arrays: `u32 count` (static arrays: schema count) + elements.
//! - Some classes append DynamicProperties or a hand-written tail.
//!
//! Classes with other hand-written serializers (compiled animation tracks,
//! shader permutations, FX code tables, a few cinematic components) fail
//! with [`Error`]; callers treat those payloads as opaque.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use anvil_schema::{Kind, Property, Schema};

const SERIALIZED: u32 = 0x0200_0000;
const MAX_COUNT: u32 = 10_000_000;

/// Classes whose FastLoad appends a DynamicProperties block.
const DYNAMIC_PROPERTY_CLASSES: &[&str] = &[
    "FXCommand", "Material", "BuildColumn", "BuildRow", "FXProperty", "FXDefaultTable",
    "FXDefaultTableInstance", "FXConstantTable", "FXConstantTableInstance", "GenericObject",
    "PropertyControllerEntry", "DynamicPropertiesSet",
];

#[derive(Debug, thiserror::Error)]
#[error("{msg} at byte {pos} ({path})")]
pub struct Error {
    pub msg: String,
    pub pos: usize,
    pub path: String,
}

/// A decoded field value. Primitive bytes are kept raw (little-endian).
#[derive(Debug, Clone)]
pub enum Value {
    /// Primitive, math, enum or object-id bytes.
    Raw(Vec<u8>),
    Handle { tag: u8, id: u32 },
    Ref { tag: u8, extra: u8, id: u32, obj: Option<Box<Obj>> },
    /// String bytes without the terminator (UTF-16LE for LSTRING).
    Str(Vec<u8>),
    Object(Box<Obj>),
    /// `status` is `None` for a bare id inside DynamicProperties.
    Ptr { status: Option<u8>, link: Option<u32>, obj: Option<Box<Obj>> },
    /// Fixed-size elements, stored contiguously.
    Packed { elem_size: usize, data: Vec<u8> },
    Array(Vec<Value>),
}

#[derive(Debug, Clone)]
pub struct DynProp {
    pub name_hash: u32,
    pub object_hash: u32,
    pub bits: u32,
    pub value: Value,
}

#[derive(Debug, Clone)]
pub struct Obj {
    pub type_hash: u32,
    pub id: u32,
    /// ManagedObject flag byte, when present.
    pub flag: Option<u8>,
    /// (property name hash, value), base class first.
    pub fields: Vec<(u32, Value)>,
    pub dyn_props: Option<Vec<DynProp>>,
    /// Raw bytes of a hand-written tail (navmesh triangles, waypoints).
    pub custom: Option<Vec<u8>>,
}

impl Obj {
    pub fn field(&self, name_hash: u32) -> Option<&Value> {
        self.fields.iter().find(|(h, _)| *h == name_hash).map(|(_, v)| v)
    }

    /// Depth-first visit of this object and every object nested in it.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Obj)) {
        fn value<'a>(v: &'a Value, f: &mut impl FnMut(&'a Obj)) {
            match v {
                Value::Object(o) => o.walk(f),
                Value::Ref { obj: Some(o), .. } | Value::Ptr { obj: Some(o), .. } => o.walk(f),
                Value::Array(items) => items.iter().for_each(|i| value(i, f)),
                _ => {}
            }
        }
        f(self);
        for (_, v) in &self.fields {
            value(v, f);
        }
        for d in self.dyn_props.iter().flatten() {
            value(&d.value, f);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Root {
    pub pre_header: Vec<u8>,
    pub status: u8,
    pub obj: Obj,
}

enum Tail {
    /// NavMeshTriangle: four u8 counts, then that many u16 in total.
    NavMeshTriangle,
    /// WayPoint: u8 link count, (u16, u16) links, seven u16.
    WayPoint,
    /// ObjectWayPoint: u8 count, (u16, u16) links.
    Links,
}

struct Level<'s> {
    props: Vec<&'s Property>,
    dynamic: bool,
    tail: Option<Tail>,
}

pub struct Codec<'s> {
    schema: &'s Schema,
    dynamic: HashSet<u32>,
    tails: HashMap<u32, fn() -> Tail>,
    managed_hash: Option<u32>,
    levels: RefCell<HashMap<u32, std::rc::Rc<Vec<Level<'s>>>>>,
    managed: RefCell<HashMap<u32, bool>>,
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'s> Codec<'s> {
    pub fn new(schema: &'s Schema) -> Self {
        let dynamic = DYNAMIC_PROPERTY_CLASSES.iter().filter_map(|n| schema.hash_of(n)).collect();
        let mut tails: HashMap<u32, fn() -> Tail> = HashMap::new();
        for (name, t) in [
            ("NavMeshTriangle", (|| Tail::NavMeshTriangle) as fn() -> Tail),
            ("WayPoint", || Tail::WayPoint),
            ("ObjectWayPoint", || Tail::Links),
        ] {
            if let Some(h) = schema.hash_of(name) {
                tails.insert(h, t);
            }
        }
        Self {
            schema,
            dynamic,
            tails,
            managed_hash: schema.hash_of("ManagedObject"),
            levels: RefCell::default(),
            managed: RefCell::default(),
        }
    }

    pub fn schema(&self) -> &'s Schema {
        self.schema
    }

    fn levels(&self, h: u32) -> std::rc::Rc<Vec<Level<'s>>> {
        if let Some(l) = self.levels.borrow().get(&h) {
            return l.clone();
        }
        let mut chain = Vec::new();
        let mut cur = self.schema.type_def(h);
        while let Some(t) = cur {
            chain.push(t);
            cur = self.schema.type_def(t.base_type_hash);
        }
        let lv: Vec<Level> = chain
            .into_iter()
            .rev()
            .map(|t| Level {
                props: t.properties.iter().filter(|p| p.flags & SERIALIZED != 0).collect(),
                dynamic: self.dynamic.contains(&t.type_hash),
                tail: self.tails.get(&t.type_hash).map(|f| f()),
            })
            .collect();
        let lv = std::rc::Rc::new(lv);
        self.levels.borrow_mut().insert(h, lv.clone());
        lv
    }

    fn is_managed(&self, h: u32) -> bool {
        if let Some(&m) = self.managed.borrow().get(&h) {
            return m;
        }
        let m = self.managed_hash.is_some_and(|mh| self.schema.is_a(h, mh));
        self.managed.borrow_mut().insert(h, m);
        m
    }

    /// Decodes one sub-object payload, which must be consumed exactly.
    pub fn decode(&self, data: &[u8]) -> Result<Root, Error> {
        let mut r = Reader { d: data, p: 0 };
        let pre = if data.len() >= 8 && data[0] == 1 {
            (12 * i32::from_le_bytes(data[4..8].try_into().unwrap()) as i64 + 7).max(0) as usize
        } else {
            0
        };
        let root = "root";
        let pre_header = r.take(pre, root)?.to_vec();
        let status = r.u8(root)?;
        let flag = r.u8(root)?;
        let id = r.u32(root)?;
        let h = r.u32(root)?;
        let mut obj = self.props(&mut r, h, id, root)?;
        obj.flag = Some(flag);
        if r.p != data.len() {
            return Err(r.err(format!("{} trailing bytes", data.len() - r.p), &self.schema.name_or_hash(h)));
        }
        Ok(Root { pre_header, status, obj })
    }

    fn props(&self, r: &mut Reader, h: u32, id: u32, path: &str) -> Result<Obj, Error> {
        if self.schema.type_def(h).is_none() {
            return Err(r.err(format!("unknown type {h:#x}"), path));
        }
        let mut obj = Obj { type_hash: h, id, flag: None, fields: Vec::new(), dyn_props: None, custom: None };
        for level in self.levels(h).iter() {
            for p in &level.props {
                let v = self.value(r, p.kind(), p.elem_kind(), p.static_count(), p.object_hash(), false, path)?;
                obj.fields.push((p.name_hash, v));
            }
            if level.dynamic {
                obj.dyn_props = Some(self.dyn_props(r, path)?);
            }
            if let Some(t) = &level.tail {
                let start = r.p;
                match t {
                    Tail::NavMeshTriangle => {
                        let counts = r.take(4, path)?;
                        let n: usize = counts.iter().map(|&c| c as usize).sum();
                        r.take(2 * n, path)?;
                    }
                    Tail::WayPoint => {
                        let n = r.u8(path)? as usize;
                        r.take(4 * n + 14, path)?;
                    }
                    Tail::Links => {
                        let n = r.u8(path)? as usize;
                        r.take(4 * n, path)?;
                    }
                }
                obj.custom = Some(r.d[start..r.p].to_vec());
            }
        }
        Ok(obj)
    }

    fn inline(&self, r: &mut Reader, with_flag: bool, path: &str) -> Result<Obj, Error> {
        let flag = if with_flag { Some(r.u8(path)?) } else { None };
        let id = r.u32(path)?;
        let h = r.u32(path)?;
        let mut obj = self.props(r, h, id, path)?;
        obj.flag = flag;
        Ok(obj)
    }

    #[allow(clippy::too_many_arguments)]
    fn value(
        &self,
        r: &mut Reader,
        kind: Kind,
        elem: Kind,
        count: usize,
        oh: u32,
        generic: bool,
        path: &str,
    ) -> Result<Value, Error> {
        use Kind::*;
        if let Some(n) = kind.primitive_size() {
            return Ok(Value::Raw(r.take(n, path)?.to_vec()));
        }
        Ok(match kind {
            Enum | ObjectId => Value::Raw(r.take(4, path)?.to_vec()),
            Handle => Value::Handle { tag: r.u8(path)?, id: r.u32(path)? },
            Reference => {
                let tag = r.u8(path)?;
                let extra = r.u8(path)?;
                let id = r.u32(path)?;
                let obj = if tag == 0 {
                    let h = r.u32(path)?;
                    Some(Box::new(self.props(r, h, id, path)?))
                } else {
                    None
                };
                Value::Ref { tag, extra, id, obj }
            }
            String | LString => {
                let n = r.u32(path)?;
                if n > MAX_COUNT {
                    return Err(r.err(format!("bad string length {n}"), path));
                }
                let w = if kind == LString { 2 } else { 1 };
                if n == 0 {
                    Value::Str(Vec::new())
                } else {
                    let b = r.take((n as usize + 1) * w, path)?;
                    Value::Str(b[..b.len() - w].to_vec())
                }
            }
            Object => Value::Object(Box::new(self.inline(r, self.is_managed(oh), path)?)),
            BaseObject => Value::Object(Box::new(self.inline(r, false, path)?)),
            ObjectPtr | BaseObjectPtr => {
                if generic && kind == ObjectPtr {
                    return Ok(Value::Ptr { status: None, link: Some(r.u32(path)?), obj: None });
                }
                match r.u8(path)? {
                    3 => Value::Ptr { status: Some(3), link: None, obj: None },
                    s @ (1 | 2 | 5) => Value::Ptr { status: Some(s), link: Some(r.u32(path)?), obj: None },
                    s @ (0 | 4) => Value::Ptr {
                        status: Some(s),
                        link: None,
                        obj: Some(Box::new(self.inline(r, self.is_managed(oh), path)?)),
                    },
                    s => return Err(r.err(format!("unknown pointer status {s}"), path)),
                }
            }
            StaticArray | BigArray | SmallArray => {
                let n = if kind == StaticArray { count as u32 } else { r.u32(path)? };
                if n > MAX_COUNT {
                    return Err(r.err(format!("bad array count {n}"), path));
                }
                let n = n as usize;
                let width = match elem {
                    Enum | ObjectId => Some(4),
                    e => e.primitive_size(),
                };
                if let Some(w) = width {
                    Value::Packed { elem_size: w, data: r.take(n * w, path)?.to_vec() }
                } else {
                    let mut items = Vec::with_capacity(n.min(4096));
                    for _ in 0..n {
                        items.push(self.value(r, elem, Bool, 0, oh, false, path)?);
                    }
                    Value::Array(items)
                }
            }
            k => return Err(r.err(format!("unhandled kind {k:?}"), path)),
        })
    }

    fn dyn_props(&self, r: &mut Reader, path: &str) -> Result<Vec<DynProp>, Error> {
        let n = r.u32(path)?;
        if n > 100_000 {
            return Err(r.err(format!("bad dynamic property count {n}"), path));
        }
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let name_hash = r.u32(path)?;
            let object_hash = r.u32(path)?;
            let bits = r.u32(path)?;
            let code = (bits >> 16) & 0x3f;
            if code > 31 {
                return Err(r.err(format!("unhandled dynamic kind {code}"), path));
            }
            let value = self.value(
                r,
                Kind::from_code(code as u8),
                Kind::from_code(((bits >> 23) & 0x1f) as u8),
                (bits & 0xffff) as usize,
                object_hash,
                true,
                path,
            )?;
            out.push(DynProp { name_hash, object_hash, bits, value });
        }
        Ok(out)
    }
}

impl Reader<'_> {
    fn err(&self, msg: String, path: &str) -> Error {
        Error { msg, pos: self.p, path: path.to_owned() }
    }
    fn take(&mut self, n: usize, path: &str) -> Result<&[u8], Error> {
        match self.p.checked_add(n).and_then(|e| self.d.get(self.p..e)) {
            Some(s) => {
                self.p += n;
                Ok(s)
            }
            None => Err(self.err(format!("eof reading {n}"), path)),
        }
    }
    fn u8(&mut self, path: &str) -> Result<u8, Error> {
        Ok(self.take(1, path)?[0])
    }
    fn u32(&mut self, path: &str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4, path)?.try_into().unwrap()))
    }
}
