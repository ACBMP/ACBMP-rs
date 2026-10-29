//! A Wanted-style match inside the deterministic sim (a duel with two
//! players; nothing here assumes two): [`Contracts`] decide who may kill
//! or stun whom, kills and stuns score, killed players respawn on a player
//! spawn after a delay, the match ends on a time or score limit.
//!
//! It lives in [`crate::World`] (and its [`crate::state::Saved`]) so the
//! server and every client's prediction/replay step the same match; no
//! clock, no RNG, ties by index.

use glam::Vec3;

use crate::rules::{Acquire, AcquireEvent, Contracts, ContractEvent, Relation, Rules, Teams};

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
/// `stSpawnPointMatesData::SortByVIPdistance` 0x0110fad8: a spawn whose
/// nearest enemy is this close (or closer) loses to one further from
/// enemies; beyond it the spawn nearest to mates wins.
pub const VIP_RULE_ENEMY_RADIUS: f32 = 35.0;
/// `BuildSpawnPointListFromEntities` 0x01174c46: distance to a kind of
/// entity when there is none.
pub const VIP_RULE_NO_ENTITY: f32 = 2000.0;

/// Retail Escort respawn (`MultiSpawnManager::GetBestSpawnPoint_VIPrule`
/// 0x011750e4, rule 6 of `Server_GetBestSpawnPoint` 0x011770c4): every
/// free spawn gets its distance to the nearest enemy and to the nearest
/// mate ([`VIP_RULE_NO_ENTITY`] when none), and the first after sorting by
/// `SortByVIPdistance` wins: if either of two spawns has an enemy within
/// [`VIP_RULE_ENEMY_RADIUS`], the one further from enemies comes first,
/// else the one nearer to mates. Callers count living teammates and, for
/// protectors, the VIPs as mates; the other team and, for hunters, the
/// VIPs as enemies (`AddVIPsInEntityInfo` 0x0116f55c). So protectors come
/// back next to their VIPs, hunters at least 35 m from them. We scan for
/// the first instead of sorting (same result whenever the order is
/// consistent; ties: lowest index). `None`: no spawns.
pub fn vip_rule(spawns: &[Vec3], mates: &[Vec3], enemies: &[Vec3]) -> Option<usize> {
    let near = |s: Vec3, set: &[Vec3]| set.iter().map(|q| q.distance(s)).fold(VIP_RULE_NO_ENTITY, f32::min);
    let d: Vec<(f32, f32)> = spawns.iter().map(|&s| (near(s, enemies), near(s, mates))).collect();
    let before = |a: (f32, f32), b: (f32, f32)| {
        if a.0 <= VIP_RULE_ENEMY_RADIUS || b.0 <= VIP_RULE_ENEMY_RADIUS { a.0 > b.0 } else { a.1 < b.1 }
    };
    (0..d.len()).reduce(|best, i| if before(d[i], d[best]) { i } else { best })
}

/// Placeholder: no contracts during the first seconds (retail's intro;
/// `GBrick_Delay` `DelayDuration` 0x00d3d70a, XML only, default 0).
pub const START_DELAY: f32 = 3.0;
/// Assassinate round length: "10 minutes" in the shipped mode description
/// (UIString 0x4e102; the GBrick_Timer XML is not shipped).
pub const ASSASSINATE_TIME: f32 = 600.0;
/// Placeholder (Assassinate): killing a crowd NPC loses the killer's
/// contract. Only UI strings point that way (spec R item "wrong kill breaks
/// the contract"); `GBrick_ScoreKill` pays 0 for an NPC (0x0103832c), and
/// 0x4e125 "Civilian kills give +#ScoreOrBonus# points to each of your
/// opponents" names no mode (not ported).
pub const NPC_KILL_LOSES_CONTRACT: bool = true;
/// Escaping a chase: `GBrick_ScoreChase` XML default `ScoreEscape` 100.
pub const ESCAPE_POINTS: u32 = 100;
/// `GBrick_Escape::ReadFromXML` (Mac 0x00c17d32) defaults, seconds to fill
/// the escape meter per [`ChasePhase`]; 0 = the meter resets (in sight).
pub const TIME_TO_ESCAPE: [f32; 4] = [0.0, 0.0, 12.0, 7.0];
/// `GBrick_Escape` `DistanceInExtremis` default (unused: the chase starts
/// on a full threat meter, see [`THREAT_RATE`]).
pub const DISTANCE_IN_EXTREMIS: f32 = 10.0;
/// Threat meter fill per second (`GamemodeParameters` data, Wanted2 /
/// TeamVIP `ThreatmeterFastSpeed`, `NeutralSpeed`, `HiddenSpeed`) indexed
/// [running, walking, concealed], seen by the target; the meter is
/// integrated in `GameInfo::UpdateLocalPlayerThreatMeter` (Mac 0x011857d0):
/// rising rates scale with the awareness, falling ones with max(1, 2 - it).
pub const THREAT_RATE: [f32; 3] = [0.1, -0.1, -0.132];
/// The same out of the target's sight (`Threatmeter*SpeedOOS`).
pub const THREAT_RATE_OOS: [f32; 3] = [-0.0495, -0.1155, -0.165];
/// `ThreatmeterFarSpeed`: seen, but beyond [`THREAT_FAR`].
pub const THREAT_RATE_FAR: f32 = -0.25;
/// `ThreatCloseDistance` / `ThreatFarDistance`: awareness distance ramp
/// (`GameInfo::GetTargetAwearness` 0x011850aa), full up to close, 0 at far.
pub const THREAT_CLOSE: f32 = 10.0;
pub const THREAT_FAR: f32 = 35.0;
/// `ThreatDistanceDowning`: awareness loss at [`THREAT_FAR`] and beyond.
pub const THREAT_DIST_DOWNING: f32 = 4.0;
/// `ThreatToTargetDowning` / `ThreatBehindTargetDowning`: the two facing terms.
pub const THREAT_TO_DOWNING: f32 = 3.0;
pub const THREAT_BEHIND_DOWNING: f32 = 2.0;
/// `ThreatLowSpeed` / `ThreatHighSpeed` (m/s): the facing terms ramp in
/// with the hunter's speed between these.
pub const THREAT_LOW_SPEED: f32 = 2.0;
pub const THREAT_HIGH_SPEED: f32 = 4.0;
/// Facing cones (deg, inner/outer) of `GetTargetAwearness` (retail
/// defaults, GameParameters +0x78/+0x7c and +0x84/+0x88).
pub const THREAT_TO_CONE: (f32, f32) = (45.57, 154.16);
pub const THREAT_BEHIND_CONE: (f32, f32) = (25.84, 134.43);
/// `ThreatmeterInAirFastSpeed` / `InAirSlowSpeed` (seen) and the OOS pair.
pub const THREAT_RATE_AIR: [f32; 2] = [0.65, 0.25];
pub const THREAT_RATE_AIR_OOS: [f32; 2] = [-0.066, -0.0825];
/// `KillerDiscoveredEvent`: the meter of a hunter its target locked on.
pub const THREAT_DISCOVERED: f32 = 0.99;

fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// 0 inside the cone of half-angle `cone.0`, 1 beyond `cone.1`.
fn cone(dot: f32, cone: (f32, f32)) -> f32 {
    let (a, b) = (cone.0.to_radians().cos(), cone.1.to_radians().cos());
    ((a - dot) / (a - b)).clamp(0.0, 1.0)
}

/// How aware `target` is of `hunter`, 0..=1 (`GameInfo::GetTargetAwearness`
/// Mac 0x011850aa): 1 / sqrt(1 + distance + to + behind) with a distance
/// term ramping [`THREAT_CLOSE`]..[`THREAT_FAR`] up to
/// [`THREAT_DIST_DOWNING`], and two facing terms scaled by the hunter's
/// speed ramp: "to" (hunter moving toward the target, 0 straight at it
/// and growing outside [`THREAT_TO_CONE`]) and "behind" (full when the
/// hunter is within [`THREAT_BEHIND_CONE`] behind the target). Which
/// vectors retail dots is our reading of the decompile.
pub fn awareness(hunter: &Presence, target: &Presence) -> f32 {
    let d = (target.pos - hunter.pos).truncate();
    let dist = d.length();
    let fwd = |yaw: f32| glam::Vec2::new(-yaw.sin(), yaw.cos());
    let dir = if dist > 1e-4 { d / dist } else { glam::Vec2::ZERO };
    let speed = ramp(hunter.speed, THREAT_LOW_SPEED, THREAT_HIGH_SPEED);
    let far = THREAT_DIST_DOWNING * ramp(dist, THREAT_CLOSE, THREAT_FAR);
    let toward = fwd(hunter.yaw).dot(dir);
    let to = if toward > 0.0 && dist > 0.0 { THREAT_TO_DOWNING * speed * cone(toward, THREAT_TO_CONE) } else { 0.0 };
    let behind = THREAT_BEHIND_DOWNING * speed * (1.0 - cone(fwd(target.yaw).dot(dir), THREAT_BEHIND_CONE));
    1.0 / (1.0 + far + to + behind).sqrt()
}
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
    /// Escort: a hunter killed a VIP ([`crate::rounds::VIP_KILLED_POINTS`]).
    VipKill,
    /// Escort: a VIP passed a checkpoint (protecting team).
    Checkpoint,
    /// Escort: a VIP reached its way's end (protecting team).
    Protect,
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
    /// Facing (entity +Y is (-sin yaw, cos yaw)).
    pub yaw: f32,
    /// Horizontal speed, m/s.
    pub speed: f32,
    /// Jumping or falling.
    pub in_air: bool,
    /// Camera yaw of the player's input (forward (cos, sin), see
    /// [`crate::PlayerInput::cam_yaw`]): where it looks (Assassinate).
    pub look: f32,
    /// Lock button held (Assassinate: a press locks the highlighted player).
    pub lock: bool,
    /// Release button held (Assassinate: a press drops the contract).
    pub release: bool,
    /// In a kill or stun (retail `IsInAnyActiveConflictWithPlayer`): no
    /// release.
    pub conflict: bool,
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
    /// Escort: points for a whole team (checkpoints, protected VIPs); a
    /// player's [`MatchEvent::Score`] also counts for its team.
    TeamScore { team: crate::rules::Team, points: u32, reason: ScoreReason },
    /// Escort: round phases, the attacking-team swap, the match end.
    Round(crate::rounds::RoundEvent),
    /// Assassinate: a player started or stopped looking at another (the
    /// looked-at player is warned).
    Acquire(AcquireEvent),
    /// Assassinate: a scoring kill earned `bonus` (its points are in the
    /// kill's [`MatchEvent::Score`]).
    Bonus { player: usize, bonus: crate::bonus::Bonus },
    /// Assassinate: `player`'s kill closed a chain of `chain` kills within
    /// 10 s (`GBrick_BloodBath`: a voice/message, no points).
    BloodBath { player: usize, chain: u32 },
    /// Assassinate: `hunter` released its contract on `target` (release
    /// button, retail CancelWithY).
    Released { hunter: usize, target: usize },
}

/// Which rule set a [`Match`] runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Wanted-style free-for-all with [`Contracts`] (the duel).
    #[default]
    Duel,
    /// Escort (teamvip): two teams, hunters vs protectors, relations from
    /// [`Teams`]; contracts stay empty. VIP bodies, ways, team scoring and
    /// rounds come later (Phase 4 E2..E5) through [`Teams::vips`] and
    /// [`Teams::attacking_team`].
    Escort(Teams),
    /// Assassinate: free-for-all, a player acquires its target by looking
    /// at it ([`crate::rules::Contracts::acquire`]); no lottery.
    Assassinate,
}

#[derive(Clone, Debug)]
pub struct Match {
    pub config: MatchConfig,
    pub mode: Mode,
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
    /// Per hunter: threat meter 0..=1 toward its target; a full meter
    /// starts the chase (`GBrick_Escape::Update`, StartChaseAtFullThreat).
    pub threat: Vec<f32>,
    /// Per hunter: the target its threat meter is toward (a new target
    /// starts from 0).
    pub threat_of: Vec<Option<usize>>,
    pub over: bool,
    /// This tick's decisions (cleared at every step).
    pub events: Vec<MatchEvent>,
    /// Escort: the VIP cycle (`World::escort` sets it up; Duel: `None`).
    pub vips: Option<crate::vip::Vips>,
    /// Escort: rounds and team totals (Duel: `None`).
    pub rounds: Option<crate::rounds::Rounds>,
    /// Assassinate: per player, its acquisition (others: empty).
    pub acquire: Vec<Acquire>,
    /// Assassinate: kill bonus and BloodBath state (others: `None`).
    pub bonus: Option<crate::bonus::Tracker>,
}

/// Seconds to whole ticks.
pub fn ticks(secs: f32) -> u32 {
    (secs * crate::TICK_HZ as f32).round() as u32
}

impl Match {
    /// An Escort match: `teams.team` must have one entry per player.
    /// `config.time_limit` is the playing time of each round (score limit
    /// unused); [`crate::rounds::ROUNDS`] rounds with retail intro/recap.
    pub fn escort(teams: Teams, config: MatchConfig, spawns: Vec<(Vec3, f32)>) -> Match {
        let n = teams.team.iter().copied().chain([teams.attacking_team]).max().map_or(2, |t| t as usize + 1).max(2);
        let rounds = crate::rounds::Rounds::retail(n, config.time_limit);
        Match { mode: Mode::Escort(teams.clone()), rounds: Some(rounds), ..Match::new(teams.team.len(), config, spawns) }
    }

    /// Escort: whether a round is being played (Duel: always).
    pub fn playing(&self) -> bool {
        self.rounds.as_ref().is_none_or(|r| r.playing())
    }

    /// `points` to player `p` (and, in Escort, to its team while playing).
    fn award(&mut self, p: usize, points: u32, reason: ScoreReason) {
        if let (Mode::Escort(teams), Some(r)) = (&self.mode, &mut self.rounds) {
            if !r.add(teams.team[p], points) {
                return;
            }
        }
        self.score[p] += points;
        self.events.push(MatchEvent::Score { player: p, points, reason });
    }

    /// Escort: `points` to the whole of `team` while playing.
    fn award_team(&mut self, team: crate::rules::Team, points: u32, reason: ScoreReason) {
        if self.rounds.as_mut().is_some_and(|r| r.add(team, points)) {
            self.events.push(MatchEvent::TeamScore { team, points, reason });
        }
    }

    /// Escort: scores this tick's VIP events: a VIP kill pays its hunter,
    /// checkpoints and way ends pay the protecting team(s).
    pub(crate) fn on_vip_events(&mut self, events: &[crate::vip::VipEvent]) {
        use crate::vip::VipEvent;
        let Mode::Escort(teams) = &self.mode else { return };
        let attacking = teams.attacking_team;
        let protecting: Vec<crate::rules::Team> = (0..self.rounds.as_ref().map_or(0, |r| r.team_score.len()) as crate::rules::Team).filter(|&t| t != attacking).collect();
        for e in events {
            let (points, reason) = match *e {
                VipEvent::Killed { killer, .. } => {
                    self.award(killer, crate::rounds::VIP_KILLED_POINTS, ScoreReason::VipKill);
                    continue;
                }
                VipEvent::Checkpoint { n, .. } => (crate::rounds::checkpoint_points(n), ScoreReason::Checkpoint),
                VipEvent::WayEnd { n, .. } => (crate::rounds::protect_points(n), ScoreReason::Protect),
                _ => continue,
            };
            for &t in &protecting {
                self.award_team(t, points, reason);
            }
            // PLACEHOLDER (rounds::ROUND_ENDS_ON_ESCAPE): the VIP escaped
            if matches!(e, VipEvent::WayEnd { .. }) {
                let team = protecting.first().copied().unwrap_or(0);
                if let Some(ev) = self.rounds.as_mut().and_then(|r| r.escape(team)) {
                    self.events.push(MatchEvent::Round(ev));
                }
            }
        }
    }

    pub fn new(players: usize, config: MatchConfig, spawns: Vec<(Vec3, f32)>) -> Match {
        Match {
            config,
            mode: Mode::Duel,
            contracts: Contracts::new(players),
            score: vec![0; players],
            respawn: vec![None; players],
            spawns,
            ticks: 0,
            health: vec![FULL_HEALTH; players],
            chases: vec![None; players],
            threat: vec![0.0; players],
            threat_of: vec![None; players],
            over: false,
            events: Vec::new(),
            vips: None,
            rounds: None,
            acquire: Vec::new(),
            bonus: None,
        }
    }

    /// An Assassinate match ([`Mode::Assassinate`]; `config.time_limit`
    /// should be [`ASSASSINATE_TIME`]).
    pub fn assassinate(players: usize, config: MatchConfig, spawns: Vec<(Vec3, f32)>) -> Match {
        Match {
            mode: Mode::Assassinate,
            acquire: vec![Acquire::default(); players],
            bonus: Some(crate::bonus::Tracker::new(players)),
            ..Match::new(players, config, spawns)
        }
    }

    /// Assassinate: the players acquiring `p` (looking at it, not yet
    /// holding it): `p`'s warning.
    pub fn acquirers(&self, p: usize) -> Vec<usize> {
        (0..self.acquire.len()).filter(|&h| self.acquire[h].acquiring(p, self.contracts.hunters[h].target)).collect()
    }

    /// Assassinate: the players `p`'s compass marks as potential targets
    /// (every other living player; 0x4e102 "Potential targets are marked
    /// on your compass").
    pub fn potential_targets(&self, p: usize) -> Vec<usize> {
        if self.mode != Mode::Assassinate {
            return Vec::new();
        }
        (0..self.score.len()).filter(|&q| q != p && self.alive(q)).collect()
    }

    /// Assassinate: `p` loses everything it was acquiring or held in
    /// memory (stunned, escaped from, died).
    fn reset_acquire(&mut self, p: usize) {
        if let Some(a) = self.acquire.get_mut(p) {
            a.reset();
        }
    }

    /// Assassinate: `p` died: its acquisition is gone and nobody looks at
    /// it or remembers it any more.
    fn died(&mut self, p: usize) {
        self.reset_acquire(p);
        for a in &mut self.acquire {
            a.forget(p);
        }
    }

    /// Assassinate: every living player's acquisition tick (after the start
    /// delay, like the duel's lottery).
    fn acquire_tick(&mut self, who: &[Presence], sight: &dyn Fn(usize, usize) -> bool) {
        let pos: Vec<Vec3> = who.iter().map(|w| w.pos).collect();
        let alive: Vec<bool> = (0..self.score.len()).map(|p| self.alive(p)).collect();
        for h in 0..self.acquire.len().min(who.len()) {
            let mut a = std::mem::take(&mut self.acquire[h]);
            let (lock, release) = a.presses(who[h].lock, who[h].release);
            if !alive[h] {
                self.acquire[h] = a;
                continue;
            }
            // retail 0x0130d708: the key press drops the selected contract
            // unless in a conflict (kill/stun) and once CancelTargetDelay
            // (0 s) has run; a chase on it ends as an escape
            if release && !who[h].conflict {
                if let Some(t) = self.contracts.hunters[h].target {
                    if self.chases[h].is_some_and(|c| c.target == t) {
                        self.chases[h] = None;
                        self.events.push(MatchEvent::Escaped { hunter: h, target: t });
                        self.award(t, ESCAPE_POINTS, ScoreReason::Escape);
                    }
                    self.threat[h] = 0.0;
                    self.events.push(MatchEvent::Released { hunter: h, target: t });
                }
                for e in self.contracts.release(h, &mut a) {
                    self.events.push(MatchEvent::Contract(e));
                }
            }
            let highlight = crate::rules::looked_at(h, &pos, who[h].look, &alive, sight);
            let cand = a.candidate(h, &pos, &alive, highlight, lock);
            let hp = cand.is_some_and(|c| who[c].running);
            let (contracts, warn) = self.contracts.acquire(h, &mut a, cand, hp);
            self.acquire[h] = a;
            self.events.extend(warn.into_iter().map(MatchEvent::Acquire));
            self.events.extend(contracts.into_iter().map(MatchEvent::Contract));
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
        if let Mode::Escort(teams) = &self.mode {
            // intro / recap: nobody may kill or stun another player or a VIP
            match teams.kind(body) {
                crate::rules::Kind::Npc => Relation::Npc,
                _ if !self.playing() => Relation::Other,
                _ => teams.relation(of, body),
            }
        } else if body < self.contracts.hunters.len() {
            let rel = self.contracts.relation(of, body);
            // Assassinate: a player being acquired may stun its acquirer
            // ("takes time and warns the target, allowing them to stun
            // you", draft 0x4dbc4/0x4dea8)
            let acquiring = self.acquire.get(body).is_some_and(|a| a.acquiring(of, self.contracts.hunters[body].target));
            match rel {
                Relation::Other if acquiring => Relation::Pursuer,
                Relation::Target if acquiring => Relation::Mutual,
                r => r,
            }
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
        let rel = self.relation(killer, victim);
        if stun {
            // a stunned hunter loses its contract on the stunner
            if matches!(rel, Relation::Pursuer | Relation::Mutual) {
                self.award(killer, STUN_POINTS, ScoreReason::Stun);
                for e in self.contracts.on_stunned(victim) {
                    self.events.push(MatchEvent::Contract(e));
                }
                self.reset_acquire(victim);
            }
            return;
        }
        let scored = matches!(rel, Relation::Target | Relation::Mutual);
        if scored {
            let extra = self.kill_bonuses(killer, victim);
            self.award(killer, KILL_POINTS + bonus + extra, ScoreReason::Kill);
        }
        if let Some(t) = &mut self.bonus {
            if let Some(chain) = t.on_kill(killer, victim, scored) {
                self.events.push(MatchEvent::BloodBath { player: killer, chain });
            }
        }
        for e in self.contracts.on_death(victim, Some(killer)) {
            self.events.push(MatchEvent::Contract(e));
        }
        self.died(victim);
        // +1: the clock part of this same tick already counts down once
        self.respawn[victim] = Some(ticks(self.config.respawn_delay).max(1) + 1);
    }

    /// Assassinate: the [`crate::bonus`] points of `killer`'s scoring kill
    /// of `victim` (one [`MatchEvent::Bonus`] each). Others: 0.
    fn kill_bonuses(&mut self, killer: usize, victim: usize) -> u32 {
        let Some(t) = &self.bonus else { return 0 };
        let mut s = t.situation(killer, victim);
        s.chasing = self.chases.get(killer).is_some_and(|c| c.is_some());
        s.incognito = self.threat_of.get(killer) != Some(&Some(victim)) || self.threat[killer] <= 0.0;
        let list = crate::bonus::bonuses(&s);
        self.events.extend(list.iter().map(|&b| MatchEvent::Bonus { player: killer, bonus: b }));
        list.iter().map(|b| b.points()).sum()
    }

    /// Assassinate: player `p` killed a crowd NPC: its contract and its
    /// acquisition are lost ([`NPC_KILL_LOSES_CONTRACT`]). Other modes: no
    /// effect.
    pub(crate) fn on_npc_kill(&mut self, p: usize) {
        if self.mode != Mode::Assassinate || !NPC_KILL_LOSES_CONTRACT || p >= self.score.len() {
            return;
        }
        for e in self.contracts.on_stunned(p) {
            self.events.push(MatchEvent::Contract(e));
        }
        self.reset_acquire(p);
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
            self.died(p);
            self.respawn[p] = Some(ticks(self.config.respawn_delay).max(1) + 1);
        }
    }

    /// Chases and escapes (`GBrick_Escape::Update` 0x00c5f4fc): a chase
    /// ends with the contract; it starts when the hunter's threat meter
    /// is full ([`THREAT_RATE`]); the escape meter fills
    /// out of sight (faster with the target hidden or blended), resets in
    /// sight, and past 1 the target escapes: it scores [`ESCAPE_POINTS`],
    /// the hunter loses the contract (`EVENT_ENDCONTRACT_TARGET_ESCAPED_PLAYER`)
    /// and is reassigned after the cooldown.
    fn chase(&mut self, who: &[Presence], los: &dyn Fn(usize, usize) -> bool) {
        for h in 0..self.chases.len() {
            let target = self.contracts.hunters[h].target.filter(|_| self.alive(h));
            let Some(t) = target.filter(|&t| t < who.len() && self.alive(t)) else {
                self.chases[h] = None;
                self.threat[h] = 0.0;
                continue;
            };
            if self.threat_of[h] != Some(t) {
                self.threat_of[h] = Some(t);
                self.threat[h] = 0.0;
            }
            if self.chases[h].is_some_and(|c| c.target != t) {
                self.chases[h] = None;
            }
            let dist = (who[t].pos - who[h].pos).truncate().length();
            let concealed = who[t].hidden || who[t].blended;
            match &mut self.chases[h] {
                None => {
                    let seen = dist <= SIGHT_RANGE && los(t, h);
                    let state = if who[h].running { 0 } else if who[h].hidden || who[h].blended { 2 } else { 1 };
                    let aware = if seen && dist <= THREAT_FAR { awareness(&who[h], &who[t]) } else { 0.0 };
                    let fast = who[h].running as usize ^ 1;
                    let rate = match (seen, aware > 0.0, who[h].in_air) {
                        (false, _, true) => THREAT_RATE_AIR_OOS[fast],
                        (false, _, false) => THREAT_RATE_OOS[state],
                        (true, false, _) => THREAT_RATE_FAR,
                        (true, true, true) => THREAT_RATE_AIR[fast],
                        (true, true, false) => THREAT_RATE[state],
                    };
                    let step = if rate > 0.0 { rate * aware } else { rate * (2.0 - aware).max(1.0) };
                    self.threat[h] = (self.threat[h] + step * crate::DT).clamp(0.0, 1.0);
                    if self.threat[h] >= 1.0 - 1e-4 {
                        self.threat[h] = 0.0;
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
                        self.events.push(MatchEvent::Escaped { hunter: h, target: t });
                        self.award(t, ESCAPE_POINTS, ScoreReason::Escape);
                        for e in self.contracts.on_stunned(h) {
                            self.events.push(MatchEvent::Contract(e));
                        }
                        self.reset_acquire(h);
                    }
                }
            }
        }
    }

    /// The spawn for `p`: Escort uses the retail VIP rule ([`vip_rule`]);
    /// the duel the one furthest from the nearest other living player
    /// (ties: lowest index). `used`: spawns already taken this tick.
    pub fn pick_spawn(&self, p: usize, pos: &[Vec3], used: &[usize]) -> usize {
        if let Mode::Escort(teams) = &self.mode {
            let mine = teams.team.get(p).copied();
            let protector = mine.is_some_and(|t| t != teams.attacking_team);
            let (mut mates, mut enemies) = (Vec::new(), Vec::new());
            // AddPlayersInEntityInfo 0x0116f1fc: living others, teammates
            // kind 0, the other team kind 1
            for (j, q) in pos.iter().enumerate() {
                if j == p || !self.alive(j) {
                    continue;
                }
                if teams.team.get(j).copied() == mine { mates.push(*q) } else { enemies.push(*q) }
            }
            // AddVIPsInEntityInfo 0x0116f55c: VIPs are mates of the
            // protecting team (GameInfo+0x514), enemies of the hunters
            if let Some(v) = &self.vips {
                for x in v.slots.iter().flatten() {
                    if protector { mates.push(x.walker.pos) } else { enemies.push(x.walker.pos) }
                }
            }
            let free: Vec<usize> = (0..self.spawns.len()).filter(|s| !used.contains(s)).collect();
            let free = if free.is_empty() { (0..self.spawns.len()).collect() } else { free };
            let points: Vec<Vec3> = free.iter().map(|&s| self.spawns[s].0).collect();
            return vip_rule(&points, &mates, &enemies).map_or(0, |i| free[i]);
        }
        let others: Vec<Vec3> = pos.iter().enumerate().filter(|&(j, _)| j != p && self.alive(j)).map(|(_, q)| *q).collect();
        let score = |s: &(Vec3, f32)| others.iter().map(|q| q.distance(s.0)).fold(f32::INFINITY, f32::min);
        (0..self.spawns.len()).max_by(|&a, &b| score(&self.spawns[a]).total_cmp(&score(&self.spawns[b])).then(b.cmp(&a))).unwrap_or(0)
    }

    /// The clock part of a tick (after the players moved): respawn
    /// countdowns (returns who respawns where), contracts, limits.
    /// `los(a, b)`: line of sight from player `a`'s head to `b`'s.
    #[cfg(test)]
    pub(crate) fn tick(&mut self, who: &[Presence], los: &dyn Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
        self.tick_with(who, los, los)
    }

    /// [`Match::tick`] with `sight(a, b)`, Assassinate's acquisition line of
    /// sight (any of [`crate::rules::LOOK_RAY_HEIGHTS`] on `b` clear from
    /// `a`'s head).
    pub(crate) fn tick_with(&mut self, who: &[Presence], los: &dyn Fn(usize, usize) -> bool, sight: &dyn Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
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
                    let used: Vec<usize> = back.iter().map(|&(_, s)| s).collect();
                    let s = self.pick_spawn(p, pos, &used);
                    self.contracts.respawn(p);
                    self.health[p] = FULL_HEALTH;
                    self.events.push(MatchEvent::Respawn { player: p, spawn: s });
                    back.push((p, s));
                }
            }
        }
        // Escort has no contracts: the relation comes from the teams
        if self.ticks >= ticks(self.config.start_delay) && self.mode == Mode::Duel {
            for e in self.contracts.update(crate::DT, pos) {
                self.events.push(MatchEvent::Contract(e));
            }
        }
        if self.ticks >= ticks(self.config.start_delay) && self.mode == Mode::Assassinate {
            self.acquire_tick(who, sight);
        }
        if self.bonus.is_some() {
            let running: Vec<bool> = who.iter().map(|w| w.running).collect();
            let alive: Vec<bool> = (0..who.len()).map(|p| self.alive(p)).collect();
            if let Some(t) = &mut self.bonus {
                t.tick(pos, &running, &alive);
            }
        }
        if self.rounds.is_some() {
            self.tick_rounds();
            return back;
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

    /// Escort: the round clock. A round's play starts with a fresh VIP
    /// cycle; between rounds the attacking team swaps; after the last
    /// recap the match is over.
    fn tick_rounds(&mut self) {
        let (Mode::Escort(teams), Some(r)) = (&mut self.mode, &mut self.rounds) else { return };
        for e in r.tick(&mut teams.attacking_team) {
            use crate::rounds::{Phase, RoundEvent};
            match e {
                RoundEvent::Phase { phase: Phase::Play, .. } => {
                    if let Some(v) = &mut self.vips {
                        *v = crate::vip::Vips::new(v.bodies.clone());
                    }
                }
                RoundEvent::Phase { .. } | RoundEvent::Swap { .. } => teams.vips.clear(),
                RoundEvent::Over { .. } => self.over = true,
                RoundEvent::Escaped { .. } => {}
            }
            self.events.push(MatchEvent::Round(e));
        }
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
    fn escape_scores_and_reassigns() {
        let mut m = m(3);
        m.config.time_limit = 100.0;
        let mut who = vec![Presence::at(Vec3::ZERO), Presence::at(Vec3::new(20.0, 0.0, 0.0)), Presence::at(Vec3::new(0.0, 30.0, 0.0))];
        let see = std::cell::Cell::new(true);
        let los = |_: usize, _: usize| see.get();
        for _ in 0..30 {
            m.tick(&who, &los);
        }
        let h = 0;
        let t = m.contracts.hunters[h].target.unwrap();
        // the hunter runs within 10 m in sight: the threat meter fills at
        // 0.1/s, the chase starts when it is full (10 s)
        who[h].pos = who[t].pos - Vec3::new(8.0, 0.0, 0.0);
        who[h].running = true;
        let mut n = 0;
        while !m.events.contains(&MatchEvent::ChaseStarted { hunter: h, target: t }) {
            m.tick(&who, &los);
            n += 1;
            assert!(n <= 601, "threat {}", m.threat[h]);
        }
        assert!(n >= 599, "{n}");
        // out of sight, not hidden: 12 s to escape; back in sight resets
        see.set(false);
        for _ in 0..ticks(6.0) {
            m.tick(&who, &los);
        }
        assert!((m.chases[h].unwrap().coef - 0.5).abs() < 0.01);
        see.set(true);
        m.tick(&who, &los);
        assert_eq!(m.chases[h].unwrap().coef, 0.0);
        assert_eq!(m.chases[h].unwrap().phase, ChasePhase::InSight);
        // out of sight and hidden: 7 s
        see.set(false);
        who[t].hidden = true;
        let mut n = 0;
        while m.chases[h].is_some() {
            m.tick(&who, &los);
            n += 1;
            assert!(n < 1000, "{:?} {:?}", m.chases[h], m.contracts.hunters[h]);
        }
        assert!((ticks(7.0)..=ticks(7.0) + 1).contains(&n), "{n}");
        assert!(m.events.contains(&MatchEvent::Escaped { hunter: h, target: t }));
        assert_eq!(m.score[t], ESCAPE_POINTS);
        assert!(m.events.contains(&MatchEvent::Contract(ContractEvent::Lost { hunter: h, target: t })));
        assert_eq!(m.contracts.hunters[h].target, None);
        // a new contract after the cooldown
        for _ in 0..ticks(crate::rules::MIN_WAIT_TIME) + 1 {
            m.tick(&who, &los);
        }
        assert!(m.contracts.hunters[h].target.is_some());
    }

    #[test]
    fn falls_hurt_and_kill() {
        let mut m = m(2);
        let pos = [Vec3::ZERO, Vec3::new(20.0, 0.0, 0.0)];
        run(&mut m, 30, &pos);
        assert_eq!(m.contracts.relation(0, 1), Relation::Mutual);
        m.on_fall(1, 30);
        assert_eq!(m.health[1], 70);
        assert!(m.alive(1));
        m.on_fall(1, 75);
        assert!(!m.alive(1));
        assert!(m.events.contains(&MatchEvent::Fall { player: 1, damage: 75, fatal: true }));
        // no killer: nobody scores, the pursuer loses its contract
        assert_eq!(m.score, vec![0, 0]);
        assert!(m.events.contains(&MatchEvent::Contract(ContractEvent::Lost { hunter: 0, target: 1 })));
        run(&mut m, ticks(1.0) + 1, &pos);
        assert!(m.alive(1));
        assert_eq!(m.health[1], FULL_HEALTH);
        // a fatal fall (200 %) from full life
        m.on_fall(0, 200);
        assert!(!m.alive(0));
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

    #[test]
    fn awareness_terms() {
        let t = Presence::at(Vec3::ZERO);
        // close and slow: fully aware
        let h = Presence::at(Vec3::new(-5.0, 0.0, 0.0));
        assert!((awareness(&h, &t) - 1.0).abs() < 1e-6);
        // at the far distance: 1 / sqrt(1 + 4)
        let far = Presence::at(Vec3::new(-35.0, 0.0, 0.0));
        assert!((awareness(&far, &t) - 1.0 / 5f32.sqrt()).abs() < 1e-5);
        // fast, behind the target (target faces +Y, hunter at -Y), running
        // straight at it: only the behind term, 1 / sqrt(1 + 2)
        let h = Presence { pos: Vec3::new(0.0, -5.0, 0.0), speed: 5.0, ..Default::default() };
        assert!((awareness(&h, &t) - 1.0 / 3f32.sqrt()).abs() < 1e-5);
        // fast, in front of the target, running straight at it: aware
        let h = Presence { pos: Vec3::new(0.0, 5.0, 0.0), yaw: std::f32::consts::PI, speed: 5.0, ..Default::default() };
        assert!((awareness(&h, &t) - 1.0).abs() < 1e-5);
        // half the speed ramp halves the terms
        let h = Presence { pos: Vec3::new(0.0, -5.0, 0.0), speed: 3.0, ..Default::default() };
        assert!((awareness(&h, &t) - 1.0 / 2f32.sqrt()).abs() < 1e-5);
    }

    #[test]
    fn threat_in_air_and_retarget() {
        let mut m = m(3);
        m.config.time_limit = 100.0;
        let mut who = vec![Presence::at(Vec3::ZERO), Presence::at(Vec3::new(20.0, 0.0, 0.0)), Presence::at(Vec3::new(0.0, 30.0, 0.0))];
        let los = |_: usize, _: usize| true;
        for _ in 0..30 {
            m.tick(&who, &los);
        }
        let h = 0;
        let t = m.contracts.hunters[h].target.unwrap();
        who[h].pos = who[t].pos - Vec3::new(8.0, 0.0, 0.0);
        who[h].running = true;
        who[h].in_air = true;
        // InAirFastSpeed 0.65/s at full awareness: 1 s -> 0.65
        for _ in 0..60 {
            m.tick(&who, &los);
        }
        assert!((m.threat[h] - 0.65).abs() < 1e-3, "{}", m.threat[h]);
        // a new target starts the meter from 0
        let other = (0..3).find(|&p| p != h && p != t).unwrap();
        m.contracts.hunters[h].target = Some(other);
        m.tick(&who, &los);
        assert!(m.threat[h] < 0.05, "{}", m.threat[h]);
    }

    #[test]
    fn escort_kills_follow_teams_not_contracts() {
        let spawns = vec![(Vec3::ZERO, 0.0)];
        let cfg = MatchConfig { time_limit: 10.0, score_limit: 0, respawn_delay: 1.0, start_delay: 0.0 };
        let mut m = Match::escort(Teams::alternating(4), cfg, spawns);
        m.rounds = Some(crate::rounds::Rounds::new(2, 0.0, 10.0, 8.0));
        let pos = [Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(15.0, 0.0, 0.0)];
        run(&mut m, 60, &pos);
        assert!(m.contracts.hunters.iter().all(|h| h.target.is_none()), "no contracts in escort");
        assert_eq!(m.relation(0, 2), Relation::Other, "no friendly fire");
        assert_eq!(m.relation(1, 0), Relation::Target);
        assert_eq!(m.relation(0, 1), Relation::Pursuer);
        assert_eq!(m.relation(0, 9), Relation::Npc);
        // protector 1 kills hunter 0: scores, 0 respawns
        m.on_action(1, 0, 0);
        assert_eq!(m.score[1], KILL_POINTS);
        assert!(!m.alive(0));
        // hunter 2 kills its teammate... it cannot be granted, and would not score
        m.on_action(2, 0, 0);
        assert_eq!(m.score[2], 0);
        // hunter 2 stuns protector 3 (its pursuer)
        m.on_action(2, 3, crate::kill::STUN);
        assert_eq!(m.score[2], STUN_POINTS);
    }

    /// Retail VIP rule (`SortByVIPdistance` 0x0110fad8): beyond 35 m of
    /// every enemy the spawn nearest to a mate wins; inside, the one
    /// furthest from enemies.
    #[test]
    fn vip_rule_near_mates_far_from_enemies() {
        let v = |x: f32| Vec3::new(x, 0.0, 0.0);
        let spawns = [v(10.0), v(50.0), v(-60.0)];
        // protector: its VIP at 0 is a mate, a hunter at 100 an enemy
        assert_eq!(vip_rule(&spawns, &[v(0.0)], &[v(100.0)]), Some(0));
        // hunter: VIP at 0 and a protector at 5 are enemies, a mate at -80;
        // 10 has an enemy within 35 m, -60 is nearer the mate than 50
        assert_eq!(vip_rule(&spawns, &[v(-80.0)], &[v(0.0), v(5.0)]), Some(2));
        // every spawn within 35 m of an enemy: the furthest from them
        assert_eq!(vip_rule(&[v(10.0), v(30.0), v(-20.0)], &[v(30.0)], &[v(0.0)]), Some(1));
        // nobody around (all at the 2000 m default): the first
        assert_eq!(vip_rule(&spawns, &[], &[]), Some(0));
        assert_eq!(vip_rule(&[], &[], &[]), None);
    }

    /// Escort respawns: a protector comes back by its VIP, a hunter away
    /// from it; a spawn taken the same tick is skipped.
    #[test]
    fn escort_respawn_by_role() {
        let v = |x: f32| Vec3::new(x, 0.0, 0.0);
        let spawns = vec![(v(5.0), 0.0), (v(60.0), 0.0), (v(120.0), 0.0)];
        let cfg = MatchConfig { time_limit: 10.0, score_limit: 0, respawn_delay: 1.0, start_delay: 0.0 };
        let mut m = Match::escort(Teams::alternating(4), cfg, spawns);
        let mut vips = crate::vip::Vips::new(vec![10, 11]);
        let walker = crate::vip::Walker { pos: v(0.0), yaw: 0.0, next: 1 };
        vips.slots[0] = Some(crate::vip::Vip { way: 0, back: false, walker, checkpoints: 0, phase: 0.0 });
        m.vips = Some(vips);
        // team 0 hunts; players 0, 2 hunters, 1, 3 protectors, far away
        let pos = [v(200.0), v(300.0), v(200.0), v(300.0)];
        // protector 1: by its VIP (mates: VIP at 0, player 3 at 300)
        assert_eq!(m.pick_spawn(1, &pos, &[]), 0);
        // ... unless that spawn is taken this tick
        assert_eq!(m.pick_spawn(1, &pos, &[0]), 1);
        // hunter 0: enemies VIP at 0 and protectors at 300, mate at 200:
        // all spawns beyond 35 m of enemies but 5 -> nearest the mate
        assert_eq!(m.pick_spawn(0, &pos, &[]), 2);
    }

    /// E4 gate: a scripted round pair in ticks (intro 1 s, play 4 s, recap
    /// 1 s): kills, VIP kills and VIP way events give the expected team
    /// totals, teams swap roles for round 2, nothing scores outside play.
    #[test]
    fn escort_round_pair_scores_and_swaps() {
        use crate::rounds::{Phase, RoundEvent, Rounds};
        use crate::vip::VipEvent;
        let spawns = vec![(Vec3::ZERO, 0.0)];
        let cfg = MatchConfig { time_limit: 4.0, score_limit: 0, respawn_delay: 1.0, start_delay: 0.0 };
        let mut m = Match::escort(Teams::alternating(4), cfg, spawns);
        m.vips = Some(crate::vip::Vips::new(vec![4, 5]));
        m.rounds = Some(Rounds::new(2, 1.0, 4.0, 1.0));
        let pos = [Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(15.0, 0.0, 0.0)];
        let who: Vec<Presence> = pos.iter().map(|&p| Presence::at(p)).collect();
        let killed = |vip, killer| VipEvent::Killed { vip, body: 4 + vip, killer, way: 0 };
        let checkpoint = |n| VipEvent::Checkpoint { vip: 0, way: 0, checkpoint: 3, n };
        let mut rounds_log = Vec::new();
        let mut tick = |m: &mut Match| {
            m.events.clear();
            m.tick(&who, &|_, _| true);
            rounds_log.extend(m.events.iter().filter_map(|e| if let MatchEvent::Round(r) = e { Some((m.ticks, *r)) } else { None }));
        };
        let team_score = |m: &Match| m.rounds.as_ref().unwrap().team_score.clone();
        // intro: no kills (relation Other), VIP events do not score
        tick(&mut m);
        assert_eq!(m.relation(1, 0), Relation::Other);
        m.on_action(1, 0, 0);
        m.on_vip_events(&[killed(0, 0), checkpoint(1)]);
        assert_eq!(team_score(&m), vec![0, 0]);
        assert_eq!(m.score, vec![0; 4]);
        while m.ticks < ticks(1.0) + 1 {
            tick(&mut m);
        }
        // round 1 (team 0 hunts): VIP kill 100 for 0; protector 1 kills
        // hunter 0 for 100; VIP 1 passes 2 checkpoints (10 + 20) and
        // reaches its way's end (200 + 2 * 10) for team 1
        assert!(m.playing());
        assert_eq!(m.relation(1, 0), Relation::Target);
        m.on_vip_events(&[killed(0, 0)]);
        m.on_action(1, 0, crate::kill::WALK);
        m.on_vip_events(&[checkpoint(1)]);
        m.on_vip_events(&[checkpoint(2), VipEvent::WayEnd { vip: 1, way: 0, n: 2 }]);
        assert_eq!(team_score(&m), vec![100, 350]);
        assert!(m.events.contains(&MatchEvent::TeamScore { team: 1, points: 220, reason: ScoreReason::Protect }));
        // the VIP escaped: play ends on the next tick (rounds::ROUND_ENDS_ON_ESCAPE)
        assert!(m.events.contains(&MatchEvent::Round(RoundEvent::Escaped { round: 0, team: 1 })));
        tick(&mut m);
        // recap: still nothing scores
        assert_eq!(m.rounds.as_ref().unwrap().phase, Phase::Recap);
        m.on_vip_events(&[killed(1, 2)]);
        assert_eq!(team_score(&m), vec![100, 350]);
        while m.ticks < ticks(3.0) + 2 {
            tick(&mut m);
        }
        // round 2: team 1 hunts; the VIP cycle restarted
        let Mode::Escort(t) = &m.mode else { unreachable!() };
        assert_eq!(t.attacking_team, 1);
        assert_eq!(m.relation(0, 1), Relation::Target, "protector 0 may kill hunter 1");
        assert_eq!(m.vips.as_ref().unwrap().respawn_in, Some(0));
        m.on_vip_events(&[killed(0, 1)]);
        m.on_action(2, 3, crate::kill::WALK);
        m.on_vip_events(&[checkpoint(1)]);
        assert_eq!(team_score(&m), vec![210, 450]);
        assert_eq!(m.rounds.as_ref().unwrap().round_score, vec![vec![100, 350], vec![110, 100]]);
        assert_eq!(m.score, vec![100, 200, 100, 0]);
        while !m.over {
            tick(&mut m);
        }
        let s = ticks(1.0);
        assert_eq!(
            rounds_log,
            vec![
                (s + 1, RoundEvent::Phase { round: 0, phase: Phase::Play }),
                // escaped one tick into play
                (s + 2, RoundEvent::Phase { round: 0, phase: Phase::Recap }),
                (2 * s + 2, RoundEvent::Swap { round: 1, attacking: 1 }),
                (2 * s + 2, RoundEvent::Phase { round: 1, phase: Phase::Intro }),
                (3 * s + 2, RoundEvent::Phase { round: 1, phase: Phase::Play }),
                (7 * s + 2, RoundEvent::Phase { round: 1, phase: Phase::Recap }),
                (8 * s + 2, RoundEvent::Phase { round: 1, phase: Phase::Over }),
                (8 * s + 2, RoundEvent::Over { winner: Some(1) }),
            ]
        );
    }

    // --- Assassinate (A1/A2)

    fn asn(n: usize) -> Match {
        let spawns = vec![(Vec3::ZERO, 0.0), (Vec3::new(50.0, 0.0, 0.0), 0.0), (Vec3::new(0.0, 40.0, 0.0), 0.0)];
        Match::assassinate(n, MatchConfig { time_limit: ASSASSINATE_TIME, score_limit: 1000, respawn_delay: 1.0, start_delay: 0.0 }, spawns)
    }

    /// Player 0 at the origin looking along +x (`look` 0) at player 1 5 m
    /// away; player 1 looks away.
    fn asn_who() -> Vec<Presence> {
        vec![Presence::at(Vec3::ZERO), Presence { look: -std::f32::consts::FRAC_PI_2, ..Presence::at(Vec3::new(5.0, 0.0, 0.0)) }]
    }

    /// A kill by `k` on `v` standing 1 m behind it, through the match's
    /// relation (what `World::step` hands `rules::check`).
    fn kill_ok(m: &Match, k: usize, v: usize) -> bool {
        use crate::rules::{check, Act, Actor, Stance, PAIR_SPACING};
        let a = Actor { pos: Vec3::new(-1.0, 0.0, 0.0), yaw: -std::f32::consts::FRAC_PI_2, stance: Stance::Ground, speed: 0.0 };
        let b = Actor { pos: Vec3::ZERO, yaw: -std::f32::consts::FRAC_PI_2, stance: Stance::Ground, speed: 0.0 };
        check(Act::Kill, &a, &b, m.relation(k, v), &m.rules(), PAIR_SPACING).is_ok()
    }

    fn steps(m: &mut Match, who: &[Presence], n: u32) {
        for _ in 0..n {
            m.tick(who, &|_, _| true);
        }
    }

    #[test]
    fn assassinate_release_and_lock() {
        let mut m = asn(2);
        let mut who = vec![Presence::at(Vec3::ZERO), Presence { look: -std::f32::consts::FRAC_PI_2, ..Presence::at(Vec3::new(5.0, 0.0, 0.0)) }];
        let mut n = 0;
        while m.contracts.hunters[0].target.is_none() {
            steps(&mut m, &who, 1);
            n += 1;
            assert!(n < 2000);
        }
        // a release in a kill or stun is refused
        who[0].release = true;
        who[0].conflict = true;
        steps(&mut m, &who, 1);
        assert_eq!(m.contracts.hunters[0].target, Some(1));
        // the button is still held: no new press
        who[0].conflict = false;
        steps(&mut m, &who, 1);
        assert_eq!(m.contracts.hunters[0].target, Some(1));
        who[0].release = false;
        steps(&mut m, &who, 1);
        who[0].release = true;
        m.tick(&who, &|_, _| true);
        assert_eq!(m.contracts.hunters[0].target, None);
        assert!(m.events.contains(&MatchEvent::Released { hunter: 0, target: 1 }));
        assert!(m.events.contains(&MatchEvent::Contract(ContractEvent::Lost { hunter: 0, target: 1 })));
        // a lock pressed on the cooled-down highlight holds it behind a wall
        who[0].release = false;
        steps(&mut m, &who, crate::rules::RELEASE_COOLDOWN_TICKS);
        who[0].lock = true;
        steps(&mut m, &who, 1);
        assert_eq!(m.acquire[0].lock, Some(1));
        for _ in 0..crate::rules::ACQUIRE_TICKS {
            m.tick(&who, &|_, _| false);
        }
        assert_eq!(m.contracts.hunters[0].target, Some(1), "locked: no line of sight needed");
    }

    #[test]
    fn assassinate_no_kill_before_acquisition() {
        let mut m = asn(3);
        assert_eq!(m.mode, Mode::Assassinate);
        assert_eq!(m.config.time_limit, 600.0);
        // nobody looks at anybody: no lottery, no contract, ever
        let who = vec![Presence { look: 3.0, ..Presence::at(Vec3::ZERO) }, Presence { look: -std::f32::consts::FRAC_PI_2, ..Presence::at(Vec3::new(5.0, 0.0, 0.0)) }, Presence { look: 3.0, ..Presence::at(Vec3::new(0.0, 5.0, 0.0)) }];
        steps(&mut m, &who, 600);
        for k in 0..3 {
            for v in 0..3 {
                if k != v {
                    assert!(!kill_ok(&m, k, v), "{k} -> {v}");
                }
            }
        }
        assert!(m.contracts.hunters.iter().all(|h| h.target.is_none()));
        // a kill outside the rules (e.g. a desync) still scores nothing
        m.on_action(0, 1, crate::kill::WALK);
        assert_eq!(m.score, vec![0, 0, 0]);
    }

    #[test]
    fn assassinate_acquire_at_tick_180() {
        let mut m = asn(2);
        let who = asn_who();
        steps(&mut m, &who, 1);
        assert_eq!(m.events, vec![MatchEvent::Acquire(AcquireEvent::Acquiring { hunter: 0, target: 1 })]);
        assert_eq!(m.acquirers(1), vec![0], "the target is warned");
        steps(&mut m, &who, 178);
        assert_eq!(m.acquire[0].look, 179);
        assert!(!kill_ok(&m, 0, 1), "tick 179: refused");
        steps(&mut m, &who, 1);
        assert!(m.events.contains(&MatchEvent::Contract(ContractEvent::Assigned { hunter: 0, target: 1 })));
        assert!(kill_ok(&m, 0, 1), "tick 180: granted");
        assert_eq!(m.relation(0, 1), Relation::Target);
        assert!(m.acquirers(1).is_empty());
        assert!(!kill_ok(&m, 1, 0), "one-way: 1 never looked at 0");
        m.on_action(0, 1, crate::kill::WALK);
        let got: Vec<_> = m.events.iter().filter_map(|e| match e { MatchEvent::Bonus { bonus, .. } => Some(*bonus), _ => None }).collect();
        // standing 3 s within 10 m, never on 1's threat meter, first kill
        use crate::bonus::Bonus;
        assert_eq!(got, vec![Bonus::Proximity, Bonus::Incognito, Bonus::FirstBlood]);
        assert_eq!(m.score, vec![KILL_POINTS + 150 + 300 + 50, 0]);
        assert!(m.events.contains(&MatchEvent::Contract(ContractEvent::Fulfilled { hunter: 0, target: 1 })));
        assert_eq!(m.acquire[0], Acquire::default(), "nobody looks at the dead");
    }

    #[test]
    fn assassinate_high_profile_at_once() {
        let mut m = asn(2);
        let mut who = asn_who();
        who[1].running = true;
        steps(&mut m, &who, 1);
        assert_eq!(m.events, vec![MatchEvent::Contract(ContractEvent::Assigned { hunter: 0, target: 1 })], "no warning: filled instantly");
        assert!(kill_ok(&m, 0, 1));
    }

    #[test]
    fn assassinate_memory_120_ticks() {
        let mut m = asn(2);
        let mut who = asn_who();
        steps(&mut m, &who, 180);
        assert_eq!(m.relation(0, 1), Relation::Target);
        // look away 119 ticks and back: still held, no new wait
        who[0].look = std::f32::consts::PI;
        steps(&mut m, &who, 119);
        assert_eq!(m.relation(0, 1), Relation::Target);
        who[0].look = 0.0;
        steps(&mut m, &who, 1);
        assert_eq!(m.relation(0, 1), Relation::Target);
        assert!(m.acquire[0].memory.is_empty());
        // away 120 ticks: lost, and the next look takes the full 180 again
        who[0].look = std::f32::consts::PI;
        steps(&mut m, &who, 119);
        assert_eq!(m.relation(0, 1), Relation::Target);
        steps(&mut m, &who, 1);
        assert!(m.events.contains(&MatchEvent::Contract(ContractEvent::Lost { hunter: 0, target: 1 })));
        assert_eq!(m.relation(0, 1), Relation::Other);
        who[0].look = 0.0;
        steps(&mut m, &who, 179);
        assert_eq!(m.relation(0, 1), Relation::Other);
        steps(&mut m, &who, 1);
        assert_eq!(m.relation(0, 1), Relation::Target);
    }

    #[test]
    fn assassinate_switch_keeps_old_target_warm() {
        let mut m = asn(3);
        // 1 at +x, 2 at +y; 0 acquires 1, then 2, then 1 again
        let mut who = vec![Presence::at(Vec3::ZERO), Presence { look: std::f32::consts::FRAC_PI_2, ..Presence::at(Vec3::new(5.0, 0.0, 0.0)) }, Presence { look: 3.0, ..Presence::at(Vec3::new(0.0, 5.0, 0.0)) }];
        steps(&mut m, &who, 180);
        assert_eq!(m.contracts.hunters[0].target, Some(1));
        who[0].look = std::f32::consts::FRAC_PI_2;
        steps(&mut m, &who, 100);
        assert_eq!(m.contracts.hunters[0].target, Some(1), "held from memory while acquiring 2");
        assert_eq!(m.acquirers(2), vec![0]);
        steps(&mut m, &who, 20);
        assert_eq!(m.contracts.hunters[0].target, None, "120 ticks away from 1: lost");
        steps(&mut m, &who, 60);
        assert_eq!(m.contracts.hunters[0].target, Some(2), "180 ticks on 2");
        // back to 1: the full time again; 2 now waits in memory
        who[0].look = 0.0;
        steps(&mut m, &who, 180);
        assert_eq!(m.contracts.hunters[0].target, Some(1));
        // 2 runs (High Profile): at once; 1 is remembered, re-focused within
        // 120 ticks it is back at once
        who[2].running = true;
        who[0].look = std::f32::consts::FRAC_PI_2;
        steps(&mut m, &who, 1);
        assert_eq!(m.contracts.hunters[0].target, Some(2));
        who[0].look = 0.0;
        steps(&mut m, &who, 1);
        assert_eq!(m.contracts.hunters[0].target, Some(1), "1 still remembered: instant");
    }

    #[test]
    fn assassinate_target_may_stun_acquirer() {
        let mut m = asn(2);
        let who = asn_who();
        assert_eq!(m.relation(1, 0), Relation::Other);
        steps(&mut m, &who, 60);
        assert_eq!(m.relation(1, 0), Relation::Pursuer, "warned target sees its acquirer as a pursuer");
        m.on_action(1, 0, crate::kill::STUN);
        assert_eq!(m.score, vec![0, STUN_POINTS]);
        assert_eq!(m.acquire[0], Acquire::default(), "the stunned acquirer starts over");
        assert_eq!(m.relation(1, 0), Relation::Other);
    }

    // --- Assassinate contract events (A3)

    /// 0 holds a contract on 1 (and 2 on 1 when `three`).
    fn asn_held(three: bool) -> (Match, Vec<Presence>) {
        let mut m = asn(if three { 3 } else { 2 });
        let mut who = asn_who();
        if three {
            // 2 at -x looking away
            who.push(Presence { look: std::f32::consts::PI, ..Presence::at(Vec3::new(-5.0, 0.0, 0.0)) });
        }
        steps(&mut m, &who, 180);
        assert_eq!(m.contracts.hunters[0].target, Some(1));
        (m, who)
    }

    #[test]
    fn assassinate_event_target_death_by_other_loses() {
        let (mut m, _) = asn_held(true);
        // 2 also holds 1 (set by hand: what matters is the death)
        m.contracts.hunters[2].target = Some(1);
        m.on_action(2, 1, crate::kill::WALK);
        let got: Vec<_> = m.events.iter().filter_map(|e| match e { MatchEvent::Bonus { bonus, .. } => Some(*bonus), _ => None }).collect();
        // 2's nearest player is 0, not 1 (no focus); not on 1's threat meter; first kill
        assert_eq!(got, vec![crate::bonus::Bonus::Incognito, crate::bonus::Bonus::FirstBlood]);
        assert_eq!(m.score, vec![0, 0, KILL_POINTS + 300 + 50]);
        assert!(m.events.contains(&MatchEvent::Contract(ContractEvent::Fulfilled { hunter: 2, target: 1 })));
        assert!(m.events.contains(&MatchEvent::Contract(ContractEvent::Lost { hunter: 0, target: 1 })));
        assert_eq!(m.contracts.hunters[0].target, None);
        assert!(m.acquire.iter().all(|a| a.focus != Some(1) && a.memory.iter().all(|&(p, _)| p != 1)));
    }

    #[test]
    fn assassinate_event_stun_by_target_loses() {
        let (mut m, _) = asn_held(false);
        assert_eq!(m.relation(1, 0), Relation::Pursuer);
        m.on_action(1, 0, crate::kill::STUN);
        assert_eq!(m.score, vec![0, STUN_POINTS]);
        assert!(m.events.contains(&MatchEvent::Contract(ContractEvent::Lost { hunter: 0, target: 1 })));
        assert_eq!(m.relation(0, 1), Relation::Other);
        assert_eq!(m.acquire[0], Acquire::default());
    }

    #[test]
    fn assassinate_event_npc_kill_loses() {
        let (mut m, _) = asn_held(false);
        m.on_npc_kill(0);
        assert_eq!(m.events.last(), Some(&MatchEvent::Contract(ContractEvent::Lost { hunter: 0, target: 1 })));
        assert_eq!(m.relation(0, 1), Relation::Other);
        assert_eq!(m.score, vec![0, 0], "an NPC pays nothing");
        // the duel ignores NPC kills
        let mut d = m_duel_with_contract();
        d.on_npc_kill(0);
        assert_eq!(d.contracts.hunters[0].target, Some(1));
    }

    fn m_duel_with_contract() -> Match {
        let mut d = m(2);
        d.contracts.hunters[0].target = Some(1);
        d
    }
}
