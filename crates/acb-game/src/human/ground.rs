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
