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
use acb_game::{rules, PlayerInput};
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

/// Slot 0 stands; slot 1 walks (low profile) to the nearest crowd blend
/// group's centre (running while farther than 1.2 m) and keeps following it.
fn script(slot: usize, tick: u32, world: &acb_game::World) -> PlayerInput {
    let mut i = PlayerInput::default();
    if slot != 1 || tick < 30 {
        return i;
    }
    let me = world.players[slot].pos;
    let members: Vec<rules::Member> = world.bodies[world.players.len()..]
        .iter()
        .map(|b| rules::Member { pos: b.entity.w_axis.truncate(), yaw: (-b.entity.y_axis.x).atan2(b.entity.y_axis.y) })
        .collect();
    let centre = rules::blend_groups(&members)
        .into_iter()
        .map(|g| g.iter().map(|&k| members[k].pos).sum::<Vec3>() / g.len() as f32)
        .min_by(|a, b| a.distance(me).total_cmp(&b.distance(me)));
    if let Some(c) = centre {
        let d = (c - me).truncate();
        i.cam_yaw = d.y.atan2(d.x);
        i.forward = d.length() > 0.3;
        i.run = d.length() > 1.2;
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
    while (world.tick < 380 || client.confirmed_tick < 370) && client.over.is_none() && t0.elapsed() < Duration::from_secs(60) {
        let input = script(slot, world.tick as u32, &world);
        client.advance(&mut world, |_| input);
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
    // slot 1 found a group and blended (its flags matched the server's)
    assert!(seen[1].blended > 30, "slot 1 blended {} ticks", seen[1].blended);
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
