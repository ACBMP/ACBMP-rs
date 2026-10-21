//! Wall climbing on freerun guidance edges.
//!
//! On the wall the character hangs from a hold on an edge with the
//! `xx_climb_wait_1m` pose: hands about 0.5 m ahead of and 1.6 m above the
//! entity, facing the wall. Moves between holds play the game's triples
//! (`xx_h_climb_1m_tr_climb_{u,d,l,r}_{l,r}hand_N`, `xx_h_climb_..._N`,
//! `..._tr_climb_1m`; N = 2 moves 1.2 m up / 1.5 m sideways, 3 moves
//! 1.8 m) with the entity carried from hold to hold in proportion to the
//! clip's own root progress, so any spacing works. Mounting from the
//! ground is `xx_h_wait_hipm_footl_tr_climbing_1m`.

use acb_anim::pose::{Clip, Skeleton};
use acb_anim::BuiltinTracks;
use acb_world::Archive;
use glam::{Vec2, Vec3};

use crate::ground::{cell, Ground};
use crate::Pose;

/// Hands relative to the entity in the wall pose (forward, up).
pub const HAND_FORWARD: f32 = 0.5;
pub const HAND_UP: f32 = 1.6;

/// A guidance edge as a climbing surface.
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    pub a: Vec3,
    pub b: Vec3,
    /// Outward horizontal normal of the wall below the edge.
    pub wall: Vec2,
    /// The edge has walkable ground on top.
    pub top: bool,
}

pub struct ClimbClips {
    pub mount: Clip,
    pub wait: Clip,
    /// [up, down, left, right] x [class 2, class 3] x (into, move, out)
    pub moves: [[(Clip, Clip, Clip); 2]; 4],
}

impl ClimbClips {
    pub fn load(archive: &Archive, builtins: Option<&BuiltinTracks>) -> anyhow::Result<ClimbClips> {
        let uids = archive.uids_of("Animation");
        let load = |name: &str| {
            uids.iter()
                .copied()
                .find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(name)))
                .and_then(|u| Clip::load_with(archive, u, builtins))
                .ok_or_else(|| anyhow::anyhow!("{name}: animation not found"))
        };
        let triple = |dir: &str, hand: &str, n: u32| -> anyhow::Result<(Clip, Clip, Clip)> {
            Ok((
                load(&format!("xx_h_climb_1m_tr_climb_{dir}_{hand}_{n}"))?,
                load(&format!("xx_h_climb_{dir}_{hand}_{n}"))?,
                load(&format!("xx_h_climb_{dir}_{hand}_{n}_tr_climb_1m"))?,
            ))
        };
        let pair = |dir: &str, hand: &str| -> anyhow::Result<[(Clip, Clip, Clip); 2]> { Ok([triple(dir, hand, 2)?, triple(dir, hand, 3)?]) };
        Ok(ClimbClips {
            mount: load("xx_h_wait_hipm_footl_tr_climbing_1m")?,
            wait: load("xx_climb_wait_1m")?,
            moves: [pair("u", "lhand")?, pair("d", "lhand")?, pair("l", "lhand")?, pair("r", "rhand")?],
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

enum Phase {
    /// Getting onto the wall: clip time.
    Mount(f32),
    /// Grabbed a hold while falling: glide onto it.
    Catch(f32),
    Wait(f32),
    /// Move direction, size class, stage 0..3, stage time, entity at start
    /// and end.
    Move { dir: usize, class: usize, stage: usize, t: f32, from: Vec3, to: Vec3, hold: Hold },
}

pub struct Climb {
    pub hold: Hold,
    /// Entity position and facing (yaw, entity +Y = (-sin, cos)).
    pub pos: Vec3,
    pub yaw: f32,
    phase: Phase,
    from: Vec3,
}

/// Facing into a wall with outward normal `n`.
fn yaw_into(n: Vec2) -> f32 {
    n.x.atan2(-n.y)
}

/// Entity position that puts the hands on `hold_point` of a wall facing `n`.
pub fn entity_for(hold_point: Vec3, n: Vec2) -> Vec3 {
    hold_point + (n * HAND_FORWARD).extend(0.0) - Vec3::Z * HAND_UP
}

impl Hold {
    /// The point of the edge nearest `p` horizontally.
    pub fn nearest(&self, p: Vec3) -> Vec3 {
        let ab = self.b - self.a;
        let t = if ab.truncate().length_squared() > 1e-6 {
            ((p - self.a).truncate().dot(ab.truncate()) / ab.truncate().length_squared()).clamp(0.05, 0.95)
        } else {
            0.5
        };
        self.a + ab * t
    }
}

/// Where the clip's root has got to between its start and end, 0..1.
fn progress(c: &Clip, t: f32) -> f32 {
    let end = c.root_motion(c.length).w_axis.truncate();
    let now = c.root_motion(t.min(c.length)).w_axis.truncate();
    if end.length() < 1e-3 {
        (t / c.length.max(1e-3)).min(1.0)
    } else {
        (now.dot(end) / end.length_squared()).clamp(0.0, 1.0)
    }
}

pub enum Outcome {
    Climbing,
    /// Reached the top: hop onto the ground at this point.
    TopOut(Vec3),
    /// Let go.
    Drop,
}

impl Climb {
    /// Starts climbing at `hold` from a character standing at `pos`.
    pub fn mount(hold: Hold, pos: Vec3) -> Climb {
        let yaw = yaw_into(hold.wall);
        Climb { hold, pos, yaw, phase: Phase::Mount(0.0), from: pos }
    }

    /// Grabs `hold` while falling past it at `pos`.
    pub fn catch(hold: Hold, pos: Vec3) -> Climb {
        let yaw = yaw_into(hold.wall);
        Climb { hold, pos, yaw, phase: Phase::Catch(0.0), from: pos }
    }

    /// Advances; `want` is the stick direction (up/down/left/right on the
    /// wall) and `find` looks for the next hold from a point in a
    /// direction. Returns the pose and what happened.
    pub fn update(
        &mut self,
        clips: &ClimbClips,
        sk: &Skeleton,
        want: Option<Dir>,
        find: &dyn Fn(Vec3, Vec2, Dir) -> Option<(Hold, Vec3)>,
        dt: f32,
    ) -> (Pose, Outcome) {
        let n = self.hold.wall;
        let hand = self.pos - (n * HAND_FORWARD).extend(0.0) + Vec3::Z * HAND_UP;
        match &mut self.phase {
            Phase::Mount(t) => {
                *t += dt;
                let c = &clips.mount;
                let target = entity_for(self.hold.nearest(self.from), n);
                self.pos = self.from.lerp(target, progress(c, *t));
                let pose = c.sample(sk, t.min(c.length));
                if *t >= c.length {
                    self.pos = target;
                    self.phase = Phase::Wait(0.0);
                }
                (pose, Outcome::Climbing)
            }
            Phase::Catch(t) => {
                const GLIDE: f32 = 0.15;
                *t += dt;
                let target = entity_for(self.hold.nearest(self.from), n);
                let k = (*t / GLIDE).min(1.0);
                self.pos = self.from.lerp(target, k * k * (3.0 - 2.0 * k));
                if *t >= GLIDE {
                    self.pos = target;
                    self.phase = Phase::Wait(0.0);
                }
                (clips.wait.sample(sk, 0.0), Outcome::Climbing)
            }
            Phase::Wait(t) => {
                *t += dt;
                let pose = clips.wait.sample(sk, *t % clips.wait.length.max(1e-3));
                let Some(dir) = want else { return (pose, Outcome::Climbing) };
                match find(hand, n, dir) {
                    Some((hold, point)) => {
                        let to = entity_for(point, hold.wall);
                        let dist = (to - self.pos).length();
                        let class = usize::from(dist > 1.5);
                        let d = [Dir::Up, Dir::Down, Dir::Left, Dir::Right].iter().position(|x| *x == dir).unwrap();
                        self.phase = Phase::Move { dir: d, class, stage: 0, t: 0.0, from: self.pos, to, hold };
                        (pose, Outcome::Climbing)
                    }
                    None if dir == Dir::Up && self.hold.top => {
                        // over the top onto the ground behind the edge
                        let ground = self.hold.nearest(hand) - (n * 0.6).extend(0.0);
                        (pose, Outcome::TopOut(ground))
                    }
                    None if dir == Dir::Down => (pose, Outcome::Drop),
                    None => (pose, Outcome::Climbing),
                }
            }
            Phase::Move { dir, class, stage, t, from, to, hold } => {
                *t += dt;
                let triple = &clips.moves[*dir][*class];
                let c = [&triple.0, &triple.1, &triple.2][*stage];
                if *stage == 1 {
                    self.pos = from.lerp(*to, progress(c, *t));
                }
                let pose = c.sample(sk, t.min(c.length));
                if *t >= c.length {
                    *t = 0.0;
                    *stage += 1;
                    if *stage == 3 {
                        self.pos = *to;
                        self.hold = *hold;
                        self.yaw = yaw_into(hold.wall);
                        self.phase = Phase::Wait(0.0);
                    }
                }
                (pose, Outcome::Climbing)
            }
        }
    }
}

/// Hold queries on the map's guidance edges.
impl Ground {
    fn holds_near(&self, p: Vec3, r: f32) -> impl Iterator<Item = &Hold> + '_ {
        let (c0, c1) = (cell(p.truncate() - Vec2::splat(r)), cell(p.truncate() + Vec2::splat(r)));
        let mut ids: Vec<u32> =
            (c0.0..=c1.0).flat_map(|x| (c0.1..=c1.1).map(move |y| (x, y))).filter_map(|k| self.hold_grid.get(&k)).flatten().copied().collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().map(|i| &self.holds[i as usize])
    }

    /// A hold to start climbing from feet at `pos` facing `fwd`: a wall
    /// edge facing us within 0.9 m, 1.3-2.6 m up.
    pub fn mount_hold(&self, pos: Vec3, fwd: Vec2) -> Option<Hold> {
        self.holds_near(pos, 1.5)
            .filter(|h| h.wall.dot(fwd) < -0.7)
            .filter_map(|h| {
                let q = h.nearest(pos);
                let d = (q - pos).truncate();
                let dz = q.z - pos.z;
                (d.length() < 0.9 && d.dot(fwd) > 0.0 && (1.3..=2.6).contains(&dz)).then_some((dz, *h))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
    }

    /// A hold to grab while falling at `pos` facing `fwd`: a wall edge
    /// facing us within 0.7 m ahead, 1.3-2.2 m above the feet.
    pub fn catch_hold(&self, pos: Vec3, fwd: Vec2) -> Option<Hold> {
        self.holds_near(pos, 1.5)
            .filter(|h| h.wall.dot(fwd) < -0.5)
            .filter_map(|h| {
                let q = h.nearest(pos);
                let d = (q - pos).truncate();
                let dz = q.z - pos.z;
                (d.length() < 0.7 && d.dot(fwd) > -0.1 && (1.3..=2.2).contains(&dz)).then_some((d.length(), *h))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
    }

    /// The next hold from hands at `hand` on a wall facing `n`, moving
    /// `dir`: an edge on the same wall plane, 0.5-2 m away that way.
    pub fn next_hold(&self, hand: Vec3, n: Vec2, dir: Dir) -> Option<(Hold, Vec3)> {
        // facing f = -n, the character's right is (f.y, -f.x)
        let right = Vec2::new(-n.y, n.x);
        let probe = match dir {
            Dir::Up => hand + Vec3::Z * 1.2,
            Dir::Down => hand - Vec3::Z * 1.2,
            Dir::Left => hand - (right * 1.2).extend(0.0),
            Dir::Right => hand + (right * 1.2).extend(0.0),
        };
        self.holds_near(probe, 2.0)
            .filter(|h| h.wall.dot(n) > 0.8)
            .filter_map(|h| {
                let q = h.nearest(probe);
                let d = q - hand;
                let (lat, up, out) = (d.truncate().dot(right), d.z, d.truncate().dot(n));
                let ok = out.abs() < 0.35
                    && match dir {
                        Dir::Up => (0.5..=2.0).contains(&up) && lat.abs() < 0.5,
                        Dir::Down => (-2.0..=-0.5).contains(&up) && lat.abs() < 0.5,
                        Dir::Left => (-2.0..=-0.5).contains(&lat) && up.abs() < 0.5,
                        Dir::Right => (0.5..=2.0).contains(&lat) && up.abs() < 0.5,
                    };
                ok.then_some((q.distance(probe), *h, q))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| (x.1, x.2))
    }

    /// Debug: standing spots from which a hold can be mounted, nearest
    /// `near` first (spot, facing).
    pub fn mount_spots(&self, near: Vec3, count: usize) -> Vec<(Vec3, Vec2)> {
        let mut out: Vec<(f32, Vec3, Vec2)> = self
            .holds
            .iter()
            .filter_map(|h| {
                let q = (h.a + h.b) / 2.0;
                let stand = q + (h.wall * 0.6).extend(0.0);
                let z = self.floor_below(stand.with_z(q.z - 1.2))?;
                let dz = q.z - z;
                // and another hold above to climb to
                let hand = q;
                let stacked = self.next_hold(hand, h.wall, Dir::Up).is_some();
                ((1.4..=2.5).contains(&dz) && stacked).then(|| (stand.with_z(z).distance(near), stand.with_z(z), -h.wall))
            })
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().take(count).map(|x| (x.1, x.2)).collect()
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entity_faces_the_wall_with_its_hands_on_the_hold() {
        for n in [Vec2::X, -Vec2::X, Vec2::Y, Vec2::new(0.6, -0.8)] {
            let yaw = yaw_into(n);
            let facing = Vec2::new(-yaw.sin(), yaw.cos());
            assert!(facing.dot(-n) > 0.999, "{n:?}");
            let hold = Vec3::new(3.0, 4.0, 5.0);
            let e = entity_for(hold, n);
            let hands = e + (facing * HAND_FORWARD).extend(0.0) + Vec3::Z * HAND_UP;
            assert!(hands.distance(hold) < 1e-5);
        }
    }
}
