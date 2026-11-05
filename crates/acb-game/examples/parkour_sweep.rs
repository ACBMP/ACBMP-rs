//! Climbs every wall with guidance edges on San Donato and reports how
//! each attempt ends: `parkour_sweep MAP.forge [MAX [SKIP]]` (env
//! ACBMP_SCHEMAS, ACBMP_EXE; about 15 s for the whole map). A wall is a
//! hold 1.0-4.5 m over the floor in front of it, within 200 m of the
//! origin, deduplicated on a 3 m grid; the player starts 1.6 m out facing
//! it and runs at it for 10 s: Shift+W, or Shift+Space+W (freerun) with
//! SWEEP_LEGS set. SWEEP_TIMING prints each wall's worst tick. SWEEP_ANGLE=D
//! comes at each wall D degrees off square (from the same side, 1.6 m out
//! along that line).
use std::sync::Arc;

use acb_anim::BuiltinTracks;
use acb_game::{PlayerInput, Setup, World, DT};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = Box::leak(Box::new(acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?));
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), schema)?;
    let max: usize = a.get(2).map_or(400, |c| c.parse().unwrap());
    let skip: usize = a.get(3).map_or(0, |c| c.parse().unwrap());
    let builtins = BuiltinTracks::from_executable(&std::fs::read(std::env::var("ACBMP_EXE")?)?);
    let name = "AC2MP_ID07_MainTemplate_Player";
    let character = Arc::new(acb_game::load_character(&archive, name)?);
    let setup = Setup {
        characters: vec![(name.into(), character)],
        count: Some(1),
        anim: String::new(),
        wander: 0.0,
        play: true,
        crowd: 0,
        pair: Vec::new(),
        at: None,
        builtins: builtins.as_ref(),
    };
    let mut w = World::load(&archive, &setup)?;
    let base = w.save();
    let g = w.ground.take().unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut starts = Vec::new();
    for h in g.holds_around(glam::Vec3::ZERO, 200.0) {
        if h.top && h.length() < 0.8 {
            continue;
        }
        let q = (h.a + h.b) / 2.0;
        let stand = q + (h.wall * 1.6).extend(0.0);
        let Some(z) = g.floor_below(stand.with_z(q.z - 0.9)) else { continue };
        let dz = q.z - z;
        if !(1.0..=4.5).contains(&dz) {
            continue;
        }
        // a clear run-up (floor all the way, no wall in between)
        if g.sweep_wall(stand.with_z(z + 0.5), -h.wall * 0.9, 0.0).is_some() {
            continue;
        }
        if !seen.insert(((q.x / 3.0) as i32, (q.y / 3.0) as i32, (q.z / 3.0) as i32)) {
            continue;
        }
        starts.push((stand.with_z(z), -h.wall, dz));
    }
    w.ground = Some(g);
    eprintln!("{} walls in all", starts.len());
    let starts: Vec<_> = starts.into_iter().skip(skip).take(max).collect();
    let mut tally: std::collections::BTreeMap<&str, usize> = Default::default();
    let off: f32 = std::env::var("SWEEP_ANGLE").map_or(0.0, |d| d.parse::<f32>().unwrap().to_radians());
    for (at, fwd, dz) in &starts {
        let wall_t = std::time::Instant::now();
        w.restore(&base);
        // the same spot on the wall, the start swung round it
        let square = *fwd;
        let fwd = &glam::Vec2::from_angle(off).rotate(square);
        let at = &(*at + ((square - *fwd) * 1.6).extend(0.0));
        let at = &at.with_z(w.ground.as_ref().unwrap().floor_below(*at + glam::Vec3::Z * 1.0).unwrap_or(at.z));
        let yaw = (-fwd.x).atan2(fwd.y);
        w.place_player(0, *at, yaw);
        let cam = fwd.y.atan2(fwd.x);
        let input = PlayerInput { forward: true, run: true, sprint: std::env::var_os("SWEEP_LEGS").is_some(), cam_yaw: cam, ..PlayerInput::default() };
        let (mut climbed, mut top_z, mut beam_end) = (false, at.z, false);
        // first stand after climbing: (on top, beam, no headroom)
        let mut stood: Option<(bool, bool, bool)> = None;
        let mut worst = (0.0f64, String::new());
        for _ in 0..(10.0 / DT) as usize {
            let was = w.players[0].climb.is_some();
            let t0 = std::time::Instant::now();
            w.step(&[input]);
            let dt_ms = t0.elapsed().as_secs_f64() * 1e3;
            if dt_ms > worst.0 {
                let p = &w.players[0];
                worst = (dt_ms, format!("{:.2?} climb {} air {} beam {}", p.pos, p.climb.is_some(), p.loco.airborne(), p.loco.on_beam()));
            }
            let p = &w.players[0];
            climbed |= p.climb.is_some();
            top_z = top_z.max(p.pos.z);
            if was && p.climb.is_none() && !p.loco.airborne() && stood.is_none() {
                let g = w.ground.as_ref().unwrap();
                stood = Some((p.pos.z > at.z + dz - 0.4, g.beam(p.pos).is_some(), g.headroom(p.pos).is_some()));
            }
        }
        let stood_label = match stood {
            Some((true, false, false)) => "stood top",
            Some((true, true, false)) => "stood top on beam",
            Some((_, _, true)) => "stood BURIED",
            Some((false, _, _)) => "stood below top",
            None => "never stood",
        };
        *tally.entry(stood_label).or_default() += 1;
        if std::env::var_os("SWEEP_TIMING").is_some() {
            println!("worst tick {:.1} ms at {}; wall {:.0} ms", worst.0, worst.1, wall_t.elapsed().as_secs_f64() * 1e3);
        }
        let p = &w.players[0];
        let g = w.ground.as_ref().unwrap();
        if p.climb.is_none() && !p.loco.airborne() {
            beam_end = p.loco.on_beam();
        }
        let up = p.pos.z - at.z;
        let kind = if p.climb.is_some() {
            "still on wall"
        } else if !climbed {
            "never climbed"
        } else if up > dz - 0.3 && beam_end {
            "top, on beam"
        } else if up > dz - 0.3 {
            "top"
        } else if beam_end {
            "beam below top"
        } else if up > 0.6 {
            "partway up"
        } else {
            "fell back"
        };
        // standing inside a wall: no headroom over the feet
        let buried = p.climb.is_none() && g.headroom(p.pos).is_some();
        *tally.entry(kind).or_default() += 1;
        if buried {
            *tally.entry("(ends with no headroom)").or_default() += 1;
        }
        println!(
            "{kind:15} [{stood_label}] {} start {:.2} {:.2} {:.2} yaw {:.0} hold +{dz:.2} -> {:.2} {:.2} {:.2} (max +{:.2}){}",
            if buried { "BURIED" } else { "      " },
            at.x,
            at.y,
            at.z,
            yaw.to_degrees(),
            p.pos.x,
            p.pos.y,
            p.pos.z,
            top_z - at.z,
            if p.loco.on_beam() { " beam" } else { "" }
        );
    }
    println!("{} walls: {tally:?}", starts.len());
    Ok(())
}
