use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(tag: &str) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("qm-unmodelled-{tag}-{stamp}.bin"))
}

fn z30(lat: f64, lon: f64) -> (i32, i32) {
    let phi = lat.clamp(-MAX_LATITUDE_DEG, MAX_LATITUDE_DEG).to_radians();
    let x = WGS84_A_M * lon.to_radians();
    let y = WGS84_A_M * (std::f64::consts::FRAC_PI_4 + phi / 2.0).tan().ln();
    let cell = |m: f64| ((m / Z30_QUANTUM_M).floor() as i64 + (1 << 29)) as i32;
    (cell(x), cell(y))
}

fn rect(lat0: f64, lon0: f64, lat1: f64, lon1: f64) -> Vec<(i32, i32)> {
    vec![
        z30(lat0, lon0),
        z30(lat0, lon1),
        z30(lat1, lon1),
        z30(lat1, lon0),
    ]
}

#[test]
fn round_trip_distance_hole_reach_and_nested_suppression() {
    let path = temp_path("roundtrip");
    // 10 km x 10 km outer range around (50.0, 14.0) with a 2 km x 2 km enclave hole at its centre.
    let outer = rect(49.95, 13.93, 50.05, 14.07);
    let hole = rect(49.99, 13.985, 50.01, 14.015);
    let parent = SiteRecord {
        group_key: 0x0000_0000_0000_0001,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryTrainingArea,
        name: "Parent Training Area".into(),
        area_m2: 100_000_000.0,
        reach_m: notification_reach_m(100_000_000.0),
        covered_by: Vec::new(),
        parts: vec![SitePart {
            rings: vec![outer, hole],
        }],
    };
    // Small child range covered by the parent.
    let child = SiteRecord {
        group_key: 0x0000_0000_0000_0002,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryRange,
        name: "Inner Subrange".into(),
        area_m2: 500_000.0,
        reach_m: notification_reach_m(500_000.0),
        covered_by: vec![0x0000_0000_0000_0001],
        parts: vec![SitePart {
            rings: vec![rect(49.96, 13.94, 49.97, 13.95)],
        }],
    };
    write(&path, &[parent, child]).unwrap();

    // Inside solid ground -> inside=true, distance=0, covered child suppressed.
    let inside = query_file(&path, 49.965, 13.945).unwrap();
    assert!(inside.reads > 0 && inside.bytes > 0);
    assert_eq!(inside.sites.len(), 1);
    assert_eq!(inside.sites[0].group_key, 1);
    assert!(inside.sites[0].inside);
    assert_eq!(inside.sites[0].distance_m, 0.0);

    // Inside the enclave hole -> NOT inside! Positive distance to hole edge (~1.0–1.1 km).
    let in_hole = query_file(&path, 50.0, 14.0).unwrap().sites;
    assert_eq!(in_hole.len(), 1);
    assert_eq!(in_hole[0].group_key, 1);
    assert!(!in_hole[0].inside);
    assert!(in_hole[0].distance_m > 900.0 && in_hole[0].distance_m < 1_300.0);

    // 5 km north of outer edge -> inside 10 km reach of large parent, outside 3 km reach of child.
    let outside = query_file(&path, 50.095, 14.0).unwrap().sites;
    assert_eq!(outside.len(), 1);
    assert!(!outside[0].inside);
    assert!(outside[0].distance_m > 4_500.0 && outside[0].distance_m < 5_500.0);

    // 15 km north -> beyond 10 km reach.
    assert!(query_file(&path, 50.19, 14.0).unwrap().sites.is_empty());

    std::fs::remove_file(&path).unwrap();
}

#[test]
fn covering_parent_outside_hole_never_hides_site_containing_click() {
    let path = temp_path("hole-safety");
    let parent = SiteRecord {
        group_key: 0x10,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryTrainingArea,
        name: "Outer With Hole".into(),
        area_m2: 100_000_000.0,
        reach_m: notification_reach_m(100_000_000.0),
        covered_by: Vec::new(),
        parts: vec![SitePart {
            rings: vec![
                rect(49.95, 13.93, 50.05, 14.07),
                rect(49.99, 13.985, 50.01, 14.015),
            ],
        }],
    };
    let hole_site = SiteRecord {
        group_key: 0x11,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryRange,
        name: "Range In Hole".into(),
        area_m2: 1_000_000.0,
        reach_m: notification_reach_m(1_000_000.0),
        // Even if marked covered_by parent, a click strictly inside hole_site while outside parent
        // must keep hole_site visible and sorted first.
        covered_by: vec![0x10],
        parts: vec![SitePart {
            rings: vec![rect(49.995, 13.995, 50.005, 14.005)],
        }],
    };
    write(&path, &[parent, hole_site]).unwrap();
    let hits = query_file(&path, 50.0, 14.0).unwrap().sites;
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].group_key, 0x11);
    assert!(hits[0].inside);
    assert_eq!(hits[1].group_key, 0x10);
    assert!(!hits[1].inside);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn boundary_edges_are_symmetric_and_equator_distance_uses_wgs84() {
    let path = temp_path("edges-wgs84");
    let ring = rect(0.0, 10.0, 0.01, 10.01);
    // Exact decoded corner coordinates of the quantized ring.
    let sw = z30_to_lonlat(ring[0].0, ring[0].1);
    let ne = z30_to_lonlat(ring[2].0, ring[2].1);
    let site = SiteRecord {
        group_key: 0x20,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryRange,
        name: "Equatorial Box".into(),
        area_m2: 1_200_000.0,
        reach_m: notification_reach_m(1_200_000.0),
        covered_by: Vec::new(),
        parts: vec![SitePart { rings: vec![ring] }],
    };
    write(&path, &[site]).unwrap();

    // All four boundary midpoints must consistently report inside=true, distance_m=0.
    let mid_lat = 0.5 * (sw[1] + ne[1]);
    let mid_lon = 0.5 * (sw[0] + ne[0]);
    for (lat, lon) in [
        (mid_lat, sw[0]),
        (mid_lat, ne[0]),
        (sw[1], mid_lon),
        (ne[1], mid_lon),
    ] {
        let hits = query_file(&path, lat, lon).unwrap().sites;
        assert_eq!(hits.len(), 1, "edge ({lat}, {lon})");
        assert!(hits[0].inside, "edge ({lat}, {lon}) should be inside");
        assert_eq!(hits[0].distance_m, 0.0);
    }

    // 0.027° north of the north edge at the equator is ~2,985.5 m on WGS84 (< 3,000 m reach).
    let eq_north = query_file(&path, ne[1] + 0.027, mid_lon).unwrap().sites;
    assert_eq!(eq_north.len(), 1);
    assert!(!eq_north[0].inside);
    assert!(
        eq_north[0].distance_m > 2_980.0 && eq_north[0].distance_m < 2_992.0,
        "equator WGS84 distance was {}",
        eq_north[0].distance_m
    );

    std::fs::remove_file(&path).unwrap();
}

#[test]
fn absent_file_is_empty_and_corrupt_or_unsorted_files_error() {
    let missing = temp_path("missing");
    let empty = query_file(&missing, 50.0, 14.0).unwrap();
    assert!(empty.sites.is_empty());
    assert_eq!((empty.reads, empty.bytes), (0, 0));

    let corrupt = temp_path("corrupt");
    std::fs::write(&corrupt, b"qmunm1\n\0\x01\x00\x00\x00").unwrap();
    assert!(query_file(&corrupt, 50.0, 14.0).is_err());
    std::fs::remove_file(&corrupt).unwrap();

    // Unsorted square table must return an error rather than silently missing a square.
    let unsorted = temp_path("unsorted");
    let s1 = SiteRecord {
        group_key: 1,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryRange,
        name: "A".into(),
        area_m2: 1_000_000.0,
        reach_m: 3_000,
        covered_by: Vec::new(),
        parts: vec![SitePart {
            rings: vec![rect(10.0, 10.0, 10.01, 10.01)],
        }],
    };
    let s2 = SiteRecord {
        group_key: 2,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryRange,
        name: "B".into(),
        area_m2: 1_000_000.0,
        reach_m: 3_000,
        covered_by: Vec::new(),
        parts: vec![SitePart {
            rings: vec![rect(50.0, 14.0, 50.01, 14.01)],
        }],
    };
    write(&unsorted, &[s1, s2]).unwrap();
    let mut raw = std::fs::read(&unsorted).unwrap();
    // Swap the first two 12-byte square directory entries right after the 24-byte header.
    let mut first = [0u8; 12];
    let mut second = [0u8; 12];
    first.copy_from_slice(&raw[24..36]);
    second.copy_from_slice(&raw[36..48]);
    raw[24..36].copy_from_slice(&second);
    raw[36..48].copy_from_slice(&first);
    std::fs::write(&unsorted, &raw).unwrap();
    assert!(query_file(&unsorted, 50.005, 14.005).is_err());
    std::fs::remove_file(&unsorted).unwrap();

    // Implausible part_count in a site record must fail validation without aborting on allocation.
    let bogus = temp_path("bogus-count");
    let s = SiteRecord {
        group_key: 3,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryRange,
        name: "C".into(),
        area_m2: 1_000_000.0,
        reach_m: 3_000,
        covered_by: Vec::new(),
        parts: vec![SitePart {
            rings: vec![rect(50.0, 14.0, 50.01, 14.01)],
        }],
    };
    write(&bogus, &[s]).unwrap();
    let mut raw = std::fs::read(&bogus).unwrap();
    let square_count = u32_at(&raw, 12) as usize;
    let postings_count = u32_at(&raw, 16) as usize;
    let site_blob_start = 24 + square_count * 12 + postings_count * 4 + 2 * 4;
    // Overwrite part_count (at offset 36 inside the site blob, since covered_count == 0) with u32::MAX.
    raw[site_blob_start + 36..site_blob_start + 40].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&bogus, &raw).unwrap();
    assert!(query_file(&bogus, 50.005, 14.005).is_err());
    std::fs::remove_file(&bogus).unwrap();
}

#[test]
fn antimeridian_and_small_reach_boundaries() {
    let path = temp_path("dateline");
    // Small range (< 5 km²) just west of +180° -> reach 3 km spans into -179.98°.
    let site = SiteRecord {
        group_key: 0x0000_0000_0000_00aa,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryDangerArea,
        name: "Dateline Range".into(),
        area_m2: 1_000_000.0,
        reach_m: notification_reach_m(1_000_000.0),
        covered_by: Vec::new(),
        parts: vec![SitePart {
            rings: vec![rect(10.0, 179.985, 10.01, 179.995)],
        }],
    };
    write(&path, &[site]).unwrap();

    // Point 1.5 km east across the antimeridian at -179.992° must match.
    let across = query_file(&path, 10.005, -179.992).unwrap().sites;
    assert_eq!(across.len(), 1);
    assert!(!across[0].inside);
    assert!(across[0].distance_m > 1_000.0 && across[0].distance_m < 2_000.0);

    // Point 4 km away exceeds the 3 km small-site reach.
    assert!(query_file(&path, 10.05, 179.99).unwrap().sites.is_empty());

    std::fs::remove_file(&path).unwrap();
}
