//! What freerun guidance a map holds: component kinds (plain, Cylinder,
//! Capsule, Barrel) with their fields, edge sub-types and how many edges
//! have collision at them. `guidance_kinds MAP.forge` (env ACBMP_SCHEMAS).
use acb_game::ground::Ground;
use anvil_fastload::{Obj, Value};
use glam::Vec3;
use std::collections::BTreeMap;

fn show(schema: &anvil_schema::Schema, o: &Obj, depth: usize) {
    for (h, v) in &o.fields {
        let name = schema.name_or_hash(*h);
        let val = match v {
            Value::Raw(b) if b.len() == 4 => format!("{} / {:.3}", u32::from_le_bytes([b[0], b[1], b[2], b[3]]), f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Value::Raw(b) => format!("raw {b:?}"),
            Value::Packed { elem_size, data } => format!("packed {} x {elem_size}", data.len() / (*elem_size).max(1)),
            Value::Array(a) => format!("array {}", a.len()),
            Value::Object(_) => "object".into(),
            Value::Ptr { obj: Some(_), .. } => "ptr obj".into(),
            _ => "other".into(),
        };
        println!("{:depth$}{name} = {val}", "");
        if let Value::Array(a) = v {
            if let Some(first) = a.iter().find_map(Value::as_obj) {
                show(schema, first, depth + 2);
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    let mut shown: BTreeMap<String, usize> = BTreeMap::new();
    let mut visit = |o: &Obj| {
        let name = schema.name_or_hash(o.type_hash);
        if !name.contains("Guidance") || !seen.insert(o.id) {
            return;
        }
        *count.entry(name.clone()).or_default() += 1;
        let s = shown.entry(name.clone()).or_default();
        if *s < 1 {
            *s += 1;
            println!("--- {name}");
            show(&schema, o, 2);
        }
    };
    for t in ["Entity", "EntityGroup"] {
        for uid in archive.uids_of_in(t, 0) {
            if let Ok(r) = archive.root(uid) {
                r.obj.walk(&mut |o| visit(o));
            }
        }
    }
    println!("{count:?}");
    if let Some(e) = schema.hash_of("GuidanceObjectSubType").and_then(|h| schema.enum_def(h)) {
        println!("sub types: {e:?}");
    }
    let edges = archive.guidance_edges();
    let g = Ground::load(&archive)?;
    for e in edges.iter().filter(|e| e.sub_type != 1).take(30) {
        let (p, q) = (Vec3::from(e.a), Vec3::from(e.b));
        println!("sub {} entity {} {:.2?} -> {:.2?} len {:.2} n {:.2?} {:.2?}", e.sub_type, e.entity, p.to_array(), q.to_array(), p.distance(q), e.normals[0], e.normals[1]);
    }
    println!("enum: {:?}", sub_types(&schema));
    hold_faults(&g, &edges);
    Ok(())
}

/// The enum holding `GuidanceObjectSubTypePole`, by value.
fn sub_types(schema: &anvil_schema::Schema) -> Vec<(i32, String)> {
    schema
        .global_enums
        .values()
        .chain(schema.types.values().flat_map(|t| &t.enums))
        .find(|e| e.values.iter().any(|v| schema.name_or_hash(v.0).contains("GuidanceObjectSubTypePole")))
        .map(|e| {
            let mut v: Vec<(i32, String)> = e.values.iter().map(|&(h, i)| (i, schema.name_or_hash(h))).collect();
            v.sort();
            v
        })
        .unwrap_or_default()
}

/// How many wall holds fail each sanity test.
fn hold_faults(g: &Ground, edges: &[acb_world::GuidanceEdge]) {
    let (mut under, mut concave, mut proud15, mut proud35, mut orphan, mut total) = (0, 0, 0, 0, 0, 0);
    let mut show = 0;
    for e in edges {
        let Some(w) = e.normals.iter().map(|n| Vec3::from(*n)).find(|n| n.z.abs() < 0.5 && n.truncate().length() > 0.5) else { continue };
        total += 1;
        let n = w.truncate().normalize();
        let q = (Vec3::from(e.a) + Vec3::from(e.b)) / 2.0;
        let ceiling = e.normals.iter().any(|n| n[2] < -0.5);
        let out = |d: f32, down: f32| q + (n * d).extend(-down);
        let is_concave = g.floor_below(out(0.25, -0.1)).is_some_and(|f| (f - q.z).abs() < 0.25);
        let p15 = g.ray(out(0.9, 0.1), (-n * 0.75).extend(0.0)).is_some();
        let p35 = g.ray(out(0.9, 0.1), (-n * 0.55).extend(0.0)).is_some();
        let none = g.ray(out(0.3, 0.05), (-n * 0.6).extend(0.0)).is_none() && g.ray(out(-0.05, -0.3), Vec3::Z * -0.6).is_none();
        under += ceiling as u32;
        concave += is_concave as u32;
        proud15 += p15 as u32;
        proud35 += p35 as u32;
        orphan += none as u32;
        if (ceiling || is_concave || p35) && show < 12 && q.truncate().length() < 40.0 {
            show += 1;
            println!("fault at {:.2?} n {:.2?} ceiling {ceiling} concave {is_concave} p15 {p15} p35 {p35} none {none}", q.to_array(), n.to_array());
        }
    }
    println!("holds {total}: underside {under}, concave {concave}, proud>0.15 {proud15}, proud>0.35 {proud35}, no collision {orphan}");
}
