//! The brain's jump decisions (`AssassinTaskGround::UpdateGroundMovement`
//! 0166fb1c) and the `HumanGround` jump impulsion gate: which button starts
//! a jump, how long a press counts, which stick directions jump, and when a
//! standing (straight) jump may start.
//!
//! The button: every jump test in 0166fb1c calls
//! `ControlScheme::ButtonPressed(PadButton)` and
//! `ButtonJustPressedWithDelay(PadButton, 0.5)` with PadButton 0 (pushed at
//! 167105c / 1671084, 1671591 / 16715b9, 1671764 / 1671784). PadButton 0 is
//! `Button1` (the pad name table: Button1..4, PadUp, ..., ShoulderRight2),
//! which `multi/DefaultBindings.map` binds to the spacebar: the Legs button
//! (kills use PadButton 1 = Button2 = left mouse, the weapon hand, in
//! `AssassinTask::ButtonWantsToKill` b16a76). Ours: [`crate::PlayerInput::sprint`]
//! (viewer Space), which is high profile + Legs as in retail.

use glam::Vec2;

/// A Legs press keeps counting for a jump this long after it went down,
/// seconds (`ButtonJustPressedWithDelay(Button1, 0.5)`, 0166fb1c 1671065).
pub const LEGS_DELAY: f32 = 0.5;

/// The stick must point within 80 deg of the facing for a freerun jump:
/// dot(stick, facing) > cos 80 deg (0166fb1c 1671029, 0.17364818).
pub const JUMP_CONE_COS: f32 = 0.173_648_18;

/// The Legs button over time, counted in fixed ticks (deterministic).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Legs {
    /// Held this tick.
    pub held: bool,
    /// Went down this tick.
    pub pressed: bool,
    /// Seconds since it last went down (`None`: not since spawn).
    pub since: Option<f32>,
}

impl Legs {
    /// One tick of the button (`down`: held this tick).
    pub fn tick(&mut self, down: bool, dt: f32) {
        self.pressed = down && !self.held;
        self.held = down;
        self.since = if self.pressed { Some(0.0) } else { self.since.map(|s| s + dt) };
    }

    /// The retail jump flag's button test: held, or pressed within
    /// [`LEGS_DELAY`] (`ButtonPressed || ButtonJustPressedWithDelay`).
    pub fn wants_jump(&self) -> bool {
        self.held || self.since.is_some_and(|s| s <= LEGS_DELAY + 1e-4)
    }
}

/// Stick `dir` within the jump cone around `facing` (both horizontal; a
/// zero stick never jumps).
pub fn in_cone(dir: Vec2, facing: Vec2) -> bool {
    let (d, f) = (dir.normalize_or_zero(), facing.normalize_or_zero());
    d != Vec2::ZERO && d.dot(f) > JUMP_CONE_COS
}

/// The freerun jump gate: Legs held or just pressed, stick in the cone.
pub fn freerun(legs: &Legs, dir: Vec2, facing: Vec2) -> bool {
    legs.wants_jump() && in_cone(dir, facing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn button_window() {
        let mut l = Legs::default();
        assert!(!l.wants_jump());
        l.tick(true, DT);
        assert!(l.pressed && l.wants_jump());
        l.tick(true, DT);
        assert!(!l.pressed && l.wants_jump(), "held");
        l.tick(false, DT);
        // released: counts for 0.5 s after the press (2 ticks so far)
        for _ in 0..28 {
            assert!(l.wants_jump());
            l.tick(false, DT);
        }
        assert!(l.wants_jump(), "exactly 0.5 s after the press");
        l.tick(false, DT);
        assert!(!l.wants_jump(), "past 0.5 s");
        // a new press restarts the window
        l.tick(true, DT);
        assert!(l.pressed && l.wants_jump());
    }

    #[test]
    fn cone_80_deg() {
        let f = Vec2::Y;
        let at = |deg: f32| Vec2::new(deg.to_radians().sin(), deg.to_radians().cos());
        assert!(in_cone(at(0.0), f));
        assert!(in_cone(at(79.0), f));
        assert!(in_cone(at(-79.0), f));
        assert!(!in_cone(at(81.0), f));
        assert!(!in_cone(at(180.0), f));
        assert!(!in_cone(Vec2::ZERO, f));
        let mut l = Legs::default();
        assert!(!freerun(&l, at(0.0), f), "no button");
        l.tick(true, DT);
        assert!(freerun(&l, at(45.0), f));
        assert!(!freerun(&l, at(90.0), f));
    }
}
