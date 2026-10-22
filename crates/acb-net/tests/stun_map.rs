//! Three players on SanDonato over lossy localhost: whoever hunts player 0
//! walks up to it, player 0 stuns its pursuer once it is within the stun
//! reach. Checks the stun events, the pursuer losing its contract, the
//! stun score (`ScoreHumiliation` 200) and that all clients stay in sync.
//!
//! Needs `ACBMP_SCHEMAS`, `ACBMP_MULTI` and `ACBMP_EXE`; skipped without them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use acb_anim::BuiltinTracks;
use acb_game::mode::{MatchConfig, STUN_POINTS};
use acb_game::PlayerInput;
use acb_net::client::{Client, ClientConfig, ClientStats};
use acb_net::link::NetSim;
use acb_net::proto::{Event, EventKind, GameState};
use acb_net::server::{run, ServeConfig};
use glam::Vec3;

const MAP: &str = "DataPC_AC2MP_SanDonato.forge";
const CHARACTER: &str = "AC2MP_ID07_MainTemplate_Player";
const PORT: u16 = 7943;
const N: usize = 3;
const RULES: MatchConfig = MatchConfig { time_limit: 14.0, score_limit: 1000, respawn_delay: 3.0, start_delay: 1.0 };

fn env() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let exe = PathBuf::from(std::env::var_os("ACBMP_EXE")?);
    Some((schemas, multi.join(MAP), exe))
}

/// Player 0 faces +X and stuns when its pursuer is close; the pursuer
/// walks at player 0 while it holds the contract; the third one idles.
fn script(slot: usize, pos: &[Vec3], game: Option<&GameState>, stunned: bool) -> PlayerInput {
    let mut i = PlayerInput::default();
    let Some(g) = game else { return i };
    let hunter = (0..N).find(|&h| h != 0 && g.players.get(h).is_some_and(|p| p.target == 0));
    if slot == 0 {
        if let Some(h) = hunter.filter(|_| !stunned) {
            i.stun = pos[h].distance(pos[0]) < 2.2;
        }
    } else if Some(slot) == hunter && !stunned {
        let d = (pos[0] - pos[slot]).truncate();
        i.cam_yaw = d.y.atan2(d.x);
        i.forward = d.length() > 1.6;
    }
    i
}

struct Seen {
    slot: usize,
    stats: ClientStats,
    events: Vec<Event>,
    game: Option<GameState>,
}

fn play(schemas: PathBuf, map: PathBuf, exe: PathBuf, name: &str) -> Seen {
    let cfg = ClientConfig {
        server: format!("127.0.0.1:{PORT}"),
        name: name.into(),
        character: CHARACTER.into(),
        map: MAP.into(),
        netsim: NetSim::parse("loss=3%,delay=60ms,jitter=5ms").unwrap(),
    };
    let (mut client, start) = Client::connect(&cfg, Duration::from_secs(120)).expect("connect");
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).unwrap()));
    let archive = acb_world::open_map(&map, schema).unwrap();
    let builtins = BuiltinTracks::from_executable(&std::fs::read(&exe).unwrap());
    let mut world = acb_net::game::load_world(&archive, &start.slots, builtins.as_ref(), start.rules, start.crowd).unwrap();
    let slot = start.slot;
    let t0 = Instant::now();
    while client.over.is_none() && t0.elapsed() < Duration::from_secs(60) {
        let pos: Vec<Vec3> =
            (0..N).map(|j| if j == slot { world.players[j].pos } else { client.remote_view(j).map(|v| v.0.w_axis.truncate()).unwrap_or(world.players[j].pos) }).collect();
        let stunned = client.events.iter().any(|e| e.kind == EventKind::StunStarted && e.a == 0);
        let game = client.game.as_ref().map(|g| g.1.clone());
        client.advance(&mut world, |_| script(slot, &pos, game.as_ref(), stunned));
        std::thread::sleep(Duration::from_millis(4));
    }
    client.leave();
    Seen { slot, stats: client.stats, events: client.events.clone(), game: client.game.clone().map(|g| g.1) }
}

#[test]
fn stun_the_pursuer() {
    let Some((schemas, map, exe)) = env() else { return };
    let server = {
        let (schemas, map, exe) = (schemas.clone(), map.clone(), exe.clone());
        std::thread::spawn(move || {
            run(ServeConfig {
                map,
                schemas,
                exe: Some(exe),
                port: PORT,
                players: N,
                snapshot_every: 2,
                netsim: NetSim::default(),
                at: vec![Some([31.0, 20.4, 0.02, -90.0]), Some([41.0, 20.4, 0.02, 90.0]), Some([41.0, 24.4, 0.02, 90.0])],
                log: None,
                duration: None,
                rules: Some(RULES),
                crowd: Default::default(),
            })
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    let mut threads = Vec::new();
    for name in ["a", "b", "c"] {
        let (schemas, map, exe) = (schemas.clone(), map.clone(), exe.clone());
        threads.push(std::thread::spawn(move || play(schemas, map, exe, name)));
        std::thread::sleep(Duration::from_millis(300));
    }
    let mut seen: Vec<Seen> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    seen.sort_by_key(|s| s.slot);
    server.join().unwrap().unwrap();
    for s in &seen {
        eprintln!("slot {}: {:?}; final {:?}", s.slot, s.stats, s.game);
        assert!(s.stats.checked > 150, "slot {}: {:?}", s.slot, s.stats);
        assert_eq!(s.stats.exact, s.stats.checked, "slot {} desynced", s.slot);
        assert_eq!(s.stats.game_desyncs, 0, "slot {} match state desynced", s.slot);
    }
    let a = &seen[0];
    for e in &a.events {
        eprintln!("  {}", e.describe());
    }
    let stun = a.events.iter().find(|e| e.kind == EventKind::StunStarted && e.a == 0).expect("player 0 stunned someone");
    let hunter = stun.b;
    assert!(a.events.iter().any(|e| e.kind == EventKind::ContractAssigned && e.a == hunter && e.b == 0 && e.tick < stun.tick), "stunned a non-pursuer");
    assert!(a.events.iter().any(|e| e.kind == EventKind::Stunned && e.a == hunter && e.tick == stun.tick));
    let score = a.events.iter().find(|e| e.kind == EventKind::Score && e.a == 0).expect("stun scored");
    assert_eq!((score.tick, score.b as u32 * 10), (stun.tick, STUN_POINTS));
    // the stunned hunter loses its contract on player 0
    assert!(a.events.iter().any(|e| e.kind == EventKind::ContractLost && e.a == hunter && e.tick == stun.tick), "{:?}", a.events);
    let g = a.game.as_ref().unwrap();
    assert_eq!(g.players[0].score as u32, STUN_POINTS);
    // player 0 survived it
    assert!(!a.events.iter().any(|e| e.kind == EventKind::Death && e.a == 0));
}
