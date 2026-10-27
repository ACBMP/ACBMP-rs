//! `HumanInAir`: what a falling character checks each tick, in retail order.

use glam::Vec3;

/// The `HumanInAir::UpdateMainMain` (01328022) checks this port has, in the
/// retail tick order (WaterWell, TargetedLedgeGrab, DefaultGrasp, Boats,
/// HayStacks, StepOff, Horses, Landing): a hold within reach wins over a
/// haystack below, and both over touching down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AirCheck {
    Grasp,
    HayStack,
    Landing,
}

/// Tick order of [`AirCheck`] (`HumanInAir::UpdateMainMain` 01328022).
pub const ORDER: [AirCheck; 3] = [AirCheck::Grasp, AirCheck::HayStack, AirCheck::Landing];

/// A fall grabs holds only while it is shorter than this, metres
/// (`HumanInAir::CheckForDefaultGrasp` 0132017a: fall < 2.0 unless the
/// jump flags force it). Longer falls go to the ground (or a haystack).
pub const GRASP_MAX_FALL: f32 = 2.0;
/// Reach of the grasp probe ahead of the body, metres
/// (`Human::CastGuidanceDepthDetector` length 1.2 in `CheckForDefaultGrasp`).
pub const GRASP_PROBE: f32 = 1.2;
/// Height offset of that probe, metres (`CheckForDefaultGrasp` argument
/// 0.38; our hold window [`crate::climb`] 1.3-2.2 m above the feet already
/// sits at hand height, so this is documentation only).
pub const GRASP_HAND_HEIGHT: f32 = 0.38;

/// The default grasp gate: the fall so far (`fall`, metres from its highest
/// point) is under [`GRASP_MAX_FALL`] and no eject window (`block`, seconds)
/// is running. Replaces our old "fell 0.3 m and vz < -1 m/s" gate.
pub fn grasp_allowed(fall: f32, block: f32) -> bool {
    fall < GRASP_MAX_FALL && block <= 0.0
}

/// Landing contact class by the contact normal's angle from straight up
/// (`HumanInAir::TestGroundCollision` 0130fea6, contact angle bands 45, 70,
/// 135 degrees; the first band that has a contact wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contact {
    /// 0-45 degrees: lands (retail result 1).
    Ground,
    /// 45-70 degrees: slope (result 2, retail slides; ours lands and logs).
    Slope,
    /// 70-135 degrees: wall (result 3).
    Wall,
}

/// Contact band limits, degrees (0130fea6: 45.0, 70.0, 135.0).
pub const GROUND_MAX_DEG: f32 = 45.0;
pub const SLOPE_MAX_DEG: f32 = 70.0;
pub const WALL_MAX_DEG: f32 = 135.0;

/// Class of a contact with unit normal `n` (None: a ceiling, > 135 deg).
pub fn classify(n: Vec3) -> Option<Contact> {
    let deg = n.normalize_or_zero().z.clamp(-1.0, 1.0).acos().to_degrees();
    if deg <= GROUND_MAX_DEG {
        Some(Contact::Ground)
    } else if deg <= SLOPE_MAX_DEG {
        Some(Contact::Slope)
    } else if deg < WALL_MAX_DEG {
        Some(Contact::Wall)
    } else {
        None
    }
}

/// Within this many metres below the height the air state started at, a
/// landing needs the landing-test delay to have run out (0130fea6: start z
/// - z <= 0.3 -> `HumanInAirData::GetLandingTestDelayTimer`).
pub const LANDING_DELAY_DROP: f32 = 0.3;

/// Per-fall `HumanInAir` data: where the air state started and its
/// landing-test delay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Air {
    /// Height the air state started at (retail InAirData start z; not the
    /// peak, which `Player::fall_from` keeps for fall damage).
    pub start_z: f32,
    /// Seconds in the air.
    pub time: f32,
    /// Landing-test delay, seconds; `None` = no timer started, so no landing
    /// at all within [`LANDING_DELAY_DROP`] of the start. Retail starts it
    /// with the length of the action that began the air state
    /// (0133caae, length * 30000 ticks).
    pub delay: Option<f32>,
}

impl Air {
    pub fn start(z: f32, delay: Option<f32>) -> Air {
        Air { start_z: z, time: 0.0, delay }
    }

    pub fn tick(&mut self, dt: f32) {
        self.time += dt;
    }

    /// May the fall land at height `z` (0130fea6 delay test)?
    pub fn landing_allowed(&self, z: f32) -> bool {
        self.start_z - z > LANDING_DELAY_DROP || self.delay.is_some_and(|d| self.time >= d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grasp_before_haystack_before_landing() {
        let at = |c| ORDER.iter().position(|x| *x == c).unwrap();
        assert!(at(AirCheck::Grasp) < at(AirCheck::HayStack));
        assert!(at(AirCheck::HayStack) < at(AirCheck::Landing));
    }

    #[test]
    fn grasp_gate_is_two_metres() {
        assert!(grasp_allowed(0.0, 0.0));
        assert!(grasp_allowed(1.99, 0.0));
        assert!(!grasp_allowed(2.0, 0.0));
        assert!(!grasp_allowed(5.0, 0.0));
        assert!(!grasp_allowed(0.5, 0.1));
        assert_eq!(GRASP_PROBE, 1.2);
    }

    #[test]
    fn landing_classes() {
        let tilt = |deg: f32| Vec3::new(deg.to_radians().sin(), 0.0, deg.to_radians().cos());
        assert_eq!(classify(Vec3::Z), Some(Contact::Ground));
        assert_eq!(classify(tilt(44.0)), Some(Contact::Ground));
        assert_eq!(classify(tilt(46.0)), Some(Contact::Slope));
        assert_eq!(classify(tilt(69.0)), Some(Contact::Slope));
        assert_eq!(classify(tilt(71.0)), Some(Contact::Wall));
        assert_eq!(classify(tilt(134.0)), Some(Contact::Wall));
        assert_eq!(classify(tilt(136.0)), None);
    }

    #[test]
    fn landing_delay_near_start() {
        let mut a = Air::start(5.0, None);
        assert!(!a.landing_allowed(4.8));
        assert!(a.landing_allowed(4.6));
        let mut b = Air::start(5.0, Some(0.5));
        assert!(!b.landing_allowed(4.9));
        for _ in 0..30 {
            a.tick(1.0 / 60.0);
            b.tick(1.0 / 60.0);
        }
        assert!(!a.landing_allowed(4.9));
        assert!(b.landing_allowed(4.9));
    }
}
