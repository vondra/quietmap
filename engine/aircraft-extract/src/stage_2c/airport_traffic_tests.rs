//! Ground projection and conservation regressions.
use super::*;

fn segment(
    osm_id: u64,
    segment_idx: u16,
    sla: f32,
    slo: f32,
    ela: f32,
    elo: f32,
) -> AirportLineSegment {
    AirportLineSegment {
        osm_id,
        segment_idx,
        start_lat: sla,
        start_lon: slo,
        end_lat: ela,
        end_lon: elo,
        grid: (
            grid::lonlat_to_grid(slo as f64, sla as f64),
            grid::lonlat_to_grid(elo as f64, ela as f64),
        ),
        length_m: flat_dist(sla, slo, ela, elo),
        aeroway_type: 1, // taxi default for projection tests
    }
}

#[test]
fn leg_parallel_to_segment_fully_inside_buffer_returns_leg_length() {
    let s = segment(1, 0, 50.105, 14.255, 50.106, 14.258);
    let out =
        project_leg_onto_airport_lines(s.start_lat, s.start_lon, s.end_lat, s.end_lon, &[s], 50.0);
    assert_eq!(out.len(), 1);
    let leg_len = flat_dist(s.start_lat, s.start_lon, s.end_lat, s.end_lon);
    assert!(
        (out[0].length_within_segment_m - leg_len).abs() < 0.5,
        "expected ~{leg_len} m, got {}",
        out[0].length_within_segment_m
    );
}

#[test]
fn leg_far_from_segment_returns_zero() {
    let s = segment(1, 0, 50.105, 14.255, 50.106, 14.258);
    let out = project_leg_onto_airport_lines(50.115, 14.255, 50.116, 14.258, &[s], 50.0);
    assert!(out.is_empty(), "leg far above should not overlap");
}

#[test]
fn leg_perpendicular_crossing_returns_short_overlap() {
    // Segment east-west; leg perpendicular through midpoint.
    // Buffer is 50 m wide perpendicular, so a perpendicular leg
    // that crosses the segment ON-axis spans the full 100 m
    // perp-buffer height (-50..+50 m).
    let s = segment(1, 0, 50.105, 14.250, 50.105, 14.260);
    // Leg from south to north through the segment midpoint.
    let mid_lat = (s.start_lat + s.end_lat) * 0.5;
    let mid_lon = (s.start_lon + s.end_lon) * 0.5;
    let north_offset_m = 100.0;
    let dlat = north_offset_m / M_PER_DEG_LAT;
    let out = project_leg_onto_airport_lines(
        mid_lat - dlat,
        mid_lon,
        mid_lat + dlat,
        mid_lon,
        &[s],
        50.0,
    );
    assert_eq!(out.len(), 1);
    assert!(
        (out[0].length_within_segment_m - 100.0).abs() < 1.0,
        "expected ~100 m perpendicular overlap, got {}",
        out[0].length_within_segment_m
    );
}

#[test]
fn leg_partially_outside_segment_clipped_at_endpoint() {
    // Segment along latitude 50.105 from lon 14.250 to 14.260
    // (~715 m east-west at 50°N). Leg starts 100 m before the
    // west end and runs 100 m past it on-axis. Overlap should
    // be ~100 m (only the inside half).
    let s = segment(1, 0, 50.105, 14.250, 50.105, 14.260);
    let cos50 = (50.105f32.to_radians()).cos();
    let dlon_100m = 100.0 / (M_PER_DEG_LON_EQUATOR * cos50);
    let out = project_leg_onto_airport_lines(
        50.105,
        s.start_lon - dlon_100m,
        50.105,
        s.start_lon + dlon_100m,
        &[s],
        50.0,
    );
    assert_eq!(out.len(), 1);
    assert!(
        (out[0].length_within_segment_m - 100.0).abs() < 1.0,
        "expected ~100 m on-axis overlap inside the segment, got {}",
        out[0].length_within_segment_m
    );
}

#[test]
fn leg_offset_within_buffer_returns_leg_length() {
    // Leg parallel to segment, offset by 30 m (< 50 m max_perp).
    let s = segment(1, 0, 50.105, 14.250, 50.105, 14.260);
    let dlat_30m = 30.0 / M_PER_DEG_LAT;
    let out = project_leg_onto_airport_lines(
        50.105 + dlat_30m,
        14.252,
        50.105 + dlat_30m,
        14.258,
        &[s],
        50.0,
    );
    assert_eq!(out.len(), 1);
    let expected = flat_dist(50.105, 14.252, 50.105, 14.258);
    assert!(
        (out[0].length_within_segment_m - expected).abs() < 0.5,
        "30 m parallel offset should not reduce overlap; expected {expected}, got {}",
        out[0].length_within_segment_m
    );
}

#[test]
fn leg_offset_beyond_buffer_returns_zero() {
    // Leg parallel to segment, offset by 60 m (> 50 m max_perp).
    let s = segment(1, 0, 50.105, 14.250, 50.105, 14.260);
    let dlat_60m = 60.0 / M_PER_DEG_LAT;
    let out = project_leg_onto_airport_lines(
        50.105 + dlat_60m,
        14.252,
        50.105 + dlat_60m,
        14.258,
        &[s],
        50.0,
    );
    assert!(out.is_empty(), "60 m perpendicular offset > 50 m max_perp");
}

#[test]
fn leg_crossing_multiple_segments_distributes_length() {
    // Three abutting east-west segments, leg runs through all three.
    let s1 = segment(1, 0, 50.105, 14.250, 50.105, 14.253);
    let s2 = segment(1, 1, 50.105, 14.253, 50.105, 14.256);
    let s3 = segment(1, 2, 50.105, 14.256, 50.105, 14.259);
    let out = project_leg_onto_airport_lines(50.105, 14.250, 50.105, 14.259, &[s1, s2, s3], 50.0);
    assert_eq!(out.len(), 3, "leg should hit all three segments");
    let total: f32 = out.iter().map(|h| h.length_within_segment_m).sum();
    let leg_len = flat_dist(50.105, 14.250, 50.105, 14.259);
    // Sum across the three abutting segments ≈ leg length within
    // ~1 m due to flat-Earth + clip rounding.
    assert!(
        (total - leg_len).abs() < 2.0,
        "expected sum ~{leg_len}, got {total}"
    );
}

#[test]
fn degenerate_zero_length_segment_skipped() {
    let s = segment(1, 0, 50.105, 14.255, 50.105, 14.255);
    let out = project_leg_onto_airport_lines(50.105, 14.255, 50.106, 14.258, &[s], 50.0);
    assert!(out.is_empty(), "zero-length seg must produce no overlap");
}

#[test]
fn degenerate_zero_length_leg_returns_no_overlap() {
    let s = segment(1, 0, 50.105, 14.250, 50.105, 14.260);
    let out = project_leg_onto_airport_lines(50.105, 14.255, 50.105, 14.255, &[s], 50.0);
    assert!(out.is_empty(), "point leg must produce no overlap");
}

#[test]
fn parallel_adjacent_taxiways_normalized_to_leg_length() {
    // Two parallel east-west "taxiways" 80 m apart, both within
    // 2 * max_perp_m (100 m) of a leg running halfway between them
    // (40 m from each). Pre-2.3b-fix: both segs would return full
    // overlap → total = 2 * leg_len. Post-fix: normalization scales
    // each to half so total = leg_len, preserving energy.
    let s1 = segment(1, 0, 50.105, 14.250, 50.105, 14.260);
    let dlat_80m = 80.0 / M_PER_DEG_LAT;
    let s2 = segment(2, 0, 50.105 + dlat_80m, 14.250, 50.105 + dlat_80m, 14.260);
    // Leg parallel to both, midway between (40 m from each).
    let dlat_40m = 40.0 / M_PER_DEG_LAT;
    let leg_sla = 50.105 + dlat_40m;
    let leg_ela = leg_sla;
    let out = project_leg_onto_airport_lines(leg_sla, 14.252, leg_ela, 14.258, &[s1, s2], 50.0);
    assert_eq!(out.len(), 2, "leg between two parallel segs must hit both");
    let leg_len = flat_dist(leg_sla, 14.252, leg_ela, 14.258);
    let total: f32 = out.iter().map(|h| h.length_within_segment_m).sum();
    assert!(
        (total - leg_len).abs() < 1.0,
        "double-overlap must normalize to leg_len; got total={total}, leg_len={leg_len}"
    );
    // Each should be ~ leg_len / 2.
    for h in &out {
        assert!(
            (h.length_within_segment_m - leg_len * 0.5).abs() < 1.0,
            "each seg should get half the leg; got {}",
            h.length_within_segment_m
        );
    }
}

#[test]
fn oblique_45_degree_crossing_returns_buffer_diagonal() {
    // Leg crosses an east-west segment at 45°. Inside the
    // 50 m perp-buffer the diagonal slice length is
    // 2 * max_perp / sin(45°) ≈ 141.4 m if the leg is long
    // enough to span the buffer. Catches transposition errors in
    // the (p,q) edge table where parallel/perpendicular tests
    // can't distinguish.
    let s = segment(1, 0, 50.105, 14.250, 50.105, 14.260);
    // Leg from south-west to north-east through midpoint at 45°.
    let mid_lat = (s.start_lat + s.end_lat) * 0.5;
    let mid_lon = (s.start_lon + s.end_lon) * 0.5;
    let offset_m = 200.0;
    let cos_mid = (mid_lat as f64).to_radians().cos() as f32;
    let dlat = offset_m / M_PER_DEG_LAT;
    let dlon = offset_m / (M_PER_DEG_LON_EQUATOR * cos_mid);
    let out = project_leg_onto_airport_lines(
        mid_lat - dlat,
        mid_lon - dlon,
        mid_lat + dlat,
        mid_lon + dlon,
        &[s],
        50.0,
    );
    assert_eq!(out.len(), 1);
    // 45° entry through y=±50 buffer → length = 100·sqrt(2) ≈ 141.4 m
    let expected = 100.0 * std::f32::consts::SQRT_2;
    assert!(
        (out[0].length_within_segment_m - expected).abs() < 2.0,
        "45° crossing diagonal ≈ {expected} m, got {}",
        out[0].length_within_segment_m
    );
}

#[test]
fn taxiway_90_degree_turn_distributes_across_two_segments() {
    // Two segments meeting at a 90° corner: s1 east-west,
    // s2 north-south. A leg following the curve (diagonal from
    // SW end of s1 to NE end of s2 via the corner) should hit
    // both segments. Total normalized length ≤ leg_len; gaps
    // and overlaps at the corner are policy-resolved by the
    // kernel's energy-preservation normalization.
    let s1 = segment(1, 0, 50.105, 14.250, 50.105, 14.260);
    let s2 = segment(1, 1, 50.105, 14.260, 50.110, 14.260);
    // Leg from middle of s1 (sweeping east) to middle of s2
    // (going north). Approximate by a single diagonal leg.
    let leg_sla = 50.105;
    let leg_ela = 50.108;
    let leg_slo = 14.255;
    let leg_elo = 14.260;
    let out = project_leg_onto_airport_lines(leg_sla, leg_slo, leg_ela, leg_elo, &[s1, s2], 50.0);
    assert!(
        !out.is_empty(),
        "leg through 90° turn must hit at least one seg"
    );
    let leg_len = flat_dist(leg_sla, leg_slo, leg_ela, leg_elo);
    let total: f32 = out.iter().map(|h| h.length_within_segment_m).sum();
    assert!(
        total <= leg_len + 0.5,
        "normalized total ({total}) must not exceed leg_len ({leg_len})"
    );
}

#[test]
fn antimeridian_crossing_preserves_ground_length() {
    let s = segment(1, 0, 50.105, 179.999, 50.105, -179.999);
    let out =
        project_leg_onto_airport_lines(s.start_lat, s.start_lon, s.end_lat, s.end_lon, &[s], 50.0);
    assert_eq!(out.len(), 1);
    assert!((out[0].length_within_segment_m - s.length_m).abs() < 0.5);
}

#[test]
fn lkpr_runway_06_sanity() {
    // LKPR runway 06/24 ≈ bearing 60°. A taxi leg parallel to it
    // at 20 m offset should yield full overlap.
    let s = segment(42, 0, 50.105, 14.255, 50.106, 14.258);
    // Offset perpendicular to seg axis by 20 m. Seg bearing ≈ 60°
    // from north → perp bearing ≈ 150°. Approximate via small dlat/dlon.
    let perp_offset_lat = 0.00009; // ~10 m north
    let perp_offset_lon = -0.000156; // ~10 m west @ 50°N (cos≈0.643)
    let out = project_leg_onto_airport_lines(
        s.start_lat + perp_offset_lat,
        s.start_lon + perp_offset_lon,
        s.end_lat + perp_offset_lat,
        s.end_lon + perp_offset_lon,
        &[s],
        50.0,
    );
    assert_eq!(out.len(), 1, "parallel leg should overlap");
    assert!(
        out[0].length_within_segment_m > s.length_m * 0.9,
        "expected ~full overlap, got {} of {}",
        out[0].length_within_segment_m,
        s.length_m
    );
}
