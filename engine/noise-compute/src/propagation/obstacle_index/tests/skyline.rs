//! Skyline regression tests.

use super::*;

#[test]
fn skyline_reports_in_range_edges_with_their_bearing() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(200.0, 0.0, 15.0), 8.0, ObstacleKind::Building, 0);
    b.add_ring(&square(2000.0, 0.0, 15.0), 8.0, ObstacleKind::Building, 1);
    let idx = b.build();
    let mut arcs = Vec::new();
    let o = ll(0.0, 0.0);
    idx.skyline_arcs_within(0, o.0, o.1, 0.0, 500.0, 0.0, 0.0, None, None, &mut |a| {
        arcs.push(a)
    });
    assert_eq!(arcs.len(), 4, "one ring in range, four edges: {arcs:?}");
    for a in &arcs {
        assert!(a.hi - a.lo < std::f64::consts::PI, "short arc: {a:?}");
        assert!(a.lo.abs() < 0.2 && a.hi.abs() < 0.2, "due east: {a:?}");
        assert!(
            (185.0..=216.0).contains(&(a.near_m as f64)),
            "range to the near face: {a:?}"
        );
        assert_eq!(a.height_m, 8.0);
    }
    // A radius that reaches neither box.
    arcs.clear();
    idx.skyline_arcs_within(0, o.0, o.1, 0.0, 100.0, 0.0, 0.0, None, None, &mut |a| {
        arcs.push(a)
    });
    assert!(arcs.is_empty());
}

#[test]
fn skyline_grazing_prune_follows_the_delta_law() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(200.0, 0.0, 15.0), 8.0, ObstacleKind::Building, 0);
    let idx = b.build();
    let o = ll(0.0, 0.0);
    let count = |delta_min: f64| {
        let mut n = 0;
        idx.skyline_arcs_within(
            0,
            o.0,
            o.1,
            0.0,
            500.0,
            4.0,
            delta_min,
            None,
            None,
            &mut |_| n += 1,
        );
        n
    };
    assert_eq!(count(0.02), 4, "δ_min below the box's 0.04 m: kept");
    assert_eq!(count(0.08), 0, "δ_min above it: pruned whole");
    // A box shorter than the sight line cannot break it at any δ floor.
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(200.0, 0.0, 15.0), 3.0, ObstacleKind::Building, 0);
    let low = b.build();
    let mut n = 0;
    low.skyline_arcs_within(0, o.0, o.1, 0.0, 500.0, 4.0, 0.0, None, None, &mut |_| {
        n += 1
    });
    assert_eq!(n, 0, "top below the 4 m sight line");
}

#[test]
fn skyline_prunes_a_low_wall_on_its_own_height_not_its_cells() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(200.0, 0.0, 20.0), 20.0, ObstacleKind::Building, 0);
    b.add_polyline(
        &[ll(190.0, 0.0), ll(210.0, 0.0)],
        3.0,
        ObstacleKind::Barrier,
        1,
    );
    let idx = b.build();
    let o = ll(0.0, 0.0);
    let tops = |los_floor_m: f64| {
        let mut heights = Vec::new();
        idx.skyline_arcs_within(
            0,
            o.0,
            o.1,
            0.0,
            500.0,
            los_floor_m,
            0.0,
            None,
            None,
            &mut |a| heights.push(a.height_m),
        );
        heights.sort_by(f32::total_cmp);
        heights.dedup();
        heights
    };
    assert_eq!(tops(0.0), vec![3.0, 20.0], "at ground level both stand");
    assert_eq!(
        tops(4.0),
        vec![20.0],
        "the 3 m wall is under a 4 m sight line"
    );
}

#[test]
fn skyline_multicell_wall_repeats_are_identical() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_polyline(
        &[ll(60.0, -400.0), ll(60.0, 400.0)],
        8.0,
        ObstacleKind::Barrier,
        0,
    );
    let idx = b.build();
    let o = ll(0.0, 0.0);
    let mut arcs = Vec::new();
    idx.skyline_arcs_within(0, o.0, o.1, 0.0, 1000.0, 0.0, 0.0, None, None, &mut |a| {
        arcs.push(a)
    });
    assert!(!arcs.is_empty());
    assert!(
        arcs.iter()
            .all(|a| a.lo == arcs[0].lo && a.hi == arcs[0].hi && a.source_id == arcs[0].source_id),
        "one wall segment, one arc geometry: {arcs:?}"
    );
    // It spans from due south-ish to due north-ish through due east.
    assert!(arcs[0].lo < -1.0 && arcs[0].hi > 1.0, "{:?}", arcs[0]);
}

#[test]
fn set_skyline_concatenates_indexes() {
    let mut b0 = ObstacleIndex::builder(OLAT, OLON);
    b0.add_ring(&square(200.0, -60.0, 10.0), 8.0, ObstacleKind::Building, 0);
    let mut b1 = ObstacleIndex::builder(OLAT, OLON);
    b1.add_ring(&square(200.0, 60.0, 10.0), 8.0, ObstacleKind::Building, 0);
    let set = ObstacleSet {
        indexes: vec![
            std::sync::Arc::new(b0.build()),
            std::sync::Arc::new(b1.build()),
        ],
    };
    let o = ll(0.0, 0.0);
    let mut arcs = Vec::new();
    set.skyline_arcs_within(o.0, o.1, 0.0, 500.0, 0.0, 0.0, None, None, &mut |a| {
        arcs.push(a)
    });
    assert_eq!(arcs.len(), 8);
    let first_ids: std::collections::BTreeSet<_> = arcs
        .iter()
        .take(4)
        .map(|arc| arc.source_id.bits())
        .collect();
    let second_ids: std::collections::BTreeSet<_> = arcs
        .iter()
        .skip(4)
        .map(|arc| arc.source_id.bits())
        .collect();
    assert_eq!(first_ids, [0, 1, 2, 3].into_iter().collect());
    assert_eq!(second_ids, [4, 5, 6, 7].into_iter().collect());
    assert!(first_ids.is_disjoint(&second_ids));
    assert!(arcs.iter().any(|a| a.hi < 0.0), "the southern box");
    assert!(arcs.iter().any(|a| a.lo > 0.0), "the northern box");
}

#[test]
fn seen_edges_span_the_set_without_colliding_across_indexes() {
    let mut b0 = ObstacleIndex::builder(OLAT, OLON);
    b0.add_ring(&square(200.0, -60.0, 10.0), 8.0, ObstacleKind::Building, 0);
    let mut b1 = ObstacleIndex::builder(OLAT, OLON);
    b1.add_ring(&square(200.0, 60.0, 10.0), 8.0, ObstacleKind::Building, 0);
    let set = ObstacleSet {
        indexes: vec![
            std::sync::Arc::new(b0.build()),
            std::sync::Arc::new(b1.build()),
        ],
    };
    let o = ll(0.0, 0.0);
    let mut seen = SeenEdges::default();
    let mut ids = Vec::new();
    set.skyline_arcs_within(
        o.0,
        o.1,
        0.0,
        500.0,
        0.0,
        0.0,
        None,
        Some(&mut seen),
        &mut |a| ids.push(a.source_id.bits()),
    );
    ids.sort_unstable();
    assert_eq!(ids, vec![0, 1, 2, 3, 4, 5, 6, 7]);
}
