//! Assassinate kill bonuses and the BloodBath chain (A4).
//!
//! Values are the `GBrick_ScoreKill::ReadFromXML` defaults (Mac 0x0103832c;
//! the mode XMLs are not shipped). Which bonuses one kill gets follows
//! `GBrick_ScoreKill::HandleConflictResolvedEvent` (0x0103beae): Proximity
//! (the conflict's "focus" flag), then exactly one of Incognito > Silent >
//! Unspotted (Unspotted only when the killer is not chasing), then
//! FirstBlood and Revenge. The flags themselves come from
//! `GameInfo::ComputeReducedField` (0x00c5b7b0), which was only partly read:
//! the conditions below are the shipped bonus texts where there is one (C)
//! and our reading otherwise (R/P, see each constant).
//!
//! Overkill (200, 10 s window) is a default too, but `GameInfo::
//! AddBonusOverkill` (0x01111d18) has no caller in the Mac build: retail
//! never awards it, neither do we.
//!
//! Pure and deterministic: [`Tracker`] lives in [`crate::mode::Match`], is
//! stepped by the match clock and carried by the net snapshot.

/// `BonusSilent` default 200 (0x0103832c).
pub const SILENT: u32 = 200;
/// `BonusUnspotted` default 50 (0x0103832c).
pub const UNSPOTTED: u32 = 50;
/// `BonusIncognito` default 300 (0x0103832c).
pub const INCOGNITO: u32 = 300;
/// `BonusFirstBlood` default 50, `AllowFirstBlood` 1 (0x0103832c).
pub const FIRST_BLOOD: u32 = 50;
/// `BonusRevenge` default 50 (0x0103832c).
pub const REVENGE: u32 = 50;
/// `BonusProximity` default 150 (0x0103832c); the in-game name is FOCUS
/// (UIString 0x4b16f "Focus +#ScoreOrBonus#").
pub const PROXIMITY: u32 = 150;
/// `BonusOverkill` default 200 (0x0103832c); never awarded (no caller of
/// `AddBonusOverkill` 0x01111d18).
pub const OVERKILL: u32 = 200;
/// Overkill window, 10 s (0x0103832c); unused (see [`OVERKILL`]).
pub const OVERKILL_WINDOW: f32 = 10.0;

/// Focus: "Stay close to a target for at least 3 seconds before killing the
/// target" (UIString 0x4d0cd, C).
pub const FOCUS_TICKS: u32 = 180;
/// Placeholder: how close "close" is. Retail compares the squared distance
/// to the nearest target with a GameInfo float (+0x5c4, not found); we use
/// Wanted2's `ThreatClose` 10 m (GamemodeParameters 0x71e41bec).
pub const FOCUS_RANGE: f32 = 10.0;
/// Placeholder: Silent = "Kill your target while remaining out of high
/// profile" (UIString 0x4d0d2, C); how long "remaining" is is ours: no high
/// profile for the last 3 s.
pub const SILENT_TICKS: u32 = 180;

/// `GBrick_BloodBath::ReadFromXML` (0x0157093e) defaults: a chain of
/// `NbKills` 3 within `Duration` 10 s.
pub const BLOODBATH_KILLS: u32 = 3;
pub const BLOODBATH_TICKS: u32 = 600;

/// One kill bonus (the ones a kill of Assassinate can get here; the duel's
/// Acrobatic/Hidden stay in [`crate::mode`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bonus {
    Proximity = 0,
    Incognito = 1,
    Silent = 2,
    Unspotted = 3,
    FirstBlood = 4,
    Revenge = 5,
}

impl Bonus {
    pub fn points(self) -> u32 {
        match self {
            Bonus::Proximity => PROXIMITY,
            Bonus::Incognito => INCOGNITO,
            Bonus::Silent => SILENT,
            Bonus::Unspotted => UNSPOTTED,
            Bonus::FirstBlood => FIRST_BLOOD,
            Bonus::Revenge => REVENGE,
        }
    }

    pub fn from_u8(v: u8) -> Option<Bonus> {
        use Bonus::*;
        [Proximity, Incognito, Silent, Unspotted, FirstBlood, Revenge].get(v as usize).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            Bonus::Proximity => "focus",
            Bonus::Incognito => "incognito",
            Bonus::Silent => "silent",
            Bonus::Unspotted => "unspotted",
            Bonus::FirstBlood => "first blood",
            Bonus::Revenge => "revenge",
        }
    }
}

/// What a scoring kill looked like (the inputs of [`bonuses`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Situation {
    /// The killer stayed within [`FOCUS_RANGE`] of the victim (its nearest
    /// other player) for [`FOCUS_TICKS`].
    pub focus: bool,
    /// R: the victim's threat meter on the killer was still empty (the
    /// victim's compass never showed it). Incognito has no shipped
    /// description; 0x4d14c "Kill 2 targets INCOGNITO without entering high
    /// profile" only says it is a stealth bonus.
    pub incognito: bool,
    /// No high profile for [`SILENT_TICKS`].
    pub silent: bool,
    /// The killer is in a chase (`NetPlayer::GetIsChasing`): no Unspotted.
    pub chasing: bool,
    /// The first scoring kill of the match (UIString 0x4bd59).
    pub first_blood: bool,
    /// The victim is the player who killed the killer last, and nobody else
    /// killed it since (UIString 0x4bd57).
    pub revenge: bool,
}

/// The bonuses of one scoring kill, in retail's order (0x0103beae).
pub fn bonuses(s: &Situation) -> Vec<Bonus> {
    let mut out = Vec::new();
    if s.focus {
        out.push(Bonus::Proximity);
    }
    if s.incognito {
        out.push(Bonus::Incognito);
    } else if s.silent {
        out.push(Bonus::Silent);
    } else if !s.chasing {
        out.push(Bonus::Unspotted);
    }
    if s.first_blood {
        out.push(Bonus::FirstBlood);
    }
    if s.revenge {
        out.push(Bonus::Revenge);
    }
    out
}

/// One remembered kill (BloodBath memory entry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    pub killer: usize,
    pub victim: usize,
    /// Ticks until the entry is dropped.
    pub left: u32,
}

/// The per-match bonus state (Assassinate).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tracker {
    /// Per player: ticks since it was last in high profile (saturating).
    pub quiet: Vec<u32>,
    /// Per player: its nearest other living player within [`FOCUS_RANGE`]
    /// and for how many ticks in a row.
    pub close: Vec<(Option<usize>, u32)>,
    /// Per player: who killed it last (cleared once anyone kills that one).
    pub last_killer: Vec<Option<usize>>,
    /// The first blood went.
    pub first_blood: bool,
    /// BloodBath kill memory.
    pub memory: Vec<Memory>,
}

impl Tracker {
    pub fn new(players: usize) -> Tracker {
        Tracker { quiet: vec![0; players], close: vec![(None, 0); players], last_killer: vec![None; players], first_blood: false, memory: Vec::new() }
    }

    /// One tick: `pos`, `running` (high profile) and `alive` per player.
    pub fn tick(&mut self, pos: &[glam::Vec3], running: &[bool], alive: &[bool]) {
        for p in 0..self.quiet.len().min(pos.len()) {
            self.quiet[p] = if running[p] || !alive[p] { 0 } else { self.quiet[p].saturating_add(1) };
            let near = (0..pos.len())
                .filter(|&q| q != p && alive[q] && alive[p])
                .map(|q| (q, pos[p].distance(pos[q])))
                .filter(|&(_, d)| d <= FOCUS_RANGE)
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
                .map(|(q, _)| q);
            self.close[p] = match (near, self.close[p]) {
                (Some(q), (Some(was), n)) if q == was => (Some(q), n.saturating_add(1)),
                (Some(q), _) => (Some(q), 1),
                (None, _) => (None, 0),
            };
        }
        self.memory.retain_mut(|m| {
            m.left = m.left.saturating_sub(1);
            m.left > 0
        });
    }

    /// The tracked part of a kill's [`Situation`].
    pub fn situation(&self, killer: usize, victim: usize) -> Situation {
        Situation {
            focus: self.close.get(killer).is_some_and(|&(q, n)| q == Some(victim) && n >= FOCUS_TICKS),
            silent: self.quiet.get(killer).is_some_and(|&n| n >= SILENT_TICKS),
            first_blood: !self.first_blood,
            revenge: self.last_killer.get(killer).is_some_and(|&k| k == Some(victim)),
            ..Default::default()
        }
    }

    /// Player `killer` killed player `victim` (`scored`: a contract kill,
    /// whose first blood counts). Returns the BloodBath chain length when it
    /// reached [`BLOODBATH_KILLS`] (the memory is then cleared).
    pub fn on_kill(&mut self, killer: usize, victim: usize, scored: bool) -> Option<u32> {
        if scored {
            self.first_blood = true;
        }
        // "before anyone else": a death ends every revenge on the victim
        for k in &mut self.last_killer {
            if *k == Some(victim) {
                *k = None;
            }
        }
        if let Some(k) = self.last_killer.get_mut(victim) {
            *k = Some(killer);
        }
        self.memory.push(Memory { killer, victim, left: BLOODBATH_TICKS });
        let n = chain(&self.memory, killer, 0);
        if n >= BLOODBATH_KILLS {
            self.memory.clear();
            Some(n)
        } else {
            None
        }
    }
}

/// `GBrick_BloodBath::GetBloodBath` (0x015638b8): the longest chain of
/// remembered kills starting at `from` (from killed a, a killed b, ...),
/// at most [`BLOODBATH_KILLS`] deep.
fn chain(memory: &[Memory], from: usize, depth: u32) -> u32 {
    if depth > BLOODBATH_KILLS {
        return 0;
    }
    memory.iter().filter(|m| m.killer == from).map(|m| 1 + chain(memory, m.victim, depth + 1)).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (situation -> bonuses, points) against the 0x0103832c defaults.
    #[test]
    fn table() {
        let s = |focus, incognito, silent, chasing, first_blood, revenge| Situation { focus, incognito, silent, chasing, first_blood, revenge };
        use Bonus::*;
        let rows: &[(Situation, &[Bonus], u32)] = &[
            (s(false, false, false, false, false, false), &[Unspotted], 50),
            (s(false, false, false, true, false, false), &[], 0),
            (s(false, false, true, true, false, false), &[Silent], 200),
            (s(false, false, true, false, false, false), &[Silent], 200),
            (s(false, true, true, false, false, false), &[Incognito], 300),
            (s(false, true, false, true, false, false), &[Incognito], 300),
            (s(true, false, false, true, false, false), &[Proximity], 150),
            (s(true, false, true, false, true, false), &[Proximity, Silent, FirstBlood], 400),
            (s(false, false, false, true, false, true), &[Revenge], 50),
            (s(true, true, true, false, true, true), &[Proximity, Incognito, FirstBlood, Revenge], 550),
        ];
        for (sit, want, pts) in rows {
            let got = bonuses(sit);
            assert_eq!(&got[..], *want, "{sit:?}");
            assert_eq!(got.iter().map(|b| b.points()).sum::<u32>(), *pts, "{sit:?}");
        }
        assert_eq!((SILENT, UNSPOTTED, INCOGNITO, FIRST_BLOOD, REVENGE, PROXIMITY, OVERKILL), (200, 50, 300, 50, 50, 150, 200));
        for b in 0..6 {
            assert_eq!(Bonus::from_u8(b).map(|b| b as u8), Some(b));
        }
    }

    #[test]
    fn focus_and_silent_ticks() {
        use glam::Vec3;
        let mut t = Tracker::new(3);
        let pos = [Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0), Vec3::new(50.0, 0.0, 0.0)];
        for _ in 0..FOCUS_TICKS - 1 {
            t.tick(&pos, &[false, true, false], &[true; 3]);
        }
        assert!(!t.situation(0, 1).focus && !t.situation(0, 1).silent);
        t.tick(&pos, &[false, true, false], &[true; 3]);
        let s = t.situation(0, 1);
        assert!(s.focus && s.silent && s.first_blood && !s.revenge);
        assert!(!t.situation(1, 0).silent, "1 runs");
        assert!(!t.situation(2, 0).focus, "too far");
        // running once resets Silent
        t.tick(&pos, &[true, true, false], &[true; 3]);
        assert!(!t.situation(0, 1).silent);
    }

    #[test]
    fn revenge_and_first_blood() {
        let mut t = Tracker::new(3);
        assert_eq!(t.on_kill(1, 0, true), None);
        assert!(!t.situation(0, 1).first_blood);
        assert!(t.situation(0, 1).revenge);
        // someone else kills 1 first: no revenge for 0
        t.on_kill(2, 1, true);
        assert!(!t.situation(0, 1).revenge);
        assert!(t.situation(1, 2).revenge);
    }

    #[test]
    fn bloodbath_chain() {
        let mut t = Tracker::new(4);
        // one player's three kills are no chain
        assert_eq!(t.on_kill(0, 1, true), None);
        assert_eq!(t.on_kill(0, 2, true), None);
        assert_eq!(t.on_kill(0, 3, true), None);
        let mut t = Tracker::new(4);
        assert_eq!(t.on_kill(2, 3, true), None);
        assert_eq!(t.on_kill(1, 2, true), None);
        assert_eq!(t.on_kill(0, 1, true), Some(3));
        assert!(t.memory.is_empty());
        // out of the 10 s window
        let mut t = Tracker::new(4);
        t.on_kill(2, 3, true);
        let pos = [glam::Vec3::ZERO; 4];
        for _ in 0..BLOODBATH_TICKS {
            t.tick(&pos, &[false; 4], &[true; 4]);
        }
        t.on_kill(1, 2, true);
        assert_eq!(t.on_kill(0, 1, true), None);
    }
}
