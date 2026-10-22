//! Shimmy dead ends a side wall jump bridges, and holds only a leap up
//! reaches: `walljumps MAP.forge [X Y Z [COUNT]]` (env ACBMP_SCHEMAS),
//! nearest the point first.
use acb_game::climb::{entity_for, right_of, Dir, Stance};
use acb_game::ground::Ground;
use glam::Vec3;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let schema = acb_world::load_schema(std::path::Path::new(&std::env::var("ACBMP_SCHEMAS")?))?;
    let archive = acb_world::open_map(std::path::Path::new(&a[1]), &schema)?;
    let g = Ground::load(&archive)?;
    let near = if a.len() >= 5 { Vec3::new(a[2].parse()?, a[3].parse()?, a[4].parse()?) } else { Vec3::ZERO };
    let count: usize = a.get(5).map_or(15, |c| c.parse().unwrap());
    let holds = g.holds_around(near, 1e5);
    let (mut side, mut up) = (Vec::new(), Vec::new());
    let mut dead = 0;
    for h in holds.iter().filter(|h| h.length() >= 2.0) {
        let mid = (h.a + h.b) / 2.0;
        if g.hang_stance(mid, h.wall) != Stance::HangWall {
            continue;
        }
        let right = right_of(h.wall);
        for (t, sign) in [(0.25 / h.length(), 1.0f32), (1.0 - 0.25 / h.length(), -1.0)] {
            let hand = h.at(t);
            // the dead end is the far side of the end we are at
            let toward_end = if (h.b - h.a).truncate().dot(right) > 0.0 { -sign } else { sign };
            if g.shimmy(h, hand, toward_end).is_some() {
                continue;
            }
            dead += 1;
            let dir = if toward_end > 0.0 { Dir::Right } else { Dir::Left };
            if let Some((_, q)) = g.jump_hold(h, hand, h.wall, dir, 0.5, 2.6) {
                side.push((hand.distance(near), hand, q, dir, entity_for(hand, h.wall, Stance::HangWall)));
            }
        }
        if g.next_hold(mid, h.wall, Dir::Up).is_none() {
            if let Some((_, q)) = g.jump_hold(h, mid, h.wall, Dir::Up, 0.9, 2.6) {
                up.push((mid.distance(near), mid, q, Dir::Up, entity_for(mid, h.wall, Stance::HangWall)));
            }
        }
    }
    side.sort_by(|a, b| a.0.total_cmp(&b.0));
    up.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!("{dead} dead ends on 2 m+ hang edges; side jump bridges {}; leap-only holds above {}", side.len(), up.len());
    for (what, list) in [("side", &side), ("up", &up)] {
        for (d, hand, q, dir, e) in list.iter().take(count) {
            println!(
                "{what} {dir:?} {d:6.1} m: hands {:.2} {:.2} {:.2} -> {:.2} {:.2} {:.2} ({:.2} m), hang entity {:.2} {:.2} {:.2}",
                hand.x, hand.y, hand.z, q.x, q.y, q.z, hand.distance(*q), e.x, e.y, e.z
            );
        }
    }
    Ok(())
}
