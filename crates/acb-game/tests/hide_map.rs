//! Hiding spots, blending and falls on San Donato with the retail data.
//! Needs the game data (`ACBMP_SCHEMAS`, `ACBMP_MULTI`, `ACBMP_EXE`);
//! skipped otherwise.

use std::path::PathBuf;

use acb_anim::BuiltinTracks;
use acb_game::mode::{Match, MatchConfig, MatchEvent};
use acb_game::{kill, rules, PlayerInput, Setup, World};
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

/// One player (body 0) at `at` (x,y,z,yaw), bodies 1.. placed by the
/// further groups of `at`.
fn world(d: &Data, count: usize, at: &[f32]) -> World {
    let character = std::sync::Arc::new(acb_game::load_character(&d.archive, "AC2MP_ID07_MainTemplate_Player").unwrap());
    let setup = Setup {
        characters: vec![("AC2MP_ID07_MainTemplate_Player".into(), character)],
        count: Some(count),
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

/// Running east (camera yaw 0 = +X) into haystack 13 at 24.62,7.
fn run_east() -> PlayerInput {
    PlayerInput { forward: true, run: true, cam_yaw: 0.0, ..PlayerInput::default() }
}

fn hide_in_13(w: &mut World) -> u64 {
    let spots = &w.clips.hide.as_ref().expect("haystacks").spots;
    assert_eq!(spots.iter().filter(|s| s.seat.is_none()).count(), 20);
    let c = spots[13].center();
    assert!((c - Vec3::new(24.62, 7.0, 0.0)).length() < 0.05, "{c}");
    for _ in 0..120 {
        let input = if w.players[0].hide.is_some() { PlayerInput::default() } else { run_east() };
        w.step(&[input]);
        if w.players[0].hidden {
            return w.tick;
        }
    }
    panic!("never hidden: at {:?}", w.players[0].pos);
}

#[test]
fn hide_enter_exit() {
    let Some(d) = data() else { return };
    let mut w = world(&d, 1, &[21.0, 7.0, 0.0, -90.0]);
    let t = hide_in_13(&mut w);
    let p = &w.players[0];
    assert_eq!(p.actor().stance, rules::Stance::Hidden);
    assert!((p.pos - Vec3::new(24.62, 7.0, 0.0)).truncate().length() < 0.05, "{:?}", p.pos);
    // the entry clip (0.87 s) from about 1.5 m out at a run
    assert!((40..=110).contains(&t), "hidden at tick {t}");
    for _ in 0..30 {
        w.step(&[PlayerInput::default()]);
    }
    assert!(w.players[0].hidden, "stays in without input");
    // leave to the left of the camera (+Y): the hop-out carries 2 m
    let left = PlayerInput { left: true, ..PlayerInput::default() };
    let mut out = None;
    let mut exiting = 0;
    for i in 0..120 {
        w.step(&[if i < 3 { left } else { PlayerInput::default() }]);
        if w.players[0].hide.is_none() {
            out = Some(i);
            break;
        }
        if !w.players[0].hidden {
            exiting += 1;
        }
    }
    // the hop-out and its transition (1.73 s) play out of hiding
    assert!(exiting > 90, "{exiting} exit ticks");
    assert!(out.is_some(), "never out");
    let p = &w.players[0];
    assert!((p.pos - Vec3::new(24.62, 9.0, 0.0)).truncate().length() < 0.1, "{:?}", p.pos);
    assert_eq!(p.actor().stance, rules::Stance::Ground);
}

#[test]
fn haystack_kill() {
    let Some(d) = data() else { return };
    // the NPC (body 1) stands 2.4 m north of the haystack
    let mut w = world(&d, 2, &[21.0, 7.0, 0.0, -90.0, 24.62, 9.4, 0.0, 180.0]);
    hide_in_13(&mut w);
    w.step(&[PlayerInput { kill: true, ..PlayerInput::default() }]);
    let p = &w.players[0];
    let set = p.kill.as_ref().expect("kill from the haystack").set();
    assert_eq!(set & !1, kill::HAY_GRAB);
    assert!(p.hide.is_none() && !p.hidden);
    assert!(w.bodies[1].scripted.is_some());
    for _ in 0..240 {
        w.step(&[PlayerInput::default()]);
    }
    assert!(w.players[0].kill.is_none(), "the grab kill ends");
}

#[test]
fn blend_among_lookalikes() {
    let Some(d) = data() else { return };
    // three persona lookalikes standing round the player on open ground
    let at = [31.0, 20.4, 0.02, -90.0, 32.0, 20.4, 0.02, 0.0, 31.8, 21.5, 0.02, 0.0, 31.8, 19.3, 0.02, 0.0];
    let mut w = world(&d, 4, &at);
    for _ in 0..10 {
        w.step(&[PlayerInput::default()]);
    }
    assert!(w.players[0].blended, "standing in the group");
    // running away: out of the group and high profile
    let sprint = PlayerInput { forward: true, run: true, cam_yaw: std::f32::consts::PI, ..PlayerInput::default() };
    // (a turn on the spot first)
    for _ in 0..120 {
        w.step(&[sprint]);
    }
    assert!(!w.players[0].blended, "at {:?}", w.players[0].pos);
}

#[test]
fn fatal_fall_is_a_death() {
    let Some(d) = data() else { return };
    // 21 m above the roll gate's landing: 200 % damage
    let mut w = world(&d, 1, &[34.46, 20.4, 21.0, -90.0]);
    let spawn = (Vec3::new(31.0, 20.4, 0.02), 0.0);
    w.mode = Some(Match::new(1, MatchConfig { time_limit: 60.0, score_limit: 0, respawn_delay: 1.0, start_delay: 0.0 }, vec![spawn]));
    let mut fell = None;
    for _ in 0..240 {
        w.step(&[PlayerInput::default()]);
        let m = w.mode.as_ref().unwrap();
        if let Some(e) = m.events.iter().find(|e| matches!(e, MatchEvent::Fall { .. })) {
            fell = Some((*e, w.tick));
            break;
        }
    }
    let (e, tick) = fell.expect("never landed");
    assert_eq!(e, MatchEvent::Fall { player: 0, damage: 200, fatal: true });
    let m = w.mode.as_ref().unwrap();
    assert!(!m.alive(0));
    assert_eq!(m.score, vec![0]);
    // back on the spawn after the delay, full life
    for _ in 0..61 {
        w.step(&[PlayerInput::default()]);
    }
    let m = w.mode.as_ref().unwrap();
    assert!(m.alive(0), "respawned (fell at tick {tick})");
    assert_eq!(m.health[0], acb_game::mode::FULL_HEALTH);
    assert!((w.players[0].pos - spawn.0).truncate().length() < 0.1);
}

/// Benches: 34 on San Donato, three seats each; the jump button next to a
/// seat sits down (blended, not hidden), a fresh move key stands up.
#[test]
fn bench_sit_blend_stand() {
    let Some(d) = data() else { return };
    let probe = world(&d, 1, &[0.0, 0.0, 0.0, 0.0]);
    let spots = probe.clips.hide.as_ref().expect("hide").spots.clone();
    let seats: Vec<usize> = (0..spots.len()).filter(|&i| spots[i].seat.is_some()).collect();
    assert_eq!(seats.len(), 34 * 3);
    let k = seats[0];
    let s = &spots[k];
    let seat = s.frame.w_axis.truncate();
    let out = s.frame.x_axis.truncate().normalize();
    // 0.4 m in front of the seat, facing the bench
    let at = seat + out * 0.4;
    let yaw = acb_game::hide::seat_yaw(s).to_degrees() + 180.0;
    let mut w = world(&d, 1, &[at.x, at.y, at.z, yaw]);
    for _ in 0..10 {
        w.step(&[PlayerInput::default()]);
    }
    w.step(&[PlayerInput { jump: true, ..PlayerInput::default() }]);
    assert!(w.players[0].hide.as_ref().is_some_and(|h| h.spot == k), "did not sit from {at}");
    let mut seated = None;
    for _ in 0..120 {
        w.step(&[PlayerInput::default()]);
        if w.players[0].blended {
            seated = Some(w.tick);
            break;
        }
    }
    let t = seated.expect("never seated");
    let p = &w.players[0];
    assert!(!p.hidden);
    assert_eq!(p.actor().stance, rules::Stance::Ground);
    assert!((p.pos - seat).truncate().length() < 0.05, "{:?} vs {seat}", p.pos);
    // the sit-down (0.67 s) and the settle (0.53 s)
    assert!((60..=90).contains(&(t - 11)), "seated after {} ticks", t - 11);
    for _ in 0..60 {
        w.step(&[PlayerInput::default()]);
    }
    assert!(w.players[0].blended, "still seated");
    // the camera looks at the bench: back = away from it; one tick to arm
    let cam = (-out.y).atan2(-out.x);
    let mut stood = false;
    for _ in 0..90 {
        w.step(&[PlayerInput { back: true, cam_yaw: cam, ..PlayerInput::default() }]);
        if w.players[0].hide.is_none() {
            stood = true;
            break;
        }
    }
    assert!(stood, "never stood up");
    let p = &w.players[0];
    assert!(!p.blended);
    assert!((p.pos - seat).dot(out) > 0.15, "stood up in front: {:?}", p.pos);
}
