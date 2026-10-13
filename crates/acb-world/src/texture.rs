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
