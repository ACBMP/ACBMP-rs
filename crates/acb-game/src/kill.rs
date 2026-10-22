//! Ground kills: a persona's paired `IDxx_G2G_{Front,Back}_Walk2All_Kill_ass`
//! (assassin) and `..._Killed_Npc` (victim) clips. Both are authored in one
//! frame, the victim's: at t = 0 the victim's Reference (pelvis) is at the
//! origin and the assassin's about a metre away. Placing that frame at the
//! victim, turned so the assassin's start pelvis points at the player, and
//! playing both clips in it reproduces the kill; the root tracks barely
//! move, the Reference bones carry the motion.

use acb_anim::pose::Clip;
use acb_anim::BuiltinTracks;
use acb_world::Archive;
use glam::{Mat4, Quat, Vec3};

/// Skeleton id of the `Reference` bone (pelvis in entity space).
const REFERENCE: u32 = 0x2c52_cbb0;

pub struct KillClips {
    /// (assassin, victim) pairs, front then back for each kind: see
    /// [`WALK`], [`STUN`], [`RUN`], [`AIR`], [`DROP`], [`HAY_GRAB`],
    /// [`TO_HAY`]. A kind the persona lacks falls back to the walking pair.
    pub sets: Vec<(Clip, Clip)>,
}

/// `{id}_G2G_{Front,Back}_Walk2All_Kill_ass` / `_Killed_Npc`.
pub const WALK: usize = 0;
/// `IDXX_G2G_Humil_{Front,Back}_Win_{ass,npc}`.
pub const STUN: usize = 2;
/// Running kill: `{id}_G2G_{Front,Back}_Run2All_Kill_ass` / `_Killed_Npc`.
pub const RUN: usize = 4;
/// Air kill (falling/jumping onto the victim):
/// `{id}_Air2G_{Front,Back}_Kill_ass` / `_Killed_npc`.
pub const AIR: usize = 6;
/// Drop kill (letting go of a hold above the victim):
/// `{id}_Drop2G_{Front,Back}_Kill_ass` / `_Killed_npc`.
pub const DROP: usize = 8;
/// From a hiding spot onto a passer-by:
/// `{id}_Hayground2Walk_grab_{front,back}_kill_ass` / `_killed_npc`.
pub const HAY_GRAB: usize = 10;
/// Into a hiding spot: `{id}_Walk2Hayground_Kill_ass` (front and back
/// alike) / `*_Walk2HayGround_Kill_Out_npc`.
pub const TO_HAY: usize = 12;

/// Victim offset of a stun: `IDXX_G2G_Humil_*_Win_{ass,npc}` are authored
/// each around its own origin (assassin facing +Y, the victim's frame
/// turned the same way), so the victim's frame sits ahead of the
/// assassin's. The engine's spacing (from its positionf step) is unknown;
/// this matches the kills' ~1 m pelvis spacing.
pub const STUN_SPACING: f32 = 0.9;

/// Reference bone translation and yaw of a clip at `t` (entity space).
pub fn reference(clip: &Clip, t: f32) -> Option<(Vec3, f32)> {
    let frame = t * 60.0;
    let key = |times: &[u32]| times.iter().rposition(|&k| k as f32 <= frame).unwrap_or(0);
    let mut pos = None;
    let mut yaw = None;
    for (id, tr) in clip.tracks.iter().filter(|(id, _)| *id == REFERENCE) {
        let _ = id;
        if let Some(v) = tr.vec3s() {
            pos = Some(v[key(&tr.times).min(v.len() - 1)]);
        } else if let Some(q) = tr.quats() {
            yaw = Some(q[key(&tr.times).min(q.len() - 1)].to_euler(glam::EulerRot::ZXY).0);
        }
    }
    Some((pos?, yaw?))
}

impl KillClips {
    /// The kill set of the persona behind builder `builder_name`
    /// (`AC2MP_ID07_MainTemplate_Player` -> `ID07_...`).
    pub fn load(archive: &Archive, builtins: Option<&BuiltinTracks>, builder_name: &str) -> Option<KillClips> {
        let id = builder_name.to_ascii_uppercase().strip_prefix("AC2MP_")?.split('_').next()?.to_string();
        let uids = archive.uids_of("Animation");
        let load = |name: String| {
            uids.iter()
                .copied()
                .find(|u| archive.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(&name)))
                .and_then(|u| Clip::load_with(archive, u, builtins))
        };
        let pair = |ass: String, victim: String| Some((load(ass)?, load(victim)?));
        let set = |side: &str| pair(format!("{id}_G2G_{side}_Walk2All_Kill_ass"), format!("{id}_G2G_{side}_Walk2All_Killed_Npc"));
        let stun = |side: &str| pair(format!("IDXX_G2G_Humil_{side}_Win_ass"), format!("IDXX_G2G_Humil_{side}_Win_npc"));
        let mut sets = vec![set("Front")?, set("Back")?, stun("Front")?, stun("Back")?];
        // the other kinds, falling back to the walking pair of that side
        let hay_victim = || {
            uids.iter()
                .copied()
                .find(|u| archive.loc(*u).is_some_and(|l| l.name.to_ascii_lowercase().ends_with("_walk2hayground_kill_out_npc")))
                .and_then(|u| Clip::load_with(archive, u, builtins))
        };
        for (k, side) in ["Front", "Back"].into_iter().enumerate().cycle().take(10) {
            let kind = sets.len() / 2;
            let lower = side.to_ascii_lowercase();
            let got = match kind {
                2 => pair(format!("{id}_G2G_{side}_Run2All_Kill_ass"), format!("{id}_G2G_{side}_Run2All_Killed_Npc")),
                3 => pair(format!("{id}_Air2G_{side}_Kill_ass"), format!("{id}_Air2G_{side}_Killed_npc")),
                4 => pair(format!("{id}_Drop2G_{side}_Kill_ass"), format!("{id}_Drop2G_{side}_Killed_npc")),
                5 => pair(format!("{id}_Hayground2Walk_grab_{lower}_kill_ass"), format!("{id}_Hayground2Walk_grab_{lower}_killed_npc")),
                _ => load(format!("{id}_Walk2Hayground_Kill_ass")).zip(hay_victim()),
            };
            let got = match got {
                Some(g) => g,
                None => sets[k].clone(),
            };
            sets.push(got);
        }
        Some(KillClips { sets })
    }

    /// Pelvis spacing of the ground kills at their start (assassin
    /// Reference at t = 0, horizontal): what the walk-in leaves to close.
    pub fn spacing(&self) -> f32 {
        let d = |set: usize| reference(&self.sets[set].0, 0.0).map(|(p, _)| p.truncate().length());
        match (d(0), d(1)) {
            (Some(a), Some(b)) => a.max(b),
            _ => crate::rules::PAIR_SPACING,
        }
    }

    /// Where the assassin's entity stands (position, yaw) at the start of
    /// set `side` played in `frame`, for a neutral stance taken from
    /// `idle`: the walk-in's goal.
    pub fn start_point(&self, side: usize, frame: Mat4, idle: &Clip) -> (Vec3, f32) {
        let neutral = reference(idle, 0.0).unwrap_or((Vec3::Z * 0.96, 66f32.to_radians()));
        match reference(&self.sets[side].0, 0.0) {
            Some((rp, ry)) => reroot(frame, rp, ry, neutral),
            None => (frame.w_axis.truncate(), 0.0),
        }
    }

    /// The set for an assassin at `from` attacking a victim at `victim`
    /// facing `victim_yaw` (front when the assassin is ahead of the victim),
    /// and the shared frame to play it in.
    pub fn plan(&self, from: Vec3, victim: Vec3, victim_yaw: f32) -> Option<(usize, Mat4)> {
        let side = side(from, victim, victim_yaw);
        let to_ass = (from - victim).truncate();
        let (ass, _) = &self.sets[side];
        let (start, _) = reference(ass, 0.0)?;
        // turn the frame so the assassin's start pelvis points at `from`
        let local = start.y.atan2(start.x);
        let world = to_ass.y.atan2(to_ass.x);
        Some((side, Mat4::from_rotation_translation(Quat::from_rotation_z(world - local), victim)))
    }
}

/// 0 when the assassin at `from` is in front of a victim at `victim`
/// facing `victim_yaw`, 1 when behind ([`crate::rules::side`]).
pub fn side(from: Vec3, victim: Vec3, victim_yaw: f32) -> usize {
    usize::from(crate::rules::side(from, victim, victim_yaw) == crate::rules::Side::Back)
}

/// Where to put an entity so that a pose whose Reference is at `pos`/`yaw`
/// (in `frame`) looks like a neutral stance (Reference at `neutral`): the
/// hand-off from a kill clip back to locomotion. Returns position and
/// facing yaw.
pub fn reroot(frame: Mat4, pos: Vec3, yaw: f32, neutral: (Vec3, f32)) -> (Vec3, f32) {
    let turn = yaw - neutral.1;
    let entity_local = pos - Quat::from_rotation_z(turn) * neutral.0;
    let (_, rot, _) = frame.to_scale_rotation_translation();
    let frame_yaw = rot.to_euler(glam::EulerRot::ZXY).0;
    (frame.transform_point3(entity_local.with_z(0.0)), frame_yaw + turn)
}
