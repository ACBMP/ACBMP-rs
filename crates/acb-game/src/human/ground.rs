//! `HumanGround` edges: walking off a drop, stopping at one, lowering onto
//! the wall below.

use glam::{Vec2, Vec3};

/// A drop at least this deep is fallen off, shallower ones are stepped down,
/// metres (`HumanGround::TestHumanGroundToInAirOnHumanGroundInterfaceCommand_FallOffEdge`
/// 00fc3ed2: gap depth >= 0.53).
pub const FALL_OFF_EDGE: f32 = 0.53;

/// The edge stop (and pull-down onto the wall below) needs a drop deeper
/// than this, metres (`TestMoveToEdgeStop...UnbalanceOnEdge` 00fd2054,
/// `TestMoveToLedge...LedgePullDown` 00fd215e, `TestEdgeStopToLedge...` 00fd226a).
pub const EDGE_STOP_DEPTH: f32 = 2.0;

/// What walking over an edge with a drop of `depth` metres does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Snap down onto the lower floor (`HumanGround::CanStepDown`).
    Step,
    /// Into the air (FallOffEdge).
    Fall,
}

pub fn edge(depth: f32) -> Edge {
    if depth >= FALL_OFF_EDGE {
        Edge::Fall
    } else {
        Edge::Step
    }
}

/// Deep enough for the edge stop and pull-down (`None`: no floor below).
pub fn edge_stop(depth: Option<f32>) -> bool {
    depth.is_none_or(|d| d > EDGE_STOP_DEPTH)
}

/// A player in low profile pushing at a gap deeper than this brakes hard
/// at the edge instead of walking off, metres. The player brain never
/// sends UnbalanceOnEdge (only `GroundNavigationTask::CheckForFallOffEdge`
/// 01538746, NPCs, does): `AssassinTaskGround::UpdateGroundMovement`
/// 0166fb1c ~l.1040-1070 calls `IHumanGround::SetBreakHard(true)` (slot
/// 0x1f) for a gap within 0.3 m (`GetBestGapInDirectionInList`), deeper
/// than 2.5 m, within [`BREAK_HARD_CONE`] of the stick (stick > 0.35, high
/// profile not held), and the move cycle stops (`TestMoveCycleToMoveStopMain`
/// reads it).
pub const BREAK_HARD_DEPTH: f32 = 2.5;
/// See [`BREAK_HARD_DEPTH`] (0166fb1c: 1.2217305 rad, 70 deg).
pub const BREAK_HARD_CONE: f32 = 1.221_730_5;

/// Deep enough for the low-profile hard brake (`None`: no floor below).
pub fn break_hard(depth: Option<f32>) -> bool {
    depth.is_none_or(|d| d > BREAK_HARD_DEPTH)
}

/// Retail `HumanGround` EdgeStop: stopped at a drop, facing over it along
/// `along`. From here the jump button pulls down onto the wall below even
/// when standing still (`TestEdgeStopToLedge...LedgePullDown` 00fd226a);
/// from Move it pulls down directly (00fd215e).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeStop {
    pub at: Vec3,
    pub along: Vec2,
}

/// Leaving the stop position by more than this ends the edge stop, metres
/// (ours).
pub const EDGE_STOP_LEAVE: f32 = 0.05;

impl EdgeStop {
    pub fn still_at(&self, pos: Vec3) -> bool {
        self.at.distance(pos) <= EDGE_STOP_LEAVE
    }
}

/// Retail obstacle collision (`HumanGround` ObstacleCollision): running into
/// a wall that is neither climbed nor wall-run stops the move and plays the
/// bump clip, front or side blended by the wall's angle to the facing.
/// The potential-obstacle test (`Human::TestForPotentialObstacleCollision`
/// 00ceee20, called by `TestMoveToObstacleCollisionInterface` 00f44ad6)
/// needs the vertical speed within this, m/s.
pub const OBSTACLE_MAX_VZ: f32 = 0.2;

/// The contact must be within this angle of the move direction, radians
/// (00ceee20: `GetBestContactPointInDirection(.., 0.7853982 = pi/4, ..)`).
pub const OBSTACLE_CONE: f32 = std::f32::consts::FRAC_PI_4;

/// The contact point must be at least this high above the feet, metres
/// (00ceee20: contact z >= feet z + 0.5; lower contacts are steps). Our
/// wall sweeps run at 0.55 and 1.3 m, so every sweep hit qualifies.
pub const OBSTACLE_MIN_HEIGHT: f32 = 0.5;

/// Walls further than this from the facing (behind the player) take the
/// other branch, radians (`HumanGround::ProcessObstacleCollision` 00fd0fd2:
/// |angle| <= 2.3561945 = 135 deg).
pub const OBSTACLE_FACING: f32 = 3.0 * std::f32::consts::FRAC_PI_4;

/// Our contact probe: the stick direction swept this far from the body,
/// metres (ours; retail reads the controller's contact manifold).
pub const OBSTACLE_PROBE: f32 = 0.05;

/// An obstacle collision: the wall normal (horizontal, out of the wall),
/// the signed angle from the facing to the way into the wall (radians,
/// + = wall to the left) and the side blend (0 = front clip, 1 = side,
/// 00fd0fd2: |clamp(angle, +-90 deg)| / 90 deg).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obstacle {
    pub normal: Vec2,
    pub angle: f32,
    pub blend: f32,
}

/// Retail Move -> ObstacleCollision: a wall contact with normal `n` while
/// moving along `dir` (unit) at vertical speed `vz`, facing `facing`.
pub fn obstacle(n: Vec2, dir: Vec2, facing: Vec2, vz: f32) -> Option<Obstacle> {
    let into = -n;
    if vz.abs() > OBSTACLE_MAX_VZ || dir == Vec2::ZERO || into.dot(dir) < OBSTACLE_CONE.cos() {
        return None;
    }
    let angle = facing.angle_to(into);
    let half = std::f32::consts::FRAC_PI_2;
    let blend = angle.clamp(-half, half).abs() / half;
    Some(Obstacle { normal: n, angle, blend })
}

impl Obstacle {
    /// The wall is in front of the facing (front / side clip), not behind.
    pub fn faced(&self) -> bool {
        self.angle.abs() <= OBSTACLE_FACING
    }

    /// Still in contact: pushing `dir` within the cone into the same wall
    /// (retail leaves when the bump action ends, `TestObstacleCollisionToMoveMain`
    /// 00f305a6; ours when the push stops).
    pub fn holds(&self, dir: Vec2) -> bool {
        dir != Vec2::ZERO && (-self.normal).dot(dir) >= OBSTACLE_CONE.cos()
    }
}

/// Retail `HumanGround::TestIsEdgeValidForFall` (00fc38ca) ->
/// `Human::TestFallOffEdgeClearance` (00d01166): walking off an edge only
/// falls when an upright box just past the edge is clear (otherwise the
/// move is refused and the character stays on the edge). The box centre is
/// this far past the edge along the move, metres.
pub const FALL_BOX_AHEAD: f32 = 0.5;
/// ... and this far above the edge's height, metres (00d01166: + 0.7).
pub const FALL_BOX_UP: f32 = 0.7;
/// Box half extents: along the move, across it, up, metres
/// (00d01166: 0.25, 0.3, 1.0; so it spans 0.3 m below the edge to 1.7 m
/// above it).
pub const FALL_BOX_HALF: Vec3 = Vec3::new(0.25, 0.3, 1.0);

/// The clearance box for falling off an edge at `edge` (the first point
/// past it, at the height walked from) moving along `dir` (unit): its
/// centre; the half extents are [`FALL_BOX_HALF`] in (dir, side, up).
pub fn fall_box(edge: Vec3, dir: Vec2) -> Vec3 {
    edge + (dir * FALL_BOX_AHEAD).extend(FALL_BOX_UP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_off_target_by_profile_and_speed() {
        let (pos, y) = (Vec3::new(1.0, 2.0, 10.0), Vec2::Y);
        let at = |s: f32, high: bool| match fall_off(pos, y, y, s, high) {
            FallOff::Target(t) => t,
            FallOff::Back => panic!("back"),
        };
        // low profile: 1 m out whatever the speed; 3 m down
        assert!((at(0.8, false) - Vec3::new(1.0, 3.0, 7.0)).length() < 1e-5);
        // high profile: run 2.6 m, sprint 3 m
        assert!((at(0.8, true).y - 4.6).abs() < 1e-5);
        assert!((at(1.0, true).y - 5.0).abs() < 1e-5);
    }

    #[test]
    fn fall_off_direction_cone() {
        let pos = Vec3::ZERO;
        let dir = |facing: Vec2| match fall_off(pos, facing, Vec2::Y, 0.0, false) {
            FallOff::Target(t) => t.truncate(),
            FallOff::Back => Vec2::ZERO,
        };
        // within 60 deg: along the facing
        let f = Vec2::from_angle(0.9).rotate(Vec2::Y);
        assert!((dir(f) - f).length() < 1e-5);
        // 90 deg off: the gap direction turned 60 deg toward the facing
        let side = dir(Vec2::X);
        assert!((side.angle_to(Vec2::Y).abs() - FALL_OFF_CONE).abs() < 1e-5);
        assert!(side.x > 0.0);
        // backing over it (120 deg and more): a plain fall
        assert_eq!(fall_off(pos, -Vec2::Y, Vec2::Y, 1.0, true), FallOff::Back);
        assert_eq!(fall_off(pos, Vec2::from_angle(2.1).rotate(Vec2::Y), Vec2::Y, 1.0, true), FallOff::Back);
    }

    #[test]
    fn fall_off_edge_is_053() {
        assert_eq!(edge(0.2), Edge::Step);
        assert_eq!(edge(0.52), Edge::Step);
        assert_eq!(edge(0.53), Edge::Fall);
        assert_eq!(edge(0.6), Edge::Fall);
    }

    #[test]
    fn edge_stop_over_two_metres() {
        assert!(!edge_stop(Some(1.0)));
        assert!(!edge_stop(Some(2.0)));
        assert!(edge_stop(Some(2.1)));
        assert!(edge_stop(None));
    }

    #[test]
    fn low_profile_brakes_over_two_and_a_half_metres() {
        assert!(!break_hard(Some(2.2)));
        assert!(!break_hard(Some(2.5)));
        assert!(break_hard(Some(2.6)));
        assert!(break_hard(None));
    }

    #[test]
    fn obstacle_cone_and_blend() {
        let n = -Vec2::X; // wall ahead along +X
        let o = obstacle(n, Vec2::X, Vec2::X, 0.0).unwrap();
        assert_eq!(o.blend, 0.0);
        assert!(o.faced());
        // pushing 44 deg off: still; 46 deg: slides past
        assert!(obstacle(n, Vec2::from_angle(44f32.to_radians()), Vec2::X, 0.0).is_some());
        assert!(obstacle(n, Vec2::from_angle(46f32.to_radians()), Vec2::X, 0.0).is_none());
        // falling or rising faster than 0.2 m/s: no obstacle
        assert!(obstacle(n, Vec2::X, Vec2::X, 0.3).is_none());
        assert!(obstacle(n, Vec2::X, Vec2::X, -0.19).is_some());
        // facing 45 deg away: half side blend; 90 and more: full side
        let o = obstacle(n, Vec2::X, Vec2::from_angle(-std::f32::consts::FRAC_PI_4), 0.0).unwrap();
        assert!((o.blend - 0.5).abs() < 1e-5 && o.angle > 0.0);
        let o = obstacle(n, Vec2::X, Vec2::from_angle(2.5), 0.0).unwrap();
        assert_eq!(o.blend, 1.0);
        assert!(!o.faced());
        assert!(o.holds(Vec2::X) && !o.holds(Vec2::Y) && !o.holds(Vec2::ZERO));
    }

    #[test]
    fn fall_box_spans_below_and_above_the_edge() {
        let c = fall_box(Vec3::new(1.0, 0.0, 5.0), Vec2::X);
        assert!(c.abs_diff_eq(Vec3::new(1.5, 0.0, 5.7), 1e-6));
        assert!((c.z - FALL_BOX_HALF.z - 4.7).abs() < 1e-6);
        assert!((c.z + FALL_BOX_HALF.z - 6.7).abs() < 1e-6);
    }
}

/// Falling off an edge (`HumanGround::ExecuteFallOffEdge` 00fd2406, from
/// `DoFallOffEdge(gap, f)`): not a plain drop but a targeted jump
/// (`Human::DoTargetedJump`, ground source, landing intention 1) to a point
/// in the open past the edge, this far below the feet, metres...
pub const FALL_OFF_DROP: f32 = 3.0;
/// ...and `FALL_OFF_REACH + 2 * anim speed * f` ahead, metres, where `f` is
/// 1 in high profile and 0 in low profile
/// (`AssassinTaskGround::UpdateGroundMovement` 0166fb1c passes 1.0 when its
/// high-profile flag +0x1f64 is set) and the anim speed is the move cycle's
/// normalized speed (HumanGround +0x904, our [`crate::locomotion::anim_speed`]:
/// 0.2 walk, 0.8 run, 1.0 sprint). A sprint lands 3 m out, a run 2.6 m:
/// the `down 300cm` jump clips' reach.
pub const FALL_OFF_REACH: f32 = 1.0;
/// The jump goes along the facing when it is within this of the gap's
/// direction, else along the gap direction turned this far toward the
/// facing, radians (00fd2406: 1.0471976)...
pub const FALL_OFF_CONE: f32 = std::f32::consts::FRAC_PI_3;
/// ...and from this far off (backing over the edge) there is no jump: the
/// backward fall action (0x935ce89e) with a 0.4 s landing-test delay
/// (12000 ticks of 30000 per second), radians (00fd2406: 2.0943952).
pub const FALL_OFF_BACK: f32 = 2.0 * std::f32::consts::FRAC_PI_3;
/// That backward fall's landing-test delay, seconds.
pub const FALL_OFF_BACK_DELAY: f32 = 0.4;

/// How a fall off an edge goes ([`fall_off`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FallOff {
    /// Backing over the edge: a plain fall, landing tests after
    /// [`FALL_OFF_BACK_DELAY`].
    Back,
    /// A targeted jump to this point (feet), [`crate::human::in_air::home`].
    Target(Vec3),
}

/// The fall off an edge at `pos` facing `facing` over a gap that lies along
/// `gap` (both horizontal, unit), moving at normalized anim speed
/// `anim_speed`, `high` in high profile (`ExecuteFallOffEdge` 00fd2406).
/// Ours: `gap` is the move direction (retail's gap direction from the edge
/// detector, +0x20 of `IHuman::Gap`).
pub fn fall_off(pos: Vec3, facing: Vec2, gap: Vec2, anim_speed: f32, high: bool) -> FallOff {
    let a = gap.angle_to(facing);
    if !(a.abs() < FALL_OFF_BACK) {
        return FallOff::Back;
    }
    let dir = if a.abs() <= FALL_OFF_CONE { facing } else { Vec2::from_angle(FALL_OFF_CONE.copysign(a)).rotate(gap) };
    let reach = FALL_OFF_REACH + 2.0 * anim_speed * if high { 1.0 } else { 0.0 };
    FallOff::Target(pos + (dir * reach).extend(-FALL_OFF_DROP))
}
