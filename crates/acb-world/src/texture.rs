//! Textures: `TextureMap.CompiledTextureMap` holds a plain mip chain,
//! largest level first.

use anvil_fastload::Value;

use crate::{Archive, Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 32-bit BGRA (D3DFMT_A8R8G8B8 byte order).
    Bgra8,
    Bc1,
    Bc2,
    Bc3,
}

impl PixelFormat {
    fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::Bgra8,
            2 | 3 => Self::Bc1,
            4 => Self::Bc2,
            5 => Self::Bc3,
            _ => return None,
        })
    }

    /// Bytes of one mip level.
    pub fn level_size(self, w: u32, h: u32) -> usize {
        let blocks = |x: u32| x.div_ceil(4).max(1) as usize;
        match self {
            Self::Bgra8 => w.max(1) as usize * h.max(1) as usize * 4,
            Self::Bc1 => blocks(w) * blocks(h) * 8,
            Self::Bc2 | Self::Bc3 => blocks(w) * blocks(h) * 16,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Mip levels present in `data`, largest first.
    pub mips: u32,
    pub data: Vec<u8>,
}

impl Archive<'_> {
    /// Decoded 2D texture; `Ok(None)` for volume textures or unknown formats.
    pub fn texture(&self, uid: u32) -> Result<Option<Texture>, Error> {
        let root = self.root(uid)?;
        let Some(c) = self.field(&root.obj, "CompiledTextureMap").and_then(Value::as_obj) else {
            return Ok(None);
        };
        let u = |n: &str| self.field(c, n).and_then(Value::as_u32).unwrap_or(0);
        let (width, height, depth) = (u("Width"), u("Height"), u("Depth"));
        let Some(format) = PixelFormat::from_code(u("PixelFormat")) else { return Ok(None) };
        if depth != 1 || width == 0 || height == 0 {
            return Ok(None);
        }
        let data = self.field(c, "Data").and_then(Value::as_bytes).unwrap_or_default();
        // keep only whole levels: some chains are shorter than NbMipMaps says
        let (mut mips, mut need, mut w, mut h) = (0, 0, width, height);
        while mips < u("NbMipMaps").max(1) && need + format.level_size(w, h) <= data.len() {
            need += format.level_size(w, h);
            mips += 1;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        if mips == 0 {
            return Ok(None);
        }
        Ok(Some(Texture { width, height, format, mips, data: data[..need].to_vec() }))
    }
}

/// RGB of a BC1 colour endpoint pair's palette (`four`: opaque 4-colour
/// mode, as BC2/BC3 always are).
fn bc1_palette(c0: u16, c1: u16, four: bool) -> [[u8; 4]; 4] {
    let rgb = |c: u16| {
        let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
        [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8, 255]
    };
    let (a, b) = (rgb(c0), rgb(c1));
    let mix = |x: u8, y: u8, wx: u32, wy: u32| ((x as u32 * wx + y as u32 * wy) / (wx + wy)) as u8;
    if four {
        [a, b, std::array::from_fn(|i| if i == 3 { 255 } else { mix(a[i], b[i], 2, 1) }), std::array::from_fn(|i| {
            if i == 3 {
                255
            } else {
                mix(a[i], b[i], 1, 2)
            }
        })]
    } else {
        [a, b, std::array::from_fn(|i| if i == 3 { 255 } else { mix(a[i], b[i], 1, 1) }), [0, 0, 0, 0]]
    }
}

impl Texture {
    /// The largest mip level as RGBA8 (BC blocks decoded on the CPU).
    pub fn rgba(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; w * h * 4];
        let d = &self.data;
        if self.format == PixelFormat::Bgra8 {
            for (o, i) in out.chunks_exact_mut(4).zip(d.chunks_exact(4)) {
                o.copy_from_slice(&[i[2], i[1], i[0], i[3]]);
            }
            return out;
        }
        let block = if self.format == PixelFormat::Bc1 { 8 } else { 16 };
        let bw = w.div_ceil(4);
        for (bi, b) in d.chunks_exact(block).take(bw * h.div_ceil(4)).enumerate() {
            let (bx, by) = (bi % bw * 4, bi / bw * 4);
            let col = &b[block - 8..];
            let (c0, c1) = (u16::from_le_bytes([col[0], col[1]]), u16::from_le_bytes([col[2], col[3]]));
            let pal = bc1_palette(c0, c1, self.format != PixelFormat::Bc1 || c0 > c1);
            let bits = u32::from_le_bytes([col[4], col[5], col[6], col[7]]);
            // BC3 alpha: two endpoints and 3-bit indices
            let a3 = (self.format == PixelFormat::Bc3).then(|| {
                let (a0, a1) = (b[0] as u32, b[1] as u32);
                let mut pal = [a0, a1, 0, 0, 0, 0, 0, 0];
                for k in 2..8u32 {
                    pal[k as usize] = if a0 > a1 {
                        ((8 - k) * a0 + (k - 1) * a1) / 7
                    } else if k < 6 {
                        ((6 - k) * a0 + (k - 1) * a1) / 5
                    } else if k == 6 {
                        0
                    } else {
                        255
                    };
                }
                let idx = u64::from_le_bytes([b[2], b[3], b[4], b[5], b[6], b[7], 0, 0]);
                (pal, idx)
            });
            for py in 0..4 {
                for px in 0..4 {
                    let (x, y) = (bx + px, by + py);
                    if x >= w || y >= h {
                        continue;
                    }
                    let k = py * 4 + px;
                    let mut c = pal[(bits >> (2 * k) & 3) as usize];
                    match self.format {
                        PixelFormat::Bc2 => {
                            let nib = (u64::from_le_bytes(b[..8].try_into().unwrap()) >> (4 * k)) & 15;
                            c[3] = (nib * 17) as u8;
                        }
                        PixelFormat::Bc3 => {
                            let (pal, idx) = a3.unwrap();
                            c[3] = pal[((idx >> (3 * k)) & 7) as usize] as u8;
                        }
                        _ => {}
                    }
                    out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&c);
                }
            }
        }
        out
    }
}
