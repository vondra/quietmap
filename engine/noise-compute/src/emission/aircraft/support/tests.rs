//! The periodic envelope tells short arcs from aggregate bounds at the seam.

use super::*;

#[test]
fn periodic_selection_distinguishes_short_arcs_from_aggregate_bounds() {
    let seam = AirborneEnvelope::new(0.001, 179.5);
    let opposite = AirborneEnvelope::new(0.001, 0.0);
    let bbox = [0.0, -179.0, 0.0, 179.0];
    assert!(seam.intersects_bbox(bbox));
    assert!(opposite.intersects_bbox(bbox));
    assert!(seam.intersects_segment([0.0, 179.0], [0.0, -179.0]));
    assert!(!opposite.intersects_segment([0.0, 179.0], [0.0, -179.0]));
    for lon in [-180.0, 180.0, 540.0] {
        let envelope = AirborneEnvelope::new(0.0, lon);
        assert!(envelope.intersects_segment([0.0, 179.99], [0.0, -179.99]));
        assert!(!envelope.intersects_segment([0.0, -179.85], [0.0, -179.75]));
        assert!(!envelope.intersects_bbox([0.0, -0.01, 0.0, 0.01]));
        assert!(!envelope.intersects_bbox([1.0, -179.0, 2.0, 179.0]));
    }
    // The existing shortest-delta convention chooses the negative half-turn.
    assert!(opposite.intersects_segment([0.0, 90.0], [0.0, -90.0]));
    assert!(!AirborneEnvelope::new(0.0, 180.0).intersects_segment([0.0, 90.0], [0.0, -90.0]));
}

#[test]
fn receiver_union_keeps_quantized_edges_and_periodic_seam() {
    for points in [
        [[51.64423, -0.50124], [51.663719, -0.491731]],
        [
            [80.000_000_1, 179.999_999_9],
            [79.999_999_9, -179.999_999_9],
        ],
    ] {
        let union = AirborneEnvelope::covering_receivers(&points).unwrap();
        for [lat, lon] in points {
            let point = AirborneEnvelope::new(lat, lon);
            for [west, east] in point.longitude_intervals {
                for edge in [west, east] {
                    for latitude in [point.south, point.north] {
                        let vertex = [latitude, edge];
                        if point.intersects_segment(vertex, vertex) {
                            assert!(union.intersects_segment(vertex, vertex));
                        }
                    }
                }
            }
        }
    }
    assert!(AirborneEnvelope::covering_receivers(&[]).is_none());
}
