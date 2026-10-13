//! Anvil `SCHM` reflection schema: the serialized layout of every engine type.
//!
//! Format and bit layout follow anvilforge-py's `schema.py`, which was
//! verified by decoding retail ACB MP objects to their exact byte length.

use std::collections::HashMap;
use std::path::Path;

const SCHM_MAGIC: u32 = u32::from_le_bytes(*b"SCHM");
/// Bytes of file header counted by the header's `data_length` field.
const HEADER_LEN: usize = 33;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("schema: {0}")]
    Format(String),
}

/// Storage kind of a property (bits 48..53 of the packed type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Bool = 0,
    Char,
    Int8,
    UInt8,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    UInt64,
    Float,
    Vector2,
    Vector3,
    Vector4,
    Quaternion,
    Matrix3x3,
    Matrix4x4,
    ObjectId,
    Handle,
    Object,
    ObjectPtr,
    BaseObjectPtr,
    BaseObject,
    StaticArray,
    BigArray,
    Enum,
    String,
    LString,
    Reference,
    SmallArray,
    SloppyHandle,
    DataDrivenEnum,
}

impl Kind {
    /// Every 5-bit code is assigned, in declaration order.
    pub fn from_code(code: u8) -> Self {
        use Kind::*;
        const ALL: [Kind; 32] = [
            Bool, Char, Int8, UInt8, Int16, UInt16, Int32, UInt32, Int64, UInt64, Float, Vector2,
            Vector3, Vector4, Quaternion, Matrix3x3, Matrix4x4, ObjectId, Handle, Object, ObjectPtr,
            BaseObjectPtr, BaseObject, StaticArray, BigArray, Enum, String, LString, Reference,
            SmallArray, SloppyHandle, DataDrivenEnum,
        ];
        ALL[(code & 31) as usize]
    }

    /// Encoded size of the fixed-size primitive and math kinds.
    pub fn primitive_size(self) -> Option<usize> {
        use Kind::*;
        Some(match self {
            Bool | Char | Int8 | UInt8 => 1,
            Int16 | UInt16 => 2,
            Int32 | UInt32 | Float => 4,
            Int64 | UInt64 => 8,
            Vector2 => 8,
            Vector3 => 12,
            Vector4 | Quaternion => 16,
            Matrix3x3 => 36,
            Matrix4x4 => 64,
            _ => return None,
        })
    }

    pub fn is_array(self) -> bool {
        matches!(self, Kind::StaticArray | Kind::BigArray | Kind::SmallArray)
    }
}

#[derive(Debug, Clone)]
pub struct Property {
    pub flags: u32,
    pub name_hash: u32,
    pub packed_type: u64,
}

impl Property {
    pub fn kind(&self) -> Kind {
        Kind::from_code(((self.packed_type >> 48) & 0x1f) as u8)
    }
    /// Element kind of an array. On a scalar `Bool`, 1 means the field is
    /// absent from the byte stream.
    pub fn elem_kind(&self) -> Kind {
        Kind::from_code(((self.packed_type >> 55) & 0x1f) as u8)
    }
    /// Element count of a `StaticArray` (not stored in the stream).
    pub fn static_count(&self) -> usize {
        ((self.packed_type >> 32) & 0xffff) as usize
    }
    /// Hash of the referenced type or enum.
    pub fn object_hash(&self) -> u32 {
        self.packed_type as u32
    }
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name_hash: u32,
    pub values: Vec<(u32, i32)>,
}

#[derive(Debug, Clone)]
pub struct TypeDef {
    pub type_hash: u32,
    pub base_type_hash: u32,
    pub flags: u32,
    pub properties: Vec<Property>,
    pub enums: Vec<EnumDef>,
}

#[derive(Debug, Clone)]
pub struct Schema {
    pub version: i32,
    pub types: HashMap<u32, TypeDef>,
    pub global_enums: HashMap<u32, EnumDef>,
    pub names: HashMap<u32, String>,
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let s = self
            .d
            .get(self.p..self.p + N)
            .ok_or_else(|| Error::Format("truncated".into()))?;
        self.p += N;
        Ok(s.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn count(&mut self) -> Result<usize, Error> {
        usize::try_from(self.i32()?).map_err(|_| Error::Format("negative count".into()))
    }
    fn cstring(&mut self) -> Result<String, Error> {
        let rest = &self.d[self.p..];
        let n = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        self.p += (n + 1).min(rest.len());
        Ok(String::from_utf8_lossy(&rest[..n]).into_owned())
    }
    fn enum_def(&mut self) -> Result<EnumDef, Error> {
        let name_hash = self.u32()?;
        let n = self.count()?;
        let values = (0..n).map(|_| Ok((self.u32()?, self.i32()?))).collect::<Result<_, Error>>()?;
        Ok(EnumDef { name_hash, values })
    }
}

impl Schema {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::parse(&std::fs::read(path)?)
    }

    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let mut r = Reader { d: data, p: 0 };
        if r.u32()? != SCHM_MAGIC {
            return Err(Error::Format("bad SCHM magic".into()));
        }
        let version = r.i32()?;
        r.u32()?; // game name hash
        r.u64()?; // code changelist
        r.u64()?; // parent changelist
        let compression = r.u8()?;
        let data_length = r.count()?;
        if compression != 0 {
            return Err(Error::Format(format!("compressed schema body ({compression})")));
        }
        let end = (HEADER_LEN + data_length.saturating_sub(HEADER_LEN)).min(data.len());
        let mut r = Reader { d: &data[..end], p: r.p };

        let type_count = r.count()?;
        let mut types = HashMap::with_capacity(type_count);
        for _ in 0..type_count {
            let type_hash = r.u32()?;
            let base_type_hash = r.u32()?;
            let flags = r.u32()?;
            let np = r.count()?;
            let ne = r.count()?;
            let properties = (0..np)
                .map(|_| Ok(Property { flags: r.u32()?, name_hash: r.u32()?, packed_type: r.u64()? }))
                .collect::<Result<_, Error>>()?;
            let enums = (0..ne).map(|_| r.enum_def()).collect::<Result<_, _>>()?;
            types.insert(type_hash, TypeDef { type_hash, base_type_hash, flags, properties, enums });
        }

        let more = |r: &Reader| r.p + 4 <= r.d.len();
        let n = if more(&r) { r.count()? } else { 0 };
        let mut global_enums = HashMap::with_capacity(n);
        for _ in 0..n {
            let e = r.enum_def()?;
            global_enums.insert(e.name_hash, e);
        }
        let n = if more(&r) { r.count()? } else { 0 };
        let mut names = HashMap::with_capacity(n);
        for _ in 0..n {
            let h = r.u32()?;
            names.insert(h, r.cstring()?);
        }
        Ok(Self { version, types, global_enums, names })
    }

    /// ACB's schema has nameless placeholder properties where the dump lost a
    /// field; Revelations' schema names the same layout. Types with a
    /// placeholder take the donor's definition, and donor-only types are
    /// added, which is the layout retail ACB data actually uses.
    pub fn with_placeholders_filled(&self, donor: &Schema) -> Schema {
        let mut s = self.clone();
        let mut names = donor.names.clone();
        names.extend(self.names.iter().map(|(k, v)| (*k, v.clone())));
        s.names = names;
        for (h, t) in &self.types {
            if t.properties.iter().any(|p| p.name_hash == 0) {
                if let Some(d) = donor.types.get(h) {
                    s.types.insert(*h, d.clone());
                }
            }
        }
        for (h, t) in &donor.types {
            s.types.entry(*h).or_insert_with(|| t.clone());
        }
        s
    }

    pub fn name(&self, hash: u32) -> Option<&str> {
        self.names.get(&hash).map(String::as_str)
    }

    /// Name, or the hash as `0xXXXXXXXX` when the dictionary lacks it.
    pub fn name_or_hash(&self, hash: u32) -> String {
        self.name(hash).map_or_else(|| format!("0x{hash:08X}"), str::to_owned)
    }

    pub fn type_def(&self, hash: u32) -> Option<&TypeDef> {
        self.types.get(&hash)
    }

    pub fn hash_of(&self, name: &str) -> Option<u32> {
        self.names.iter().find(|(h, n)| *n == name && self.types.contains_key(h)).map(|(h, _)| *h)
    }

    /// All properties of a type, base class first: the stream order.
    pub fn property_chain(&self, hash: u32) -> Vec<&Property> {
        let mut chain = Vec::new();
        let mut cur = self.types.get(&hash);
        while let Some(t) = cur {
            chain.push(t);
            cur = self.types.get(&t.base_type_hash);
        }
        chain.iter().rev().flat_map(|t| t.properties.iter()).collect()
    }

    /// True when `hash` is `base` or derives from it.
    pub fn is_a(&self, hash: u32, base: u32) -> bool {
        let mut cur = self.types.get(&hash);
        while let Some(t) = cur {
            if t.type_hash == base {
                return true;
            }
            cur = self.types.get(&t.base_type_hash);
        }
        false
    }

    pub fn enum_def(&self, hash: u32) -> Option<&EnumDef> {
        self.global_enums
            .get(&hash)
            .or_else(|| self.types.values().flat_map(|t| &t.enums).find(|e| e.name_hash == hash))
    }
}
