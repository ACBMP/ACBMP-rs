//! Freerun jumps, after retail's targeted jumps.
//!
//! **Where:** a landing is any floor of the collision geometry (railing and
//! beam tops and posts included, not only guidance edges) inside the
//! retail landing detection polygon [`DETECTION`]: forward distance against
//! height relative to the feet at takeoff. Narrow tops are centred under
//! the feet ([`Ground::settle_top`]).
//!
//! **How long, which path:** retail flies a targeted jump on its clips
//! (`IgnoreGravity`): the takeoff clip `xx_h_run_{dir}_{dist}_*_to_air`
//! then the air clip `xx_h_air_{dir}_{dist}_*_to_freestep`, back to back,
//! the jump's progress being elapsed time over the two lengths, and the
//! clips' root displacement warped onto the target. The distance classes
//! (050 / 300 / 550 cm) are blended by the actual distance. So flight time
//! and arc come from the data; [`Flight`] reproduces that in closed form
//! of the elapsed time, stepped on the 60 Hz tick.
//!
//! **Falling** (no target: off an edge, dropping from a wall) is
//! ballistic under [`GRAVITY`].

use acb_anim::pose::Clip;
use glam::{Mat3, Vec2, Vec3};

use crate::ground::{Ground, STEP};
use crate::locomotion::{self, Clips};
use crate::player::Player;
use acb_anim::pose::Skeleton;

/// Free-fall gravity, m/s^2 (HumanInAir's drop time and in-air speed).
pub const GRAVITY: f32 = 9.81;

/// Retail landing detection polygon for a running jump onto the feet
/// (`HumanSharedData` jump detection curve set, the default source's
/// first curve): (forward, up) metres from the feet at takeoff, along the
/// facing. Up to 1.3 m up within 3.5 m, level ground out to about 5.5 m,
/// 8 m when 3 m lower, nothing deeper than 5 m.
pub const DETECTION: [(f32, f32); 7] = [(0.5, -5.0), (0.5, 1.3), (3.5, 1.3), (4.7, 0.8), (6.0, -0.5), (8.0, -3.0), (8.0, -5.0)];

/// Scale of the polygon's forward reach (retail
/// `Human::ForwardJumpDetectionScale`, 1 by default; it applies to the
/// inner points, the near and far columns stay).
pub const FORWARD_SCALE: f32 = 1.0;

/// How far ahead targets are searched (retail: 9 m times the scale).
pub const SEARCH: f32 = 9.0 * FORWARD_SCALE;

/// Highest landing the polygon allows `fwd` metres ahead (`None` outside
/// its forward range).
pub fn ceiling(fwd: f32) -> Option<f32> {
    let pts: Vec<(f32, f32)> =
        DETECTION.iter().enumerate().map(|(i, &(f, u))| (if (2..DETECTION.len() - 1).contains(&i) { f * FORWARD_SCALE } else { f }, u)).collect();
    let top = &pts[1..pts.len() - 1];
    if fwd < top[0].0 || fwd > top[top.len() - 1].0 {
        return None;
    }
    top.windows(2).find(|w| fwd <= w[1].0).map(|w| {
        let s = if w[1].0 > w[0].0 { (fwd - w[0].0) / (w[1].0 - w[0].0) } else { 1.0 };
        w[0].1 + (w[1].1 - w[0].1) * s
    })
}

/// Whether a landing `fwd` ahead and `up` higher is in [`DETECTION`].
pub fn reachable(fwd: f32, up: f32) -> bool {
    ceiling(fwd).is_some_and(|c| up <= c && up >= DETECTION[0].1)
}

/// Launch velocity that covers `d` in `t` seconds under gravity, passing
/// 0.05 m above the target then (descending there, so it clears the edge
/// before it).
pub fn launch(d: Vec3, t: f32) -> Vec3 {
    (d.truncate() / t).extend((d.z + 0.05 + 0.5 * GRAVITY * t * t) / t)
}

/// Jump direction classes: indices into [`locomotion::JUMPS`].
const UP: [usize; 2] = [0, 1];
const FRONT: [usize; 3] = [2, 3, 4];
const DOWN: [usize; 3] = [5, 6, 7];

/// End height of a direction class's clips (their mean), metres: retail
/// up / front / down jumps end 1.3 m up, 0.5 m and 3 m down.
fn class_height(clips: &Clips, kinds: &[usize]) -> f32 {
    kinds.iter().map(|&k| root_path(clips.jump(k, 0).0, clips.jump(k, 0).1).0[SAMPLES].z).sum::<f32>() / kinds.len() as f32
}

/// The jump kinds blended for a displacement `d`, `(kind, weight)`: the
/// two direction classes whose clips end nearest above and below `d.z`,
/// by height, each blended over its distance classes by the horizontal
/// distance.
pub fn weights(clips: &Clips, d: Vec3) -> Vec<(usize, f32)> {
    let h = d.truncate().length();
    let classes: [&[usize]; 3] = [&DOWN, &FRONT, &UP];
    let zs: Vec<f32> = classes.iter().map(|c| class_height(clips, c)).collect();
    let (lo, hi, w) = if d.z <= zs[0] {
        (0, 0, 0.0)
    } else if d.z >= zs[2] {
        (2, 2, 0.0)
    } else {
        let i = usize::from(d.z > zs[1]);
        (i, i + 1, (d.z - zs[i]) / (zs[i + 1] - zs[i]))
    };
    let mut out: Vec<(usize, f32)> = Vec::new();
    for (c, cw) in [(lo, 1.0 - w), (hi, w)] {
        for (k, kw) in blend(clips, classes[c], h) {
            if kw * cw > 0.0 {
                out.push((k, kw * cw));
            }
        }
    }
    out
}

/// The kind of most weight in a blend.
pub fn main_kind(kinds: &[(usize, f32)]) -> usize {
    kinds.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map_or(2, |k| k.0)
}

/// Samples per flight.
const SAMPLES: usize = 32;

/// A clip pair's root path: the takeoff's root track then the air clip's
/// from where the takeoff ends, `SAMPLES + 1` points uniform in time
/// (local: +Y forward, +Z up), and the total length.
fn root_path(takeoff: &Clip, air: &Clip) -> (Vec<Vec3>, f32) {
    let (a, b) = (takeoff.length, air.length);
    let end = takeoff.root_motion(a);
    let pts = (0..=SAMPLES)
        .map(|i| {
            let t = (a + b) * i as f32 / SAMPLES as f32;
            if t <= a {
                takeoff.root_motion(t).w_axis.truncate()
            } else {
                (end * air.root_motion(t - a)).w_axis.truncate()
            }
        })
        .collect();
    (pts, a + b)
}

/// The two jump kinds blended for a horizontal distance `h` within a
/// direction class, by the clips' own distances: (kind, weight).
fn blend(clips: &Clips, kinds: &[usize], h: f32) -> [(usize, f32); 2] {
    let reach = |k: usize| {
        let (p, _) = root_path(clips.jump(k, 0).0, clips.jump(k, 0).1);
        p[SAMPLES].truncate().length()
    };
    let r: Vec<f32> = kinds.iter().map(|&k| reach(k)).collect();
    if h <= r[0] {
        return [(kinds[0], 1.0), (kinds[0], 0.0)];
    }
    for i in 1..kinds.len() {
        if h <= r[i] {
            let w = (h - r[i - 1]) / (r[i] - r[i - 1]).max(1e-3);
            return [(kinds[i - 1], 1.0 - w), (kinds[i], w)];
        }
    }
    [(kinds[kinds.len() - 1], 1.0), (kinds[kinds.len() - 1], 0.0)]
}

/// A targeted jump in flight: the path is a closed-form function of the
/// elapsed time, so it does not depend on the tick or render rate.
#[derive(Clone)]
pub struct Flight {
    /// World positions of the feet, uniform in time over `dur`.
    path: Vec<Vec3>,
    pub dur: f32,
    pub t: f32,
    pub target: Vec3,
    pub lag: f32,
}

impl Flight {
    /// The flight from `start` to `target`, facing along the jump, on the
    /// blend of `kinds` (`(kind, weight)`) off `foot`: the blended clip
    /// path rotated onto the jump, its end error spread over the flight
    /// (horizontally linear; vertically early when the target is higher,
    /// late when lower, so edges are cleared). Retail lets a jump that
    /// ends lower than its clips drop under gravity: the extra drop adds
    /// its fall time. `lag` > 1 holds the horizontal progress back (`u^lag`
    /// of it at time fraction `u`), going up before going over an edge
    /// close ahead.
    pub fn new(clips: &Clips, kinds: &[(usize, f32)], foot: usize, start: Vec3, target: Vec3, lag: f32) -> Flight {
        let d = target - start;
        let yaw = (-d.x).atan2(d.y);
        let rot = Mat3::from_rotation_z(yaw);
        let mut local = vec![Vec3::ZERO; SAMPLES + 1];
        let mut dur = 0.0;
        for &(k, w) in kinds {
            if w <= 0.0 {
                continue;
            }
            let (c, l) = clips.jump(k, foot);
            let (p, len) = root_path(c, l);
            for (o, q) in local.iter_mut().zip(p) {
                *o += q * w;
            }
            dur += len * w;
        }
        let err = d - rot * local[SAMPLES];
        if err.z < 0.0 {
            dur += (2.0 * -err.z / GRAVITY).sqrt();
        }
        let horiz = |u: f32| {
            let x = u * SAMPLES as f32;
            let i = (x.floor() as usize).min(SAMPLES - 1);
            let q = local[i].lerp(local[i + 1], x - i as f32);
            (rot * q).truncate() + err.truncate() * u
        };
        let path = local
            .iter()
            .enumerate()
            .map(|(i, &q)| {
                let u = i as f32 / SAMPLES as f32;
                let s = if err.z > 0.0 { 1.0 - (1.0 - u) * (1.0 - u) } else { u * u };
                start + horiz(u.powf(lag)).extend((rot * q).z + err.z * s)
            })
            .collect();
        Flight { path, dur: dur.max(1.0 / 60.0), t: 0.0, target, lag }
    }

    /// Feet position `t` seconds into the flight.
    pub fn at(&self, t: f32) -> Vec3 {
        let x = (t / self.dur).clamp(0.0, 1.0) * SAMPLES as f32;
        let i = (x.floor() as usize).min(SAMPLES - 1);
        self.path[i].lerp(self.path[i + 1], x - i as f32)
    }

    /// Advances by `dt`: the new feet position and whether it landed.
    pub fn step(&mut self, dt: f32) -> (Vec3, bool) {
        self.t += dt;
        (self.at(self.t), self.t >= self.dur)
    }

    /// Nothing in the way of the body (feet, waist and head height); the
    /// feet are not checked over the last half metre, where they come
    /// onto the top (up jumps arrive rising, at their clips' apex, over
    /// the edge in front of the target).
    pub fn clear(&self, ground: &Ground) -> bool {
        self.path.windows(2).all(|w| {
            let near = (w[1] - self.target).truncate().length() < 0.5;
            [0.2, 0.9, 1.6].iter().filter(|&&h| !(near && h < 0.5)).all(|&h| ground.ray(w[0] + Vec3::Z * h, w[1] - w[0]).is_none())
        })
    }
}

/// How far ahead a ledge or obstacle starts an up jump.
const UP_REACH: f32 = 2.0;
/// Sample spacing of the floor profile ahead, metres.
const PROFILE_STEP: f32 = 0.05;
/// Lateral offsets of the profiles (posts slightly off the line).
const LANES: [f32; 3] = [0.0, -0.25, 0.25];

/// A freerun jump target for feet at `pos` running along `fwd` (unit):
/// with `off_edge` false, the top of an obstacle rising more than a step
/// within [`UP_REACH`]; with it true (the floor ends), the first floor
/// past the gap, preferring one no more than 1.75 m lower. Targets are
/// in [`DETECTION`], stood on (centred on narrow tops) with headroom, and
/// returned with the flight that reaches them unobstructed.
pub fn target(ground: &Ground, clips: &Clips, pos: Vec3, fwd: Vec2, off_edge: bool) -> Option<(Vec3, Flight, bool)> {
    let side = Vec2::new(fwd.y, -fwd.x);
    for lane in LANES {
        let line = |d: f32| pos.truncate() + fwd * d + side * lane;
        // floor profile ahead, probed under the polygon's ceiling
        let n = ((if off_edge { SEARCH } else { UP_REACH }) / PROFILE_STEP) as usize;
        let prof: Vec<(f32, Option<f32>)> = (1..=n)
            .map(|i| {
                let d = i as f32 * PROFILE_STEP;
                let top = ceiling(d.max(DETECTION[0].0)).unwrap_or(DETECTION[1].1);
                (d, ground.floor_below(line(d).extend(pos.z + top + 0.05)))
            })
            .collect();
        if std::env::var_os("ACBMP_JUMPDBG").is_some() && lane == 0.0 {
            let zs: Vec<String> = prof.iter().step_by(4).map(|(_, z)| z.map_or("-".into(), |z| format!("{:.2}", z - pos.z))).collect();
            eprintln!("dbg prof {off_edge} {:.2?} {}", pos.to_array(), zs.join(" "));
        }
        let mut picks: Vec<(f32, f32)> = Vec::new();
        if off_edge {
            let Some(gap) = prof.iter().position(|(_, z)| z.is_none_or(|z| z < pos.z - STEP)) else { continue };
            let after: Vec<(f32, f32)> = prof[gap..]
                .iter()
                .filter_map(|&(d, z)| z.filter(|z| *z >= pos.z - STEP).map(|z| (d, z)))
                .take(1)
                .collect();
            picks.extend(after);
            // or down onto the first floor below the edge
            if let Some((d, z)) = prof[gap..].iter().find_map(|&(d, z)| z.map(|z| (d, z))) {
                picks.push((d, z));
            }
        } else {
            let mut last = pos.z;
            for &(d, z) in &prof {
                let Some(z) = z else { break };
                if z - last > STEP {
                    picks.push((d, z));
                    break;
                }
                if z < last - STEP {
                    break;
                }
                last = z;
            }
        }
        for (d, z) in picks {
            let edge = line(d).extend(z);
            // land a little past the edge, or centred on a narrow top
            // (from the profile sample itself: a 5 cm railing is missed a
            // step further on)
            let (at, narrow) = ground.settle_top(edge, fwd);
            let at = if narrow { at } else { ground.settle_top(edge + (fwd * 0.4).extend(0.0), fwd).0 };
            let rel = at - pos;
            let ahead = rel.truncate().dot(fwd);
            if std::env::var_os("ACBMP_JUMPDBG").is_some() {
                eprintln!("dbg cand {:.2?} ahead {ahead:.2} dz {:.2} narrow {narrow}", at.to_array(), rel.z);
            }
            if !reachable(ahead.max(DETECTION[0].0), rel.z) || (off_edge && rel.z < -3.2) {
                continue;
            }
            if ground.ray(at + Vec3::Z * 0.3, Vec3::Z * 1.5).is_some() {
                continue;
            }
            let kinds = weights(clips, rel);
            let flight = [1.0, 1.5, 2.0, 3.0].into_iter().map(|lag| Flight::new(clips, &kinds, 0, pos, at, lag)).find(|f| f.clear(ground));
            if let Some(flight) = flight {
                return Some((at, flight, narrow));
            }
            let flight = Flight::new(clips, &kinds, 0, pos, at, 1.0);
            if std::env::var_os("ACBMP_JUMPDBG").is_some() {
                let hit = flight.path.windows(2).enumerate().find_map(|(i, w)| [0.9f32, 1.6].iter().find(|&&h| ground.ray(w[0] + Vec3::Z * h, w[1] - w[0]).is_some()).map(|h| (i, *h, w[0])));
                eprintln!("dbg blocked path to {:.2?} narrow {narrow} dur {:.3} at {hit:?} end {:?}", at.to_array(), flight.dur, flight.path.last());
            }
        }
    }
    None
}

/// Launches the player on `flight` to `target`; `why` names the trigger
/// for `ACBMP_TRACE`.
pub(crate) fn take_off(p: &mut Player, clips: &Clips, sk: &Skeleton, target: Vec3, flight: Flight, why: &str) {
    let lag = flight.lag;
    let d = target - p.pos;
    let kinds = weights(clips, d);
    let main = main_kind(&kinds);
    let foot = p.loco.start_jump(clips, sk, main);
    // the flight on the clips of the foot it takes off from
    let flight = if foot == 0 { flight } else { Flight::new(clips, &kinds, foot, p.pos, target, lag) };
    p.yaw = (-d.x).atan2(d.y);
    p.vel = Vec3::ZERO;
    p.fall_from = p.pos.z.max(target.z);
    if crate::trace() {
        eprintln!(
            "jump {} {} ({why}) from {:.2?} to {:.2?}: {:.2} m ahead {:+.2} m in {:.3} s",
            locomotion::JUMPS[main].0,
            locomotion::JUMPS[main].1,
            p.pos.to_array(),
            target.to_array(),
            d.truncate().length(),
            d.z,
            flight.dur,
        );
    }
    p.flight = Some(flight);
}

impl Ground {
    /// Where to stand on the top under `p` (its floor height in `p.z`):
    /// a top narrower than [`NARROW`] across or along `fwd` (railing,
    /// beam, post) is centred on that axis. Returns the spot and whether
    /// the top was narrow.
    pub fn settle_top(&self, p: Vec3, fwd: Vec2) -> (Vec3, bool) {
        let Some(z) = self.floor_below(p + Vec3::Z * 0.15) else { return (p, false) };
        let mut at = p.with_z(z);
        let mut narrow = false;
        for axis in [fwd, Vec2::new(fwd.y, -fwd.x)] {
            let extent = |s: f32| {
                (1..=(NARROW / SETTLE_STEP) as usize)
                    .map(|i| i as f32 * SETTLE_STEP)
                    .find(|&e| {
                        let q = at + (axis * s * e).extend(0.0);
                        self.floor_below(q + Vec3::Z * 0.15).is_none_or(|f| (f - z).abs() > 0.12)
                    })
                    .unwrap_or(NARROW)
            };
            let (a, b) = (extent(1.0), extent(-1.0));
            if a + b < NARROW {
                narrow = true;
                at += (axis * (a - b) * 0.5).extend(0.0);
            }
        }
        (at, narrow)
    }
}

/// Tops narrower than this are balanced on (centred), metres.
pub const NARROW: f32 = 0.6;
/// Resolution of [`Ground::settle_top`]'s width probes (railings are 5-9 cm).
const SETTLE_STEP: f32 = 0.01;

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

    /// Retail reach (detection polygon): 1.3 m up out to 3.5 m, 0.8 m up
    /// at 4.7 m, level ground to about 5.5 m, 8 m when 3 m lower, not
    /// beyond 8 m or below 5 m, nothing closer than 0.5 m.
    #[test]
    fn retail_reach() {
        assert_eq!(ceiling(0.5), Some(1.3));
        assert_eq!(ceiling(3.5), Some(1.3));
        assert!((ceiling(4.7).unwrap() - 0.8).abs() < 1e-5);
        let level: f32 = 4.7 + 1.3 * 0.8 / 1.3; // where the 0.8 -> -0.5 edge crosses 0
        assert!((level - 5.5).abs() < 1e-5 && ceiling(level).unwrap().abs() < 1e-5);
        assert!(reachable(2.0, 1.0)); // a railing two metres ahead
        assert!(reachable(1.0, 1.3) && !reachable(1.0, 1.35));
        assert!(reachable(5.4, 0.0) && !reachable(5.6, 0.0));
        assert!(reachable(8.0, -3.0) && !reachable(8.1, -3.0));
        assert!(!reachable(0.4, 0.0) && !reachable(3.0, -5.1));
        assert_eq!(GRAVITY, 9.81);
    }
}
