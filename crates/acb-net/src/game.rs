//! What server and clients share about a match: the same [`World`] built
//! from the same [`Slot`]s, players read into [`NetPos`], and the
//! server's per-player input buffer.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use acb_anim::BuiltinTracks;
use acb_game::mode::{Match, MatchConfig, MatchEvent};
use acb_game::rules::ContractEvent;
use acb_game::{Setup, World};
use acb_world::Archive;
use glam::Vec3;

use crate::proto::{AcquireState, BonusState, CrowdConfig, EscortRules, EscortState, Event, EventKind, GameState, NetPos, PlayerGame, Reject, Slot, VipState, WireInput, F_AIRBORNE, F_ATTACKING, F_BLENDED, F_CLIMBING, F_HIDDEN, F_SEATED, F_VICTIM, NONE, NONE16};

/// Builds a match world: slot `i` is player `i` on the map's player spawn
/// `i` (or its `at`) plus `crowd.count` NPCs seeded from `crowd.seed`
/// (bodies after the players); with `rules` a match on the map's player
/// spawns. Server and clients call this with the same slots and rules, so
/// both start from the same state.
pub fn load_world(archive: &Archive, slots: &[Slot], builtins: Option<&BuiltinTracks>, rules: Option<MatchConfig>, crowd: CrowdConfig) -> anyhow::Result<World> {
    load_match(archive, slots, builtins, rules, crowd, None, false)
}

/// [`load_world`], with `escort` (and `rules`) an Escort match instead of
/// the duel: teams per slot, VIP bodies and ways, rounds with `escort`'s
/// intro / recap around `rules.time_limit` of play, players on the team
/// spawns. `assassinate` (and `rules`, no `escort`): an Assassinate match.
pub fn load_match(archive: &Archive, slots: &[Slot], builtins: Option<&BuiltinTracks>, rules: Option<MatchConfig>, crowd: CrowdConfig, escort: Option<&EscortRules>, assassinate: bool) -> anyhow::Result<World> {
    let mut cache: HashMap<String, Arc<acb_anim::character::Character>> = HashMap::new();
    let mut characters = Vec::new();
    for s in slots {
        let c = match cache.get(&s.character) {
            Some(c) => c.clone(),
            None => {
                let c = Arc::new(acb_game::load_character(archive, &s.character)?);
                cache.insert(s.character.clone(), c.clone());
                c
            }
        };
        characters.push((s.character.clone(), c));
    }
    let setup = Setup {
        characters,
        count: Some(slots.len()),
        anim: "xx_l_wait_hipm_footl".into(),
        wander: 0.0,
        play: true,
        crowd: crowd.count as usize,
        pair: Vec::new(),
        at: None,
        builtins,
    };
    let mut world = World::load(archive, &setup)?;
    if world.bodies.len() < slots.len() {
        anyhow::bail!("the map has {} player spawns for {} players", world.bodies.len(), slots.len());
    }
    world.seed_crowd(crowd.seed as u64);
    world.add_players(slots.len());
    for (i, s) in slots.iter().enumerate() {
        if let Some([x, y, z, yaw]) = s.at {
            world.place_player(i, Vec3::new(x, y, z), yaw.to_radians());
        }
    }
    if let Some(cfg) = rules {
        let spawns = archive
            .player_spawns()
            .into_iter()
            .filter(|s| s.kind == 0)
            .map(|s| {
                let m = glam::Mat4::from_cols_array(&s.matrix);
                let f = m.y_axis.truncate();
                (m.w_axis.truncate(), (-f.x).atan2(f.y))
            })
            .collect();
        match escort {
            Some(e) => {
                anyhow::ensure!(e.team.len() == slots.len(), "escort: {} teams for {} slots", e.team.len(), slots.len());
                let game = if e.manhunt { acb_game::rules::TeamGame::Manhunt } else { acb_game::rules::TeamGame::Escort };
                let teams = acb_game::rules::Teams { game, team: e.team.clone(), attacking_team: e.attacking, vips: Vec::new(), escorted: Vec::new() };
                if e.manhunt {
                    world.manhunt(archive, builtins, teams, cfg, spawns);
                } else {
                    world.escort(archive, builtins, teams, cfg, spawns)?;
                }
                if let Some(m) = &mut world.mode {
                    let n = m.rounds.as_ref().map_or(2, |r| r.team_score.len());
                    m.rounds = Some(acb_game::rounds::Rounds::new(n, e.intro, cfg.time_limit, e.recap));
                }
            }
            None if assassinate => world.mode = Some(Match::assassinate(slots.len(), cfg, spawns)),
            None => world.mode = Some(Match::new(slots.len(), cfg, spawns)),
        }
    }
    Ok(world)
}

fn phase_u8(p: acb_game::rounds::Phase) -> u8 {
    use acb_game::rounds::Phase;
    match p {
        Phase::Intro => 0,
        Phase::Play => 1,
        Phase::Recap => 2,
        Phase::Over => 3,
    }
}

fn phase_of(v: u8) -> acb_game::rounds::Phase {
    use acb_game::rounds::Phase;
    match v {
        0 => Phase::Intro,
        1 => Phase::Play,
        2 => Phase::Recap,
        _ => Phase::Over,
    }
}

/// Escort rounds, team totals and VIPs as a snapshot carries them.
fn escort_state(m: &Match) -> Option<EscortState> {
    let (acb_game::mode::Mode::Escort(teams), Some(r)) = (&m.mode, &m.rounds) else { return None };
    let v = m.vips.as_ref();
    let targets = v.map(|v| v.targets()).unwrap_or_default();
    let vips = v
        .map(|v| {
            v.slots
                .iter()
                .zip(&v.bodies)
                .map(|(s, b)| match s {
                    Some(x) => VipState {
                        alive: true,
                        targetable: targets.contains(b),
                        way: x.way.min(255) as u8,
                        back: x.back,
                        checkpoints: x.checkpoints.min(255) as u8,
                        next: x.walker.next.min(u16::MAX as usize) as u16,
                        pos: x.walker.pos.to_array(),
                        yaw: x.walker.yaw,
                    },
                    None => VipState::default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(EscortState {
        round: r.round.min(255) as u8,
        phase: phase_u8(r.phase),
        left: r.left,
        attacking: teams.attacking_team,
        team_score: r.team_score.clone(),
        respawn_in: v.and_then(|v| v.respawn_in).map_or(NONE16, |t| t.min(NONE16 as u32 - 1) as u16),
        target_in: v.map_or(0, |v| v.target_in.min(u16::MAX as u32) as u16),
        spawns: v.map_or(0, |v| v.spawns.min(255) as u8),
        pool: v.map(|v| v.pool.iter().map(|&w| w.min(255) as u8).collect()).unwrap_or_default(),
        vips,
    })
}

/// Puts the server's Escort state into `m`. A VIP the server has alive
/// and we do not is re-created from its slot state (way, waypoint, pos,
/// yaw); returns those VIPs' bodies (the caller resets them).
fn apply_escort(m: &mut Match, e: &EscortState) -> Vec<usize> {
    let mut recreated = Vec::new();
    if let acb_game::mode::Mode::Escort(teams) = &mut m.mode {
        teams.attacking_team = e.attacking;
    }
    if let Some(r) = &mut m.rounds {
        r.round = e.round as u32;
        r.phase = phase_of(e.phase);
        r.left = e.left;
        for (t, s) in e.team_score.iter().enumerate().take(r.team_score.len()) {
            r.team_score[t] = *s;
        }
    }
    if let Some(v) = &mut m.vips {
        v.respawn_in = (e.respawn_in != NONE16).then_some(e.respawn_in as u32);
        v.target_in = e.target_in as u32;
        v.spawns = e.spawns as u32;
        v.pool = e.pool.iter().map(|&w| w as usize).collect();
        for (i, (slot, s)) in v.slots.iter_mut().zip(&e.vips).enumerate() {
            if !s.alive {
                *slot = None;
                continue;
            }
            let walker = acb_game::vip::Walker { pos: glam::Vec3::from_array(s.pos), yaw: s.yaw, next: s.next as usize };
            match slot {
                Some(x) => {
                    x.way = s.way as usize;
                    x.back = s.back;
                    x.checkpoints = s.checkpoints as u32;
                    x.walker = walker;
                }
                // the server has it alive and we lost it: re-create it
                // (retail Quazal::VIPdata::OnUpdate 0x0125c33a re-adds a VIP
                // the master's data has and the local list lacks)
                None => {
                    let phase = (i as f32 * 0.37).fract();
                    *slot = Some(acb_game::vip::Vip { way: s.way as usize, back: s.back, walker, checkpoints: s.checkpoints as u32, phase });
                    recreated.push(v.bodies[i]);
                }
            }
        }
    }
    recreated
}

/// The match as a snapshot carries it.
pub fn game_state(m: &Match) -> GameState {
    let t = |secs: f32| (secs * acb_game::TICK_HZ as f32).round().clamp(0.0, u16::MAX as f32) as u16;
    GameState {
        ticks_left: m.ticks_left(),
        over: m.over,
        players: m
            .contracts
            .hunters
            .iter()
            .enumerate()
            .map(|(i, h)| PlayerGame {
                target: h.target.map_or(NONE, |x| x as u8),
                score: m.score[i].min(u16::MAX as u32) as u16,
                respawn: m.respawn[i].map_or(0, |r| r.min(u16::MAX as u32) as u16),
                cooldown: t(h.cooldown),
                health: m.health[i].min(255) as u8,
                chase: m.chases[i].map_or(NONE, |c| c.phase as u8),
                escape: m.chases[i].map_or(0, |c| (c.coef.clamp(0.0, 1.0) * 65535.0).round() as u16),
                threat: (m.threat[i].clamp(0.0, 1.0) * 65535.0).round() as u16,
            })
            .collect(),
        escort: escort_state(m),
        acquire: m
            .acquire
            .iter()
            .map(|a| AcquireState {
                focus: a.focus.map_or(NONE, |x| x as u8),
                look: a.look.min(u16::MAX as u32) as u16,
                memory: a.memory.iter().map(|&(p, left)| (p as u8, left.min(u16::MAX as u32) as u16)).collect(),
                lock: a.lock.map_or(NONE, |x| x as u8),
                buttons: a.buttons,
                released: a.released.map_or((NONE, 0), |(p, left)| (p as u8, left.min(u16::MAX as u32) as u16)),
            })
            .collect(),
        bonus: m.bonus.as_ref().map(|t| {
            let u = |x: u32| x.min(u16::MAX as u32) as u16;
            let id = |x: Option<usize>| x.map_or(NONE, |x| x as u8);
            BonusState {
                first_blood: t.first_blood,
                players: (0..t.quiet.len()).map(|p| (u(t.quiet[p]), id(t.close[p].0), u(t.close[p].1), id(t.last_killer[p]))).collect(),
                memory: t.memory.iter().map(|k| (k.killer as u8, k.victim as u8, u(k.left))).collect(),
            }
        }),
    }
}

/// Puts the server's match state into `m` (after a mismatch).
/// Returns the VIP bodies re-created (see [`apply_escort`]).
pub fn apply_game_state(m: &mut Match, g: &GameState) -> Vec<usize> {
    m.ticks = acb_game::mode::ticks(m.config.time_limit).saturating_sub(g.ticks_left);
    m.over = g.over;
    for (i, p) in g.players.iter().enumerate().take(m.score.len()) {
        let h = &mut m.contracts.hunters[i];
        h.target = (p.target != NONE).then_some(p.target as usize);
        h.cooldown = p.cooldown as f32 / acb_game::TICK_HZ as f32;
        h.active = p.respawn == 0;
        m.score[i] = p.score as u32;
        m.respawn[i] = (p.respawn > 0).then_some(p.respawn as u32);
        m.health[i] = p.health as u32;
        m.threat[i] = p.threat as f32 / 65535.0;
        m.threat_of[i] = h.target;
        m.chases[i] = match (acb_game::mode::ChasePhase::from_u8(p.chase), h.target) {
            (Some(phase), Some(target)) => Some(acb_game::mode::Chase { target, coef: p.escape as f32 / 65535.0, phase }),
            _ => None,
        };
    }
    for (a, s) in m.acquire.iter_mut().zip(&g.acquire) {
        a.focus = (s.focus != NONE).then_some(s.focus as usize);
        a.look = s.look as u32;
        a.memory = s.memory.iter().map(|&(p, left)| (p as usize, left as u32)).collect();
        a.lock = (s.lock != NONE).then_some(s.lock as usize);
        a.buttons = s.buttons;
        a.released = (s.released.0 != NONE).then_some((s.released.0 as usize, s.released.1 as u32));
    }
    if let (Some(t), Some(b)) = (&mut m.bonus, &g.bonus) {
        let id = |x: u8| (x != NONE).then_some(x as usize);
        t.first_blood = b.first_blood;
        for (p, &(quiet, near, ticks, last)) in b.players.iter().enumerate().take(t.quiet.len()) {
            t.quiet[p] = quiet as u32;
            t.close[p] = (id(near), ticks as u32);
            t.last_killer[p] = id(last);
        }
        t.memory = b.memory.iter().map(|&(k, v, left)| acb_game::bonus::Memory { killer: k as usize, victim: v as usize, left: left as u32 }).collect();
    }
    g.escort.as_ref().map(|e| apply_escort(m, e)).unwrap_or_default()
}

/// The bodies of living VIPs that are not walking (re-created, or a kill
/// we predicted and the server did not have): back on their feet.
pub fn revive_vips(world: &mut acb_game::World) -> usize {
    let Some(v) = world.mode.as_ref().and_then(|m| m.vips.as_ref()) else { return 0 };
    let living: Vec<usize> = v.slots.iter().zip(&v.bodies).filter(|(s, _)| s.is_some()).map(|(_, &b)| b).collect();
    let mut n = 0;
    for b in living {
        if let Some(body) = world.bodies.get_mut(b) {
            if body.scripted.is_some() {
                body.scripted = None;
                body.agent = None;
                n += 1;
            }
        }
    }
    n
}

/// The match decisions of the last tick as events.
pub fn match_events(m: &Match, tick: u32) -> Vec<Event> {
    use acb_game::rules::AcquireEvent;
    use acb_game::rounds::RoundEvent;
    use acb_game::vip::VipEvent;
    let ev = |kind, a: usize, b: usize| Event { tick, kind, a: a.min(255) as u8, b: b.min(255) as u8 };
    let vips = m.vips.iter().flat_map(|v| v.events.iter()).filter_map(|e| match *e {
        VipEvent::Spawned { vip, way, .. } => Some(ev(EventKind::VipSpawned, vip, way)),
        VipEvent::Targetable => Some(ev(EventKind::VipTargetable, 0, 0)),
        VipEvent::Killed { vip, killer, .. } => Some(ev(EventKind::VipKilled, vip, killer)),
        VipEvent::Checkpoint { vip, n, .. } => Some(ev(EventKind::VipCheckpoint, vip, n as usize)),
        VipEvent::WayEnd { vip, n, .. } => Some(ev(EventKind::VipWayEnd, vip, n as usize)),
    });
    // after the VIP events of the tick (the escape follows the way end)
    let rounds = m.events.iter().filter_map(|e| match *e {
        MatchEvent::TeamScore { team, points, .. } => Some(ev(EventKind::TeamScore, team as usize, (points / 10) as usize)),
        MatchEvent::Round(RoundEvent::Phase { round, phase }) => Some(ev(EventKind::RoundPhase, round as usize, phase_u8(phase) as usize)),
        MatchEvent::Round(RoundEvent::Swap { round, attacking }) => Some(ev(EventKind::RoundSwap, round as usize, attacking as usize)),
        MatchEvent::Round(RoundEvent::Over { winner }) => Some(ev(EventKind::EscortOver, winner.map_or(255, |w| w as usize), 0)),
        MatchEvent::Round(RoundEvent::Escaped { round, team }) => Some(ev(EventKind::VipEscaped, team as usize, round as usize)),
        _ => None,
    });
    let mut masks: BTreeMap<usize, usize> = BTreeMap::new();
    for e in &m.events {
        if let MatchEvent::Bonus { player, bonus } = *e {
            *masks.entry(player).or_default() |= 1 << bonus as usize;
        }
    }
    let bonuses = masks.into_iter().map(|(p, mask)| ev(EventKind::Bonus, p, mask));
    m.events
        .iter()
        .filter_map(|e| match *e {
            MatchEvent::Score { player, points, .. } => Some(ev(EventKind::Score, player, (points / 10) as usize)),
            MatchEvent::Contract(ContractEvent::Assigned { hunter, target }) => Some(ev(EventKind::ContractAssigned, hunter, target)),
            MatchEvent::Contract(ContractEvent::Lost { hunter, target }) => Some(ev(EventKind::ContractLost, hunter, target)),
            MatchEvent::Contract(ContractEvent::Fulfilled { hunter, target }) => Some(ev(EventKind::ContractFulfilled, hunter, target)),
            MatchEvent::Over { winner } => Some(ev(EventKind::MatchOver, winner.map_or(255, |w| w), 0)),
            MatchEvent::ChaseStarted { hunter, target } => Some(ev(EventKind::ChaseStarted, hunter, target)),
            MatchEvent::Escaped { hunter, target } => Some(ev(EventKind::Escaped, hunter, target)),
            MatchEvent::Fall { player, damage, fatal } => Some(ev(if fatal { EventKind::FatalFall } else { EventKind::Fall }, player, damage as usize)),
            // reported by detect_events
            MatchEvent::Respawn { .. } => None,
            // below
            MatchEvent::TeamScore { .. } | MatchEvent::Round(_) => None,
            MatchEvent::Acquire(AcquireEvent::Acquiring { hunter, target }) => Some(ev(EventKind::Acquiring, hunter, target)),
            MatchEvent::Acquire(AcquireEvent::Stopped { hunter, target }) => Some(ev(EventKind::AcquireStopped, hunter, target)),
            // one event per kill (events dedup by tick, kind, player)
            MatchEvent::Bonus { .. } => None,
            MatchEvent::BloodBath { player, chain } => Some(ev(EventKind::BloodBath, player, chain as usize)),
            MatchEvent::Released { hunter, target } => Some(ev(EventKind::Released, hunter, target)),
        })
        .chain(rounds)
        .chain(vips)
        .chain(bonuses)
        .collect()
}

/// Player `i`'s action flags.
pub fn flags(world: &World, i: usize) -> u8 {
    let p = &world.players[i];
    let mut f = 0;
    if p.loco.airborne() {
        f |= F_AIRBORNE;
    }
    if p.kill.is_some() {
        f |= F_ATTACKING;
    }
    if p.climb.is_some() {
        f |= F_CLIMBING;
    }
    if world.bodies.get(p.body).is_some_and(|b| b.scripted.is_some()) {
        f |= F_VICTIM;
    }
    if p.hidden {
        f |= F_HIDDEN;
    }
    if p.blended {
        f |= F_BLENDED;
    }
    if p.hide.as_ref().is_some_and(|h| h.sit.is_some()) {
        f |= F_SEATED;
    }
    f
}

/// Player `i` as a snapshot carries it.
pub fn net_pos(world: &World, i: usize) -> NetPos {
    let p = &world.players[i];
    let vel = (p.pos - p.prev_pos) * acb_game::TICK_HZ as f32;
    NetPos::new(p.pos, p.yaw, vel, flags(world, i))
}

pub fn net_positions(world: &World) -> Vec<NetPos> {
    (0..world.players.len()).map(|i| net_pos(world, i)).collect()
}

/// The server's inputs from one client: what arrived for which tick,
/// which ones it has without a gap, and what it applies when an input is
/// missing at its tick (the last one again).
#[derive(Default)]
pub struct InputBuffer {
    pending: BTreeMap<u32, WireInput>,
    /// Every tick up to this has arrived (or was played already).
    pub through: u32,
    /// The input applied last.
    pub last: WireInput,
    /// Ticks played with a repeated input because theirs had not arrived.
    pub missed: u64,
    /// Inputs that arrived after their tick was played.
    pub late: u64,
    /// Inputs received, counting each tick once.
    pub received: u64,
}

impl InputBuffer {
    /// Takes in a packet's inputs for ticks `last - n + 1 ..= last` while
    /// the server is at `played` (ticks up to it are done).
    pub fn receive(&mut self, last: u32, inputs: &[WireInput], played: u32) {
        let first = (last + 1).saturating_sub(inputs.len() as u32);
        for (k, i) in inputs.iter().enumerate() {
            let t = first + k as u32;
            if t == 0 || t <= self.through || self.pending.contains_key(&t) {
                continue;
            }
            // one that is too late is kept only to close the gap for the ack
            if t <= played {
                self.late += 1;
            } else {
                self.received += 1;
            }
            self.pending.insert(t, *i);
        }
        while self.pending.contains_key(&(self.through + 1)) {
            self.through += 1;
        }
        self.prune(played);
    }

    fn prune(&mut self, played: u32) {
        let through = self.through;
        self.pending.retain(|&t, _| t > through || t > played);
    }

    /// The input to apply at `tick` (ticks are taken in order).
    pub fn take(&mut self, tick: u32) -> WireInput {
        match self.pending.get(&tick) {
            Some(i) => self.last = *i,
            None => self.missed += 1,
        }
        self.prune(tick);
        self.last
    }
}

/// One authoritative tick: every player's kill/stun request is checked
/// with [`World::validate_action`] (the rules hook); a refused one is
/// dropped from `wire` (the inputs as applied, which the server relays, so
/// clients replaying them undo their predicted kill) and reported on the
/// press (`prev`: last tick's inputs as sent, updated). Then the world steps and the decided kills, deaths and respawns
/// are returned with the rejections.
pub fn authoritative_step(world: &mut World, wire: &mut [WireInput], prev: &mut Vec<WireInput>, tick: u32) -> Vec<Event> {
    let mut out = Vec::new();
    // presses count against the inputs as sent, not as applied
    let raw = wire.to_vec();
    // a match: kills only on the target, stuns only on a pursuer (contracts)
    let (rules, mode) = match &world.mode {
        Some(m) => (m.rules(), Some(m.clone())),
        None => (acb_game::rules::Rules::default(), None),
    };
    let relation = |i: usize, j: usize| mode.as_ref().map_or(acb_game::rules::Relation::Npc, |m| m.relation(i, j));
    for (i, w) in wire.iter_mut().enumerate() {
        if !w.attacks() {
            continue;
        }
        let pressed = !prev.get(i).is_some_and(|p| p.attacks());
        let busy = world.players.get(i).is_some_and(|p| p.kill.is_some() || p.approach.is_some());
        if busy {
            if pressed {
                out.push(Event { tick, kind: EventKind::KillRejected, a: i as u8, b: Reject::Busy as u8 });
            }
            continue;
        }
        if let Err(r) = world.validate_action(i, &w.to_input(), &rules, &relation) {
            w.clear_attacks();
            if pressed {
                out.push(Event { tick, kind: EventKind::KillRejected, a: i as u8, b: Reject::from_rules(r) as u8 });
            }
        }
    }
    *prev = raw;
    let inputs: Vec<_> = wire.iter().map(|w| w.to_input()).collect();
    let before = Before::of(world);
    world.step(&inputs);
    out.extend(detect_events(world, &before, wire, tick));
    if let Some(m) = &world.mode {
        out.extend(match_events(m, tick));
    }
    // one tick's events in (kind, player) order (clients skip what they saw)
    out.sort_by_key(|e| (e.tick, e.kind as u8, e.a));
    out.dedup_by_key(|e| (e.tick, e.kind as u8, e.a));
    out
}

/// Player state before a tick, to tell what the tick decided.
pub struct Before {
    attacking: Vec<bool>,
    pos: Vec<Vec3>,
    scripted: Vec<bool>,
}

impl Before {
    pub fn of(w: &World) -> Before {
        Before {
            attacking: w.players.iter().map(|p| p.kill.is_some()).collect(),
            pos: w.players.iter().map(|p| p.pos).collect(),
            scripted: w.bodies.iter().map(|b| b.scripted.is_some()).collect(),
        }
    }
}

/// Kills and stuns started, victims, and
/// players put back in control or moved away (respawns), from the
/// world's state around one tick. The decisions are the sim's (and its
/// rules'); this only reports them.
pub fn detect_events(w: &World, b: &Before, wire: &[WireInput], tick: u32) -> Vec<Event> {
    let mut out = Vec::new();
    for (i, p) in w.players.iter().enumerate() {
        let attacking = p.kill.is_some();
        if attacking && !b.attacking[i] {
            let victim = w.bodies.iter().enumerate().position(|(j, body)| j != p.body && !b.scripted[j] && body.scripted.is_some());
            let stun = wire[i].stuns();
            let v = victim.map_or(u8::MAX, |v| v as u8);
            out.push(Event { tick, kind: if stun { EventKind::StunStarted } else { EventKind::KillStarted }, a: i as u8, b: v });
            if victim.is_some() {
                out.push(Event { tick, kind: if stun { EventKind::Stunned } else { EventKind::Death }, a: v, b: i as u8 });
            }
        }
        let freed = b.scripted.get(p.body).copied().unwrap_or(false) && w.bodies.get(p.body).is_some_and(|body| body.scripted.is_none());
        if freed || p.pos.distance(b.pos[i]) > 3.0 {
            out.push(Event { tick, kind: EventKind::Respawn, a: i as u8, b: 0 });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(t: u32) -> WireInput {
        WireInput { buttons: (t % 7) as u16, yaw: t as u16 }
    }

    /// A client sending every tick's input together with the ones the
    /// server has not acknowledged (up to `MAX_INPUTS`): with half the
    /// packets lost, every input still reaches the server before its tick.
    #[test]
    fn redundant_inputs_survive_loss() {
        let mut buf = InputBuffer::default();
        let mut rng = 0x1234_5678u32;
        let mut lost = 0;
        let lead = 4; // client ticks ahead of the server
        let mut ack = 0; // the server's `through`, as the client last heard it
        let mut applied = Vec::new();
        for client_tick in 1..=600u32 {
            let from = ack + 1;
            let first = from.max((client_tick + 1).saturating_sub(crate::proto::MAX_INPUTS as u32));
            let inputs: Vec<_> = (first..=client_tick).map(input).collect();
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let server_tick = client_tick.saturating_sub(lead);
            if rng % 2 == 0 {
                buf.receive(client_tick, &inputs, server_tick.saturating_sub(1));
            } else {
                lost += 1;
            }
            if server_tick > 0 {
                applied.push((server_tick, buf.take(server_tick)));
            }
            // acks come back with the snapshots, also lossy
            if rng % 3 != 0 {
                ack = buf.through;
            }
        }
        assert!(lost > 250, "{lost}");
        // a loss burst longer than the lead can make an input late; count
        let wrong = applied.iter().filter(|(t, i)| *i != input(*t)).count();
        assert_eq!(wrong as u64, buf.missed);
        assert!(buf.missed < 30, "{} of {} ticks missed", buf.missed, applied.len());
        assert!(buf.through >= 590);
    }

    #[test]
    fn inputs_without_loss_arrive_once() {
        let mut buf = InputBuffer::default();
        for t in 1..=100u32 {
            // the last 3 every packet
            let first = t.saturating_sub(2).max(1);
            let inputs: Vec<_> = (first..=t).map(input).collect();
            buf.receive(t, &inputs, t.saturating_sub(3));
            if t > 2 {
                assert_eq!(buf.take(t - 2), input(t - 2));
            }
        }
        assert_eq!(buf.missed, 0);
        assert_eq!(buf.late, 0);
        assert_eq!(buf.received, 100);
        assert_eq!(buf.through, 100);
    }

    #[test]
    fn missing_input_repeats_the_last() {
        let mut buf = InputBuffer::default();
        buf.receive(1, &[input(1)], 0);
        assert_eq!(buf.take(1), input(1));
        assert_eq!(buf.take(2), input(1));
        assert_eq!(buf.missed, 1);
        // arriving now is too late for tick 2, but fills the ack
        buf.receive(3, &[input(2), input(3)], 2);
        assert_eq!(buf.late, 1);
        assert_eq!(buf.through, 3);
        assert_eq!(buf.take(3), input(3));
    }

    /// A client that lost a VIP the server still has (desync) gets it back
    /// from the snapshot: same slot state, its body reported for reset; a
    /// VIP the server has dead goes.
    #[test]
    fn lost_vip_is_recreated() {
        use acb_game::vip::{Vip, Vips, Walker};
        let cfg = MatchConfig { time_limit: 10.0, score_limit: 0, respawn_delay: 1.0, start_delay: 0.0 };
        let make = || {
            let mut m = Match::escort(acb_game::rules::Teams::alternating(2), cfg, vec![(Vec3::ZERO, 0.0)]);
            m.vips = Some(Vips::new(vec![7, 8]));
            m
        };
        let mut server = make();
        let v = server.vips.as_mut().unwrap();
        v.pool = vec![3, 1];
        v.slots[1] = Some(Vip { way: 2, back: true, walker: Walker { pos: Vec3::new(1.0, 2.0, 3.0), yaw: 0.5, next: 4 }, checkpoints: 2, phase: 0.1 });
        let mut client = make();
        client.vips.as_mut().unwrap().slots[0] = server.vips.as_ref().unwrap().slots[1].clone();
        let g = game_state(&server);
        assert_ne!(game_state(&client), g);
        assert_eq!(apply_game_state(&mut client, &g), vec![8]);
        assert_eq!(game_state(&client), g);
        let c = client.vips.as_ref().unwrap();
        assert!(c.slots[0].is_none());
        let x = c.slots[1].as_ref().unwrap();
        assert_eq!((x.way, x.back, x.checkpoints, x.walker.next, x.walker.yaw), (2, true, 2, 4, 0.5));
        assert_eq!(c.pool, vec![3, 1]);
        // in step again: nothing more to re-create
        assert!(apply_game_state(&mut client, &g).is_empty());
    }
}
