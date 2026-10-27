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
}
