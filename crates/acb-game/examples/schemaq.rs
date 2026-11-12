//! Schema lookups: `schemaq name 0xHASH...` (hash -> name), `schemaq enum
//! TYPE PROPERTY [VALUE...]` (an enum property's values), `schemaq props
//! TYPE` (property chain). Env ACBMP_SCHEMAS.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    match a[1].as_str() {
        "name" => {
            for h in &a[2..] {
                let v = u32::from_str_radix(h.trim_start_matches("0x"), 16)?;
                println!("{h} {}", schema.name_or_hash(v));
            }
        }
        "props" => {
            let t = schema.hash_of(&a[2]).ok_or_else(|| anyhow::anyhow!("no type"))?;
            for p in schema.property_chain(t) {
                println!("{} {:?} {:?} -> {}", schema.name_or_hash(p.name_hash), p.kind(), p.elem_kind(), schema.name_or_hash(p.object_hash()));
            }
        }
        "enum" => {
            let t = schema.hash_of(&a[2]).ok_or_else(|| anyhow::anyhow!("no type"))?;
            let p = schema.property_chain(t).into_iter().find(|p| schema.name(p.name_hash) == Some(a[3].as_str())).ok_or_else(|| anyhow::anyhow!("no property"))?;
            let e = schema.enum_def(p.object_hash()).ok_or_else(|| anyhow::anyhow!("no enum {:#x}", p.object_hash()))?;
            let want: Vec<i32> = a[4..].iter().filter_map(|v| v.parse().ok()).collect();
            println!("enum {}", schema.name_or_hash(e.name_hash));
            for (n, v) in &e.values {
                if want.is_empty() || want.contains(v) {
                    println!("  {v} {}", schema.name_or_hash(*n));
                }
            }
        }
        _ => {}
    }
    Ok(())
}
