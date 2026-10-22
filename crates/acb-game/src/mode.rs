//! A Wanted-style match inside the deterministic sim (a duel with two
//! players; nothing here assumes two): [`Contracts`] decide who may kill
//! or stun whom, kills and stuns score, killed players respawn on a player
//! spawn after a delay, the match ends on a time or score limit.
//!
//! It lives in [`crate::World`] (and its [`crate::state::Saved`]) so the
//! server and every client's prediction/replay step the same match; no
//! clock, no RNG, ties by index.

use glam::Vec3;

use crate::rules::{Contracts, ContractEvent, Relation, Rules};

/// `GBrick_ScoreKill` XML default `TargetKilled` (Mac 0x0103832c).
pub const KILL_POINTS: u32 = 100;
/// Stunning a pursuer: `GBrick_ScoreChase` XML default `ScoreHumiliation`
/// 200 (Mac 0x010381ec; the mode XMLs are not shipped, so the brick
/// defaults are the best data there is).
pub const STUN_POINTS: u32 = 200;
/// Air and drop kills: `GBrick_ScoreKill` default `BonusAcrobatic` 100.
pub const BONUS_ACROBATIC: u32 = 100;
/// Out of a hiding spot: `GBrick_ScoreKill` default `BonusHidden` 200.
pub const BONUS_HIDDEN: u32 = 200;
/// Placeholder: seconds from the kill to the victim's respawn. Only the
/// mode XML has it (`GBrick_Respawn::ReadFromXML` 0x00ec1870 reads
/// `RespawnTime`, whole seconds, default 0); the kill clips run ~3 s.
pub const RESPAWN_DELAY: f32 = 5.0;
/// Placeholder: no contracts during the first seconds (retail's intro;
/// `GBrick_Delay` `DelayDuration` 0x00d3d70a, XML only, default 0).
pub const START_DELAY: f32 = 3.0;
/// Escaping a chase: `GBrick_ScoreChase` XML default `ScoreEscape` 100.
pub const ESCAPE_POINTS: u32 = 100;
/// `GBrick_Escape::ReadFromXML` (Mac 0x00c17d32) defaults, seconds to fill
/// the escape meter per [`ChasePhase`]; 0 = the meter resets (in sight).
pub const TIME_TO_ESCAPE: [f32; 4] = [0.0, 0.0, 12.0, 7.0];
/// `GBrick_Escape` `DistanceInExtremis` default: our chase start range
/// (a running pursuer this close and in sight starts the chase; the
/// retail trigger is a `ChaseRequestEvent`, sender not traced).
pub const DISTANCE_IN_EXTREMIS: f32 = 10.0;
/// In sight for the escape meter: line of sight within this range (our
/// reading: retail `GameBrick::CanSee` is a head-to-head ray; we bound it by
/// the compass `DetectionRange`).
pub const SIGHT_RANGE: f32 = crate::rules::DETECTION_RANGE;
/// Full life, percent (`HealthSettings` fall damage is % of max life).
pub const FULL_HEALTH: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchConfig {
    /// Seconds of play.
    pub time_limit: f32,
    /// First to reach it wins (0: none).
    pub score_limit: u32,
    pub respawn_delay: f32,
    pub start_delay: f32,
}

impl Default for MatchConfig {
    fn default() -> Self {
        MatchConfig { time_limit: 300.0, score_limit: 1000, respawn_delay: RESPAWN_DELAY, start_delay: START_DELAY }
    }
}

/// Why points were scored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScoreReason {
    Kill,
    Stun,
    Escape,
}

/// `GBrick_Escape` chase phases (`GetNewPhase` 0x00c0f710): in sight or
/// not, target hidden (hiding spot or blended) or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChasePhase {
    InSight = 0,
    InSightHidden = 1,
    OutOfSight = 2,
    OutOfSightHidden = 3,
}

impl ChasePhase {
    pub fn new(in_sight: bool, hidden: bool) -> ChasePhase {
        match (in_sight, hidden) {
            (true, false) => ChasePhase::InSight,
            (true, true) => ChasePhase::InSightHidden,
            (false, false) => ChasePhase::OutOfSight,
            (false, true) => ChasePhase::OutOfSightHidden,
        }
    }

    pub fn from_u8(v: u8) -> Option<ChasePhase> {
        [ChasePhase::InSight, ChasePhase::InSightHidden, ChasePhase::OutOfSight, ChasePhase::OutOfSightHidden].get(v as usize).copied()
    }
}

/// A hunter chasing its target: the target escapes when `coef` passes 1
/// (`TickEscapingCoef` 0x00c0f794: += dt / time of the phase, reset in
/// sight).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chase {
    pub target: usize,
    pub coef: f32,
    pub phase: ChasePhase,
}

/// A player as the match sees it this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Presence {
    pub pos: Vec3,
    /// In a hiding spot.
    pub hidden: bool,
    /// Blended in a crowd group ([`crate::rules::blended`]).
    pub blended: bool,
    /// High profile on foot (running): starts a chase when seen.
    pub running: bool,
}

impl Presence {
    pub fn at(pos: Vec3) -> Presence {
        Presence { pos, ..Default::default() }
    }
}

/// What the match decided during the last tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchEvent {
    Contract(ContractEvent),
    Score { player: usize, points: u32, reason: ScoreReason },
    /// Back in the game at spawn `spawn`.
    Respawn { player: usize, spawn: usize },
    /// `hunter` was seen running at its target: the escape meter runs.
    ChaseStarted { hunter: usize, target: usize },
    /// `target` escaped `hunter` (who loses the contract).
    Escaped { hunter: usize, target: usize },
    /// `player` lost `damage` % of its life in a fall (`fatal`: died; no
    /// killer, nobody scores).
    Fall { player: usize, damage: u32, fatal: bool },
    /// The match ended; `winner` is `None` on a draw.
    Over { winner: Option<usize> },
}

#[derive(Clone, Debug)]
pub struct Match {
    pub config: MatchConfig,
    pub contracts: Contracts,
    pub score: Vec<u32>,
    /// Per player: ticks until the respawn while dead.
    pub respawn: Vec<Option<u32>>,
    /// Player spawn points (position, yaw).
    pub spawns: Vec<(Vec3, f32)>,
    /// Ticks played.
    pub ticks: u32,
    /// Per player: life left, % (falls; full again on respawn).
    pub health: Vec<u32>,
    /// Per hunter: its chase of its target.
    pub chases: Vec<Option<Chase>>,
    pub over: bool,
    /// This tick's decisions (cleared at every step).
    pub events: Vec<MatchEvent>,
}

/// Seconds to whole ticks.
pub fn ticks(secs: f32) -> u32 {
    (secs * crate::TICK_HZ as f32).round() as u32
}

impl Match {
    pub fn new(players: usize, config: MatchConfig, spawns: Vec<(Vec3, f32)>) -> Match {
        Match {
            config,
            contracts: Contracts::new(players),
            score: vec![0; players],
            respawn: vec![None; players],
            spawns,
            ticks: 0,
            health: vec![FULL_HEALTH; players],
            chases: vec![None; players],
            over: false,
            events: Vec::new(),
        }
    }

    /// Kill/stun rules while the match runs: other players only through
    /// contracts, crowd NPCs may be killed (no points), not stunned.
    pub fn rules(&self) -> Rules {
        Rules { free_play: false, allow_killing_npcs: true }
    }

    /// How body `body` stands to player `of` (bodies `0..players` are the
    /// players).
    pub fn relation(&self, of: usize, body: usize) -> Relation {
        if body < self.contracts.hunters.len() {
            self.contracts.relation(of, body)
        } else {
            Relation::Npc
        }
    }

    pub fn alive(&self, p: usize) -> bool {
        self.respawn.get(p).is_some_and(|r| r.is_none())
    }

    pub fn ticks_left(&self) -> u32 {
        ticks(self.config.time_limit).saturating_sub(self.ticks)
    }

    /// Player `killer` started a kill (or stun) of player `victim` with
    /// kill set `set` ([`crate::kill::KillClips::sets`]).
    pub(crate) fn on_action(&mut self, killer: usize, victim: usize, set: usize) {
        use crate::kill;
        let stun = (kill::STUN..kill::STUN + 2).contains(&set);
        let bonus = match set & !1 {
            kill::AIR | kill::DROP => BONUS_ACROBATIC,
            kill::HAY_GRAB => BONUS_HIDDEN,
            _ => 0,
        };
        let rel = self.contracts.relation(killer, victim);
        if stun {
            // a stunned hunter loses its contract on the stunner
            if matches!(rel, Relation::Pursuer | Relation::Mutual) {
                self.score[killer] += STUN_POINTS;
                self.events.push(MatchEvent::Score { player: killer, points: STUN_POINTS, reason: ScoreReason::Stun });
                for e in self.contracts.on_stunned(victim) {
                    self.events.push(MatchEvent::Contract(e));
                }
            }
            return;
        }
        if matches!(rel, Relation::Target | Relation::Mutual) {
            self.score[killer] += KILL_POINTS + bonus;
            self.events.push(MatchEvent::Score { player: killer, points: KILL_POINTS + bonus, reason: ScoreReason::Kill });
        }
        for e in self.contracts.on_death(victim, Some(killer)) {
            self.events.push(MatchEvent::Contract(e));
        }
        // +1: the clock part of this same tick already counts down once
        self.respawn[victim] = Some(ticks(self.config.respawn_delay).max(1) + 1);
    }

    /// Player `p` touched down after a fall with `damage` % of max life
    /// (`jump::landing`, retail `HealthSettings`): a fatal fall (life at
    /// 0) is a death without a killer. Retail has no suicide penalty in the
    /// data or in `NetPlayer::M2All_Suicide` (0x0112125c just applies the
    /// death), so nobody scores; the pursuers lose the contract.
    pub(crate) fn on_fall(&mut self, p: usize, damage: u32) {
        if damage == 0 || !self.alive(p) || self.over {
            return;
        }
        self.health[p] = self.health[p].saturating_sub(damage);
        let fatal = self.health[p] == 0;
        self.events.push(MatchEvent::Fall { player: p, damage, fatal });
        if fatal {
            for e in self.contracts.on_death(p, None) {
                self.events.push(MatchEvent::Contract(e));
            }
            self.respawn[p] = Some(ticks(self.config.respawn_delay).max(1) + 1);
        }
    }

    /// Chases and escapes (`GBrick_Escape::Update` 0x00c5f4fc): a chase
    /// ends with the contract; it starts when the hunter, running, is in
    /// sight of its target within [`DISTANCE_IN_EXTREMIS`]; the meter fills
    /// out of sight (faster with the target hidden or blended), resets in
    /// sight, and past 1 the target escapes: it scores [`ESCAPE_POINTS`],
    /// the hunter loses the contract (`EVENT_ENDCONTRACT_TARGET_ESCAPED_PLAYER`)
    /// and is reassigned after the cooldown.
    fn chase(&mut self, who: &[Presence], los: &dyn Fn(usize, usize) -> bool) {
        for h in 0..self.chases.len() {
            let target = self.contracts.hunters[h].target.filter(|_| self.alive(h));
            let Some(t) = target.filter(|&t| t < who.len() && self.alive(t)) else {
                self.chases[h] = None;
                continue;
            };
            if self.chases[h].is_some_and(|c| c.target != t) {
                self.chases[h] = None;
            }
            let dist = (who[t].pos - who[h].pos).truncate().length();
            let concealed = who[t].hidden || who[t].blended;
            match &mut self.chases[h] {
                None => {
                    if who[h].running && dist <= DISTANCE_IN_EXTREMIS && los(t, h) {
                        self.chases[h] = Some(Chase { target: t, coef: 0.0, phase: ChasePhase::new(true, concealed) });
                        self.events.push(MatchEvent::ChaseStarted { hunter: h, target: t });
                    }
                }
                Some(c) => {
                    let in_sight = dist <= SIGHT_RANGE && los(h, t);
                    c.phase = ChasePhase::new(in_sight, concealed);
                    let time = TIME_TO_ESCAPE[c.phase as usize];
                    c.coef = if time > 0.0 { c.coef + crate::DT / time } else { 0.0 };
                    if c.coef > 1.0 {
                        self.chases[h] = None;
                        self.score[t] += ESCAPE_POINTS;
                        self.events.push(MatchEvent::Escaped { hunter: h, target: t });
                        self.events.push(MatchEvent::Score { player: t, points: ESCAPE_POINTS, reason: ScoreReason::Escape });
                        for e in self.contracts.on_stunned(h) {
                            self.events.push(MatchEvent::Contract(e));
                        }
                    }
                }
            }
        }
    }

    /// The spawn for `p`: the one furthest from the nearest other living
    /// player (ties: lowest index).
    pub fn pick_spawn(&self, p: usize, pos: &[Vec3]) -> usize {
        let others: Vec<Vec3> = pos.iter().enumerate().filter(|&(j, _)| j != p && self.alive(j)).map(|(_, q)| *q).collect();
        let score = |s: &(Vec3, f32)| others.iter().map(|q| q.distance(s.0)).fold(f32::INFINITY, f32::min);
        (0..self.spawns.len()).max_by(|&a, &b| score(&self.spawns[a]).total_cmp(&score(&self.spawns[b])).then(b.cmp(&a))).unwrap_or(0)
    }

    /// The clock part of a tick (after the players moved): respawn
    /// countdowns (returns who respawns where), contracts, limits.
    /// `los(a, b)`: line of sight from player `a`'s head to `b`'s.
    pub(crate) fn tick(&mut self, who: &[Presence], los: &dyn Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
        let pos: Vec<Vec3> = who.iter().map(|w| w.pos).collect();
        let pos = &pos[..];
        let mut back = Vec::new();
        if self.over {
            return back;
        }
        self.ticks += 1;
        self.chase(who, los);
        for p in 0..self.respawn.len() {
            if let Some(r) = &mut self.respawn[p] {
                *r -= 1;
                if *r == 0 {
                    self.respawn[p] = None;
                    let s = self.pick_spawn(p, pos);
                    self.contracts.respawn(p);
                    self.health[p] = FULL_HEALTH;
                    self.events.push(MatchEvent::Respawn { player: p, spawn: s });
                    back.push((p, s));
                }
            }
        }
        if self.ticks >= ticks(self.config.start_delay) {
            for e in self.contracts.update(crate::DT, pos) {
                self.events.push(MatchEvent::Contract(e));
            }
        }
        let best = self.score.iter().copied().max().unwrap_or(0);
        let by_score = self.config.score_limit > 0 && best >= self.config.score_limit;
        if by_score || self.ticks >= ticks(self.config.time_limit) {
            self.over = true;
            let leaders: Vec<usize> = (0..self.score.len()).filter(|&i| self.score[i] == best).collect();
            let winner = (leaders.len() == 1).then(|| leaders[0]);
            self.events.push(MatchEvent::Over { winner });
        }
        back
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(n: usize) -> Match {
        let spawns = vec![(Vec3::ZERO, 0.0), (Vec3::new(50.0, 0.0, 0.0), 0.0), (Vec3::new(0.0, 40.0, 0.0), 0.0)];
        Match::new(n, MatchConfig { time_limit: 10.0, score_limit: 200, respawn_delay: 1.0, start_delay: 0.5 }, spawns)
    }

    fn run(m: &mut Match, n: u32, pos: &[Vec3]) -> Vec<(usize, usize)> {
        let who: Vec<Presence> = pos.iter().map(|&p| Presence::at(p)).collect();
        (0..n).flat_map(|_| m.tick(&who, &|_, _| true)).collect()
    }

    #[test]
    fn duel_kill_scores_respawns_and_reassigns() {
        let mut m = m(2);
        let pos = [Vec3::ZERO, Vec3::new(20.0, 0.0, 0.0)];
        run(&mut m, 29, &pos);
        assert_eq!(m.contracts.hunters[0].target, None, "no contracts before the start delay");
        run(&mut m, 1, &pos);
        assert_eq!(m.contracts.relation(0, 1), Relation::Mutual);
        m.on_action(0, 1, crate::kill::WALK);
        assert_eq!(m.score, vec![100, 0]);
        assert!(!m.alive(1));
        assert_eq!(m.contracts.hunters[0].target, None);
        // victim at the old place: the spawn furthest from player 0 (index 1)
        // (on_action outside a step: one extra countdown tick)
        let back = run(&mut m, 61, &pos);
        assert_eq!(back, vec![(1, 1)]);
        assert!(m.alive(1));
        // both cool down MinWaitTime, then hunt each other again
        run(&mut m, ticks(crate::rules::MIN_WAIT_TIME) + 1, &pos);
        assert_eq!(m.contracts.relation(0, 1), Relation::Mutual);
        m.on_action(1, 0, crate::kill::WALK);
        m.on_action(0, 1, crate::kill::STUN); // no contract any more: no points
        assert_eq!(m.score, vec![100, 100]);
    }

    #[test]
    fn three_players_and_limits() {
        let mut m = m(3);
        let pos = [Vec3::ZERO, Vec3::new(20.0, 0.0, 0.0), Vec3::new(0.0, 30.0, 0.0)];
        run(&mut m, 30, &pos);
        let targets: Vec<_> = m.contracts.hunters.iter().map(|h| h.target).collect();
        assert!(targets.iter().all(|t| t.is_some()), "{targets:?}");
        // one pursuer per target with 3 players
        for t in 0..3 {
            assert!(m.contracts.pursuers(t) <= 1);
        }
        let h = 0;
        let t = targets[0].unwrap();
        m.on_action(h, t, crate::kill::WALK);
        m.score[h] = 200;
        let _ = run(&mut m, 1, &pos);
        assert!(m.over);
        assert_eq!(m.events.last(), Some(&MatchEvent::Over { winner: Some(h) }));
    }
}
