//! In-process duel on SanDonato over localhost UDP with simulated loss,
//! delay and jitter on both clients (the "first playable" gate):
//! - before the contracts are handed out, A's kill on B is refused as not
//!   a valid victim;
//! - A and B then hunt each other; B runs at A and turns away, A's kill
//!   (predicted against B still coming) is refused (range) and rolled back;
//! - A walks up to B and kills it: A scores, B respawns on a spawn away
//!   from A, contracts are fulfilled and reassigned;
//! - both clients reproduce the server's positions and match state exactly.
//!
//! Needs `ACBMP_SCHEMAS`, `ACBMP_MULTI` and `ACBMP_EXE`; skipped without them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use acb_anim::BuiltinTracks;
use acb_game::mode::MatchConfig;
use acb_game::PlayerInput;
use acb_net::client::{Client, ClientConfig, ClientStats};
use acb_net::link::NetSim;
use acb_net::proto::{Event, EventKind, GameState, Reject};
use acb_net::server::{run, ServeConfig};
use glam::Vec3;

const MAP: &str = "DataPC_AC2MP_SanDonato.forge";
const CHARACTER: &str = "AC2MP_ID07_MainTemplate_Player";
const PORT: u16 = 7941;
const RULES: MatchConfig = MatchConfig { time_limit: 16.0, score_limit: 1000, respawn_delay: 3.0, start_delay: 1.0 };

fn env() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let exe = PathBuf::from(std::env::var_os("ACBMP_EXE")?);
    Some((schemas, multi.join(MAP), exe))
}

/// A (slot 0) stands facing +X, presses kill at 0.5 s (no contracts yet)
/// and at 3.5 s (B dodging); from 6 s it runs at B, walks the last metres
/// and kills it at ~8.6 s. B (slot 1) starts 10 m ahead facing A, runs at
/// it from 2 s and turns back at 3.4 s (A cannot have its input by 3.5 s at 150 ms
/// delay, so A always predicts the kill), resting from 5 s.
/// Purely a function of the sim tick (never of what arrived over the
/// network yet), so every run plays out the same.
fn script(slot: usize, tick: u32) -> PlayerInput {
    let t = tick as f32 / 60.0;
    let mut i = PlayerInput { cam_yaw: if slot == 0 { 0.0 } else { std::f32::consts::PI }, ..Default::default() };
    if slot == 0 {
        if (0.5..0.75).contains(&t) || (3.5..3.75).contains(&t) || (8.6..9.1).contains(&t) {
            i.kill = true;
        } else if (6.0..8.6).contains(&t) {
            i.forward = true;
            i.run = t < 7.6;
        }
    } else if (2.0..3.4).contains(&t) {
        i.forward = true;
        i.run = true;
    } else if (3.4..5.0).contains(&t) {
        i.back = true;
        i.run = true;
    }
    i
}

struct Seen {
    slot: usize,
    name: String,
    stats: ClientStats,
    events: Vec<Event>,
    game: Option<GameState>,
    own_to: Vec3,
    other_to: Vec3,
    over: Option<String>,
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
    assert_eq!(start.rules, Some(RULES));
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).unwrap()));
    let archive = acb_world::open_map(&map, schema).unwrap();
    let builtins = BuiltinTracks::from_executable(&std::fs::read(&exe).unwrap());
    let mut world = acb_net::game::load_world(&archive, &start.slots, builtins.as_ref(), start.rules, start.crowd).unwrap();
    let slot = start.slot;
    let other = 1 - slot;
    let t0 = Instant::now();
    while client.over.is_none() && t0.elapsed() < Duration::from_secs(60) {
        client.advance(&mut world, |t| script(slot, t));
        std::thread::sleep(Duration::from_millis(4));
    }
    let other_to = client.remote_view(other).map(|v| v.0.w_axis.truncate()).unwrap_or_default();
    let own_to = world.players[slot].pos;
    client.leave();
    Seen { slot, name: name.into(), stats: client.stats, events: client.events.clone(), game: client.game.clone().map(|g| g.1), own_to, other_to, over: client.over.clone() }
}

#[test]
fn duel_over_lossy_localhost() {
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
                duration: None,
                rules: Some(RULES),
                crowd: Default::default(),
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
        eprintln!("slot {}: {:?}; over {:?}; final {:?}", s.slot, s.stats, s.over, s.game);
        for e in s.events.iter().filter(|e| e.kind != EventKind::KillRejected || e.a as usize == s.slot) {
            eprintln!("  {}", e.describe());
        }
        assert!(s.stats.checked > 200, "slot {}: {:?}", s.slot, s.stats);
        assert_eq!(s.stats.exact, s.stats.checked, "slot {} desynced", s.slot);
        assert!(s.stats.game_checked > 200);
        assert_eq!(s.stats.game_desyncs, 0, "slot {} match state desynced", s.slot);
        // slot 0 runs A's script and wins, whichever client's join reached
        // the server first (5% loss can delay "a"'s join past "b"'s)
        let win = format!("100, {} 0 ({} wins)", seen[1].name, seen[0].name);
        assert!(s.over.as_deref().is_some_and(|o| o.ends_with(&win)), "{:?}", s.over);
    }
    let a = &seen[0];
    let ev = |kind: EventKind, x: u8, y: u8| a.events.iter().find(|e| e.kind == kind && e.a == x && e.b == y).map(|e| e.tick);
    // the non-target kill: refused for the relation, before any contract
    let first = ev(EventKind::ContractAssigned, 0, 1).expect("A got a contract on B");
    let rel = ev(EventKind::KillRejected, 0, Reject::NotTarget as u8).expect("not your target");
    assert!(rel < first);
    // the dodge: predicted, refused for range, rolled back
    assert!(ev(EventKind::KillRejected, 0, Reject::Range as u8).is_some(), "{:?}", a.stats);
    assert!(a.stats.rollbacks >= 1, "{:?}", a.stats);
    // the kill
    let death = ev(EventKind::Death, 1, 0).expect("B killed by A");
    assert_eq!(ev(EventKind::Score, 0, 10), Some(death));
    assert_eq!(ev(EventKind::ContractFulfilled, 0, 1), Some(death));
    let respawn = a.events.iter().find(|e| e.kind == EventKind::Respawn && e.a == 1 && e.tick > death).expect("B respawned").tick;
    assert_eq!(respawn - death, acb_game::mode::ticks(RULES.respawn_delay));
    let again = a.events.iter().find(|e| e.kind == EventKind::ContractAssigned && e.a == 0 && e.tick > death).expect("A reassigned").tick;
    // the contract cooldown already counts down in the death tick
    assert!(again + 1 >= death + acb_game::mode::ticks(acb_game::rules::MIN_WAIT_TIME), "{again} {death}");
    let g = a.game.as_ref().unwrap();
    assert_eq!((g.players[0].score, g.players[1].score), (100, 0));
    // B is where A sees it, away from A
    assert!(seen[0].other_to.distance(seen[1].own_to) < 0.05);
    assert!(seen[1].own_to.distance(seen[0].own_to) > 10.0, "B respawned next to A");
}
