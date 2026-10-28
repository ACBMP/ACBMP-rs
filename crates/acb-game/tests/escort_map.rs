//! Escort VIP bodies on San Donato: two VIPs walk their ways, a hunter
//! kills both through the kill flow, both respawn 5 s later at least 35 m
//! from it and become targets 10 s after that. Needs the game data
//! (`ACBMP_SCHEMAS`, `ACBMP_MULTI`, `ACBMP_EXE`); skipped otherwise.

use std::path::PathBuf;

use acb_anim::BuiltinTracks;
use acb_game::mode::{MatchConfig, Mode};
use acb_game::rules::{Relation, Teams};
use acb_game::vip::{self, VipEvent};
use acb_game::{PlayerInput, Setup, World};
use glam::{Vec2, Vec3};

const PERSONA: &str = "AC2MP_ID07_MainTemplate_Player";

fn world() -> Option<World> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let exe = PathBuf::from(std::env::var_os("ACBMP_EXE")?);
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).ok()?));
    let archive = acb_world::open_map(&multi.join("DataPC_AC2MP_SanDonato.forge"), schema).ok()?;
    let builtins = BuiltinTracks::from_executable(&std::fs::read(exe).ok()?);
    let character = std::sync::Arc::new(acb_game::load_character(&archive, PERSONA).unwrap());
    let setup = Setup {
        characters: vec![(PERSONA.into(), character)],
        count: Some(2),
        anim: "xx_l_wait_hipm_footl".into(),
        wander: 0.0,
        play: true,
        crowd: 0,
        pair: Vec::new(),
        at: None,
        builtins: builtins.as_ref(),
    };
    let mut w = World::load(&archive, &setup).unwrap();
    w.add_players(2);
    let spawns: Vec<(Vec3, f32)> = w.players.iter().map(|p| (p.pos, p.yaw)).collect();
    let config = MatchConfig { time_limit: 600.0, score_limit: 0, respawn_delay: 5.0, start_delay: 0.0 };
    w.escort(&archive, builtins.as_ref(), Teams::alternating(2), config, spawns).unwrap();
    Some(w)
}

fn vips(w: &World) -> &vip::Vips {
    w.mode.as_ref().unwrap().vips.as_ref().unwrap()
}

fn step(w: &mut World, hunter: PlayerInput, events: &mut Vec<(u64, VipEvent)>) {
    w.step(&[hunter, PlayerInput::default()]);
    events.extend(vips(w).events.iter().map(|e| (w.tick, e.clone())));
}

#[test]
fn san_donato_vips_walk_die_and_respawn() {
    let Some(mut w) = world() else {
        eprintln!("skipped: no game data");
        return;
    };
    let mut ev = Vec::new();
    // first spawn: both at once, targets at once (hunter 0 -> VIP bodies)
    step(&mut w, PlayerInput::default(), &mut ev);
    let bodies = vips(&w).bodies.clone();
    assert_eq!(bodies.len(), vip::NB_VIPS);
    assert_eq!(ev.iter().filter(|(_, e)| matches!(e, VipEvent::Spawned { .. })).count(), 2, "{ev:?}");
    let rel = |w: &World, p: usize, b: usize| w.mode.as_ref().unwrap().relation(p, b);
    assert_eq!(rel(&w, 0, bodies[0]), Relation::Target);
    assert_eq!(rel(&w, 1, bodies[0]), Relation::Other, "protectors never target VIPs");
    // they walk (crowd pace)
    let at = |w: &World, b: usize| w.bodies[b].entity.w_axis.truncate();
    let start: Vec<Vec3> = bodies.iter().map(|&b| at(&w, b)).collect();
    for _ in 0..300 {
        step(&mut w, PlayerInput::default(), &mut ev);
    }
    for (k, &b) in bodies.iter().enumerate() {
        let d = (at(&w, b) - start[k]).truncate().length();
        eprintln!("VIP {k} walked {d:.2} m in 5 s");
        assert!(d > 4.0 && d < 10.0, "VIP {k} walked {d}");
    }
    // the hunter kills each: put 4 m behind it, run at it pressing kill
    // (a running kill, Run2Ground: no walk-in behind a walking VIP)
    let mut killer = Vec3::ZERO;
    let mut last_kill = 0;
    for (k, &b) in bodies.iter().enumerate() {
        let f = w.bodies[b].entity.y_axis.truncate();
        let yaw = (-f.x).atan2(f.y);
        w.place_player(0, at(&w, b) - Vec2::new(-yaw.sin(), yaw.cos()).extend(0.0) * 4.0, yaw);
        let before = ev.len();
        let mut dead = false;
        for _ in 0..240 {
            let to = (at(&w, b) - w.players[0].pos).truncate();
            let chase = PlayerInput { forward: true, run: true, cam_yaw: to.y.atan2(to.x), kill: true, ..Default::default() };
            step(&mut w, chase, &mut ev);
            if ev[before..].iter().any(|(_, e)| matches!(e, VipEvent::Killed { vip, killer: 0, .. } if *vip == k)) {
                dead = true;
                break;
            }
        }
        assert!(dead, "VIP {k} not killed: {:?}", &ev[before..]);
        assert!(w.bodies[b].scripted.is_some(), "the VIP plays its victim clip");
        assert_ne!(rel(&w, 0, b), Relation::Target, "a dead VIP is no target");
        last_kill = w.tick;
        // let the kill clip end
        for _ in 0..200 {
            step(&mut w, PlayerInput::default(), &mut ev);
            if w.players[0].kill.is_none() {
                break;
            }
        }
        killer = w.players[0].pos;
    }
    // both dead: 300 ticks later both respawn, >= 35 m from the killer
    let before = ev.len();
    while w.tick < last_kill + 300 {
        step(&mut w, PlayerInput::default(), &mut ev);
    }
    let spawned: Vec<u64> = ev[before..].iter().filter(|(_, e)| matches!(e, VipEvent::Spawned { .. })).map(|(t, _)| *t).collect();
    assert_eq!(spawned, vec![last_kill + 300; 2], "{:?}", &ev[before..]);
    for &b in &bodies {
        let d = (at(&w, b) - killer).truncate().length();
        eprintln!("respawned VIP body {b} {d:.1} m from the killer");
        assert!(d >= vip::MIN_DIST_FROM_KILLERS, "{d}");
        assert!(w.bodies[b].scripted.is_none());
        assert_eq!(rel(&w, 0, b), Relation::Other, "hunters wait 10 s");
    }
    // targets again 600 ticks after the respawn
    while w.tick < last_kill + 300 + 600 {
        step(&mut w, PlayerInput::default(), &mut ev);
    }
    let t = ev.iter().find(|(t, e)| *t > last_kill && *e == VipEvent::Targetable).map(|(t, _)| *t);
    assert_eq!(t, Some(last_kill + 900));
    assert!(matches!(&w.mode.as_ref().unwrap().mode, Mode::Escort(t) if t.vips == bodies));
}
