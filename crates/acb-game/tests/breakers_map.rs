//! Chase breakers on San Donato with the retail data: a door shuts behind
//! a runner, a platform falls under whoever stands on it, a lift takes a
//! runner up and reloads. Needs the game data (`ACBMP_SCHEMAS`,
//! `ACBMP_MULTI`, `ACBMP_EXE`); skipped otherwise.

use std::path::PathBuf;

use acb_anim::BuiltinTracks;
use acb_game::{PlayerInput, Setup, World, TICK_HZ};
use glam::{Vec2, Vec3};

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

/// One player at `at` (x,y,z,yaw).
fn world(d: &Data, at: &[f32]) -> World {
    let character = std::sync::Arc::new(acb_game::load_character(&d.archive, "AC2MP_ID07_MainTemplate_Player").unwrap());
    let setup = Setup {
        characters: vec![("AC2MP_ID07_MainTemplate_Player".into(), character)],
        count: Some(1),
        anim: "xx_l_wait_hipm_footl".into(),
        wander: 0.0,
        play: true,
        crowd: 0,
        pair: Vec::new(),
        at: Some(at.to_vec()),
        builtins: d.builtins.as_ref(),
    };
    World::load(&d.archive, &setup).unwrap()
}

/// Moving along `dir` (world).
fn go(dir: Vec2, run: bool) -> PlayerInput {
    PlayerInput { forward: true, run, cam_yaw: dir.y.atan2(dir.x), ..PlayerInput::default() }
}

fn secs(s: f32) -> u64 {
    (s * TICK_HZ as f32).round() as u64
}

/// The double door `AC2MP_GEN_Chasebreaker_door_34_Group_001` at
/// -18,-13.47: its blocker spans y -14.55..-12.40 on the plane x = -18.
const DOOR: Vec3 = Vec3::new(-18.0, -13.47, 0.0);

/// Is the doorway shut (something between a point 1 m before and 1 m
/// behind the door, chest high)?
fn shut(w: &World) -> bool {
    w.ground.as_ref().unwrap().ray(DOOR + Vec3::new(-1.0, 0.0, 1.2), Vec3::X * 2.0).is_some()
}

#[test]
fn a_door_shuts_behind_a_runner() {
    let Some(d) = data() else { return };
    // walking through leaves it open
    let mut w = world(&d, &[DOOR.x - 2.5, DOOR.y, DOOR.z, -90.0]);
    assert!(!shut(&w), "the door starts open");
    for _ in 0..secs(8.0) {
        w.step(&[go(Vec2::X, false)]);
        assert!(!shut(&w), "shut on a walker at {:?}", w.players[0].pos);
        if w.players[0].pos.x > DOOR.x + 2.0 {
            break;
        }
    }
    assert!(w.players[0].pos.x > DOOR.x + 2.0, "the walker never got through: {:?}", w.players[0].pos);
    // running through shuts it for 3.5 s
    let mut w = world(&d, &[DOOR.x - 2.5, DOOR.y, DOOR.z, -90.0]);
    let mut at = None;
    for _ in 0..secs(3.0) {
        w.step(&[go(Vec2::X, true)]);
        if shut(&w) {
            at = Some(w.tick);
            break;
        }
    }
    let at = at.expect("the door never shut");
    let x = w.players[0].pos.x;
    assert!(x > DOOR.x + 0.3 && x < DOOR.x + 1.0, "shut with the runner at {x}");
    // the runner turns back at once: the door holds
    let mut open = None;
    for _ in 0..secs(5.0) {
        w.step(&[go(-Vec2::X, true)]);
        if !shut(&w) {
            open = Some(w.tick);
            break;
        }
        assert!(w.players[0].pos.x > DOOR.x, "through the shut door: {:?}", w.players[0].pos);
    }
    assert_eq!(open.expect("the door never opened again") - at, secs(3.5));
    // and lets the runner back through
    for _ in 0..secs(2.0) {
        w.step(&[go(-Vec2::X, true)]);
    }
    assert!(w.players[0].pos.x < DOOR.x - 1.0, "not back through: {:?}", w.players[0].pos);
}

/// The hanging platform `AC2MP_GEN_Platform_chasebreaker_01A_Grp_001`.
const PLATFORM: Vec3 = Vec3::new(-38.16, -2.55, 3.04);

#[test]
fn a_platform_falls_under_its_load() {
    let Some(d) = data() else { return };
    let mut w = world(&d, &[PLATFORM.x, PLATFORM.y, PLATFORM.z, 0.0]);
    let there = |w: &World| w.ground.as_ref().unwrap().floor_below(PLATFORM + Vec3::Z).is_some_and(|z| (z - PLATFORM.z).abs() < 0.1);
    assert!(there(&w));
    assert!((w.players[0].pos.z - PLATFORM.z).abs() < 0.1, "not on the platform: {:?}", w.players[0].pos);
    // its edges are holds
    let holds = |w: &World| -> Vec<_> { w.ground.as_ref().unwrap().holds_around(PLATFORM, 2.5).into_iter().filter(|h| (h.a.z - PLATFORM.z).abs() < 0.2).collect() };
    let edges = holds(&w);
    assert!(edges.len() >= 4, "{} holds", edges.len());
    // it goes 0.8 to 1.0 s after someone is on it
    let mut gone = None;
    for _ in 0..secs(1.5) {
        w.step(&[PlayerInput::default()]);
        if !there(&w) {
            gone = Some(w.tick);
            break;
        }
    }
    let gone = gone.expect("the platform never fell");
    // its holds go with it (whoever hangs on one falls)
    let g = w.ground.as_ref().unwrap();
    assert!(holds(&w).is_empty());
    assert!(edges.iter().all(|h| g.hold_gone(h)));
    assert!((secs(0.8)..=secs(1.0) + 1).contains(&gone), "fell after {gone} ticks");
    // whoever stood on it lands below
    for _ in 0..secs(2.0) {
        w.step(&[PlayerInput::default()]);
    }
    let p = &w.players[0];
    assert!(p.pos.z < 1.0 && !p.loco.airborne(), "not down: {:?}", p.pos);
    // back after 4.5 s
    let mut back = None;
    for _ in 0..secs(4.0) {
        w.step(&[PlayerInput::default()]);
        if there(&w) {
            back = Some(w.tick);
            break;
        }
    }
    assert_eq!(back.expect("the platform never came back") - gone, secs(4.5));
    let g = w.ground.as_ref().unwrap();
    assert_eq!(holds(&w).len(), edges.len());
    assert!(!edges.iter().any(|h| g.hold_gone(h)));
}

#[test]
fn a_lift_takes_a_runner_and_reloads() {
    let Some(d) = data() else { return };
    // the 8 m lift at 13.70,12.11: its start point is 15.49,12.09, facing -X
    let mut w = world(&d, &[18.5, 12.09, 0.0, 90.0]);
    let lift = &w.breakers.as_ref().unwrap().lifts[0];
    assert!((lift.start - Vec3::new(15.49, 12.09, 0.0)).length() < 0.05, "{:?}", lift.start);
    // up 1.600 s + rewind 4.000 s
    assert_eq!(lift.reload as u64, secs(5.6));
    let start = lift.start;
    // walking past does not ride
    for _ in 0..secs(3.0) {
        w.step(&[go(-Vec2::X, false)]);
        assert!(w.players[0].lift.is_none(), "took a walker");
    }
    let mut w = world(&d, &[18.5, 12.09, 0.0, 90.0]);
    let mut on = None;
    for _ in 0..secs(2.0) {
        w.step(&[go(-Vec2::X, true)]);
        if w.players[0].lift.is_some() {
            on = Some(w.tick);
            break;
        }
    }
    let on = on.unwrap_or_else(|| panic!("never taken: at {:?}", w.players[0].pos));
    assert_eq!(w.breaker_state.lifts[0] as u64, secs(5.6));
    // unkillable on the way up
    assert_eq!(w.players[0].actor().stance, acb_game::rules::Stance::Lift);
    let mut off = None;
    for _ in 0..secs(3.0) {
        w.step(&[go(-Vec2::X, true)]);
        if w.players[0].lift.is_none() {
            off = Some(w.tick);
            break;
        }
    }
    // onto the rope 0.567 s, up 0.433 s, off 0.733 s
    let off = off.expect("never off the lift");
    assert!((off - on).abs_diff(secs(1.733)) <= 2, "rode {} ticks", off - on);
    let p = &w.players[0];
    assert!((p.pos.z - start.z - 8.61).abs() < 0.1, "off at {:?}", p.pos);
    // lands on the roof
    for _ in 0..secs(1.5) {
        w.step(&[PlayerInput::default()]);
    }
    let p = &w.players[0];
    assert!(p.pos.z > start.z + 7.0 && !p.loco.airborne(), "not on the roof: {:?}", p.pos);
    // the lift is out until its rewind is over
    let b = w.breakers.as_ref().unwrap();
    assert_eq!(b.lift_for(&w.breaker_state, start, -Vec2::X), None);
    while w.tick < on + secs(5.6) - 1 {
        w.step(&[PlayerInput::default()]);
    }
    let b = w.breakers.as_ref().unwrap();
    assert_eq!(b.lift_for(&w.breaker_state, start, -Vec2::X), None);
    w.step(&[PlayerInput::default()]);
    let b = w.breakers.as_ref().unwrap();
    assert_eq!(b.lift_for(&w.breaker_state, start, -Vec2::X), Some(0));
}

/// A client replays ticks from a saved state: the breakers come back with
/// it, collision included.
#[test]
fn breakers_restore_with_the_world() {
    let Some(d) = data() else { return };
    let mut w = world(&d, &[DOOR.x - 2.5, DOOR.y, DOOR.z, -90.0]);
    let before = w.save();
    while !shut(&w) {
        w.step(&[go(Vec2::X, true)]);
        assert!(w.tick < secs(3.0));
    }
    let after = w.save();
    w.restore(&before);
    assert!(!shut(&w));
    w.restore(&after);
    assert!(shut(&w));
    for _ in 0..secs(3.5) {
        w.step(&[PlayerInput::default()]);
    }
    assert!(!shut(&w));
}
