//! Ray crossings (dev4 `tests/crossings.rs`, `tests/ray_cell_bounds.rs`).

use super::*;

#[test]
fn ray_through_square_crosses_twice() {
    let mut stock = Stock::new();
    stock.building(7, 12.0, &[square(500.0, 0.0, 10.0)]);
    let c = stock.with_scene(|scene| crossings_of(scene, [0.0, 0.0], [1000.0, 0.0]));
    assert_eq!(c.len(), 2, "enter and exit");
    assert!(c[0].t < c[1].t);
    assert!(
        (c[0].t - 0.49).abs() < 0.005,
        "entry at ~490 m, got {}",
        c[0].t
    );
    assert!(
        (c[1].t - 0.51).abs() < 0.005,
        "exit at ~510 m, got {}",
        c[1].t
    );
    assert!(
        c.iter()
            .all(|x| x.height_m == 12.0 && x.building && x.footprint_id == 7)
    );
}

#[test]
fn ray_beside_square_misses() {
    let mut stock = Stock::new();
    stock.building(1, 12.0, &[square(500.0, 100.0, 10.0)]);
    assert!(
        stock
            .with_scene(|scene| crossings_of(scene, [0.0, 0.0], [1000.0, 0.0]))
            .is_empty()
    );
}

#[test]
fn receiver_inside_footprint_sees_entry_only() {
    let mut stock = Stock::new();
    stock.building(3, 20.0, &[square(1000.0, 0.0, 15.0)]);
    let c = stock.with_scene(|scene| crossings_of(scene, [0.0, 0.0], [1000.0, 0.0]));
    assert_eq!(c.len(), 1, "only the entry edge crosses");
    assert!(c[0].t > 0.0 && c[0].t < 1.0);
}

#[test]
fn long_wall_crossing_reported_once() {
    let mut stock = Stock::new();
    stock.wall(9, 4.0, &[[500.0, -400.0], [500.0, 400.0]]);
    let c = stock.with_scene(|scene| crossings_of(scene, [0.0, 0.0], [1000.0, 0.0]));
    assert_eq!(c.len(), 1, "one wall, one crossing");
    assert!(!c[0].building);
    assert!((c[0].t - 0.5).abs() < 0.005);
}

#[test]
fn two_buildings_sorted_by_chainage() {
    let mut stock = Stock::new();
    stock.building(1, 6.0, &[square(300.0, 0.0, 10.0)]);
    stock.building(2, 9.0, &[square(700.0, 0.0, 10.0)]);
    let c = stock.with_scene(|scene| crossings_of(scene, [0.0, 0.0], [1000.0, 0.0]));
    assert_eq!(c.len(), 4);
    assert!(c.windows(2).all(|w| w[0].t < w[1].t));
    let ids: Vec<u64> = c.iter().map(|x| x.footprint_id).collect();
    assert_eq!(ids, vec![1, 1, 2, 2], "near building's two edges first");
}

#[test]
fn diagonal_ray_hits_offset_wall() {
    let mut stock = Stock::new();
    stock.wall(4, 5.0, &[[400.0, 260.0], [640.0, 100.0]]);
    let c = stock.with_scene(|scene| crossings_of(scene, [0.0, 0.0], [1000.0, 500.0]));
    assert_eq!(c.len(), 1, "diagonal wall must be hit exactly once");
}

#[test]
fn shallow_wall_with_many_intervening_hits_reported_once() {
    let mut stock = Stock::new();
    // Crossed early, then running beside the ray through many cells.
    stock.wall(99, 4.0, &[[100.0, -40.0], [2000.0, 80.0]]);
    for i in 0..10 {
        stock.building(i, 6.0, &[square(500.0 + 120.0 * i as f64, 0.0, 8.0)]);
    }
    let c = stock.with_scene(|scene| crossings_of(scene, [0.0, 0.0], [2000.0, 0.0]));
    let walls = c.iter().filter(|x| x.footprint_id == 99).count();
    assert_eq!(walls, 1, "wall must appear exactly once, got {walls}");
    assert_eq!(c.len(), 21, "1 wall + 10 buildings x 2 edges");
}

#[test]
fn ring_vertex_hit_is_single_candidate() {
    let mut stock = Stock::new();
    stock.building(5, 10.0, &[square(500.0, 100.0, 100.0)]);
    // A diagonal ray through the stored south-west corner (at t = 0.25), exiting through the top.
    let corner = stock.snapped([400.0, 0.0]);
    let from = [corner[0] - 100.0, corner[1] - 100.0];
    let to = [corner[0] + 300.0, corner[1] + 300.0];
    let c = stock.with_scene(|scene| crossings_of(scene, from, to));
    let at_corner = c.iter().filter(|x| (x.t - 0.25).abs() < 0.01).count();
    assert!(
        at_corner <= 1,
        "corner hit must dedup to one candidate, got {at_corner}"
    );
    assert!(!c.is_empty());
}

#[test]
fn vertical_ray_crosses_horizontal_wall() {
    let mut stock = Stock::new();
    stock.wall(1, 3.0, &[[-50.0, 300.0], [50.0, 300.0]]);
    let c = stock.with_scene(|scene| crossings_of(scene, [0.0, 0.0], [0.0, 600.0]));
    assert_eq!(c.len(), 1);
    assert!((c[0].t - 0.5).abs() < 0.005);
}

#[test]
fn ray_from_far_outside_still_hits() {
    let mut stock = Stock::new();
    stock.building(2, 9.0, &[square(0.0, 0.0, 20.0)]);
    let c = stock.with_scene(|scene| crossings_of(scene, [-5000.0, 0.0], [5000.0, 0.0]));
    assert_eq!(c.len(), 2, "enter and exit despite far endpoints");
}

#[test]
fn reversed_ray_is_symmetric() {
    let mut stock = Stock::new();
    stock.building(1, 6.0, &[square(300.0, 0.0, 10.0)]);
    let (forward, reverse) = stock.with_scene(|scene| {
        (
            crossings_of(scene, [0.0, 0.0], [1000.0, 0.0]),
            crossings_of(scene, [1000.0, 0.0], [0.0, 0.0]),
        )
    });
    assert_eq!((forward.len(), reverse.len()), (2, 2));
    assert!((forward[0].t - (1.0 - reverse[1].t)).abs() < 1e-9);
    assert!((forward[1].t - (1.0 - reverse[0].t)).abs() < 1e-9);
}

#[test]
fn shared_polyline_vertex_dedups() {
    let mut stock = Stock::new();
    let vertex = stock.snapped([500.0, 0.0]);
    stock.wall(8, 4.0, &[[500.0, -100.0], vertex, [500.0, 100.0]]);
    let c = stock.with_scene(|scene| crossings_of(scene, [0.0, vertex[1]], [1000.0, vertex[1]]));
    assert_eq!(c.len(), 1, "shared vertex must not double-count");
}

#[test]
fn min_y_wall_matches_the_reference() {
    let mut stock = Stock::new();
    stock.building(0, 10.0, &[square(0.0, 0.0, 10.0)]);
    stock.building(1, 10.0, &[square(0.0, 200.0, 10.0)]);
    stock.with_scene(|scene| {
        let (from, to) = ([0.0, 2_000.0], [0.0, -2_000.0]);
        let walked = crossings_of(scene, from, to);
        assert_eq!(
            walked.len(),
            4,
            "both footprints, entry and exit: {walked:?}"
        );
        assert_eq!(walked, every_crossing(scene, from, to));
    });
}

/// Half the stocks put their walls exactly on cell lines, so crossings sit on cell boundaries by
/// construction: the ray walk and the builder's supercover must still meet in a cell listing
/// every edge the ray crosses. Rays run 6.4 m apart over 49 m cells (dev4: 8.4 m over 64 m).
#[test]
fn the_walk_never_loses_a_crossing_over_a_swept_population() {
    let mut state = 0xC85E_ED01_u64;
    let mut next = |lo: f64, hi: f64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        lo + (hi - lo) * ((state >> 11) as f64 / (1u64 << 53) as f64)
    };
    let mut checked = 0usize;
    for trial in 0..200 {
        let mut stock = Stock::new();
        let origin = stock.steps([0.0, 0.0]);
        let lattice_corner = [
            origin.x.div_euclid(256) * 256,
            origin.y.div_euclid(256) * 256,
        ];
        for id in 0..12 {
            if trial % 2 == 0 {
                let (column, row) = (
                    (next(-6.0, 6.0) as i64) * 256,
                    (next(-6.0, 6.0) as i64) * 256,
                );
                let at = |dx: i64, dy: i64| GlobalSteps {
                    x: lattice_corner[0] + column + dx,
                    y: lattice_corner[1] + row + dy,
                };
                stock.building_in_steps(
                    id,
                    10.0,
                    vec![at(0, 0), at(256, 0), at(256, 256), at(0, 256)],
                );
            } else {
                let (east, north, half) =
                    (next(-450.0, 450.0), next(-450.0, 450.0), next(4.0, 30.0));
                stock.building(id, 10.0, &[square(east, north, half)]);
            }
        }
        stock.with_scene(|scene| {
            for k in 0..157 {
                let offset = -500.0 + k as f64 * 6.4;
                for (from, to) in [
                    ([offset, -1500.0], [offset, 1500.0]),
                    ([-1500.0, offset], [1500.0, offset]),
                    ([offset - 1500.0, -1500.0], [offset + 1500.0, 1500.0]),
                ] {
                    let walked = crossings_of(scene, from, to);
                    for want in every_crossing(scene, from, to) {
                        assert!(
                            walked
                                .iter()
                                .any(|got| got.footprint_id == want.footprint_id
                                    && (got.t - want.t).abs() < 1e-9),
                            "walk dropped id {} t {} (trial {trial})",
                            want.footprint_id,
                            want.t
                        );
                        checked += 1;
                    }
                }
            }
        });
    }
    assert!(
        checked > 100_000,
        "the sweep must reach the walk: {checked}"
    );
}

/// dev4 `ray_cell_bounds.rs`: the box filter only skips edges whose crossing cannot lie in the
/// piece; every exact hit passes the box of the piece reduced to the hit itself, in any tile.
#[test]
fn the_box_filter_never_rejects_an_exact_hit() {
    use super::super::crossings::{edge_meets_box, piece_box};
    let mut state = 0x0d15_ea5e_5eed_u64;
    let mut next = |lo: f64, hi: f64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        lo + (hi - lo) * ((state >> 11) as f64 / (1u64 << 53) as f64)
    };
    let mut hits = 0;
    for _ in 0..4_000 {
        let offset = [
            next(-200_000.0, 200_000.0) as i64,
            next(-200_000.0, 200_000.0) as i64,
        ];
        let vertex = |x: f64, y: f64| [x as i16, y as i16];
        let (a, b) = (
            vertex(next(-400.0, 400.0), next(-400.0, 400.0)),
            vertex(next(-400.0, 400.0), next(-400.0, 400.0)),
        );
        let steps = |v: [i16; 2]| {
            [
                (offset[0] + i64::from(v[0])) as f64,
                (offset[1] + i64::from(v[1])) as f64,
            ]
        };
        let start = [
            offset[0] as f64 + next(-500.0, 500.0),
            offset[1] as f64 + next(-500.0, 500.0),
        ];
        let delta = [next(-800.0, 800.0), next(-800.0, 800.0)];
        let pad = 1e-9 * (1.0 + delta[0].abs() + delta[1].abs());
        if let Some(t) = segment_intersection_t(start, delta, steps(a), steps(b)) {
            hits += 1;
            let at = [start[0] + delta[0] * t, start[1] + delta[1] * t];
            assert!(
                edge_meets_box(a, b, piece_box([at, at], pad, offset)),
                "hit at t {t} rejected"
            );
        }
    }
    assert!(hits > 300, "the draw must exercise hits: {hits}");
    // The box is closed: a touch on any side passes, one step apart does not.
    let piece = [[0, 0], [10, 10]];
    for (touch, apart) in [
        (([-3, 5], [0, 5]), ([-3, 5], [-1, 5])),
        (([10, 5], [13, 5]), ([11, 5], [13, 5])),
        (([5, -3], [5, 0]), ([5, -3], [5, -1])),
        (([5, 10], [5, 13]), ([5, 11], [5, 13])),
    ] {
        assert!(edge_meets_box(touch.0, touch.1, piece), "{touch:?}");
        assert!(!edge_meets_box(apart.0, apart.1, piece), "{apart:?}");
    }
}
