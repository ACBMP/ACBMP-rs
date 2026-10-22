//! Hiding spots (haystacks): map data, entry / exit with the retail clips.
//!
//! **Data:** every map Entity (placed directly or inside an EntityGroup)
//! with a `HayStackComponent`: `HayStackType` (0 on the ground, 1 in a
//! cart), `DetectionBoxMin/Max` in entity space (San Donato: 20 spots,
//! boxes about +-1.4 x +-0.9 m, 0..1.6 m high). Benches
//! (`RestObjectAttributeComponent`, three `RestSpot`s each, 34 on San
//! Donato) are not hiding spots in our model yet.
//!
//! **Clips** (DataPC.forge): in on foot `xx_h_freestep_footr_to_haystack_01`
//! (high profile, 1.0 m forward and 0.6 m down), from the air
//! `xx_h_air_to_haystack` (1.31 m forward, 1.2 m down), inside
//! `xx_l_haystack_wait` (loop), out `xx_l_haystack_hop_out` (2.0 m forward)
//! then `xx_l_haystack_hop_out_tr_l_wait`.
//!
//! **Rules (our reading):** a ground entry needs high profile (run or
//! sprint) moving into the box from within [`ENTRY_MARGIN`]; an air entry a
//! descending fall over the box no higher than [`AIR_ENTRY`] above its
//! top, and it takes no fall damage. Any move input inside leaves towards
//! it (camera relative). Hidden ([`crate::rules::Stance::Hidden`]) only
//! while fully inside; entering and leaving are plain ground stances.

use acb_anim::pose::Clip;
use acb_world::Archive;
use glam::{Mat4, Quat, Vec2, Vec3};


/// A hiding spot.
#[derive(Clone, Debug)]
pub struct Spot {
    pub entity: u32,
    /// Entity placement (box frame).
    pub frame: Mat4,
    /// Detection box in entity space.
    pub min: Vec3,
    pub max: Vec3,
    /// `HayStackType`: 0 ground, 1 cart.
    pub kind: u32,
}

impl Spot {
    /// Floor centre of the box, world space.
    pub fn center(&self) -> Vec3 {
        let c = (self.min + self.max) * 0.5;
        self.frame.transform_point3(Vec3::new(c.x, c.y, self.min.z))
    }

    /// `p` in box space.
    pub fn local(&self, p: Vec3) -> Vec3 {
        self.frame.inverse().transform_point3(p)
    }

    /// Horizontal distance from `p` to the box (0 inside), box space.
    pub fn outside(&self, p: Vec3) -> f32 {
        let l = self.local(p);
        let dx = (self.min.x - l.x).max(l.x - self.max.x).max(0.0);
        let dy = (self.min.y - l.y).max(l.y - self.max.y).max(0.0);
        Vec2::new(dx, dy).length()
    }

    /// Box top height, world.
    pub fn top(&self) -> f32 {
        self.frame.w_axis.z + self.max.z
    }
}

/// Every haystack the map places (forge 0).
pub fn spots(archive: &Archive) -> Vec<Spot> {
    let schema = archive.schema();
    let entity = schema.hash_of("Entity");
    let mut out: Vec<Spot> = Vec::new();
    let mut visit = |e: &anvil_fastload::Obj| {
        if Some(e.type_hash) != entity || out.iter().any(|s| s.entity == e.id) {
            return;
        }
        let Some(m) = archive.field(e, "GlobalMatrix").and_then(|v| v.as_f32s()) else { return };
        let Ok(m) = <[f32; 16]>::try_from(m) else { return };
        let comps = archive.field(e, "Components").map(|v| v.items()).unwrap_or_default();
        for c in comps.iter().filter_map(|v| v.as_obj()).filter(|c| archive.is_type(c, "HayStackComponent")) {
            let v3 = |n: &str| archive.field(c, n).and_then(|v| v.as_f32s()).filter(|f| f.len() >= 3).map(|f| Vec3::new(f[0], f[1], f[2]));
            let (Some(min), Some(max)) = (v3("DetectionBoxMin"), v3("DetectionBoxMax")) else { continue };
            let kind = archive.field(c, "HayStackType").and_then(|v| v.as_u32()).unwrap_or(0);
            let active = archive.field(c, "Active").and_then(|v| v.as_u32()).unwrap_or(1) != 0;
            if active {
                out.push(Spot { entity: e.id, frame: Mat4::from_cols_array(&m), min, max, kind });
            }
        }
    };
    for ty in ["Entity", "EntityGroup"] {
        for uid in archive.uids_of_in(ty, 0) {
            if let Ok(r) = archive.root(uid) {
                r.obj.walk(&mut |o| visit(o));
            }
        }
    }
    out
}

/// Hiding clips.
#[derive(Clone)]
pub struct HideClips {
    pub enter: Clip,
    pub air: Clip,
    pub wait: Clip,
    pub exit: Clip,
}

impl HideClips {
    pub fn load(archive: &Archive, builtins: Option<&acb_anim::BuiltinTracks>) -> Option<HideClips> {
        let load = |n: &str| {
            archive
                .uids_of("Animation")
                .into_iter()
                .find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(n)))
                .and_then(|a| Clip::load_with(archive, a, builtins))
        };
        Some(HideClips {
            enter: load("xx_h_freestep_footr_to_haystack_01")?,
            air: load("xx_h_air_to_haystack")?,
            wait: load("xx_l_haystack_wait")?,
            exit: load("xx_l_haystack_hop_out")?.then(&load("xx_l_haystack_hop_out_tr_l_wait")?),
        })
    }
}

/// The map's spots and the clips.
#[derive(Clone)]
pub struct Hide {
    pub spots: Vec<Spot>,
    pub clips: HideClips,
}

/// Ground entry reach outside the box, metres (our choice: one stride).
pub const ENTRY_MARGIN: f32 = 1.2;
/// Air entry: no higher than this above the box top, metres.
pub const AIR_ENTRY: f32 = 1.5;
/// Blend-in time into the entry clip, seconds.
const BLEND: f32 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Enter,
    In,
    Exit,
}

/// A player in (or going into / out of) a hiding spot.
#[derive(Clone)]
pub struct Hiding {
    pub spot: usize,
    pub phase: Phase,
    /// Seconds into the phase.
    pub t: f32,
    /// Entered from the air.
    pub air: bool,
    /// The phase clip's frame (world).
    frame: Mat4,
    from_pose: Vec<(Vec3, Quat)>,
    from_world: Mat4,
}

/// Which spot a player at `pos` facing `yaw` going `dir` (world, unit or
/// zero) enters, if any: `air` = falling.
pub fn entry(spots: &[Spot], pos: Vec3, dir: Vec2, high_profile: bool, air: bool, vz: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, s) in spots.iter().enumerate() {
        let c = s.center();
        let ok = if air {
            vz < 0.0 && s.outside(pos) <= 0.0 && pos.z <= s.top() + AIR_ENTRY && pos.z >= s.frame.w_axis.z + 0.3
        } else {
            let to = (c - pos).truncate();
            let d = s.outside(pos);
            high_profile && dir != Vec2::ZERO && d <= ENTRY_MARGIN && (pos.z - c.z).abs() < 0.6 && to.normalize_or_zero().dot(dir) > 0.5
        };
        let d = (c - pos).truncate().length();
        if ok && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|b| b.0)
}

fn yaw_of(dir: Vec2) -> f32 {
    (-dir.x).atan2(dir.y)
}

impl Hiding {
    /// Start entering spot `i`: the clip is placed so it ends at the box
    /// floor centre, heading from the player towards it.
    pub fn enter(hide: &Hide, i: usize, pos: Vec3, yaw: f32, air: bool, pose: Vec<(Vec3, Quat)>, world: Mat4) -> Hiding {
        let s = &hide.spots[i];
        let c = s.center();
        let to = (c - pos).truncate();
        let heading = if to.length() > 0.05 { yaw_of(to.normalize()) } else { yaw };
        let clip = if air { &hide.clips.air } else { &hide.clips.enter };
        let end = clip.root_motion(clip.length).w_axis.truncate();
        let rot = Quat::from_rotation_z(heading);
        let origin = c - rot * end;
        let frame = Mat4::from_rotation_translation(rot, origin);
        Hiding { spot: i, phase: Phase::Enter, t: 0.0, air, frame, from_pose: pose, from_world: world }
    }

    pub fn hidden(&self) -> bool {
        self.phase == Phase::In
    }

    /// One tick. `leave`: a move direction (world, unit) to leave towards.
    /// Returns the pose, its world placement, the logical position / yaw,
    /// and whether the player is out (the exit finished).
    pub fn update(&mut self, hide: &Hide, sk: &acb_anim::pose::Skeleton, leave: Option<Vec2>, dt: f32) -> (Vec<(Vec3, Quat)>, Mat4, Vec3, f32, bool) {
        self.t += dt;
        let spot = &hide.spots[self.spot];
        let floor = spot.center().z;
        let clips = &hide.clips;
        if self.phase == Phase::Enter && self.t >= self.clip(clips).length {
            self.phase = Phase::In;
            self.t = 0.0;
            // the wait loop in the entry's end frame
            self.frame *= if self.air { clips.air.root_motion(clips.air.length) } else { clips.enter.root_motion(clips.enter.length) };
        }
        if self.phase == Phase::In {
            if let Some(dir) = leave {
                let (_, _, at) = self.frame.to_scale_rotation_translation();
                self.frame = Mat4::from_rotation_translation(Quat::from_rotation_z(yaw_of(dir)), at);
                self.phase = Phase::Exit;
                self.t = 0.0;
            }
        }
        let clip = self.clip(clips);
        let ct = match self.phase {
            Phase::In => self.t % clip.length.max(1e-3),
            _ => self.t.min(clip.length),
        };
        let mut pose = clip.sample(sk, ct);
        let mut world = self.frame * clip.root_motion(ct);
        if self.phase == Phase::Enter && self.t < BLEND {
            let w = self.t / BLEND;
            let mut from = self.from_pose.clone();
            acb_anim::pose::blend(&mut from, &pose, w);
            pose = from;
            let (_, r0, t0) = self.from_world.to_scale_rotation_translation();
            let (_, r1, t1) = world.to_scale_rotation_translation();
            world = Mat4::from_rotation_translation(r0.slerp(r1, w), t0.lerp(t1, w));
        }
        // the logical position follows the root (these clips move it)
        let (_, r, at) = (self.frame * clip.root_motion(ct)).to_scale_rotation_translation();
        let (pos, yaw) = (at, r.to_euler(glam::EulerRot::ZXY).0);
        let out = self.phase == Phase::Exit && self.t >= clip.length;
        (pose, world, pos.with_z(floor), yaw, out)
    }

    fn clip<'a>(&self, c: &'a HideClips) -> &'a Clip {
        match self.phase {
            Phase::Enter if self.air => &c.air,
            Phase::Enter => &c.enter,
            Phase::In => &c.wait,
            Phase::Exit => &c.exit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot() -> Spot {
        Spot { entity: 1, frame: Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)), min: Vec3::new(-1.4, -0.9, 0.0), max: Vec3::new(1.4, 0.9, 1.6), kind: 0 }
    }

    #[test]
    fn entry_rules() {
        let s = [spot()];
        // running at it from 0.4 m outside the box edge (x 8.6)
        let p = Vec3::new(8.2, 0.0, 0.0);
        assert_eq!(entry(&s, p, Vec2::X, true, false, 0.0), Some(0));
        // walking (low profile), facing away, too far
        assert_eq!(entry(&s, p, Vec2::X, false, false, 0.0), None);
        assert_eq!(entry(&s, p, -Vec2::X, true, false, 0.0), None);
        assert_eq!(entry(&s, Vec3::new(7.9, 0.0, 0.0), Vec2::X, true, false, 0.0), None);
        // falling onto it: from 1.5 m above the top, not from higher up
        assert_eq!(entry(&s, Vec3::new(10.2, 0.3, 3.0), Vec2::ZERO, false, true, -3.0), Some(0));
        assert_eq!(entry(&s, Vec3::new(10.2, 0.3, 3.2), Vec2::ZERO, false, true, -3.0), None);
        assert_eq!(entry(&s, Vec3::new(10.2, 0.3, 3.0), Vec2::ZERO, false, true, 1.0), None);
    }
}
