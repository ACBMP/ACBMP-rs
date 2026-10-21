//! Lists narrow walkable tops (railings, beams, posts) in a map's
//! collision: `narrow MAP.forge CX CY RADIUS` (env ACBMP_SCHEMAS).
use acb_game::ground::Ground;
use glam::{Vec2, Vec3};
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let g = Ground::load(&archive)?;
    if a[2] == "line" {
        // floor heights (from 60 m down) along a segment, every 0.1 m
        let v: Vec<f32> = a[3..7].iter().map(|x| x.parse().unwrap()).collect();
        let (p0, p1) = (Vec2::new(v[0], v[1]), Vec2::new(v[2], v[3]));
        let n = ((p1 - p0).length() / 0.1) as usize;
        let zs: Vec<String> = (0..=n).map(|i| g.floor_below(p0.lerp(p1, i as f32 / n as f32).extend(60.0)).map_or("-".into(), |z| format!("{z:.2}"))).collect();
        println!("{}", zs.join(" "));
        return Ok(());
    }
    if a[2] == "tris" {
        // thin long walkable faces standing above the floor beside them
        for t in g.floor_triangles() {
            let lo = t[0].min(t[1]).min(t[2]);
            let hi = t[0].max(t[1]).max(t[2]);
            let ext = (hi - lo).truncate();
            let (thin, long) = (ext.min_element(), ext.max_element());
            if thin > 0.3 || long < 1.0 || hi.z - lo.z > 0.3 {
                continue;
            }
            let c = (lo + hi) * 0.5;
            let across = if ext.x < ext.y { Vec2::X } else { Vec2::Y };
            let side = |s: f32| g.floor_below((c.truncate() + across * s * (thin * 0.5 + 0.15)).extend(c.z - 0.05)).map_or(-99.0, |z| z);
            let drop = c.z - side(1.0).max(side(-1.0));
            if drop > 0.6 && drop < 1.4 {
                println!("thin {:.2} {:.2} {:.2} w {thin:.2} len {long:.2} drop {drop:.2} across {}", c.x, c.y, c.z, if across == Vec2::X { "x" } else { "y" });
            }
        }
        return Ok(());
    }
    let (cx, cy, r): (f32, f32, f32) = (a[2].parse()?, a[3].parse()?, a[4].parse()?);
    let mut seen = std::collections::HashSet::new();
    let n = (r / 0.1) as i32;
    for i in -n..=n {
        for j in -n..=n {
            let p = Vec2::new(cx + i as f32 * 0.1, cy + j as f32 * 0.1);
            let Some(z) = g.floor_below(p.extend(60.0)) else { continue };
            let below = |d: Vec2| g.floor_below((p + d).extend(z + 0.05)).map_or(-100.0, |f| f);
            let drop = |d: Vec2| {
                let (a, b) = (below(d), below(-d));
                if (a - b).abs() > 0.15 { 0.0 } else { z - a.max(b) }
            };
            let flat = |d: Vec2| [1.0f32, -1.0].iter().all(|s| (below(d * *s) - z).abs() < 0.05);
            let (dx, dy) = (drop(Vec2::X * 0.2), drop(Vec2::Y * 0.2));
            let narrow_x = dx > 0.6 && dx < 1.3 && (dy > 0.6 || flat(Vec2::Y * 0.4) && flat(Vec2::Y * 0.8));
            let narrow_y = dy > 0.6 && dy < 1.3 && (dx > 0.6 || flat(Vec2::X * 0.4) && flat(Vec2::X * 0.8));
            if !(narrow_x || narrow_y) { continue; }
            let h = dx.max(dy);
            let kind = if narrow_x && narrow_y { "post" } else { "rail" };
            let key = ((p.x / 1.0).round() as i32, (p.y / 1.0).round() as i32, kind);
            if seen.insert(key) {
                println!("{kind} {:.2} {:.2} {:.2} drop {:.2} {}", p.x, p.y, z, h.min(99.0), if narrow_x { "across x" } else { "across y" });
            }
        }
    }
    let _ = Vec3::ZERO;
    Ok(())
}
