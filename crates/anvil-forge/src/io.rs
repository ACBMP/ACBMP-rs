//! Little-endian read helpers shared by the container parsers.

use std::io::{self, Read, Seek, SeekFrom};

pub(crate) trait ReadExt: Read {
    fn bytes_n<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let mut b = [0; N];
        self.read_exact(&mut b)?;
        Ok(b)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.bytes_n::<1>()?[0])
    }
    #[allow(dead_code)]
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.bytes_n()?))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.bytes_n()?))
    }
    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.bytes_n()?))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.bytes_n()?))
    }
    fn i64(&mut self) -> io::Result<i64> {
        Ok(i64::from_le_bytes(self.bytes_n()?))
    }
    /// NUL-terminated 8-bit string; engine strings are Latin-1.
    fn read_cstring(&mut self) -> io::Result<String> {
        let mut s = String::new();
        loop {
            match self.u8()? {
                0 => return Ok(s),
                c => s.push(c as char),
            }
        }
    }
}

impl<R: Read + ?Sized> ReadExt for R {}

pub(crate) trait SeekExt: Seek {
    fn skip(&mut self, n: u64) -> io::Result<()> {
        self.seek(SeekFrom::Current(n as i64)).map(|_| ())
    }
}

impl<S: Seek + ?Sized> SeekExt for S {}

/// Bounds-checked little-endian cursor over a byte slice.
pub struct Cursor<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes(b.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    pub fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
}
