//! Site and leisure rows retain geometry kind, ratings and lifecycle evidence.

use super::*;

#[test]
fn motorsport_two_node_line_is_not_an_area_and_indoor_shooting_keeps_its_tags() {
    let (root, square) = fixture("sports", |spill| {
        for (id, pairs, ring) in [
            (
                41,
                vec![("highway", "raceway"), ("sport", "karting")],
                Some(vec![[50.0, 14.0], [50.0, 14.001]]),
            ),
            (
                42,
                vec![
                    ("sport", "shooting"),
                    ("building", "yes"),
                    ("indoor", "yes"),
                ],
                None,
            ),
        ] {
            spill
                .emit_polygon(
                    &FeatureType::Leisure,
                    crate::osm::geometry::square_of(50.0, 14.0),
                    id,
                    if id == 42 { "node" } else { "way" },
                    50.0,
                    14.0,
                    &tags(&pairs),
                    ring.as_deref(),
                )
                .unwrap();
        }
    });
    for batch in read(&square.join("leisure.arrow")) {
        let ids = batch
            .column_by_name("osm_id")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        for row in 0..batch.num_rows() {
            let sport = batch
                .column_by_name("sport")
                .unwrap()
                .as_any()
                .downcast_ref::<UInt8Array>()
                .unwrap()
                .value(row);
            if ids.value(row) == 41 {
                assert_eq!(sport, 10);
                assert!(batch.column_by_name("area_m2").unwrap().is_null(row));
                assert_eq!(
                    batch
                        .column_by_name("geom")
                        .unwrap()
                        .as_any()
                        .downcast_ref::<BinaryArray>()
                        .unwrap()
                        .value(row)
                        .len(),
                    20
                );
                assert_eq!(
                    batch
                        .column_by_name("geometry_kind")
                        .unwrap()
                        .as_any()
                        .downcast_ref::<UInt8Array>()
                        .unwrap()
                        .value(row),
                    2
                );
            } else {
                assert_eq!(sport, 11);
                let raw: Tags =
                    serde_json::from_str(evidence_tags(&batch, "leisure", row).unwrap()).unwrap();
                assert_eq!(raw["indoor"], "yes");
                assert_eq!(raw["building"], "yes");
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn plant_generator_ratings_and_lifecycle_round_trip() {
    let (root, square) = fixture("power", |spill| {
        for (id, pairs) in [
            (
                51,
                vec![
                    ("power", "plant"),
                    ("plant:source", "solar"),
                    ("plant:output:electricity", "24 MW"),
                ],
            ),
            (
                52,
                vec![
                    ("power", "transformer"),
                    ("rating", "25 MVA"),
                    ("voltage", "110000;22000"),
                ],
            ),
            (53, vec![("landuse", "quarry"), ("abandoned", "yes")]),
        ] {
            spill
                .emit_polygon(
                    &FeatureType::Industrial,
                    crate::osm::geometry::square_of(50.0, 14.0),
                    id,
                    "node",
                    50.0,
                    14.0,
                    &tags(&pairs),
                    None,
                )
                .unwrap();
        }
    });
    let mut classes = Vec::new();
    for batch in read(&square.join("industrial.arrow")) {
        let values = batch
            .column_by_name("source_type")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap();
        for row in 0..batch.num_rows() {
            let raw: Tags =
                serde_json::from_str(evidence_tags(&batch, "industrial", row).unwrap()).unwrap();
            match values.value(row) {
                13 => assert_eq!(raw["plant:output:electricity"], "24 MW"),
                15 => assert_eq!(raw["rating"], "25 MVA"),
                12 => assert_eq!(raw["abandoned"], "yes"),
                _ => panic!("generic source"),
            }
            classes.push(values.value(row));
        }
    }
    classes.sort();
    assert_eq!(classes, vec![12, 13, 15]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_closed_industrial_and_leisure_rings_emit_areas() {
    let closed = vec![[50.0, 14.0], [50.0, 14.001], [50.001, 14.001], [50.0, 14.0]];
    let near_closed = vec![
        [50.0, 14.0],
        [50.0, 14.001],
        [50.001, 14.001],
        [50.0000001, 14.0],
    ];
    let (root, square) = fixture("closed-areas", |spill| {
        for family in [FeatureType::Industrial, FeatureType::Leisure] {
            for (id, kind, ring) in [
                (1, "way", Some(closed.clone())),
                (2, "relation", Some(closed.clone())),
                (3, "way", Some(closed[..3].to_vec())),
                (4, "relation", Some(closed[..3].to_vec())),
                (5, "relation", Some(closed[..2].to_vec())),
                (6, "way", None),
                (7, "node", None),
            ] {
                spill
                    .emit_polygon(
                        &family,
                        crate::osm::geometry::square_of(50.0, 14.0),
                        id,
                        kind,
                        50.0,
                        14.0,
                        &Tags::new(),
                        ring.as_deref(),
                    )
                    .unwrap();
            }
        }
        let tags = tags(&[("highway", "raceway"), ("sport", "motor")]);
        for (id, kind, ring) in [
            (8, "way", &closed[..2]),
            (9, "way", closed.as_slice()),
            (10, "relation", &closed[..3]),
        ] {
            spill
                .emit_polygon(
                    &FeatureType::Leisure,
                    crate::osm::geometry::square_of(50.0, 14.0),
                    id,
                    kind,
                    50.0,
                    14.0,
                    &tags,
                    Some(ring),
                )
                .unwrap();
        }
        for (id, kind, ring, area) in [
            (11, "way", &closed[..2], None),
            (12, "way", closed.as_slice(), None),
            (13, "way", &closed[..3], Some("yes")),
            (14, "way", closed.as_slice(), Some("yes")),
            (15, "relation", &closed[..3], None),
            (16, "way", near_closed.as_slice(), None),
        ] {
            let mut tags = Tags::from([
                ("leisure".into(), "track".into()),
                ("sport".into(), "running".into()),
            ]);
            if let Some(area) = area {
                tags.insert("area".into(), area.into());
            }
            spill
                .emit_polygon(
                    &FeatureType::Leisure,
                    crate::osm::geometry::square_of(50.0, 14.0),
                    id,
                    kind,
                    50.0,
                    14.0,
                    &tags,
                    Some(ring),
                )
                .unwrap();
        }
    });
    for (family, expected) in [
        ("industrial", vec![1, 2, 7]),
        ("leisure", vec![1, 2, 7, 8, 9, 11, 12, 14, 16]),
    ] {
        let mut ids: Vec<i64> = Vec::new();
        for batch in read(&square.join(format!("{family}.arrow"))) {
            let values = batch
                .column_by_name("osm_id")
                .unwrap()
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap();
            if family == "leisure" {
                let kinds = batch
                    .column_by_name("geometry_kind")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<UInt8Array>()
                    .unwrap();
                for row in 0..batch.num_rows() {
                    if matches!(values.value(row), 11 | 12 | 14 | 16) {
                        assert_eq!(
                            kinds.value(row),
                            if matches!(values.value(row), 11 | 16) {
                                2
                            } else {
                                1
                            }
                        );
                        assert_eq!(
                            batch.column_by_name("area_m2").unwrap().is_null(row),
                            matches!(values.value(row), 11 | 16)
                        );
                    }
                }
            }
            ids.extend(values.values());
        }
        ids.sort();
        assert_eq!(ids, expected, "{family}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
