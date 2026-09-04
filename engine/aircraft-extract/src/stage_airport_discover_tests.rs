//! Regression tests for stage airport discover behavior.

use super::*;

/// Convert local (east_m, north_m) offsets at 50°N anchor into a
/// lat/lon pair, sharing the same constants as production code.
fn local_at_50n(east_m: f32, north_m: f32) -> (f32, f32) {
    let dlon = east_m / (M_PER_DEG_LON_EQUATOR * (50.0f32.to_radians()).cos());
    let dlat = north_m / M_PER_DEG_LAT;
    (50.0 + dlat, 14.0 + dlon)
}

/// Synthetic east-west runway: 1000 m line at 50°N, 60 points
/// strung along it with 5 m perp jitter.
fn synthetic_runway() -> Vec<(f32, f32)> {
    (0..60)
        .map(|i| {
            let along = i as f32 * 1000.0 / 60.0 - 500.0; // -500..+500 m
            let perp = ((i * 13) % 11) as f32 - 5.0; // -5..+5 m jitter
            local_at_50n(along, perp)
        })
        .collect()
}

/// Synthetic blob: 60 points scattered in a ~100 × 100 m square
/// using coprime-of-100 strides so successive points don't land on
/// the same row/column — should not look like a runway.
fn synthetic_blob() -> Vec<(f32, f32)> {
    (0..60)
        .map(|i| {
            let dx = ((i * 7) % 100) as f32 - 50.0;
            let dy = ((i * 13) % 100) as f32 - 50.0;
            local_at_50n(dx, dy)
        })
        .collect()
}

#[test]
fn discovers_runway_as_line() {
    let pts = synthetic_runway();
    let out = discover_strips(&pts, 50.0, 5);
    assert_eq!(out.len(), 1, "expected one cluster from runway pts");
    let strip = out[0];
    assert!(strip.is_line, "elongated cluster should classify as line");
    assert!(
        (strip.length_m - 1000.0).abs() < 50.0,
        "runway length ~1000m, got {}",
        strip.length_m
    );
    // East-west runway: bearing 90° (east) or 270° (west); PCA
    // doesn't disambiguate direction → accept either.
    let h = strip.heading_deg;
    assert!(
        (h - 90.0).abs() < 5.0 || (h - 270.0).abs() < 5.0,
        "expected E-W bearing, got {h}"
    );
}

#[test]
fn isotropic_blob_classifies_as_non_line() {
    let pts = synthetic_blob();
    let out = discover_strips(&pts, 30.0, 5);
    assert!(!out.is_empty());
    let strip = out[0];
    assert!(
        !strip.is_line,
        "isotropic blob should not classify as line; ratio fired anyway"
    );
}

#[test]
fn empty_input_no_clusters() {
    let out = discover_strips(&[], 50.0, 5);
    assert!(out.is_empty());
}

#[test]
fn below_min_samples_no_clusters() {
    let pts = vec![(50.0, 14.0); 3];
    let out = discover_strips(&pts, 50.0, 30);
    assert!(out.is_empty(), "3 points cannot form a min=30 cluster");
}

#[test]
fn two_distant_clusters_distinguished() {
    let mut pts = synthetic_runway();
    // Second runway 10 km north.
    let dlat_10km = 10_000.0 / 110_540.0;
    for &(la, lo) in &synthetic_runway() {
        pts.push((la + dlat_10km, lo));
    }
    let out = discover_strips(&pts, 50.0, 5);
    assert_eq!(out.len(), 2, "two clusters separated by 10 km");
}

#[test]
fn width_clamped_to_band() {
    // 60 points with 80 m perp spread → should clamp to upper 60 m.
    let pts: Vec<_> = (0..60)
        .map(|i| {
            let along = i as f32 * 1000.0 / 60.0 - 500.0;
            let perp = if i % 2 == 0 { 40.0 } else { -40.0 };
            local_at_50n(along, perp)
        })
        .collect();
    let out = discover_strips(&pts, 100.0, 5);
    assert!(!out.is_empty());
    assert_eq!(out[0].width_m, 60.0, "perp 80m should clamp to 60");
}

#[test]
fn width_clamped_to_floor() {
    // 60 points strung along a 1000 m line with ~1 m perp jitter
    // → raw width ~2 m should clamp UP to the 10 m floor.
    let pts: Vec<_> = (0..60)
        .map(|i| {
            let along = i as f32 * 1000.0 / 60.0 - 500.0;
            let perp = if i % 2 == 0 { 1.0 } else { -1.0 };
            local_at_50n(along, perp)
        })
        .collect();
    let out = discover_strips(&pts, 50.0, 5);
    assert!(!out.is_empty());
    assert_eq!(
        out[0].width_m, 10.0,
        "tight 2m spread should clamp up to 10"
    );
}
