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
/// timer).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Hide {
    #[default]
    Idle,
    /// Hidden, waiting [`START_INTERVAL`]; seconds left.
    Starting(f32),
    /// Paying every [`TIME_INTERVAL`]; seconds to the next bonus.
    Paying(f32),
    /// Left the hiding after a bonus; seconds of [`COOLDOWN_INTERVAL`] left.
    Cooldown(f32),
}

impl Hide {
    /// Advances by `dt` with the prey `hidden` (hiding spot or blended, not
    /// being killed, alive: `GBrick_BlendedScore::IsHidden` 012d2890) and
    /// `mates` other hidden prey within [`RADIUS`]: the points earned this
    /// step (`Update` 0130f28e, `AwardBonus` 012fd80a).
    pub fn step(&mut self, dt: f32, hidden: bool, mates: usize) -> u32 {
        let award = || BONUS[mates.min(BONUS.len() - 1)];
        match *self {
            Hide::Idle if hidden => *self = Hide::Starting(START_INTERVAL - dt),
            Hide::Idle => {}
            Hide::Starting(_) if !hidden => *self = Hide::Idle,
            Hide::Paying(_) if !hidden => *self = Hide::Cooldown(COOLDOWN_INTERVAL - dt),
            Hide::Cooldown(t) if !hidden => *self = if t - dt <= 0.0 { Hide::Idle } else { Hide::Cooldown(t - dt) },
            Hide::Starting(t) | Hide::Paying(t) | Hide::Cooldown(t) => {
                if t - dt <= 0.0 {
                    *self = Hide::Paying(TIME_INTERVAL);
                    return award();
                }
                *self = match *self {
                    Hide::Starting(_) => Hide::Starting(t - dt),
                    Hide::Paying(_) => Hide::Paying(t - dt),
                    _ => Hide::Cooldown(t - dt),
                };
            }
        }
        0
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

    const DT: f32 = 1.0 / 60.0;

    fn run(h: &mut Hide, secs: f32, hidden: bool, mates: usize) -> u32 {
        (0..(secs / DT).round() as usize).map(|_| h.step(DT, hidden, mates)).sum()
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
    }

    #[test]
    fn mates_within_twenty_metres() {
        assert_eq!(mates_near(Vec3::ZERO, &[Vec3::new(19.0, 0.0, 0.0), Vec3::new(21.0, 0.0, 0.0)]), 1);
    }
}
