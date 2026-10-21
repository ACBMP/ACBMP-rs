//! In-process match on SanDonato over localhost UDP with simulated loss,
//! delay and jitter on both clients: the server runs the sim, slot 0
//! stands and presses kill while slot 1 runs at it and turns away just
//! before. Checks that both clients reproduce the server's state exactly,
//! see each other move, and that the out-of-range kill slot 0 predicted is
//! rejected by the server and rolled back. Needs `ACBMP_SCHEMAS`,
//! `ACBMP_MULTI` and `ACBMP_EXE`; skipped without them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use acb_anim::BuiltinTracks;
use acb_game::PlayerInput;
use acb_net::client::{Client, ClientConfig, ClientStats};
use acb_net::link::NetSim;
use acb_net::server::{run, ServeConfig};

const MAP: &str = "DataPC_AC2MP_SanDonato.forge";
const CHARACTER: &str = "AC2MP_ID07_MainTemplate_Player";
const PORT: u16 = 7931;

fn env() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let exe = PathBuf::from(std::env::var_os("ACBMP_EXE")?);
    Some((schemas, multi.join(MAP), exe))
}

/// Slot 0 stands still facing +X and holds kill from 2.5 to 2.75 s; slot 1
/// starts 10 m ahead facing it, runs at it from 1 s and turns back at 2.27 s.
fn script(slot: usize, tick: u32) -> PlayerInput {
    let t = tick as f32 / 60.0;
    let mut i = PlayerInput { cam_yaw: if slot == 0 { 0.0 } else { std::f32::consts::PI }, ..Default::default() };
    if slot == 0 {
        i.kill = (2.5..2.75).contains(&t);
    } else if (1.0..2.27).contains(&t) {
        i.forward = true;
        i.run = true;
    } else if (2.27..4.0).contains(&t) {
        i.back = true;
        i.run = true;
    }
    i
}

struct Seen {
    slot: usize,
    stats: ClientStats,
    /// Where the other player started and was last drawn.
    other_from: glam::Vec3,
    other_to: glam::Vec3,
    own_to: glam::Vec3,
}

fn play(schemas: PathBuf, map: PathBuf, exe: PathBuf, name: &str) -> Seen {
    let cfg = ClientConfig {
        server: format!("127.0.0.1:{PORT}"),
        name: name.into(),
        character: CHARACTER.into(),
        map: MAP.into(),
        netsim: NetSim::parse("loss=5%,delay=150ms,jitter=10ms").unwrap(),
    };
    let (mut client, start) = Client::connect(&cfg, Duration::from_secs(120)).expect("connect");
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).unwrap()));
    let archive = acb_world::open_map(&map, schema).unwrap();
    let builtins = BuiltinTracks::from_executable(&std::fs::read(&exe).unwrap());
    let mut world = acb_net::game::load_world(&archive, &start.slots, builtins.as_ref()).unwrap();
    let slot = start.slot;
    let other = 1 - slot;
    let other_from = world.players[other].pos;
    let t0 = Instant::now();
    while world.tick < 300 && client.over.is_none() && t0.elapsed() < Duration::from_secs(60) {
        client.advance(&mut world, |t| script(slot, t));
        std::thread::sleep(Duration::from_millis(4));
    }
    // let the confirmed ticks catch up
    let wait = Instant::now();
    while client.confirmed_tick < 290 && wait.elapsed() < Duration::from_secs(2) {
        client.advance(&mut world, |t| script(slot, t));
        std::thread::sleep(Duration::from_millis(4));
    }
    let other_to = client.remote_view(other).map(|v| v.0.w_axis.truncate()).unwrap_or(other_from);
    let own_to = world.players[slot].pos;
    client.leave();
    Seen { slot, stats: client.stats, other_from, other_to, own_to }
}

#[test]
fn match_over_lossy_localhost() {
    let Some((schemas, map, exe)) = env() else { return };
    let server = {
        let (schemas, map, exe) = (schemas.clone(), map.clone(), exe.clone());
        std::thread::spawn(move || {
            run(ServeConfig {
                map,
                schemas,
                exe: Some(exe),
                port: PORT,
                players: 2,
                snapshot_every: 2,
                netsim: NetSim::default(),
                at: vec![Some([31.0, 20.4, 0.02, -90.0]), Some([41.0, 20.4, 0.02, 90.0])],
                log: None,
                duration: Some(7.0),
            })
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    let a = {
        let (schemas, map, exe) = (schemas.clone(), map.clone(), exe.clone());
        std::thread::spawn(move || play(schemas, map, exe, "a"))
    };
    std::thread::sleep(Duration::from_millis(300));
    let b = std::thread::spawn(move || play(schemas, map, exe, "b"));
    let mut seen = [a.join().unwrap(), b.join().unwrap()];
    seen.sort_by_key(|s| s.slot);
    server.join().unwrap().unwrap();
    for s in &seen {
        eprintln!(
            "slot {}: {:?}; other from {:.2?} to {:.2?}, own at {:.2?}",
            s.slot,
            s.stats,
            s.other_from.to_array(),
            s.other_to.to_array(),
            s.own_to.to_array()
        );
        assert!(s.stats.checked > 100, "slot {}: {:?}", s.slot, s.stats);
        assert_eq!(s.stats.exact, s.stats.checked, "slot {} desynced", s.slot);
    }
    // each sees where the other really is: slot 1 ran and came back, slot 0 stood
    assert!(seen[0].other_to.distance(seen[1].own_to) < 0.05, "slot 0 sees slot 1 at {:?}, it is at {:?}", seen[0].other_to, seen[1].own_to);
    assert!(seen[1].other_to.distance(seen[0].own_to) < 0.05);
    assert!(seen[1].own_to.distance(seen[0].other_from) > 1.0, "slot 1 did not move");
    // the kill: refused by the server, the prediction undone
    assert!(seen[0].stats.rejected >= 1, "{:?}", seen[0].stats);
    assert!(seen[0].stats.rollbacks >= 1, "{:?}", seen[0].stats);
}
