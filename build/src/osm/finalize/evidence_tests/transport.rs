//! Transport controls and whole-way extents retain their evidence.

use super::*;

#[test]
fn crossing_links_both_families_and_retains_unattached_whistle() {
    let (root, square) = fixture("controls", |spill| {
        let mut controls = ControlPoints::default();
        for (id, pairs) in [
            (
                2,
                vec![("railway", "level_crossing"), ("crossing:barrier", "half")],
            ),
            (4, vec![("railway:signal:whistle", "PL-PKP:w6a")]),
        ] {
            controls.insert(ModelNode {
                id,
                lat: 50.0,
                lon: 14.0,
                control: Some(tags(&pairs)),
                power: None,
            });
        }
        let nodes = [
            (1, Some([50.0, 13.999])),
            (2, Some([50.0, 14.0])),
            (3, Some([50.0, 14.001])),
        ];
        controls.link_way(11, "roads", &nodes, spill).unwrap();
        controls.link_way(22, "railways", &nodes, spill).unwrap();
        controls.finish(spill).unwrap();
    });
    let batches = read(&square.join("transport_nodes.arrow"));
    let points: Vec<_> = batches
        .iter()
        .flat_map(|b| control_points(b).unwrap())
        .collect();
    assert_eq!(points.len(), 3);
    let linked: Vec<_> = points.iter().filter(|p| p.node_id == 2).collect();
    assert_eq!(linked.len(), 2);
    assert!(
        linked
            .iter()
            .any(|p| p.family == "roads" && p.way_id == Some(11))
    );
    assert!(
        linked
            .iter()
            .any(|p| p.family == "railways" && p.way_id == Some(22))
    );
    assert!(
        linked
            .iter()
            .all(|p| p.vertex_index == Some(1) && p.way_m.unwrap() > 0.0)
    );
    let orphan = points.iter().find(|p| p.node_id == 4).unwrap();
    assert_eq!(orphan.way_id, None);
    assert!(orphan.tags_json.contains("PL-PKP:w6a"));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn bridge_microsegment_retains_original_abutments_and_speed_evidence() {
    let (root, square) = fixture("bridge", |spill| {
        let nodes = [(1, Some([50.0, 14.0])), (2, Some([50.0, 14.01]))];
        let extent = crate::osm::transport::way_extent(&nodes);
        spill
            .emit_segment(
                &FeatureType::Road,
                crate::osm::geometry::square_of(50.0, 14.0),
                31,
                0,
                &([50.0, 14.0], [50.0, 14.001], 71.0),
                &tags(&[
                    ("highway", "motorway"),
                    ("bridge", "yes"),
                    ("layer", "2"),
                    ("surface", "paving_stones"),
                    ("source:maxspeed", "DE:urban"),
                    ("maxspeed:hgv", "30 mph"),
                ]),
                Some(&format!("{extent}\t0,0,0,0.1,1,1")),
            )
            .unwrap();
    });
    let batches = read(&square.join("roads.arrow"));
    let batch = &batches[0];
    let u8col = |key| {
        batch
            .column_by_name(key)
            .unwrap()
            .as_any()
            .downcast_ref::<UInt8Array>()
            .unwrap()
            .value(0)
    };
    assert_eq!((u8col("speed_limit"), u8col("oneway")), (50, 4));
    assert_eq!(
        batch
            .column_by_name("maxspeed_hgv")
            .unwrap()
            .as_any()
            .downcast_ref::<UInt16Array>()
            .unwrap()
            .value(0),
        48
    );
    let whole_end = batch
        .column_by_name("way_end_gx")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap()
        .value(0);
    let piece_end = batch
        .column_by_name("end_gx")
        .unwrap()
        .as_any()
        .downcast_ref::<Int32Array>()
        .unwrap()
        .value(0);
    assert_ne!(whole_end, piece_end);
    let raw: Tags = serde_json::from_str(evidence_tags(batch, "roads", 0).unwrap()).unwrap();
    assert_eq!(raw["layer"], "2");
    assert_eq!(raw["surface"], "paving_stones");
    // The roads batch carries its contract stamp: every consumer refuses an
    // unstamped square, so a producer that dropped the stamp would lose data.
    assert_eq!(
        batch
            .schema()
            .metadata()
            .get("osm_roads_contract")
            .map(String::as_str),
        Some(crate::osm::contracts::ROADS_CONTRACT)
    );
    std::fs::remove_dir_all(root).unwrap();
}
