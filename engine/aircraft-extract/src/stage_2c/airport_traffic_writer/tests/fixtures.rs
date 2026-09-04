//! Canonical airport-line Arrow fixtures.
use super::*;
pub(crate) fn write_real_airport_lines_arrow(path: &Path, rows: &[FakeRealLine]) {
    use arrow::array::{
        Float32Builder, Int16Builder, Int32Builder, Int64Builder, StringBuilder, UInt8Builder,
    };
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::collections::HashMap;
    use std::sync::Arc;

    let schema = Arc::new(
        Schema::new(vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("segment_idx", DataType::Int16, false),
            Field::new("start_gy", DataType::Int32, false),
            Field::new("start_gx", DataType::Int32, false),
            Field::new("end_gy", DataType::Int32, false),
            Field::new("end_gx", DataType::Int32, false),
            Field::new("length_m", DataType::Float32, false),
            Field::new("heading_deg", DataType::Float32, false),
            Field::new("aeroway_type", DataType::UInt8, false),
            Field::new("ref", DataType::Utf8, true),
            Field::new("surface", DataType::Utf8, true),
            Field::new("width_m", DataType::Float32, true),
        ])
        .with_metadata({
            let mut md = HashMap::new();
            md.insert(
                "schema_version".to_string(),
                crate::SCHEMA_VERSION.to_string(),
            );
            md
        }),
    );

    let n = rows.len();
    let mut osm_id = Int64Builder::with_capacity(n);
    let mut seg_idx = Int16Builder::with_capacity(n);
    let mut sla = Int32Builder::with_capacity(n);
    let mut slo = Int32Builder::with_capacity(n);
    let mut ela = Int32Builder::with_capacity(n);
    let mut elo = Int32Builder::with_capacity(n);
    let mut len = Float32Builder::with_capacity(n);
    let mut heading = Float32Builder::with_capacity(n);
    let mut atype = UInt8Builder::with_capacity(n);
    let mut ref_col = StringBuilder::with_capacity(n, 0);
    let mut surface = StringBuilder::with_capacity(n, 0);
    let mut width = Float32Builder::with_capacity(n);
    for r in rows {
        osm_id.append_value(r.osm_id);
        seg_idx.append_value(r.segment_idx);
        sla.append_value(grid::lonlat_to_grid(r.start_lon, r.start_lat).1);
        slo.append_value(grid::lonlat_to_grid(r.start_lon, r.start_lat).0);
        ela.append_value(grid::lonlat_to_grid(r.end_lon, r.end_lat).1);
        elo.append_value(grid::lonlat_to_grid(r.end_lon, r.end_lat).0);
        len.append_value(r.length_m);
        heading.append_value(0.0);
        atype.append_value(r.aeroway_type);
        ref_col.append_null();
        surface.append_null();
        width.append_null();
    }
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(osm_id.finish()),
            Arc::new(seg_idx.finish()),
            Arc::new(sla.finish()),
            Arc::new(slo.finish()),
            Arc::new(ela.finish()),
            Arc::new(elo.finish()),
            Arc::new(len.finish()),
            Arc::new(heading.finish()),
            Arc::new(atype.finish()),
            Arc::new(ref_col.finish()),
            Arc::new(surface.finish()),
            Arc::new(width.finish()),
        ],
    )
    .unwrap();
    crate::arrow_io::write_record_batches(path, &schema, &[batch]).unwrap();
}

pub(crate) struct FakeRealLine {
    pub osm_id: i64,
    pub segment_idx: i16,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub length_m: f32,
    pub aeroway_type: u8,
}
