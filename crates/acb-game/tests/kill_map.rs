//! Kill variants on San Donato with the retail clips. Needs the game data
//! (`ACBMP_SCHEMAS`, `ACBMP_MULTI`, `ACBMP_EXE`); skipped otherwise.

use std::path::PathBuf;

use acb_anim::BuiltinTracks;
use acb_game::kill::{self, KillClips};

struct Data {
    archive: acb_world::Archive<'static>,
    builtins: Option<BuiltinTracks>,
}

fn data() -> Option<Data> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let exe = PathBuf::from(std::env::var_os("ACBMP_EXE")?);
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).ok()?));
    let archive = acb_world::open_map(&multi.join("DataPC_AC2MP_SanDonato.forge"), schema).ok()?;
    let builtins = BuiltinTracks::from_executable(&std::fs::read(exe).ok()?);
    Some(Data { archive, builtins })
}

/// Every kind loads its own pair for ID07 (no fallback to the walking one).
#[test]
fn kill_sets_load() {
    let Some(d) = data() else { return };
    let k = KillClips::load(&d.archive, d.builtins.as_ref(), "AC2MP_ID07_MainTemplate_Player").unwrap();
    assert_eq!(k.sets.len(), 14);
    for (i, (a, v)) in k.sets.iter().enumerate() {
        let r = |c: &acb_anim::pose::Clip, t: f32| kill::reference(c, t).map(|(p, y)| format!("({:.2},{:.2},{:.2}) {:.0}", p.x, p.y, p.z, y.to_degrees()));
        eprintln!(
            "set {i}: ass {:.2}s {:?} -> {:?} | victim {:.2}s {:?} -> {:?}",
            a.length,
            r(a, 0.0),
            r(a, a.length),
            v.length,
            r(v, 0.0),
            r(v, v.length)
        );
    }
    for kind in [kill::RUN, kill::AIR, kill::DROP, kill::HAY_GRAB] {
        assert!(k.sets[kind].0.length != k.sets[kill::WALK].0.length, "kind {kind} fell back");
    }
}
