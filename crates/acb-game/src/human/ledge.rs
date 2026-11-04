//! `HumanLedge` / `HumanClimb` wall jumps: what the jump button does on a
//! wall (the brain `AssassinTaskLedge::UpdateLedge` 012a8298) and where the
//! rebound (back eject) goes.
//!
//! Retail's rebound is a targeted jump: the brain picks the best jump target
//! in the jump direction (`Human::GetJumpTargetListInDirection` 00d078f8 with
//! the hang's jump source, Human +0x960 = 0, i.e. the ground jump polygon,
//! then `GetBestEdgeJumpTargetIndex`), or with none the farthest foothold
//! ([`farthest`]), and `IHumanLedge::DoRebound` (slot +0x1c) plays the
//! rebound preparation; when it ends `HumanLedge::ProcessReboundTransition`
//! 00d8d890 calls `Human::DoTargetedJumpOrFarthestJump` 00d06d30 (source 2)
//! away from the wall. A ground-style targeted jump: no ballistic flag
//! (InAirData +0x1fa), unlike `ReleaseToInAir` 00d86ac2 / `ReleaseToTarget`
//! 00d8ca6e (drops onto a target below).

use glam::{Vec2, Vec3};

/// Stick magnitudes up to this count as no stick: the jump goes straight
/// away from the wall (`UpdateLedge`, 0.35).
pub const STICK_DEAD: f32 = 0.35;
/// No stick, or the stick more than this from "away from the wall" (into
/// the wall), tries the climb leap up first (`UpdateLedge`, 2.0943952 rad).
pub const LEAP_FIRST: f32 = 2.094_395_2;
/// The stick at most this far from "away" rebounds; further (into the
/// wall) does not (`UpdateLedge`, 2.3561945 rad).
pub const REBOUND_MAX: f32 = 2.356_194_5;
/// Within this of "away" the rebound follows the stick (`UpdateLedge`,
/// 1.0471976 rad); beyond it the direction is "away" turned [`REBOUND_SIDE`]
/// toward the stick's side.
pub const REBOUND_CONE: f32 = 1.047_197_6;
/// See [`REBOUND_CONE`] (`UpdateLedge`, `RotateVector` by 0x3fc6d3f2 =
/// 1.5533 rad, 89 deg).
pub const REBOUND_SIDE: f32 = 1.553_343;

/// Forward reach of the farthest foothold, metres: the jump detection
/// curve's far point (slot 1, point n-2: 8.0 m times
/// ForwardJumpDetectionScale), `Human::GetFarthestFootHoldInDirection`
/// 00d14144 for detection type 0 and jump source 0.
pub const FARTHEST_REACH: f32 = 8.0 * crate::jump::FORWARD_SCALE;
/// How far below the start the farthest foothold is, metres (00d14144).
pub const FARTHEST_DROP: f32 = 3.0;

/// What the jump button does on a wall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallJump {
    /// Try the climb leap to a hold above before anything else.
    pub leap_first: bool,
    /// Rebound along this flat unit direction (if the leap is not taken).
    pub rebound: Option<Vec2>,
}

/// The jump button on a wall with the stick `stick` (world, flat, length =
/// deflection) and `away` the flat unit normal out of the wall.
pub fn wall_jump(stick: Vec2, away: Vec2) -> WallJump {
    let dir = if stick.length() <= STICK_DEAD { away } else { stick.normalize() };
    // signed angle from away to the stick (counter-clockwise positive)
    let a = away.perp_dot(dir).atan2(away.dot(dir));
    let leap_first = stick.length() <= STICK_DEAD || a.abs() > LEAP_FIRST;
    let rebound = if a.abs() > REBOUND_MAX {
        None
    } else if a.abs() < REBOUND_CONE {
        Some(dir)
    } else {
        Some(Vec2::from_angle(REBOUND_SIDE.copysign(a)).rotate(away))
    };
    WallJump { leap_first, rebound }
}

/// The farthest foothold from `pos` along the flat unit `dir`: where a
/// rebound with no jump target in reach aims
/// (`Human::GetFarthestFootHoldInDirection` 00d14144).
pub fn farthest(pos: Vec3, dir: Vec2) -> Vec3 {
    pos + (dir * FARTHEST_REACH).extend(-FARTHEST_DROP)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AWAY: Vec2 = Vec2::new(0.0, -1.0);

    #[test]
    fn no_stick_leaps_first_then_rebounds_straight_away() {
        let j = wall_jump(Vec2::new(0.2, 0.0), AWAY);
        assert!(j.leap_first);
        assert_eq!(j.rebound, Some(AWAY));
    }

    #[test]
    fn stick_into_the_wall_only_leaps() {
        let j = wall_jump(-AWAY, AWAY);
        assert!(j.leap_first);
        assert_eq!(j.rebound, None);
    }

    #[test]
    fn stick_within_60_deg_is_followed() {
        let s = Vec2::from_angle(0.5).rotate(AWAY);
        let j = wall_jump(s, AWAY);
        assert!(!j.leap_first);
        assert!((j.rebound.unwrap() - s).length() < 1e-5);
    }

    #[test]
    fn stick_along_the_wall_turns_89_deg_from_away_on_its_side() {
        for side in [1.0f32, -1.0] {
            let s = Vec2::from_angle(side * std::f32::consts::FRAC_PI_2).rotate(AWAY);
            let d = wall_jump(s, AWAY).rebound.unwrap();
            assert!((d.dot(AWAY) - REBOUND_SIDE.cos()).abs() < 1e-5);
            assert!(d.dot(s) > 0.99);
        }
    }

    #[test]
    fn farthest_is_8_m_out_3_m_down() {
        let t = farthest(Vec3::new(1.0, 2.0, 10.0), AWAY);
        assert!((t - Vec3::new(1.0, -6.0, 7.0)).length() < 1e-5);
    }
}
