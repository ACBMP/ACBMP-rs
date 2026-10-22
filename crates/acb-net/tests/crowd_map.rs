//! In-process match on SanDonato with a crowd over lossy localhost UDP:
//! the server and both clients build the same crowd from `Start` and
//! simulate it as part of the deterministic world. Checks that both
//! clients reproduce the server's players exactly (flags, i.e. blending,
//! included), that the crowd moves, and that the two clients' confirmed
//! crowds agree tick by tick. Needs `ACBMP_SCHEMAS`, `ACBMP_MULTI` and
//! `ACBMP_EXE`; skipped without them.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use acb_anim::BuiltinTracks;
use acb_game::PlayerInput;
use acb_net::client::{Client, ClientConfig, ClientStats};
use acb_net::link::NetSim;
use acb_net::proto::CrowdConfig;
use acb_net::server::{run, ServeConfig};
use glam::Vec3;

const MAP: &str = "DataPC_AC2MP_SanDonato.forge";
const CHARACTER: &str = "AC2MP_ID07_MainTemplate_Player";
const PORT: u16 = 7934;
const CROWD: CrowdConfig = CrowdConfig { count: 40, seed: 7 };

fn env() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let exe = PathBuf::from(std::env::var_os("ACBMP_EXE")?);
    Some((schemas, multi.join(MAP), exe))
}

/// Slot 0 stands; slot 1 walks (low profile, may blend) away and back.
fn script(slot: usize, tick: u32) -> PlayerInput {
    let t = tick as f32 / 60.0;
    let mut i = PlayerInput { cam_yaw: if slot == 0 { 0.0 } else { std::f32::consts::PI }, ..Default::default() };
    if slot == 1 {
        i.forward = (0.5..2.5).contains(&t);
        i.back = (2.5..4.5).contains(&t);
    }
    i
}

struct Seen {
    slot: usize,
    stats: ClientStats,
    crowd: BTreeMap<u64, Vec<Vec3>>,
    blended: u32,
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
    assert_eq!(start.crowd, CROWD);
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).unwrap()));
    let archive = acb_world::open_map(&map, schema).unwrap();
    let builtins = BuiltinTracks::from_executable(&std::fs::read(&exe).unwrap());
    let mut world = acb_net::game::load_world(&archive, &start.slots, builtins.as_ref(), start.rules, start.crowd).unwrap();
    assert_eq!(world.bodies.len(), start.slots.len() + CROWD.count as usize);
    let slot = start.slot;
    let mut crowd = BTreeMap::new();
    let mut blended = 0;
    let t0 = Instant::now();
    while (world.tick < 300 || client.confirmed_tick < 290) && client.over.is_none() && t0.elapsed() < Duration::from_secs(60) {
        client.advance(&mut world, |t| script(slot, t));
        if let Some((t, c)) = client.confirmed_crowd() {
            crowd.insert(t, c);
        }
        blended += u32::from(world.players[slot].blended);
        std::thread::sleep(Duration::from_millis(4));
    }
    client.leave();
    Seen { slot, stats: client.stats, crowd, blended }
}

#[test]
fn crowd_over_lossy_localhost() {
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
                rules: Some(acb_net::mode::MatchConfig { time_limit: 60.0, score_limit: 1000, respawn_delay: 5.0, start_delay: 3.0 }),
                crowd: CROWD,
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
        eprintln!("slot {}: {:?}; {} confirmed crowd ticks, {} blended ticks", s.slot, s.stats, s.crowd.len(), s.blended);
        assert!(s.stats.checked > 100, "slot {}: {:?}", s.slot, s.stats);
        assert_eq!(s.stats.exact, s.stats.checked, "slot {} desynced", s.slot);
        assert_eq!(s.stats.game_desyncs, 0, "slot {} match desynced", s.slot);
    }
    // both clients simulate the same crowd
    let common: Vec<u64> = seen[0].crowd.keys().filter(|t| seen[1].crowd.contains_key(t)).copied().collect();
    assert!(common.len() > 50, "{} common confirmed ticks", common.len());
    for t in &common {
        assert_eq!(seen[0].crowd[t], seen[1].crowd[t], "crowd differs at tick {t}");
    }
    let (first, last) = (&seen[0].crowd[&common[0]], &seen[0].crowd[common.last().unwrap()]);
    let moved = first.iter().zip(last).filter(|(a, b)| a.distance(**b) > 0.5).count();
    assert!(moved > 5, "only {moved} NPCs moved");
}
