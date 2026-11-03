//! Collision area by slope on a map, in retail's landing classes (ground
//! < 45 deg, slope 45-70, wall < 135; `human::in_air::classify`) and around
//! our 53 deg floor limit, plus the biggest slope patches:
//! `slopes MAP.forge [COUNT]` (env ACBMP_SCHEMAS).
use acb_game::ground::Ground;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let g = Ground::load(&archive)?;
    let count: usize = a.get(2).map_or(10, |c| c.parse().unwrap());
    let bands = [(0.0, 45.0), (45.0, 53.0), (53.0, 70.0), (70.0, 90.0), (90.0, 180.0)];
    let mut area = [0.0f32; 5];
    let mut patches = Vec::new();
    for t in g.all_triangles() {
        let c = (t[1] - t[0]).cross(t[2] - t[0]);
        let ar = c.length() / 2.0;
        if ar < 1e-6 {
            continue;
        }
        // the face is taken upward (collision meshes are not consistently wound)
        let deg = (c.z.abs() / c.length()).acos().to_degrees();
        let i = bands.iter().position(|(lo, hi)| deg >= *lo && deg < *hi).unwrap_or(4);
        area[i] += ar;
        if (45.0..70.0).contains(&deg) {
            patches.push((ar, deg, (t[0] + t[1] + t[2]) / 3.0));
        }
    }
    for (i, (lo, hi)) in bands.iter().enumerate().take(4) {
        println!("{lo:3}-{hi:3} deg: {:9.1} m2", area[i]);
    }
    patches.sort_by(|x, y| y.0.total_cmp(&x.0));
    for (ar, deg, c) in patches.iter().take(count) {
        println!("{ar:6.2} m2 {deg:4.1} deg at {:.2} {:.2} {:.2}", c.x, c.y, c.z);
    }
    Ok(())
}
