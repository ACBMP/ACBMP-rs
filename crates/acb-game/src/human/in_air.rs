//! `HumanInAir`: what a falling character checks each tick, in retail order.

use glam::{Vec2, Vec3};

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
    /// Wall-reception checks passed so far (retail InAirData +0x3a4).
    pub wall_hits: u8,
}

impl Air {
    pub fn start(z: f32, delay: Option<f32>) -> Air {
        Air { start_z: z, time: 0.0, delay, wall_hits: 0 }
    }

    pub fn tick(&mut self, dt: f32) {
        self.time += dt;
    }

    /// May the fall land at height `z` (0130fea6 delay test)?
    pub fn landing_allowed(&self, z: f32) -> bool {
        self.start_z - z > LANDING_DELAY_DROP || self.delay.is_some_and(|d| self.time >= d)
    }
}

/// Retail InAir -> TurnOnWall (WallReception): a fall that runs face first
/// into a wall turns its back to it and drops down it
/// (`HumanInAir::TestMainToTurnOnWallOnInterfaceCommand_WallReception`
/// 01315f90; TurnOnWall -> Main when the reception clip ends, 012d4a4a).
/// The velocity into the wall must exceed this, m/s (01315f90: velocity .
/// -n > 0.17364818).
pub const WALL_INTO: f32 = 0.173_648_18;
/// The wall must face the character: its normal within this of the
/// reversed facing, radians (01315f90: < 0.7853982).
pub const WALL_FACING: f32 = std::f32::consts::FRAC_PI_4;
/// Only after this long in the air, seconds (01315f90: ticks / 30000 > 0.2).
pub const WALL_MIN_AIR: f32 = 0.2;
/// Horizontal speed above this, m/s (01315f90: length > 2.0).
pub const WALL_SPEED: f32 = 2.0;
/// Heading within this of the facing, radians (01315f90: < 1.0471976).
pub const WALL_HEADING: f32 = std::f32::consts::FRAC_PI_3;
/// The check must pass this many times (01315f90: counter +0x3a4 >= 2).
pub const WALL_HITS: u8 = 2;

/// One wall-reception check: wall contact normal `n` (horizontal unit, out
/// of the wall), velocity `vel`, facing `facing` (unit).
pub fn wall_reception(n: Vec2, vel: Vec3, facing: Vec2, air_time: f32) -> bool {
    let h = vel.truncate();
    h.dot(-n) > WALL_INTO
        && (-facing).angle_to(n).abs() < WALL_FACING
        && air_time > WALL_MIN_AIR
        && h.length() > WALL_SPEED
        && facing.angle_to(h).abs() < WALL_HEADING
}

impl Air {
    /// Counts a passed [`wall_reception`] check; true once there were
    /// [`WALL_HITS`] of them: turn on the wall.
    pub fn wall(&mut self, hit: bool) -> bool {
        if hit {
            self.wall_hits = self.wall_hits.saturating_add(1);
        }
        hit && self.wall_hits >= WALL_HITS
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

    #[test]
    fn wall_reception_rules() {
        let n = -glam::Vec2::X; // wall ahead along +X
        let f = glam::Vec2::X;
        let v = Vec3::new(3.0, 0.0, -2.0);
        assert!(wall_reception(n, v, f, 0.3));
        // too early, too slow, moving along the wall, facing it sideways
        assert!(!wall_reception(n, v, f, 0.2));
        assert!(!wall_reception(n, Vec3::new(1.9, 0.0, 0.0), f, 0.3));
        assert!(!wall_reception(n, Vec3::new(0.1, 3.0, 0.0), glam::Vec2::Y, 0.3));
        assert!(!wall_reception(n, v, glam::Vec2::from_angle(0.8), 0.3));
        // heading 61 deg off the facing
        assert!(!wall_reception(-glam::Vec2::from_angle(0.5), Vec3::new(3.0, 0.0, 0.0), glam::Vec2::from_angle(1.07), 0.3));
        // two passed checks turn
        let mut a = Air::start(5.0, None);
        assert!(!a.wall(true));
        assert!(!a.wall(false));
        assert!(a.wall(true));
    }
}
