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
        parts: vec![SitePart {
            rings: vec![outer, hole],
        }],
    };
    // Small child range strictly inside the solid south-west corner of the parent.
    let child = SiteRecord {
        group_key: 0x0000_0000_0000_0002,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryRange,
        name: "Inner Subrange".into(),
        area_m2: 500_000.0,
        reach_m: notification_reach_m(500_000.0),
        parts: vec![SitePart {
            rings: vec![rect(49.96, 13.94, 49.97, 13.95)],
        }],
    };
    write(&path, &[parent, child]).unwrap();

    // Inside solid ground -> inside=true, distance=0, nested child suppressed.
    let inside = query_file(&path, 49.965, 13.945).unwrap();
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].group_key, 1);
    assert!(inside[0].inside);
    assert_eq!(inside[0].distance_m, 0.0);

    // Inside the enclave hole -> NOT inside! Positive distance to hole edge (~1.0–1.1 km).
    let in_hole = query_file(&path, 50.0, 14.0).unwrap();
    assert_eq!(in_hole.len(), 1);
    assert_eq!(in_hole[0].group_key, 1);
    assert!(!in_hole[0].inside);
    assert!(in_hole[0].distance_m > 900.0 && in_hole[0].distance_m < 1_300.0);

    // 5 km north of outer edge -> inside 10 km reach of large parent, outside 3 km reach of child.
    let outside = query_file(&path, 50.095, 14.0).unwrap();
    assert_eq!(outside.len(), 1);
    assert!(!outside[0].inside);
    assert!(outside[0].distance_m > 4_500.0 && outside[0].distance_m < 5_500.0);

    // 15 km north -> beyond 10 km reach.
    assert!(query_file(&path, 50.19, 14.0).unwrap().is_empty());

    std::fs::remove_file(&path).unwrap();
}

#[test]
fn absent_file_is_empty_and_truncated_file_errors() {
    let missing = temp_path("missing");
    assert!(query_file(&missing, 50.0, 14.0).unwrap().is_empty());

    let corrupt = temp_path("corrupt");
    std::fs::write(&corrupt, b"qmunm1\n\0\x01\x00\x00\x00").unwrap();
    assert!(query_file(&corrupt, 50.0, 14.0).is_err());
    std::fs::remove_file(&corrupt).unwrap();
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
        parts: vec![SitePart {
            rings: vec![rect(10.0, 179.985, 10.01, 179.995)],
        }],
    };
    write(&path, &[site]).unwrap();

    // Point 1.5 km east across the antimeridian at -179.992° must match.
    let across = query_file(&path, 10.005, -179.992).unwrap();
    assert_eq!(across.len(), 1);
    assert!(!across[0].inside);
    assert!(across[0].distance_m > 1_000.0 && across[0].distance_m < 2_000.0);

    // Point 4 km away exceeds the 3 km small-site reach.
    assert!(query_file(&path, 10.05, 179.99).unwrap().is_empty());

    std::fs::remove_file(&path).unwrap();
}

#[test]
fn shared_boundary_subsector_is_suppressed_but_adjacent_touching_range_is_kept() {
    let path = temp_path("shared-boundary");
    let parent = SiteRecord {
        group_key: 0x10,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryTrainingArea,
        name: "Large Estate".into(),
        area_m2: 100_000_000.0,
        reach_m: notification_reach_m(100_000_000.0),
        parts: vec![SitePart {
            rings: vec![rect(50.0, 14.0, 50.1, 14.14)],
        }],
    };
    // Sub-sector sharing the south and west boundaries with the parent.
    let shared_child = SiteRecord {
        group_key: 0x11,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryTrainingArea,
        name: "Sector SW".into(),
        area_m2: 6_000_000.0,
        reach_m: notification_reach_m(6_000_000.0),
        parts: vec![SitePart {
            rings: vec![rect(50.0, 14.0, 50.03, 14.04)],
        }],
    };
    // Adjacent range sharing the east border outside the parent: must NOT be suppressed.
    let adjacent = SiteRecord {
        group_key: 0x12,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryDangerArea,
        name: "Neighbouring Range".into(),
        area_m2: 10_000_000.0,
        reach_m: notification_reach_m(10_000_000.0),
        parts: vec![SitePart {
            rings: vec![rect(50.0, 14.14, 50.05, 14.20)],
        }],
    };
    // Tiny external sliver 2 m south of parent's fence (~5 m wide): must NOT be suppressed.
    let sliver = SiteRecord {
        group_key: 0x13,
        layer: Layer::Industry,
        kind: UnmodelledKind::MilitaryRange,
        name: "Outside Sliver".into(),
        area_m2: 500.0,
        reach_m: notification_reach_m(500.0),
        parts: vec![SitePart {
            rings: vec![rect(49.99993, 14.05, 49.99998, 14.051)],
        }],
    };
    write(&path, &[parent, shared_child, adjacent, sliver]).unwrap();

    // Near the south-east corner: parent, adjacent and outside sliver stay; shared_child suppressed.
    let hits = query_file(&path, 49.995, 14.05).unwrap();
    let keys: Vec<u64> = hits.iter().map(|h| h.group_key).collect();
    assert!(keys.contains(&0x10), "parent must remain: {keys:?}");
    assert!(
        !keys.contains(&0x11),
        "shared-boundary sub-sector must be suppressed: {keys:?}"
    );
    assert!(keys.contains(&0x12), "adjacent range must remain: {keys:?}");
    assert!(
        keys.contains(&0x13),
        "external sliver must remain: {keys:?}"
    );

    std::fs::remove_file(&path).unwrap();
}
