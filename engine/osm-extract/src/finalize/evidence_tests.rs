//! Producer-to-reader regressions for controls, structure extents and new source evidence.

use super::*;
use crate::{
    classify::{FeatureType, Tags},
    model_nodes::{ControlPoints, ModelNode},
    spill::Spiller,
};
use arrow::{array::*, ipc::reader::FileReader, record_batch::RecordBatch};

fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
fn fixture(
    name: &str,
    write: impl FnOnce(&mut Spiller),
) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("osm-evidence-{}-{name}", std::process::id()));
    let spill = root.join("spill");
    let out = root.join("prepared");
    let mut writer = Spiller::new(&spill, 1).unwrap();
    write(&mut writer);
    crate::transport::TransportSpill::new(&spill)
        .unwrap()
        .finish()
        .unwrap();
    writer.complete("fixture").unwrap();
    drop(writer);
    finalize(&spill, &out, 1).unwrap();
    (
        root,
        out.join(grid::square_name(grid::square_of(50.0, 14.0))),
    )
}
fn read(path: &Path) -> Vec<RecordBatch> {
    FileReader::try_new(File::open(path).unwrap(), None)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}
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
        .flat_map(|b| square_store::osm_evidence::control_points(b).unwrap())
        .collect();
    assert_eq!(points.len(), 3);
    let linked: Vec<_> = points.iter().filter(|p| p.node_id == 2).collect();
    assert_eq!(linked.len(), 2);
    assert!(linked
        .iter()
        .any(|p| p.family == "roads" && p.way_id == Some(11)));
    assert!(linked
        .iter()
        .any(|p| p.family == "railways" && p.way_id == Some(22)));
    assert!(linked
        .iter()
        .all(|p| p.vertex_index == Some(1) && p.way_m.unwrap() > 0.0));
    let orphan = points.iter().find(|p| p.node_id == 4).unwrap();
    assert_eq!(orphan.way_id, None);
    assert!(orphan.tags_json.contains("PL-PKP:w6a"));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn bridge_microsegment_retains_original_abutments_and_speed_evidence() {
    let (root, square) = fixture("bridge", |spill| {
        let nodes = [(1, Some([50.0, 14.0])), (2, Some([50.0, 14.01]))];
        let extent = crate::transport::way_extent(&nodes);
        spill
            .emit_segment(
                &FeatureType::Road,
                grid::square_of(50.0, 14.0),
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
    let raw: Tags =
        serde_json::from_str(square_store::osm_evidence::tags(batch, "roads", 0).unwrap()).unwrap();
    assert_eq!(raw["layer"], "2");
    assert_eq!(raw["surface"], "paving_stones");
    let mut old = batch.schema().metadata().clone();
    old.remove("osm_roads_contract");
    assert!(square_store::osm_contract::validate(
        &Schema::new_with_metadata(batch.schema().fields().clone(), old),
        "roads"
    )
    .is_err());
    std::fs::remove_dir_all(root).unwrap();
}
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
                    grid::square_of(50.0, 14.0),
                    id,
                    "way",
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
                let raw: Tags = serde_json::from_str(
                    square_store::osm_evidence::tags(&batch, "leisure", row).unwrap(),
                )
                .unwrap();
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
                    grid::square_of(50.0, 14.0),
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
            let raw: Tags = serde_json::from_str(
                square_store::osm_evidence::tags(&batch, "industrial", row).unwrap(),
            )
            .unwrap();
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
