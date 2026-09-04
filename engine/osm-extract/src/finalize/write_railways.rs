//! `railways.arrow` writer: one row per rail microsegment with integer grid
//! geometry + electrification/gauge/usage columns. See `finalize` for dispatch.

use anyhow::Result;
use arrow::array::*;
use arrow::datatypes::*;
use std::path::Path;
use std::sync::Arc;

use super::{parse_grid_cell, segment_row_bbox, write_arrow_spatially_batched};

pub(super) fn write_railways(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let n = rows.len();
    let schema = Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("segment_idx", DataType::Int16, false),
        Field::new("start_gx", DataType::Int32, false),
        Field::new("start_gy", DataType::Int32, false),
        Field::new("end_gx", DataType::Int32, false),
        Field::new("end_gy", DataType::Int32, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("rail_type", DataType::UInt8, false),
        Field::new("usage", DataType::UInt8, false),
        // UInt16 because 300+ km/h high-speed lines overflow u8.
        Field::new("maxspeed", DataType::UInt16, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("ref", DataType::Utf8, true),
        Field::new("electrified", DataType::UInt8, false), // 0=unknown, 1=yes, 2=no
        Field::new("gauge", DataType::UInt16, false),      // mm (1435=standard)
        Field::new("bridge", DataType::Boolean, false),
        Field::new("tunnel", DataType::Boolean, false),
        Field::new("highspeed", DataType::Boolean, false),
        Field::new("service", DataType::UInt8, false), // 0=none, 1=yard, 2=siding, 3=spur, 4=crossover
        // Dataset provenance — 0 = unspecified, populated by enrich-railway-*.ts.
        Field::new("source_id", DataType::UInt16, false),
    ]);

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut seg_idx = Int16Builder::with_capacity(n);
    let mut sgx = Int32Builder::with_capacity(n);
    let mut sgy = Int32Builder::with_capacity(n);
    let mut egx = Int32Builder::with_capacity(n);
    let mut egy = Int32Builder::with_capacity(n);
    let mut len = Float32Builder::with_capacity(n);
    let mut rtype = UInt8Builder::with_capacity(n);
    let mut usage = UInt8Builder::with_capacity(n);
    let mut maxspd = UInt16Builder::with_capacity(n);
    let mut name = StringBuilder::with_capacity(n, n * 10);
    let mut ref_col = StringBuilder::with_capacity(n, n * 5);
    let mut electrified = UInt8Builder::with_capacity(n);
    let mut gauge = UInt16Builder::with_capacity(n);
    let mut rail_bridge = BooleanBuilder::with_capacity(n);
    let mut rail_tunnel = BooleanBuilder::with_capacity(n);
    let mut highspeed = BooleanBuilder::with_capacity(n);
    let mut service = UInt8Builder::with_capacity(n);
    let mut source_id = UInt16Builder::with_capacity(n);
    let mut row_bboxes = Vec::with_capacity(n);

    for row in rows {
        // TSV: sq(0) osm_id(1) seg_idx(2) s_gx(3) s_gy(4) e_gx(5) e_gy(6) len(7)
        //      rail_type(8) usage(9) maxspeed(10) name(11) ref(12) electrified(13) gauge(14)
        //      bridge(15) tunnel(16) highspeed(17) service(18)
        if row.len() < 19 {
            continue;
        }
        let s_gx = parse_grid_cell(&row[3]);
        let s_gy = parse_grid_cell(&row[4]);
        let e_gx = parse_grid_cell(&row[5]);
        let e_gy = parse_grid_cell(&row[6]);
        row_bboxes.push(segment_row_bbox(s_gx, s_gy, e_gx, e_gy));
        osm_id.append_value(row[1].parse().unwrap_or(0));
        seg_idx.append_value(row[2].parse().unwrap_or(0));
        sgx.append_value(s_gx);
        sgy.append_value(s_gy);
        egx.append_value(e_gx);
        egy.append_value(e_gy);
        len.append_value(row[7].parse().unwrap_or(0.0));
        rtype.append_value(row[8].parse().unwrap_or(0));
        usage.append_value(row[9].parse().unwrap_or(0));
        maxspd.append_value(row[10].parse().unwrap_or(0));
        name.append_value(&row[11]);
        ref_col.append_value(row.get(12).map(|s| s.as_str()).unwrap_or(""));
        electrified.append_value(row.get(13).and_then(|s| s.parse().ok()).unwrap_or(0));
        gauge.append_value(row.get(14).and_then(|s| s.parse().ok()).unwrap_or(0));
        rail_bridge.append_value(row.get(15).map(|s| s == "1").unwrap_or(false));
        rail_tunnel.append_value(row.get(16).map(|s| s == "1").unwrap_or(false));
        highspeed.append_value(row.get(17).map(|s| s == "1").unwrap_or(false));
        service.append_value(row.get(18).and_then(|s| s.parse().ok()).unwrap_or(0));
        source_id.append_value(0);
    }

    write_arrow_spatially_batched(
        path,
        schema,
        vec![
            Arc::new(osm_id.finish()),
            Arc::new(seg_idx.finish()),
            Arc::new(sgx.finish()),
            Arc::new(sgy.finish()),
            Arc::new(egx.finish()),
            Arc::new(egy.finish()),
            Arc::new(len.finish()),
            Arc::new(rtype.finish()),
            Arc::new(usage.finish()),
            Arc::new(maxspd.finish()),
            Arc::new(name.finish()),
            Arc::new(ref_col.finish()),
            Arc::new(electrified.finish()),
            Arc::new(gauge.finish()),
            Arc::new(rail_bridge.finish()),
            Arc::new(rail_tunnel.finish()),
            Arc::new(highspeed.finish()),
            Arc::new(service.finish()),
            Arc::new(source_id.finish()),
        ],
        &row_bboxes,
    )
}
