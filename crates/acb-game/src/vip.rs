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

/// `GBrick_SpawnVIPs` TimeBeforeRespawnAllVIPs default (ReadFromXML
/// 0x012d1376): all VIPs dead -> this long -> all respawn.
pub const TIME_BEFORE_RESPAWN_ALL: f32 = 5.0;
/// HuntersDelayBeforeRespawnAllVIPs: respawned VIPs become hunter targets
/// this long after they appear (not on the first spawn of a round).
pub const HUNTERS_DELAY: f32 = 10.0;

impl Way {
    /// The same way walked goal to start; its last point (the old start)
    /// is a checkpoint like the old goal.
    pub fn reversed(&self) -> Way {
        let last = self.points.len().saturating_sub(1);
        let mut checkpoints: Vec<usize> = self.checkpoints.iter().filter(|&&i| i != last && i != 0).map(|&i| last - i).collect();
        checkpoints.push(last);
        checkpoints.sort_unstable();
        checkpoints.dedup();
        Way { points: self.points.iter().rev().copied().collect(), checkpoints }
    }
}

/// The ways of one map and attacking team, each also reversed.
#[derive(Clone, Debug)]
pub struct WaySet {
    pub fwd: Vec<Way>,
    pub back: Vec<Way>,
}

impl WaySet {
    pub fn new(fwd: Vec<Way>) -> WaySet {
        let back = fwd.iter().map(Way::reversed).collect();
        WaySet { fwd, back }
    }

    pub fn get(&self, way: usize, back: bool) -> &Way {
        if back { &self.back[way] } else { &self.fwd[way] }
    }
}

/// `ChooseBestVipWay` (0x0130be7e): refill the pool from all ways when it
/// is empty, then sort by distance to the hunters and take the first.
/// Our reading of the sort: the nearest way whose start is at least
/// `MIN_DIST_FROM_KILLERS` from every hunter; if none is, the farthest.
/// Ties go to the lower index. Takes the way out of the pool.
pub fn choose_way(pool: &mut Vec<usize>, ways: &[Way], killers: &[Vec3]) -> Option<usize> {
    if pool.is_empty() {
        pool.extend(0..ways.len());
    }
    let dist = |w: usize| killers.iter().map(|k| (ways[w].start() - *k).truncate().length()).fold(f32::INFINITY, f32::min);
    let ok = pool.iter().copied().filter(|&w| dist(w) >= MIN_DIST_FROM_KILLERS).min_by(|&a, &b| dist(a).total_cmp(&dist(b)).then(a.cmp(&b)));
    let pick = ok.or_else(|| pool.iter().copied().max_by(|&a, &b| dist(a).total_cmp(&dist(b)).then(b.cmp(&a))))?;
    pool.retain(|&w| w != pick);
    Some(pick)
}

/// A living VIP walking its way.
#[derive(Clone, Debug)]
pub struct Vip {
    pub way: usize,
    /// Walking the way goal to start. Our reading: retail has no shipped
    /// data for what a VIP does at its way's end (the round may end on it,
    /// MPEMT_ENDROUND_*_VIPEscaped); ours turns round and walks it back.
    pub back: bool,
    pub walker: Walker,
    /// Checkpoints passed since it spawned (the `n` of the checkpoint event).
    pub checkpoints: u32,
    /// Walk cycle phase 0..1.
    pub phase: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VipEvent {
    /// VIP slot `vip` appeared on `way` (body `body`).
    Spawned { vip: usize, body: usize, way: usize },
    /// The VIPs became hunter targets (after `HUNTERS_DELAY`).
    Targetable,
    /// Retail GameModeEvent 0xf sub 2.
    Killed { vip: usize, body: usize, killer: usize, way: usize },
    /// Retail GameModeEvent 0xf sub 1: VIP, way, waypoint, n passed so far.
    Checkpoint { vip: usize, way: usize, checkpoint: usize, n: u32 },
    /// The VIP reached its way's end (it turns round; see [`Vip::back`]).
    WayEnd { vip: usize, way: usize },
}

/// The VIP cycle (`GBrick_SpawnVIPs::Update` 0x0130c150): NbVIPs spawn on
/// the best ways; a killed VIP is removed and its way returned to the pool
/// (no one-by-one replacement); when all are dead, `TIME_BEFORE_RESPAWN_ALL`
/// later all respawn, and hunters may target them `HUNTERS_DELAY` after
/// that (except on the first spawn). Everything in ticks: part of the
/// simulated match state.
#[derive(Clone, Debug)]
pub struct Vips {
    /// Body reserved for each VIP slot.
    pub bodies: Vec<usize>,
    pub slots: Vec<Option<Vip>>,
    /// Ways free to spawn on.
    pub pool: Vec<usize>,
    /// Ticks until all VIPs (re)spawn.
    pub respawn_in: Option<u32>,
    /// Ticks until hunters may target the living VIPs.
    pub target_in: u32,
    /// Spawns so far (0: the next is the first of the round).
    pub spawns: u32,
    /// This tick's events (the world clears them every tick).
    pub events: Vec<VipEvent>,
}

impl Vips {
    /// VIPs on `bodies`; they spawn on the first tick.
    pub fn new(bodies: Vec<usize>) -> Vips {
        let slots = vec![None; bodies.len()];
        Vips { bodies, slots, pool: Vec::new(), respawn_in: Some(0), target_in: 0, spawns: 0, events: Vec::new() }
    }

    /// Bodies hunters may target now.
    pub fn targets(&self) -> Vec<usize> {
        if self.target_in > 0 {
            return Vec::new();
        }
        self.bodies.iter().zip(&self.slots).filter(|(_, s)| s.is_some()).map(|(&b, _)| b).collect()
    }

    pub fn slot_of(&self, body: usize) -> Option<usize> {
        self.bodies.iter().position(|&b| b == body)
    }

    /// Hunter `killer` killed body `body`: if it is a living VIP, it is
    /// removed and its way goes back to the pool (`HandleKillEvent`
    /// 0x01301724); the last one starts the respawn timer.
    pub fn killed(&mut self, body: usize, killer: usize) -> bool {
        let Some(vip) = self.slot_of(body) else { return false };
        let Some(v) = self.slots[vip].take() else { return false };
        if !self.pool.contains(&v.way) {
            self.pool.push(v.way);
        }
        self.events.push(VipEvent::Killed { vip, body, killer, way: v.way });
        if self.slots.iter().all(Option::is_none) {
            self.respawn_in = Some(crate::mode::ticks(TIME_BEFORE_RESPAWN_ALL));
        }
        true
    }

    /// One tick of timers: spawns when due (ways chosen against `killers`,
    /// the hunters' positions). Returns the slots that spawned.
    pub fn tick(&mut self, ways: &WaySet, killers: &[Vec3]) -> Vec<usize> {
        if self.target_in > 0 {
            self.target_in -= 1;
            if self.target_in == 0 {
                self.events.push(VipEvent::Targetable);
            }
        }
        match self.respawn_in {
            Some(0) => {}
            Some(n) => {
                self.respawn_in = Some(n - 1);
                return Vec::new();
            }
            None => return Vec::new(),
        }
        self.respawn_in = None;
        let mut spawned = Vec::new();
        for vip in 0..self.slots.len() {
            let Some(way) = choose_way(&mut self.pool, &ways.fwd, killers) else { break };
            let walker = Walker::new(&ways.fwd[way]);
            // spread the walk cycles (our choice)
            let phase = (vip as f32 * 0.37).fract();
            self.slots[vip] = Some(Vip { way, back: false, walker, checkpoints: 0, phase });
            self.events.push(VipEvent::Spawned { vip, body: self.bodies[vip], way });
            spawned.push(vip);
        }
        self.target_in = if self.spawns == 0 { 0 } else { crate::mode::ticks(HUNTERS_DELAY) };
        self.spawns += 1;
        spawned
    }

    /// Walks every living VIP one step at `pace` m/s; a walk cycle of
    /// `cycle` seconds advances the phase.
    pub fn walk(&mut self, nav: &Nav, ways: &WaySet, pace: f32, cycle: f32, dt: f32) {
        for (vip, slot) in self.slots.iter_mut().enumerate() {
            let Some(v) = slot else { continue };
            let way = ways.get(v.way, v.back);
            if let Some(i) = v.walker.step(nav, way, pace, dt) {
                if way.is_checkpoint(i) {
                    v.checkpoints += 1;
                    self.events.push(VipEvent::Checkpoint { vip, way: v.way, checkpoint: i, n: v.checkpoints });
                }
            }
            if v.walker.arrived(way) {
                self.events.push(VipEvent::WayEnd { vip, way: v.way });
                v.back = !v.back;
                let yaw = v.walker.yaw;
                v.walker = Walker::new(ways.get(v.way, v.back));
                v.walker.yaw = yaw;
            }
            v.phase = (v.phase + dt / cycle.max(1e-3)).fract();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Straight 100 m ways along +X at y = 0, 100, 200, ...
    fn lines(n: usize) -> WaySet {
        let way = |k: usize| {
            let y = k as f32 * 100.0;
            Way { points: vec![Vec3::new(0.0, y, 0.0), Vec3::new(50.0, y, 0.0), Vec3::new(100.0, y, 0.0)], checkpoints: vec![1, 2] }
        };
        WaySet::new((0..n).map(way).collect())
    }

    #[test]
    fn cycle_in_ticks() {
        let ways = lines(4);
        // a hunter by way 0's start: way 0 is too close, way 1 (100 m) is the nearest far enough
        let killers = [Vec3::new(0.0, 10.0, 0.0)];
        let mut v = Vips::new(vec![7, 8]);
        assert_eq!(v.tick(&ways, &killers), vec![0, 1]);
        let way_of = |v: &Vips, s: usize| v.slots[s].as_ref().unwrap().way;
        assert_eq!((way_of(&v, 0), way_of(&v, 1)), (1, 2));
        // first spawn: targets at once
        assert_eq!(v.targets(), vec![7, 8]);
        // kill one: removed, its way back in the pool, no respawn yet
        assert!(v.killed(7, 0));
        assert!(!v.killed(7, 0), "already dead");
        assert_eq!(v.targets(), vec![8]);
        assert!(v.pool.contains(&1));
        for _ in 0..1000 {
            assert!(v.tick(&ways, &killers).is_empty());
        }
        // the second: 5 s (300 ticks) later both respawn
        assert!(v.killed(8, 1));
        assert_eq!(v.respawn_in, Some(300));
        let mut at = None;
        // t 0: the tick of the kill (World::step kills, then ticks)
        for t in 0..=400 {
            if !v.tick(&ways, &killers).is_empty() {
                at = Some(t);
                break;
            }
        }
        assert_eq!(at, Some(300));
        assert!(v.slots.iter().all(Option::is_some));
        // respawned: hunters wait 10 s (600 ticks)
        assert!(v.targets().is_empty());
        for _ in 0..599 {
            v.tick(&ways, &killers);
        }
        assert!(v.targets().is_empty());
        v.events.clear();
        v.tick(&ways, &killers);
        assert_eq!(v.events, vec![VipEvent::Targetable]);
        assert_eq!(v.targets(), vec![7, 8]);
        // every spawned way starts >= 35 m from the killer
        for s in v.slots.iter().flatten() {
            assert!(ways.fwd[s.way].start().distance(killers[0]) >= MIN_DIST_FROM_KILLERS);
        }
    }

    #[test]
    fn reversed_way_flags_its_goal() {
        let w = &lines(1).fwd[0];
        let r = w.reversed();
        assert_eq!(r.points[0], w.points[2]);
        assert_eq!(r.checkpoints, vec![1, 2]);
    }

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
