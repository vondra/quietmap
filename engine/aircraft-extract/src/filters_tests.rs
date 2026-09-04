//! Telemetry, terrain-anomaly and dateline regression cases.
use super::*;

fn pt(ts: f64, lat: f32, lon: f32, alt_ft: f32, baro: f32, flags: u8) -> TracePoint {
    TracePoint {
        timestamp: ts,
        lat,
        lon,
        alt_ft,
        speed_kt: 250.0,
        track_deg: 90.0,
        baro_rate_fpm: baro,
        flags,
    }
}

#[test]
fn nonfinite_telemetry_is_rejected_before_identity_or_energy_math() {
    for bad in [f32::NAN, f32::INFINITY, -1.0] {
        let mut p = pt(1000.0, 50.0, 14.0, 1000.0, 0.0, 0);
        p.speed_kt = bad;
        assert!(!point_is_sane(&p));
    }
    let mut p = pt(f64::NAN, 50.0, 14.0, 1000.0, 0.0, 0);
    assert!(!point_is_sane(&p));
    p.timestamp = u32::MAX as f64 + 1.0;
    assert!(!point_is_sane(&p));
    assert!(!segment_is_keepable(
        f32::NAN,
        5.0,
        100.0,
        100.0,
        250.0,
        0,
        true
    ));
}

#[test]
fn dateline_crossing_is_not_a_teleport() {
    let mut points = vec![
        pt(1.0, 1.0, 179.999, 10000.0, 0.0, 0),
        pt(6.0, 1.0, -179.999, 10000.0, 0.0, 0),
    ];
    let mut agl = vec![3000.0; 2];
    let mut elev = vec![0.0; 2];
    validate_flight_trajectory(&mut points, &mut agl, &mut elev);
    assert_eq!(points.len(), 2);
}

#[test]
fn point_is_sane_drops_zero_lat_lon() {
    let p = pt(0.0, 0.0, 0.0, 1000.0, 0.0, 0);
    assert!(!point_is_sane(&p));
}

#[test]
fn point_is_sane_drops_huge_altitude_when_airborne() {
    let p = pt(0.0, 50.0, 14.0, 80_000.0, 0.0, 0);
    assert!(!point_is_sane(&p));
}

#[test]
fn point_is_sane_keeps_subsea_aerodrome_when_on_ground() {
    // Dead Sea / Bet She'an — alt < 0 is real on the ground.
    let p = pt(
        0.0,
        32.5,
        35.5,
        0.0,
        0.0,
        crate::trace::FLAG_ALT_IS_GROUND | crate::trace::FLAG_ON_GROUND_RAW,
    );
    assert!(point_is_sane(&p));
}

#[test]
fn point_is_sane_drops_nan() {
    let p = pt(0.0, f32::NAN, 14.0, 1000.0, 0.0, 0);
    assert!(!point_is_sane(&p));
}

#[test]
fn validate_truncates_at_underground_anomaly() {
    let mut points = vec![
        pt(0.0, 50.0, 14.0, 5000.0, 0.0, 0),
        pt(5.0, 50.001, 14.001, 4000.0, 0.0, 0),
        pt(10.0, 50.002, 14.002, 3000.0, 0.0, 0),
        pt(15.0, 50.003, 14.003, 0.0, 0.0, 0), // start of fake tail
        pt(20.0, 50.004, 14.004, -5000.0, 0.0, 0),
    ];
    // Synthetic AGL: last two points underground.
    let mut agl = vec![1500.0, 1000.0, 800.0, -100.0, -500.0];
    let mut elev = vec![0.0f32; agl.len()];
    validate_flight_trajectory(&mut points, &mut agl, &mut elev);
    // Cuts at the first agl < HARD_AGL_FLOOR (-300m), which is
    // index 4 (the -500 point). Backtracks through index 3 (-100 m
    // is also negative) to keep index 3 only if AGL >= 0. Here
    // index 2 has 800m → keep first 3 points.
    assert_eq!(points.len(), 3);
    assert_eq!(agl.len(), 3);
    assert_eq!(elev.len(), 3, "elev_m must follow agl truncation");
}

#[test]
fn validate_truncates_at_sustained_descent() {
    let mut points = vec![
        pt(0.0, 50.0, 14.0, 5000.0, -1000.0, 0),
        pt(5.0, 50.001, 14.001, 4500.0, -8500.0, 0),
        pt(10.0, 50.002, 14.002, 4000.0, -9000.0, 0),
        pt(15.0, 50.003, 14.003, 3500.0, -8500.0, 0),
        pt(20.0, 50.004, 14.004, 3000.0, -8500.0, 0),
    ];
    let mut agl = vec![1500.0, 1000.0, 600.0, 200.0, 100.0];
    let mut elev = vec![0.0f32; agl.len()];
    validate_flight_trajectory(&mut points, &mut agl, &mut elev);
    assert!(points.len() <= 1, "got {}", points.len());
}

#[test]
fn teleport_drop_compares_against_last_kept_point_and_keeps_parallel_data() {
    for (end_lat, end_lon, expected) in [
        (50.001, 14.001, vec![0, 1, 3]),
        (70.001, 30.001, vec![0, 1]),
    ] {
        let mut points = vec![
            pt(0.0, 50.0, 14.0, 5000.0, 0.0, 0),
            pt(5.0, 50.0, 14.0, 5500.0, 0.0, 0),
            pt(7.0, 70.0, 30.0, 5500.0, 0.0, 0),
            pt(9.0, end_lat, end_lon, 5500.0, 0.0, 0),
        ];
        let mut agl = vec![1500.0, 1501.0, 1502.0, 1503.0];
        let mut elev = vec![10.0, 11.0, 12.0, 13.0];
        validate_flight_trajectory(&mut points, &mut agl, &mut elev);
        assert_eq!(points.len(), expected.len());
        assert_eq!(
            agl,
            expected
                .iter()
                .map(|i| 1500.0 + *i as f32)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            elev,
            expected
                .iter()
                .map(|i| 10.0 + *i as f32)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn segment_is_keepable_rejects_short_taxi_remnant() {
    assert!(!segment_is_keepable(5.0, 1.0, 0.0, 0.0, 250.0, 0, true));
}

#[test]
fn segment_is_keepable_rejects_supersonic_teleport() {
    // 200 km / 30 s = 24 000 kt — mode-S decode error.
    assert!(!segment_is_keepable(
        200_000.0, 30.0, 0.0, 0.0, 250.0, 0, true
    ));
}

#[test]
fn segment_is_keepable_keeps_legitimate_oceanic_segment() {
    // 200 km / 30 min = 400 kt — real cruise across an ADS-B
    // coverage hole.
    assert!(segment_is_keepable(
        200_000.0, 1800.0, 10_500.0, 10_500.0, 450.0, 0, true
    ));
}

#[test]
fn segment_is_keepable_rejects_nonpositive_dt() {
    assert!(!segment_is_keepable(
        1000.0, 0.0, 100.0, 200.0, 250.0, 0, true
    ));
    assert!(!segment_is_keepable(
        1000.0, -5.0, 100.0, 200.0, 250.0, 0, true
    ));
}

#[test]
fn segment_is_keepable_rejects_underground() {
    assert!(!segment_is_keepable(
        1000.0, 5.0, -500.0, -400.0, 250.0, 0, true
    ));
}

#[test]
fn segment_is_keepable_keeps_sane_jet_segment() {
    assert!(segment_is_keepable(
        1000.0, 5.0, 100.0, 200.0, 250.0, 0, true
    ));
}

#[test]
fn segment_is_keepable_rejects_helicopter_above_ceiling() {
    let heli = noise_compute::emission::aircraft::profile_idx("EC35");
    // Spike: one endpoint at FL250+ → reject (typical mode-S decode error).
    assert!(!segment_is_keepable(
        1000.0, 5.0, 200.0, 7_500.0, 80.0, heli, true
    ));
    // Sustained legitimate civil ops at FL130 over 3 km terrain ≈ 1 km AGL → keep.
    assert!(segment_is_keepable(
        1000.0, 5.0, 800.0, 1_000.0, 80.0, heli, true
    ));
    // Right at ceiling — keep (strict > comparison, matches HARD_AGL_FLOOR convention).
    assert!(segment_is_keepable(
        1000.0, 5.0, 4_000.0, 5_000.0, 80.0, heli, true
    ));
    // Same-altitude jet at 7.5 km is not affected by the heli filter.
    let jet = noise_compute::emission::aircraft::profile_idx("B738");
    assert!(segment_is_keepable(
        1000.0, 5.0, 200.0, 7_500.0, 250.0, jet, true
    ));
}
