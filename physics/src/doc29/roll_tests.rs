use super::*;

/// Eq. 4-24a and 4-24b: about 0 dB beside the start of roll, a dB more at 120 deg, 13.5 dB less
/// for a jet straight behind (10 dB for a propeller), weakened past 762 m.
#[test]
fn the_start_of_roll_directivity() {
    assert!(start_of_roll_directivity_db(90.0, 100.0, false).abs() < 0.3);
    assert!((start_of_roll_directivity_db(120.0, 100.0, false) - 0.93).abs() < 0.01);
    assert!((start_of_roll_directivity_db(180.0, 100.0, false) + 13.48).abs() < 0.01);
    assert!((start_of_roll_directivity_db(180.0, 100.0, true) + 10.13).abs() < 0.01);
    let near = start_of_roll_directivity_db(170.0, 762.0, false);
    assert!((start_of_roll_directivity_db(170.0, 1_524.0, false) - near / 2.0).abs() < 1e-9);
}

/// Eq. 4-21a: a long segment heard from its start delivers half the infinite line's energy.
#[test]
fn the_end_energy_fraction() {
    assert!((end_energy_fraction(1e6, 100.0) - 0.5).abs() < 1e-4);
    assert!(end_energy_fraction(10.0, 100.0) < end_energy_fraction(100.0, 100.0));
}
