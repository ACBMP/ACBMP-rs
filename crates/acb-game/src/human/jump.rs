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

/// Retail Move -> JumpImpulsionForwardFromIdle (JumpToTarget command,
/// `TestMoveToJumpImpulsionForwardFromIdle...JumpToTarget` 00f6720c): a
/// freerun jump to a target may start from standing still, or early in a
/// running action (a start clip) whose ratio is below this; later it waits.
pub const FORWARD_FROM_IDLE_RATIO: f32 = 0.75;
/// Below this speed the character counts as standing (retail tests its
/// locomotion state, +0x8ec == 0; the threshold is ours), m/s.
pub const IDLE_SPEED: f32 = 0.1;
/// ForwardFromIdle blends its flat and its up clip by the target's height
/// above the feet, up to this, metres (00fbbc7a: clamp(dz, 0, 1.3) / 1.3).
pub const FORWARD_FROM_IDLE_UP: f32 = 1.3;

/// May a freerun jump to a target start via ForwardFromIdle? `standing`:
/// no locomotion; `action`: the running action's ratio, if any. Returns
/// the ratio the impulsion clip starts at (1 from standing,
/// `ratio / 0.75` early in an action), `None` = not now (00f6720c).
pub fn forward_from_idle(standing: bool, action: Option<f32>) -> Option<f32> {
    if standing {
        return Some(1.0);
    }
    action.filter(|r| *r < FORWARD_FROM_IDLE_RATIO).map(|r| (r / FORWARD_FROM_IDLE_RATIO).max(0.0))
}

/// The ForwardFromIdle up blend for a target `dz` metres above the feet
/// (0 = flat clip, 1 = up clip), 00fbbc7a.
pub fn forward_from_idle_up(dz: f32) -> f32 {
    dz.clamp(0.0, FORWARD_FROM_IDLE_UP) / FORWARD_FROM_IDLE_UP
}

/// The JumpImpulsion anticipation: pending commands (JumpToTarget,
/// SideStepToTarget, EnterWallingVertical, StraightJump) only leave the
/// state once it has run this long times the impulsion ratio (1 on entry),
/// seconds (`_EnterJumpImpulsion` 00f80e88: ratio * 0.2 s; the tests
/// 00f3159c / 00f31634 / 00fddfdc / 00fde04a / 00f316cc defer until then).
pub const IMPULSION_ANTICIPATION: f32 = 0.2;

/// Where a JumpImpulsion goes once [`IMPULSION_ANTICIPATION`] is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImpulsionExit {
    /// JumpToTarget: JumpImpulsionForwardFromIdle, then InAir when its clip
    /// ends (00f3159c, 00f3c4e0).
    ForwardFromIdle,
    /// SideStepToTarget: straight to InAir (00f31634).
    SideStep,
    /// EnterWallingVertical from standing: WallingImpulsion (00fddfdc).
    WallingImpulsion,
    /// EnterWallingVertical while moving: Walling (00fde04a).
    Walling,
}

/// A pending JumpImpulsion command's exit after `t` s in the state, or
/// `None` while the anticipation runs (the command stays pending).
pub fn impulsion_exit(t: f32, ratio: f32, exit: ImpulsionExit, moving: bool) -> Option<ImpulsionExit> {
    if t + 1e-4 < ratio * IMPULSION_ANTICIPATION {
        return None;
    }
    Some(match exit {
        ImpulsionExit::WallingImpulsion | ImpulsionExit::Walling if moving => ImpulsionExit::Walling,
        ImpulsionExit::WallingImpulsion | ImpulsionExit::Walling => ImpulsionExit::WallingImpulsion,
        e => e,
    })
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

    #[test]
    fn forward_from_idle_and_impulsion_exits() {
        assert_eq!(forward_from_idle(true, None), Some(1.0));
        assert_eq!(forward_from_idle(false, None), None);
        assert_eq!(forward_from_idle(false, Some(0.375)), Some(0.5));
        assert_eq!(forward_from_idle(false, Some(0.8)), None);
        assert_eq!(forward_from_idle_up(2.0), 1.0);
        assert!((forward_from_idle_up(0.65) - 0.5).abs() < 1e-6);
        assert_eq!(impulsion_exit(0.1, 1.0, ImpulsionExit::SideStep, false), None);
        assert_eq!(impulsion_exit(0.2, 1.0, ImpulsionExit::SideStep, false), Some(ImpulsionExit::SideStep));
        assert_eq!(impulsion_exit(0.25, 1.0, ImpulsionExit::Walling, false), Some(ImpulsionExit::WallingImpulsion));
        assert_eq!(impulsion_exit(0.25, 1.0, ImpulsionExit::WallingImpulsion, true), Some(ImpulsionExit::Walling));
        assert_eq!(impulsion_exit(0.05, 0.25, ImpulsionExit::ForwardFromIdle, false), Some(ImpulsionExit::ForwardFromIdle));
    }
}
