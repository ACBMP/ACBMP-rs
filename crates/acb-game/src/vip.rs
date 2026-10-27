//! Escort (teamvip) VIP ways. Retail builds them from the map's
//! `AdditionalWorldData_TeamVIP` (`BuildVipWaysFromWorld` 0x012d1228): a
//! list of `TeamVIPNavflowPath`s, each a list of waypoints, some flagged as
//! checkpoints. No PC forge ships that data, so we generate the ways from
//! the navmesh: start and goal on CrowdFlow lane points (main navmesh
//! component; triangle centroids when a map has no lanes), both at least
//! `MIN_DIST_FROM_KILLERS` from every team spawn, joined by the crowd's A*
//! path, string-pulled, with a checkpoint every `CHECKPOINT_EVERY` metres
//! and at the goal. A fixed seed and float maths that only use
//! `+ - * / sqrt` make the output the same on every machine.

use glam::{Vec2, Vec3};

use crate::crowd::{Nav, Rng};

/// `GBrick_SpawnVIPs` NbVIPs default (ReadFromXML 0x012d1376).
pub const NB_VIPS: usize = 2;
/// `WayChoice::g_MinDistFromKillers` (0x02e9097c): a way's start must be
/// this far from every hunter.
pub const MIN_DIST_FROM_KILLERS: f32 = 35.0;
/// Our reading: retail flags hand-placed waypoints; we space checkpoints.
pub const CHECKPOINT_EVERY: f32 = 25.0;
/// Ways generated per map (the pool `ChooseBestVipWay` picks from).
pub const WAYS: usize = 4;
/// Way length bounds (our choice: a long walk, not a lap of the map).
pub const MIN_LENGTH: f32 = 150.0;
pub const MAX_LENGTH: f32 = 400.0;
/// A way is up to `LEGS` A* legs between lane points; each leg's goal
/// stays `LEG_SPREAD` from the earlier ones (no walking back and forth).
const LEGS: usize = 6;
/// Candidate start/goal spacing along the lanes.
const LANE_STEP: f32 = 5.0;
const LEG_SPREAD: f32 = 30.0;
/// Starts of different ways stay this far apart (our choice).
const START_SPREAD: f32 = 40.0;
const SEED: u64 = 0x7EA4_71B0_E5C0_0002;
/// Sample step, clearance and height tolerance when checking a segment stays on the mesh.
const STEP: f32 = 0.1;
/// Clearance to the navmesh border either side of a way (our choice).
const MARGIN: f32 = 0.2;
const SEAM: f32 = 0.05;
const DZ: f32 = 0.6;

#[derive(Clone, Debug, PartialEq)]
pub struct Way {
    /// Navmesh polyline, start first.
    pub points: Vec<Vec3>,
    /// Indices into `points` flagged as checkpoints (the goal is the last).
    pub checkpoints: Vec<usize>,
}

impl Way {
    pub fn length(&self) -> f32 {
        self.points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    pub fn start(&self) -> Vec3 {
        self.points[0]
    }

    pub fn is_checkpoint(&self, i: usize) -> bool {
        self.checkpoints.binary_search(&i).is_ok()
    }
}

/// Team spawn positions (SpawnType 2, any TeamIndex) of the map.
pub fn team_spawns(archive: &acb_world::Archive) -> Vec<(u32, Vec3)> {
    archive
        .player_spawns()
        .into_iter()
        .filter(|s| s.kind == 2)
        .map(|s| (s.team, Vec3::new(s.matrix[12], s.matrix[13], s.matrix[14])))
        .collect()
}

fn far_from(p: Vec3, avoid: &[Vec3]) -> bool {
    avoid.iter().all(|a| a.distance(p) >= MIN_DIST_FROM_KILLERS)
}

/// Navmesh height under `p`, bridging the few-millimetre seams between
/// navmesh tiles (`SEAM` either way along x or y).
pub fn height(nav: &Nav, p: Vec3) -> Option<f32> {
    [Vec3::ZERO, Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y]
        .iter()
        .find_map(|d| nav.locate(p + *d * SEAM + Vec3::Z * 0.5).map(|(_, z)| z))
}

/// Whether the straight walk `a` -> `b` stays on the main navmesh, with
/// `MARGIN` to either side, within `DZ` of the straight height line.
pub fn clear(nav: &Nav, a: Vec3, b: Vec3) -> bool {
    let n = (a.distance(b) / STEP).ceil().max(1.0) as usize;
    let d = (b - a).truncate().normalize_or_zero();
    let side = Vec3::new(d.y, -d.x, 0.0) * MARGIN;
    (0..=n).all(|i| {
        let p = a.lerp(b, i as f32 / n as f32);
        [p, p + side, p - side]
            .iter()
            .all(|q| height(nav, *q).is_some_and(|z| (z - q.z).abs() < DZ))
    })
}

/// Greedy string pull: from each kept point, the furthest following
/// point still reachable in a straight clear line.
fn pull(nav: &Nav, pts: &[Vec3]) -> Vec<Vec3> {
    let mut out = vec![pts[0]];
    let mut i = 0;
    while i + 1 < pts.len() {
        let mut j = i + 1;
        while j + 1 < pts.len() && clear(nav, pts[i], pts[j + 1]) {
            j += 1;
        }
        out.push(pts[j]);
        i = j;
    }
    out
}

/// Insert a point every `CHECKPOINT_EVERY` metres and flag it; the goal is
/// a checkpoint too (one closer than half a spacing before it is dropped).
fn flag(pts: Vec<Vec3>) -> Way {
    let mut points = vec![pts[0]];
    let mut checkpoints = Vec::new();
    let mut since = 0.0;
    for w in pts.windows(2) {
        let (mut a, b) = (w[0], w[1]);
        while since + a.distance(b) >= CHECKPOINT_EVERY {
            let p = a + (b - a).normalize() * (CHECKPOINT_EVERY - since);
            checkpoints.push(points.len());
            points.push(p);
            a = p;
            since = 0.0;
        }
        since += a.distance(b);
        if since > 1e-3 {
            points.push(b);
        }
    }
    if since > 1e-3 && since < CHECKPOINT_EVERY * 0.5 {
        if let Some(c) = checkpoints.pop() {
            points.remove(c);
        }
    }
    let goal = points.len() - 1;
    if checkpoints.last() != Some(&goal) {
        checkpoints.push(goal);
    }
    Way { points, checkpoints }
}

/// Up to `WAYS` ways whose start and goal are at least
/// `MIN_DIST_FROM_KILLERS` from every point of `avoid`: the attacking
/// team's spawn group (`team_spawns` filtered by TeamIndex), or all four
/// groups for ways that suit either side (fewer on small maps).
/// Deterministic for a given navmesh and `avoid`; about 0.1-0.5 s.
pub fn ways_for(nav: &Nav, avoid: &[Vec3]) -> Vec<Way> {
    // on the mesh with `MARGIN` all round
    let snap = |p: Vec3| {
        let (t, z) = nav.locate(p + Vec3::Z * 0.5)?;
        let p = p.with_z(z);
        let m = MARGIN * 1.5;
        clear(nav, p - Vec3::X * m, p + Vec3::X * m).then_some((t, p))
    };
    let mut cand: Vec<(u32, Vec3)> = nav.lane_points(LANE_STEP).into_iter().filter_map(snap).filter(|c| far_from(c.1, avoid)).collect();
    if cand.len() < 8 {
        cand = nav.main_centroids().filter_map(snap).filter(|c| far_from(c.1, avoid)).collect();
    }
    let mut ways: Vec<Way> = Vec::new();
    if cand.len() < 2 {
        return ways;
    }
    let mut rng = Rng::new(SEED);
    let length = |p: &[Vec3]| p.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>();
    for _ in 0..256 {
        if ways.len() >= WAYS {
            break;
        }
        let (mut from, start) = cand[rng.below(cand.len())];
        if ways.iter().any(|w| w.start().distance(start) < START_SPREAD) {
            continue;
        }
        // legs to ever new lane points until the walk is long enough
        let mut path = vec![start];
        let mut visited = vec![start];
        for _leg in 0..LEGS {
            let total = length(&path);
            if total >= MIN_LENGTH {
                break;
            }
            let at = *path.last().unwrap();
            let mut best: Option<(u32, Vec3, Vec<Vec3>, f32)> = None;
            for _ in 0..16 {
                let (to, goal) = cand[rng.below(cand.len())];
                if visited.iter().any(|v| v.truncate().distance(goal.truncate()) < LEG_SPREAD) {
                    continue;
                }
                let Some(mut leg) = nav.path(from, to, goal) else { continue };
                leg.insert(0, at);
                let leg = pull(nav, &leg);
                let len = length(&leg);
                if total + len <= MAX_LENGTH && best.as_ref().is_none_or(|b| len > b.3) && leg.windows(2).all(|w| clear(nav, w[0], w[1])) {
                    best = Some((to, goal, leg, len));
                }
            }
            let Some((to, goal, leg, _)) = best else { break };
            path.extend_from_slice(&leg[1..]);
            visited.push(goal);
            from = to;
        }
        if length(&path) >= MIN_LENGTH {
            ways.push(flag(path));
        }
    }
    ways
}

/// A walker on a way: moves along the polyline itself (string-pulled ways
/// hug navmesh corners, so the crowd agent's turning arc would leave the
/// mesh there), facing turns at the crowd agent's rate.
#[derive(Clone, Debug)]
pub struct Walker {
    pub pos: Vec3,
    pub yaw: f32,
    /// Index of the waypoint being walked to; `points.len()` = arrived.
    pub next: usize,
}

const ARRIVE: f32 = 0.01;
const TURN: f32 = 4.0;

fn wrap(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

impl Walker {
    pub fn new(way: &Way) -> Walker {
        let d = way.points.get(1).map_or(Vec2::Y, |p| (*p - way.points[0]).truncate());
        Walker { pos: way.points[0], yaw: (-d.x).atan2(d.y), next: 1 }
    }

    pub fn arrived(&self, way: &Way) -> bool {
        self.next >= way.points.len()
    }

    /// One step at `pace` m/s. Returns the waypoint index when one is
    /// reached (check `Way::is_checkpoint`).
    pub fn step(&mut self, nav: &Nav, way: &Way, pace: f32, dt: f32) -> Option<usize> {
        let target = *way.points.get(self.next)?;
        let to = (target - self.pos).truncate();
        // reached, or passed it (beyond the perpendicular through it)
        let prev = way.points[self.next - 1];
        let along = (target - prev).truncate();
        if to.length() < ARRIVE || to.dot(along) < 0.0 {
            self.next += 1;
            return Some(self.next - 1);
        }
        // the body turns at the crowd's rate; the feet stay on the way
        let want = (-to.x).atan2(to.y);
        self.yaw = wrap(self.yaw + wrap(want - self.yaw).clamp(-TURN * dt, TURN * dt));
        let d = (target - self.pos).clamp_length_max(pace * dt);
        let moved = self.pos + d;
        self.pos = height(nav, moved).map_or(moved, |z| moved.with_z(z));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_every_spacing_and_goal() {
        let w = flag(vec![Vec3::ZERO, Vec3::new(60.0, 0.0, 0.0), Vec3::new(60.0, 40.0, 0.0)]);
        // 100 m: checkpoints at 25, 50, 75 and the goal at 100
        let at: Vec<Vec3> = w.checkpoints.iter().map(|&i| w.points[i]).collect();
        assert_eq!(at.len(), 4, "{at:?}");
        assert!(at[0].distance(Vec3::new(25.0, 0.0, 0.0)) < 1e-3);
        assert!(at[2].distance(Vec3::new(60.0, 15.0, 0.0)) < 1e-3);
        assert_eq!(*w.checkpoints.last().unwrap(), w.points.len() - 1);
        assert!((w.length() - 100.0).abs() < 1e-3);
        // a short tail drops the checkpoint just before the goal
        let w = flag(vec![Vec3::ZERO, Vec3::new(55.0, 0.0, 0.0)]);
        assert_eq!(w.checkpoints.len(), 2);
    }
}
