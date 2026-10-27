//! `HumanGround` Move -> HayStack (EnterHayStack): entering a haystack on
//! contact. Retail walks the character controller's contacts for one with
//! a haystack entity and enters when the player both faces and pushes into
//! it; there is no profile (walk / run) requirement and no reach beyond the
//! contact itself.

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
}
