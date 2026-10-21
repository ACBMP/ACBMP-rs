//! Skeletons, sampling and forward kinematics.

use std::collections::HashMap;

use acb_world::Archive;
use anvil_fastload::{Obj, Value};
use glam::{Mat4, Quat, Vec3};

use crate::{parse_tracks, BuiltinTracks, RawTrack, Track, ValueKind};

/// Frames per second of key times (`max key / Length` is 60 for every
/// retail animation).
pub const FRAME_RATE: f32 = 60.0;

#[derive(Debug, Clone)]
pub struct Bone {
    pub id: u32,
    pub parent: Option<usize>,
    pub local_pos: Vec3,
    pub local_rot: Quat,
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub uid: u32,
    pub bones: Vec<Bone>,
}

fn vec4(v: Option<&Value>) -> Option<[f32; 4]> {
    v.and_then(Value::as_f32s).and_then(|f| f.get(..4).map(|s| [s[0], s[1], s[2], s[3]]))
}

impl Skeleton {
    pub fn load(archive: &Archive, uid: u32) -> Option<Skeleton> {
        let root = archive.root(uid).ok()?;
        let objs: Vec<&Obj> = archive.field(&root.obj, "Bones")?.items().iter().filter_map(Value::as_obj).collect();
        let index: HashMap<u32, usize> = objs.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
        let bones = objs
            .iter()
            .map(|b| {
                let p = vec4(archive.field(b, "LocalPosition")).unwrap_or([0.0; 4]);
                let r = vec4(archive.field(b, "LocalRotation")).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                Bone {
                    id: archive.field(b, "BoneID").and_then(Value::as_u32).unwrap_or(0),
                    parent: archive.field(b, "FatherBone").and_then(Value::target_id).and_then(|f| index.get(&f).copied()),
                    local_pos: Vec3::new(p[0], p[1], p[2]),
                    local_rot: Quat::from_array(r).normalize(),
                }
            })
            .collect();
        Some(Skeleton { uid, bones })
    }

    /// World matrices of every bone for the given local transforms.
    pub fn globals(&self, local: &[(Vec3, Quat)]) -> Vec<Mat4> {
        let mut out: Vec<Option<Mat4>> = vec![None; self.bones.len()];
        fn resolve(i: usize, s: &Skeleton, local: &[(Vec3, Quat)], out: &mut Vec<Option<Mat4>>, depth: u32) -> Mat4 {
            if let Some(m) = out[i] {
                return m;
            }
            let (p, r) = local[i];
            let m = Mat4::from_rotation_translation(r, p);
            let g = match s.bones[i].parent {
                Some(pi) if depth < 256 && pi != i => resolve(pi, s, local, out, depth + 1) * m,
                _ => m,
            };
            out[i] = Some(g);
            g
        }
        (0..self.bones.len()).map(|i| resolve(i, self, local, &mut out, 0)).collect()
    }

    pub fn bind_pose(&self) -> Vec<(Vec3, Quat)> {
        self.bones.iter().map(|b| (b.local_pos, b.local_rot)).collect()
    }
}

/// A decoded animation: tracks paired with their TrackIDs.
#[derive(Debug, Clone)]
pub struct Clip {
    pub uid: u32,
    pub length: f32,
    pub tracks: Vec<(u32, RawTrack)>,
}

impl Clip {
    pub fn load(archive: &Archive, uid: u32) -> Option<Clip> {
        Self::load_with(archive, uid, None)
    }

    /// Like [`Clip::load`], resolving builtin tracks through `builtins`
    /// (without it they are skipped, which drops e.g. root motion).
    pub fn load_with(archive: &Archive, uid: u32, builtins: Option<&BuiltinTracks>) -> Option<Clip> {
        let root = archive.root(uid).ok()?;
        let tracks = parse_tracks(root.obj.custom.as_ref()?).ok()?;
        let ids: Vec<u32> = archive
            .field(&root.obj, "AnimTrackData")
            .and_then(Value::as_obj)
            .and_then(|d| archive.field(d, "AnimTrackDataMapping"))
            .map(|v| {
                v.items()
                    .iter()
                    .filter_map(Value::as_obj)
                    .map(|m| archive.field(m, "TrackID").and_then(Value::as_u32).unwrap_or(0))
                    .collect()
            })
            .unwrap_or_default();
        let length = archive.field(&root.obj, "Length").and_then(Value::as_f32s).and_then(|v| v.first().copied()).unwrap_or(0.0);
        let tracks = ids
            .into_iter()
            .zip(tracks)
            .filter_map(|(id, t)| match t {
                Track::Data(d) => Some((id, d)),
                Track::Builtin(b) => Some((id, builtins?.track(b).ok()?)),
            })
            .collect();
        Some(Clip { uid, length, tracks })
    }

    /// Local bone transforms at `seconds`, starting from the bind pose.
    pub fn sample(&self, skeleton: &Skeleton, seconds: f32) -> Vec<(Vec3, Quat)> {
        let frame = seconds * FRAME_RATE;
        let index: HashMap<u32, usize> = skeleton.bones.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
        let mut pose = skeleton.bind_pose();
        for (id, t) in &self.tracks {
            let Some(&bi) = index.get(id) else { continue };
            let (k0, k1, f) = key_span(&t.times, frame);
            match t.compression.kind() {
                ValueKind::Quat => {
                    if let Some(q) = t.quats() {
                        let (a, b) = (q[k0], q[k1]);
                        pose[bi].1 = a.slerp(b, f).normalize();
                    }
                }
                ValueKind::Vec3 => {
                    if let Some(v) = t.vec3s() {
                        pose[bi].0 = v[k0].lerp(v[k1], f);
                    }
                }
                _ => {}
            }
        }
        pose
    }

    /// Root motion: the entity's displacement from the clip's start at
    /// `seconds`, in entity space (TrackID 0's vector and quaternion tracks,
    /// usually builtin; locomotion cycles move along +Y).
    pub fn root_motion(&self, seconds: f32) -> Mat4 {
        let frame = seconds * FRAME_RATE;
        let (mut pos, mut rot) = (Vec3::ZERO, Quat::IDENTITY);
        for (_, t) in self.tracks.iter().filter(|(id, _)| *id == 0) {
            let (k0, k1, f) = key_span(&t.times, frame);
            if let Some(v) = t.vec3s() {
                pos = v[k0].lerp(v[k1], f);
            } else if let Some(q) = t.quats() {
                rot = q[k0].slerp(q[k1], f).normalize();
            }
        }
        Mat4::from_rotation_translation(rot, pos)
    }

    /// Root motion between two clip times, wrapping when `to < from`.
    pub fn root_delta(&self, from: f32, to: f32) -> Mat4 {
        let m = |t| self.root_motion(t);
        if to >= from {
            m(from).inverse() * m(to)
        } else {
            m(from).inverse() * m(self.length) * m(0.0).inverse() * m(to)
        }
    }

    /// Translation of [`Clip::root_delta`], cheaper to blend.
    pub fn root_step(&self, from: f32, to: f32) -> Vec3 {
        self.root_delta(from, to).w_axis.truncate()
    }

    /// This clip followed by `next`, as one clip of the summed length.
    ///
    /// Locomotion cycles are stored as half strides: `*_hipm_footl` ends in
    /// exactly the first frame of `*_hipm_footr` and vice versa, so a full
    /// loopable stride is `footl.then(&footr)`; looping one half alone
    /// snaps back half way through the stride. `next`'s keys are shifted by
    /// this clip's length (a track whose keys end early holds its last
    /// value up to the seam), and its root motion (TrackID 0) continues
    /// from where this clip's ends. Rotation and translation keys are
    /// stored uncompressed; scalar tracks are kept when both halves use the
    /// same compression. A track only one half has holds its nearest key
    /// over the other half.
    pub fn then(&self, next: &Clip) -> Clip {
        let seam = (self.length * FRAME_RATE).round() as u32;
        // next's root continues from this clip's end: A * M_next(t)
        let carry = self.root_motion(self.length) * next.root_motion(0.0).inverse();
        let (_, carry_rot, carry_pos) = carry.to_scale_rotation_translation();
        let mut tracks = Vec::new();
        let mut used = vec![false; next.tracks.len()];
        for (id, a) in &self.tracks {
            let kind = a.compression.kind();
            let partner = next.tracks.iter().enumerate().find(|(i, (nid, b))| !used[*i] && nid == id && b.compression.kind() == kind);
            let Some((i, (_, b))) = partner else {
                tracks.push((*id, a.clone()));
                continue;
            };
            used[i] = true;
            let root = *id == 0;
            let joined = match kind {
                ValueKind::Quat => {
                    let (qa, qb) = (a.quats().unwrap_or_default(), b.quats().unwrap_or_default());
                    let qb: Vec<Quat> = if root { qb.iter().map(|q| (carry_rot * *q).normalize()).collect() } else { qb };
                    let (times, keys) = join_keys(&a.times, &qa, seam, &b.times, &qb);
                    Some(RawTrack::quat_none(times, &keys))
                }
                ValueKind::Vec3 => {
                    let (va, vb) = (a.vec3s().unwrap_or_default(), b.vec3s().unwrap_or_default());
                    let vb: Vec<Vec3> = if root { vb.iter().map(|v| carry_rot * *v + carry_pos).collect() } else { vb };
                    let (times, keys) = join_keys(&a.times, &va, seam, &b.times, &vb);
                    Some(RawTrack::vec3_none(times, &keys))
                }
                _ if a.compression == b.compression => {
                    let size = a.compression.value_size();
                    let ka: Vec<&[u8]> = a.values.chunks_exact(size).collect();
                    let kb: Vec<&[u8]> = b.values.chunks_exact(size).collect();
                    let (times, keys) = join_keys(&a.times, &ka, seam, &b.times, &kb);
                    Some(RawTrack { times, values: keys.concat(), ..a.clone() })
                }
                _ => None,
            };
            if let Some(t) = joined {
                tracks.push((*id, t));
            }
        }
        for (i, (id, b)) in next.tracks.iter().enumerate() {
            if !used[i] {
                let mut b = b.clone();
                b.times = b.times.iter().map(|t| t + seam).collect();
                tracks.push((*id, b));
            }
        }
        Clip { uid: self.uid, length: self.length + next.length, tracks }
    }

    /// For looping: `self.then(other)` when the two are the halves of one
    /// stride (each continues into the other far better than `self` loops
    /// into itself), else `self` unchanged.
    pub fn looped_with(self, other: &Clip) -> Clip {
        if self.seam(other) + other.seam(&self) < 0.25 * self.seam(&self) {
            self.then(other)
        } else {
            self
        }
    }

    /// How far this clip's last frame is from `next`'s first: summed angle
    /// (radians) between their rotation keys, over the non-root TrackIDs
    /// both animate. Near zero when `next` continues this clip (the two
    /// halves of a stride); `self.seam(self)` is the jump of looping it.
    pub fn seam(&self, next: &Clip) -> f32 {
        let end = |c: &Clip, last: bool| -> HashMap<u32, Quat> {
            c.tracks
                .iter()
                .filter(|(id, _)| *id != 0)
                .filter_map(|(id, t)| t.quats().and_then(|q| if last { q.last().copied() } else { q.first().copied() }).map(|q| (*id, q)))
                .collect()
        };
        let (a, b) = (end(self, true), end(next, false));
        a.iter().filter_map(|(id, q)| b.get(id).map(|p| q.angle_between(*p))).sum()
    }

    /// Whether some track animates bone `id`.
    pub fn drives(&self, id: u32) -> bool {
        self.tracks.iter().any(|(t, _)| *t == id)
    }

    /// TrackIDs that are bones of `skeleton`.
    pub fn bone_coverage(&self, skeleton: &Skeleton) -> usize {
        let ids: std::collections::HashSet<u32> = skeleton.bones.iter().map(|b| b.id).collect();
        self.tracks.iter().filter(|(id, _)| ids.contains(id)).count()
    }
}

/// Name of the other half of a stride: a trailing `_footl` <-> `_footr`
/// (`xx_h_run_hipm_footl` -> `xx_h_run_hipm_footr`).
pub fn stride_partner(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let (stem, other) = match () {
        _ if lower.ends_with("_footl") => (&name[..name.len() - 1], "r"),
        _ if lower.ends_with("_footr") => (&name[..name.len() - 1], "l"),
        _ => return None,
    };
    Some(format!("{stem}{other}"))
}

/// Keys of `a` then those of `b` shifted by `seam` frames; `a`'s last
/// value is held up to the seam. Both halves keep their key at the seam,
/// so the frame at the seam itself samples `b`'s first key (as playing the
/// clips back to back would).
fn join_keys<T: Copy>(ta: &[u32], ka: &[T], seam: u32, tb: &[u32], kb: &[T]) -> (Vec<u32>, Vec<T>) {
    let (mut times, mut keys) = (ta.to_vec(), ka.to_vec());
    if let (Some(&t), Some(&k)) = (times.last(), keys.last()) {
        if t < seam {
            times.push(seam);
            keys.push(k);
        }
    }
    times.extend(tb.iter().map(|t| t + seam));
    keys.extend_from_slice(kb);
    (times, keys)
}

/// Keys around `frame` and the blend factor between them.
fn key_span(times: &[u32], frame: f32) -> (usize, usize, f32) {
    if times.len() < 2 || frame <= times[0] as f32 {
        return (0, 0, 0.0);
    }
    let last = times.len() - 1;
    if frame >= times[last] as f32 {
        return (last, last, 0.0);
    }
    let k = times.partition_point(|&t| t as f32 <= frame) - 1;
    let (a, b) = (times[k] as f32, times[k + 1] as f32);
    (k, k + 1, if b > a { (frame - a) / (b - a) } else { 0.0 })
}

/// Blends `other` into `pose` by weight `w` (0 keeps `pose`): lerped
/// translations, shortest-path normalized-lerped rotations.
pub fn blend(pose: &mut [(Vec3, Quat)], other: &[(Vec3, Quat)], w: f32) {
    for ((t, q), (ot, oq)) in pose.iter_mut().zip(other) {
        *t = t.lerp(*ot, w);
        let oq = if q.dot(*oq) < 0.0 { -*oq } else { *oq };
        *q = q.lerp(oq, w).normalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 20-frame half stride: root 1 m along +Y, bone 7 swinging 0 -> 1 rad.
    fn half(swing: (f32, f32)) -> Clip {
        let root = RawTrack::vec3_none(vec![0, 20], &[Vec3::ZERO, Vec3::Y]);
        let bone = RawTrack::quat_none(vec![0, 10], &[Quat::from_rotation_x(swing.0), Quat::from_rotation_x(swing.1)]);
        Clip { uid: 1, length: 20.0 / FRAME_RATE, tracks: vec![(0, root), (7, bone)] }
    }

    #[test]
    fn halves_join_into_one_stride() {
        let (l, r) = (half((0.0, 1.0)), half((1.0, 0.0)));
        assert!(l.seam(&r) < 1e-6 && r.seam(&l) < 1e-6 && l.seam(&l) > 0.9);
        let c = l.clone().looped_with(&r);
        assert!((c.length - 40.0 / FRAME_RATE).abs() < 1e-6);
        // root continues: 2 m over the stride, 1.5 m half way through the right foot
        assert!((c.root_motion(c.length).w_axis.truncate() - Vec3::Y * 2.0).length() < 1e-5);
        assert!((c.root_motion(30.0 / FRAME_RATE).w_axis.truncate() - Vec3::Y * 1.5).length() < 1e-5);
        // the swing holds its last key up to the seam, then plays the right half
        let sk = Skeleton { uid: 0, bones: vec![Bone { id: 7, parent: None, local_pos: Vec3::ZERO, local_rot: Quat::IDENTITY }] };
        let angle = |t: f32| c.sample(&sk, t)[0].1.angle_between(Quat::IDENTITY);
        assert!((angle(15.0 / FRAME_RATE) - 1.0).abs() < 1e-4);
        assert!((angle(25.0 / FRAME_RATE) - 0.5).abs() < 1e-3);
        assert!(angle(c.length) < 1e-3);
        // wrapping root delta: one frame either side of the loop point
        let step = c.root_step(c.length - 1.0 / FRAME_RATE, 1.0 / FRAME_RATE);
        assert!((step.y - 0.1).abs() < 1e-4, "{step}");
        // not halves of one stride: left alone
        assert!((l.clone().looped_with(&l).length - l.length).abs() < 1e-6);
        assert_eq!(stride_partner("xx_h_run_hipm_footl").as_deref(), Some("xx_h_run_hipm_footr"));
        assert_eq!(stride_partner("xx_h_run_hipm_footr").as_deref(), Some("xx_h_run_hipm_footl"));
        assert_eq!(stride_partner("xx_h_runstop_footl_tr_h_wait"), None);
    }
}
