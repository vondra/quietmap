//! Box sizes: the limit follows the steepest NPD curve, and Prague's bands start at z19.

use super::*;

/// Web-map cell edge (m) at `zoom` and latitude 50.08 deg (Prague).
fn prague(zoom: u8) -> f64 {
    40_075_016.686 * 50.08f64.to_radians().cos() / f64::from(1u32 << zoom)
}

#[test]
fn the_edge_limit_grows_with_clearance() {
    let limits: Vec<f64> = [60.0, 300.0, 1_000.0, 3_000.0, 10_000.0]
        .iter()
        .map(|&clearance| box_edge_limit_m(clearance, BOX_EDGE_LEVEL_STEP_DB))
        .collect();
    assert!(
        limits.windows(2).all(|pair| pair[0] < pair[1]),
        "{limits:?}"
    );
    // 1,000 ft: the steepest class row falls about 30 dB/km there, so 3 dB span about 100 m.
    let at_1000_ft = box_edge_limit_m(304.8, BOX_EDGE_LEVEL_STEP_DB);
    assert!((60.0..130.0).contains(&at_1000_ft), "{at_1000_ft}");
}

#[test]
fn prague_bands_start_at_the_first_layer_and_coarsen_upward() {
    let bands = clearance_bands(prague, 12, 13_000.0, BOX_EDGE_LEVEL_STEP_DB);
    assert_eq!(bands[0].zoom, 19, "{:?}", bands[0]);
    assert!((bands[0].edge_m - 49.0).abs() < 1.0);
    assert_eq!(bands[0].clearance_m, 0.0);
    for pair in bands.windows(2) {
        assert!((pair[1].clearance_m - pair[0].clearance_m - pair[0].edge_m).abs() < 1e-9);
    }
    for band in &bands {
        let spanned = band_edge_limit_m(band.clearance_m, band.clearance_m + band.edge_m);
        assert!(band.edge_m <= spanned || band.zoom == 19, "{band:?}");
    }
    assert!(bands.last().unwrap().zoom <= 16, "{:?}", bands.last());
    let top = bands.last().unwrap();
    assert!(top.clearance_m + top.edge_m >= 13_000.0);
    eprintln!(
        "{} bands: {:?}",
        bands.len(),
        bands
            .iter()
            .map(|band| (band.zoom, band.clearance_m.round()))
            .collect::<Vec<_>>()
    );
}
