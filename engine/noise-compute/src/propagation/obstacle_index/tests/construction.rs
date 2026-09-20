//! Construction regression tests.

use super::*;

#[test]
fn grid_pitch_follows_edge_density_down_to_the_floor() {
    assert_eq!(
        obstacle_grid_cell_m(f64::MIN - f64::MAX, f64::MIN - f64::MAX, 0),
        32.0
    );
    assert_eq!(obstacle_grid_cell_m(640.0, 640.0, 400), 64.0);
    assert_eq!(obstacle_grid_cell_m(640.0, 640.0, 40_000), 32.0);
}

#[test]
fn empty_index_yields_no_crossings() {
    let idx = ObstacleIndex::builder(OLAT, OLON).build();
    assert_eq!(idx.edge_count(), 0);
    assert!(run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0)).is_empty());
}

#[test]
fn building_obstacle_height_is_clamped_at_edge_formation() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(
        &square(300.0, 0.0, 10.0),
        31_231.0,
        ObstacleKind::Building,
        1,
    );
    b.add_polyline(
        &[ll(700.0, -20.0), ll(700.0, 20.0)],
        31_231.0,
        ObstacleKind::Barrier,
        2,
    );
    let idx = b.build();
    let crossings = run(&idx, ll(0.0, 0.0), ll(1000.0, 0.0));

    let building_heights: Vec<_> = crossings
        .iter()
        .filter(|candidate| candidate.kind == ObstacleKind::Building)
        .map(|candidate| candidate.height_m)
        .collect();
    assert_eq!(building_heights, vec![828.0, 828.0]);
    assert_eq!(
        crossings
            .iter()
            .find(|candidate| candidate.kind == ObstacleKind::Barrier)
            .map(|candidate| candidate.height_m),
        Some(31_231.0)
    );
}

#[test]
fn degenerate_inputs_are_ignored() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(
        &[ll(0.0, 0.0), ll(10.0, 0.0)],
        5.0,
        ObstacleKind::Building,
        1,
    );
    b.add_ring(&square(500.0, 0.0, 10.0), 0.0, ObstacleKind::Building, 2);
    b.add_polyline(&[ll(0.0, 0.0)], 3.0, ObstacleKind::Barrier, 3);
    let idx = b.build();
    assert_eq!(idx.edge_count(), 0);
}

#[test]
fn non_finite_inputs_are_rejected() {
    let mut b = ObstacleIndex::builder(OLAT, OLON);
    b.add_ring(
        &[ll(0.0, 0.0), (f64::NAN, 14.0), ll(10.0, 10.0)],
        5.0,
        ObstacleKind::Building,
        1,
    );
    b.add_ring(
        &square(500.0, 0.0, 10.0),
        f32::NAN,
        ObstacleKind::Building,
        2,
    );
    b.add_polyline(
        &[ll(0.0, 0.0), (50.0, f64::INFINITY)],
        3.0,
        ObstacleKind::Barrier,
        3,
    );
    let idx = b.build();
    assert_eq!(idx.edge_count(), 0);
}
