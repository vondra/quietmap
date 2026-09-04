//! Airport aeroway writers: `airport_areas.arrow` (polygon aprons/runways with
//! identity tags) and `airport_lines.arrow` (≤250m runway/taxiway microsegments
//! with integer grid geometry). See `finalize` for dispatch.

use anyhow::Result;
use arrow::array::*;
use arrow::datatypes::*;
use grid::poly::{encode_grid_poly, ring_area_m2};
use std::path::Path;
use std::sync::Arc;

use super::{
    decode_tsv_ring, parse_grid_cell, polygon_row_bbox, segment_row_bbox,
    write_arrow_spatially_batched,
};

pub(super) fn write_airport_areas(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let n = rows.len();
    let schema = Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("centroid_gx", DataType::Int32, false),
        Field::new("centroid_gy", DataType::Int32, false),
        Field::new("aeroway_type", DataType::UInt8, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("ref", DataType::Utf8, true),
        Field::new("icao", DataType::Utf8, true),
        Field::new("iata", DataType::Utf8, true),
        Field::new("operator", DataType::Utf8, true),
        Field::new("surface", DataType::Utf8, true),
        Field::new("width_m", DataType::Float32, true),
        Field::new("aerodrome_type", DataType::Utf8, true),
        Field::new("access", DataType::Utf8, true),
        Field::new("geom", DataType::Binary, true),
        Field::new("area_m2", DataType::Float32, true),
    ]);

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut cgx = Int32Builder::with_capacity(n);
    let mut cgy = Int32Builder::with_capacity(n);
    let mut aeroway_type = UInt8Builder::with_capacity(n);
    let mut name = StringBuilder::with_capacity(n, n * 10);
    let mut ref_col = StringBuilder::with_capacity(n, n * 6);
    let mut icao = StringBuilder::with_capacity(n, n * 4);
    let mut iata = StringBuilder::with_capacity(n, n * 4);
    let mut operator = StringBuilder::with_capacity(n, n * 10);
    let mut surface = StringBuilder::with_capacity(n, n * 8);
    let mut width_m = Float32Builder::with_capacity(n);
    let mut aerodrome_type = StringBuilder::with_capacity(n, n * 8);
    let mut access = StringBuilder::with_capacity(n, n * 8);
    let mut geom = BinaryBuilder::with_capacity(n, n * 100);
    let mut area_m2 = Float32Builder::with_capacity(n);
    let mut row_bboxes = Vec::with_capacity(n);

    for row in rows {
        // TSV: sq(0) osm_id(1) c_gx(2) c_gy(3) aeroway_type(4) name(5) ref(6) icao(7)
        //      iata(8) operator(9) surface(10) width_m(11) aerodrome_type(12) access(13) ring(14)
        if row.len() < 14 {
            continue;
        }
        let c_gx = parse_grid_cell(&row[2]);
        let c_gy = parse_grid_cell(&row[3]);
        let ring = decode_tsv_ring(row.get(14).map(|s| s.as_str()).unwrap_or(""));
        row_bboxes.push(polygon_row_bbox(ring.as_deref(), c_gx, c_gy));
        osm_id.append_value(row[1].parse().unwrap_or(0));
        cgx.append_value(c_gx);
        cgy.append_value(c_gy);
        aeroway_type.append_value(row[4].parse().unwrap_or(255));
        name.append_value(row.get(5).map(|s| s.as_str()).unwrap_or(""));
        ref_col.append_value(row.get(6).map(|s| s.as_str()).unwrap_or(""));
        icao.append_value(row.get(7).map(|s| s.as_str()).unwrap_or(""));
        iata.append_value(row.get(8).map(|s| s.as_str()).unwrap_or(""));
        operator.append_value(row.get(9).map(|s| s.as_str()).unwrap_or(""));
        surface.append_value(row.get(10).map(|s| s.as_str()).unwrap_or(""));
        let width: f32 = row.get(11).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if width > 0.0 {
            width_m.append_value(width);
        } else {
            width_m.append_null();
        }
        aerodrome_type.append_value(row.get(12).map(|s| s.as_str()).unwrap_or(""));
        access.append_value(row.get(13).map(|s| s.as_str()).unwrap_or(""));
        match ring {
            Some(ring) => {
                match ring_area_m2(&ring) {
                    Some(a) => area_m2.append_value(a as f32),
                    None => area_m2.append_null(),
                }
                geom.append_value(encode_grid_poly(&ring));
            }
            None => {
                geom.append_null();
                area_m2.append_null();
            }
        }
    }

    write_arrow_spatially_batched(
        path,
        schema,
        vec![
            Arc::new(osm_id.finish()),
            Arc::new(cgx.finish()),
            Arc::new(cgy.finish()),
            Arc::new(aeroway_type.finish()),
            Arc::new(name.finish()),
            Arc::new(ref_col.finish()),
            Arc::new(icao.finish()),
            Arc::new(iata.finish()),
            Arc::new(operator.finish()),
            Arc::new(surface.finish()),
            Arc::new(width_m.finish()),
            Arc::new(aerodrome_type.finish()),
            Arc::new(access.finish()),
            Arc::new(geom.finish()),
            Arc::new(area_m2.finish()),
        ],
        &row_bboxes,
    )
}

/// `airport_lines.arrow`: one row per ≤250m microsegment of OSM aeroway
/// runway/taxiway/stopway/airstrip lines. Geometry-only — airport identity
/// is computed downstream by aircraft-extract Stage 2C via the existing
/// `nearest_aerodrome_within` snap (area-aware radius with 3km LKPR floor).
/// Closed-ring runway ways are rerouted to airport_areas; multipolygon
/// members are skipped (relation handler already produces the area).
pub(super) fn write_airport_lines(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let n = rows.len();
    let schema = Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("segment_idx", DataType::Int16, false),
        Field::new("start_gx", DataType::Int32, false),
        Field::new("start_gy", DataType::Int32, false),
        Field::new("end_gx", DataType::Int32, false),
        Field::new("end_gy", DataType::Int32, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("heading_deg", DataType::Float32, false),
        // 0=runway, 1=taxiway, 6=stopway, 7=airstrip
        // (matches airport_areas.arrow convention)
        Field::new("aeroway_type", DataType::UInt8, false),
        Field::new("ref", DataType::Utf8, true),
        Field::new("surface", DataType::Utf8, true),
        Field::new("width_m", DataType::Float32, true),
    ]);

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut seg_idx = Int16Builder::with_capacity(n);
    let mut sgx = Int32Builder::with_capacity(n);
    let mut sgy = Int32Builder::with_capacity(n);
    let mut egx = Int32Builder::with_capacity(n);
    let mut egy = Int32Builder::with_capacity(n);
    let mut len = Float32Builder::with_capacity(n);
    let mut heading = Float32Builder::with_capacity(n);
    let mut atype = UInt8Builder::with_capacity(n);
    let mut ref_col = StringBuilder::with_capacity(n, n * 4);
    let mut surface = StringBuilder::with_capacity(n, n * 8);
    let mut width_m = Float32Builder::with_capacity(n);
    let mut row_bboxes = Vec::with_capacity(n);

    for row in rows {
        // TSV: sq(0) osm_id(1) seg_idx(2) s_gx(3) s_gy(4) e_gx(5) e_gy(6)
        //      len(7) heading(8) aeroway_type(9) ref(10) surface(11) width(12)
        if row.len() < 12 {
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
        heading.append_value(row[8].parse().unwrap_or(0.0));
        // 255 = "other" sentinel matching airport_areas convention
        // (see classify::aeroway_type docstring).
        atype.append_value(row[9].parse().unwrap_or(255));
        // Nullable Utf8 columns: emit null for empty, not "".
        match row.get(10).map(|s| s.as_str()).filter(|s| !s.is_empty()) {
            Some(v) => ref_col.append_value(v),
            None => ref_col.append_null(),
        }
        match row.get(11).map(|s| s.as_str()).filter(|s| !s.is_empty()) {
            Some(v) => surface.append_value(v),
            None => surface.append_null(),
        }
        match row
            .get(12)
            .and_then(|s| if s.is_empty() { None } else { s.parse().ok() })
        {
            Some(v) => width_m.append_value(v),
            None => width_m.append_null(),
        }
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
            Arc::new(heading.finish()),
            Arc::new(atype.finish()),
            Arc::new(ref_col.finish()),
            Arc::new(surface.finish()),
            Arc::new(width_m.finish()),
        ],
        &row_bboxes,
    )
}
