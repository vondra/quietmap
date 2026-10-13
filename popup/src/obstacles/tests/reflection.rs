//! Reflection area, continuity and agreement across obstacle tiles and receiver frames.

use super::*;
use crate::obstacles::reflection_coverage::REFLECTION_LATTICE_STEPS;
use physics::reflection::{REFLECTION_WINDOW_M, receiver_reflection_db};

fn direct_lattice_reflection(scene: &Scene, receiver: [f64; 2], own: Option<u64>) -> f64 {
    let spacing = REFLECTION_LATTICE_STEPS as f64;
    let centre = scene.lattice.steps(receiver);
    let half = scene.reflection_window_half_steps(centre);
    let low = [0, 1].map(|axis| (centre[axis] - half[axis]) / spacing);
    let high = [0, 1].map(|axis| (centre[axis] + half[axis]) / spacing);
    let mut covered = 0.0;
    for row in low[1].floor() as i64..high[1].ceil() as i64 {
        for column in low[0].floor() as i64..high[0].ceil() as i64 {
            let weight = (high[0].min(column as f64 + 1.0) - low[0].max(column as f64))
                * (high[1].min(row as f64 + 1.0) - low[1].max(row as f64));
            let probe = [column, row].map(|index| (index as f64 + 0.5) * spacing);
            if scene.contains_built(probe, 5.0, own).unwrap() {
                covered += weight;
            }
        }
    }
    receiver_reflection_db(covered / ((high[0] - low[0]) * (high[1] - low[1])))
}

#[test]
fn tall_building_union_keeps_holes_overlaps_and_own_facade_exclusion() {
    let mut stock = Stock::new();
    stock.building(
        0,
        20.0,
        &[square(-25.0, 0.0, 45.0), square(-25.0, 0.0, 18.0)],
    );
    stock.building(1, 12.0, &[square(25.0, 0.0, 45.0)]);
    stock.building(2, 5.0, &[square(0.0, 0.0, 200.0)]);
    stock.wall(3, 20.0, &[[-200.0, -20.0], [200.0, 20.0]]);
    stock.building_of_class(4, 8.0, EnvelopeClass::Outdoor, &[square(80.0, 0.0, 15.0)]);
    stock.with_scene(|scene| {
        scene
            .prepare_reflection_coverage([-10.0, -10.0], [60.0, 10.0])
            .unwrap();
        for receiver in [[0.0, 0.0], [9.3, -4.1], [52.0, 0.0]] {
            for own in [None, Some(0), Some(1), Some(1 << 48)] {
                let actual = scene.reflection_db(receiver, own).unwrap();
                let expected = direct_lattice_reflection(scene, receiver, own);
                assert!(
                    (actual - expected).abs() < 1e-9,
                    "{receiver:?} {own:?}: {actual} vs {expected}"
                );
            }
        }
        let without_own = scene.reflection_db([0.0, 0.0], Some(0)).unwrap();
        assert!(without_own > 0.0);
        assert!(without_own < scene.reflection_db([0.0, 0.0], None).unwrap());
    });
}

#[test]
fn crossing_lattice_cells_cannot_step_the_bonus() {
    let mut stock = Stock::new();
    stock.building(0, 12.0, &[rectangle(-100.0, -100.0, -10.0, 100.0)]);
    stock.with_scene(|scene| {
        scene
            .prepare_reflection_coverage([-10.0, 0.0], [31.0, 0.0])
            .unwrap();
        let mut previous = scene.reflection_db([-10.0, 0.0], None).unwrap();
        for metre in -9..=30 {
            let point = [f64::from(metre), 0.0];
            let current = scene.reflection_db(point, None).unwrap();
            // Differentiating the nine Bernoulli trials bounds the gain slope by 9 * 1.5 dB.
            let bound = 9.0 * REFLECTION_MAX_DB / 2.0 / REFLECTION_WINDOW_M;
            assert!((current - previous).abs() <= bound + 1e-10);
            let nearby = scene
                .reflection_db([point[0] + 1e-6, point[1]], None)
                .unwrap();
            assert!((nearby - current).abs() <= bound * 1e-6 + 1e-10);
            previous = current;
        }
    });
}

#[test]
fn tile_seams_and_the_antimeridian_use_the_same_coverage_in_every_frame() {
    for (latitude, longitude) in [(0.0, 0.0), (0.0, 180.0), (50.0, 14.0)] {
        let mut stock = Stock::at(latitude, longitude);
        stock.building(7, 12.0, &[rectangle(-110.0, -110.0, -5.0, 110.0)]);
        let files = stock.files();
        let origin = stock.frame.to_mercator([17.4, 8.1]);
        let other_frame = LocalFrame::at(origin);
        let mut original = Scene::new(stock.frame);
        let mut moved = Scene::new(other_frame);
        for (tile, bytes) in &files {
            match bytes {
                Some(bytes) => {
                    original.insert(*tile, Obstacles::parse(bytes).unwrap());
                    moved.insert(*tile, Obstacles::parse(bytes).unwrap());
                }
                None => {
                    original.insert_empty(*tile);
                    moved.insert_empty(*tile);
                }
            }
        }
        original
            .prepare_reflection_coverage([-2.0, -1.0], [13.0, 5.0])
            .unwrap();
        moved
            .prepare_reflection_coverage([-30.0, -30.0], [30.0, 30.0])
            .unwrap();
        for receiver in [[-1.0, 0.0], [0.0, 0.0], [1.0, 0.0], [12.3, 4.5]] {
            let actual = original.reflection_db(receiver, None).unwrap();
            assert!((actual - direct_lattice_reflection(&original, receiver, None)).abs() < 1e-9);
            let position = stock.frame.to_mercator(receiver);
            let relocated = moved
                .reflection_db(other_frame.to_metres(position), None)
                .unwrap();
            assert!((actual - relocated).abs() < 1e-6, "{actual} vs {relocated}");
        }
    }
}

#[test]
fn cropping_the_area_table_cannot_change_a_receiver_or_answer_outside_its_extent() {
    let mut stock = Stock::new();
    stock.building(
        0,
        20.0,
        &[square(-25.0, 0.0, 45.0), square(-25.0, 0.0, 18.0)],
    );
    stock.building(1, 12.0, &[square(25.0, 0.0, 45.0)]);
    stock.building(2, 12.0, &[square(400.0, 0.0, 20.0)]);
    let positions = [[0.0, 0.0], [9.3, -4.1], [52.0, 0.0]];
    let broad = stock.with_scene(|scene| {
        scene
            .prepare_reflection_coverage([-500.0; 2], [500.0; 2])
            .unwrap();
        positions.map(|receiver| {
            [None, Some(0), Some(1)].map(|own| scene.reflection_db(receiver, own).unwrap())
        })
    });
    stock.with_scene(|scene| {
        assert!(scene.reflection_db(positions[0], None).is_err());
        scene
            .prepare_reflection_coverage([0.0, -5.0], [52.0, 0.0])
            .unwrap();
        for (index, receiver) in positions.into_iter().enumerate() {
            for (choice, own) in [None, Some(0), Some(1)].into_iter().enumerate() {
                assert_eq!(
                    scene.reflection_db(receiver, own).unwrap().to_bits(),
                    broad[index][choice].to_bits()
                );
            }
        }
        assert!(scene.reflection_db([54.0, 0.0], None).is_err());
    });
}
