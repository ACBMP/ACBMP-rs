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

/// `HumanInAir` Main -> NarrowObject (`TestMainToNarrowObjectMain`
/// 012cc888, flag +0x3a0 bit1): set by `CheckForDefaultGrasp` (0132017a)
/// when `GetBeamAnchor` (0130fb08) finds a beam (narrow-object entry type
/// 3; pilotis entry 4 and targeted beam landings 8 are not ported). The
/// beam is searched in a box of these half extents (side, sight, up),
/// metres (0130fb08: 0.55, 0.4, 0.35) ...
pub const BEAM_BOX_HALF: Vec3 = Vec3::new(0.55, 0.4, 0.35);
/// ... centred this far above the probe point, metres (0130fb08: +0.15;
/// retail's point is the entity matrix applied to a CharacterComponent
/// offset (+0x40) we have not identified: ours is the feet).
pub const BEAM_BOX_UP: f32 = 0.15;

/// Whether a beam top at `z` is caught by a fall that started at
/// `start_z` (ours: a beam at the start height, the one just stepped off,
/// is not, see [`LANDING_DELAY_DROP`]).
pub fn beam_catch_allowed(start_z: f32, z: f32) -> bool {
    start_z - z > LANDING_DELAY_DROP
}

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
    /// The drop after a (re)spawn (retail InAirData +0x3b0 ==
    /// [`crate::human::haystack::RESPAWN_DROP`]): no haystack entry.
    pub respawn: bool,
}

impl Air {
    pub fn start(z: f32, delay: Option<f32>) -> Air {
        Air { start_z: z, time: 0.0, delay, wall_hits: 0, respawn: false }
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

/// Retail InAir -> StepOff (`Human::EvaluateTransitionToInAirStepOff`
/// 00cef704, flag +0x3a0 bit 10, `TestMainToStepOffMain` 012cc81c): a fall
/// touching a surface whose collision material forbids standing (material
/// type 2) pushes off it instead of landing, when there is room below.
/// Contacts with a triangle normal below this z are ceilings, ignored.
pub const STEP_OFF_CEILING: f32 = -std::f32::consts::FRAC_1_SQRT_2;
/// A default-material contact with a triangle normal z above this is
/// ground: no step off (the landing takes it), 00cef704.
pub const STEP_OFF_STAND: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// A no-stand contact with the triangle or contact normal z above this
/// steps off straight away; otherwise the mean contact normal z must exceed
/// it, 00cef704.
pub const STEP_OFF_UP: f32 = 0.5;
/// The clearance ray straight down must be free for at least this, metres
/// (00cef704: `CheckClearanceRayCast` 0.8, >= 0.8).
pub const STEP_OFF_CLEARANCE: f32 = 0.8;
/// The push off: this far out along the horizontal mean contact normal and
/// this far up, metres, over [`STEP_OFF_TIME`] s
/// (`Human::ExecuteTransitionToInAirStepOff` 00d129e8: 0.4, 0.4, 0.2).
pub const STEP_OFF_PUSH: f32 = 0.4;
pub const STEP_OFF_RISE: f32 = 0.4;
pub const STEP_OFF_TIME: f32 = 0.2;

/// One collision contact of a falling character, for [`step_off`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepContact {
    /// Normal of the touched mesh triangle (unit).
    pub tri: Vec3,
    /// Contact normal (unit, out of the surface).
    pub normal: Vec3,
    /// Collision material type 2 (no standing).
    pub no_stand: bool,
    /// The contact is another character (not a mesh): never steps off.
    pub character: bool,
}

/// Does this tick's contact set start a StepOff? `clear_below` is the free
/// distance straight down (asked only when needed). The no-stand flag is
/// `CollisionMaterial::AIType == AIType_StepOff` (2; retail material +0x10);
/// on San Donato only `Restricted_BounceOff_WoodSolid_NoCam` has it.
pub fn step_off(contacts: &[StepContact], clear_below: impl FnOnce() -> f32) -> bool {
    let (mut sum, mut no_stand, mut direct) = (Vec3::ZERO, false, false);
    for c in contacts {
        if c.character {
            return false;
        }
        if c.tri.z < STEP_OFF_CEILING {
            continue;
        }
        sum += c.normal;
        if !c.no_stand {
            if c.tri.z > STEP_OFF_STAND {
                return false;
            }
        } else {
            no_stand = true;
            direct |= c.tri.z > STEP_OFF_UP || c.normal.z > STEP_OFF_UP;
        }
    }
    let up = sum.length_squared() < 1e-12 || sum.normalize().z > STEP_OFF_UP;
    (direct || (no_stand && up)) && clear_below() >= STEP_OFF_CLEARANCE
}

/// The step off push direction: the horizontal mean contact normal (unit;
/// zero when the contacts are flat), 00d129e8.
pub fn step_off_dir(contacts: &[StepContact]) -> Vec2 {
    contacts.iter().map(|c| c.normal.truncate()).sum::<Vec2>().normalize_or_zero()
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

    #[test]
    fn step_off_needs_no_stand_material_and_room() {
        let c = |tri: Vec3, no_stand| StepContact { tri, normal: tri, no_stand, character: false };
        let steep = Vec3::new(0.8, 0.0, 0.6);
        let flat = Vec3::Z;
        // default material: never (flat lands, steep slides)
        assert!(!step_off(&[c(flat, false)], || 5.0));
        assert!(!step_off(&[c(steep, false)], || 5.0));
        // no-stand top with room below steps off; without room it lands
        assert!(step_off(&[c(steep, true)], || 5.0));
        assert!(!step_off(&[c(steep, true)], || 0.5));
        // a standable default contact alongside vetoes it
        assert!(!step_off(&[c(steep, true), c(flat, false)], || 5.0));
        // a side-on no-stand wall: mean normal too flat
        assert!(!step_off(&[c(Vec3::X, true)], || 5.0));
        assert!(step_off_dir(&[c(steep, true)]).abs_diff_eq(Vec2::X, 1e-6));
    }
}
