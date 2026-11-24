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

#[test]
fn fallback_vintage_station_pairs_do_not_apply_unmatched_track_residuals() {
    let dir = tempfile::tempdir().unwrap();
    let pt = (50.08, 14.43);
    let sq = geometry::square_of(pt.0, pt.1);
    write_square(dir.path(), [sq.x, sq.y], pt.0, pt.1, true);
    let model: RailModel = serde_json::from_value(serde_json::json!({
        "network_bounds": {"110:CZ": [49.5, 14.0, 50.5, 15.0]},
        "residuals": {"110:CZ": {"source_id": 9863, "passenger": 2.0, "freight": 1.0}},
        "spatial_priors": {},
        "sources": [
            {"id": 110, "rank": 1, "edition": 2024, "owner": "CZ", "countries": ["CZ"], "residual": false, "whole_line": true},
            {"id": 9863, "rank": 9, "edition": 2024, "owner": "CZ", "countries": ["CZ"], "residual": true, "whole_line": false}
        ],
        "type_priors": [[80.0, 20.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
        "usage_priors": [[80.0, 20.0], [30.0, 5.0], [0.0, 15.0], [40.0, 10.0]],
        "mainline_freight": {},
        "passenger_periods": [0.7, 0.2, 0.1],
        "freight_periods": [0.5, 0.17, 0.33],
        "urban_periods": [0.7, 0.25, 0.05],
        "regional_freight_periods": []
    }))
    .unwrap();
    let write_flows = |path: &Path, attributes: std::collections::BTreeMap<String, String>| {
        // Station pair far from the edge leaves the edge unmatched so residual eligibility is tested.
        use crate::railways::movements::{Location, Movement, Record, Stop, write_record};
        let mut out = File::create(path).unwrap();
        write_record(
            &mut out,
            &Record::Window {
                version: crate::railways::movements::STREAM_VERSION,
                start: "2024-01-01".into(),
                end: "2025-01-01".into(),
            },
        )
        .unwrap();
        write_record(
            &mut out,
            &Record::Movement(Box::new(Movement {
                id: "cz/1".into(),
                source_id: 110,
                country: "CZ".into(),
                passenger: Some(10.0),
                freight: None,
                passenger_status: 2,
                freight_status: 0,
                matching: 2,
                rail_types: vec![0],
                coverage: Default::default(),
                missing_calendar_estimate: [0.0, 0.0],
                attributes,
                location: Location::StationPair {
                    from: Stop {
                        source_id: "1".into(),
                        id: "A".into(),
                        latitude: 50.4,
                        longitude: 14.9,
                    },
                    to: Stop {
                        source_id: "2".into(),
                        id: "B".into(),
                        latitude: 50.45,
                        longitude: 14.95,
                    },
                },
            })),
        )
        .unwrap();
    };
    let selected = HashSet::from([[sq.x, sq.y]]);
    let regular_flows = dir.path().join("regular.jsonl");
    write_flows(&regular_flows, Default::default());
    let regular = crate::railways::matcher::read(
        &model,
        dir.path(),
        &regular_flows,
        "2024-01-01",
        "2025-01-01",
        &selected,
    )
    .unwrap();
    assert_eq!(regular.claims[&[sq.x, sq.y]].len(), 1);
    assert_eq!(regular.claims[&[sq.x, sq.y]][0].0.source_id, 9863);

    let fallback_flows = dir.path().join("fallback.jsonl");
    write_flows(
        &fallback_flows,
        std::collections::BTreeMap::from([(
            "fallback_vintage".into(),
            "2025-12-14..2026-12-12".into(),
        )]),
    );
    let fallback = crate::railways::matcher::read(
        &model,
        dir.path(),
        &fallback_flows,
        "2024-01-01",
        "2025-01-01",
        &selected,
    )
    .unwrap();
    assert!(
        fallback.claims.get(&[sq.x, sq.y]).is_none_or(Vec::is_empty),
        "fallback timetable must not stamp residual traffic on unmatched edges"
    );
}
