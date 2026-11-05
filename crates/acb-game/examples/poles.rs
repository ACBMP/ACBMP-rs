//! Where San Donato's guidance poles are and which floors a running jump
//! reaches them from: `poles MAP.forge EXE` (env ACBMP_SCHEMAS). Prints a
//! `--at` for the viewer (x,y,z,yaw facing the pole) per pole and side.
use acb_anim::BuiltinTracks;
use acb_game::ground::Ground;
use acb_game::jump;
use acb_game::locomotion::Clips;
use glam::Vec3;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let builtins = BuiltinTracks::from_executable(&std::fs::read(&a[2])?);
    let clips = Clips::load(&archive, builtins.as_ref())?;
    let g = Ground::load(&archive)?;
    let bars: Vec<_> = g.holds_around(Vec3::ZERO, 400.0).into_iter().filter(|h| h.bar && h.length() >= 1.0).collect();
    println!("{} pole holds", bars.len());
    for h in bars {
        let mid = (h.a + h.b) / 2.0;
        let mut found = 0;
        for d in [1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 5.0, 6.0] {
            for up in [-2.0, 0.0, 2.0] {
                let probe = mid + (h.wall * d).extend(up);
                let Some(f) = g.floor_below(probe) else { continue };
                if found > 0 {
                    continue;
                }
                let feet = probe.with_z(f);
                if let Some((t, flight, _)) = jump::bar_target(&g, &clips, feet, -h.wall) {
                    found += 1;
                    let yaw = (h.wall.x).atan2(-h.wall.y).to_degrees();
                    println!("pole {:.2?} n {:.2?} from --at={:.2},{:.2},{:.2},{:.0} d {d} hang feet {:.2?} in {:.2} s", mid.to_array(), h.wall.to_array(), feet.x, feet.y, feet.z, yaw, t.to_array(), flight.dur);
                }
            }
        }
    }
    Ok(())
}
