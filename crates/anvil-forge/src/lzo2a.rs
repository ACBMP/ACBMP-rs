//! LZO2A decompressor, ported from liblzo2 2.10 `lzo2a_d.ch` (safe variant).
//!
//! Brotherhood and Revelations compress every `.data` block with algorithm 2,
//! which is LZO2A. The reference build uses `SWD_N = 8191`, so there is no
//! extra M3 offset bit, and `LZO_EOF_CODE` is on: a long match whose offset is
//! zero ends the stream.

use crate::Error;

const M1_MIN_LEN: usize = 2;

/// Decompresses `input` into a buffer of exactly `out_len` bytes.
pub fn decompress(input: &[u8], out_len: usize) -> Result<Vec<u8>, Error> {
    let mut out: Vec<u8> = Vec::with_capacity(out_len);
    let mut ip = 0usize;
    let mut b: u32 = 0;
    let mut k: u32 = 0;

    let next = |ip: &mut usize| -> Result<u8, Error> {
        let v = *input.get(*ip).ok_or(Error::Lzo("input overrun"))?;
        *ip += 1;
        Ok(v)
    };

    macro_rules! needbits {
        ($j:expr) => {
            if k < $j {
                b |= (next(&mut ip)? as u32) << k;
                k += 8;
            }
        };
    }
    macro_rules! dumpbits {
        ($j:expr) => {
            b >>= $j;
            k -= $j;
        };
    }

    loop {
        if ip >= input.len() && k == 0 {
            return Err(Error::Lzo("no EOF code found"));
        }
        needbits!(1);
        if b & 1 == 0 {
            dumpbits!(1);
            if out.len() >= out_len {
                return Err(Error::Lzo("output overrun"));
            }
            out.push(next(&mut ip)?);
            continue;
        }
        dumpbits!(1);

        needbits!(1);
        let (dist, len);
        if b & 1 == 0 {
            dumpbits!(1);
            // M1: 2..5 bytes, 1..256 back
            needbits!(2);
            len = M1_MIN_LEN + (b & 3) as usize;
            dumpbits!(2);
            dist = 1 + next(&mut ip)? as usize;
        } else {
            dumpbits!(1);
            let t0 = next(&mut ip)? as usize;
            let t1 = next(&mut ip)? as usize;
            dist = (t0 & 31) | (t1 << 5);
            let t = t0 >> 5;
            if t == 0 {
                let mut t = 10 - 1;
                loop {
                    let v = next(&mut ip)?;
                    if v != 0 {
                        t += v as usize;
                        break;
                    }
                    t += 255;
                }
                len = t;
            } else {
                if dist == 0 {
                    break;
                }
                len = t + 2;
            }
        }

        if dist == 0 || dist > out.len() {
            return Err(Error::Lzo("lookbehind overrun"));
        }
        if out.len() + len > out_len {
            return Err(Error::Lzo("output overrun"));
        }
        // byte-wise copy: matches may overlap their own output
        let start = out.len() - dist;
        for i in 0..len {
            let v = out[start + i];
            out.push(v);
        }
    }

    if ip != input.len() {
        return Err(Error::Lzo(if ip < input.len() {
            "input not consumed"
        } else {
            "input overrun"
        }));
    }
    if out.len() != out_len {
        return Err(Error::Lzo("decompressed size mismatch"));
    }
    Ok(out)
}
