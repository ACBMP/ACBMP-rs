//! Climbing on San Donato's real guidance edges. Needs the game data:
//! `ACBMP_SCHEMAS` and `ACBMP_MULTI` (the `multi` directory); skipped
//! otherwise.

use std::path::PathBuf;

use acb_game::ground::Ground;
use glam::Vec3;

fn ground() -> Option<Ground> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).ok()?));
    let archive = acb_world::open_map(&multi.join("DataPC_AC2MP_SanDonato.forge"), schema).ok()?;
    Ground::load(&archive).ok()
}

#[test]
fn explore() {
    let Some(g) = ground() else { return };
    let p = std::env::var("PROBE").ok().map(|s| { let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect(); Vec3::new(v[0], v[1], v[2]) }).unwrap_or(Vec3::new(-13.3, 44.6, 2.22));
    for h in g.holds_around(p, 4.0) {
        let q = h.nearest(p);
        println!("{:?} -> {:?} wall {:?} top {} dz {:.2} d {:.2}", h.a, h.b, h.wall, h.top, q.z - p.z, (q - p).truncate().length());
    }
    // height of every hold above the floor in front of it
    let mut hist = [0usize; 12];
    let mut none = 0;
    for h in g.holds_around(Vec3::ZERO, 1e4) {
        let q = (h.a + h.b) / 2.0;
        let stand = q + (h.wall * 0.6).extend(0.0);
        match g.floor_below(stand.with_z(q.z - 0.3)) {
            Some(z) => hist[((q.z - z) / 0.5).clamp(0.0, 11.0) as usize] += 1,
            None => none += 1,
        }
    }
    println!("hold height above the floor in front, 0.5 m bins: {hist:?}, no floor {none}");
    // wall-run spots under two stacked holds (lower one in wall-run reach)
    let mut found = Vec::new();
    for l in g.holds_around(p, 40.0) {
        let q = (l.a + l.b) / 2.0;
        let stand = q + (l.wall * 2.5).extend(0.0);
        let Some(z) = g.floor_below(stand.with_z(q.z - 1.0)) else { continue };
        if !(1.8..=4.2).contains(&(q.z - z)) || l.length() < 3.0 {
            continue;
        }
        if let Some((u, uq)) = g.next_hold(q, l.wall, acb_game::climb::Dir::Up) {
            found.push((stand.with_z(z).distance(p), stand.with_z(z), l.wall, q, uq, u.top));
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    for f in found.iter().take(8) {
        println!("stack: stand {:.2?} wall {:.2?} low {:.2?} up {:.2?} top {}", f.1, f.2, f.3, f.4, f.5);
    }
    for (spot, f) in g.mount_spots(p, 8) {
        println!("mount spot {spot:.2?} facing {f:.2?} yaw {:.0}", f.y.atan2(f.x).to_degrees());
    }
}

#[test]
fn pose_probe() {
    let Ok(clips) = std::env::var("POSE_PROBE") else { return };
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS").unwrap());
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI").unwrap());
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).unwrap()));
    let archive = acb_world::open_map(&multi.join("DataPC_AC2MP_SanDonato.forge"), schema).unwrap();
    let exe = std::fs::read(multi.join("../ACBMP.exe")).unwrap();
    let builtins = acb_anim::BuiltinTracks::from_executable(&exe);
    let c = acb_game::load_character(&archive, "AC2MP_ID07_MainTemplate_Player").unwrap();
    let names = ["LeftHand", "RightHand", "LeftFoot", "RightFoot", "Reference", "Pelvis", "Head", "LeftToeBase", "RightToeBase"];
    let ids: Vec<(u32, &str)> = names.iter().map(|n| (crc32(n.as_bytes()), *n)).collect();
    for spec in clips.split(',') {
        let (name, t) = spec.split_once('@').map(|(n, t)| (n, t.parse::<f32>().unwrap())).unwrap_or((spec, 0.0));
        let uid = archive.uids_of("Animation").into_iter().find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(name))).unwrap();
        let clip = acb_anim::pose::Clip::load_with(&archive, uid, builtins.as_ref()).unwrap();
        let t = if t < 0.0 { clip.length } else { t };
        let g = c.skeleton.globals(&clip.sample(&c.skeleton, t));
        let root = clip.root_motion(t).w_axis.truncate();
        print!("{name}@{t:.2} root {:.2?}:", root.to_array());
        for (i, b) in c.skeleton.bones.iter().enumerate() {
            if let Some((_, n)) = ids.iter().find(|(h, _)| *h == b.id) {
                print!(" {n} {:.2?}", g[i].w_axis.truncate().to_array());
            }
        }
        println!();
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

#[test]
fn schema_explore() {
    let Ok(pat) = std::env::var("SCHEMA_EXPLORE") else { return };
    let schema = acb_world::load_schema(&PathBuf::from(std::env::var_os("ACBMP_SCHEMAS").unwrap())).unwrap();
    if let Some(hashes) = pat.strip_prefix("hashes=") {
        for h in std::fs::read_to_string(hashes).unwrap().split_whitespace() {
            if let Ok(v) = u32::from_str_radix(h.trim_start_matches("0x"), 16) {
                println!("{h} {}", schema.name_or_hash(v));
            }
        }
        return;
    }
    let mut names: Vec<_> = schema
        .types
        .keys()
        .filter_map(|h| schema.name(*h).map(|n| (n.to_string(), *h)))
        .filter(|(n, _)| pat.split(',').any(|p| n.contains(p)))
        .collect();
    names.sort();
    for (n, h) in names {
        let t = schema.type_def(h).unwrap();
        println!("{n} : {}", schema.name_or_hash(t.base_type_hash));
        for p in &t.properties {
            println!("    {} {:?}", schema.name_or_hash(p.name_hash), p.kind());
        }
    }
}

/// The entry checks find San Donato's walls: a wall run at the spawn-side
/// wall grabs its 3.8 m roof edge, and walking into the low roof wall
/// north of it climbs straight on.
#[test]
fn entries_on_san_donato() {
    let Some(g) = ground() else { return };
    let (feet, fwd) = (Vec3::new(-14.47, 44.18, 2.22), glam::Vec2::X);
    let (hit, n) = g.wall_ahead(feet, fwd, fwd * 4.0).expect("wall ahead");
    assert!(n.dot(-fwd) > 0.9, "wall normal {n:?}");
    let (hold, point) = g.wall_run_hold(hit, n, feet.z).expect("wall-run hold");
    assert!((point.z - 6.02).abs() < 0.1 && hold.top, "{point:?}");
    // running parallel to the wall is no wall run
    assert!(g.wall_ahead(feet, fwd, glam::Vec2::Y * 4.0).is_none());
    assert!(g.climb_entry(Vec3::new(-11.19, 47.82, 6.61), glam::Vec2::Y).is_some());
}
