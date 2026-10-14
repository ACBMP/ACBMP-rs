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
