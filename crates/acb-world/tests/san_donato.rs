//! Regression numbers on the retail San Donato map. Needs the game data:
//! `ACBMP_SCHEMAS` (schema directory) and `ACBMP_MULTI` (the `multi`
//! directory with the forges); skipped otherwise.

use std::path::PathBuf;

use acb_world::{load_schema, open_map};

fn data() -> Option<(PathBuf, PathBuf)> {
    let schemas = PathBuf::from(std::env::var_os("ACBMP_SCHEMAS")?);
    let map = PathBuf::from(std::env::var_os("ACBMP_MULTI")?).join("DataPC_AC2MP_SanDonato.forge");
    map.exists().then_some((schemas, map))
}

#[test]
fn san_donato() {
    let Some((schemas, map)) = data() else {
        eprintln!("skipped: set ACBMP_SCHEMAS and ACBMP_MULTI");
        return;
    };
    let schema = load_schema(&schemas).unwrap();
    let a = open_map(&map, &schema).unwrap();

    let colliders = a.colliders();
    assert_eq!(colliders.len(), 998);
    let (world, skipped) = a.world_collision();
    assert_eq!(world.triangles.len(), 55916);
    assert_eq!(skipped, vec![("SphereShape".to_string(), 7)]);

    let edges = a.guidance_edges();
    assert_eq!(edges.len(), 4075);
    let length: f32 = edges.iter().map(|e| (0..3).map(|i| (e.a[i] - e.b[i]).powi(2)).sum::<f32>().sqrt()).sum();
    assert!((length - 6438.0).abs() < 5.0, "{length}");

    let g = a.nav_graph();
    assert_eq!(g.triangles.len(), 8321);
    assert_eq!(a.crowd_flows().len(), 59);

    let amb = a.ambiance().unwrap();
    assert_eq!((amb.fog_near, amb.fog_far), (30.0, 180.0));
    assert_eq!(amb.lightmaps.len(), 2);
    assert_eq!(amb.lightmaps[0].uv_scale, 0.008);
    assert_eq!(amb.sky_layers.len(), 1);

    assert_eq!(a.player_spawns().iter().filter(|s| s.kind == 0).count(), 30);
}
