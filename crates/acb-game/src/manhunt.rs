//! Manhunt (catsmice): the prey's hide bonus (`GBrick_BlendedScore`).
//!
//! The rest of the mode is Escort's machinery with
//! [`crate::rules::TeamGame::Manhunt`]: two teams, two rounds, the attacking
//! team hunts every player of the other team (retail
//! `GameInfo::SetupTeamLoop_CatsMice` 0117f9d8), the prey can stun its
//! hunters. Rules text (loc 0x4db3f): "2 rounds of 5 minutes ... cooperate to
//! hide (doing it near your teammates gives more points)". Values are the
//! `GBrick_BlendedScore::ReadFromXML` defaults (012d1822; the mode's XML did
//! not ship).

use glam::Vec3;

/// Seconds hidden before the first bonus (`StartInterval`, +0x4c).
pub const START_INTERVAL: f32 = 1.0;
/// Seconds between bonuses while hidden (`TimeInterval`, +0x44).
pub const TIME_INTERVAL: f32 = 5.0;
/// Seconds after leaving before hiding pays again (`CooldownInterval`,
/// +0x48; `StopBonus` 012ffad6).
pub const COOLDOWN_INTERVAL: f32 = 7.0;
/// Hidden teammates within this many metres raise the bonus (`Radius`,
/// +0x50; `UpdateHiddenTargets` 0130f0d2).
pub const RADIUS: f32 = 20.0;
/// Points per bonus by the number of hidden prey together, 1 to 4
/// (`BlendedBonus1P`..`4P`, +0x54..+0x60).
pub const BONUS: [u32; 4] = [10, 20, 30, 50];

/// One prey's hide bonus timer (`GBrick_BlendedScore` +0x6c state, +0x70
/// timer), counted in whole sim ticks so peers agree exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hide {
    #[default]
    Idle,
    /// Hidden, waiting [`START_INTERVAL`]; ticks left.
    Starting(u32),
    /// Paying every [`TIME_INTERVAL`]; ticks to the next bonus.
    Paying(u32),
    /// Left the hiding after a bonus; ticks of [`COOLDOWN_INTERVAL`] left.
    Cooldown(u32),
}

/// Seconds to whole sim ticks.
fn ticks(secs: f32) -> u32 {
    crate::mode::ticks(secs)
}

impl Hide {
    /// Advances one sim tick with the prey `hidden` (hiding spot or
    /// blended, not being killed, alive: `GBrick_BlendedScore::IsHidden`
    /// 012d2890) and `mates` other hidden prey within [`RADIUS`]: the points
    /// earned this tick (`Update` 0130f28e, `AwardBonus` 012fd80a).
    pub fn step(&mut self, hidden: bool, mates: usize) -> u32 {
        let award = BONUS[mates.min(BONUS.len() - 1)];
        let (next, points) = match *self {
            Hide::Idle if hidden => (Hide::Starting(ticks(START_INTERVAL) - 1), 0),
            Hide::Idle => (Hide::Idle, 0),
            Hide::Starting(_) if !hidden => (Hide::Idle, 0),
            Hide::Paying(_) if !hidden => (Hide::Cooldown(ticks(COOLDOWN_INTERVAL) - 1), 0),
            Hide::Cooldown(t) if !hidden => (if t <= 1 { Hide::Idle } else { Hide::Cooldown(t - 1) }, 0),
            Hide::Starting(t) | Hide::Paying(t) | Hide::Cooldown(t) if t <= 1 => (Hide::Paying(ticks(TIME_INTERVAL)), award),
            Hide::Starting(t) => (Hide::Starting(t - 1), 0),
            Hide::Paying(t) => (Hide::Paying(t - 1), 0),
            Hide::Cooldown(t) => (Hide::Cooldown(t - 1), 0),
        };
        *self = next;
        points
    }

    /// Wire form: (state 0..=3, ticks).
    pub fn to_parts(self) -> (u8, u32) {
        match self {
            Hide::Idle => (0, 0),
            Hide::Starting(t) => (1, t),
            Hide::Paying(t) => (2, t),
            Hide::Cooldown(t) => (3, t),
        }
    }

    /// Inverse of [`Hide::to_parts`].
    pub fn from_parts(state: u8, t: u32) -> Hide {
        match state {
            1 => Hide::Starting(t),
            2 => Hide::Paying(t),
            3 => Hide::Cooldown(t),
            _ => Hide::Idle,
        }
    }
}

/// How many of `others` (positions of the other hidden prey of the same
/// team) are within [`RADIUS`] of `at`.
pub fn mates_near(at: Vec3, others: &[Vec3]) -> usize {
    others.iter().filter(|o| o.distance_squared(at) <= RADIUS * RADIUS).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(h: &mut Hide, secs: f32, hidden: bool, mates: usize) -> u32 {
        (0..ticks(secs)).map(|_| h.step(hidden, mates)).sum()
    }

    #[test]
    fn first_bonus_after_one_second_then_every_five() {
        let mut h = Hide::Idle;
        assert_eq!(run(&mut h, 0.9, true, 0), 0);
        assert_eq!(run(&mut h, 0.2, true, 0), 10);
        assert_eq!(run(&mut h, 4.8, true, 0), 0);
        assert_eq!(run(&mut h, 0.3, true, 0), 10);
    }

    #[test]
    fn more_hidden_mates_pay_more() {
        let mut h = Hide::Idle;
        assert_eq!(run(&mut h, 1.1, true, 3), 50);
        let mut h = Hide::Idle;
        assert_eq!(run(&mut h, 1.1, true, 9), 50);
        let mut h = Hide::Idle;
        assert_eq!(run(&mut h, 1.1, true, 1), 20);
    }

    #[test]
    fn leaving_starts_the_cooldown() {
        let mut h = Hide::Idle;
        assert_eq!(run(&mut h, 1.1, true, 0), 10);
        assert_eq!(run(&mut h, 2.0, false, 0), 0);
        // back in hiding: pays once the 7 s cooldown has run
        assert_eq!(run(&mut h, 4.9, true, 0), 0);
        assert_eq!(run(&mut h, 0.2, true, 0), 10);
        // left before the first bonus: no cooldown
        let mut h = Hide::Idle;
        run(&mut h, 0.5, true, 0);
        run(&mut h, 0.1, false, 0);
        assert_eq!(h, Hide::Idle);
        for x in [Hide::Idle, Hide::Starting(3), Hide::Paying(7), Hide::Cooldown(9)] {
            let (s, t) = x.to_parts();
            assert_eq!(Hide::from_parts(s, t), x);
        }
    }

    #[test]
    fn mates_within_twenty_metres() {
        assert_eq!(mates_near(Vec3::ZERO, &[Vec3::new(19.0, 0.0, 0.0), Vec3::new(21.0, 0.0, 0.0)]), 1);
    }
}
