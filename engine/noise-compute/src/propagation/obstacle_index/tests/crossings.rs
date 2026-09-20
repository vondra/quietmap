//! Crossings regression tests.

use super::*;

#[test]
fn ray_through_square_crosses_twice() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(500.0, 0.0, 10.0), 12.0, ObstacleKind::Building, 7);
    let idx = b.build();
    let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
    assert_eq!(c.len(), 2, "enter + exit");
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
    assert!(c
        .iter()
        .all(|x| x.height_m == 12.0 && x.kind == ObstacleKind::Building && x.id == 7));
}

#[test]
fn ray_beside_square_misses() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(500.0, 100.0, 10.0), 12.0, ObstacleKind::Building, 1);
    let idx = b.build();
    assert!(run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0)).is_empty());
}

#[test]
fn receiver_inside_footprint_sees_entry_only() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(1000.0, 0.0, 15.0), 20.0, ObstacleKind::Building, 3);
    let idx = b.build();
    let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
    assert_eq!(c.len(), 1, "only the entry edge crosses");
    assert!(c[0].t > 0.0 && c[0].t < 1.0);
}

#[test]
fn long_wall_crossing_reported_once() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_polyline(
        &[ll(500.0, -400.0), ll(500.0, 400.0)],
        4.0,
        ObstacleKind::Barrier,
        9,
    );
    let idx = b.build();
    let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
    assert_eq!(c.len(), 1, "one wall, one crossing");
    assert_eq!(c[0].kind, ObstacleKind::Barrier);
    assert!((c[0].t - 0.5).abs() < 0.005);
}

#[test]
fn two_buildings_sorted_by_chainage() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(300.0, 0.0, 10.0), 6.0, ObstacleKind::Building, 1);
    b.add_ring(&square(700.0, 0.0, 10.0), 9.0, ObstacleKind::Building, 2);
    let idx = b.build();
    let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
    assert_eq!(c.len(), 4);
    assert!(c.windows(2).all(|w| w[0].t < w[1].t));
    assert_eq!(
        c.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![1, 1, 2, 2],
        "near building's two edges first"
    );
}

#[test]
fn diagonal_ray_hits_offset_wall() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_polyline(
        &[ll(400.0, 260.0), ll(640.0, 100.0)],
        5.0,
        ObstacleKind::Barrier,
        4,
    );
    let idx = b.build();
    let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 500.0));
    assert_eq!(c.len(), 1, "diagonal wall must be hit exactly once");
}

#[test]
fn shallow_wall_with_many_intervening_hits_reported_once() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    // Wall from (100, -40) to (2000, 80): shallow diagonal, crossed early.
    b.add_polyline(
        &[ll(100.0, -40.0), ll(2000.0, 80.0)],
        4.0,
        ObstacleKind::Barrier,
        99,
    );
    // Ten small buildings straight along the ray after the wall crossing.
    for i in 0..10 {
        let cx = 500.0 + 120.0 * i as f64;
        b.add_ring(&square(cx, 0.0, 8.0), 6.0, ObstacleKind::Building, i);
    }
    let idx = b.build();
    let c = run(&idx, ll(0.0, 0.0), ll(2000.0, 0.0));
    let walls = c.iter().filter(|x| x.id == 99).count();
    assert_eq!(walls, 1, "wall must appear exactly once, got {walls}");
    assert_eq!(c.len(), 21, "1 wall + 10 buildings x 2 edges");
}

#[test]
fn ring_vertex_hit_is_single_candidate() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(
        &square(500.0, 100.0, 100.0),
        10.0,
        ObstacleKind::Building,
        5,
    );
    let idx = b.build();
    // Diagonal ray through the square's bottom-left corner (400, 0):
    // the corner lies at t=0.25, followed by an exit through the top edge.
    let c = run(&idx, ll(300.0, -100.0), ll(700.0, 300.0));
    let at_corner: Vec<_> = c.iter().filter(|x| (x.t - 0.25).abs() < 0.01).collect();
    assert!(
        at_corner.len() <= 1,
        "corner hit must dedup to one candidate, got {}",
        at_corner.len()
    );
    assert!(!c.is_empty());
}

#[test]
fn vertical_ray_crosses_horizontal_wall() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_polyline(
        &[ll(-50.0, 300.0), ll(50.0, 300.0)],
        3.0,
        ObstacleKind::Barrier,
        1,
    );
    let idx = b.build();
    let c = run(&idx, ll(0.0, 0.0), ll(0.0, 600.0));
    assert_eq!(c.len(), 1);
    assert!((c[0].t - 0.5).abs() < 0.005);
}

#[test]
fn ray_from_outside_slab_still_hits() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(0.0, 0.0, 20.0), 9.0, ObstacleKind::Building, 2);
    let idx = b.build();
    let c = run(&idx, ll(-5000.0, 0.0), ll(5000.0, 0.0));
    assert_eq!(c.len(), 2, "enter + exit despite far-outside endpoints");
}

#[test]
fn reversed_ray_is_symmetric() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(&square(300.0, 0.0, 10.0), 6.0, ObstacleKind::Building, 1);
    let idx = b.build();
    let fwd = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
    let rev = run(&idx, ll(1000.0, 0.0), ll(0.0, 0.0));
    assert_eq!(fwd.len(), 2);
    assert_eq!(rev.len(), 2);
    assert!((fwd[0].t - (1.0 - rev[1].t)).abs() < 1e-9);
    assert!((fwd[1].t - (1.0 - rev[0].t)).abs() < 1e-9);
}

#[test]
fn shared_polyline_vertex_dedups() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_polyline(
        &[ll(500.0, -100.0), ll(500.0, 0.0), ll(500.0, 100.0)],
        4.0,
        ObstacleKind::Barrier,
        8,
    );
    let idx = b.build();
    let c = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));
    assert_eq!(c.len(), 1, "shared vertex must not double-count");
}
