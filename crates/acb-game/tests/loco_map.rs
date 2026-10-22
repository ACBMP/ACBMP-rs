//! Ground locomotion against retail: steady-state speeds (retail
//! HumanGround is root-motion driven, its speed table is built from the
//! anim set: `HumanSharedData::AddSpeedTableForAnimSet`, so the speeds are
//! the `xx_` cycles' root speeds) and the heading turn rate (retail
//! `HumanGround::UpdateOrientation` curve). The map part needs the game
//! data (`ACBMP_SCHEMAS`, `ACBMP_MULTI`, `ACBMP_EXE`); skipped otherwise.

use std::path::PathBuf;
use std::sync::Arc;

use acb_anim::BuiltinTracks;
use acb_game::locomotion::{heading_speed, norm_yaw, turn_toward, HEADING_MAX_SPEED, HEADING_MIN_SPEED};
use acb_game::{PlayerInput, Setup, World, DT};

/// Retail heading: 1.5 rad/s up to 10 degrees of error, 6 rad/s from 40,
/// linear between; a 90 degree turn takes 0.26 s + the tail.
#[test]
fn heading_turn_rate() {
    let d = std::f32::consts::PI / 180.0;
    assert_eq!(heading_speed(5.0 * d), HEADING_MIN_SPEED);
    assert_eq!(heading_speed(90.0 * d), HEADING_MAX_SPEED);
    assert!((heading_speed(25.0 * d) - 3.75).abs() < 1e-4);
    assert!((heading_speed(-25.0 * d) - 3.75).abs() < 1e-4);
    // 90 degree turn at 60 Hz: ticks until the facing matches
    let (mut yaw, want) = (0.0f32, 90.0 * d);
    let mut ticks = 0;
    while yaw != norm_yaw(want) {
        yaw = turn_toward(yaw, want, DT);
        ticks += 1;
        assert!(ticks < 120);
    }
    // 50 deg at 6 rad/s (8.3 ticks), 40->10 deg on the ramp, 10 deg at 1.5
    assert!((20..=32).contains(&ticks), "{ticks} ticks");
    // shortest way across +-180, result in (-180, 180]
    let y = turn_toward(170.0 * d, -170.0 * d, DT);
    assert!(y > 170.0 * d && y <= std::f32::consts::PI, "{}", y.to_degrees());
    assert_eq!(norm_yaw(-std::f32::consts::PI), std::f32::consts::PI);
    assert!((norm_yaw(-258.0 * d).to_degrees() - 102.0).abs() < 1e-3);
}

fn world() -> Option<(acb_world::Archive<'static>, Option<BuiltinTracks>)> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let exe = PathBuf::from(std::env::var_os("ACBMP_EXE")?);
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).ok()?));
    let archive = acb_world::open_map(&multi.join("DataPC_AC2MP_SanDonato.forge"), schema).ok()?;
    let builtins = BuiltinTracks::from_executable(&std::fs::read(exe).ok()?);
    Some((archive, builtins))
}

/// Steady speed (m/s) holding `input` for 3 s, measured over the last 1 s.
fn steady(archive: &acb_world::Archive, builtins: Option<&BuiltinTracks>, input: PlayerInput, at: Option<[f32; 4]>) -> f32 {
    let character = Arc::new(acb_game::load_character(archive, "AC2MP_ID07_MainTemplate_Player").unwrap());
    let setup = Setup {
        characters: vec![("AC2MP_ID07_MainTemplate_Player".into(), character)],
        count: Some(1),
        anim: String::new(),
        wander: 0.0,
        play: true,
        crowd: 0,
        pair: Vec::new(),
        at: at.map(|a| a.to_vec()),
        builtins,
    };
    let mut w = World::load(archive, &setup).unwrap();
    let n = (3.0 / DT) as usize;
    let mut from = None;
    for i in 0..n {
        w.step(&[input]);
        if i + 1 == n - 60 {
            from = Some(w.players[0].pos);
        }
    }
    (w.players[0].pos - from.unwrap()).truncate().length()
}

/// Walk 1.90, run 5.12, sprint 6.28 m/s: the cycles' root speeds.
#[test]
fn steady_speeds() {
    let Some((archive, builtins)) = world() else { return };
    // the San Donato spawn plaza, running +x across it
    let at = None;
    let base = PlayerInput { forward: true, cam_yaw: 0.0, ..PlayerInput::default() };
    let walk = steady(&archive, builtins.as_ref(), base, at);
    let run = steady(&archive, builtins.as_ref(), PlayerInput { run: true, ..base }, at);
    let sprint = steady(&archive, builtins.as_ref(), PlayerInput { run: true, sprint: true, ..base }, at);
    eprintln!("walk {walk:.3} run {run:.3} sprint {sprint:.3}");
    assert!((walk - 1.90).abs() < 0.05, "walk {walk}");
    assert!((run - 5.12).abs() < 0.08, "run {run}");
    assert!((sprint - 6.28).abs() < 0.08, "sprint {sprint}");
}
