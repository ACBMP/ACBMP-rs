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

use glam::Vec2;

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

#[cfg(test)]
mod tests {
    use super::*;

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
