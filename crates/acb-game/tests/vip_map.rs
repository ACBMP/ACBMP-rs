//! Escort VIP ways on San Donato with the retail data. Needs the game data
//! (`ACBMP_SCHEMAS`, `ACBMP_MULTI`); skipped otherwise.

use std::path::PathBuf;

use acb_game::crowd::Nav;
use acb_game::vip::{self, Walker};

fn archive() -> Option<acb_world::Archive<'static>> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let multi = PathBuf::from(std::env::var_os("ACBMP_MULTI")?);
    let schema = Box::leak(Box::new(acb_world::load_schema(&schemas).ok()?));
    acb_world::open_map(&multi.join("DataPC_AC2MP_SanDonato.forge"), schema).ok()
}

#[test]
fn san_donato_ways_walkable_far_from_hunters() {
    let Some(a) = archive() else {
        eprintln!("skipped: no game data");
        return;
    };
    let nav = Nav::load(&a).unwrap();
    let spawns = vip::team_spawns(&a);
    assert_eq!(spawns.len(), 16, "4 team spawn groups of 4");
    let avoid: Vec<_> = spawns.iter().map(|s| s.1).collect();
    let ways = vip::ways_for(&nav, &avoid);
    assert!(ways.len() >= vip::NB_VIPS, "{} ways", ways.len());
    assert_eq!(ways, vip::ways_for(&nav, &avoid), "deterministic");
    let pace = 1.4;
    let dt = 1.0 / 60.0;
    for (i, w) in ways.iter().enumerate() {
        let near = avoid.iter().map(|p| p.distance(w.start())).fold(f32::INFINITY, f32::min);
        assert!(near >= vip::MIN_DIST_FROM_KILLERS, "way {i} starts {near} m from a team spawn");
        assert!(w.checkpoints.len() >= 3, "way {i}: {} checkpoints", w.checkpoints.len());
        assert!(w.length() >= vip::MIN_LENGTH, "way {i}: {} m", w.length());
        // no navmesh gaps along the polyline
        for (j, s) in w.points.windows(2).enumerate() {
            assert!(vip::clear(&nav, s[0], s[1]), "way {i} segment {j} leaves the navmesh");
        }
        // a crowd-kinematics walker arrives in time, passing every checkpoint
        let bound = w.length() / pace * 1.25 + 5.0;
        let mut walker = Walker::new(w);
        let (mut t, mut passed) = (0.0, 0);
        while !walker.arrived(w) && t < bound {
            if walker.step(&nav, w, pace, dt).is_some_and(|k| w.is_checkpoint(k)) {
                passed += 1;
            }
            assert!(nav.locate(walker.pos + glam::Vec3::Z * 0.5).is_some(), "way {i}: walker off the navmesh at {:?}", walker.pos);
            t += dt;
        }
        assert!(walker.arrived(w), "way {i}: not arrived after {t:.1} s (bound {bound:.1}), at {:?}", walker.pos);
        assert_eq!(passed, w.checkpoints.len(), "way {i}");
        eprintln!("way {i}: {:.0} m, {} checkpoints, walked in {t:.1} s (bound {bound:.1}), start {near:.1} m from team spawns", w.length(), w.checkpoints.len());
    }
}
