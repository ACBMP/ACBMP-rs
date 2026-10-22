//! Kill variants on San Donato with the retail clips. Needs the game data
//! (`ACBMP_SCHEMAS`, `ACBMP_MULTI`, `ACBMP_EXE`); skipped otherwise.

use std::path::PathBuf;

use acb_anim::BuiltinTracks;
use acb_game::kill::{self, KillClips};
use acb_game::{PlayerInput, Setup, World};
use glam::Vec3;

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

/// Every kind loads its own pair for ID07 (no fallback to the walking one).
#[test]
fn kill_sets_load() {
    let Some(d) = data() else { return };
    let k = KillClips::load(&d.archive, d.builtins.as_ref(), "AC2MP_ID07_MainTemplate_Player").unwrap();
    assert_eq!(k.sets.len(), 18);
    for (i, (a, v)) in k.sets.iter().enumerate() {
        let r = |c: &acb_anim::pose::Clip, t: f32| kill::reference(c, t).map(|(p, y)| format!("({:.2},{:.2},{:.2}) {:.0}", p.x, p.y, p.z, y.to_degrees()));
        eprintln!(
            "set {i}: ass {:.2}s {:?} -> {:?} | victim {:.2}s {:?} -> {:?}",
            a.length,
            r(a, 0.0),
            r(a, a.length),
            v.length,
            r(v, 0.0),
            r(v, v.length)
        );
    }
    let start = |set: usize| kill::reference(&k.sets[set].0, 0.0).unwrap().0;
    for kind in [kill::RUN, kill::AIR, kill::DROP, kill::HAY_GRAB, kill::TO_HAY] {
        assert!(start(kind).distance(start(kill::WALK)) > 0.05, "kind {kind} fell back");
    }
}

/// Two players on the plaza by the duel spawns (free play).
fn world(d: &Data) -> World {
    let character = std::sync::Arc::new(acb_game::load_character(&d.archive, "AC2MP_ID07_MainTemplate_Player").unwrap());
    let setup = Setup {
        characters: vec![("AC2MP_ID07_MainTemplate_Player".into(), character)],
        count: Some(2),
        anim: "xx_l_wait_hipm_footl".into(),
        wander: 0.0,
        play: true,
        crowd: 0,
        pair: Vec::new(),
        at: None,
        builtins: d.builtins.as_ref(),
    };
    let mut w = World::load(&d.archive, &setup).unwrap();
    w.add_players(2);
    w
}

/// Steps until player 0 starts a kill (pressing kill from `press` on, with
/// `input` otherwise); returns the kill set and the tick.
fn run_until_kill(w: &mut World, ticks: u32, press: impl Fn(&World) -> bool, input: PlayerInput) -> Option<(usize, u64)> {
    for _ in 0..ticks {
        let kill = press(w);
        w.step(&[PlayerInput { kill, ..input }, PlayerInput::default()]);
        if let Some(k) = &w.players[0].kill {
            return Some((k.set(), w.tick));
        }
    }
    None
}

/// Running at a standing victim: a running kill (no walk-in) inside 2.5 m.
#[test]
fn running_kill() {
    let Some(d) = data() else { return };
    let mut w = world(&d);
    w.place_player(0, Vec3::new(31.0, 20.4, 0.02), -90f32.to_radians());
    w.place_player(1, Vec3::new(41.0, 20.4, 0.02), -90f32.to_radians());
    let run = PlayerInput { forward: true, run: true, cam_yaw: 0.0, ..Default::default() };
    let near = |w: &World| w.players[0].pos.distance(w.players[1].pos) < 2.4 && w.players[0].moved >= acb_game::rules::RUN_SPEED;
    let (set, tick) = run_until_kill(&mut w, 400, near, run).expect("no kill");
    eprintln!("running kill: set {set} at tick {tick}");
    assert_eq!(set & !1, kill::RUN);
    assert!(w.players[0].approach.is_none());
    // the lunge, then the clip; the victim goes down
    for _ in 0..240 {
        w.step(&[PlayerInput::default(), PlayerInput::default()]);
    }
    assert!(w.players[0].kill.is_none());
    assert!(w.players[0].pos.distance(Vec3::new(41.0, 20.4, w.players[0].pos.z)) < 2.0, "{:?}", w.players[0].pos);
}

/// Falling from 4 m onto a victim 2 m ahead: an air kill, landing on it.
#[test]
fn air_kill() {
    let Some(d) = data() else { return };
    let mut w = world(&d);
    w.place_player(0, Vec3::new(37.0, 20.4, 0.02), -90f32.to_radians());
    w.place_player(1, Vec3::new(39.5, 20.4, 0.02), -90f32.to_radians());
    let ground = w.players[1].pos.z;
    {
        let p = &mut w.players[0];
        p.pos.z += 4.0;
        p.prev_pos = p.pos;
        p.fall_from = p.pos.z;
        p.loco.start_fall();
    }
    let (set, tick) = run_until_kill(&mut w, 60, |_| true, PlayerInput::default()).expect("no air kill");
    eprintln!("air kill: set {set} at tick {tick} z {:.2}", w.players[0].pos.z);
    assert_eq!(set & !1, kill::AIR);
    for _ in 0..240 {
        w.step(&[PlayerInput::default(), PlayerInput::default()]);
    }
    let p = &w.players[0];
    assert!(p.kill.is_none() && (p.pos.z - ground).abs() < 0.1, "{:?}", p.pos);
    assert!(p.pos.distance(w.players[1].pos) < 2.0);
}

/// Lag compensation: a victim that walked out of reach 150 ms ago is
/// still killable with that view lag, not without it.
#[test]
fn rewound_kill() {
    let Some(d) = data() else { return };
    for (lag, want) in [(0u8, false), (9, true)] {
        let mut w = world(&d);
        w.place_player(0, Vec3::new(31.0, 20.4, 0.02), -90f32.to_radians());
        w.place_player(1, Vec3::new(33.0, 20.4, 0.02), -90f32.to_radians());
        // B runs off along +X for 1 s
        let away = PlayerInput { forward: true, run: true, cam_yaw: 0.0, ..Default::default() };
        let mut out = None;
        for _ in 0..60 {
            w.step(&[PlayerInput::default(), away]);
            let d = w.players[0].pos.distance(w.players[1].pos);
            let Some(then) = w.seen.len().checked_sub(9).map(|k| w.seen[k][1].pos.distance(w.players[0].pos)) else { continue };
            if d > 4.7 && then < 4.35 {
                out = Some(w.tick);
                break;
            }
        }
        assert!(out.is_some(), "no tick with the victim in reach 9 ticks ago only");
        let req = PlayerInput { kill: true, view_lag: lag, cam_yaw: 0.0, ..Default::default() };
        let rules = acb_game::rules::Rules { free_play: false, allow_killing_npcs: true };
        let got = w.validate_action(0, &req, &rules, &|_, _| acb_game::rules::Relation::Target);
        eprintln!("lag {lag}: {got:?}");
        assert_eq!(got.is_ok_and(|r| r != acb_game::action::Request::None), want);
    }
}
