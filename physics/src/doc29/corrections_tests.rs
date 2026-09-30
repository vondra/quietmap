//! Corrections: Doc 29's lateral attenuation values and dev4's speed, finite-segment and
//! installation cases.

use super::*;

#[test]
fn speed_correction_is_zero_at_the_reference_and_3_db_at_half_speed() {
    assert!(speed_correction_db(160.0, 160.0).abs() < 1e-12);
    assert!((speed_correction_db(160.0, 80.0) - 3.0103).abs() < 1e-4);
    assert_eq!(speed_correction_db(160.0, 5.0), 0.0);
}

#[test]
fn finite_segment_correction_alongside_behind_and_infinite() {
    let alongside = finite_segment_correction_db(500.0, 1_000.0, 370.0);
    assert!(alongside < 0.0 && alongside > -10.0, "{alongside}");
    let behind = finite_segment_correction_db(-500.0, 1_000.0, 370.0);
    assert!(behind < -5.0, "{behind}");
    // A segment reaching far both ways carries the infinite line's energy.
    let infinite = finite_segment_correction_db(500_000.0, 1_000_000.0, 370.0);
    assert!(infinite.abs() < 1e-3, "{infinite}");
    // Half of it on either side of the closest point.
    let half = finite_segment_correction_db(0.0, 1_000_000.0, 370.0);
    assert!((half + 3.0103).abs() < 1e-3, "{half}");
}

#[test]
fn lateral_attenuation_is_zero_overhead_and_near_its_peak_at_grazing() {
    assert_eq!(lateral_attenuation_db(1_000.0, 0.0), 0.0);
    let grazing = lateral_attenuation_db(1.0, 2_000.0);
    assert!(grazing > 10.0 && grazing < 10.9, "{grazing}");
}

/// Doc 29 Eq. 4-17/4-18, including a receiver above the aircraft and the far field.
#[test]
fn lateral_attenuation_matches_doc29() {
    for (height_m, lateral_m, expected_db) in [
        (100.0, 1_000.0, 5.326),
        (100.0, 8_000.0, 9.901),
        (-50.0, 200.0, 4.988),
    ] {
        let actual = lateral_attenuation_db(height_m, lateral_m);
        assert!(
            (actual - expected_db).abs() < 0.001,
            "{height_m}/{lateral_m}: {actual}"
        );
    }
}

#[test]
fn lateral_attenuation_is_never_negative() {
    for lateral_m in [0.0, 5.0, 100.0, 913.0, 915.0, 16_000.0] {
        for height_m in [-3_000.0, -1.0, 0.0, 1.0, 50.0, 800.0, 12_000.0] {
            assert!(
                lateral_attenuation_db(height_m, lateral_m) >= 0.0,
                "{height_m}/{lateral_m}"
            );
        }
    }
}

/// Eq. 4-15 stays under its bound at every elevation; fuselage engines only lose, propellers
/// have none.
#[test]
fn installation_correction_peaks_below_its_bound() {
    let mut wing_peak = f64::NEG_INFINITY;
    for tenth_degree in 0..=900 {
        let angle = f64::from(tenth_degree).to_radians() / 10.0;
        let (height_m, slant_m) = (1_000.0 * angle.sin(), 1_000.0);
        wing_peak = wing_peak.max(installation_correction_db(
            Installation::Wing,
            height_m,
            slant_m,
        ));
        assert!(installation_correction_db(Installation::Fuselage, height_m, slant_m) <= 1e-12);
        assert_eq!(
            installation_correction_db(Installation::Propeller, height_m, slant_m),
            0.0
        );
    }
    assert!(
        wing_peak > 0.40 && wing_peak < INSTALLATION_CORRECTION_MAX_DB,
        "{wing_peak}"
    );
    // Below the receiver the correction is read at 0 deg: -1.49 dB for wing engines.
    let below = installation_correction_db(Installation::Wing, -300.0, 1_000.0);
    assert!(
        (below - 10.0 * 0.062 * 0.0039f64.log10()).abs() < 1e-12,
        "{below}"
    );
}
