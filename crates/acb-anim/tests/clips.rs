//! Clip playback on retail animations: one-shots reach their last key,
//! locomotion strides loop without a jump. Needs the game data:
//! `ACBMP_SCHEMAS` (schema directory) and `ACBMP_MULTI` (the `multi`
//! directory with DataPC.forge; ACBMP.exe is looked up next to it unless
//! `ACBMP_EXE` is set); skipped otherwise.

use std::path::PathBuf;

use acb_anim::pose::{Clip, Skeleton};
use acb_anim::BuiltinTracks;
use acb_world::Archive;
use glam::{Quat, Vec3};

struct Data {
    archive: Archive<'static>,
    builtins: BuiltinTracks,
}

fn data() -> Option<Data> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let exe = std::env::var_os("ACBMP_EXE").map(PathBuf::from).unwrap_or_else(|| multi.join("../ACBMP.exe"));
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).ok()?));
    let archive = Archive::open(&multi.join("DataPC.forge"), schema).ok()?;
    let builtins = BuiltinTracks::from_executable(&std::fs::read(exe).ok()?)?;
    Some(Data { archive, builtins })
}

impl Data {
    fn clip(&self, name: &str) -> Clip {
        let a = &self.archive;
        let uid = a.uids_of("Animation").into_iter().find(|u| a.loc(*u).is_some_and(|l| l.name.eq_ignore_ascii_case(name)));
        Clip::load_with(a, uid.unwrap_or_else(|| panic!("{name} not found")), Some(&self.builtins)).unwrap()
    }

    fn skeleton(&self, clip: &Clip) -> Skeleton {
        let a = &self.archive;
        a.uids_of("Skeleton")
            .into_iter()
            .filter_map(|s| Skeleton::load(a, s))
            .max_by_key(|s| (clip.bone_coverage(s), s.bones.len()))
            .unwrap()
    }
}

/// Summed bone rotation difference, radians.
fn dist(a: &[(Vec3, Quat)], b: &[(Vec3, Quat)]) -> f32 {
    a.iter().zip(b).map(|((_, p), (_, q))| p.angle_between(*q)).sum()
}

const FRAME: f32 = 1.0 / 60.0;

#[test]
fn one_shot_plays_to_its_last_key() {
    let Some(d) = data() else {
        eprintln!("skipped: set ACBMP_SCHEMAS and ACBMP_MULTI");
        return;
    };
    // air assassination, 140 frames
    let clip = d.clip("ID07_Air2G_Front_Kill_ass");
    let sk = d.skeleton(&clip);
    assert!((clip.length - 140.0 / 60.0).abs() < 1e-4, "{}", clip.length);
    let last = clip.tracks.iter().map(|(_, t)| *t.times.last().unwrap()).max().unwrap();
    assert_eq!(last, 140, "key times span the whole clip");

    // at the end every bone sits on its track's last key
    let end = clip.sample(&sk, clip.length);
    for (id, t) in &clip.tracks {
        let Some(bi) = sk.bones.iter().position(|b| b.id == *id) else { continue };
        if let Some(q) = t.quats() {
            assert!(end[bi].1.angle_between(*q.last().unwrap()) < 1e-3, "track {id:#x} not at its last key");
        }
    }
    // the pose moves through the clip and settles at the end, no reset
    let poses: Vec<_> = (0..=140).map(|f| clip.sample(&sk, f as f32 * FRAME)).collect();
    let steps: Vec<f32> = poses.windows(2).map(|w| dist(&w[0], &w[1])).collect();
    let typical = steps.iter().sum::<f32>() / steps.len() as f32;
    let worst = steps.iter().copied().fold(0.0, f32::max);
    assert!(worst < 12.0 * typical, "a frame jumps {worst} (typical {typical})");
    assert!(dist(&poses[139], &poses[140]) < dist(&poses[0], &poses[140]) * 0.1);
    // a one-shot is not looped: past the end it holds the last frame
    assert!(dist(&clip.sample(&sk, clip.length + 0.5), &end) < 1e-4);
}

#[test]
fn strides_join_into_seamless_cycles() {
    let Some(d) = data() else {
        eprintln!("skipped: set ACBMP_SCHEMAS and ACBMP_MULTI");
        return;
    };
    // (cycle, stride length in metres)
    for (name, stride) in [("xx_l_walk_hipm", 1.897), ("xx_h_run_hipm", 3.414), ("xx_h_sprint_hipm", 3.348)] {
        let (l, r) = (d.clip(&format!("{name}_footl")), d.clip(&format!("{name}_footr")));
        let sk = d.skeleton(&l);
        // each half continues into the other, looping one alone jumps
        assert!(l.seam(&r) + r.seam(&l) < 0.25 * l.seam(&l), "{name}: halves do not join");

        let c = l.then(&r);
        assert!((c.length - l.length - r.length).abs() < 1e-6);
        let moved = c.root_motion(c.length).w_axis.truncate();
        assert!((moved.y - stride).abs() < 0.005 && moved.x.abs() < 0.005, "{name}: stride {moved}");
        // the first half is untouched
        for t in [0.0, l.length * 0.5, l.length - FRAME] {
            assert!(dist(&c.sample(&sk, t), &l.sample(&sk, t)) < 1e-3, "{name}: first half differs at {t}");
            assert!((c.root_motion(t).w_axis - l.root_motion(t).w_axis).length() < 1e-4);
        }
        // the second half is the right foot's clip, carried forward (on the
        // bones both halves drive; one the right half lacks holds the left
        // half's last key rather than snapping to the bind pose)
        let both: Vec<usize> = (0..sk.bones.len()).filter(|&i| l.drives(sk.bones[i].id) && r.drives(sk.bones[i].id)).collect();
        let on_both = |p: Vec<(Vec3, Quat)>| both.iter().map(|&i| p[i]).collect::<Vec<_>>();
        for t in [0.25 * FRAME, r.length * 0.5, r.length] {
            let e = dist(&on_both(c.sample(&sk, l.length + t)), &on_both(r.sample(&sk, t)));
            assert!(e < 2e-3, "{name}: second half differs at {t} by {e}");
            let want = l.root_motion(l.length).w_axis.truncate() + r.root_motion(t).w_axis.truncate();
            assert!((c.root_motion(l.length + t).w_axis.truncate() - want).length() < 1e-3);
        }

        // frame-to-frame motion stays smooth through the seam and the wrap
        let n = (c.length * 60.0).round() as usize;
        let at = |f: usize| c.sample(&sk, (f % n) as f32 * FRAME);
        let steps: Vec<f32> = (0..n).map(|f| dist(&at(f), &at(f + 1))).collect();
        let typical = steps.iter().sum::<f32>() / n as f32;
        let wrap = steps[n - 1];
        let seam = steps[(l.length * 60.0).round() as usize - 1];
        let half_loop = dist(&l.sample(&sk, l.length), &l.sample(&sk, 0.0));
        println!(
            "{name}: {:.3} s, stride {:.3} m; per-frame change typical {typical:.2}, seam {seam:.2}, wrap {wrap:.2}; footl looped alone jumps {half_loop:.2}",
            c.length, moved.y
        );
        assert!(wrap < 2.5 * typical, "{name}: wrap jumps {wrap} (typical {typical})");
        assert!(seam < 2.5 * typical, "{name}: seam jumps {seam} (typical {typical})");
        // looping the left half alone is the reported reset
        assert!(half_loop > 2.5 * typical, "{name}");

        // root motion over the wrap: two frames' worth, not a jump back
        let step = c.root_step(c.length - FRAME, FRAME);
        let speed = stride / c.length;
        assert!((step.y - 2.0 * FRAME * speed).abs() < 0.03, "{name}: wrap step {step}");
    }
}
