//! What the climb entry sees at a spot: `wallprobe MAP.forge X Y Z YAW_DEG`
//! (env ACBMP_SCHEMAS): the walls along the stick, and whether a wall run or
//! a mount would start.
use acb_game::ground::Ground;
use glam::{Vec2, Vec3};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let g = Ground::load(&archive)?;
    let n = |i: usize| a[i].parse::<f32>().unwrap();
    let pos = Vec3::new(n(2), n(3), n(4));
    let yaw = n(5).to_radians();
    let fwd = Vec2::new(-yaw.sin(), yaw.cos());
    for h in [0.3, 0.5, 0.7, 1.0, 1.3, 1.8] {
        let s = g.sweep_wall(pos + Vec3::Z * h, fwd * 1.5, 0.0);
        println!("height {h}: wall {:?}", s.map(|(t, n)| (format!("{:.2} m", t * 1.5), format!("n {:.2} {:.2}, off square {:.0} deg", n.x, n.y, fwd.dot(-n).clamp(-1.0, 1.0).acos().to_degrees()))));
    }
    let wall = g.climb_wall(pos, fwd, fwd);
    println!("climb wall {wall:?}");
    if let Some(n) = wall {
        let sq = -n;
        println!("wall ahead {:?}", g.wall_ahead(pos, sq, sq * 5.0).map(|(hit, n)| (hit, n, g.wall_run_hold(hit, n, pos.z, false).map(|x| x.1))));
        println!("mount {:?}", g.climb_entry(pos, sq).map(|x| x.1));
    }
    for h in g.holds_around(pos, 3.0) {
        let q = h.nearest(pos);
        println!("hold at {:.2?} n {:.2?} top {} bar {} (dz {:.2})", q.to_array(), h.wall.to_array(), h.top, h.bar, q.z - pos.z);
    }
    Ok(())
}
