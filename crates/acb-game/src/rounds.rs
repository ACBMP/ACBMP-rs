//! Escort (teamvip) scoring and rounds (Phase 4 E4): team totals, the
//! intro / play / recap phases of a round and the attacking-team swap
//! between rounds. Pure and in ticks: part of the simulated match state.
//!
//! Retail sources (GBrick defaults, the teamvip XML did not ship):
//! `GBrick_ScoreKill::ReadFromXML` 0x0103832c (ScoreVIPKilled 100,
//! ScoreEnemyKilled 100), `GBrick_ScoreVIP::ReadFromXML` 0x01037fd0 +
//! `GameInfo::GetScoreProtectVIP` 0x0111182c / `GetScoreVIPCheckpoint`
//! 0x01111876, `GBrick_Round::ReadFromXML` 0x00f4af42 (IntroDuration 8,
//! ScoreRecapDuration 8).

use crate::mode::ticks;
use crate::rules::Team;

/// ScoreVIPKilled (GBrick_ScoreKill default): a hunter kills a VIP.
pub const VIP_KILLED_POINTS: u32 = 100;
/// ScoreEnemyKilled (GBrick_ScoreKill default): = `mode::KILL_POINTS`.
pub const ENEMY_KILLED_POINTS: u32 = 100;
/// ScoreVIPBase / ScoreVIPBonus / ScoreVIPMAX (GBrick_ScoreVIP defaults).
pub const SCORE_VIP_BASE: u32 = 200;
pub const SCORE_VIP_BONUS: u32 = 10;
pub const SCORE_VIP_MAX: u32 = 500;
/// Rounds: retail default NumberOfRounds is 3, our reading is that teamvip
/// sets nbRoundFromNbTeams (2 teams -> 2 rounds, each team attacks once).
pub const ROUNDS: u32 = 2;
/// IntroDuration (GBrick_Round default), seconds.
pub const INTRO_DURATION: f32 = 8.0;
/// ScoreRecapDuration (GBrick_Round default), seconds.
pub const RECAP_DURATION: f32 = 8.0;
/// PLACEHOLDER: playing time of one round, seconds. Not in the shipped
/// data (teamvip XML missing); needs a retail capture.
pub const ROUND_LENGTH: f32 = 300.0;

/// Protectors' points when a VIP passes a checkpoint, `n` = checkpoints that
/// VIP passed so far (`GetScoreVIPCheckpoint`: min(n * bonus, max - base)).
/// PLACEHOLDER reading: what retail counts in `n` and that the event pays
/// this (GameModeEvent 0xf sub 1 -> GetScoreFromReason(5, n)) is ours.
pub fn checkpoint_points(n: u32) -> u32 {
    (n * SCORE_VIP_BONUS).min(SCORE_VIP_MAX - SCORE_VIP_BASE)
}

/// Protectors' points when a VIP reaches its way's end having passed `n`
/// checkpoints (`GetScoreProtectVIP`: min(base + n * bonus, max)).
/// PLACEHOLDER reading: paid on our WayEnd (retail VipReachedDestination).
pub fn protect_points(n: u32) -> u32 {
    (SCORE_VIP_BASE + n * SCORE_VIP_BONUS).min(SCORE_VIP_MAX)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Before play: no kills, no VIPs.
    Intro,
    Play,
    /// Score recap after play: no kills, VIPs frozen.
    Recap,
    /// After the last recap.
    Over,
}

/// What [`Rounds::tick`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundEvent {
    /// Round `round` (0-based) entered `phase`.
    Phase { round: u32, phase: Phase },
    /// Round `round` starts with `attacking` hunting the VIPs.
    Swap { round: u32, attacking: Team },
    /// The last round's recap ended; `winner` is `None` on a draw.
    Over { winner: Option<Team> },
}

/// Rounds of an Escort match and the team totals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rounds {
    /// Phase lengths, ticks.
    pub intro: u32,
    pub play: u32,
    pub recap: u32,
    pub rounds: u32,
    /// Current round, 0-based.
    pub round: u32,
    pub phase: Phase,
    /// Ticks left in the phase.
    pub left: u32,
    /// Team totals over all rounds (index = team). Our reading: summed over
    /// both rounds (retail CumulativeScores default 0 is unclear here).
    pub team_score: Vec<u32>,
    /// Per round, per team.
    pub round_score: Vec<Vec<u32>>,
}

impl Rounds {
    /// `teams` teams, phase lengths in seconds.
    pub fn new(teams: usize, intro: f32, play: f32, recap: f32) -> Rounds {
        let intro = ticks(intro);
        Rounds {
            intro,
            play: ticks(play),
            recap: ticks(recap),
            rounds: ROUNDS,
            round: 0,
            phase: Phase::Intro,
            left: intro,
            team_score: vec![0; teams],
            round_score: vec![vec![0; teams]; ROUNDS as usize],
        }
    }

    /// Retail phase lengths with a round of `play` seconds.
    pub fn retail(teams: usize, play: f32) -> Rounds {
        Rounds::new(teams, INTRO_DURATION, play, RECAP_DURATION)
    }

    pub fn playing(&self) -> bool {
        self.phase == Phase::Play
    }

    /// Adds `points` to `team` (only while playing). Returns whether it counted.
    pub fn add(&mut self, team: Team, points: u32) -> bool {
        let t = team as usize;
        if !self.playing() || t >= self.team_score.len() {
            return false;
        }
        self.team_score[t] += points;
        if let Some(r) = self.round_score.get_mut(self.round as usize) {
            r[t] += points;
        }
        true
    }

    /// One tick; `attacking` is the attacking team, swapped here at the
    /// start of every round after the first.
    pub fn tick(&mut self, attacking: &mut Team) -> Vec<RoundEvent> {
        let mut out = Vec::new();
        // a zero-length phase passes on the same tick
        while self.phase != Phase::Over && self.left == 0 {
            match self.phase {
                Phase::Intro => {
                    self.phase = Phase::Play;
                    self.left = self.play;
                }
                Phase::Play => {
                    self.phase = Phase::Recap;
                    self.left = self.recap;
                }
                Phase::Recap if self.round + 1 < self.rounds => {
                    self.round += 1;
                    *attacking = ((*attacking as usize + 1) % self.team_score.len().max(1)) as Team;
                    out.push(RoundEvent::Swap { round: self.round, attacking: *attacking });
                    self.phase = Phase::Intro;
                    self.left = self.intro;
                }
                Phase::Recap => {
                    self.phase = Phase::Over;
                    let best = self.team_score.iter().copied().max().unwrap_or(0);
                    let leaders: Vec<usize> = (0..self.team_score.len()).filter(|&t| self.team_score[t] == best).collect();
                    out.push(RoundEvent::Phase { round: self.round, phase: Phase::Over });
                    out.push(RoundEvent::Over { winner: (leaders.len() == 1).then(|| leaders[0] as Team) });
                    return out;
                }
                Phase::Over => unreachable!(),
            }
            out.push(RoundEvent::Phase { round: self.round, phase: self.phase });
        }
        if self.phase != Phase::Over {
            self.left -= 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retail_score_formulas() {
        assert_eq!(checkpoint_points(1), 10);
        assert_eq!(checkpoint_points(40), 300);
        assert_eq!(protect_points(0), 200);
        assert_eq!(protect_points(3), 230);
        assert_eq!(protect_points(99), 500);
    }

    #[test]
    fn two_rounds_swap_and_end() {
        let mut r = Rounds::new(2, 1.0, 2.0, 1.0);
        let mut att: Team = 0;
        let mut log = Vec::new();
        for tick in 1..=ticks(10.0) {
            for e in r.tick(&mut att) {
                log.push((tick, e));
            }
        }
        // intro 60 ticks: play from tick 61, recap from 181, round 2 intro 241
        assert_eq!(log[0], (61, RoundEvent::Phase { round: 0, phase: Phase::Play }));
        assert_eq!(log[1], (181, RoundEvent::Phase { round: 0, phase: Phase::Recap }));
        assert_eq!(log[2], (241, RoundEvent::Swap { round: 1, attacking: 1 }));
        assert_eq!(log[3], (241, RoundEvent::Phase { round: 1, phase: Phase::Intro }));
        assert_eq!(log[4], (301, RoundEvent::Phase { round: 1, phase: Phase::Play }));
        assert_eq!(log[5], (421, RoundEvent::Phase { round: 1, phase: Phase::Recap }));
        assert_eq!(log[6], (481, RoundEvent::Phase { round: 1, phase: Phase::Over }));
        assert_eq!(log[7], (481, RoundEvent::Over { winner: None }));
        assert_eq!(log.len(), 8);
        assert_eq!(att, 1);
    }
}
