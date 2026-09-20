//! Grid-slab rejection preserves exact crossing sets.

use super::*;

fn boxes_at(offsets: &[(f64, f64)]) -> ObstacleSet {
    let mut indexes = Vec::new();
    for (i, &(cx, cy)) in offsets.iter().enumerate() {
        let mut b = ObstacleIndex::builder(OLAT, OLON);
        b.add_ring(
            &[
                ll(cx - 20.0, cy - 20.0),
                ll(cx + 20.0, cy - 20.0),
                ll(cx + 20.0, cy + 20.0),
                ll(cx - 20.0, cy + 20.0),
            ],
            9.0,
            ObstacleKind::Building,
            i as u32,
        );
        indexes.push(std::sync::Arc::new(b.build()));
    }
    ObstacleSet { indexes }
}

#[test]
fn slab_reject_never_changes_the_crossing_set() {
    // A grid_disk(1)-shaped ring of seven separated footprints.
    let set = boxes_at(&[
        (0.0, 0.0),
        (300.0, 0.0),
        (150.0, 260.0),
        (-150.0, 260.0),
        (-300.0, 0.0),
        (-150.0, -260.0),
        (150.0, -260.0),
    ]);
    let mut with = Vec::new();
    let mut without = Vec::new();
    let mut checked = 0usize;
    let mut skipped = 0usize;
    for i in 0..60 {
        for j in 0..60 {
            let src = ll(-500.0 + i as f64 * 17.3, -450.0 + j as f64 * 15.1);
            let rcv = ll(480.0 - j as f64 * 16.7, 430.0 - i as f64 * 14.9);
            set.crossings(src.0, src.1, rcv.0, rcv.1, &mut with);
            // Reference: every index walked, no reject.
            without.clear();
            for idx in &set.indexes {
                idx.append_crossings(
                    src.0,
                    src.1,
                    rcv.0,
                    rcv.1,
                    CellGate::All,
                    &mut CrossingScratch::default(),
                    &mut without,
                );
            }
            without.sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
            checked += 1;
            skipped += set
                .indexes
                .iter()
                .filter(|idx| !idx.segment_may_hit(src.0, src.1, rcv.0, rcv.1))
                .count();
            assert_eq!(with.len(), without.len(), "count differs");
            for (a, b) in with.iter().zip(&without) {
                assert_eq!(a.t, b.t, "chainage differs");
                assert_eq!(a.height_m, b.height_m);
                assert_eq!(a.id, b.id);
            }
        }
    }
    // And it must actually reject: the whole point is the walks not taken.
    let per_ray = skipped as f64 / checked as f64;
    assert!(
        per_ray > 2.0,
        "only {per_ray:.2} of 7 indexes rejected per ray — no win"
    );
    println!("slab reject: {per_ray:.2}/7 indexes skipped per ray over {checked} rays");
}
