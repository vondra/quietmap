//! Free-field exactness and placement of the point-sum nodes.

use super::*;

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
    let lever = norm(sub(receiver, add(start, scale(unit, foot)))).max(LINE_PERPENDICULAR_FLOOR_M);
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

