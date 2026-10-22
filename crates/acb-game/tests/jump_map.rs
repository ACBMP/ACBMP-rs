//! Freerun jumps on San Donato's collision with the retail clips. Needs
//! the game data: `ACBMP_SCHEMAS`, `ACBMP_MULTI` (the `multi` directory)
//! and `ACBMP_EXE` (builtin root tracks); skipped otherwise.

use std::path::PathBuf;
use std::sync::Arc;

use acb_anim::BuiltinTracks;
use acb_game::ground::Ground;
use acb_game::locomotion::{Clips, JUMPS};
use acb_game::{jump, PlayerInput, Setup, World, DT};
use glam::Vec2;

struct Data {
    archive: acb_world::Archive<'static>,
    builtins: Option<BuiltinTracks>,
}

fn data() -> Option<Data> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let exe = PathBuf::from(std::env::var_os("ACBMP_EXE")?);
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).ok()?));
    let archive = acb_world::open_map(&multi.join("DataPC_AC2MP_SanDonato.forge"), schema).ok()?;
    let builtins = BuiltinTracks::from_executable(&std::fs::read(exe).ok()?);
    Some(Data { archive, builtins })
}

/// Clip pair lengths and end displacement (forward, up) per jump kind,
/// off the left foot: the values NOTES.md documents.
#[test]
fn clip_timings() {
    let Some(d) = data() else { return };
    let clips = Clips::load(&d.archive, d.builtins.as_ref()).unwrap();
    // (kind, forward, up, seconds)
    let want = [
        (0, 0.0, 1.31, 0.2),
        (1, 2.5, 1.31, 0.4),
        (2, 0.5, -0.5, 0.27),
        (3, 2.5, -0.5, 0.4),
        (4, 5.2, -0.5, 0.73),
        (5, 0.5, -3.4, 0.667),
        (6, 2.5, -3.0, 0.533),
        (7, 5.2, -3.0, 0.867),
    ];
    let mut bad = false;
    for (k, fwd, up, secs) in want {
        let (takeoff, air) = clips.jump(k, 0);
        let end = (takeoff.root_motion(takeoff.length) * air.root_motion(air.length)).w_axis;
        let len = takeoff.length + air.length;
        println!("{} {}: {:.2} m ahead {:+.2} m in {len:.3} s", JUMPS[k].0, JUMPS[k].1, end.y, end.z);
        bad |= (end.y - fwd).abs() > 0.15 || (end.z - up).abs() > 0.1 || (len - secs).abs() > 0.04;
    }
    assert!(!bad);
}

/// Running at the 5 cm balustrade round the stairwell south-east of the
/// plaza (x = 38.60, top 1.01 m, 2.36 m clear above): the player jumps
/// up onto it from 2 m, lands centred and stays; then walks along it
/// without falling off.
#[test]
fn thin_railing_landing() {
    let Some(d) = data() else { return };
    let character = Arc::new(acb_game::load_character(&d.archive, "AC2MP_ID07_MainTemplate_Player").unwrap());
    let setup = Setup {
        characters: vec![("AC2MP_ID07_MainTemplate_Player".into(), character)],
        count: Some(1),
        anim: String::new(),
        wander: 0.0,
        play: true,
        crowd: 0,
        pair: Vec::new(),
        at: Some(vec![34.5, -54.38, 0.1, -90.0]),
        builtins: d.builtins.as_ref(),
    };
    let mut w = World::load(&d.archive, &setup).unwrap();
    // forward = (cos yaw, sin yaw): due +x, then along the rail (+y, up
    // to its 2.2 m newel post)
    let run = PlayerInput { forward: true, run: true, cam_yaw: 0.0, ..PlayerInput::default() };
    let mut landed = None;
    let mut took_off = None;
    for i in 0..(3.0 / DT) as usize {
        let flying = w.players[0].flight.is_some();
        w.step(&[if landed.is_none() { run } else { PlayerInput::default() }]);
        let p = &w.players[0];
        if !flying && p.flight.is_some() {
            took_off = Some((i, p.pos, p.flight.as_ref().unwrap().dur));
        }
        if flying && p.flight.is_none() {
            landed = Some((i, p.pos));
        }
        assert!(p.climb.is_none(), "climbed instead of jumping at {:?}", p.pos);
    }
    let (i0, from, dur) = took_off.expect("no jump");
    let (i1, at) = landed.expect("no landing");
    println!("jump from {from:.2?} ({dur:.3} s), landed at {at:.2?} after {} ticks", i1 - i0);
    assert!((at.x - 38.60).abs() < 0.02 && (at.z - 1.01).abs() < 0.02, "{at:?}");
    assert_eq!(i1 - i0, (dur / DT).ceil() as usize);
    let p = &w.players[0];
    assert!(p.pos.distance(at) < 0.01 && !p.loco.airborne(), "did not stay: {:?}", p.pos);
    // walk along it, 7 degrees off its axis: kept on the centre line
    let along = PlayerInput { forward: true, cam_yaw: (90.0f32 - 7.0).to_radians(), ..PlayerInput::default() };
    let y0 = p.pos.y;
    for _ in 0..(2.5 / DT) as usize {
        w.step(&[along]);
    }
    let p = &w.players[0];
    println!("walked to {:.2?}", p.pos);
    assert!(p.pos.y - y0 > 0.3, "did not walk: {:?}", p.pos);
    assert!((p.pos.x - 38.60).abs() < 0.03 && (p.pos.z - 1.01).abs() < 0.02, "fell off: {:?}", p.pos);
}

/// Every thin (< 10 cm) railing top with the body's room over it, run at
/// from 1.4 m on each side over level floor: how many give a landing on
/// the railing (11 of 15 on San Donato).
#[test]
fn thin_railings_sweep() {
    let Some(d) = data() else { return };
    let g = Ground::load(&d.archive).unwrap();
    let clips = Clips::load(&d.archive, d.builtins.as_ref()).unwrap();
    let (mut tried, mut landed, mut covered) = (0, 0, 0);
    let mut seen = std::collections::HashSet::new();
    for t in g.floor_triangles() {
        let lo = t[0].min(t[1]).min(t[2]);
        let hi = t[0].max(t[1]).max(t[2]);
        let ext = (hi - lo).truncate();
        if ext.min_element() > 0.1 || ext.max_element() < 1.0 || hi.z - lo.z > 0.05 {
            continue;
        }
        let c = (lo + hi) * 0.5;
        if !seen.insert(((c.x * 2.0) as i32, (c.y * 2.0) as i32, (c.z * 4.0) as i32)) {
            continue;
        }
        let across = if ext.x < ext.y { Vec2::X } else { Vec2::Y };
        // (from just over the top: a cover 0.2 m over it counts)
        if g.headroom(c - glam::Vec3::Z * 0.25).is_some() {
            covered += 1;
            continue;
        }
        for s in [1.0, -1.0] {
            let start = c.truncate() + across * s * 1.4;
            let Some(z) = g.floor_below(start.extend(c.z - 0.6)) else { continue };
            if !(0.6..1.3).contains(&(c.z - z)) {
                continue;
            }
            // a run-up: level floor, free above, from the start to 0.4 m
            // before the face
            let level = (0..=10).all(|i| {
                let q = start - across * s * (i as f32 * 0.1);
                g.floor_below(q.extend(z + 1.8)).is_some_and(|f| (f - z).abs() < 0.2)
            });
            if !level {
                continue;
            }
            tried += 1;
            // up onto it, or across a gap in front of it (from its edge)
            let hit = jump::target(&g, &clips, start.extend(z), -across * s, false).or_else(|| jump::target(&g, &clips, start.extend(z), -across * s, true));
            // centred on it, or past its edge where it is part of a wider top
            let on = |&(at, _, narrow): &(glam::Vec3, jump::Flight, bool)| {
                (at.z - c.z).abs() < 0.05 && (!narrow || (at.truncate() - c.truncate()).dot(across).abs() < 0.05)
            };
            if hit.as_ref().is_some_and(on) {
                landed += 1;
                println!("landing on {c:.2?} from {:.2?}", start.extend(z));
            } else {
                println!("no landing on {c:.2?} from {:.2?}: {:?}", start.extend(z), hit.map(|h| h.0));
            }
        }
    }
    println!("thin railings: {tried} approaches, {landed} land on the rail, {covered} tops covered");
    // the rest have an arch lintel or clutter over the body's path
    assert!(tried >= 12 && landed * 10 >= tried * 7, "{landed}/{tried}");
}
