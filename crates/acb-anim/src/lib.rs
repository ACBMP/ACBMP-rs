//! Brotherhood animation tracks.
//!
//! An `Animation` object's FastLoad tail (`Animation::CustomSerializeFastLoad`)
//! is a u32 track count, then per track one descriptor byte: an index into
//! `AnimTrackTable::ms_AnimTrackDescriptorTable`, or 0xFE followed by a u32
//! built-in track id. Descriptor `d` is compression `d / 3` (see
//! [`Compression`]) with key-time format `d % 3` (8and8, Var16, Bitmap); the
//! table and every layout below come from the Mac build's
//! `AnimTrackTable::Init` and `AnimTrackDescriptorTyped<...>` methods.

use glam::{Quat, Vec3};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("truncated track data")]
    Truncated,
    #[error("unknown track descriptor {0}")]
    Descriptor(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compression {
    QuatNone,
    Quat16,
    Quat24,
    Quat32,
    Quat48,
    Quat64,
    QuatZFloat8,
    QuatZFloat16,
    Quat96,
    Vector3None,
    Vector332,
    Vector348,
    FloatNone,
    Float8,
    Float16,
    ByteLinear,
    ByteLowest,
    ByteNearest,
}

impl Compression {
    pub const ALL: [Compression; 18] = [
        Self::QuatNone,
        Self::Quat16,
        Self::Quat24,
        Self::Quat32,
        Self::Quat48,
        Self::Quat64,
        Self::QuatZFloat8,
        Self::QuatZFloat16,
        Self::Quat96,
        Self::Vector3None,
        Self::Vector332,
        Self::Vector348,
        Self::FloatNone,
        Self::Float8,
        Self::Float16,
        Self::ByteLinear,
        Self::ByteLowest,
        Self::ByteNearest,
    ];

    /// Bytes per key (`GetCompressedTypeSize`).
    pub fn value_size(self) -> usize {
        anvil_fastload::ANIM_VALUE_SIZES[self as usize]
    }

    pub fn kind(self) -> ValueKind {
        use Compression::*;
        match self {
            QuatNone | Quat16 | Quat24 | Quat32 | Quat48 | Quat64 | QuatZFloat8 | QuatZFloat16 | Quat96 => {
                ValueKind::Quat
            }
            Vector3None | Vector332 | Vector348 => ValueKind::Vec3,
            FloatNone | Float8 | Float16 => ValueKind::Float,
            ByteLinear | ByteLowest | ByteNearest => ValueKind::Byte,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueKind {
    Quat,
    Vec3,
    Float,
    Byte,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeFormat {
    /// (keys - 1) x u8 absolute frame numbers; key 0 is frame 0.
    EightAndEight,
    /// (keys - 1) x u16 absolute frame numbers.
    Var16,
    /// Bitmap of frames that have a key, scaled by `1 << shift`.
    Bitmap,
}

#[derive(Debug, Clone)]
pub struct RawTrack {
    pub descriptor: u8,
    pub compression: Compression,
    pub format: TimeFormat,
    /// Frame of each key.
    pub times: Vec<u32>,
    /// `times.len() * compression.value_size()` bytes.
    pub values: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum Track {
    /// Shared engine track (`AnimTrackBuiltinTracks::GetPermanentTrack`).
    Builtin(u32),
    Data(RawTrack),
}

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Error> {
        let s = self.d.get(self.p..self.p + n).ok_or(Error::Truncated)?;
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

/// Parses the track stream (an Animation's custom FastLoad tail).
pub fn parse_tracks(tail: &[u8]) -> Result<Vec<Track>, Error> {
    let mut r = Rd { d: tail, p: 0 };
    let n = r.u32()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let d = r.u8()?;
        if d == anvil_fastload::ANIM_BUILTIN_TRACK {
            out.push(Track::Builtin(r.u32()?));
            continue;
        }
        let compression = *Compression::ALL.get(d as usize / 3).ok_or(Error::Descriptor(d))?;
        r.u32()?; // runtime allocation size
        let keys = r.u32()? as usize;
        let (format, times) = match d % 3 {
            0 => {
                let t = r.take(keys.saturating_sub(1))?;
                (TimeFormat::EightAndEight, std::iter::once(0).chain(t.iter().map(|&b| b as u32)).take(keys).collect())
            }
            1 => {
                let t = r.take(2 * keys.saturating_sub(1))?;
                let rest = t.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32);
                (TimeFormat::Var16, std::iter::once(0).chain(rest).take(keys).collect())
            }
            _ => {
                let head = r.u8()?;
                let shift = head >> 6;
                let bitmap = r.take((head & 0x3f) as usize)?;
                let mut t = Vec::with_capacity(keys);
                for bit in 0..bitmap.len() * 8 {
                    if t.len() == keys {
                        break;
                    }
                    if bitmap[bit / 8] & (1 << (bit % 8)) != 0 {
                        t.push((bit as u32) << shift);
                    }
                }
                (TimeFormat::Bitmap, t)
            }
        };
        let values = r.take(keys * compression.value_size())?.to_vec();
        out.push(Track::Data(RawTrack { descriptor: d, compression, format, times, values }));
    }
    Ok(out)
}

const FRAC_1_SQRT_2: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Rebuilds a "smallest three" quaternion: `v` are the three stored
/// components in order, `missing` the slot of the dropped largest one, which
/// is reconstructed positive (negated when `negate`).
fn smallest_three(v: [f32; 3], missing: usize, negate: bool) -> Quat {
    let mut d = (1.0 - v[0] * v[0] - v[1] * v[1] - v[2] * v[2]).max(0.0).sqrt();
    if negate {
        d = -d;
    }
    let mut q = [0f32; 4];
    let mut k = 0;
    for (i, slot) in q.iter_mut().enumerate() {
        if i == missing {
            *slot = d;
        } else {
            *slot = v[k];
            k += 1;
        }
    }
    Quat::from_array(q)
}

fn dequant(x: u64, scale: f32) -> f32 {
    x as f32 * scale - FRAC_1_SQRT_2
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn f32_at(b: &[u8], i: usize) -> f32 {
    f32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}

/// `Quaternion16`: 4-bit components at bits 8, 4, 0; missing slot in bits
/// 14-15; bit 13 negates the rebuilt component.
pub fn quat16(b: &[u8]) -> Quat {
    let u = u16_at(b, 0) as u64;
    let s = 0.101_015_255;
    let v = [dequant((u >> 8) & 0xf, s), dequant((u >> 4) & 0xf, s), dequant(u & 0xf, s)];
    smallest_three(v, ((u >> 14) & 3) as usize, u & 0x2000 != 0)
}

/// `Quaternion24`: three bytes, low 7 bits each; missing slot from the top
/// bits of bytes 0 and 1, `(b1 << 1) | b0`.
pub fn quat24(b: &[u8]) -> Quat {
    let s = 0.011_223_917;
    let v = [dequant((b[0] & 0x7f) as u64, s), dequant((b[1] & 0x7f) as u64, s), dequant((b[2] & 0x7f) as u64, s)];
    smallest_three(v, (((b[1] >> 7) << 1) | (b[0] >> 7)) as usize, false)
}

/// `Quaternion32`: 10-bit components at bits 20, 10, 0; missing slot in bits
/// 30-31.
pub fn quat32(b: &[u8]) -> Quat {
    let u = u32::from_le_bytes(b[..4].try_into().unwrap()) as u64;
    let s = 0.001_383_770_6;
    let v = [dequant((u >> 20) & 0x3ff, s), dequant((u >> 10) & 0x3ff, s), dequant(u & 0x3ff, s)];
    smallest_three(v, (u >> 30) as usize, false)
}

/// `AnimCompressionTraitQuat48`: three u16, low 15 bits each; missing slot
/// from their sign bits, `(s1 << 1) | s0`.
pub fn quat48(b: &[u8]) -> Quat {
    let s = [u16_at(b, 0), u16_at(b, 2), u16_at(b, 4)];
    let k = 4.316_100_6e-5;
    let v = s.map(|x| dequant((x & 0x7fff) as u64, k));
    smallest_three(v, (((s[1] >> 15) << 1) | (s[0] >> 15)) as usize, false)
}

/// `Quaternion64` (from the SSE in `GetQuaternionInternal`): 20-bit
/// components at bits 8..28, {52..64 low, 0..8 high}, 32..52; missing slot
/// in bits 30-31.
pub fn quat64(b: &[u8]) -> Quat {
    let q = u64::from_le_bytes(b[..8].try_into().unwrap());
    let s = 1.348_701_7e-6;
    let a = (q >> 8) & 0xfffff;
    let bb = ((q >> 52) & 0xfff) | ((q & 0xff) << 12);
    let c = (q >> 32) & 0xfffff;
    smallest_three([dequant(a, s), dequant(bb, s), dequant(c, s)], ((q >> 30) & 3) as usize, false)
}

/// `Quaternion96`: three f32; missing slot in their lowest mantissa bits,
/// `(f1 & 1) << 1 | (f0 & 1)`.
pub fn quat96(b: &[u8]) -> Quat {
    let v = [f32_at(b, 0), f32_at(b, 4), f32_at(b, 8)];
    let bit = |x: f32| (x.to_bits() & 1) as usize;
    smallest_three(v, (bit(v[1]) << 1) | bit(v[0]), false)
}

/// Uncompressed quaternion (`QuatNone`): four f32, x y z w.
pub fn quat_none(b: &[u8]) -> Quat {
    Quat::from_xyzw(f32_at(b, 0), f32_at(b, 4), f32_at(b, 8), f32_at(b, 12))
}

/// Single-axis (Z) rotation: one component `v`, the other sqrt(1 - v^2);
/// the flag bit puts the rebuilt one in z instead of w.
fn z_rotation(v: f32, flag: bool) -> Quat {
    let c = (1.0 - v * v).max(0.0).sqrt();
    if flag { Quat::from_xyzw(0.0, 0.0, c, v) } else { Quat::from_xyzw(0.0, 0.0, v, c) }
}

/// `QuatZFloat16`: low 15 bits, scale sqrt2/32767; sign bit selects order.
pub fn quat_z16(b: &[u8]) -> Quat {
    let u = u16_at(b, 0);
    z_rotation(dequant((u & 0x7fff) as u64, 4.316_100_6e-5), u & 0x8000 != 0)
}

/// `QuatZFloat8`: low 7 bits, scale sqrt2/126; sign bit selects order.
pub fn quat_z8(b: &[u8]) -> Quat {
    z_rotation(dequant((b[0] & 0x7f) as u64, 0.011_223_917), b[0] & 0x80 != 0)
}

/// Uncompressed vector (`Vector3None`): three f32.
pub fn vec3_none(b: &[u8]) -> Vec3 {
    Vec3::new(f32_at(b, 0), f32_at(b, 4), f32_at(b, 8))
}

/// `Vector332`: one i32 holding signed 9 (bits 23..32), 12 (11..23) and 11
/// (0..11) bit millimetre values.
pub fn vec3_332(b: &[u8]) -> Vec3 {
    let i = i32::from_le_bytes(b[..4].try_into().unwrap());
    let x = i >> 23;
    let y = (i << 9) >> 20;
    let z = ((i << 21) >> 21) as f32;
    Vec3::new(x as f32, y as f32, z) * 0.001
}

/// `Vector348`: three i16 millimetre values.
pub fn vec3_348(b: &[u8]) -> Vec3 {
    let s = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]) as f32 * 0.001;
    Vec3::new(s(0), s(2), s(4))
}

impl RawTrack {
    /// Decoded quaternion keys (None for non-rotation tracks).
    pub fn quats(&self) -> Option<Vec<Quat>> {
        use Compression::*;
        let f: fn(&[u8]) -> Quat = match self.compression {
            QuatNone => quat_none,
            Quat16 => quat16,
            Quat24 => quat24,
            Quat32 => quat32,
            Quat48 => quat48,
            Quat64 => quat64,
            Quat96 => quat96,
            QuatZFloat8 => quat_z8,
            QuatZFloat16 => quat_z16,
            _ => return None,
        };
        Some(self.values.chunks_exact(self.compression.value_size()).map(f).collect())
    }

    /// Decoded translation keys (None for non-vector tracks).
    pub fn vec3s(&self) -> Option<Vec<Vec3>> {
        let f: fn(&[u8]) -> Vec3 = match self.compression {
            Compression::Vector3None => vec3_none,
            Compression::Vector332 => vec3_332,
            Compression::Vector348 => vec3_348,
            _ => return None,
        };
        Some(self.values.chunks_exact(self.compression.value_size()).map(f).collect())
    }

    /// Decoded scalar keys (None for non-float tracks).
    pub fn floats(&self) -> Option<Vec<f32>> {
        let v = &self.values;
        Some(match self.compression {
            Compression::FloatNone => v.chunks_exact(4).map(|c| f32_at(c, 0)).collect(),
            Compression::Float8 => v.iter().map(|&b| b as i8 as f32 * 0.008).collect(),
            Compression::Float16 => v.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 * 0.008).collect(),
            Compression::ByteLinear | Compression::ByteLowest | Compression::ByteNearest => {
                v.iter().map(|&b| b as f32).collect()
            }
            _ => return None,
        })
    }
}

pub mod character;
pub mod pose;
