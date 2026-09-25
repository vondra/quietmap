//! Ray cell bounds regression tests.

use super::*;

#[test]
fn ray_cell_aabb_keeps_boundary_touches_and_rejects_separation() {
    let edge = |x0, y0, x1, y1| ObstacleEdge {
        x0,
        y0,
        x1,
        y1,
        height_m: 1.0,
        id: 0,
        kind: ObstacleKind::Building.code(),
    };
    let ray_x = (0.0, 1.0);
    let ray_y = (0.0, 1.0);
    for (name, touch, outside) in [
        (
            "left",
            edge(0.0, 0.2, 0.0, 0.8),
            edge(-1e-6, 0.2, -1e-6, 0.8),
        ),
        (
            "right",
            edge(1.0, 0.2, 1.0, 0.8),
            edge(1.0 + 1e-6, 0.2, 1.0 + 1e-6, 0.8),
        ),
        (
            "bottom",
            edge(0.2, 0.0, 0.8, 0.0),
            edge(0.2, -1e-6, 0.8, -1e-6),
        ),
        (
            "top",
            edge(0.2, 1.0, 0.8, 1.0),
            edge(0.2, 1.0 + 1e-6, 0.8, 1.0 + 1e-6),
        ),
    ] {
        assert!(
            ray_cell_aabb_may_overlap(ray_x, ray_y, &touch),
            "a closed AABB must keep its {name} boundary touch"
        );
        assert!(
            !ray_cell_aabb_may_overlap(ray_x, ray_y, &outside),
            "strictly separated {name} AABBs can skip the exact predicate"
        );
    }
}

#[test]
fn ray_cell_aabb_never_rejects_an_exact_intersection_point() {
    let mut state = 0x0d15_ea5e_5eed_u64;
    let mut hits = 0usize;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((state >> 32) as u32 as f64 / u32::MAX as f64) * 200.0 - 100.0
    };
    for _ in 0..4_000 {
        let (sx, sy) = (next(), next());
        let (dx, dy) = (next(), next());
        let edge = ObstacleEdge {
            x0: next() as f32,
            y0: next() as f32,
            x1: next() as f32,
            y1: next() as f32,
            height_m: 1.0,
            id: 0,
            kind: ObstacleKind::Building.code(),
        };
        if let Some(t) = segment_intersection_t(
            sx,
            sy,
            dx,
            dy,
            edge.x0 as f64,
            edge.y0 as f64,
            edge.x1 as f64,
            edge.y1 as f64,
        ) {
            hits += 1;
            let (ray_x, ray_y) = ray_cell_aabb(sx, sy, dx, dy, t, t);
            assert!(
                ray_cell_aabb_may_overlap(ray_x, ray_y, &edge),
                "exact hit t={t} cannot be outside its edge AABB"
            );
        }
    }
    assert!(
        hits > 100,
        "test distribution must exercise exact hits: {hits}"
    );
}

fn unscreened_crossings(
    idx: &ObstacleIndex,
    from: (f64, f64),
    to: (f64, f64),
) -> Vec<CrossingCandidate> {
    let (sx, sy) = idx.to_local(from.0, from.1);
    let (rx, ry) = idx.to_local(to.0, to.1);
    let (dx, dy) = (rx - sx, ry - sy);
    let mut out: Vec<_> = idx
        .edges
        .iter()
        .filter_map(|edge| {
            segment_intersection_t(
                sx,
                sy,
                dx,
                dy,
                edge.x0 as f64,
                edge.y0 as f64,
                edge.x1 as f64,
                edge.y1 as f64,
            )
            .map(|t| CrossingCandidate {
                t,
                height_m: edge.height_m,
                kind: edge.kind(),
                id: edge.id,
                index: 0,
            })
        })
        .collect();
    out.sort_unstable_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    out.dedup_by(|a, b| a.id == b.id && (a.t - b.t).abs() < 1e-9);
    out
}

#[test]
fn min_y_wall_matches_unscreened_reference() {
    let mut builder = ObstacleIndex::builder(OLAT, OLON);
    builder.add_ring(&square(0.0, 0.0, 10.0), 10.0, ObstacleKind::Building, 0);
    builder.add_ring(&square(0.0, 200.0, 10.0), 10.0, ObstacleKind::Building, 1);
    let idx = builder.build();
    let from = ll(0.0, 2_000.0);
    let to = ll(0.0, -2_000.0);
    let screened = run(&idx, from, to);
    let reference = unscreened_crossings(&idx, from, to);
    assert_eq!(
        screened.len(),
        4,
        "both footprints, entry + exit: {screened:?}"
    );
    assert_eq!(
        screened.len(),
        reference.len(),
        "screened={screened:?}, reference={reference:?}"
    );
    for (got, expected) in screened.iter().zip(&reference) {
        assert_eq!(got.t, expected.t);
        assert_eq!(got.id, expected.id);
        assert_eq!(got.height_m, expected.height_m);
        assert_eq!(got.kind, expected.kind);
    }
}

#[test]
fn screen_never_loses_a_crossing_over_a_swept_population() {
    const PITCH_M: f64 = 64.0;
    let mut state = 0xC85E_ED01_u64;
    let mut next = |lo: f64, hi: f64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        lo + (hi - lo) * ((state >> 11) as f64 / (1u64 << 53) as f64)
    };
    let mut checked = 0usize;
    for trial in 0..200 {
        let mut builder = ObstacleIndex::builder(OLAT, OLON);
        for id in 0..12 {
            // Half the trials snap corners onto the grid pitch, so
            // crossings sit exactly ON cell boundaries by construction.
            // The pitch is pinned below rather than derived from the
            // stock: what this sweeps is the last-ulp meeting of the edge
            // supercover and the query DDA, the same property at every
            // pitch, and fixtures cannot be snapped to a pitch their own
            // extent decides.
            let (cx, cy, half) = if trial % 2 == 0 {
                (
                    (next(-6.0, 6.0) as i64) as f64 * PITCH_M,
                    (next(-6.0, 6.0) as i64) as f64 * PITCH_M,
                    PITCH_M / 2.0,
                )
            } else {
                (next(-450.0, 450.0), next(-450.0, 450.0), next(4.0, 30.0))
            };
            builder.add_ring(&square(cx, cy, half), 10.0, ObstacleKind::Building, id);
        }
        let bounds = builder.edge_bounds();
        let idx = builder.build_at_pitch_m(bounds, PITCH_M);
        for k in 0..120 {
            let off = -500.0 + k as f64 * 8.4;
            for &(from, to) in &[
                (ll(off, -1500.0), ll(off, 1500.0)), // axis-aligned
                (ll(-1500.0, off), ll(1500.0, off)), // axis-aligned
                (ll(off - 1500.0, -1500.0), ll(off + 1500.0, 1500.0)), // diagonal
            ] {
                let screened = run(&idx, from, to);
                for want in unscreened_crossings(&idx, from, to) {
                    assert!(
                        screened
                            .iter()
                            .any(|got| got.id == want.id && (got.t - want.t).abs() < 1e-9),
                        "screen dropped id={} t={} (trial {trial})",
                        want.id,
                        want.t
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 100_000, "sweep must reach the walk: {checked}");
}
