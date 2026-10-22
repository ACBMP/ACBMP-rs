//! The client: joins a server, predicts its own player by running
//! [`World::step`] with its inputs ahead of the server, and reconciles
//! with every snapshot.
//!
//! The server relays the inputs it applied for every player. The client
//! keeps a confirmed copy of the world ([`Saved`]) at the newest tick it
//! has every player's applied inputs for, steps it with exactly those
//! (the same deterministic sim the server runs) and checks the result
//! against the snapshot's quantized positions. Then it rewinds the
//! predicted world to the confirmed one and replays its own inputs the
//! server has not applied yet (remote players repeat their last applied
//! input). A kill the server refused is not in the applied inputs, so the
//! replay undoes the predicted kill: that is the rollback.
//!
//! Remote players are drawn from the confirmed ticks, ~100 ms behind the
//! newest one, interpolated (placement and pose) between ticks.
//!
//! The clock: the client runs `lead` ticks ahead of the server so its
//! input for a tick arrives before the server plays it (a jitter buffer
//! of a few ticks); each snapshot says by how much the newest input was
//! early, and the client clock runs a little faster or slower to keep it
//! in [`LEAD_MIN`, `LEAD_MAX`].

use std::collections::{BTreeMap, VecDeque};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use acb_game::state::Saved;
use acb_game::{Pose, PlayerInput, World, TICK_HZ};
use glam::Mat4;

use crate::game;
use crate::link::{Link, NetSim};
use crate::proto::*;

/// Ticks early the client aims its inputs at the server.
const LEAD_MIN: i32 = 2;
const LEAD_MAX: i32 = 5;
/// Remote players are drawn this many ticks behind the newest confirmed
/// tick (100 ms).
const INTERP_TICKS: f64 = 6.0;
/// Confirmed ticks of remote placements kept for drawing.
const RING: usize = 60;
/// Server silent this long: the connection is lost.
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct ClientConfig {
    /// host:port
    pub server: String,
    pub name: String,
    pub character: String,
    /// Map file name, e.g. DataPC_AC2MP_SanDonato.forge
    pub map: String,
    pub netsim: NetSim,
}

/// What the server said when the match started.
pub struct Start {
    pub slot: usize,
    pub slots: Vec<Slot>,
    pub snapshot_every: u8,
    /// The match (`None`: free play); pass to [`game::load_world`].
    pub rules: Option<acb_game::mode::MatchConfig>,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct ClientStats {
    pub snapshots: u64,
    /// Confirmed ticks checked against the server's positions, and the
    /// ones that differed.
    pub checked: u64,
    /// Checked ticks where every player's quantized state was identical.
    pub exact: u64,
    pub desyncs: u64,
    /// Predictions the reconciliation changed (own position > 1 mm or
    /// action state), and kills it undid.
    pub corrections: u64,
    pub rollbacks: u64,
    pub rejected: u64,
    /// Snapshot match states compared / different (overwritten).
    pub game_checked: u64,
    pub game_desyncs: u64,
}

pub struct Client {
    link: Link,
    server: SocketAddr,
    pub slot: usize,
    n: usize,
    epoch: Instant,
    /// Round-trip time, seconds (smoothed).
    pub rtt: f32,
    rtt_measured: bool,
    loaded_ms: u32,
    heard: Instant,
    /// Predicted ticks, fractional; `None` until the first snapshot.
    clock: Option<f64>,
    rate: f64,
    /// After a clock jump: lead reports are stale until the server has
    /// our inputs through this tick.
    jumped: u32,
    last_frame: Instant,
    last_ready: Instant,
    confirmed: Option<Saved>,
    pub confirmed_tick: u32,
    /// Per player: inputs the server applied after `confirmed_tick`.
    applied: Vec<BTreeMap<u32, WireInput>>,
    /// Per player: the input applied at `confirmed_tick` (remote players
    /// are predicted to hold it).
    last_applied: Vec<WireInput>,
    /// Own inputs not yet confirmed or not yet acknowledged.
    own: BTreeMap<u32, WireInput>,
    input_ack: u32,
    snapshot_ack: u32,
    newest_snapshot: Option<u32>,
    /// Decoded snapshot positions (delta baselines).
    snaps: VecDeque<(u32, Vec<NetPos>)>,
    /// Snapshot positions the confirmed world has not reached yet.
    pending_check: BTreeMap<u32, Vec<NetPos>>,
    /// Snapshot match states the confirmed world has not reached yet.
    pending_game: BTreeMap<u32, GameState>,
    /// The newest match state from the server (HUD) and its tick.
    pub game: Option<(u32, GameState)>,
    /// The last event that concerns this player, for the HUD.
    pub last_message: Option<String>,
    /// Per player: (tick, placement, pose) of the confirmed ticks.
    ring: Vec<VecDeque<(u32, Mat4, Pose)>>,
    interp: f64,
    last_event: Option<(u32, usize)>,
    pub events: Vec<Event>,
    pub stats: ClientStats,
    /// Set when the server ended the match or went silent.
    pub over: Option<String>,
}

fn trace() -> bool {
    acb_game::trace()
}

impl Client {
    /// Joins `cfg.server` and waits (up to `wait`) for the match to start.
    pub fn connect(cfg: &ClientConfig, wait: Duration) -> anyhow::Result<(Client, Start)> {
        let server = cfg.server.to_socket_addrs()?.next().ok_or_else(|| anyhow::anyhow!("{}: no address", cfg.server))?;
        let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
        let sock = UdpSocket::bind(bind)?;
        let seed = std::process::id() as u64 ^ 0x9e37_79b9_7f4a_7c15;
        let mut link = Link::new(sock, cfg.netsim, seed)?;
        let epoch = Instant::now();
        let hello = Msg::Hello { name: cfg.name.clone(), character: cfg.character.clone(), map: cfg.map.clone() }.encode();
        let deadline = Instant::now() + wait;
        let mut slot = None;
        let mut rtt: Option<f32> = None;
        let mut last_send = Instant::now() - Duration::from_secs(1);
        loop {
            anyhow::ensure!(Instant::now() < deadline, "no match started by {} within {:?}", cfg.server, wait);
            if last_send.elapsed() > Duration::from_millis(250) {
                if slot.is_none() {
                    link.send(&hello, server);
                    link.send(&Msg::Ping { client_ms: epoch.elapsed().as_millis() as u32 }.encode(), server);
                } else {
                    link.send(&Msg::Ping { client_ms: epoch.elapsed().as_millis() as u32 }.encode(), server);
                }
                last_send = Instant::now();
            }
            let Some((data, from)) = link.recv(Duration::from_millis(50)) else { continue };
            if from != server {
                continue;
            }
            match Msg::decode(&data) {
                Some(Msg::Welcome { slot: s, players }) => {
                    if slot.is_none() {
                        eprintln!("net: joined {} as slot {s} of {players}", cfg.server);
                    }
                    slot = Some(s as usize);
                }
                Some(Msg::Reject { reason }) => anyhow::bail!("server refused: {reason}"),
                Some(Msg::Lobby { joined, players, echo_ms }) => {
                    if echo_ms != 0 {
                        let r = (epoch.elapsed().as_millis() as u32).wrapping_sub(echo_ms) as f32 / 1000.0;
                        rtt = Some(rtt.map_or(r, |o| o * 0.7 + r * 0.3));
                    } else {
                        eprintln!("net: lobby {joined}/{players}");
                    }
                }
                Some(Msg::Start { map, snapshot_every, slots, rules }) => {
                    let Some(slot) = slot else { continue };
                    anyhow::ensure!(map.eq_ignore_ascii_case(&cfg.map), "server started {map}");
                    let n = slots.len();
                    let now = Instant::now();
                    let client = Client {
                        link,
                        server,
                        slot,
                        n,
                        epoch,
                        rtt: rtt.unwrap_or(0.05),
                        rtt_measured: rtt.is_some(),
                        loaded_ms: u32::MAX,
                        heard: now,
                        clock: None,
                        rate: 1.0,
                        jumped: 0,
                        last_frame: now,
                        last_ready: now - Duration::from_secs(1),
                        confirmed: None,
                        confirmed_tick: 0,
                        applied: vec![BTreeMap::new(); n],
                        last_applied: vec![WireInput::default(); n],
                        own: BTreeMap::new(),
                        input_ack: 0,
                        snapshot_ack: 0,
                        newest_snapshot: None,
                        snaps: VecDeque::new(),
                        pending_check: BTreeMap::new(),
                        pending_game: BTreeMap::new(),
                        game: None,
                        last_message: None,
                        ring: vec![VecDeque::new(); n],
                        interp: 0.0,
                        last_event: None,
                        events: Vec::new(),
                        stats: ClientStats::default(),
                        over: None,
                    };
                    return Ok((client, Start { slot, slots, snapshot_every, rules }));
                }
                _ => {}
            }
        }
    }

    fn ms(&self) -> u32 {
        self.epoch.elapsed().as_millis() as u32
    }

    /// Whether the match runs (the first snapshot arrived).
    pub fn running(&self) -> bool {
        self.clock.is_some()
    }

    /// Takes in packets, reconciles `world` (built by
    /// [`game::load_world`] from the [`Start`] slots) and runs the ticks
    /// due by the client clock, asking `input(tick)` for the own player's
    /// input of each. Returns how far the clock is between the last two
    /// predicted ticks (render interpolation; 1 = on the last tick).
    pub fn advance(&mut self, world: &mut World, mut input: impl FnMut(u32) -> PlayerInput) -> f32 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f64().min(0.25);
        self.last_frame = now;
        if self.confirmed.is_none() && world.tick == 0 {
            // first call: the world is loaded (that took a while)
            self.confirmed = Some(world.save());
            self.heard = now;
            self.loaded_ms = self.ms();
        }
        if !self.running() && self.last_ready.elapsed() > Duration::from_millis(250) {
            self.link.send(&Msg::Ready.encode(), self.server);
            // the round trip, for where to start the clock
            self.link.send(&Msg::Ping { client_ms: self.ms() }.encode(), self.server);
            self.last_ready = Instant::now();
        }
        while let Some((data, from)) = self.link.recv(Duration::ZERO) {
            if from != self.server {
                continue;
            }
            self.heard = Instant::now();
            match Msg::decode(&data) {
                Some(Msg::Snapshot(s)) => self.snapshot(world, s),
                Some(Msg::Over { reason }) => self.over = Some(reason),
                // (only pings sent since the world loaded)
                Some(Msg::Lobby { echo_ms, .. }) if echo_ms != 0 && echo_ms >= self.loaded_ms => {
                    let r = self.ms().wrapping_sub(echo_ms) as f32 / 1000.0;
                    if r < 5.0 {
                        self.rtt = if self.rtt_measured { self.rtt * 0.7 + r * 0.3 } else { r };
                        self.rtt_measured = true;
                    }
                }
                _ => {}
            }
        }
        if self.heard.elapsed() > TIMEOUT && self.over.is_none() {
            self.over = Some("server timed out".into());
        }
        let Some(clock) = &mut self.clock else { return 1.0 };
        *clock += dt * TICK_HZ as f64 * self.rate;
        let clock = *clock;
        self.interp += dt * TICK_HZ as f64;
        let target = self.confirmed_tick as f64 - INTERP_TICKS;
        if (self.interp - target).abs() > 2.0 * INTERP_TICKS {
            self.interp = target;
        } else {
            self.interp += (target - self.interp) * (1.0 - (-dt * 4.0).exp());
        }
        let mut stepped = false;
        while (world.tick as f64) + 1.0 <= clock {
            let t = world.tick as u32 + 1;
            let w = WireInput::from_input(&input(t));
            self.own.insert(t, w);
            let inputs = self.predicted_inputs(t);
            world.step(&inputs);
            stepped = true;
        }
        // one packet per frame with every unacknowledged input (a burst of
        // ticks in one frame must not read as late inputs)
        if stepped {
            self.send_inputs(world.tick as u32);
        }
        ((clock - world.tick as f64) as f32 + 1.0).clamp(0.0, 1.0)
    }

    /// Everyone's input for predicted tick `t`.
    fn predicted_inputs(&self, t: u32) -> Vec<PlayerInput> {
        (0..self.n)
            .map(|j| {
                let w = if j == self.slot {
                    self.own.range(..=t).next_back().map_or(WireInput::default(), |e| *e.1)
                } else {
                    self.last_applied[j]
                };
                w.to_input()
            })
            .collect()
    }

    fn send_inputs(&mut self, last: u32) {
        let first = (self.input_ack + 1).max((last + 1).saturating_sub(MAX_INPUTS as u32)).max(1);
        let inputs: Vec<WireInput> = (first..=last).map(|t| self.own.get(&t).copied().unwrap_or_default()).collect();
        let msg = Msg::Input { snapshot_ack: self.snapshot_ack, client_ms: self.ms(), last, inputs };
        self.link.send(&msg.encode(), self.server);
    }

    fn snapshot(&mut self, world: &mut World, s: Snapshot) {
        if self.newest_snapshot.is_some_and(|n| s.tick <= n) {
            return;
        }
        self.newest_snapshot = Some(s.tick);
        self.stats.snapshots += 1;
        // positions
        let base = if s.baseline == FULL { None } else { self.snaps.iter().find(|b| b.0 == s.baseline).map(|b| b.1.as_slice()) };
        if let Some(pos) = apply_delta(base, &s).filter(|_| s.baseline == FULL || base.is_some()) {
            self.snapshot_ack = s.tick;
            self.snaps.push_back((s.tick, pos.clone()));
            while self.snaps.len() > 64 {
                self.snaps.pop_front();
            }
            if s.tick > self.confirmed_tick {
                self.pending_check.insert(s.tick, pos);
            }
        }
        if let Some(g) = &s.game {
            if s.tick > self.confirmed_tick {
                self.pending_game.insert(s.tick, g.clone());
            }
            self.game = Some((s.tick, g.clone()));
        }
        // applied inputs
        for (j, runs) in s.inputs.iter().enumerate().take(self.n) {
            for (t, w) in runs.ticks() {
                if t > self.confirmed_tick {
                    self.applied[j].insert(t, w);
                }
            }
        }
        self.input_ack = self.input_ack.max(s.input_ack);
        // round trip and clock
        if s.echo_ms != 0 {
            let r = self.ms().wrapping_sub(s.echo_ms).saturating_sub(s.hold_ms as u32) as f32 / 1000.0;
            if r < 5.0 {
                self.rtt = self.rtt * 0.9 + r * 0.1;
            }
        }
        match self.clock {
            None => {
                // the server is about half a round trip past this tick
                let ahead = self.rtt as f64 * TICK_HZ as f64 + (LEAD_MIN + LEAD_MAX) as f64 / 2.0;
                self.clock = Some(s.tick as f64 + ahead);
                self.interp = s.tick as f64 - INTERP_TICKS;
                eprintln!("net: match running, rtt {:.0} ms, {ahead:.1} ticks ahead", self.rtt * 1000.0);
            }
            Some(ref mut c) if s.input_ack > 0 && s.input_ack >= self.jumped => {
                let lead = s.lead as i32;
                if lead < -LEAD_MAX || lead > 4 * LEAD_MAX {
                    // far off: jump (ahead: the server repeated our input;
                    // back: the predicted world waits for the clock)
                    *c += ((LEAD_MIN + LEAD_MAX) / 2 - lead) as f64;
                    // (the burst of inputs right after still reads as late)
                    self.jumped = c.max(world.tick as f64).ceil() as u32 + 1;
                    if trace() {
                        eprintln!("net: clock jump by {} ticks (lead {lead})", (LEAD_MIN + LEAD_MAX) / 2 - lead);
                    }
                }
                self.rate = if lead < LEAD_MIN {
                    1.03
                } else if lead > LEAD_MAX {
                    0.97
                } else {
                    1.0
                };
            }
            _ => {}
        }
        // events
        for e in &s.events {
            let key = (e.tick, e.kind as usize * 256 + e.a as usize);
            if self.last_event.is_some_and(|l| key <= l) {
                continue;
            }
            self.last_event = Some(key);
            if e.kind == EventKind::KillRejected && e.a as usize == self.slot {
                self.stats.rejected += 1;
            }
            let mine = e.a as usize == self.slot || (matches!(e.kind, EventKind::Death | EventKind::Stunned | EventKind::ContractLost | EventKind::ContractFulfilled) && e.b as usize == self.slot) || e.kind == EventKind::MatchOver;
            if trace() || mine {
                eprintln!("net: event {}", e.describe());
            }
            if mine {
                self.last_message = Some(self.message(e));
            }
            self.events.push(*e);
        }
        self.reconcile(world, s.tick);
    }

    /// Steps the confirmed world through every tick it has all applied
    /// inputs for (up to `upto`), then replays the prediction on top.
    fn reconcile(&mut self, world: &mut World, upto: u32) {
        let Some(confirmed) = &self.confirmed else { return };
        let mut end = self.confirmed_tick;
        while end < upto && self.applied.iter().all(|a| a.contains_key(&(end + 1))) {
            end += 1;
        }
        if end == self.confirmed_tick {
            return;
        }
        let predicted_tick = world.tick as u32;
        let me = self.slot;
        let before = world.players.get(me).map(|p| (p.pos, p.kill.is_some() || p.approach.is_some()));
        // the confirmed ticks
        world.restore(confirmed);
        for t in self.confirmed_tick + 1..=end {
            let wire: Vec<WireInput> = (0..self.n).map(|j| self.applied[j].remove(&t).unwrap_or_default()).collect();
            let inputs: Vec<_> = wire.iter().map(|w| w.to_input()).collect();
            world.step(&inputs);
            self.last_applied = wire;
            for j in 0..self.n {
                let Some(b) = world.players.get(j).and_then(|p| world.bodies.get(p.body)) else { continue };
                let r = &mut self.ring[j];
                r.push_back((t, b.world, b.pose.clone()));
                while r.len() > RING {
                    r.pop_front();
                }
            }
            if let Some(server) = self.pending_check.remove(&t) {
                self.check(world, t, &server);
            }
            if let Some(server) = self.pending_game.remove(&t) {
                self.check_game(world, t, &server);
            }
        }
        self.pending_check.retain(|&k, _| k > end);
        self.pending_game.retain(|&k, _| k > end);
        self.confirmed_tick = end;
        self.confirmed = Some(world.save());
        self.own.retain(|&k, _| k > end.min(self.input_ack));
        // the prediction again on top
        for t in end + 1..=predicted_tick {
            let inputs = self.predicted_inputs(t);
            world.step(&inputs);
        }
        let after = world.players.get(me).map(|p| (p.pos, p.kill.is_some() || p.approach.is_some()));
        if let (Some((p0, k0)), Some((p1, k1))) = (before, after) {
            let err = p0.distance(p1);
            if err > 1e-3 || k0 != k1 {
                self.stats.corrections += 1;
                if k0 && !k1 {
                    self.stats.rollbacks += 1;
                    eprintln!("net: rollback: predicted kill undone (server tick {end}, predicted {predicted_tick})");
                }
                if trace() {
                    eprintln!("net: correction at {predicted_tick} from {end}: own pos off {err:.3} m, action {k0} -> {k1}");
                }
            }
        }
    }

    /// Compares the confirmed match at `t` with the server's; the server's
    /// wins.
    fn check_game(&mut self, world: &mut World, t: u32, server: &GameState) {
        let Some(m) = &mut world.mode else { return };
        self.stats.game_checked += 1;
        let ours = game::game_state(m);
        if &ours != server {
            self.stats.game_desyncs += 1;
            if self.stats.game_desyncs < 20 {
                eprintln!("net: match desync tick {t}: ours {ours:?} server {server:?}");
            }
            game::apply_game_state(m, server);
        }
    }

    /// Compares the confirmed world at `t` with the server's positions.
    fn check(&mut self, world: &mut World, t: u32, server: &[NetPos]) {
        self.stats.checked += 1;
        let ours = game::net_positions(world);
        let mut off = false;
        for (j, (a, b)) in ours.iter().zip(server).enumerate() {
            if !a.close_to(b) {
                off = true;
                if self.stats.desyncs < 20 {
                    eprintln!("net: desync tick {t} player {j}: ours {:?} server {:?}", a.pos(), b.pos());
                }
                // the server is right: put the player where it says
                if let Some(p) = world.players.get_mut(j) {
                    p.pos = b.pos();
                    p.yaw = b.yaw();
                    p.prev_pos = p.pos;
                }
            }
        }
        if off {
            self.stats.desyncs += 1;
        }
        if ours.as_slice() == server {
            self.stats.exact += 1;
        }
        if trace() && t % 30 == 0 {
            for (j, q) in server.iter().enumerate() {
                eprintln!("net: confirmed {t} player {j} pos {:.2} {:.2} {:.2} yaw {} flags {}", q.pos().x, q.pos().y, q.pos().z, q.yaw, q.flags);
            }
        }
    }

    /// An event in the own player's words (HUD).
    pub fn message(&self, e: &Event) -> String {
        let me = self.slot as u8;
        match e.kind {
            EventKind::KillRejected => format!("kill rejected: {}", crate::proto::Reject::name(e.b)),
            EventKind::KillStarted if e.a == me => format!("killing player {}", e.b),
            EventKind::StunStarted if e.a == me => format!("stunning player {}", e.b),
            EventKind::Death if e.a == me => format!("killed by player {}", e.b),
            EventKind::Stunned if e.a == me => format!("stunned by player {}", e.b),
            EventKind::Respawn if e.a == me => "respawned".into(),
            EventKind::Score => format!("+{} points", e.b as u32 * 10),
            EventKind::ContractAssigned => format!("new target: player {}", e.b),
            EventKind::ContractLost if e.a == me => format!("contract on player {} lost", e.b),
            EventKind::ContractLost => format!("player {} lost you", e.a),
            EventKind::ContractFulfilled if e.a == me => format!("contract on player {} fulfilled", e.b),
            EventKind::ContractFulfilled => format!("killed by player {} (its target)", e.a),
            EventKind::MatchOver if e.a == crate::proto::NONE => "match over: draw".into(),
            EventKind::MatchOver if e.a == me => "match over: you win".into(),
            EventKind::MatchOver => format!("match over: player {} wins", e.a),
            _ => e.describe(),
        }
    }

    /// The HUD line for the own player from the predicted match state
    /// (a pure function of the sim tick, so the same at any frame rate):
    /// target and its distance with the compass fill, scores, time left,
    /// respawn countdown and the last event that concerned this player.
    /// `None` outside a match.
    pub fn hud(&self, world: &World) -> Option<String> {
        let m = world.mode.as_ref()?;
        let me = self.slot;
        let pos = |p: usize| world.players.get(p).map(|p| p.pos).unwrap_or_default();
        let mut s = match (m.respawn.get(me).copied().flatten(), m.contracts.hunters.get(me).and_then(|h| h.target)) {
            (Some(r), _) => format!("respawn in {:.1} s", r as f32 / TICK_HZ as f32),
            (None, Some(t)) => {
                let l = acb_game::rules::locate(pos(me), pos(t), false, false);
                format!("target p{t} {:.1} m compass {:.0}%", l.dist, l.fill * 100.0)
            }
            (None, None) => "no target".into(),
        };
        let pursued = (0..m.contracts.hunters.len()).filter(|&h| h != me && m.contracts.hunters[h].target == Some(me)).count();
        if pursued > 0 {
            s += &format!(" | pursued x{pursued}");
        }
        let scores: Vec<String> = m.score.iter().enumerate().map(|(i, v)| if i == me { format!("[p{i} {v}]") } else { format!("p{i} {v}") }).collect();
        let left = m.ticks_left() / TICK_HZ;
        s += &format!(" | {} | {}:{:02}", scores.join(" "), left / 60, left % 60);
        if let Some(msg) = &self.last_message {
            s += &format!(" | {msg}");
        }
        Some(s)
    }

    /// Placement and pose to draw remote player `j` with: its confirmed
    /// ticks ~100 ms behind the newest, interpolated. `None` for the own
    /// player (draw the predicted body) or before there is any.
    pub fn remote_view(&self, j: usize) -> Option<(Mat4, Pose)> {
        if j == self.slot {
            return None;
        }
        let r = self.ring.get(j)?;
        let (first, last) = (r.front()?, r.back()?);
        let at = self.interp.clamp(first.0 as f64, last.0 as f64);
        let i = r.partition_point(|e| (e.0 as f64) <= at);
        let a = &r[i.saturating_sub(1)];
        let Some(b) = r.get(i) else { return Some((a.1, a.2.clone())) };
        let alpha = ((at - a.0 as f64) / (b.0 - a.0).max(1) as f64) as f32;
        let (s0, r0, t0) = a.1.to_scale_rotation_translation();
        let (s1, r1, t1) = b.1.to_scale_rotation_translation();
        let m = Mat4::from_scale_rotation_translation(s0.lerp(s1, alpha), r0.slerp(r1, alpha), t0.lerp(t1, alpha));
        let mut pose = a.2.clone();
        acb_anim::pose::blend(&mut pose, &b.2, alpha);
        Some((m, pose))
    }

    /// The tick remote players are drawn at.
    pub fn remote_tick(&self) -> f64 {
        self.interp
    }

    /// Tells the server we leave.
    pub fn leave(&mut self) {
        for _ in 0..3 {
            self.link.send(&Msg::Bye.encode(), self.server);
        }
    }

    pub fn link_stats(&self) -> crate::link::Stats {
        self.link.stats
    }
}
