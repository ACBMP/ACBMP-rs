use std::collections::BTreeMap;
use std::path::PathBuf;
#[test]
fn probe() {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS").unwrap());
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI").unwrap());
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).unwrap()));
    let a = acb_world::open_map(&multi.join("DataPC_AC2MP_SanDonato.forge"), schema).unwrap();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let pat = std::env::var("PROBE").unwrap_or("Hay|Hid|Hide|Blend|Crowd|Bench|Escape".into());
    let re: Vec<&str> = pat.split('|').collect();
    for ty in ["Entity", "EntityGroup"] {
        for uid in a.uids_of_in(ty, 0) {
            let Ok(r) = a.root(uid) else { continue };
            let name = a.loc(uid).map(|l| l.name.clone()).unwrap_or_default();
            let gm = a.field(&r.obj, "GlobalMatrix").and_then(|v| v.as_f32s()).unwrap_or_default();
            r.obj.walk(&mut |o| {
                let n = schema.name_or_hash(o.type_hash);
                *counts.entry(n.clone()).or_default() += 1;
                if re.iter().any(|p| n.contains(p)) {
                    let pos = if gm.len() == 16 { format!("{:.2} {:.2} {:.2}", gm[12], gm[13], gm[14]) } else { String::new() };
                    println!("HIT {ty} {uid:#x} {name} @ {pos}: {n} #{:#x}", o.id);
                }
            });
        }
    }
    for (n, c) in counts {
        println!("TYPE {c} {n}");
    }
}

#[test]
fn probe_types() {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS").unwrap());
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI").unwrap());
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).unwrap()));
    let a = acb_world::open_map(&multi.join("DataPC_AC2MP_SanDonato.forge"), schema).unwrap();
    let pat = std::env::var("PROBE2").unwrap_or("Falla|GiveUpChase|Escape|FallDamage|Suicide|Blend".into());
    let re: Vec<&str> = pat.split('|').collect();
    for t in schema.types.values() {
        let tn = schema.name_or_hash(t.type_hash);
        let props: Vec<String> = t.properties.iter().map(|p| schema.name_or_hash(p.name_hash)).filter(|n| re.iter().any(|r| n.contains(r))).collect();
        if !props.is_empty() {
            println!("PT {tn} roots {} : {}", a.uids_of(&tn).len(), props.join(","));
        }
    }
}
