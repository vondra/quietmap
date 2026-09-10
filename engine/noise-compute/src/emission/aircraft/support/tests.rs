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
