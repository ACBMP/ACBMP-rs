//! `CompressedLocalizationData.Data`: the game text of one language.
//!
//! Layout (all integers big-endian):
//! - `u16 escape`, `u16 node_count`, then `node_count` nodes of two `u16`.
//!   A node `(a, 0)` is the UTF-16 code unit `a`; a node `(a, b)` with
//!   `b != 0` expands to node `b` followed by node `a` (a pair grammar).
//! - `u16 section_count`, then per section three `u32`: the first string id,
//!   the offset of its encoded bytes and the offset of its string table
//!   (offsets from the start of the blob).
//! - A string table: `u16 n`, `u16 end0` (the length of the section's first
//!   string, id = first id), then `n` pairs `(u16 id delta, u16 end)`; each
//!   later string runs from the previous end to its own end.
//! - Encoded bytes are node codes: a byte below `escape` is the code itself,
//!   `0xff` is followed by a 16-bit code, any other byte `b` with the next
//!   byte `c` gives `(b << 8 | c) - escape * 255`. Code `k` names node `k + 1`.

use crate::Error;

/// One decoded string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocString {
    pub id: u32,
    pub text: String,
}

fn be16(d: &[u8], at: usize) -> Result<u16, Error> {
    d.get(at..at + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or_else(|| Error::Format(format!("localization data truncated at {at:#x}")))
}

fn be32(d: &[u8], at: usize) -> Result<u32, Error> {
    d.get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| Error::Format(format!("localization data truncated at {at:#x}")))
}

/// Decoded view over one blob.
pub struct Localization<'a> {
    data: &'a [u8],
    escape: u16,
    nodes: Vec<(u16, u16)>,
    /// (first id, bytes offset, table offset)
    sections: Vec<(u32, usize, usize)>,
}

impl<'a> Localization<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        let escape = be16(data, 0)?;
        let count = be16(data, 2)? as usize;
        let nodes = (0..count)
            .map(|i| Ok((be16(data, 4 + i * 4)?, be16(data, 6 + i * 4)?)))
            .collect::<Result<Vec<_>, Error>>()?;
        let at = 4 + count * 4;
        let sections = (0..be16(data, at)? as usize)
            .map(|i| {
                let s = at + 2 + i * 12;
                Ok((be32(data, s)?, be32(data, s + 4)? as usize, be32(data, s + 8)? as usize))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Self { data, escape, nodes, sections })
    }

    /// Every string, in id order of the sections.
    pub fn strings(&self) -> Result<Vec<LocString>, Error> {
        let mut out = Vec::new();
        for &(first, bytes, table) in &self.sections {
            let n = be16(self.data, table)? as usize;
            let mut start = 0usize;
            let mut end = be16(self.data, table + 2)? as usize;
            let mut id = first;
            for k in 0..=n {
                if k > 0 {
                    id = first.wrapping_add(be16(self.data, table + 4 * k)? as u32);
                    start = end;
                    end = be16(self.data, table + 2 + 4 * k)? as usize;
                }
                let raw = self
                    .data
                    .get(bytes + start..bytes + end)
                    .ok_or_else(|| Error::Format(format!("string {id:#x} out of range")))?;
                out.push(LocString { id, text: self.decode(raw)? });
            }
        }
        Ok(out)
    }

    /// Decode one string's code bytes.
    pub fn decode(&self, raw: &[u8]) -> Result<String, Error> {
        let bad = |what: &str| Error::Format(format!("localization: {what}"));
        let mut units: Vec<u16> = Vec::new();
        let mut stack: Vec<u16> = Vec::new();
        let mut i = 0;
        while i < raw.len() {
            let b = raw[i];
            i += 1;
            let code = if (b as u16) < self.escape {
                b as u16
            } else {
                let next = |j: usize| raw.get(j).copied().ok_or_else(|| bad("code cut short"));
                if b == 0xff {
                    let c = (next(i)? as u16) << 8 | next(i + 1)? as u16;
                    i += 2;
                    c
                } else {
                    let c = ((b as u16) << 8 | next(i)? as u16).wrapping_sub(self.escape.wrapping_mul(0xff));
                    i += 1;
                    c
                }
            };
            stack.push(code.wrapping_add(1));
            while let Some(mut node) = stack.pop() {
                loop {
                    let &(a, b) = self.nodes.get(node as usize).ok_or_else(|| bad("node out of range"))?;
                    if b == 0 {
                        units.push(a);
                        break;
                    }
                    if stack.len() > 4096 || units.len() > 1 << 20 {
                        return Err(bad("runaway expansion"));
                    }
                    stack.push(a);
                    node = b;
                }
            }
        }
        while units.last() == Some(&0) {
            units.pop();
        }
        Ok(String::from_utf16_lossy(&units))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built blob: nodes 'H','i','!', pair "Hi", one section with
    /// two strings ("Hi!" id 10 and "!" id 13), one using an escaped code.
    #[test]
    fn decodes_pairs_tables_and_escapes() {
        let mut d = Vec::new();
        let escape = 3u16;
        d.extend(escape.to_be_bytes());
        // node 0 unused; 1 'H', 2 'i', 3 '!'; (a, b) expands b then a, so "Hi" is (2, 1)
        let nodes: [(u16, u16); 5] = [(0, 0), (b'H' as u16, 0), (b'i' as u16, 0), (b'!' as u16, 0), (2, 1)];
        d.extend((nodes.len() as u16).to_be_bytes());
        for (a, b) in nodes {
            d.extend(a.to_be_bytes());
            d.extend(b.to_be_bytes());
        }
        d.extend(1u16.to_be_bytes());
        let section = d.len();
        d.extend([0u8; 12]);
        let table = d.len();
        // n = 1, end0 = 4; (delta 3, end 5)
        for v in [1u16, 4, 3, 5] {
            d.extend(v.to_be_bytes());
        }
        let bytes = d.len();
        // "Hi" = node 4 = code 3 (>= escape: 0xff form), '!' = code 2; then '!' again
        d.extend([0xff, 0x00, 0x03, 0x02, 0x02]);
        d[section..section + 4].copy_from_slice(&10u32.to_be_bytes());
        d[section + 4..section + 8].copy_from_slice(&(bytes as u32).to_be_bytes());
        d[section + 8..section + 12].copy_from_slice(&(table as u32).to_be_bytes());
        let loc = Localization::parse(&d).unwrap();
        let s = loc.strings().unwrap();
        assert_eq!(s, vec![LocString { id: 10, text: "Hi!".into() }, LocString { id: 13, text: "!".into() }]);
        // two-byte form: b >= escape, b != 0xff -> (b << 8 | c) - escape * 255
        assert_eq!(loc.decode(&[0x03, 0x00]).unwrap(), "Hi");
    }
}
