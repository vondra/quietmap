//! Screening: dev4's edge-loss rule, its cap and smooth shadow boundary, and the composition.

use super::*;

#[test]
fn the_loss_is_zero_at_grazing_and_capped_at_18_db() {
    assert_eq!(path_difference_loss_db(0.0), 0.0);
    assert_eq!(path_difference_loss_db(-1.0), 0.0);
    // About 6.4 m of path difference saturate the cap.
    let six_metres = 10.0 * ((3.0 + 29.2 * 6.0) / 3.0f64).log10();
    assert!((path_difference_loss_db(6.0) - six_metres).abs() < 1e-12);
    assert_eq!(path_difference_loss_db(6.5), SCREENING_CAP_DB);
    // A near-unity-tangent wall 500 m out, the aircraft 3 km away at the receiver's height.
    assert_eq!(edge_loss_db(500.0, 500.0, 3_000.0, 0.0), SCREENING_CAP_DB);
}

#[test]
fn an_edge_screens_only_between_receiver_and_source_above_the_line_of_sight() {
    // A 100 m ridge 1 km out.
    assert!(edge_loss_db(1_000.0, 100.0, 2_000.0, 150.0) > 5.0);
    // Beyond the aircraft, or at it: the ridge cannot screen.
    assert_eq!(edge_loss_db(1_000.0, 100.0, 800.0, 20.0), 0.0);
    assert_eq!(edge_loss_db(1_000.0, 100.0, 1_000.0, 20.0), 0.0);
    // Below the line of sight, or on it.
    assert_eq!(edge_loss_db(1_000.0, 100.0, 2_000.0, 250.0), 0.0);
    assert_eq!(edge_loss_db(1_000.0, 100.0, 2_000.0, 200.0), 0.0);
    // Directly overhead there is no horizontal path to cut.
    assert_eq!(edge_loss_db(1_000.0, 100.0, 0.0, 500.0), 0.0);
}

/// Sweeping the elevation in 0.1 deg steps across an edge's horizon angle, the loss steps by less
/// than 0.5 dB and meets 0 at the boundary (no hard cut).
#[test]
fn the_loss_is_smooth_across_the_shadow_boundary() {
    let (edge_range_m, edge_height_m, source_range_m) = (158.0, 20.0, 5_000.0);
    let losses: Vec<f64> = (0..=10)
        .map(|step| {
            let elevation = (6.7 + 0.1 * f64::from(step)).to_radians();
            let source_height_m = source_range_m * elevation.tan();
            edge_loss_db(edge_range_m, edge_height_m, source_range_m, source_height_m)
        })
        .collect();
    assert!(losses.iter().any(|&loss| loss > 0.0) && losses.contains(&0.0));
    for pair in losses.windows(2) {
        assert!((pair[1] - pair[0]).abs() < 0.5, "{losses:?}");
    }
}

/// AEDT: screening and lateral attenuation are mutually exclusive, the larger applies; terrain
/// and buildings compete by maximum, never adding.
#[test]
fn screening_competes_with_lateral_attenuation_by_maximum() {
    assert_eq!(screened_sel_db(80.0, 5.0, 18.0, 0.0), 67.0);
    assert_eq!(screened_sel_db(80.0, 5.0, 3.0, 0.0), 80.0);
    assert_eq!(screened_sel_db(80.0, 5.0, 18.0, 18.0), 67.0);
    assert_eq!(screened_sel_db(80.0, 5.0, 2.0, 12.0), 73.0);
}
