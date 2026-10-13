//! `.forge` archive index (container format 25, Brotherhood/Revelations).
//!
//! Layout, as read by anvilforge-py's `forge.py` / `fileset.py`:
//! - `"scimitar\0"`, u32 version (25), u64 header end, opaque bytes up to it
//! - 4 + 4 + 8 + 8 + 4 unused bytes, u32 fileset count, i64 first fileset offset
//! - each fileset: u32 entry count, u32, u64, i64 next fileset (-1 = last),
//!   u32 first index, u32 last index, i64 info table, u64 data base,
//!   then `count` triplets of (i64 offset, u32 id, i32 length)
//! - info table: one 188-byte record per entry (the name lives here)
//! - each payload sits after a 440-byte `FILEDATA` header at its offset

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::io::{ReadExt, SeekExt};
use crate::Error;

const VERSION: u32 = 25;
const INFO_RECORD_SIZE: u64 = 188;
const FILEDATA_HEADER_SIZE: u64 = 440;
/// Entry id the toolchain gives the archive's global metafile.
pub const METAFILE_ID: u32 = 16;

#[derive(Debug, Clone)]
pub struct Entry {
    /// Position in the archive, the `N` of anvilforge's `N_-_name.data`.
    pub index: usize,
    pub id: u32,
    /// Absolute offset of the payload (after the FILEDATA header).
    pub offset: u64,
    pub length: u32,
    pub name: String,
    pub extension: u32,
    pub umac_hash: u64,
}

impl Entry {
    pub fn is_metafile(&self) -> bool {
        self.id == METAFILE_ID
    }
}

pub struct Forge {
    file: BufReader<File>,
    entries: Vec<Entry>,
}

impl Forge {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let mut f = BufReader::new(File::open(path)?);

        let magic = f.read_cstring()?;
        if !magic.eq_ignore_ascii_case("scimitar") {
            return Err(Error::Format(format!("bad forge magic {magic:?}")));
        }
        let version = f.u32()?;
        if version != VERSION {
            return Err(Error::Format(format!("forge version {version}, expected {VERSION}")));
        }
        let header_end = f.u64()?;
        // magic (9) + version (4) + header_end (8) = 21 bytes already read
        f.skip(header_end - 21)?;
        f.skip(4 + 4 + 8 + 8 + 4)?;
        let fileset_count = f.u32()?;
        let mut next = f.i64()?;

        let mut entries = Vec::new();
        for _ in 0..fileset_count {
            if next < 0 {
                break;
            }
            f.seek(SeekFrom::Start(next as u64))?;
            let count = f.u32()?;
            f.skip(4 + 8)?;
            next = f.i64()?;
            f.skip(4 + 4)?;
            let info_table = f.i64()? as u64;
            f.skip(8)?;

            let triplets = (0..count)
                .map(|_| Ok((f.i64()?, f.u32()?, f.i32()?)))
                .collect::<Result<Vec<_>, Error>>()?;

            for (i, (offset, id, length)) in triplets.into_iter().enumerate() {
                f.seek(SeekFrom::Start(info_table + i as u64 * INFO_RECORD_SIZE))?;
                f.skip(4)?; // info's own length copy; the triplet's wins
                let umac_hash = f.u64()?;
                f.skip(4)?; // engine version
                let extension = f.u32()?;
                f.skip(4 * 6)?; // revisions, next/prev, parent, timestamp
                let name = f.read_cstring()?;
                entries.push(Entry {
                    index: entries.len(),
                    id,
                    offset: offset as u64 + FILEDATA_HEADER_SIZE,
                    length: length as u32,
                    name,
                    extension,
                    umac_hash,
                });
            }
        }
        Ok(Self { file: f, entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Raw payload bytes of one entry (a `.data` container or the metafile).
    pub fn read(&mut self, entry: &Entry) -> Result<Vec<u8>, Error> {
        self.file.seek(SeekFrom::Start(entry.offset))?;
        let mut buf = vec![0; entry.length as usize];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }
}
