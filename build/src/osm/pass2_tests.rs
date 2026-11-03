//! Regression cases for pass2.

#[test]
fn outer_building_keeps_its_independent_source_family() {
    use crate::osm::classify::{FeatureType, Tags};
    use crate::osm::relations::{RelationInfo, RelationManifest};
    let manifest = RelationManifest {
        way_to_relations: [
            (1, vec![(2, "outer".to_owned())]),
            (3, vec![(2, "inner".to_owned())]),
        ]
        .into(),
        relations: [(
            2,
            RelationInfo {
                feature_types: vec![FeatureType::Building],
                tags: Tags::new(),
                member_ways: vec![],
            },
        )]
        .into(),
    };
    assert!(super::relation_covers_kind(
        1,
        &FeatureType::Building,
        &manifest
    ));
    assert!(!super::relation_covers_kind(
        1,
        &FeatureType::Industrial,
        &manifest
    ));
    assert!(!super::relation_covers_kind(
        1,
        &FeatureType::Leisure,
        &manifest
    ));
    assert!(!super::relation_covers_kind(
        3,
        &FeatureType::Building,
        &manifest
    ));
}

use super::{centroid, facility_part_tags, ring_for_spill};

#[test]
fn multipolygon_parts_keep_the_full_nameplate_and_carry_their_area_share() {
    use crate::osm::classify::Tags;
    use std::collections::BTreeMap;
    // Two equal closed halves plus one unclosed fragment: the halves keep
    // the full 24 MW nameplate and carry 0.5 each, the fragment keeps the
    // original tags without a share (the spill drops it) and a single
    // part stays byte-identical.
    let half = |lon0: f64| {
        vec![
            [50.0, lon0],
            [50.0, lon0 + 0.0001],
            [50.0001, lon0 + 0.0001],
            [50.0001, lon0],
            [50.0, lon0],
        ]
    };
    let tags: Tags = [("plant:output:electricity".to_string(), "24 MW".to_string())].into();
    let share = |part: &Tags| {
        let btree: BTreeMap<String, String> =
            part.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        btree
            .get("qm:facility_share")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(1.0)
    };
    let parts = facility_part_tags(&[half(14.0), half(14.0002)], &tags);
    assert_eq!(parts.len(), 2);
    for part in &parts {
        assert_eq!(part.get("plant:output:electricity").unwrap(), "24 MW");
        assert!((share(part) - 0.5).abs() < 1e-9, "{}", share(part));
    }
    let double = vec![
        [50.0, 14.0],
        [50.0, 14.0002],
        [50.0001, 14.0002],
        [50.0001, 14.0],
        [50.0, 14.0],
    ];
    let open = vec![[50.0, 14.0], [50.0, 14.0001]];
    let parts = facility_part_tags(&[half(14.0), double, open], &tags);
    // The double-area part carries twice the single-area part (1/3 +
    // 2/3, within snapped-grid rounding: the two rings snap to
    // different cell boundaries, so the ratio is approximate).
    assert!(
        (share(&parts[0]) - 1.0 / 3.0).abs() < 1e-3,
        "{}",
        share(&parts[0])
    );
    assert!(
        (share(&parts[1]) - 2.0 / 3.0).abs() < 1e-3,
        "{}",
        share(&parts[1])
    );
    assert_eq!(parts[2].get("plant:output:electricity").unwrap(), "24 MW");
    assert!(!parts[2].contains_key("qm:facility_share"));
    let single = facility_part_tags(&[half(14.0)], &tags);
    assert_eq!(single[0].get("plant:output:electricity").unwrap(), "24 MW");
    assert!(!single[0].contains_key("qm:facility_share"));
}

#[test]
fn antimeridian_ring_keeps_its_centroid_but_not_unsafe_geometry() {
    let ring = [
        [10.0, 179.99],
        [10.0, -179.99],
        [10.01, -179.99],
        [10.01, 179.99],
        [10.0, 179.99],
    ];
    let (lat, lon) = centroid(&ring);
    assert!((lat - 10.005).abs() < 1e-12);
    assert!((lon + 180.0).abs() < 1e-12, "lon={lon}");
    let mut omitted = 0;
    assert!(ring_for_spill(&ring, &mut omitted).is_none());
    assert_eq!(omitted, 1);
}
