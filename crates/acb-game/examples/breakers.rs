//! A map's chase breakers as the game reads them: trigger + scene groups
//! (doors, platforms, poles) with their timelines, lifts, and every
//! inactive collider with its bounds: `breakers MAP.forge` (env
//! ACBMP_SCHEMAS).
use acb_world::collision::xform;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let breakers = archive.breakers();
    println!("{} breakers", breakers.len());
    for b in &breakers {
        let at = &b.matrix[12..15];
        println!(
            "{:#010x} {} kind {:?} at {:.2?} zone {:.2?} watch {} every {} s enter {:?} update {:?} exit {:?} condition {:?} {:?} length {:.2} s",
            b.group, b.name, b.kind, at, b.zone, b.watch, b.update_frequency, b.on_enter, b.on_update, b.on_exit, b.on_condition, b.condition, b.length
        );
        for t in &b.toggles {
            println!("    {:5.2} s {} {} of {:#010x}", t.time, if t.on { "on " } else { "off" }, schema.name_or_hash(t.component), t.entity);
        }
        for m in &b.moves {
            println!("    {:5.2} s +{:.2} s move {:#010x} by {:.2?} turn {:.3?} (target type {})", m.start, m.duration, m.entity, m.translate, m.rotate, m.target);
        }
    }
    let lifts = archive.lifts();
    println!("{} lifts", lifts.len());
    for l in &lifts {
        println!(
            "{:#010x} {} at {:.2?} detect {:.2?} r {} start {:.2?} rise {} m anims {:x?}",
            l.entity,
            l.name,
            &l.matrix[12..15],
            xform(&l.matrix, l.detection_offset),
            l.activation_radius,
            xform(&l.matrix, l.assassin_offset),
            l.rise(),
            l.animations
        );
    }
    for c in archive.colliders() {
        let dynamic = lifts.iter().any(|l| l.entity == c.entity) || breakers.iter().any(|b| b.toggles.iter().any(|t| t.entity == c.entity));
        if c.active && !dynamic {
            continue;
        }
        let Some(mesh) = archive.collision_mesh(c.shape).ok().flatten() else { continue };
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &mesh.vertices {
            let w = xform(&c.matrix, *v);
            for k in 0..3 {
                lo[k] = lo[k].min(w[k]);
                hi[k] = hi[k].max(w[k]);
            }
        }
        println!(
            "collider entity {:#010x} {} shape {:#010x} active {} layer {} fixed {} guidance {:#x}: {:.2?} .. {:.2?}",
            c.entity,
            schema.name_or_hash(c.component),
            c.shape,
            c.active,
            c.layer,
            c.fixed,
            c.guidance,
            lo,
            hi
        );
    }
    Ok(())
}
