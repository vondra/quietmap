//! Regression tests for `Network::read` schema requirements across selected and off-selection squares.
use super::*;
use crate::railways::model::RailModel;
use arrow_array::{Float32Array, Int16Array, Int32Array, Int64Array, UInt8Array};
use arrow_ipc::writer::FileWriter;
use arrow_schema::{DataType, Field, Schema};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::sync::Arc;

fn write_arrow(path: &Path, batch: &RecordBatch) {
    let mut writer = FileWriter::try_new(File::create(path).unwrap(), &batch.schema()).unwrap();
    writer.write(batch).unwrap();
    writer.finish().unwrap();
}

fn write_square(dir: &Path, square: [u16; 2], lat: f64, lon: f64, with_traffic_mode: bool) {
    let sq_dir = dir.join(format!("z9/{}/{}", square[0], square[1]));
    fs::create_dir_all(&sq_dir).unwrap();
    let (gx0, gy0) = geometry::lonlat_to_grid(lon, lat);
    let (gx1, gy1) = geometry::lonlat_to_grid(lon + 0.001, lat);
    let pieces_schema = Arc::new(Schema::new_with_metadata(
        vec![
            Field::new("way_id", DataType::Int64, false),
            Field::new("segment_idx", DataType::Int16, false),
            Field::new("from_m", DataType::Float64, false),
            Field::new("to_m", DataType::Float64, false),
            Field::new("start_fraction", DataType::Float64, false),
            Field::new("end_fraction", DataType::Float64, false),
            Field::new("start_vertex", DataType::UInt16, false),
            Field::new("end_vertex", DataType::UInt16, false),
            Field::new("start_node", DataType::Int64, false),
            Field::new("end_node", DataType::Int64, false),
            Field::new("start_gx", DataType::Int32, false),
            Field::new("start_gy", DataType::Int32, false),
            Field::new("end_gx", DataType::Int32, false),
            Field::new("end_gy", DataType::Int32, false),
        ],
        HashMap::from([("transport_pieces_contract".into(), "1".into())]),
    ));
    let pieces = RecordBatch::try_new(
        pieces_schema,
        vec![
            Arc::new(Int64Array::from(vec![1i64])),
            Arc::new(Int16Array::from(vec![0i16])),
            Arc::new(Float64Array::from(vec![0.0f64])),
            Arc::new(Float64Array::from(vec![100.0f64])),
            Arc::new(Float64Array::from(vec![0.0f64])),
            Arc::new(Float64Array::from(vec![0.0f64])),
            Arc::new(UInt16Array::from(vec![0u16])),
            Arc::new(UInt16Array::from(vec![1u16])),
            Arc::new(Int64Array::from(vec![10i64])),
            Arc::new(Int64Array::from(vec![11i64])),
            Arc::new(Int32Array::from(vec![gx0])),
            Arc::new(Int32Array::from(vec![gy0])),
            Arc::new(Int32Array::from(vec![gx1])),
            Arc::new(Int32Array::from(vec![gy1])),
        ],
    )
    .unwrap();
    write_arrow(&sq_dir.join("railways.pieces.arrow"), &pieces);

    let mut fields = vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("segment_idx", DataType::Int16, false),
        Field::new("rail_type", DataType::UInt8, false),
        Field::new("usage", DataType::UInt8, false),
        Field::new("service", DataType::UInt8, false),
        Field::new("country_iso", DataType::UInt16, false),
        Field::new("length_m", DataType::Float32, false),
    ];
    let mut columns: Vec<Arc<dyn Array>> = vec![
        Arc::new(Int64Array::from(vec![1i64])),
        Arc::new(Int16Array::from(vec![0i16])),
        Arc::new(UInt8Array::from(vec![0u8])),
        Arc::new(UInt8Array::from(vec![0u8])),
        Arc::new(UInt8Array::from(vec![0u8])),
        Arc::new(UInt16Array::from(vec![u16::from_le_bytes(*b"US")])),
        Arc::new(Float32Array::from(vec![100.0f32])),
    ];
    if with_traffic_mode {
        fields.push(Field::new("traffic_mode", DataType::UInt8, false));
        columns.push(Arc::new(UInt8Array::from(vec![0u8])));
    }
    let rails = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap();
    write_arrow(&sq_dir.join("railways.arrow"), &rails);
}

/// An off-selection square inside a crossing source's bounds must fail closed when
/// `traffic_mode` is missing, rather than silently defaulting to mode 0 in global allocation.
#[test]
fn off_selection_square_without_traffic_mode_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let selected_pt = (40.5, -74.5);
    let neighbor_pt = (40.5, -73.8);
    let selected = geometry::square_of(selected_pt.0, selected_pt.1);
    let neighbor = geometry::square_of(neighbor_pt.0, neighbor_pt.1);
    assert_ne!([selected.x, selected.y], [neighbor.x, neighbor.y]);
    write_square(
        dir.path(),
        [selected.x, selected.y],
        selected_pt.0,
        selected_pt.1,
        true,
    );
    write_square(
        dir.path(),
        [neighbor.x, neighbor.y],
        neighbor_pt.0,
        neighbor_pt.1,
        false,
    );
    let flows = dir.path().join("flows.jsonl");
    fs::write(
        &flows,
        concat!(
            r#"{"record":"window","version":3,"start":"2024-01-01","end":"2025-01-01"}"#,
            "\n",
            r#"{"record":"crossing","id":"X1","lat":40.5,"lon":-74.5,"soundings":[0.0,0.0,0.0],"speed_kmh":80.0,"source_id":111,"synth_osm":-1,"total_periods":[10.0,2.0,4.0],"passenger_daily":0.0,"passenger_lt1":false,"measured_periods":true}"#,
            "\n"
        ),
    )
    .unwrap();
    let model: RailModel = serde_json::from_value(serde_json::json!({
        "network_bounds": {"111:US": [40.0, -75.0, 41.0, -73.0]},
        "residuals": {},
        "spatial_priors": {},
        "sources": [{"id": 111, "rank": 4, "edition": 2024, "owner": "US", "countries": ["US"], "residual": false, "whole_line": true}],
        "type_priors": [[80.0, 20.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
        "usage_priors": [[80.0, 20.0], [30.0, 5.0], [0.0, 15.0], [40.0, 10.0]],
        "mainline_freight": {},
        "passenger_periods": [0.7, 0.2, 0.1],
        "freight_periods": [0.5, 0.17, 0.33],
        "urban_periods": [0.7, 0.25, 0.05],
        "regional_freight_periods": []
    }))
    .unwrap();
    let err = crate::railways::matcher::read(
        &model,
        dir.path(),
        &flows,
        "2024-01-01",
        "2025-01-01",
        &HashSet::from([[selected.x, selected.y]]),
    )
    .err()
    .expect("off-selection square missing traffic_mode must fail");
    assert!(err.contains("traffic_mode"), "unexpected error: {err}");

    write_square(
        dir.path(),
        [neighbor.x, neighbor.y],
        neighbor_pt.0,
        neighbor_pt.1,
        true,
    );
    assert!(
        crate::railways::matcher::read(
            &model,
            dir.path(),
            &flows,
            "2024-01-01",
            "2025-01-01",
            &HashSet::from([[selected.x, selected.y]]),
        )
        .is_ok()
    );
}
