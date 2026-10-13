use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anvil_forge::{DataFile, Forge};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "acbmp", about = "Inspect ACB MP game data")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the entries of a forge archive
    Ls { forge: PathBuf },
    /// FNV-1a digest over every sub-object's type hash and payload, in order
    Digest { forge: PathBuf },
    /// Count sub-objects per type hash across every .data entry
    Census {
        forge: PathBuf,
        /// Rows to print
        #[arg(long, default_value_t = 40)]
        top: usize,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Ls { forge } => {
            let forge = Forge::open(forge)?;
            for e in forge.entries() {
                println!("{:6} {:#010x} {:10} {}", e.index, e.id, e.length, e.name);
            }
        }
        Cmd::Census { forge, top } => census(forge, top)?,
        Cmd::Digest { forge } => digest(forge)?,
    }
    Ok(())
}

fn census(path: PathBuf, top: usize) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let mut forge = Forge::open(&path)?;
    let entries = forge.entries().to_vec();
    let mut per_type: HashMap<u32, (usize, usize)> = HashMap::new();
    let (mut files, mut objects, mut failed) = (0, 0, 0);
    for e in entries.iter().filter(|e| !e.is_metafile()) {
        let raw = forge.read(e)?;
        match DataFile::parse(&raw) {
            Ok(df) => {
                files += 1;
                for o in df.objects {
                    objects += 1;
                    let slot = per_type.entry(o.type_hash).or_default();
                    slot.0 += 1;
                    slot.1 += o.payload.len();
                }
            }
            Err(err) => {
                failed += 1;
                eprintln!("entry {} ({}): {err}", e.index, e.name);
            }
        }
    }
    println!(
        "{} entries, {files} data files, {failed} failed, {objects} objects, {} types, {:.1}s",
        entries.len(),
        per_type.len(),
        t0.elapsed().as_secs_f64()
    );
    let mut rows: Vec<_> = per_type.into_iter().collect();
    rows.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    for (hash, (count, bytes)) in rows.into_iter().take(top) {
        println!("{:10.1}MB {count:7} {hash:#010x}", bytes as f64 / 1e6);
    }
    Ok(())
}

fn digest(path: PathBuf) -> anyhow::Result<()> {
    let mut forge = Forge::open(&path)?;
    let entries = forge.entries().to_vec();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    };
    let mut n = 0;
    for e in entries.iter().filter(|e| !e.is_metafile()) {
        for o in DataFile::parse(&forge.read(e)?)?.objects {
            eat(&o.type_hash.to_le_bytes());
            eat(&o.payload);
            n += 1;
        }
    }
    println!("{n} objects digest {h:016x}");
    if let Some(out) = std::env::var_os("ACBMP_DUMP") {
        // concatenation of the same bytes, for cross-checking with other tools
        let mut w = std::io::BufWriter::new(std::fs::File::create(out)?);
        for e in entries.iter().filter(|e| !e.is_metafile()) {
            for o in DataFile::parse(&forge.read(e)?)?.objects {
                std::io::Write::write_all(&mut w, &o.type_hash.to_le_bytes())?;
                std::io::Write::write_all(&mut w, &o.payload)?;
            }
        }
    }
    Ok(())
}
