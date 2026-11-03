//! Regression cases for microsegment.

use super::{SourcePosition, bearing_deg};
use crate::osm::geometry::flat_dist;

fn split_at_junctions(
    nodes: impl IntoIterator<Item = (Option<[f64; 2]>, bool)>,
    max_length_m: f64,
) -> Vec<([f64; 2], [f64; 2], f32)> {
    let nodes: Vec<_> = nodes.into_iter().collect();
    super::split_at_junctions(nodes.iter().copied(), max_length_m)
        .iter()
        .map(|interval| interval.geometry(|index| nodes[index].0.unwrap()))
        .collect()
}

fn split(coords: &[[f64; 2]], max_length_m: f64) -> Vec<([f64; 2], [f64; 2], f32)> {
    split_at_junctions(
        coords.iter().map(|&point| (Some(point), false)),
        max_length_m,
    )
}

#[test]
fn source_intervals_keep_original_indices_through_merging_splits_and_gaps() {
    let nodes = [
        (Some([0.0, 0.0]), false),
        (Some([0.0, 0.0001]), false),
        (Some([0.0, 0.0001]), false),
        (Some([0.0, 0.0002]), true),
        (Some([0.0, 0.0002]), false),
        (Some([0.0, 0.0062]), false),
        (None, false),
        (Some([0.0, 0.007]), false),
        (Some([0.0, 0.0071]), false),
    ];
    let segments = super::split_at_junctions(nodes, 250.0);
    assert_eq!(segments.len(), 5);
    let interpolated = |fraction_to_next| SourcePosition {
        vertex_index: 4,
        fraction_to_next,
    };
    let expected = [
        (SourcePosition::vertex(0), SourcePosition::vertex(3)),
        (SourcePosition::vertex(3), interpolated(1.0 / 3.0)),
        (interpolated(1.0 / 3.0), interpolated(2.0 / 3.0)),
        (interpolated(2.0 / 3.0), SourcePosition::vertex(5)),
        (SourcePosition::vertex(7), SourcePosition::vertex(8)),
    ];
    assert_eq!(
        segments
            .iter()
            .map(|s| (s.start, s.end))
            .collect::<Vec<_>>(),
        expected
    );
    for interval in segments {
        let (start, end, length) = interval.geometry(|index| nodes[index].0.unwrap());
        assert!((flat_dist(start[0], start[1], end[0], end[1]) - f64::from(length)).abs() < 1e-4);
    }
}

#[test]
fn protected_collinear_source_node_remains_an_exact_segment_endpoint() {
    let coords = [[50.0, 14.0], [50.0, 14.0005], [50.0, 14.001]];
    let merged = split_at_junctions(coords.map(|point| (Some(point), false)), 250.0);
    assert_eq!(merged.len(), 1);
    let protected = split_at_junctions(
        coords
            .into_iter()
            .enumerate()
            .map(|(index, point)| (Some(point), index == 1)),
        250.0,
    );
    assert_eq!(protected.len(), 2);
    assert_eq!((protected[0].0, protected[0].1), (coords[0], coords[1]));
    assert_eq!((protected[1].0, protected[1].1), (coords[1], coords[2]));
}

#[test]
fn repeated_adjacent_source_node_cannot_emit_a_graph_self_loop() {
    let a = [50.0, 14.0];
    let junction = [50.0, 14.0005];
    let b = [50.0, 14.001];
    let segments = split_at_junctions(
        [
            (Some(a), false),
            (Some(junction), true),
            (Some(junction), true),
            (Some(b), false),
        ],
        250.0,
    );
    assert_eq!(segments.len(), 2);
    assert_eq!((segments[0].0, segments[0].1), (a, junction));
    assert_eq!((segments[1].0, segments[1].1), (junction, b));
    assert!(segments.iter().all(|segment| segment.2 > 0.0));
}

#[test]
fn missing_source_node_ends_the_connected_range_without_a_bridge() {
    let a = [50.0, 14.0];
    let b = [50.0002, 14.0];
    let c = [50.0004, 14.0];
    let d = [50.0006, 14.0];
    let segments = split_at_junctions(
        [Some(a), Some(b), None, Some(c), Some(d)].map(|point| (point, false)),
        250.0,
    );
    assert_eq!(
        segments
            .iter()
            .map(|segment| (segment.0, segment.1))
            .collect::<Vec<_>>(),
        vec![(a, b), (c, d)]
    );
}

#[test]
fn interpolation_preserves_source_endpoint_bits_before_grid_quantization() {
    let coords = [[0.1234567, 179.99], [0.1245678, 180.0]];
    let segments = split(&coords, 250.0);
    assert!(segments.len() > 1);
    assert_eq!(segments.first().unwrap().0, coords[0]);
    assert_eq!(segments.last().unwrap().1, coords[1]);
}

fn near(a: f32, b: f32) -> bool {
    let diff = (a - b).abs();
    diff < 0.5 || (360.0 - diff).abs() < 0.5
}

/// 100 m straight line discretised into 11 collinear OSM vertices at
/// 10 m spacing — the merger should emit ONE microsegment.
#[test]
fn collinear_dense_polyline_merges_to_single_segment() {
    let mut coords = Vec::new();
    for i in 0..=10 {
        let t = i as f64 / 10.0;
        coords.push([
            50.0,
            14.0 + t * 100.0 / (111_320.0 * (50.0_f64.to_radians()).cos()),
        ]);
    }
    let segs = split(&coords, 250.0);
    assert_eq!(
        segs.len(),
        1,
        "dense collinear polyline should merge to 1, got {}",
        segs.len()
    );
    assert!(
        (segs[0].2 - 100.0).abs() < 1.0,
        "merged length ~100 m, got {}",
        segs[0].2
    );
}

/// 300 m straight line at 10 m spacing — should emit two segments
/// at the 250 m max-length cap, not 30 segments.
#[test]
fn straight_line_past_max_emits_at_cap() {
    let cos_lat = (50.0_f64.to_radians()).cos();
    let mut coords = Vec::new();
    for i in 0..=30 {
        let t = i as f64 / 30.0;
        coords.push([50.0, 14.0 + t * 300.0 / (111_320.0 * cos_lat)]);
    }
    let segs = split(&coords, 250.0);
    assert_eq!(
        segs.len(),
        2,
        "300 m polyline at max=250 m → 2 segs, got {}",
        segs.len()
    );
    let total: f32 = segs.iter().map(|s| s.2).sum();
    assert!(
        (total - 300.0).abs() < 1.0,
        "total length ~300 m, got {total}"
    );
}

/// A sharp 90° corner mid-polyline breaks the chord tolerance so
/// the walker emits two separate segments at the bend.
#[test]
fn sharp_bend_breaks_walker_at_corner() {
    let cos_lat = (50.0_f64.to_radians()).cos();
    let step_lon = 50.0 / (111_320.0 * cos_lat);
    let step_lat = 50.0 / 110_540.0;
    // Three collinear east-going then three collinear north-going.
    let coords = vec![
        [50.0, 14.0],
        [50.0, 14.0 + step_lon],
        [50.0, 14.0 + 2.0 * step_lon],
        [50.0, 14.0 + 3.0 * step_lon],
        [50.0 + step_lat, 14.0 + 3.0 * step_lon],
        [50.0 + 2.0 * step_lat, 14.0 + 3.0 * step_lon],
    ];
    let segs = split(&coords, 250.0);
    assert!(
        segs.len() >= 2,
        "sharp bend should emit at least 2 segments, got {}",
        segs.len()
    );
}

#[test]
fn long_single_hop_uses_uniform_interpolation() {
    let cos_lat = (50.0_f64.to_radians()).cos();
    let coords = vec![[50.0, 14.0], [50.0, 14.0 + 600.0 / (111_320.0 * cos_lat)]];
    let segs = split(&coords, 250.0);
    assert_eq!(
        segs.len(),
        3,
        "600 m / 250 m max → 3 interpolated segs, got {}",
        segs.len()
    );
    for s in &segs {
        assert!(
            s.2 <= 250.0 + 0.5,
            "interpolated seg length should be ≤ 250 m, got {}",
            s.2
        );
    }
}

#[test]
fn antimeridian_interpolation_stays_short_and_canonical() {
    let segments = split(&[[0.0, 179.997], [0.0, -179.997]], 250.0);
    assert_eq!(segments.len(), 3);
    for (start, end, length_m) in segments {
        assert!((-180.0..180.0).contains(&start[1]), "lon={}", start[1]);
        assert!((-180.0..180.0).contains(&end[1]), "lon={}", end[1]);
        assert!(length_m <= 250.0);
        assert!((flat_dist(start[0], start[1], end[0], end[1]) - f64::from(length_m)).abs() < 0.1);
    }
}

#[test]
fn due_north() {
    assert!(near(bearing_deg(50.0, 14.0, 51.0, 14.0), 0.0));
}

#[test]
fn due_east() {
    assert!(near(bearing_deg(50.0, 14.0, 50.0, 15.0), 90.0));
}

#[test]
fn due_south() {
    assert!(near(bearing_deg(50.0, 14.0, 49.0, 14.0), 180.0));
}

#[test]
fn due_west() {
    assert!(near(bearing_deg(50.0, 14.0, 50.0, 13.0), 270.0));
}

#[test]
fn lkpr_rwy06_designator_matches_geometry() {
    // LKPR RWY 06 threshold at (50.103, 14.236), other end at
    // (50.118, 14.286). Designator "06" → magnetic bearing ~060°.
    // Without cos(lat) scaling, raw atan2 would read ~70° at 50°N.
    let bearing = bearing_deg(50.103, 14.236, 50.118, 14.286);
    assert!(
        (bearing - 60.0).abs() < 5.0,
        "expected ~60°, got {bearing}°"
    );
}
