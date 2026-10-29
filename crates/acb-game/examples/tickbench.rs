//! Milliseconds per sim tick for one player by activity: `tickbench
//! MAP.forge` (env ACBMP_SCHEMAS, ACBMP_EXE).
use std::sync::Arc;
use std::time::Instant;

use acb_anim::BuiltinTracks;
use acb_game::{PlayerInput, Setup, World};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = Box::leak(Box::new(acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?));
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), schema)?;
    let builtins = BuiltinTracks::from_executable(&std::fs::read(std::env::var("ACBMP_EXE")?)?);
    let name = "AC2MP_ID07_MainTemplate_Player";
    let character = Arc::new(acb_game::load_character(&archive, name)?);
    let setup = Setup {
        characters: vec![(name.into(), character)],
        count: Some(1),
        anim: String::new(),
        wander: 0.0,
        play: true,
        crowd: 0,
        pair: Vec::new(),
        at: None,
        builtins: builtins.as_ref(),
    };
    let mut w = World::load(&archive, &setup)?;
    let base = w.save();
    let runs: [(&str, [f32; 4], PlayerInput); 5] = [
        ("climb", [-18.0, 44.6, 2.22, -97.0], PlayerInput { forward: true, run: true, cam_yaw: (-7f32).to_radians(), ..PlayerInput::default() }),
        ("idle", [40.0, 20.4, 0.1, -90.0], PlayerInput::default()),
        ("walk", [31.0, 20.4, 0.1, -90.0], PlayerInput { forward: true, ..PlayerInput::default() }),
        ("sprint", [31.0, 20.4, 0.1, -90.0], PlayerInput { forward: true, run: true, sprint: true, ..PlayerInput::default() }),
        ("fall", [31.0, 20.4, 30.0, -90.0], PlayerInput::default()),
    ];
    for (what, at, input) in runs {
        w.restore(&base);
        w.place_player(0, glam::Vec3::new(at[0], at[1], at[2]), at[3].to_radians());
        if what == "fall" {
            // place_player snaps to the floor: put it back up in the air
            w.players[0].pos.z = at[2];
            w.players[0].loco.start_fall();
        }
        let n = 120;
        let t = Instant::now();
        for _ in 0..n {
            w.step(&[input]);
        }
        println!("{what:7} {:.2} ms/tick, at {:.2?} climbing {}", t.elapsed().as_secs_f64() * 1000.0 / n as f64, w.players[0].pos, w.players[0].climb.is_some());
    }
    Ok(())
}
