//! Prints length and root displacement of the freerun jump clips:
//! `jumpclips MAP.forge` (env ACBMP_SCHEMAS, ACBMP_EXE).
use acb_anim::{pose::Clip, BuiltinTracks};
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).expect("map");
    let pat = std::env::args().nth(2).unwrap_or("_to_air|_to_surface|jumpstraight".into());
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&map), &schema)?;
    let b = BuiltinTracks::from_executable(&std::fs::read(std::env::var("ACBMP_EXE")?)?);
    let pats: Vec<&str> = pat.split('|').collect();
    let mut rows = Vec::new();
    for u in archive.uids_of("Animation") {
        let Some(l) = archive.loc(u) else { continue };
        let n = l.name.to_ascii_lowercase();
        if !pats.iter().any(|p| p.split('&').all(|q| n.contains(q))) {
            continue;
        }
        let Some(c) = Clip::load_with(&archive, u, b.as_ref()) else { continue };
        let d = c.root_motion(c.length).w_axis;
        // apex of the root track
        let apex = (0..=40).map(|i| c.root_motion(c.length * i as f32 / 40.0).w_axis.z).fold(f32::MIN, f32::max);
        rows.push(format!("{n:70} len {:.3} disp {:6.2} {:6.2} {:6.2} apex {:5.2}", c.length, d.x, d.y, d.z, apex));
    }
    rows.sort();
    for r in rows { println!("{r}"); }
    Ok(())
}
