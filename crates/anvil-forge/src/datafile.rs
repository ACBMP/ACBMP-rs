//! `.data` resource container: a dependency table, then magic-delimited,
//! block-compressed sections. The first section is a table of contents that
//! nothing reads; every later section is a run of named sub-objects.
//!
//! Mirrors anvilforge-py's `datafile.py` (`iter_datafile_subparts`).

use crate::io::Cursor;
use crate::{lzo2a, Error};

/// Marks the start of every block set.
pub const DATA_MAGIC: u64 = 1_154_322_941_026_740_787;

/// One serialized object inside a `.data` container.
#[derive(Debug, Clone)]
pub struct SubObject {
    /// Running index across all content sections.
    pub index: usize,
    /// Type-name hash; the same hash the schema keys types by.
    pub type_hash: u32,
    pub name: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct DataFile {
    /// Ids of other entries this one references.
    pub dependencies: Vec<u64>,
    pub objects: Vec<SubObject>,
}

impl DataFile {
    pub fn parse(raw: &[u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(raw);
        let bad = |what: &str| Error::Format(format!("truncated .data ({what})"));

        let dep_count = c.u32().ok_or_else(|| bad("dependency count"))?;
        let dependencies = (0..dep_count)
            .map(|_| c.u64().ok_or_else(|| bad("dependency")))
            .collect::<Result<Vec<_>, _>>()?;

        let mut objects = Vec::new();
        let mut first = true;
        while c.remaining() >= 8 {
            if c.u64() != Some(DATA_MAGIC) {
                // trailing tool signature, not data
                break;
            }
            let blob = read_block_set(&mut c)?;
            if first {
                first = false;
                continue;
            }
            parse_records(&blob, &mut objects)?;
        }
        Ok(Self { dependencies, objects })
    }
}

fn read_block_set(c: &mut Cursor) -> Result<Vec<u8>, Error> {
    let bad = || Error::Format("truncated block set".into());
    c.u16().ok_or_else(bad)?; // data version
    let algorithm = c.u8().ok_or_else(bad)?;
    c.u16().ok_or_else(bad)?; // uncompressed block align
    c.u16().ok_or_else(bad)?; // compressed block align
    let count = c.u16().ok_or_else(bad)?;
    if count == 0 {
        return Err(Error::Format("block set with zero blocks".into()));
    }
    let sizes = (0..count)
        .map(|_| Some((c.u16()? as usize, c.u16()? as usize)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(bad)?;

    let mut out = Vec::with_capacity(sizes.iter().map(|s| s.0).sum());
    for (unpacked, packed) in sizes {
        c.u32().ok_or_else(bad)?; // block CRC
        let data = c.take(packed).ok_or_else(bad)?;
        if unpacked == packed {
            out.extend_from_slice(data);
        } else {
            match algorithm {
                // LZO1X: retail ACB doesn't use it, but forges rewritten by
                // the map tools do
                0 | 1 => out.extend(
                    lzokay_native::decompress_all(data, Some(unpacked))
                        .map_err(|e| Error::Format(format!("lzo1x: {e:?}")))?,
                ),
                2 => out.extend(lzo2a::decompress(data, unpacked)?),
                a => return Err(Error::Format(format!("compression algorithm {a} not supported"))),
            }
        }
    }
    Ok(out)
}

/// Extra bytes a payload carries ahead of its declared size: an import table
/// of `n` 12-byte entries plus 7 bytes of header, flagged by a leading 1.
fn import_table_len(peek: &[u8]) -> usize {
    if peek.len() >= 8 && peek[0] == 1 {
        let n = i32::from_le_bytes(peek[4..8].try_into().unwrap());
        (12 * n as i64 + 7).max(0) as usize
    } else {
        0
    }
}

fn parse_records(blob: &[u8], out: &mut Vec<SubObject>) -> Result<(), Error> {
    let mut c = Cursor::new(blob);
    let bad = || Error::Format("truncated sub-object record".into());
    while c.remaining() > 0 {
        let type_hash = c.u32().ok_or_else(bad)?;
        let mut size = (c.i32().ok_or_else(bad)? as i64 + 1) as usize;
        let name_len = c.u32().ok_or_else(bad)? as usize;
        let name = c.take(name_len).ok_or_else(bad)?.iter().map(|&b| b as char).collect();
        let peek_end = (c.pos + 8).min(blob.len());
        size += import_table_len(&blob[c.pos..peek_end]);
        let payload = c.take(size).ok_or_else(bad)?.to_vec();
        out.push(SubObject { index: out.len(), type_hash, name, payload });
    }
    Ok(())
}
