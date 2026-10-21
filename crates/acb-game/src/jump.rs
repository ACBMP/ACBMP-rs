//! Freerun jumps: while running, a guidance ledge a little above and
//! ahead, or a drop with floor further on, launches the character on a
//! parabola that comes down on the target. The pose side (takeoff clip
//! held in the air, landing clip at touchdown) is
//! [`Locomotion::start_jump`](crate::locomotion::Locomotion::start_jump);
//! the clips' root tracks are not trajectories, the engine flies the
//! character.

use acb_anim::pose::Skeleton;
use glam::{Vec2, Vec3};

use crate::ground::{cell, Ground};
use crate::locomotion::{self, Clips};
use crate::player::Player;

/// Gravity, m/s^2 (ClothSettings use the same 10).
pub const GRAVITY: f32 = 10.0;

/// Horizontal speed a jump is timed for, m/s, and the flight time range.
const JUMP_SPEED: f32 = 6.0;
const FLIGHT: (f32, f32) = (0.4, 0.8);

/// Launch velocity that covers `d` in `t` seconds under gravity, passing
/// 0.05 m above the target then (descending there, so it clears the edge
/// before it).
pub fn launch(d: Vec3, t: f32) -> Vec3 {
    (d.truncate() / t).extend((d.z + 0.05 + 0.5 * GRAVITY * t * t) / t)
}

/// Jump kind ([`locomotion::JUMPS`]) for a displacement `d`: up 050 / 300,
/// front or down 050 / 300 / 550 by height and distance.
pub fn kind(d: Vec3) -> usize {
    let horiz = d.truncate().length();
    match (d.z, horiz) {
        (z, _) if z > 0.3 => usize::from(z > 1.2),                                  // up 050 / 300
        (z, h) if z < -1.0 => 5 + usize::from(h > 2.5) + usize::from(h > 4.0), // down
        (_, h) => 2 + usize::from(h > 2.5) + usize::from(h > 4.0),             // front
    }
}

/// A ledge to jump up onto from feet at `pos` running along `fwd`.
pub fn ledge_up(ground: &Ground, pos: Vec3, fwd: Vec2) -> Option<Vec3> {
    ground.ledge_ahead(pos, fwd, 2.0, 0.4, 2.2)
}

/// Where a running jump off a drop at `pos` along `fwd` lands: the first
/// floor 1.5 to 5.5 m ahead no more than 0.6 m higher and 5 m lower, with
/// headroom on the way.
pub fn across(ground: &Ground, pos: Vec3, fwd: Vec2) -> Option<Vec3> {
    (6..=22).map(|i| i as f32 * 0.25).find_map(|d| {
        let q = pos + (fwd * d).extend(0.0);
        let z = ground.floor_below(q + Vec3::Z * 0.6)?;
        if z < pos.z - 5.0 {
            return None;
        }
        // nothing in the way at chest height
        ground.sweep_wall(pos + Vec3::Z * 1.3, fwd * d, 0.3).is_none().then(|| q.with_z(z))
    })
}

/// Launches the player at `target`; `why` names the trigger for
/// `ACBMP_TRACE`.
pub(crate) fn take_off(p: &mut Player, clips: &Clips, sk: &Skeleton, target: Vec3, why: &str) {
    let d = target - p.pos;
    let horiz = d.truncate().length();
    let kind = kind(d);
    // a parabola that comes down onto the target at `t`
    let t = (horiz / JUMP_SPEED).clamp(FLIGHT.0, FLIGHT.1);
    p.vel = launch(d, t);
    p.fall_from = p.pos.z.max(target.z);
    p.loco.start_jump(clips, sk, kind);
    if crate::trace() {
        eprintln!(
            "jump {} from {:.2?} to {:.2?} ({why}) in {t:.2}s",
            locomotion::JUMPS[kind].0,
            p.pos.to_array(),
            target.to_array(),
        );
    }
}

impl Ground {
    /// A ledge to jump up onto from `pos` running along `fwd` (unit): the
    /// nearest guidance edge ahead within `reach` metres whose top is `min`
    /// to `max` above the feet and has floor just behind it. Returns the
    /// landing point on top.
    pub fn ledge_ahead(&self, pos: Vec3, fwd: Vec2, reach: f32, min: f32, max: f32) -> Option<Vec3> {
        let probe = pos.truncate() + fwd * reach * 0.5;
        let (c0, c1) = (cell(probe - Vec2::splat(reach)), cell(probe + Vec2::splat(reach)));
        let mut best: Option<(f32, Vec3)> = None;
        for x in c0.0..=c1.0 {
            for y in c0.1..=c1.1 {
                for &i in self.ledge_grid.get(&(x, y)).into_iter().flatten() {
                    let (a, b) = self.ledges[i as usize];
                    // closest point of the edge to the line ahead
                    let ab = b - a;
                    let t = if ab.length_squared() > 1e-6 { ((pos - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                    let q = a + ab * t;
                    let d = (q - pos).truncate();
                    let (dist, dz) = (d.length(), q.z - pos.z);
                    if dist < 0.3 || dist > reach || dz < min || dz > max || d.normalize().dot(fwd) < 0.8 {
                        continue;
                    }
                    // somewhere to stand just past the edge
                    let land = q + (fwd * 0.45).extend(0.0);
                    let Some(z) = self.floor_below(land + Vec3::Z * 0.5) else { continue };
                    if (z - q.z).abs() > 0.3 {
                        continue;
                    }
                    if best.is_none_or(|(bd, _)| dist < bd) {
                        best = Some((dist, land.with_z(z)));
                    }
                }
            }
        }
        best.map(|b| b.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_lands_on_the_target() {
        let d = Vec3::new(1.0, 2.5, 1.1);
        let t = 0.5;
        let v = launch(d, t);
        let at = v * t - Vec3::Z * (0.5 * GRAVITY * t * t);
        assert!((at.truncate() - d.truncate()).length() < 1e-5);
        assert!((at.z - (d.z + 0.05)).abs() < 1e-5);
        // and comes down there
        assert!(v.z - GRAVITY * t < 0.0);
    }

    #[test]
    fn kinds_by_height_and_distance() {
        assert_eq!(locomotion::JUMPS[kind(Vec3::new(0.0, 1.0, 0.5))], ("up", "050cm"));
        assert_eq!(locomotion::JUMPS[kind(Vec3::new(0.0, 1.0, 1.5))], ("up", "300cm"));
        assert_eq!(locomotion::JUMPS[kind(Vec3::new(0.0, 3.0, 0.0))], ("front", "300cm"));
        assert_eq!(locomotion::JUMPS[kind(Vec3::new(0.0, 5.0, -2.0))], ("down", "550cm"));
    }
}
