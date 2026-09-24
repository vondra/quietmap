//! Free-field exactness, the two line-chain gaps (normalization #5, horizontal angle #28) and
//! the node composition of the point-sum kernel.

use super::*;
use crate::propagation::geo;

const FINE: NodeSpacing = NodeSpacing {
    max_angle_rad: 0.5 * std::f64::consts::PI / 180.0,
    max_length_m: 2.0,
};
const COARSE: NodeSpacing = NodeSpacing {
    max_angle_rad: 3.0 * std::f64::consts::PI / 180.0,
    max_length_m: 250.0,
};

fn inverse_square_sum(nodes: &[LineNode]) -> f64 {
    nodes
        .iter()
        .map(|node| node.length_m / node.slant_distance_m.powi(2))
        .sum()
}

/// `∫ds/(D² + s²)` over the line, D the 3D perpendicular distance.
fn exact_line_integral(start: [f64; 3], end: [f64; 3], receiver: [f64; 3]) -> f64 {
    let along = sub(end, start);
    let length = norm(along);
    let unit = scale(along, 1.0 / length);
    let foot = dot(sub(receiver, start), unit);
    let lever = norm(sub(receiver, add(start, scale(unit, foot)))).max(FLC_MIN_PERP_M);
    (((length - foot) / lever).atan() - (-foot / lever).atan()) / lever
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

#[test]
fn free_field_node_sum_is_the_exact_line_integral_for_any_mesh() {
    let scenes: [([f64; 3], [f64; 3], [f64; 3]); 7] = [
        ([-125.0, 0.0, 0.05], [125.0, 0.0, 0.05], [0.0, 50.0, 4.0]),
        ([0.0, 0.0, 0.05], [250.0, 0.0, 0.05], [500.0, 30.0, 4.0]),
        ([-125.0, 0.0, 0.05], [125.0, 0.0, 0.05], [0.0, 1.0, 4.0]),
        ([0.0, 0.0, 0.5], [250.0, 0.0, 12.0], [40.0, 90.0, 4.0]),
        ([-100.0, -50.0, 0.05], [100.0, 50.0, 0.05], [40.0, 90.0, 30.0]),
        ([2000.0, 0.0, 0.05], [2250.0, 0.0, 0.05], [0.0, 45.0, 4.0]),
        ([-5.0, 0.0, 0.05], [5.0, 0.0, 0.05], [0.0, 1.0, 4.0]),
    ];
    for (start, end, receiver) in scenes {
        let exact = exact_line_integral(start, end, receiver);
        for spacing in [FINE, COARSE] {
            let got = inverse_square_sum(&line_nodes(start, end, receiver, spacing));
            assert!(db(got / exact).abs() < 1e-9, "{start:?}->{end:?} at {receiver:?}");
        }
    }
}

/// The deleted H0 fixture B4, now against the production line chain itself: over flat ground
/// the point sum sits `10·lg(2π²/10^1.1)` = 1.9533 dB above `−10·lg(2πd) + FLC` (#5).
#[test]
fn point_sum_is_1_9533_db_above_the_production_line_chain() {
    let (start, end, receiver) = ([-125.0, 0.0, 4.0], [125.0, 0.0, 4.0], [0.0, 1000.0, 4.0]);
    let point_sum_db = db(inverse_square_sum(&line_nodes(start, end, receiver, COARSE)))
        - POINT_SOURCE_DIVERGENCE_OFFSET_DB;
    let line_chain_db = -db(2.0 * std::f64::consts::PI * 1000.0)
        + geo::finite_line_correction_for_divergence(250.0, 1000.0, 0.5, 1000.0);
    let gap = point_sum_db - line_chain_db;
    assert!((gap - 1.9533).abs() < 0.001, "gap {gap:.4} dB");
}

/// #28: beside a road the production chain takes the subtended angle in plan while divergence
/// runs on the slant distance. A 10 m line, receiver 1 m out and 3.95 m up: the horizontal
/// angle is 1.90 dB louder than the slant one, which the 1.95 dB normalization gap almost
/// exactly hides (net +0.05 dB).
#[test]
fn horizontal_finite_line_angle_overstates_a_near_line_by_1_90_db() {
    let (start, end, receiver) = ([-5.0, 0.0, 0.05], [5.0, 0.0, 0.05], [0.0, 1.0, 4.0]);
    let slant = (1.0_f64 + 3.95 * 3.95).sqrt();
    let point_sum_db = db(inverse_square_sum(&line_nodes(start, end, receiver, COARSE)))
        - POINT_SOURCE_DIVERGENCE_OFFSET_DB;
    let line_chain_db =
        -db(2.0 * std::f64::consts::PI * slant) + geo::finite_line_correction_for_divergence(10.0, 1.0, 0.5, 1.0);
    let normalization_db = db(2.0 * std::f64::consts::PI.powi(2) / 10f64.powf(1.1));
    let horizontal_angle_excess = line_chain_db + normalization_db - point_sum_db;
    assert!(
        (horizontal_angle_excess - 1.899).abs() < 0.002,
        "{horizontal_angle_excess:.4} dB"
    );
    assert!((point_sum_db - line_chain_db - 0.054).abs() < 0.002);
}

/// H0 fixture B5: an end-on receiver never depends on the placement floor.
#[test]
fn end_on_receiver_is_exact_without_a_perpendicular_lever() {
    let exact = 1.0 / 500.0 - 1.0 / 750.0;
    let got = inverse_square_sum(&line_nodes(
        [500.0, 0.0, 0.0],
        [750.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        COARSE,
    ));
    assert!(db(got / exact).abs() < 1e-4, "{} dB", db(got / exact));
}

#[test]
fn split_pieces_carry_the_energy_of_the_whole() {
    let receiver = [37.0, 44.0, 4.0];
    let whole = inverse_square_sum(&line_nodes([-120.0, 0.0, 0.05], [120.0, 0.0, 0.05], receiver, COARSE));
    let split: f64 = (0..33)
        .map(|index| {
            let x0 = -120.0 + 240.0 * f64::from(index) / 33.0;
            let x1 = -120.0 + 240.0 * f64::from(index + 1) / 33.0;
            inverse_square_sum(&line_nodes([x0, 0.0, 0.05], [x1, 0.0, 0.05], receiver, COARSE))
        })
        .sum();
    assert!((whole / split - 1.0).abs() < 1e-12);
    assert!(line_nodes([1.0, 2.0, 3.0], [1.0, 2.0, 3.0], receiver, COARSE).is_empty());
}

#[test]
fn nodes_lie_on_the_piece_and_honour_the_spacing() {
    let nodes = line_nodes([-125.0, 0.0, 0.05], [125.0, 0.0, 0.05], [0.0, 55.0, 4.0], COARSE);
    assert!(nodes.iter().all(|node| (0.0..=1.0).contains(&node.piece_fraction)));
    assert!(nodes.iter().all(|node| node.position_m[1] == 0.0));
    let span_deg = 2.0 * (125.0_f64 / (55.0_f64.powi(2) + 3.95_f64.powi(2)).sqrt()).atan().to_degrees();
    assert!(nodes.len() >= (span_deg / 3.0).ceil() as usize);
}

#[test]
fn barrier_replaces_ground_per_band_and_divergence_is_the_point_form() {
    let terms = NodePathTerms {
        atmospheric_db: [0.5; NUM_BANDS],
        ground_db: [3.0, 7.0, 3.0, 7.0, 3.0, 7.0, 3.0, -3.0],
        terrain_db: [2.0; NUM_BANDS],
        screening_db: [4.0; NUM_BANDS],
        vegetation_db: [1.0; NUM_BANDS],
    };
    let got = node_attenuation_bands(100.0, &terms);
    let expected = [58.5, 59.5, 58.5, 59.5, 58.5, 59.5, 58.5, 58.5];
    for band in 0..NUM_BANDS {
        assert!((got[band] - expected[band]).abs() < 1e-12, "band {band}");
    }
    let clear = NodePathTerms {
        ground_db: [-3.0; NUM_BANDS],
        ..NodePathTerms::default()
    };
    assert_eq!(node_attenuation_bands(0.2, &clear), [8.0; NUM_BANDS]);
}
