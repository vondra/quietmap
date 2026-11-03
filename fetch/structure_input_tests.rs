//! Shared survey caches retain publisher interleaving and exclude unadmitted sources.

use super::*;
use arrow_array::{ArrayRef, BinaryArray, Float32Array};
use arrow_schema::{DataType, Field, Schema};
use geos::{Geom, Geometry};
use parquet::arrow::ArrowWriter;
use std::sync::Arc;

#[test]
fn shared_survey_objects_are_read_once_in_original_order() {
    let root = std::env::temp_dir().join(format!("qm-survey-order-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("observations.parquet");
    let polygon = Geometry::new_from_wkt(
        "POLYGON ((14.17 49.78,14.1702 49.78,14.1702 49.7802,14.17 49.7802,14.17 49.78))",
    )
    .unwrap();
    let wkb = polygon.to_wkb().unwrap();
    let schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("geometry", DataType::Binary, false),
            Field::new("height_m", DataType::Float32, false),
            Field::new("source", DataType::Utf8, false),
        ],
        std::collections::HashMap::from([(
            "measured_heights_contract".to_owned(),
            "measured_heights_v1".to_owned(),
        )]),
    ));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(BinaryArray::from_vec(vec![&wkb, &wkb, &wkb])),
        Arc::new(Float32Array::from(vec![12.0, 34.0, 99.0])),
        Arc::new(StringArray::from(vec!["second", "first", "later"])),
    ];
    let mut writer =
        ArrowWriter::try_new(File::create(&path).unwrap(), schema.clone(), None).unwrap();
    writer
        .write(&RecordBatch::try_new(schema, columns).unwrap())
        .unwrap();
    writer.close().unwrap();
    let source = |name: &str| {
        (
            Objects {
                format: "measured-heights".into(),
                source: Some(name.into()),
                objects: vec![Object {
                    path: "observations.parquet".into(),
                    degree: Some([49, 14]),
                }],
            },
            root.clone(),
        )
    };
    let mut rows = vec![Vec::new()];
    surveys::convert(
        &[source("first"), source("second")],
        &[Square { x: 276, y: 174 }],
        &mut rows,
        false,
    )
    .unwrap();
    assert_eq!(
        rows[0].iter().map(|r| r.height_m).collect::<Vec<_>>(),
        vec![Some(12.0), Some(34.0)]
    );
    let (mut unscoped, directory) = source("first");
    unscoped.source = None;
    assert!(
        surveys::convert(
            &[(unscoped, directory)],
            &[Square { x: 276, y: 174 }],
            &mut rows,
            false
        )
        .unwrap_err()
        .to_string()
        .contains("requires a publisher source")
    );
    std::fs::remove_dir_all(root).unwrap();
}
