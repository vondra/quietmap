//! `roads.arrow` writer: one row per road microsegment with integer grid
//! geometry + classification + provenance columns. See `finalize` for dispatch.

use anyhow::Result;
use arrow::array::*;
use arrow::datatypes::*;
use std::path::Path;
use std::sync::Arc;

use super::{parse_grid_cell, segment_row_bbox, write_arrow_spatially_batched};

pub(super) fn write_roads(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let n = rows.len();
    let schema = Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("segment_idx", DataType::Int16, false),
        Field::new("start_gx", DataType::Int32, false),
        Field::new("start_gy", DataType::Int32, false),
        Field::new("end_gx", DataType::Int32, false),
        Field::new("end_gy", DataType::Int32, false),
        Field::new("length_m", DataType::Float32, false),
        // 0=motorway..6=living_street, 7=service, 8=track, 9=unclassified,
        // 10=motorway_link, 11=trunk_link, 12=primary_link
        Field::new("road_class", DataType::UInt8, false),
        Field::new("speed_limit", DataType::UInt8, false),
        Field::new("surface_type", DataType::UInt8, false),
        Field::new("oneway", DataType::Boolean, false),
        Field::new("lanes", DataType::UInt8, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("ref", DataType::Utf8, true),
        Field::new("bridge", DataType::Boolean, false),
        Field::new("tunnel", DataType::Boolean, false),
        Field::new("toll", DataType::Boolean, false),
        Field::new("lit", DataType::UInt8, false), // 0=unknown, 1=yes, 2=no
        Field::new("junction", DataType::UInt8, false), // 0=none, 1=roundabout, 2=mini_roundabout
        // 0=yes, 1=private, 2=no, 3=destination, 4=motor_vehicle_no (legacy),
        // 5=permissive, 6=customers, 7=agricultural, 8=forestry
        Field::new("access", DataType::UInt8, false),
        // Data provenance — 0 = unspecified, populated by enrich-roads-*.ts.
        Field::new("source_id", DataType::UInt16, false),
    ]);

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut seg_idx = Int16Builder::with_capacity(n);
    let mut sgx = Int32Builder::with_capacity(n);
    let mut sgy = Int32Builder::with_capacity(n);
    let mut egx = Int32Builder::with_capacity(n);
    let mut egy = Int32Builder::with_capacity(n);
    let mut len = Float32Builder::with_capacity(n);
    let mut rclass = UInt8Builder::with_capacity(n);
    let mut speed = UInt8Builder::with_capacity(n);
    let mut surface = UInt8Builder::with_capacity(n);
    let mut oneway = BooleanBuilder::with_capacity(n);
    let mut lanes = UInt8Builder::with_capacity(n);
    let mut name = StringBuilder::with_capacity(n, n * 10);
    let mut ref_col = StringBuilder::with_capacity(n, n * 5);
    let mut bridge = BooleanBuilder::with_capacity(n);
    let mut tunnel = BooleanBuilder::with_capacity(n);
    let mut toll = BooleanBuilder::with_capacity(n);
    let mut lit = UInt8Builder::with_capacity(n);
    let mut junction = UInt8Builder::with_capacity(n);
    let mut access = UInt8Builder::with_capacity(n);
    let mut source_id = UInt16Builder::with_capacity(n);
    let mut row_bboxes = Vec::with_capacity(n);

    for row in rows {
        // TSV: sq(0) osm_id(1) seg_idx(2) s_gx(3) s_gy(4) e_gx(5) e_gy(6) len(7)
        //      road_class(8) speed(9) surface(10) oneway(11) lanes(12) name(13) ref(14)
        //      bridge(15) tunnel(16) toll(17) lit(18) junction(19) access(20)
        if row.len() < 21 {
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
        rclass.append_value(row[8].parse().unwrap_or(0));
        speed.append_value(row[9].parse().unwrap_or(0));
        surface.append_value(row[10].parse().unwrap_or(0));
        oneway.append_value(row[11] == "1");
        lanes.append_value(row[12].parse().unwrap_or(0));
        name.append_value(&row[13]);
        ref_col.append_value(row.get(14).map(|s| s.as_str()).unwrap_or(""));
        bridge.append_value(row.get(15).map(|s| s == "1").unwrap_or(false));
        tunnel.append_value(row.get(16).map(|s| s == "1").unwrap_or(false));
        toll.append_value(row.get(17).map(|s| s == "1").unwrap_or(false));
        lit.append_value(row.get(18).and_then(|s| s.parse().ok()).unwrap_or(0));
        junction.append_value(row.get(19).and_then(|s| s.parse().ok()).unwrap_or(0));
        access.append_value(row.get(20).and_then(|s| s.parse().ok()).unwrap_or(0));
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
            Arc::new(rclass.finish()),
            Arc::new(speed.finish()),
            Arc::new(surface.finish()),
            Arc::new(oneway.finish()),
            Arc::new(lanes.finish()),
            Arc::new(name.finish()),
            Arc::new(ref_col.finish()),
            Arc::new(bridge.finish()),
            Arc::new(tunnel.finish()),
            Arc::new(toll.finish()),
            Arc::new(lit.finish()),
            Arc::new(junction.finish()),
            Arc::new(access.finish()),
            Arc::new(source_id.finish()),
        ],
        &row_bboxes,
    )
}
