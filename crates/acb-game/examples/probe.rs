//! Freerun geometry around a point: `probe MAP.forge X Y Z` (env
//! ACBMP_SCHEMAS): holds within 2.5 m, and the floor and headroom on a
//! grid round it.
use acb_game::ground::Ground;
use glam::Vec3;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let g = Ground::load(&archive)?;
    let p = Vec3::new(a[2].parse()?, a[3].parse()?, a[4].parse()?);
    for h in g.holds_around(p, 2.5) {
        let q = h.nearest(p);
        println!(
            "hold {:.2?} -> {:.2?} wall {:.2?} top {} len {:.2} hang clear at {:.2?}: {}",
            h.a.to_array(),
            h.b.to_array(),
            h.wall.to_array(),
            h.top,
            h.length(),
            q.to_array(),
            g.hang_clear(q, h.wall)
        );
    }
    println!("floor below (z+3) / headroom on a 0.25 m grid, rows +y down:");
    for j in (-6..=6).rev() {
        let row: Vec<String> = (-6..=6)
            .map(|i| {
                let q = p + Vec3::new(i as f32 * 0.25, j as f32 * 0.25, 0.0);
                match g.floor_below(q + Vec3::Z * 3.0) {
                    Some(f) => format!("{:6.2}{}", f, if g.headroom(q.with_z(f)).is_some() { "x" } else { " " }),
                    None => "     - ".into(),
                }
            })
            .collect();
        println!("{:+.2} {}", j as f32 * 0.25, row.join(""));
    }
    Ok(())
}
