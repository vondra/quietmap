//! Free-field exactness, normalization (#5), the in-plane angle (#28) and the wide-bucket nodes of
//! the line quadrature.

use super::*;
use crate::propagation::point_sum::{line_nodes, NodeSpacing, POINT_SOURCE_DIVERGENCE_OFFSET_DB};

const COARSE: NodeSpacing = NodeSpacing {
    max_angle_rad: 3.0 * PI / 180.0,
    max_length_m: 250.0,
};

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

fn no_obstacles(_: f64, _: f64, _: f64, _: &mut dyn FnMut(ReceiverSkylineArc)) {}

fn relative(point: [f64; 3], receiver: [f64; 3]) -> [f64; 3] {
    [point[0] - receiver[0], point[1] - receiver[1], point[2] - receiver[2]]
}

/// Free-field energy of the quadrature per unit L_W′: Σ Δφ / (10^1.1 d⊥).
fn free_field_energy(start: [f64; 3], end: [f64; 3], receiver: [f64; 3]) -> f64 {
    let geometry = LinePieceGeometry::new(relative(start, receiver), relative(end, receiver)).unwrap();
    let mut nodes = Vec::new();
    line_quadrature_nodes(&geometry, LineDirectivity::Omnidirectional, &mut no_obstacles, &mut nodes);
    nodes.iter().map(|node| node.weight_rad).sum::<f64>() * geometry.divergence_factor()
}

/// The point sum with `20·lg r + 11` per node, per unit L_W′.
fn point_sum_energy(start: [f64; 3], end: [f64; 3], receiver: [f64; 3]) -> f64 {
    line_nodes(start, end, receiver, COARSE)
        .iter()
        .map(|node| node.length_m / node.slant_distance_m.powi(2))
        .sum::<f64>()
        / 10f64.powf(POINT_SOURCE_DIVERGENCE_OFFSET_DB / 10.0)
}

#[test]
fn free_field_quadrature_is_the_exact_point_sum_on_any_geometry() {
    let scenes: [([f64; 3], [f64; 3], [f64; 3]); 6] = [
        ([-125.0, 0.0, 0.05], [125.0, 0.0, 0.05], [0.0, 50.0, 4.0]),
        ([0.0, 0.0, 0.05], [250.0, 0.0, 0.05], [500.0, 30.0, 4.0]),
        ([-125.0, 0.0, 0.05], [125.0, 0.0, 0.05], [0.0, 1.0, 4.0]),
        ([0.0, 0.0, 0.5], [250.0, 0.0, 12.0], [40.0, 90.0, 4.0]),
        ([2000.0, 0.0, 0.05], [2250.0, 0.0, 0.05], [0.0, 45.0, 4.0]),
        ([-5.0, 0.0, 0.05], [5.0, 0.0, 0.05], [0.0, 1.0, 4.0]),
    ];
    for (start, end, receiver) in scenes {
        let got = free_field_energy(start, end, receiver);
        let exact = point_sum_energy(start, end, receiver);
        assert!(db(got / exact).abs() < 1e-9, "{start:?}->{end:?} at {receiver:?}");
    }
}

/// #5, the deleted H0 fixture B4: the point sum sits `10·lg(2π²/10^1.1)` = 1.9533 dB above the
/// retired `−10·lg(2πd) + 10·lg(θ/π)` line chain; an infinite line is `10·lg d⊥ + 6.03`.
#[test]
fn the_line_is_1_9533_db_above_the_retired_two_pi_chain() {
    let (start, end, receiver) = ([-125.0, 0.0, 4.0], [125.0, 0.0, 4.0], [0.0, 1000.0, 4.0]);
    let theta = 2.0 * (125.0_f64 / 1000.0).atan();
    let retired = -db(2.0 * PI * 1000.0) + db(theta / PI);
    let gap = db(free_field_energy(start, end, receiver)) - retired;
    assert!((gap - 1.9533).abs() < 0.0005, "gap {gap:.4} dB");
    let infinite = db(free_field_energy([-1e7, 0.0, 4.0], [1e7, 0.0, 4.0], [0.0, 100.0, 4.0]));
    assert!((infinite + db(100.0) + 6.0285).abs() < 1e-3, "{infinite}");
}

/// #28: the retired chain took the finite-line angle in plan; a 10 m line with the receiver 1 m
/// out and 3.95 m up reads 1.90 dB louder that way than with the in-plane angle, which the
/// quadrature uses.
#[test]
fn the_in_plane_angle_replaces_the_horizontal_one_beside_a_short_elevated_line() {
    let (start, end, receiver) = ([-5.0, 0.0, 0.05], [5.0, 0.0, 0.05], [0.0, 1.0, 4.0]);
    let slant = (1.0_f64 + 3.95 * 3.95).sqrt();
    let horizontal_angle = 2.0 * 5.0_f64.atan();
    let retired = -db(2.0 * PI * slant) + db(horizontal_angle / PI);
    let normalization = db(2.0 * PI * PI / POINT_DIVERGENCE_LINEAR);
    let excess = retired + normalization - db(free_field_energy(start, end, receiver));
    assert!((excess - 1.899).abs() < 0.002, "{excess:.4} dB");
}

#[test]
fn a_receiver_on_the_line_reads_it_half_a_metre_away() {
    let geometry = LinePieceGeometry::new([-125.0, 0.0, 0.0], [125.0, 0.0, 0.0]).unwrap();
    assert_eq!(geometry.perpendicular_m(), LINE_PERPENDICULAR_FLOOR_M);
    assert!(LinePieceGeometry::new([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]).is_none());
}

/// Buckets of equal Δφ: the nodes crowd where the line is near, and a far piece keeps one node
/// per bucket (none of its buckets is wide).
#[test]
fn nodes_are_uniform_in_the_in_plane_angle() {
    let geometry = LinePieceGeometry::new([-125.0, 20.0, -3.95], [125.0, 20.0, -3.95]).unwrap();
    let mut nodes = Vec::new();
    line_quadrature_nodes(&geometry, LineDirectivity::Omnidirectional, &mut no_obstacles, &mut nodes);
    assert_eq!(nodes.len(), LINE_BUCKET_COUNT);
    let angles: Vec<f64> = nodes.iter().map(|n| geometry.in_plane_angle_at(n.along_m)).collect();
    let step = geometry.subtended_angle_rad() / LINE_BUCKET_COUNT as f64;
    for pair in angles.windows(2) {
        assert!((pair[1] - pair[0] - step).abs() < 1e-12);
    }
    assert!((nodes[2].along_m - 125.0).abs() < 1e-9, "the middle node faces the receiver");
}

/// A wall standing in front of part of a wide bucket: the bucket's single node is replaced by
/// geometry-placed nodes, blocked ones where the wall stands and clear ones beside it, whose
/// weights still add up to the bucket's Δφ.
#[test]
fn a_wall_in_front_of_a_wide_bucket_places_blocked_and_clear_nodes() {
    let geometry = LinePieceGeometry::new([-125.0, 20.0, 0.0], [125.0, 20.0, 0.0]).unwrap();
    // A wall 10 m north of the receiver covering azimuths 80°…100°.
    let wall = ReceiverSkylineArc {
        lo_rad: 80.0_f64.to_radians(),
        hi_rad: 100.0_f64.to_radians(),
        nearest_m: 10.0,
    };
    let mut skyline = |lo: f64, hi: f64, _radius: f64, visit: &mut dyn FnMut(ReceiverSkylineArc)| {
        if wall.hi_rad > lo && wall.lo_rad < hi {
            visit(wall);
        }
    };
    let mut nodes = Vec::new();
    line_quadrature_nodes(&geometry, LineDirectivity::Omnidirectional, &mut skyline, &mut nodes);
    assert!(nodes.len() > LINE_BUCKET_COUNT);
    let total: f64 = nodes.iter().map(|n| n.weight_rad).sum();
    assert!((total - geometry.subtended_angle_rad()).abs() < 1e-12);
    let azimuth = |node: &LineQuadratureNode| {
        let p = geometry.point_at(node.along_m);
        p[1].atan2(p[0]).to_degrees()
    };
    let clear: Vec<_> = nodes.iter().filter(|n| !n.obstacles_on_ray).collect();
    assert!(!clear.is_empty(), "the gaps beside the wall are clear nodes");
    for node in clear {
        let a = azimuth(node);
        assert!(!(80.0..=100.0).contains(&a), "clear node behind the wall at {a}°");
    }
    assert!(nodes.iter().any(|n| n.obstacles_on_ray && (80.0..=100.0).contains(&azimuth(n))));
}

/// The track dipole integrates in closed form: over an infinite line its mean is
/// 0.01 + 0.99/2, and each node's weight is the integral over its own angle interval.
#[test]
fn the_track_dipole_weights_integrate_its_directivity() {
    let half_pi = std::f64::consts::FRAC_PI_2;
    let geometry = LinePieceGeometry::new([-100.0, 5.0, 0.0], [100.0, 5.0, 0.0]).unwrap();
    let whole = LineDirectivity::TrackDipole.weight(&geometry, -half_pi, half_pi);
    assert!((whole / std::f64::consts::PI - 0.505).abs() < 1e-12, "{whole}");
    let steps = 100_000;
    let (a, b) = (-0.3_f64, 1.1_f64);
    let numeric: f64 = (0..steps)
        .map(|i| {
            let phi = a + (b - a) * (i as f64 + 0.5) / steps as f64;
            LineDirectivity::TrackDipole.factor(phi.cos().powi(2)) * (b - a) / steps as f64
        })
        .sum();
    assert!((LineDirectivity::TrackDipole.weight(&geometry, b, a) - numeric).abs() < 1e-9);
    assert_eq!(LineDirectivity::Omnidirectional.weight(&geometry, a, b), b - a);
}

/// Independent spatial point sum: horizontal directivity is evaluated in x/y, while
/// divergence uses the 3D range. Covers both heights, slopes, near-coincident quadratics,
/// a receiver on the track axis, finite off-end lines, rotations and endpoint reversal.
#[test]
fn horizontal_track_dipole_matches_a_spatial_point_sum_above_and_beside_the_track() {
    for (start, end) in [
        ([-100.0, 1.0, -3.5], [100.0, 1.0, -3.5]),
        ([-100.0, 5.0, -3.5], [100.0, 5.0, -3.5]),
        ([-100.0, 0.0, -3.5], [100.0, 0.0, -3.5]),
        ([-100.0, 5.0, 0.001], [100.0, 5.0, 0.001]),
        ([-100.0, 5.0, -10.0], [100.0, 5.0, 10.0]),
        ([200.0, 1.0, -3.5], [400.0, 1.0, 6.5]),
        ([-70.0, -71.0, -3.5], [70.0, 69.0, -3.5]),
        ([100.0, 1.0, -3.5], [-100.0, 1.0, -3.5]),
    ] {
        let geometry = LinePieceGeometry::new(start, end).unwrap();
        let mut nodes = Vec::new();
        line_quadrature_nodes(&geometry, LineDirectivity::TrackDipole, &mut no_obstacles, &mut nodes);
        let got = nodes.iter().map(|node| node.weight_rad).sum::<f64>() * geometry.divergence_factor();
        let along = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
        let horizontal_sq = along[0] * along[0] + along[1] * along[1];
        let steps = 100_000;
        let expected = (0..steps).map(|i| {
            let f = (i as f64 + 0.5) / steps as f64;
            let p = std::array::from_fn::<_, 3, _>(|axis| start[axis] + f * along[axis]);
            let cross = p[0] * along[1] - p[1] * along[0];
            let horizontal_range_sq = p[0] * p[0] + p[1] * p[1];
            let sin_squared = if horizontal_range_sq == 0.0 { 0.0 }
                else { cross * cross / (horizontal_sq * horizontal_range_sq) };
            (0.01 + 0.99 * sin_squared) / p.iter().map(|v| v * v).sum::<f64>()
        }).sum::<f64>() * geometry.length_m() / steps as f64 / POINT_DIVERGENCE_LINEAR;
        assert!(db(got / expected).abs() < 1e-6, "{start:?}->{end:?}: {} dB", db(got / expected));
    }
}
