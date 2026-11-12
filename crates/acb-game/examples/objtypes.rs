//! Histogram of every object type nested in a map's roots; with a pattern,
//! the trees of the entities holding an object whose type name contains it:
//! `objtypes MAP.forge [PATTERN]` (env ACBMP_SCHEMAS). `objtypes MAP.forge
//! 0xUID...` prints the trees of those roots instead.
use std::collections::BTreeMap;

use anvil_fastload::{Obj, Value};
use anvil_schema::Schema;

fn raw(b: &[u8]) -> String {
    match b.len() {
        1 => format!("{}", b[0]),
        2 => format!("{}", u16::from_le_bytes([b[0], b[1]])),
        4 => {
            let u = u32::from_le_bytes(b.try_into().unwrap());
            let f = f32::from_bits(u);
            if u < 0x10000 { format!("{u}") } else if f.abs() > 1e-6 && f.abs() < 1e7 { format!("{f} ({u:#x})") } else { format!("{u:#010x}") }
        }
        n if n % 4 == 0 && n <= 64 => {
            let f: Vec<f32> = b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
            format!("{f:.3?}")
        }
        n => format!("<{n} bytes>"),
    }
}

fn value(s: &Schema, v: &Value, depth: usize, skip: &[u32]) {
    let pad = "  ".repeat(depth);
    match v {
        Value::Raw(b) => println!("{}", raw(b)),
        Value::Handle { id, .. } => println!("handle {id:#010x}"),
        Value::Ref { id, obj, .. } => match obj {
            Some(o) => obj_dump(s, o, depth, skip),
            None => println!("ref {id:#010x}"),
        },
        Value::Str(b) => println!("{:?}", String::from_utf8_lossy(b)),
        Value::Object(o) => obj_dump(s, o, depth, skip),
        Value::Ptr { link, obj, .. } => match obj {
            Some(o) => obj_dump(s, o, depth, skip),
            None => println!("ptr {:#010x}", link.unwrap_or(0)),
        },
        Value::Packed { elem_size, data } => println!("<packed {} x {elem_size}>", data.len() / elem_size.max(&1)),
        Value::Array(items) => {
            println!("[{}]", items.len());
            for (i, it) in items.iter().enumerate().take(12) {
                print!("{pad}  [{i}] ");
                value(s, it, depth + 1, skip);
            }
        }
    }
}

fn obj_dump(s: &Schema, o: &Obj, depth: usize, skip: &[u32]) {
    let pad = "  ".repeat(depth);
    println!("{} {:#010x}", s.name_or_hash(o.type_hash), o.id);
    if skip.contains(&o.type_hash) {
        return;
    }
    for (h, v) in &o.fields {
        print!("{pad}  {}: ", s.name_or_hash(*h));
        value(s, v, depth + 1, skip);
    }
    for d in o.dyn_props.iter().flatten() {
        print!("{pad}  dyn {}: ", s.name_or_hash(d.name_hash));
        value(s, &d.value, depth + 1, skip);
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let pat = a.get(2).map(|s| s.to_lowercase());
    let skip: Vec<u32> = ["Visual", "GuidanceSystem", "BoundingVolume", "DataLayerFilter", "EntityDescriptor", "MeshInstanceData", "LODSelectorInstance", "CompiledMeshInstance"]
        .iter()
        .filter_map(|n| schema.hash_of(n))
        .collect();
    if a.get(2).is_some_and(|p| p.starts_with("0x")) {
        for u in &a[2..] {
            let uid = u32::from_str_radix(u.trim_start_matches("0x"), 16)?;
            match archive.root(uid) {
                Ok(r) => {
                    print!("=== {}: ", archive.loc(uid).map(|l| l.name.clone()).unwrap_or_default());
                    obj_dump(&schema, &r.obj, 0, &[]);
                }
                Err(e) => println!("=== {u}: {e}"),
            }
        }
        return Ok(());
    }
    let mut hist: BTreeMap<String, usize> = BTreeMap::new();
    let entity = schema.hash_of("Entity");
    for ty in ["Entity", "EntityGroup"] {
        for uid in archive.uids_of_in(ty, 0) {
            let Ok(r) = archive.root(uid) else { continue };
            let root_name = archive.loc(uid).map(|l| l.name.clone()).unwrap_or_default();
            r.obj.walk(&mut |o| {
                *hist.entry(schema.name_or_hash(o.type_hash)).or_default() += 1;
            });
            let Some(pat) = &pat else { continue };
            // the entities holding a match, innermost first; else the root
            let mut ents: Vec<&Obj> = Vec::new();
            r.obj.walk(&mut |o| {
                if Some(o.type_hash) == entity {
                    ents.push(o);
                }
            });
            ents.reverse();
            ents.push(&r.obj);
            let mut shown: Vec<u32> = Vec::new();
            for e in ents {
                let mut hits: Vec<u32> = Vec::new();
                e.walk(&mut |o| {
                    if schema.name_or_hash(o.type_hash).to_lowercase().contains(pat) {
                        hits.push(o.id);
                    }
                });
                hits.retain(|h| !shown.contains(h));
                if !hits.is_empty() {
                    shown.extend(hits);
                    print!("=== {root_name}: ");
                    obj_dump(&schema, e, 0, &skip);
                }
            }
        }
    }
    if pat.is_none() {
        for (n, c) in hist {
            println!("{c:6} {n}");
        }
    }
    Ok(())
}
