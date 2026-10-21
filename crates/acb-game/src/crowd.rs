//! Ambient crowd. On maps with `CrowdFlow` lanes NPCs walk them: along a
//! lane at a lateral offset within its width, then onto a connected lane
//! (weighted; one-way lanes are entered at their first point only), now
//! and then pausing. Without lanes they wander the navmesh: a random point
//! on the main walkable component, an A* path across the triangle graph
//! (through the links' crossing points), a pause, repeat. Animation is the
//! idle/walk blend; the walk cycle's root speed sets the pace.

use std::collections::{BinaryHeap, HashMap};

use acb_anim::pose::{blend, Clip, Skeleton};
use acb_world::{Archive, CrowdFlow, NavGraph};
use glam::{Mat4, Quat, Vec2, Vec3};

const CELL: f32 = 4.0;

pub struct Nav {
    graph: NavGraph,
    flows: Vec<CrowdFlow>,
    flow_index: HashMap<u32, usize>,
    /// Triangles of the largest connected component.
    main: Vec<u32>,
    centroid: Vec<Vec3>,
    /// XY cell -> triangles overlapping it.
    grid: HashMap<(i32, i32), Vec<u32>>,
}

fn cell(x: f32, y: f32) -> (i32, i32) {
    ((x / CELL).floor() as i32, (y / CELL).floor() as i32)
}

/// Barycentric height of `p` over triangle `t`, if `p.xy` is inside.
fn height_in(t: [Vec3; 3], p: Vec2) -> Option<f32> {
    let [a, b, c] = t;
    let d = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y);
    if d.abs() < 1e-9 {
        return None;
    }
    let l1 = ((b.y - c.y) * (p.x - c.x) + (c.x - b.x) * (p.y - c.y)) / d;
    let l2 = ((c.y - a.y) * (p.x - c.x) + (a.x - c.x) * (p.y - c.y)) / d;
    let l3 = 1.0 - l1 - l2;
    (l1 >= -1e-4 && l2 >= -1e-4 && l3 >= -1e-4).then(|| l1 * a.z + l2 * b.z + l3 * c.z)
}

impl Nav {
    pub fn load(archive: &Archive) -> Option<Nav> {
        let graph = archive.nav_graph();
        if graph.triangles.is_empty() {
            return None;
        }
        let n = graph.triangles.len();
        let mut comp = vec![u32::MAX; n];
        let mut best: Vec<u32> = Vec::new();
        for s in 0..n {
            if comp[s] != u32::MAX {
                continue;
            }
            let mut members = vec![s as u32];
            comp[s] = s as u32;
            let mut i = 0;
            while i < members.len() {
                let t = members[i] as usize;
                for (o, _) in &graph.links[t] {
                    if comp[*o as usize] == u32::MAX {
                        comp[*o as usize] = s as u32;
                        members.push(*o);
                    }
                }
                i += 1;
            }
            if members.len() > best.len() {
                best = members;
            }
        }
        let tri = |t: &[u32; 3]| t.map(|i| Vec3::from(graph.vertices[i as usize]));
        let centroid = graph.triangles.iter().map(|t| tri(t).iter().sum::<Vec3>() / 3.0).collect();
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for &t in &best {
            let v = tri(&graph.triangles[t as usize]);
            let lo = cell(v[0].x.min(v[1].x).min(v[2].x), v[0].y.min(v[1].y).min(v[2].y));
            let hi = cell(v[0].x.max(v[1].x).max(v[2].x), v[0].y.max(v[1].y).max(v[2].y));
            for x in lo.0..=hi.0 {
                for y in lo.1..=hi.1 {
                    grid.entry((x, y)).or_default().push(t);
                }
            }
        }
        let flows: Vec<CrowdFlow> = archive.crowd_flows().into_iter().filter(|f| f.points.len() >= 2).collect();
        let flow_index = flows.iter().enumerate().map(|(i, f)| (f.entity, i)).collect();
        Some(Nav { graph, flows, flow_index, main: best, centroid, grid })
    }

    pub fn triangles(&self) -> usize {
        self.main.len()
    }

    pub fn flows(&self) -> usize {
        self.flows.len()
    }

    /// Points of lane `f` walked forward or backward from point `from`,
    /// shifted sideways by `offset` metres.
    fn lane(&self, f: usize, forward: bool, from: usize, offset: f32) -> Vec<Vec3> {
        let p: Vec<Vec3> = self.flows[f].points.iter().map(|v| Vec3::from(*v)).collect();
        let n = p.len();
        let side = |i: usize| {
            let d = (p[(i + 1).min(n - 1)] - p[i.saturating_sub(1)]).truncate().normalize_or_zero();
            Vec3::new(d.y, -d.x, 0.0) * offset
        };
        let order: Vec<usize> = if forward { (from..n).collect() } else { (0..=from).rev().collect() };
        order.into_iter().map(|i| p[i] + side(i)).collect()
    }

    /// Start somewhere on a random lane (by length).
    fn lane_start(&self, rng: &mut Rng) -> Option<FlowState> {
        let len = |f: &CrowdFlow| f.points.windows(2).map(|w| Vec3::from(w[0]).distance(Vec3::from(w[1]))).sum::<f32>();
        let total: f32 = self.flows.iter().map(len).sum();
        let mut pick = rng.unit() * total;
        let f = self.flows.iter().position(|f| {
            pick -= len(f);
            pick <= 0.0
        })?;
        let flow = &self.flows[f];
        let forward = !flow.bidirectional || rng.unit() < 0.5;
        let offset = (rng.unit() - 0.5) * flow.width * 0.8;
        Some(FlowState { flow: f, forward, offset, from: rng.below(flow.points.len() - 1) + usize::from(!forward) })
    }

    /// The lane to take at the end of `s` (connected, weighted), or the
    /// same lane back when nothing connects.
    fn next_lane(&self, s: &FlowState, at: Vec3, rng: &mut Rng) -> FlowState {
        let flow = &self.flows[s.flow];
        let ends = if s.forward { &flow.tail } else { &flow.head };
        let options: Vec<(usize, bool, f32)> = ends
            .iter()
            .filter_map(|e| self.flow_index.get(e).copied())
            .filter_map(|f| {
                let g = &self.flows[f];
                let (first, last) = (Vec3::from(g.points[0]), Vec3::from(*g.points.last().unwrap()));
                let forward = first.distance(at) <= last.distance(at);
                (forward || g.bidirectional).then_some((f, forward, g.weight.max(0.05)))
            })
            .collect();
        let total: f32 = options.iter().map(|o| o.2).sum();
        let mut pick = rng.unit() * total;
        let chosen = options.iter().find(|o| {
            pick -= o.2;
            pick <= 0.0
        });
        match chosen {
            Some(&(f, forward, _)) => {
                let n = self.flows[f].points.len();
                let offset = s.offset.clamp(-self.flows[f].width * 0.4, self.flows[f].width * 0.4);
                FlowState { flow: f, forward, offset, from: if forward { 0 } else { n - 1 } }
            }
            None => {
                let n = flow.points.len();
                let forward = !s.forward && flow.bidirectional || !flow.bidirectional;
                FlowState { flow: s.flow, forward, offset: -s.offset, from: if forward { 0 } else { n - 1 } }
            }
        }
    }

    fn corners(&self, t: u32) -> [Vec3; 3] {
        self.graph.triangles[t as usize].map(|i| Vec3::from(self.graph.vertices[i as usize]))
    }

    /// Main-component triangle under `p` (the one nearest in height).
    pub fn locate(&self, p: Vec3) -> Option<(u32, f32)> {
        self.grid
            .get(&cell(p.x, p.y))?
            .iter()
            .filter_map(|&t| height_in(self.corners(t), p.truncate()).map(|z| (t, z)))
            .min_by(|a, b| (a.1 - p.z).abs().total_cmp(&(b.1 - p.z).abs()))
    }

    /// A uniformly chosen triangle's random interior point.
    pub fn random_point(&self, rng: &mut Rng) -> (u32, Vec3) {
        let t = self.main[rng.below(self.main.len())];
        let [a, b, c] = self.corners(t);
        let (mut u, mut v) = (rng.unit(), rng.unit());
        if u + v > 1.0 {
            (u, v) = (1.0 - u, 1.0 - v);
        }
        (t, a + (b - a) * u + (c - a) * v)
    }

    /// Waypoints from triangle `from` to `goal` in triangle `to`: the
    /// crossing points of an A* path over triangle centroids.
    pub fn path(&self, from: u32, to: u32, goal: Vec3) -> Option<Vec<Vec3>> {
        #[derive(PartialEq)]
        struct Open(f32, u32);
        impl Eq for Open {}
        impl PartialOrd for Open {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for Open {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                o.0.total_cmp(&self.0)
            }
        }
        let h = |t: u32| self.centroid[t as usize].distance(goal);
        let mut cost: HashMap<u32, f32> = HashMap::from([(from, 0.0)]);
        let mut came: HashMap<u32, (u32, Vec3)> = HashMap::new();
        let mut open = BinaryHeap::from([Open(h(from), from)]);
        while let Some(Open(_, t)) = open.pop() {
            if t == to {
                let mut pts = vec![goal];
                let mut cur = t;
                while let Some(&(prev, via)) = came.get(&cur) {
                    pts.push(via);
                    cur = prev;
                }
                pts.reverse();
                return Some(pts);
            }
            let here = cost[&t];
            let at = came.get(&t).map_or(self.centroid[t as usize], |c| c.1);
            for &(o, via) in &self.graph.links[t as usize] {
                let via = Vec3::from(via);
                let c = here + at.distance(via);
                if cost.get(&o).is_none_or(|&old| c < old) {
                    cost.insert(o, c);
                    came.insert(o, (t, via));
                    open.push(Open(c + h(o), o));
                }
            }
        }
        None
    }
}

/// xorshift64*, enough for wandering.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Where an agent is in the lane network.
#[derive(Clone, Copy)]
struct FlowState {
    flow: usize,
    forward: bool,
    offset: f32,
    /// Point index the current path starts at.
    from: usize,
}

#[derive(Clone)]
pub struct Agent {
    flow: Option<FlowState>,
    pub pos: Vec3,
    /// Facing: entity +Y is (-sin yaw, cos yaw).
    pub yaw: f32,
    path: Vec<Vec3>,
    /// Seconds left standing before picking the next goal.
    wait: f32,
    /// 0 idle .. 1 walk.
    speed: f32,
    phase: f32,
    idle_time: f32,
    rng: Rng,
}

fn wrap_angle(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

impl Agent {
    pub fn new(nav: &Nav, seed: u64) -> Agent {
        let mut rng = Rng::new(seed);
        let flow = nav.lane_start(&mut rng);
        let (path, pos) = match flow {
            Some(s) => {
                let path = nav.lane(s.flow, s.forward, s.from, s.offset);
                let pos = path[0];
                (path, nav.locate(pos + Vec3::Z * 0.5).map_or(pos, |(_, z)| pos.with_z(z)))
            }
            None => (Vec::new(), nav.random_point(&mut rng).1),
        };
        let yaw = match path.get(1) {
            Some(n) => (-(n.x - pos.x)).atan2(n.y - pos.y),
            None => rng.unit() * std::f32::consts::TAU,
        };
        let wait = rng.unit() * 3.0;
        Agent { flow, pos, yaw, path, wait, speed: 0.0, phase: rng.unit(), idle_time: rng.unit() * 5.0, rng }
    }

    pub fn entity(&self) -> Mat4 {
        Mat4::from_rotation_translation(Quat::from_rotation_z(self.yaw), self.pos)
    }

    /// One step: plans, walks the path, keeps clear of `others` (people
    /// nearby), returns the pose.
    pub fn update(&mut self, nav: &Nav, idle: &Clip, walk: &Clip, sk: &Skeleton, others: &[Vec3], dt: f32) -> Vec<(Vec3, Quat)> {
        if self.path.is_empty() {
            self.wait -= dt;
            if self.wait <= 0.0 {
                match self.flow {
                    Some(s) => {
                        let next = nav.next_lane(&s, self.pos, &mut self.rng);
                        self.path = nav.lane(next.flow, next.forward, next.from, next.offset);
                        self.flow = Some(next);
                        // most keep walking; some stop for a while
                        self.wait = if self.rng.unit() < 0.15 { 2.0 + self.rng.unit() * 6.0 } else { 0.0 };
                    }
                    None => {
                        let (to, goal) = nav.random_point(&mut self.rng);
                        if let Some((from, _)) = nav.locate(self.pos) {
                            self.path = nav.path(from, to, goal).unwrap_or_default();
                        }
                        self.wait = 2.0 + self.rng.unit() * 6.0;
                    }
                }
            }
        }
        let walking = !self.path.is_empty();
        self.speed = (self.speed + if walking { 2.0 } else { -2.0 } * dt).clamp(0.0, 1.0);
        let pace = walk.root_motion(walk.length).w_axis.truncate().length() / walk.length.max(1e-3);
        if let Some(&next) = self.path.first() {
            let to = (next - self.pos).truncate();
            if to.length() < 0.3 {
                self.path.remove(0);
            } else {
                let want = (-to.x).atan2(to.y);
                let turn = 4.0 * dt;
                self.yaw = wrap_angle(self.yaw + wrap_angle(want - self.yaw).clamp(-turn, turn));
            }
        }
        let fwd = Vec2::new(-self.yaw.sin(), self.yaw.cos());
        // separation: shoulder room of 0.7 m, mostly sideways
        let mut push = Vec2::ZERO;
        for o in others {
            let d = (self.pos - *o).truncate();
            let len = d.length();
            if len > 1e-3 && len < 0.7 && (self.pos.z - o.z).abs() < 1.5 {
                push += d / len * (0.7 - len);
            }
        }
        let step = fwd * pace * self.speed * dt + (push * 3.0 * dt).clamp_length_max(1.5 * dt);
        let moved = self.pos + step.extend(0.0);
        // stay on the navmesh; follow its height
        match nav.locate(moved + Vec3::Z * 0.5) {
            Some((_, z)) => self.pos = moved.with_z(z),
            None if self.path.is_empty() => {}
            None => {
                // cut a corner off the mesh: head for the waypoint directly
                if let Some(&next) = self.path.first() {
                    let d = (next - self.pos).truncate().normalize_or_zero() * pace * self.speed * dt;
                    self.pos += d.extend(0.0);
                    self.pos.z += (next.z - self.pos.z).clamp(-dt, dt);
                }
            }
        }
        if self.speed > 0.0 {
            self.phase = (self.phase + dt * self.speed / walk.length.max(1e-3)).fract();
        }
        self.idle_time = (self.idle_time + dt) % idle.length.max(1e-3);
        let mut pose = idle.sample(sk, self.idle_time);
        if self.speed > 0.0 {
            blend(&mut pose, &walk.sample(sk, self.phase * walk.length), self.speed);
        }
        pose
    }
}
