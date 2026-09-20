//! Signed delta bounds and reusable crossing scratch regressions.

use super::*;
use crate::constants::PENUMBRA_DELTA_FLOOR_M;

fn flat_penumbra_prune<'a>(t: &'a [f64], elev: &'a [f32]) -> CellPrune<'a> {
    CellPrune {
        t,
        elevation_m: elev,
        src_e: 4.0,
        rcv_e: 4.0,
        dist_m: 50.0,
        floor_m: PENUMBRA_DELTA_FLOOR_M,
    }
}

#[test]
fn low_top_prune_keeps_penumbra_candidate() {
    let (t, elev) = ([0.0, 1.0], [0.0f32, 0.0]);
    let p = flat_penumbra_prune(&t, &elev);

    // The true max over t ∈ [0,1]: reflection point t* = 1/(1+1) = 0.5.
    let exact = -(2.0 * (25.0f64 * 25.0 + 1.0).sqrt() - 50.0);
    let bound = p.max_delta(3.0, 0.0, 1.0);
    assert!(
        (bound - exact).abs() < 1e-12,
        "bound {bound} is not the exact max {exact}"
    );
    assert!((bound - -0.039_984_012_8).abs() < 1e-9, "bound {bound}");
    // …and it clears the floor, so the cell survives the prune.
    assert!(
        bound > PENUMBRA_DELTA_FLOOR_M,
        "{bound} <= {PENUMBRA_DELTA_FLOOR_M}"
    );

    // What the endpoints alone said — 25× too deep, under the floor, cell
    // dropped. This is the number the fix moved.
    let endpoints = -(1.0 + (50.0f64 * 50.0 + 1.0).sqrt() - 50.0);
    assert!((endpoints - -1.009_999_0).abs() < 1e-6, "{endpoints}");
    assert!(endpoints < PENUMBRA_DELTA_FLOOR_M, "{endpoints}");

    // Sub-windows: the clamp must still land on the exact max of the window
    // it is given, on both sides of the reflection point.
    for &(a, b) in &[(0.0, 0.4), (0.6, 1.0), (0.45, 0.55), (0.0, 1.0)] {
        let got = p.max_delta(3.0, a, b);
        let brute = (0..=2000)
            .map(|k| a + (b - a) * k as f64 / 2000.0)
            .map(|tt| {
                -(((tt * 50.0f64).powi(2) + 1.0).sqrt()
                    + (((1.0 - tt) * 50.0f64).powi(2) + 1.0).sqrt()
                    - 50.0)
            })
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            got >= brute - 1e-12,
            "window [{a},{b}]: bound {got} below sampled max {brute}"
        );
        assert!(
            got <= brute + 1e-6,
            "window [{a},{b}]: bound {got} far above sampled max {brute}"
        );
    }
}

#[test]
fn penumbra_wall_survives_the_pruned_walk() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(
        &[
            ll(24.0, -30.0),
            ll(26.0, -30.0),
            ll(26.0, 30.0),
            ll(24.0, 30.0),
        ],
        3.0,
        ObstacleKind::Barrier,
        7,
    );
    let idx = b.build();
    let src = ll(0.0, 0.0);
    let rcv = ll(50.0, 0.0);

    let (t, elev) = ([0.0, 0.5, 1.0], [0.0f32, 0.0, 0.0]);
    let p = flat_penumbra_prune(&t, &elev);
    let mut pruned = Vec::new();
    idx.crossings_pruned(src.0, src.1, rcv.0, rcv.1, &p, &mut pruned);
    let mut plain = Vec::new();
    idx.crossings(src.0, src.1, rcv.0, rcv.1, &mut plain);

    assert!(!plain.is_empty(), "the unpruned walk must see the wall");
    assert_eq!(
        pruned.len(),
        plain.len(),
        "prune dropped a candidate the loop's floor keeps"
    );
    for (a, c) in pruned.iter().zip(&plain) {
        assert_eq!((a.t, a.height_m, a.id), (c.t, c.height_m, c.id));
    }
}

#[test]
fn generation_scratch_matches_fresh_pruned_walk() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    for id in 0..24 {
        let x = 4.0 + id as f64 * 2.0;
        b.add_ring(
            &[ll(x, -8.0), ll(x + 0.8, -8.0), ll(x + 0.8, 8.0), ll(x, 8.0)],
            6.0,
            ObstacleKind::Building,
            id,
        );
    }
    let idx = b.build();
    let set = ObstacleSet {
        indexes: vec![std::sync::Arc::new(idx)],
    };
    let src = ll(0.0, 0.0);
    let (t, elev) = ([0.0, 0.5, 1.0], [0.0f32, 0.0, 0.0]);
    let p = flat_penumbra_prune(&t, &elev);
    let mut fresh = Vec::new();
    let mut reused = Vec::new();
    let mut scratch = CrossingScratch::default();
    for end_x in [40.0, 55.0, 70.0, 85.0, 100.0, 115.0] {
        let rcv = ll(end_x, 0.0);
        set.crossings_pruned(src.0, src.1, rcv.0, rcv.1, &p, &mut fresh);
        set.walk_ray(
            src.0,
            src.1,
            rcv.0,
            rcv.1,
            CellGate::Delta(&p),
            &mut scratch,
            &mut reused,
        );
        assert_eq!(
            reused.len(),
            fresh.len(),
            "scratch changed ray ending at {end_x} m"
        );
        for (actual, expected) in reused.iter().zip(&fresh) {
            assert_eq!(
                (actual.t, actual.height_m, actual.kind, actual.id),
                (expected.t, expected.height_m, expected.kind, expected.id),
                "scratch changed ray ending at {end_x} m"
            );
        }
    }
}
