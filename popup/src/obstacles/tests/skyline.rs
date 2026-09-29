//! The receiver skyline (dev4 `tests/skyline.rs`).

use super::*;
use physics::line::SkylineArc;

fn arcs(scene: &Scene, wedge: [f64; 2], radius_m: f64, los_floor_m: f64) -> Vec<SkylineArc> {
    let mut arcs = Vec::new();
    scene.skyline_arcs(
        [0.0, 0.0],
        wedge[0],
        wedge[1],
        radius_m,
        los_floor_m,
        &mut |arc| arcs.push(arc),
    );
    arcs
}

/// Distinct arcs: an edge listed in several cells is handed over once per cell.
fn distinct(mut arcs: Vec<SkylineArc>) -> Vec<SkylineArc> {
    arcs.sort_by(|a, b| {
        a.lo_rad
            .total_cmp(&b.lo_rad)
            .then(a.hi_rad.total_cmp(&b.hi_rad))
    });
    arcs.dedup_by(|a, b| {
        a.lo_rad == b.lo_rad && a.hi_rad == b.hi_rad && a.nearest_m == b.nearest_m
    });
    arcs
}

const EAST: [f64; 2] = [-0.5, 0.5];

#[test]
fn skyline_reports_in_range_edges_with_their_bearing() {
    let mut stock = Stock::new();
    stock.building(0, 8.0, &[square(200.0, 0.0, 15.0)]);
    stock.building(1, 8.0, &[square(2000.0, 0.0, 15.0)]);
    stock.with_scene(|scene| {
        let found = distinct(arcs(scene, EAST, 500.0, 0.0));
        assert_eq!(found.len(), 4, "one ring in range, four edges: {found:?}");
        for arc in &found {
            assert!(
                arc.hi_rad - arc.lo_rad < std::f64::consts::PI,
                "short arc: {arc:?}"
            );
            assert!(
                arc.lo_rad.abs() < 0.2 && arc.hi_rad.abs() < 0.2,
                "due east: {arc:?}"
            );
            assert!(
                (184.8..=216.2).contains(&arc.nearest_m),
                "range to the face: {arc:?}"
            );
        }
        assert!(
            arcs(scene, EAST, 100.0, 0.0).is_empty(),
            "a radius reaching neither box"
        );
        assert!(
            arcs(scene, [2.0, 2.5], 500.0, 0.0).is_empty(),
            "a wedge facing away"
        );
    });
}

#[test]
fn a_box_under_the_sight_line_is_pruned_whole() {
    for (height_m, expected) in [(8.0, 4), (4.0, 0), (3.0, 0)] {
        let mut stock = Stock::new();
        stock.building(0, height_m, &[square(200.0, 0.0, 15.0)]);
        let found = stock.with_scene(|scene| distinct(arcs(scene, EAST, 500.0, 4.0)));
        assert_eq!(
            found.len(),
            expected,
            "a {height_m} m box under a 4 m sight line"
        );
    }
}

#[test]
fn a_low_wall_is_pruned_on_its_own_height_not_its_cells() {
    let mut stock = Stock::new();
    stock.building(0, 20.0, &[square(200.0, 0.0, 20.0)]);
    stock.wall(1, 3.0, &[[190.0, 0.0], [210.0, 0.0]]);
    stock.with_scene(|scene| {
        let wall = |arc: &SkylineArc| (arc.nearest_m - 190.0).abs() < 0.5;
        let at_ground = arcs(scene, EAST, 500.0, 0.0);
        assert!(at_ground.iter().any(wall), "at ground level both stand");
        assert_eq!(distinct(at_ground).len(), 5);
        let above = arcs(scene, EAST, 500.0, 4.0);
        assert!(
            !above.iter().any(wall),
            "the 3 m wall is under a 4 m sight line"
        );
        assert_eq!(distinct(above).len(), 4);
    });
}

#[test]
fn a_multicell_wall_repeats_one_arc() {
    let mut stock = Stock::new();
    stock.wall(0, 8.0, &[[60.0, -400.0], [60.0, 400.0]]);
    let found = stock.with_scene(|scene| arcs(scene, [-1.5, 1.5], 1000.0, 0.0));
    assert!(found.len() > 1, "the wall spans many cells");
    assert!(
        found
            .iter()
            .all(|a| a.lo_rad == found[0].lo_rad && a.hi_rad == found[0].hi_rad),
        "one wall segment, one arc: {found:?}"
    );
    assert!(
        found[0].lo_rad < -1.0 && found[0].hi_rad > 1.0,
        "{:?}",
        found[0]
    );
}

#[test]
fn a_wall_half_a_metre_away_is_kept_and_one_through_the_receiver_is_not() {
    let mut stock = Stock::new();
    stock.wall(0, 3.0, &[[0.5, -5.0], [0.5, 5.0]]);
    stock.wall(1, 3.0, &[[-5.0, 0.0], [5.0, 0.0]]);
    stock.with_scene(|scene| {
        let found = distinct(arcs(scene, EAST, 50.0, 0.0));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0].nearest_m - 0.5).abs() < 0.2);
    });
}
