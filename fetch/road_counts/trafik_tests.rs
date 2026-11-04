//! Swedish light/motorcycle grouping preserves measured trucks and rejects inconsistent totals.

use super::{tests::inputs, trafik};
use serde_json::json;

#[test]
fn measured_swedish_trucks_and_total_survive_without_an_invented_motorcycle_share() {
    let (directory, inputs) = inputs("SE");
    let feature = |id, total| {
        let mut properties = serde_json::Map::new();
        for (key, value) in [
            ("id", json!(id)),
            ("Matmetod", json!("Stickprovsmätning")),
            ("Adt_samtliga_fordon", json!(total)),
            ("ROLE", json!("")),
            ("DIRECTION", json!("")),
            ("Matarsperiod", json!(200000)),
        ] {
            properties.insert(key.into(), value);
        }
        for (category, count) in [("latta", 70), ("medeltunga", 10), ("tunga", 20)] {
            for period in ["06_18", "18_22", "22_06"] {
                properties.insert(
                    format!("Adt_{category}_fordon_{period}"),
                    json!(if period == "06_18" { count } else { 0 }),
                );
            }
        }
        json!({"type":"Feature","properties":properties,"geometry":{"type":"LineString","coordinates":[[674000,6580000],[674010,6580000]]}})
    };
    let raw = json!({"type":"FeatureCollection","name":"traffic_fixture","crs":{"type":"name","properties":{"name":"EPSG:3006"}},"features":[feature(1,100),feature(2,104),feature(3,105)]});
    let path = directory.path().join("traffic.json");
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let recipe=serde_json::from_value(json!({"source":{"input_id":"survey","source_id":1,"source_rank":1},"layer":"traffic_fixture","search_radius_m":50.0,"minimum_alignment_cosine":0.86})).unwrap();
    let rows = trafik::convert(&recipe, &inputs, &path).unwrap();
    assert_eq!(rows.len(), 2);
    for (row, total) in rows.iter().zip([100.0, 104.0]) {
        assert_eq!(row.total, Some(total));
        assert_eq!(row.vehicle_classes, [None, Some(10.0), Some(20.0), None]);
        assert_eq!(row.vehicle_groups.len(), 1);
        assert_eq!(row.vehicle_groups[0].classes, 9);
        assert_eq!(row.vehicle_groups[0].count, total - 30.0);
        assert!(row.measured && row.estimated_classes == 9);
        let (classes, status) = row.daily_classes([0.5, 0.0, 0.0, 0.5]).unwrap();
        assert_eq!(classes[1], 10.0);
        assert_eq!(classes[2], 20.0);
        assert_eq!(classes.iter().sum::<f64>(), total);
        assert_eq!(status, 9);
    }
}
