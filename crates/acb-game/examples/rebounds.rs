//! Where a rebound (back eject) off each San Donato hang goes: the jump
//! target the search finds straight away from the wall (a floor, a hang
//! across) or, with none, the farthest foothold (8 m out, 3 m down).
//! `rebounds MAP.forge EXE [X Y Z [COUNT]]` (env ACBMP_SCHEMAS), the
//! highest hangs over their floor first.
use acb_anim::BuiltinTracks;
use acb_game::climb::{entity_for, Stance};
use acb_game::ground::Ground;
use acb_game::locomotion::Clips;
use acb_game::{human, jump};
use glam::Vec3;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let builtins = BuiltinTracks::from_executable(&std::fs::read(&a[2])?);
    let clips = Clips::load(&archive, builtins.as_ref())?;
    let g = Ground::load(&archive)?;
    let near = if a.len() >= 6 { Vec3::new(a[3].parse()?, a[4].parse()?, a[5].parse()?) } else { Vec3::ZERO };
    let count: usize = a.get(6).map_or(15, |c| c.parse().unwrap());
    let mut rows = Vec::new();
    let (mut floor, mut hang, mut far) = (0, 0, 0);
    for h in g.holds_around(near, 200.0).iter().filter(|h| h.length() >= 1.0) {
        let mid = (h.a + h.b) / 2.0;
        if g.hang_stance(mid, h.wall) != Stance::HangWall {
            continue;
        }
        let pos = entity_for(mid, h.wall, Stance::HangWall);
        let away = h.wall;
        let below = g.floor_below(pos + Vec3::Z * 0.1).map_or(-1.0, |z| pos.z - z);
        let to = jump::target(&g, &clips, pos, away, true);
        let to = if to.as_ref().is_none_or(|t| t.0.z < pos.z - 1.75) { jump::hang_target(&g, &clips, pos, away).or(to) } else { to };
        let (kind, target) = match &to {
            Some((t, f, _)) if f.grab.is_some() => {
                hang += 1;
                ("hang", *t)
            }
            Some((t, _, _)) => {
                floor += 1;
                ("floor", *t)
            }
            None => {
                far += 1;
                ("farthest", human::ledge::farthest(pos, away))
            }
        };
        let d = target - pos;
        rows.push((below, format!("{:7.2} {:7.2} {:6.2}  below {below:5.2}  {kind:8} {:5.2} out {:+6.2} up  wall {:+.2} {:+.2}", pos.x, pos.y, pos.z, d.truncate().length(), d.z, away.x, away.y)));
    }
    println!("hangs: {} (floor {floor}, hang {hang}, farthest {far})", floor + hang + far);
    rows.sort_by(|x, y| y.0.total_cmp(&x.0));
    for (_, r) in rows.iter().take(count) {
        println!("{r}");
    }
    Ok(())
}
