//! `HumanGround` Move -> HayStack (EnterHayStack): entering a haystack on
//! contact. Retail walks the character controller's contacts for one with
//! a haystack entity and enters when the player both faces and pushes into
//! it; there is no profile (walk / run) requirement and no reach beyond the
//! contact itself.
//!
//! `HumanInAir` Main -> HayStack (`TestMainToHayStackMain` 012ccb82, flag
//! +0x3a2 bit0 set by `CheckForHayStacks` 01319100): in the air retail
//! enters on any controller contact with a haystack entity (no facing or
//! push test, `IsContactAHayStack` 012d56aa: subtype 8), else when a
//! segment from the feet [`AIR_SEGMENT`] up crosses a haystack's
//! detection box (`DetectHayStack` 01318f9a, over far objects within
//! [`AIR_RADIUS`]). There is no fall speed or height-above-top rule.
//!
//! Before any of that the check is skipped while the haystack ignore timer
//! runs ([`ignore_blocks`]) and during a respawn drop ([`RESPAWN_DROP`]).
//! During a leap of faith (`HumanInAir::IsInLeapOfFaith` 012e64a0, the
//! vtable +0x88 test) retail instead casts a ray from the previous position
//! through the current one ([`leap_ray`]) and enters the jump's deep
//! haystack target when the ray hits and the feet are near its height
//! ([`near_deep_target`]); the contact and box tests fall back to that
//! target too.

use glam::{Vec2, Vec3};

/// The facing must be within 100 deg of the way into the haystack (the
/// negated contact normal): `-n . facing > cos 100`
/// (`HumanGround::TestMoveToHayStackOnHumanGroundInterfaceCommand_EnterHayStack`
/// 00fe030c: -0.17364818).
pub const FACING_COS: f32 = -0.173_648_18;

/// The move direction must be within 50 deg of the way in:
/// `-n . dir > cos 50` (00fe030c: 0.64278764).
pub const PUSH_COS: f32 = 0.642_787_64;

/// Our haystacks are detection boxes, not controller shapes: a contact is
/// the body's radius plus this much from the box, metres (ours: one
/// sprint tick at 60 Hz is ~0.1 m, so a contact is not stepped over).
pub const CONTACT_SKIN: f32 = 0.12;

/// Whether a contact with horizontal normal `n` (unit, out of the haystack
/// towards the player) enters it, facing `facing` (unit) and moving along
/// `dir` (unit, zero = no input).
pub fn enters(n: Vec2, facing: Vec2, dir: Vec2) -> bool {
    let into = -n;
    dir != Vec2::ZERO && into.dot(facing) > FACING_COS && into.dot(dir) > PUSH_COS
}

/// `DetectHayStack` (01318f9a): the probe segment runs from the feet this
/// far straight up, metres (1.8).
pub const AIR_SEGMENT: f32 = 1.8;

/// `CheckForHayStacks` (01319100): haystacks are searched among far
/// objects within this radius, metres (20.0; ours checks every spot).
pub const AIR_RADIUS: f32 = 20.0;

/// `CheckForHayStacks` (01319100): a contact normal with `z <= 0.9` is a
/// side entry (+0x424), above that a top entry (ours: one air clip).
pub const SIDE_NORMAL_Z: f32 = 0.9;

/// The air test for feet at local height `z` in a box spanning
/// `min_z..max_z` (box space), the feet's horizontal distance from the
/// box being `outside` (0 inside): a contact within `reach` (ours, see
/// [`CONTACT_SKIN`]) or the 1.8 m segment crossing the box.
pub fn air_enters(outside: f32, z: f32, min_z: f32, max_z: f32, reach: f32) -> bool {
    outside <= reach && z <= max_z && z + AIR_SEGMENT >= min_z
}

/// `HumanInAirData::GetHayStackIgnoreTimer` (00ec45f0, InAirData +0x228):
/// the air haystack check is skipped while the clock is before the timer's
/// end. Retail starts the timer lazily at the clock's current time (so it
/// never blocks then) and nothing else in the Mac build writes its end
/// (+0x230): kept for fidelity, `None` = never armed.
pub fn ignore_blocks(now: f32, until: Option<f32>) -> bool {
    until.is_some_and(|u| now < u)
}

/// InAirData +0x3b0 value for the drop after a respawn
/// (`BhvAssassin::PostRespawn` 010dad1b, also `Human::PlayerLeaving`
/// 00c9fc9b); `CheckForHayStacks` returns early for it (01319100).
pub const RESPAWN_DROP: u32 = 9;

/// May the air state look for haystacks at all: not while the ignore
/// timer runs, not during a respawn drop (01319100 prologue).
pub fn air_check_allowed(now: f32, ignore_until: Option<f32>, respawn_drop: bool) -> bool {
    !ignore_blocks(now, ignore_until) && !respawn_drop
}

/// `HumanInAir::IsNearDeepHayStackTarget` (0130aa50): the feet must be
/// within this height of the deep haystack target (HumanInAir +0x420),
/// metres (`|z - target z| < 2.0`).
pub const DEEP_TARGET_DZ: f32 = 2.0;

pub fn near_deep_target(z: f32, target_z: f32) -> bool {
    (z - target_z).abs() < DEEP_TARGET_DZ
}

/// `CheckForHayStacks` (01319100) leap-of-faith ray: from the previous
/// position along the step, this many times the step's length (3.0).
pub const LEAP_RAY_SCALE: f32 = 3.0;
/// No ray (and no entry) for a step this short or shorter, metres (0.0005).
pub const LEAP_RAY_MIN: f32 = 0.0005;

/// The leap-of-faith ray `(from, to)` for a step `prev -> cur`, if any.
pub fn leap_ray(prev: Vec3, cur: Vec3) -> Option<(Vec3, Vec3)> {
    let d = cur - prev;
    let len = d.length() * LEAP_RAY_SCALE;
    (len > LEAP_RAY_MIN).then(|| (prev, prev + d.normalize() * len))
}

/// The leap-of-faith entry: the ray hit something (the clearance ray cast
/// fails) and the feet are near the deep target's height.
pub fn leap_enters(ray_hit: bool, z: f32, target_z: f32) -> bool {
    ray_hit && near_deep_target(z, target_z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_timer_and_respawn_veto() {
        assert!(!ignore_blocks(5.0, None));
        assert!(ignore_blocks(5.0, Some(5.5)));
        assert!(!ignore_blocks(5.5, Some(5.5)));
        assert!(air_check_allowed(1.0, None, false));
        assert!(!air_check_allowed(1.0, Some(2.0), false));
        assert!(!air_check_allowed(1.0, None, true));
    }

    #[test]
    fn deep_target_within_2_m() {
        assert!(near_deep_target(11.9, 10.0));
        assert!(near_deep_target(8.1, 10.0));
        assert!(!near_deep_target(12.0, 10.0));
        assert!(!near_deep_target(7.9, 10.0));
    }

    #[test]
    fn leap_ray_three_steps_long() {
        let (a, b) = leap_ray(Vec3::ZERO, Vec3::new(0.0, 0.0, -0.5)).unwrap();
        assert_eq!(a, Vec3::ZERO);
        assert!((b.z + 1.5).abs() < 1e-6);
        assert!(leap_ray(Vec3::ZERO, Vec3::new(0.0001, 0.0, 0.0)).is_none());
        assert!(leap_enters(true, 3.0, 2.0));
        assert!(!leap_enters(false, 3.0, 2.0));
        assert!(!leap_enters(true, 5.0, 2.0));
    }

    fn at(deg: f32) -> Vec2 {
        Vec2::from_angle(deg.to_radians())
    }

    #[test]
    fn push_within_50_deg() {
        // haystack to the +X side: contact normal points back at us (-X)
        let n = -Vec2::X;
        assert!(enters(n, Vec2::X, at(0.0)));
        assert!(enters(n, Vec2::X, at(49.0)));
        assert!(!enters(n, Vec2::X, at(51.0)));
        assert!(!enters(n, Vec2::X, Vec2::ZERO));
    }

    #[test]
    fn facing_within_100_deg() {
        let n = -Vec2::X;
        assert!(enters(n, at(99.0), Vec2::X));
        assert!(!enters(n, at(101.0), Vec2::X));
        assert!(!enters(n, -Vec2::X, Vec2::X));
    }

    #[test]
    fn air_segment_crosses_box() {
        // box 0..1.6 high: feet just under the top enter, above do not
        assert!(air_enters(0.0, 1.59, 0.0, 1.6, 0.0));
        assert!(!air_enters(0.0, 1.61, 0.0, 1.6, 0.0));
        // head (1.8 m up) still reaching the box bottom
        assert!(air_enters(0.0, -1.79, 0.0, 1.6, 0.0));
        assert!(!air_enters(0.0, -1.81, 0.0, 1.6, 0.0));
        // beside the box: only within reach
        assert!(air_enters(0.3, 1.0, 0.0, 1.6, 0.47));
        assert!(!air_enters(0.5, 1.0, 0.0, 1.6, 0.47));
    }
}
