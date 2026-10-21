//! The authoritative headless server: a lobby until `players` clients
//! joined, then the real [`acb_game::World`] at 60 Hz with every client's
//! inputs applied at their ticks; snapshots (delta against what each
//! client acknowledged) carry positions, the inputs the server applied and
//! the events it decided. No GPU: the world is built from the archive only.

use std::collections::VecDeque;
use std::io::Write;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use acb_anim::BuiltinTracks;
use acb_game::World;
use glam::Vec3;

use crate::game::{self, InputBuffer};
use crate::link::{Link, NetSim};
use crate::proto::*;

pub struct ServeConfig {
    pub map: PathBuf,
    pub schemas: PathBuf,
    /// ACBMP.exe for the builtin animation tracks (root motion); clients
    /// must use the same one.
    pub exe: Option<PathBuf>,
    pub port: u16,
    pub players: usize,
    /// Send a snapshot every this many ticks (2 = 30 Hz).
    pub snapshot_every: u32,
    pub netsim: NetSim,
    /// Per slot: start x, y, z, yaw degrees instead of the spawn.
    pub at: Vec<Option<[f32; 4]>>,
    /// Write every tick's quantized player state and the events here.
    pub log: Option<PathBuf>,
    /// Stop after this many seconds of play.
    pub duration: Option<f32>,
}

/// Ticks of applied inputs a snapshot repeats at most.
const RELAY: u32 = 64;
/// A client silent this long has left.
const TIMEOUT: Duration = Duration::from_secs(5);

struct Peer {
    addr: SocketAddr,
    name: String,
    character: String,
    ready: bool,
    gone: bool,
    inputs: InputBuffer,
    /// Newest snapshot tick the client has.
    snapshot_ack: Option<u32>,
    newest_input: u32,
    echo_ms: u32,
    echo_at: Instant,
    lead_min: Option<i64>,
    lead: i8,
    heard: Instant,
    snapshots: u64,
    snapshot_bytes: u64,
}

pub fn run(cfg: ServeConfig) -> anyhow::Result<()> {
    anyhow::ensure!((1..=MAX_PLAYERS).contains(&cfg.players), "--players 1..={MAX_PLAYERS}");
    let schema = Box::leak(Box::new(acb_world::load_schema(&cfg.schemas)?));
    let archive = acb_world::open_map(&cfg.map, schema)?;
    let map_name = cfg.map.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let exe = cfg.exe.clone().or_else(|| {
        let p = cfg.map.parent()?.parent()?.join("ACBMP.exe");
        p.exists().then_some(p)
    });
    let builtins = exe.as_ref().and_then(|p| BuiltinTracks::from_executable(&std::fs::read(p).ok()?));
    if builtins.is_none() {
        eprintln!("serve: no ACBMP.exe builtin tracks (--exe): no root motion");
    }
    let sock = UdpSocket::bind(("0.0.0.0", cfg.port))?;
    let mut link = Link::new(sock, cfg.netsim, 0x5e4e_e7)?;
    eprintln!("serve: {map_name} on udp {}, waiting for {} players{}", link.local_addr()?, cfg.players, if cfg.netsim.active() { format!(" (netsim {:?})", cfg.netsim) } else { String::new() });

    // ------------------------------------------------------------ lobby
    let mut peers: Vec<Peer> = Vec::new();
    while peers.len() < cfg.players {
        let Some((data, addr)) = link.recv(Duration::from_millis(250)) else { continue };
        let Some(msg) = Msg::decode(&data) else { continue };
        let known = peers.iter().position(|p| p.addr == addr);
        match msg {
            Msg::Hello { name, character, map } => {
                if let Some(slot) = known {
                    link.send(&Msg::Welcome { slot: slot as u8, players: cfg.players as u8 }.encode(), addr);
                    continue;
                }
                let refuse = if !map.eq_ignore_ascii_case(&map_name) {
                    Some(format!("server runs {map_name}, not {map}"))
                } else if let Err(e) = acb_game::load_character(&archive, &character) {
                    Some(format!("{e:#}"))
                } else {
                    None
                };
                if let Some(reason) = refuse {
                    eprintln!("serve: refused {addr} ({name}): {reason}");
                    link.send(&Msg::Reject { reason }.encode(), addr);
                    continue;
                }
                let slot = peers.len();
                eprintln!("serve: {addr} joined as {name} ({character}), slot {slot}");
                let now = Instant::now();
                peers.push(Peer {
                    addr,
                    name,
                    character,
                    ready: false,
                    gone: false,
                    inputs: InputBuffer::default(),
                    snapshot_ack: None,
                    newest_input: 0,
                    echo_ms: 0,
                    echo_at: now,
                    lead_min: None,
                    lead: 0,
                    heard: now,
                    snapshots: 0,
                    snapshot_bytes: 0,
                });
                link.send(&Msg::Welcome { slot: slot as u8, players: cfg.players as u8 }.encode(), addr);
                let joined = peers.len() as u8;
                for p in &peers {
                    link.send(&Msg::Lobby { joined, players: cfg.players as u8, echo_ms: 0 }.encode(), p.addr);
                }
            }
            Msg::Ping { client_ms } if known.is_some() => {
                link.send(&Msg::Lobby { joined: peers.len() as u8, players: cfg.players as u8, echo_ms: client_ms }.encode(), addr);
            }
            Msg::Bye => {
                if let Some(i) = known {
                    eprintln!("serve: {} left the lobby", peers[i].name);
                    peers.remove(i);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------- load
    let slots: Vec<Slot> = peers
        .iter()
        .enumerate()
        .map(|(i, p)| Slot { name: p.name.clone(), character: p.character.clone(), at: cfg.at.get(i).copied().flatten() })
        .collect();
    let t = Instant::now();
    let mut world = game::load_world(&archive, &slots, builtins.as_ref())?;
    eprintln!("serve: world with {} players loaded in {:.1}s", world.players.len(), t.elapsed().as_secs_f64());
    let start = Msg::Start { map: map_name.clone(), snapshot_every: cfg.snapshot_every as u8, slots: slots.clone() }.encode();
    let mut last_start = Instant::now() - Duration::from_secs(1);
    let wait = Instant::now();
    while !peers.iter().all(|p| p.ready) {
        if last_start.elapsed() > Duration::from_millis(250) {
            for p in peers.iter().filter(|p| !p.ready) {
                link.send(&start, p.addr);
            }
            last_start = Instant::now();
        }
        anyhow::ensure!(wait.elapsed() < Duration::from_secs(300), "players did not get ready");
        let Some((data, addr)) = link.recv(Duration::from_millis(50)) else { continue };
        let Some(i) = peers.iter().position(|p| p.addr == addr) else { continue };
        peers[i].heard = Instant::now();
        match Msg::decode(&data) {
            Some(Msg::Ready) if !peers[i].ready => {
                eprintln!("serve: {} ready", peers[i].name);
                peers[i].ready = true;
            }
            Some(Msg::Ping { client_ms }) => link.send(&Msg::Lobby { joined: peers.len() as u8, players: cfg.players as u8, echo_ms: client_ms }.encode(), addr),
            _ => {}
        }
    }

    // -------------------------------------------------------------- run
    let mut log = match &cfg.log {
        Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p)?)),
        None => None,
    };
    let n = world.players.len();
    // used[t - 1]: the inputs applied at tick t
    let mut used: Vec<Vec<WireInput>> = Vec::new();
    let mut prev_inputs = vec![WireInput::default(); n];
    let mut events: Vec<Event> = Vec::new();
    let mut history: VecDeque<(u32, Vec<NetPos>)> = VecDeque::new();
    let t0 = Instant::now();
    eprintln!("serve: match started");
    send_snapshots(&mut link, &mut peers, &world, &used, &events, &mut history);
    let mut behind_warned = false;
    loop {
        let tick = world.tick as u32 + 1;
        let due = t0 + Duration::from_secs_f64(tick as f64 / acb_game::TICK_HZ as f64);
        // take packets until the tick is due
        loop {
            let now = Instant::now();
            let wait = due.saturating_duration_since(now);
            let Some((data, addr)) = link.recv(wait) else { break };
            let Some(i) = peers.iter().position(|p| p.addr == addr) else { continue };
            handle(&mut peers[i], &data, world.tick as u32, &mut link, &start);
            if Instant::now() >= due {
                break;
            }
        }
        if Instant::now() > due + Duration::from_millis(250) && !behind_warned {
            eprintln!("serve: running behind real time");
            behind_warned = true;
        }
        for p in peers.iter_mut().filter(|p| !p.gone && p.heard.elapsed() > TIMEOUT) {
            eprintln!("serve: {} timed out", p.name);
            p.gone = true;
        }
        if peers.iter().all(|p| p.gone) {
            eprintln!("serve: everyone left");
            break;
        }
        if cfg.duration.is_some_and(|d| tick as f32 / acb_game::TICK_HZ as f32 > d) {
            for p in &peers {
                link.send(&Msg::Over { reason: "time".into() }.encode(), p.addr);
            }
            break;
        }

        // the tick: everyone's input for it (gone players stand still)
        let wire: Vec<WireInput> = peers.iter_mut().map(|p| if p.gone { WireInput::default() } else { p.inputs.take(tick) }).collect();
        let inputs: Vec<_> = wire.iter().map(|w| w.to_input()).collect();
        let before = Before::of(&world);
        world.step(&inputs);
        let new = detect_events(&world, &before, &wire, &prev_inputs, tick);
        for e in &new {
            eprintln!("serve: {}", e.describe());
            if let Some(l) = &mut log {
                let _ = writeln!(l, "event {} {:?} {} {}", e.tick, e.kind, e.a, e.b);
            }
        }
        events.extend(new);
        prev_inputs.clone_from(&wire);
        used.push(wire);
        if let Some(l) = &mut log {
            for (i, q) in game::net_positions(&world).iter().enumerate() {
                let _ = writeln!(l, "srv {tick} {i} {} {} {} {} {}", q.x, q.y, q.z & 0x7fff, q.yaw, q.flags);
            }
        }
        if tick % cfg.snapshot_every.max(1) == 0 {
            send_snapshots(&mut link, &mut peers, &world, &used, &events, &mut history);
        }
    }
    let secs = world.tick as f64 / acb_game::TICK_HZ as f64;
    eprintln!("serve: {} ticks ({secs:.1} s), {} events", world.tick, events.len());
    for p in &peers {
        eprintln!(
            "serve: {}: {} inputs, {} late, {} ticks without input; {} snapshots, avg {:.0} bytes",
            p.name,
            p.inputs.received,
            p.inputs.late,
            p.inputs.missed,
            p.snapshots,
            p.snapshot_bytes as f64 / p.snapshots.max(1) as f64
        );
    }
    let s = link.stats;
    eprintln!(
        "serve: sent {} packets ({:.1} KB/s), received {} ({:.1} KB/s), netsim dropped {}",
        s.sent,
        s.sent_bytes as f64 / 1024.0 / secs.max(1e-3),
        s.recv,
        s.recv_bytes as f64 / 1024.0 / secs.max(1e-3),
        s.dropped
    );
    Ok(())
}

fn handle(p: &mut Peer, data: &[u8], played: u32, link: &mut Link, start: &[u8]) {
    let Some(msg) = Msg::decode(data) else { return };
    p.heard = Instant::now();
    match msg {
        Msg::Input { snapshot_ack, client_ms, last, inputs } => {
            p.inputs.receive(last, &inputs, played);
            if p.snapshot_ack.is_none_or(|a| snapshot_ack > a) {
                p.snapshot_ack = Some(snapshot_ack);
            }
            if last >= p.newest_input {
                p.newest_input = last;
                p.echo_ms = client_ms;
                p.echo_at = Instant::now();
            }
            let lead = last as i64 - played as i64;
            p.lead_min = Some(p.lead_min.map_or(lead, |m| m.min(lead)));
        }
        Msg::Bye => {
            eprintln!("serve: {} left", p.name);
            p.gone = true;
        }
        Msg::Ping { client_ms } => link.send(&Msg::Lobby { joined: 0, players: 0, echo_ms: client_ms }.encode(), p.addr),
        // still loading: it did not see the match start
        Msg::Hello { .. } | Msg::Ready => link.send(start, p.addr),
        _ => {}
    }
}

fn send_snapshots(link: &mut Link, peers: &mut [Peer], world: &World, used: &[Vec<WireInput>], events: &[Event], history: &mut VecDeque<(u32, Vec<NetPos>)>) {
    let tick = world.tick as u32;
    let now = game::net_positions(world);
    history.push_back((tick, now.clone()));
    while history.len() > 128 {
        history.pop_front();
    }
    let n = now.len();
    for p in peers.iter_mut().filter(|p| !p.gone) {
        let base = p.snapshot_ack.and_then(|a| history.iter().find(|h| h.0 == a));
        let (changed, pos) = make_delta(base.map(|b| b.1.as_slice()), &now);
        let from = p.snapshot_ack.map_or(1, |a| a + 1).max((tick + 1).saturating_sub(RELAY)).max(1);
        let inputs = (0..n)
            .map(|j| {
                let span: Vec<WireInput> = (from..=tick).map(|t| used[t as usize - 1][j]).collect();
                InputRuns::encode(from, &span)
            })
            .collect();
        let ack = p.snapshot_ack.unwrap_or(0);
        let mut evs: Vec<Event> = events.iter().rev().take_while(|e| p.snapshot_ack.is_none() || e.tick > ack).take(32).copied().collect();
        evs.reverse();
        if let Some(l) = p.lead_min.take() {
            p.lead = l.clamp(-128, 127) as i8;
        }
        let s = Snapshot {
            tick,
            baseline: base.map_or(FULL, |b| b.0),
            input_ack: p.inputs.through,
            lead: p.lead,
            echo_ms: p.echo_ms,
            hold_ms: p.echo_at.elapsed().as_millis().min(u16::MAX as u128) as u16,
            changed,
            players: n as u8,
            pos,
            inputs,
            events: evs,
        };
        let bytes = Msg::Snapshot(s).encode();
        p.snapshots += 1;
        p.snapshot_bytes += bytes.len() as u64;
        link.send(&bytes, p.addr);
    }
}

/// Player state before a tick, to tell what the tick decided.
struct Before {
    attacking: Vec<bool>,
    airborne: Vec<bool>,
    climbing: Vec<bool>,
    pos: Vec<Vec3>,
    scripted: Vec<bool>,
}

impl Before {
    fn of(w: &World) -> Before {
        Before {
            attacking: w.players.iter().map(|p| p.kill.is_some()).collect(),
            airborne: w.players.iter().map(|p| p.loco.airborne()).collect(),
            climbing: w.players.iter().map(|p| p.climb.is_some()).collect(),
            pos: w.players.iter().map(|p| p.pos).collect(),
            scripted: w.bodies.iter().map(|b| b.scripted.is_some()).collect(),
        }
    }
}

/// Kills and stuns started, attempts refused (on the press), victims, and
/// players put back in control or moved away (respawns), from the
/// world's state around one tick. The decisions are the sim's (and its
/// rules'); this only reports them.
pub fn detect_events(w: &World, b: &Before, wire: &[WireInput], prev: &[WireInput], tick: u32) -> Vec<Event> {
    let mut out = Vec::new();
    for (i, p) in w.players.iter().enumerate() {
        let attacking = p.kill.is_some();
        let pressed = wire[i].attacks() && !prev[i].attacks();
        if attacking && !b.attacking[i] {
            let victim = w.bodies.iter().enumerate().position(|(j, body)| j != p.body && !b.scripted[j] && body.scripted.is_some());
            let stun = wire[i].stuns();
            let v = victim.map_or(u8::MAX, |v| v as u8);
            out.push(Event { tick, kind: if stun { EventKind::StunStarted } else { EventKind::KillStarted }, a: i as u8, b: v });
            if victim.is_some() {
                out.push(Event { tick, kind: if stun { EventKind::Stunned } else { EventKind::Death }, a: v, b: i as u8 });
            }
        } else if pressed && !attacking {
            let reason = if b.climbing[i] {
                Reject::Climbing
            } else if b.airborne[i] {
                Reject::Airborne
            } else {
                Reject::NoTarget
            };
            out.push(Event { tick, kind: EventKind::KillRejected, a: i as u8, b: reason as u8 });
        } else if pressed && b.attacking[i] {
            out.push(Event { tick, kind: EventKind::KillRejected, a: i as u8, b: Reject::Busy as u8 });
        }
        let freed = b.scripted.get(p.body).copied().unwrap_or(false) && w.bodies.get(p.body).is_some_and(|body| body.scripted.is_none());
        if freed || p.pos.distance(b.pos[i]) > 3.0 {
            out.push(Event { tick, kind: EventKind::Respawn, a: i as u8, b: 0 });
        }
    }
    out
}
