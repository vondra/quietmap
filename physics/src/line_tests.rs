//! Free-field exactness, normalization, the in-plane angle and the wide-bucket nodes of the line
//! quadrature (ported from dev4 with an independent numeric point sum as the reference).

use super::*;

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

fn no_obstacles(_: f64, _: f64, _: f64, _: &mut dyn FnMut(SkylineArc)) {}

fn relative(point: [f64; 3], receiver: [f64; 3]) -> [f64; 3] {
    [
        point[0] - receiver[0],
        point[1] - receiver[1],
        point[2] - receiver[2],
    ]
}

/// Free-field energy of the quadrature per unit L_W': sum(dphi) / (10^1.1 d_perp).
fn free_field_energy(start: [f64; 3], end: [f64; 3], receiver: [f64; 3]) -> f64 {
    let geometry =
        LinePieceGeometry::new(relative(start, receiver), relative(end, receiver)).unwrap();
    let mut nodes = Vec::new();
    line_quadrature_nodes(&geometry, &mut no_obstacles, &mut nodes);
    nodes.iter().map(|node| node.weight_rad).sum::<f64>() * geometry.divergence_factor()
}

/// The incoherent point sum with 20 lg r + 11 per point, by a fine midpoint rule.
fn point_sum_energy(start: [f64; 3], end: [f64; 3], receiver: [f64; 3]) -> f64 {
    let steps = 400_000;
    let along = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let length = (along[0] * along[0] + along[1] * along[1] + along[2] * along[2]).sqrt();
    (0..steps)
        .map(|i| {
            let f = (i as f64 + 0.5) / steps as f64;
            let p: [f64; 3] =
                std::array::from_fn(|axis| start[axis] + f * along[axis] - receiver[axis]);
            1.0 / (p[0] * p[0] + p[1] * p[1] + p[2] * p[2])
        })
        .sum::<f64>()
        * length
        / steps as f64
        / POINT_DIVERGENCE_LINEAR
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
        assert!(
            db(got / exact).abs() < 1e-6,
            "{start:?}->{end:?} at {receiver:?}: {}",
            db(got / exact)
        );
    }
}

/// The point sum sits 10 lg(2 pi^2 / 10^1.1) = 1.9533 dB above the retired -10 lg(2 pi d) +
/// 10 lg(theta/pi) chain; an infinite line is 10 lg d_perp + 6.0285.
#[test]
fn the_line_is_1_9533_db_above_the_retired_two_pi_chain() {
    let (start, end, receiver) = ([-125.0, 0.0, 4.0], [125.0, 0.0, 4.0], [0.0, 1000.0, 4.0]);
    let theta = 2.0 * (125.0_f64 / 1000.0).atan();
    let retired = -db(2.0 * PI * 1000.0) + db(theta / PI);
    let gap = db(free_field_energy(start, end, receiver)) - retired;
    assert!((gap - 1.9533).abs() < 0.0005, "gap {gap:.4} dB");
    let infinite = db(free_field_energy(
        [-1e7, 0.0, 4.0],
        [1e7, 0.0, 4.0],
        [0.0, 100.0, 4.0],
    ));
    assert!((infinite + db(100.0) + 6.0285).abs() < 1e-3, "{infinite}");
}

/// The finite-line angle is the in-plane one: a 10 m line with the receiver 1 m out and 3.95 m
/// up reads 1.90 dB louder with the angle taken in plan.
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

/// Buckets of equal dphi: nodes crowd where the line is near, a far piece keeps one per bucket.
#[test]
fn nodes_are_uniform_in_the_in_plane_angle() {
    let geometry = LinePieceGeometry::new([-125.0, 20.0, -3.95], [125.0, 20.0, -3.95]).unwrap();
    let mut nodes = Vec::new();
    line_quadrature_nodes(&geometry, &mut no_obstacles, &mut nodes);
    assert_eq!(nodes.len(), LINE_BUCKET_COUNT);
    let angles: Vec<f64> = nodes
        .iter()
        .map(|n| geometry.in_plane_angle_at(n.along_m))
        .collect();
    let step = geometry.subtended_angle_rad() / LINE_BUCKET_COUNT as f64;
    for pair in angles.windows(2) {
        assert!((pair[1] - pair[0] - step).abs() < 1e-12);
    }
    assert!(
        (nodes[2].along_m - 125.0).abs() < 1e-9,
        "the middle node faces the receiver"
    );
}

fn wall_scene(nearest_m: f64) -> (LinePieceGeometry, Vec<LineQuadratureNode>) {
    let geometry = LinePieceGeometry::new([-125.0, 20.0, 0.0], [125.0, 20.0, 0.0]).unwrap();
    let wall = SkylineArc {
        lo_rad: 80.0_f64.to_radians(),
        hi_rad: 100.0_f64.to_radians(),
        nearest_m,
    };
    let mut skyline = |lo: f64, hi: f64, _radius: f64, visit: &mut dyn FnMut(SkylineArc)| {
        if wall.hi_rad > lo && wall.lo_rad < hi {
            visit(wall);
        }
    };
    let mut nodes = Vec::new();
    line_quadrature_nodes(&geometry, &mut skyline, &mut nodes);
    (geometry, nodes)
}

/// A wall in front of part of a wide bucket: blocked nodes where it stands, clear ones beside it,
/// weights still adding up to the bucket's dphi. A wall 0.5 m from the receiver blocks the same
/// (no lower distance bound: dev4 read 16 dB loud 0.4 m outside a 6 m wall without it).
#[test]
fn a_wall_in_front_of_a_wide_bucket_places_blocked_and_clear_nodes() {
    for nearest_m in [10.0, 0.5] {
        let (geometry, nodes) = wall_scene(nearest_m);
        assert!(nodes.len() > LINE_BUCKET_COUNT);
        let total: f64 = nodes.iter().map(|n| n.weight_rad).sum();
        assert!((total - geometry.subtended_angle_rad()).abs() < 1e-12);
        let azimuth = |node: &LineQuadratureNode| {
            let p = geometry.point_at(node.along_m);
            p[1].atan2(p[0]).to_degrees()
        };
        let clear: Vec<_> = nodes.iter().filter(|n| !n.obstacles_on_ray).collect();
        assert!(
            !clear.is_empty(),
            "the gaps beside the wall are clear nodes"
        );
        for node in clear {
            let a = azimuth(node);
            assert!(
                !(80.0..=100.0).contains(&a),
                "clear node behind the wall at {a} deg"
            );
        }
        assert!(
            nodes
                .iter()
                .any(|n| n.obstacles_on_ray && (80.0..=100.0).contains(&azimuth(n)))
        );
    }
}
