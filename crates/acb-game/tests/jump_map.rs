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
        // Legs (sprint) tapped for one tick at 0.3 s: the jump needs it
        // pressed within 0.5 s (retail), as tools/gates.sh `railing`
        let legs = PlayerInput { sprint: i == (0.3 / DT) as usize, ..run };
        w.step(&[if landed.is_none() { legs } else { PlayerInput::default() }]);
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

/// The beam set (retail `HumanNarrowObjectBeam` clips) loads; its cycles'
/// root speeds are the retail speeds on a beam.
#[test]
fn beam_clips() {
    let Some(d) = data() else { return };
    let clips = Clips::load(&d.archive, d.builtins.as_ref()).unwrap();
    let b = clips.beam().expect("beam clips");
    let speed = |c: &acb_anim::pose::Clip| c.root_motion(c.length).w_axis.truncate().length() / c.length;
    let yaw = |c: &acb_anim::pose::Clip| {
        let (_, r, _) = c.root_motion(c.length).to_scale_rotation_translation();
        r.to_euler(glam::EulerRot::ZXY).0.to_degrees()
    };
    println!("beam walk {:.2} m/s ({:.3} s), jog {:.2} ({:.3} s), sprint {:.2} ({:.3} s)", speed(&b.walk), b.walk.length, speed(&b.run), b.run.length, speed(&b.sprint), b.sprint.length);
    for (n, c) in [("land", &b.land)] {
        let m = c.root_motion(c.length).w_axis;
        println!("{n}: {:.3} s, root {:+.2} {:+.2} {:+.2}, yaw {:+.0}", c.length, m.x, m.y, m.z, yaw(c));
    }
    assert!(speed(&b.walk) < speed(&b.run) && speed(&b.run) < speed(&b.sprint));
}

/// Jumps to a hang (retail TargetedLedgeGrab): from the floor 3 m out in
/// front of San Donato's wall edges, facing them, some edges are jump
/// targets; every one found carries its hold, faces it and ends with the
/// hands in the catch band (1.3-2.2 m over the feet) in reach.
#[test]
fn hang_targets() {
    let Some(d) = data() else { return };
    let g = Ground::load(&d.archive).unwrap();
    let clips = Clips::load(&d.archive, d.builtins.as_ref()).unwrap();
    let (mut tried, mut found) = (0, 0);
    for h in g.holds_around(glam::Vec3::ZERO, 200.0) {
        let mid = (h.a + h.b) * 0.5;
        let start = mid.truncate() + h.wall * 3.0;
        let Some(z) = g.floor_below(start.extend(mid.z)) else { continue };
        if mid.z - z < 1.5 || mid.z - z > 3.5 {
            continue;
        }
        tried += 1;
        let pos = start.extend(z);
        let Some((feet, flight, _)) = jump::hang_target(&g, &clips, pos, -h.wall) else { continue };
        found += 1;
        // a top within a jump's rise is landed on instead
        let Some((hold, q)) = flight.grab else {
            let floor = g.floor_below(feet + glam::Vec3::Z * 0.3).expect("a floor under the landing");
            assert!((floor - feet.z).abs() < 0.15 && g.headroom(feet).is_none(), "landing at {feet:?}");
            continue;
        };
        assert!(hold.wall.dot(-h.wall) < -0.5, "faces the jumper");
        let up = q.z - feet.z;
        assert!((1.3..=2.2).contains(&up), "hands {up:.2} m over the feet");
        assert!(g.catch_hold(feet, -hold.wall, acb_game::human::in_air::GRASP_PROBE).is_some(), "catchable at {feet:?}");
    }
    eprintln!("hang targets: {found} of {tried} wall edges");
    assert!(tried > 0 && found > 0, "{found} of {tried}");
}

fn player_at(d: &Data, at: [f32; 4]) -> World {
    let character = Arc::new(acb_game::load_character(&d.archive, "AC2MP_ID07_MainTemplate_Player").unwrap());
    let setup = Setup {
        characters: vec![("AC2MP_ID07_MainTemplate_Player".into(), character)],
        count: Some(1),
        anim: String::new(),
        wander: 0.0,
        play: true,
        crowd: 0,
        pair: Vec::new(),
        at: Some(at.to_vec()),
        builtins: d.builtins.as_ref(),
    };
    World::load(&d.archive, &setup).unwrap()
}

/// Standing on the open plaza (facing +x) and setting off backwards or
/// sideways: high profile turns in 0.2 s and is moving 0.45 s after the
/// press, low profile walks out of its 0.4 s turn (no turn on the spot
/// plus settle into the wait first).
#[test]
fn turning_starts_move_at_once() {
    let Some(d) = data() else { return };
    let mut w = player_at(&d, [40.0, 20.4, 0.1, -90.0]);
    let saved = w.save();
    for (input, gait) in [
        (PlayerInput { back: true, sprint: true, ..PlayerInput::default() }, "sprint"),
        (PlayerInput { left: true, run: true, ..PlayerInput::default() }, "run"),
        (PlayerInput { back: true, ..PlayerInput::default() }, "walk"),
    ] {
        w.restore(&saved);
        let from = w.players[0].pos;
        let mut moved_at = None;
        for i in 0..(1.5 / DT) as usize {
            w.step(&[input]);
            if moved_at.is_none() && w.players[0].pos.distance(from) > 0.05 {
                moved_at = Some(i as f32 * DT);
            }
        }
        let t = moved_at.unwrap_or(f32::INFINITY);
        println!("{gait}: moving after {t:.2} s, at {:.2?}", w.players[0].pos);
        assert!(t < if gait == "walk" { 0.55 } else { 0.45 }, "{gait}: moving only after {t} s");
    }
}

/// Off the 6.3 m roof east of the first spawn (edge at x = -13.1, 3.9 m
/// down to the next roof, no jump target in reach): freerunning (Legs held)
/// a leap for the farthest foothold, 8 m out and 3 m down, on an arc that
/// goes on past that height (`jump::leap`); just running, retail FallOffEdge
/// (00fd2406): a targeted jump to 1 + 2 * anim speed metres out (run 2.6 m;
/// low profile 1 m, but low profile stops at a drop this deep), 3 m down,
/// past that height straight down, with no rise (ours: the ballistic part
/// starts at the edge).
#[test]
fn fall_off_an_edge_by_profile() {
    let Some(d) = data() else { return };
    let west = std::f32::consts::PI;
    let sprint = PlayerInput { forward: true, run: true, sprint: true, cam_yaw: west, ..PlayerInput::default() };
    let run = PlayerInput { forward: true, run: true, cam_yaw: west, ..PlayerInput::default() };
    for (input, want, rise, name) in [(sprint, 8.0..9.5, 0.3..0.6, "sprint"), (run, 2.5..2.7, -0.01..0.01, "run")] {
        let mut w = player_at(&d, [-8.0, 43.5, 6.5, 90.0]);
        let mut left_at = None;
        let mut peak = f32::MIN;
        for _ in 0..(4.0 / DT) as usize {
            w.step(&[input]);
            let p = &w.players[0];
            if p.loco.airborne() && left_at.is_none() {
                left_at = Some(p.pos);
            }
            if left_at.is_some() && p.loco.airborne() {
                peak = peak.max(p.pos.z);
            }
            if left_at.is_some() && !p.loco.airborne() {
                break;
            }
        }
        let from = left_at.unwrap_or_else(|| panic!("{name}: never left the roof"));
        let p = &w.players[0];
        println!("{name}: left at {from:.2?}, peak +{:.2} m, landed {:.2?}", peak - from.z, p.pos);
        assert!(!p.loco.airborne(), "{name}: still in the air at {:?}", p.pos);
        assert!(rise.contains(&(peak - from.z)), "{name}: rose {:.2} m", peak - from.z);
        let out = from.x - p.pos.x;
        assert!(want.contains(&out), "{name}: landed {out:.2} m out, want {want:?}");
    }
}

/// Walking (low profile) to the end of the stairwell balustrade stops
/// there; F then steps off it and drops to the floor 0.94 m below.
#[test]
fn jump_button_steps_off_a_beam_end() {
    let Some(d) = data() else { return };
    let mut w = player_at(&d, [38.60, -54.6, 1.1, 180.0]);
    let walk = PlayerInput { forward: true, cam_yaw: -std::f32::consts::FRAC_PI_2, ..PlayerInput::default() };
    for _ in 0..(2.0 / DT) as usize {
        w.step(&[walk]);
    }
    let p = &w.players[0];
    assert!((p.pos.z - 1.01).abs() < 0.05 && !p.loco.airborne(), "not stopped on the rail end: {:?}", p.pos);
    for _ in 0..(0.5 / DT) as usize {
        w.step(&[PlayerInput::default()]);
    }
    w.step(&[PlayerInput { jump: true, ..PlayerInput::default() }]);
    for _ in 0..(1.5 / DT) as usize {
        w.step(&[PlayerInput::default()]);
    }
    let p = &w.players[0];
    println!("after F: {:.2?}", p.pos);
    assert!(p.pos.z < 0.2 && !p.loco.airborne() && p.climb.is_none(), "did not step off: {:?}", p.pos);
}

/// A rebound (S+F on a wall) with nothing in jump reach behind: retail aims
/// at the farthest foothold, 8 m straight out from the wall and 3 m down
/// (`Human::GetFarthestFootHoldInDirection` 00d14144), then drops. From
/// the 10 m hang on the west tower (wall facing -y) that lands 8.0-8.4 m
/// out (gate `rebound`; gate `eject` covers a low hang, whose target is
/// the floor at the wall's foot).
#[test]
fn rebound_targets() {
    let Some(d) = data() else { return };
    let jump = PlayerInput { jump: true, back: true, ..PlayerInput::default() };
    let mut w = player_at(&d, [-46.81, 21.36, 10.6, 0.0]);
    for _ in 0..(1.5 / DT) as usize {
        w.step(&[PlayerInput::default()]);
    }
    assert!(w.players[0].climb.is_some(), "not hanging: {:?}", w.players[0].pos);
    let start = w.players[0].pos;
    w.step(&[jump]);
    for _ in 0..(4.0 / DT) as usize {
        w.step(&[PlayerInput::default()]);
    }
    let p = &w.players[0];
    let out = start.y - p.pos.y;
    println!("tower: from {start:.2?} landed {:.2?}, {out:.2} m out", p.pos);
    assert!(!p.loco.airborne() && p.climb.is_none(), "not landed: {:?}", p.pos);
    assert!((8.0..8.4).contains(&out), "landed {out:.2} m out");
}

/// A run and jump at a guidance pole (San Donato's roof 14.81 -1.06, the pole
/// over the alley at 6.28 m) goes into the swing, and the stick held forward
/// flies on through it across the alley.
#[test]
fn swinging_on_a_pole() {
    let Some(d) = data() else { return };
    let north = std::f32::consts::FRAC_PI_2;
    let run = PlayerInput { forward: true, run: true, sprint: true, cam_yaw: north, ..PlayerInput::default() };
    // let go of everything in the air: it stays swinging, then dies down
    let mut w = player_at(&d, [14.81, -1.06, 3.59, 0.0]);
    let mut swung = false;
    for i in 0..(5.0 / DT) as usize {
        w.step(&[if i < (0.4 / DT) as usize { run } else { PlayerInput::default() }]);
        swung |= w.players[0].trace_state().0 == "swing";
    }
    let p = &w.players[0];
    assert!(swung, "never swung, ended at {:?}", p.pos);
    assert_eq!(p.trace_state().0, "climb", "not hanging after the swing: {:?}", p.pos);
    assert!((p.pos.y - 2.98).abs() < 0.1, "hanging at {:?}", p.pos);
    // the stick held: straight through, on past the pole
    let mut w = player_at(&d, [14.81, -1.06, 3.59, 0.0]);
    let mut swung = false;
    for _ in 0..(1.2 / DT) as usize {
        w.step(&[run]);
        swung |= w.players[0].trace_state().0 == "swing";
    }
    let p = &w.players[0];
    assert!(swung && p.climb.is_none() && p.pos.y > 3.5, "did not fly on: {:?}", p.pos);
}

/// The maps's poles are there, and edges that are no holds are not: the foot
/// of a wall (a fountain's, on San Donato) marks an edge, nobody hangs from it.
#[test]
fn only_real_edges_are_holds() {
    let Some(d) = data() else { return };
    let g = Ground::load(&d.archive).unwrap();
    let holds = g.holds_around(glam::Vec3::ZERO, 400.0);
    assert!(holds.iter().filter(|h| h.bar).count() >= 24);
    // edges at a wall's foot that the guidance marks, with the wall's normal
    for (at, n) in [([17.89, 12.34, 0.02], [0.98, 0.18]), ([17.37, 13.05, 0.02], [0.33, 0.95]), ([16.68, 12.92, 0.02], [-0.71, 0.70])] {
        let at = glam::Vec3::from(at);
        let n = glam::Vec2::from(n);
        let near: Vec<_> = g.holds_around(at, 0.3).into_iter().filter(|h| h.wall.dot(n) > 0.9).collect();
        assert!(near.is_empty(), "hold at the foot of a wall: {near:?}");
    }
    for h in holds.iter().filter(|h| !h.bar) {
        assert!([0.5, 0.25, 0.75].iter().any(|&t| g.hold_fault(h.at(t), h.wall).is_none()), "hold {h:?}");
    }
}

/// From the first tick a stick is held the character moves at its gait's
/// speed, facing the stick, and stops the tick it is released.
#[test]
fn gait_changes_are_instant() {
    let Some(d) = data() else { return };
    let mut w = player_at(&d, [-18.0, 44.6, 2.22, 90.0]);
    for _ in 0..30 {
        w.step(&[PlayerInput::default()]);
    }
    let east = 0.0;
    let run = PlayerInput { forward: true, run: true, cam_yaw: east, ..PlayerInput::default() };
    let from = w.players[0].pos;
    w.step(&[run]);
    let p = &w.players[0];
    assert_eq!(p.loco.speed, 2.0);
    assert!((p.yaw + std::f32::consts::FRAC_PI_2).abs() < 1e-3, "facing {}", p.yaw);
    assert!((p.pos - from).truncate().length() > 0.05, "moved {:?}", p.pos - from);
    for _ in 0..10 {
        w.step(&[run]);
    }
    w.step(&[PlayerInput::default()]);
    assert_eq!(w.players[0].loco.speed, 0.0);
    let at = w.players[0].pos;
    w.step(&[PlayerInput::default()]);
    assert_eq!(w.players[0].pos, at, "still moving after the release");
}

/// Standing against a wall, Shift+W climbs on the next tick, with no time to
/// charge up, and a wall well off square climbs just the same.
#[test]
fn climbing_starts_at_once() {
    let Some(d) = data() else { return };
    let g = Ground::load(&d.archive).unwrap();
    // the same point on the wall from further and further round
    let spot = glam::Vec2::new(-13.12, 44.2);
    for (deg, ok) in [(0.0f32, true), (-30.0, true), (-55.0, true), (-75.0, false)] {
        let push = glam::Vec2::from_angle(deg.to_radians()).rotate(glam::Vec2::X);
        let feet = (spot - push * 1.0).extend(2.22);
        assert_eq!(g.climb_wall(feet, push, push).is_some(), ok, "{deg} deg off square");
    }
    let mut w = player_at(&d, [-14.3, 44.2, 2.22, -90.0]);
    for _ in 0..30 {
        w.step(&[PlayerInput::default()]);
    }
    w.step(&[PlayerInput { forward: true, run: true, cam_yaw: 0.0, ..PlayerInput::default() }]);
    assert!(w.players[0].climb.is_some(), "still on foot at {:?}", w.players[0].pos);
}

/// The drop button lets go of a hang, at once and past every hold on the way
/// down; on foot it works from a stand at an edge whichever way it faces.
#[test]
fn the_drop_button_drops() {
    let Some(d) = data() else { return };
    let drop = PlayerInput { jump: true, ..PlayerInput::default() };
    // wall run to the hang on the roof wall east of the first spawn
    let mut w = player_at(&d, [-14.3, 44.2, 2.22, -90.0]);
    for _ in 0..(0.4 / DT) as usize {
        w.step(&[PlayerInput { forward: true, run: true, cam_yaw: 0.0, ..PlayerInput::default() }]);
    }
    for _ in 0..(1.0 / DT) as usize {
        w.step(&[PlayerInput::default()]);
    }
    assert!(w.players[0].climb.is_some(), "not hanging: {:?}", w.players[0].pos);
    w.step(&[drop]);
    assert!(w.players[0].climb.is_none(), "still on the wall");
    for _ in 0..(2.0 / DT) as usize {
        w.step(&[PlayerInput::default()]);
    }
    let p = &w.players[0];
    assert!(p.climb.is_none() && !p.loco.airborne() && p.pos.z < 2.5, "ended at {:?}", p.pos);
    // on the roof edge, not having walked into it, facing along it
    let mut w = player_at(&d, [-12.95, 43.99, 6.5, 0.0]);
    for _ in 0..(0.5 / DT) as usize {
        w.step(&[PlayerInput::default()]);
    }
    assert!(w.players[0].climb.is_none() && !w.players[0].loco.airborne());
    w.step(&[drop]);
    let p = &w.players[0];
    assert!(p.climb.is_some() || p.loco.airborne(), "nothing happened at {:?}", p.pos);
}

/// Wall Runner (retail `HumanWalling::UpdateHumanWallingMain`, AC RE Memory
/// `acbmp_wallrunner_investigation`): a top edge 4.5-6 m over the feet, out of a
/// plain wall run's reach, is grabbed with the perk through the long step.
#[test]
fn wall_runner_reaches_higher() {
    use acb_game::climb::{yaw_into, WALL_RUNNER_SCAN};
    let Some(d) = data() else { return };
    let w = player_at(&d, [0.0, 0.0, 0.0, 0.0]);
    let g = w.ground.as_ref().unwrap();
    // a wall with a top 4.5-6 m up and floor 1 m in front of it, where the
    // perk takes the long step (nothing in the low scan; a plain run may
    // still have a lower edge, which retail then skips too)
    let spot = g
        .holds_around(glam::Vec3::ZERO, 300.0)
        .into_iter()
        .filter(|h| h.top && (h.b - h.a).length() > 1.5)
        .find_map(|h| {
            let q = (h.a + h.b) * 0.5;
            let out = (q.truncate() + h.wall * 1.0).extend(q.z);
            let f = g.floor_below(out)?;
            let feet = out.with_z(f);
            let dz = q.z - f;
            if !(WALLRUNNER_MARGIN + WALL_RUNNER_SCAN.0..WALL_RUNNER_SCAN.1 - WALLRUNNER_MARGIN).contains(&dz) {
                return None;
            }
            let (hit, n) = g.wall_ahead(feet, -h.wall, -h.wall * 5.0)?;
            let plain = g.wall_run_hold(hit, n, f, false).map(|t| t.1.z);
            let long = g.wall_run_hold(hit, n, f, true).filter(|t| t.2)?;
            ((long.1.z - q.z).abs() < 0.3 && plain.is_none_or(|z| z < q.z - 0.5)).then_some((feet, h.wall, q.z))
        });
    let (feet, n, top) = spot.expect("no wall only the perk reaches");
    let yaw = yaw_into(n).to_degrees();
    eprintln!("wall runner spot {:.2} {:.2} {:.2} yaw {yaw:.0}, top {top:.2}", feet.x, feet.y, feet.z);
    let run = |perk: bool| {
        let mut w = player_at(&d, [feet.x, feet.y, feet.z, yaw]);
        w.players[0].wall_runner = perk;
        let go = PlayerInput { forward: true, run: true, cam_yaw: (-n).y.atan2((-n).x), ..PlayerInput::default() };
        for _ in 0..(1.0 / DT) as usize {
            w.step(&[go]);
        }
        for _ in 0..(2.0 / DT) as usize {
            w.step(&[PlayerInput::default()]);
        }
        let p = &w.players[0];
        (p.climb.as_ref().map(|c| c.hold.a.z.max(c.hold.b.z)), p.pos)
    };
    let (with, at) = run(true);
    assert!(with.is_some_and(|z| (z - top).abs() < 0.3), "perk: hold {with:?} at {at:?}, top {top} from {feet:?}");
    let (without, at) = run(false);
    assert!(without.is_none_or(|z| z < top - 0.3), "no perk: hold {without:?} at {at:?}");
}

/// Keeps the probe away from the scan's edges, metres.
const WALLRUNNER_MARGIN: f32 = 0.2;
