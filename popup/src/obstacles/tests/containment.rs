//! Containment and the reflection bonus (dev4 `tests/containment.rs`, `tests/reflection.rs`).

use super::*;

fn contains(scene: &Scene, point: [f64; 2], min_height_m: f64) -> bool {
    scene
        .contains_built(scene.lattice.steps(point), min_height_m, None)
        .unwrap()
}

#[test]
fn contains_and_enclosure_thresholds() {
    let mut stock = Stock::new();
    stock.building(1, 12.0, &[square(0.0, 0.0, 60.0)]);
    stock.with_scene(|scene| {
        assert!(contains(scene, [0.0, 0.0], 5.0), "centre is inside");
        assert!(!contains(scene, [300.0, 0.0], 5.0), "outside");
        assert!(
            !contains(scene, [0.0, 0.0], 20.0),
            "the height gate excludes the 12 m footprint"
        );
        // 60 m half-size against 75 m probes: only the centre probe inside, 1/9, 0 dB.
        assert_eq!(scene.reflection_db([0.0, 0.0], None).unwrap(), 0.0);
    });
    let mut block = Stock::new();
    block.building(1, 12.0, &[square(0.0, 0.0, 200.0)]);
    assert_eq!(
        block.with_scene(|scene| scene.reflection_db([0.0, 0.0], None).unwrap()),
        3.0
    );
}

#[test]
fn overlapping_footprints_contain_correctly() {
    let mut stock = Stock::new();
    stock.building(1, 12.0, &[square(0.0, 0.0, 50.0)]);
    stock.building(2, 15.0, &[square(20.0, 0.0, 50.0)]);
    stock.with_scene(|scene| {
        assert!(contains(scene, [0.0, 0.0], 5.0), "inside both");
        assert!(contains(scene, [60.0, 0.0], 5.0), "inside the second only");
        assert!(!contains(scene, [200.0, 0.0], 5.0), "outside both");
        let winner = scene.enclosing_building([0.0, 0.0]).unwrap().unwrap();
        assert_eq!(
            (winner.id, winner.height_m),
            (2, 15.0),
            "the taller footprint wins"
        );
    });
}

#[test]
fn parity_on_a_vertex_row_is_consistent() {
    let mut stock = Stock::new();
    stock.building(1, 10.0, &[square(100.0, 0.0, 40.0)]);
    let corner = stock.steps([60.0, -40.0]);
    stock.with_scene(|scene| {
        // West of the square exactly on its corner row: the half-line grazes both corners and
        // the probe stays outside; on a mid-edge row too; inside stays inside.
        let [_, corner_row] = scene_steps(scene, corner);
        let [west, _] = scene.lattice.steps([-200.0, 0.0]);
        assert!(!scene.contains_built([west, corner_row], 5.0, None).unwrap());
        assert!(!contains(scene, [-200.0, 0.0], 5.0));
        assert!(contains(scene, [100.0, -39.9], 5.0));
    });
}

#[test]
fn a_far_footprint_does_not_phantom_capture() {
    let mut stock = Stock::new();
    stock.building(0, 10.0, &[square(0.0, 0.0, 20.0)]);
    // An 800 m block whose interior would swallow the end of a fixed-length probe ray.
    stock.building(1, 10.0, &[square(1800.0, 0.0, 400.0)]);
    stock.with_scene(|scene| {
        assert!(
            !contains(scene, [100.0, 0.0], 5.0),
            "between the two, inside neither"
        );
        assert!(contains(scene, [1800.0, 0.0], 5.0));
    });
}

#[test]
fn an_oversized_footprint_is_still_contained() {
    let mut stock = Stock::new();
    stock.building(0, 10.0, &[square(0.0, 0.0, 1500.0)]);
    stock.with_scene(|scene| {
        assert!(
            contains(scene, [-1400.0, 0.0], 5.0),
            "2.9 km from the east wall"
        );
        assert!(!contains(scene, [-1600.0, 0.0], 5.0));
    });
}

#[test]
fn a_courtyard_reads_outside_and_the_annulus_inside() {
    let mut stock = Stock::new();
    stock.building(0, 12.0, &[square(0.0, 0.0, 50.0), square(0.0, 0.0, 20.0)]);
    stock.with_scene(|scene| {
        assert!(!contains(scene, [0.0, 0.0], 5.0), "courtyard centre");
        assert!(
            contains(scene, [35.0, 0.0], 5.0),
            "between the hole and the outer wall"
        );
        assert_eq!(scene.enclosing_building([0.0, 0.0]).unwrap(), None);
        let block = scene.enclosing_building([35.0, 0.0]).unwrap().unwrap();
        assert_eq!(
            block.rings.iter().map(|ring| ring.hole).collect::<Vec<_>>(),
            vec![false, true]
        );
        assert!(block.rings.iter().all(|ring| ring.points.len() == 5));
    });
}

#[test]
fn a_tangent_vertex_keeps_parity() {
    let mut stock = Stock::new();
    stock.building(0, 10.0, &[vec![[0.0, 0.0], [30.0, 40.0], [-30.0, 40.0]]]);
    let apex = stock.steps([0.0, 0.0]);
    stock.with_scene(|scene| {
        // West of the apex on its row: both slanted edges meet the row at the apex, two counts.
        let [_, apex_row] = scene_steps(scene, apex);
        let [west, _] = scene.lattice.steps([-200.0, 0.0]);
        assert!(!scene.contains_built([west, apex_row], 5.0, None).unwrap());
        assert!(contains(scene, [0.0, 20.0], 5.0), "interior");
    });
}

#[test]
fn outdoor_roofs_are_not_buildings_to_click_but_count_for_reflection() {
    let mut stock = Stock::new();
    stock.building_of_class(4, 6.0, EnvelopeClass::Outdoor, &[square(0.0, 0.0, 200.0)]);
    stock.wall(5, 6.0, &[[-10.0, 0.0], [10.0, 0.0]]);
    stock.with_scene(|scene| {
        assert_eq!(scene.enclosing_building([0.0, 0.0]).unwrap(), None);
        assert_eq!(scene.reflection_db([0.0, 0.0], None).unwrap(), 3.0);
    });
}

/// CNOSSOS 2.8 excludes the façade's own reflection: at a receiver 0.1 m in front of a 160 m
/// block, the probes inside that block stop counting, a block across the street still does.
#[test]
fn the_density_bonus_ignores_the_receivers_own_building() {
    let mut stock = Stock::new();
    stock.building(0, 20.0, &[square(-80.1, 0.0, 80.0)]);
    stock.building(1, 20.0, &[square(110.0, 0.0, 90.0)]);
    stock.with_scene(|scene| {
        // Six of nine probes are built: three in the own block, three across the street.
        assert_eq!(scene.reflection_db([0.0, 0.0], None).unwrap(), 3.0);
        assert_eq!(scene.reflection_db([0.0, 0.0], Some(0)).unwrap(), 1.5);
        // Another footprint's id is another building: the own block still counts.
        assert_eq!(scene.reflection_db([0.0, 0.0], Some(1 << 48)).unwrap(), 3.0);
    });
}
