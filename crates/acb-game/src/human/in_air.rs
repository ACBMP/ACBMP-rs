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
/// 3; pilotis entry 4 is [`pilotis`], targeted beam landings 8 [`arrival`]). The
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

/// `Human::CheckForPilotis` (00d1109a), called by `CheckForDefaultGrasp`
/// (0132017a, before `GetBeamAnchor`): a post top (pilotis) grabbed in a
/// short fall, narrow-object entry type 4 (PilotisFromInAir). Retail casts
/// a guidance box of these half extents (side, sight), metres (0.6, 0.5)
/// ...
pub const PILOTIS_BOX_HALF: Vec2 = Vec2::new(0.6, 0.5);
/// ... spanning this height range around its origin, metres (the grasp
/// call passes `true`: -0.3..1.1; `false` would be -0.6..0.6; retail's
/// origin is a gap point at hand height, ours the feet).
pub const PILOTIS_BOX_Z: (f32, f32) = (-0.3, 1.1);
/// The nearest of the four top edges must be within this of the probe
/// point, horizontally, metres (00d1109a: min of four lengths <= 0.375).
pub const PILOTIS_EDGE_REACH: f32 = 0.375;
/// The top may be at most this wide along and across the sight, metres
/// (00d1109a: |e0 - e1| <= 0.7 and |e2 - e3| <= 0.7).
pub const PILOTIS_MAX_WIDTH: f32 = 0.7;
/// Clearance capsule over the post centre: from this far above the top
/// ... metres (00d1109a: +0.6) ...
pub const PILOTIS_CLEAR_FROM: f32 = 0.6;
/// ... to this far, metres (+1.4; radius 0.4, ours: one ray up the centre).
pub const PILOTIS_CLEAR_TO: f32 = 1.4;
/// Seconds the retail displacement onto the post takes (0132017a
/// `DisplacementManager::LegacySetup` 0.2; ours snaps).
pub const PILOTIS_SNAP_TIME: f32 = 0.2;

/// The pilotis test on a top's four edge points, `edges` = [behind (-sight),
/// ahead (+sight), one side, the other side] (retail
/// `GetClosestGapInDirection` 00d10ce2 x4: -sight and +sight from the box
/// origin, then +-side from their midpoint): the post centre (their mean)
/// if the nearest edge is within [`PILOTIS_EDGE_REACH`] of `probe` and the
/// top is at most [`PILOTIS_MAX_WIDTH`] both ways.
pub fn pilotis(probe: Vec3, edges: [Vec3; 4]) -> Option<Vec3> {
    let flat = |v: Vec3| v.truncate();
    let near = edges.iter().map(|e| (flat(*e) - flat(probe)).length()).fold(f32::INFINITY, f32::min);
    let along = (flat(edges[0]) - flat(edges[1])).length();
    let across = (flat(edges[2]) - flat(edges[3])).length();
    (near <= PILOTIS_EDGE_REACH && along <= PILOTIS_MAX_WIDTH && across <= PILOTIS_MAX_WIDTH)
        .then(|| edges.iter().copied().sum::<Vec3>() * 0.25)
}

/// `HumanInAir::CheckForTargetedLedgeGrab` (0131c1e4): a targeted jump has
/// arrived when the hand / feet target is closer than this, metres (0.01);
/// what follows depends on the target foothold's type (InAirData +0x2d0).
pub const TARGET_ARRIVED: f32 = 0.01;
/// Foothold types (InAirData +0x2d0) the arrival dispatches on: hangs
/// (1 and 0x20000; with a contact the fall goes on flat), a ladder
/// (0x2000), a beam top (0x100) and a fourth (0x200, not identified).
pub const FOOTHOLD_HANG: [u32; 2] = [1, 0x20000];
pub const FOOTHOLD_LADDER: u32 = 0x2000;
pub const FOOTHOLD_BEAM: u32 = 0x100;
/// The narrow-object entry type a targeted beam landing sets (NarrowObjectData
/// +200 = 8, then InAir -> NarrowObject via flag +0x3a0 bit1).
pub const NARROW_ENTRY_TARGETED_BEAM: u32 = 8;

/// What a targeted jump does on arrival (0131c1e4 dispatch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrival {
    Hang,
    Ladder,
    /// Onto a beam: the NarrowObject (beam) state, its landing clip picked
    /// by [`beam_landing_left`].
    Beam,
    Other,
}

pub fn arrival(foothold: u32) -> Arrival {
    match foothold {
        f if FOOTHOLD_HANG.contains(&f) => Arrival::Hang,
        FOOTHOLD_LADDER => Arrival::Ladder,
        FOOTHOLD_BEAM => Arrival::Beam,
        _ => Arrival::Other,
    }
}

/// `Human::GetFrontFoot` (00cbb628) for the beam landing clip: the left
/// landing (front foot 1) when the left foot is ahead (entity space,
/// `forward_l > forward_r`), else the right one (a tie picks the right).
pub fn beam_landing_left(forward_l: f32, forward_r: f32) -> bool {
    forward_l > forward_r
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
    /// A targeted jump's target in the open (feet): the fall homes onto it
    /// ([`home`]).
    pub home: Option<Vec3>,
}

impl Air {
    pub fn start(z: f32, delay: Option<f32>) -> Air {
        Air { start_z: z, time: 0.0, delay, wall_hits: 0, respawn: false, home: None }
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
    fn drop_time_matches_free_fall() {
        assert!((drop_time(3.0, 0.0, 0.0) - (6.0f32 / GRAVITY).sqrt()).abs() < 1e-5);
        // rising first takes longer; below the target no time left to fall
        assert!(drop_time(3.0, 0.0, 2.0) > drop_time(3.0, 0.0, 0.0));
        assert!(drop_time(0.0, 1.0, 0.0).abs() < 1e-6);
    }

    #[test]
    fn homing_meets_the_target_at_its_height() {
        // a sprint (5.5 m/s) off a 10 m roof toward a point 3 m out, 3 m down
        let dt = 1.0 / 60.0;
        let target = Vec3::new(0.0, 3.0, 7.0);
        let (mut pos, mut vel) = (Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 5.5, 0.0));
        let mut at_height = None;
        for _ in 0..240 {
            let v = home(pos, vel, target, dt);
            vel = v.extend(vel.z - GRAVITY * dt);
            pos += vel * dt;
            if at_height.is_none() && pos.z <= target.z {
                at_height = Some(pos);
            }
        }
        let hit = at_height.unwrap();
        assert!((hit.truncate() - target.truncate()).length() < 0.1, "{hit:?}");
        // then straight down
        assert!((pos.truncate() - hit.truncate()).length() < 0.1, "{pos:?}");
    }

    #[test]
    fn targeted_arrival() {
        assert_eq!(arrival(1), Arrival::Hang);
        assert_eq!(arrival(0x20000), Arrival::Hang);
        assert_eq!(arrival(0x2000), Arrival::Ladder);
        assert_eq!(arrival(0x100), Arrival::Beam);
        assert_eq!(arrival(0x200), Arrival::Other);
        assert!(beam_landing_left(0.2, 0.1));
        assert!(!beam_landing_left(0.1, 0.2));
        assert!(!beam_landing_left(0.1, 0.1));
    }

    #[test]
    fn pilotis_post() {
        let e = |x: f32, y: f32| Vec3::new(x, y, 2.0);
        // a 0.4 m square post at the origin, sight +Y
        let post = [e(0.0, -0.2), e(0.0, 0.2), e(0.2, 0.0), e(-0.2, 0.0)];
        assert_eq!(pilotis(Vec3::new(0.0, -0.5, 2.3), post), Some(Vec3::new(0.0, 0.0, 2.0)));
        // nearest edge 0.4 m away: too far
        assert_eq!(pilotis(Vec3::new(0.0, -0.6, 2.3), post), None);
        // 0.8 m long top (a wall end, not a post)
        assert_eq!(pilotis(Vec3::ZERO, [e(0.0, -0.4), e(0.0, 0.4), e(0.2, 0.0), e(-0.2, 0.0)]), None);
        assert_eq!(pilotis(Vec3::ZERO, [e(0.0, -0.2), e(0.0, 0.2), e(0.36, 0.0), e(-0.36, 0.0)]), None);
        assert!(pilotis(Vec3::ZERO, [e(0.0, -0.35), e(0.0, 0.35), e(0.35, 0.0), e(-0.35, 0.0)]).is_some());
    }

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

/// Gravity of the targeted jump's ballistic part, m/s^2
/// (`HumanInAir::UpdateMainMain` 01328022 passes -9.81 to [`drop_time`]).
pub const GRAVITY: f32 = 9.81;

/// Seconds to fall from `z` to `target_z` starting at vertical speed `vz`
/// (`GetDropTime` 012cf475: (vz + sqrt(vz^2 + 2 g max(z - target_z, 0))) / g).
pub fn drop_time(z: f32, target_z: f32, vz: f32) -> f32 {
    let h = (z - target_z).max(0.0);
    (vz + (vz * vz + 2.0 * GRAVITY * h).sqrt()) / GRAVITY
}

/// The constant horizontal acceleration that takes `pos` moving at `vel`
/// onto `target` in `t` seconds (`GetTargettingAcceleration` 01327fdd:
/// 2 (target - pos - t vel) / t^2).
pub fn targeting_accel(pos: Vec2, target: Vec2, vel: Vec2, t: f32) -> Vec2 {
    (target - pos - vel * t) * 2.0 / (t * t)
}

/// One tick of a targeted jump's ballistic part
/// (`HumanInAir::UpdateMainMain` 01328022, after the takeoff action): above
/// the target the horizontal velocity homes onto it so the two meet at the
/// target's height ([`drop_time`], [`targeting_accel`], applied for at most
/// the drop time); at or below it the horizontal velocity is zeroed and the
/// fall goes straight down. Returns the horizontal velocity for this tick;
/// gravity is the caller's.
pub fn home(pos: Vec3, vel: Vec3, target: Vec3, dt: f32) -> Vec2 {
    if pos.z <= target.z {
        return Vec2::ZERO;
    }
    let t = drop_time(pos.z, target.z, vel.z);
    if t <= 1e-4 {
        return Vec2::ZERO;
    }
    let a = targeting_accel(pos.truncate(), target.truncate(), vel.truncate(), t);
    vel.truncate() + a * dt.min(t)
}
