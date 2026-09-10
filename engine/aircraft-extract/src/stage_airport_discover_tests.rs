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
fn translating_strip_across_dateline_preserves_membership_geometry_and_airport() {
    let vertices = |anchor: f64| {
        (0..60)
            .map(|i| {
                (
                    0.0,
                    grid::geo::normalize_longitude(anchor + f64::from(i - 30) / 65536.0) as f32,
                )
            })
            .collect::<Vec<_>>()
    };
    let baseline = discover_strips(&vertices(0.0), 10.0, 3);
    assert_eq!(baseline.len(), 1);
    for anchor in [-180.0, 180.0] {
        let shifted = discover_strips(&vertices(anchor), 10.0, 3);
        assert_eq!(shifted.len(), 1, "dateline split one physical strip");
        assert_eq!(shifted[0].vertex_count, baseline[0].vertex_count);
        assert_eq!(shifted[0].length_m, baseline[0].length_m);
        assert_eq!(shifted[0].heading_deg, baseline[0].heading_deg);
        assert_eq!(shifted[0].is_line, baseline[0].is_line);
        let areas = [noise_compute::types::AirportArea::new(
            1,
            5,
            "Dateline".into(),
            "SAME".into(),
            0.0,
            anchor,
            Vec::new(),
            0.0,
        )];
        let index = crate::airport_index::AerodromeIndex::build(&areas);
        assert_eq!(
            index
                .nearest(
                    f64::from(shifted[0].center_lat),
                    f64::from(shifted[0].center_lon)
                )
                .map(|a| a.airport_key.as_str()),
            Some("SAME")
        );
    }
}

#[test]
fn dense_strip_keeps_its_exact_shared_latitude() {
    let members: Vec<_> = (0..20_000)
        .map(|i| (50.1_f32, 14.0 + (i % 60) as f32 / 65536.0))
        .collect();
    let old_mean = members.iter().map(|v| v.0).sum::<f32>() / members.len() as f32;
    assert!((old_mean - members[0].0).abs() > 0.009);
    let strip = fit_strip(&members);
    assert_eq!(strip.center_lat, members[0].0);
    assert!(strip.is_line);
}

#[test]
fn repeated_coordinates_preserve_every_member_without_repeated_neighborhood_scans() {
    let vertices: Vec<_> = (0..273_604)
        .map(|i| {
            (
                89.509_8_f32,
                if i % 2 == 0 {
                    -86.108_78_f32
                } else {
                    -86.174_7_f32
                },
            )
        })
        .collect();
    let (labels, clusters) = dbscan_2d(&vertices, 200.0, 5);
    assert_eq!(clusters, 1);
    assert!(labels.iter().all(|label| *label == Some(0)));
    assert_eq!(
        discover_strips(&vertices, 200.0, 5),
        vec![fit_strip(&vertices)]
    );
}

#[test]
fn duplicate_initial_noise_keeps_first_reachable_cluster_border_label() {
    let vertices: Vec<_> = [0.0, 0.0, -15.0, -15.0, -15.0, -9.0, 15.0, 15.0, 15.0, 9.0]
        .into_iter()
        .map(|east| local_at_50n(east, 0.0))
        .collect();
    let (labels, clusters) = dbscan_2d(&vertices, 10.0, 5);
    assert_eq!(clusters, 2);
    assert_eq!(labels, [vec![Some(0); 6], vec![Some(1); 4]].concat());
}
