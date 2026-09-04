//! `leisure.arrow` writer. Its own v2 per-file contract — a NEW file, never
//! confused with buildings. No capacity column (dropped at this contract
//! bump: the area-law unification removed capacity scaling). See `finalize`.

use anyhow::Result;
use arrow::array::*;
use arrow::datatypes::*;
use grid::poly::{encode_grid_poly, ring_area_m2};
use std::path::Path;
use std::sync::Arc;

use super::{
    decode_tsv_ring, parse_grid_cell, polygon_row_bbox, schema_with_contract,
    write_arrow_spatially_batched, LEISURE_CONTRACT_V2,
};

/// `leisure.arrow`: one row per leisure AREA source (sports pitch / playground
/// / pool / beer garden). Geometry + `sport` class drive the emission.
pub(super) fn write_leisure(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let n = rows.len();
    let schema = schema_with_contract(
        vec![
            Field::new("osm_id", DataType::Int64, false),
            Field::new("centroid_gx", DataType::Int32, false),
            Field::new("centroid_gy", DataType::Int32, false),
            // emission leisure class id (PITCH/PADEL/…).
            Field::new("sport", DataType::UInt8, false),
            Field::new("opening_hours_frac", DataType::UInt8, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("geom", DataType::Binary, true),
            Field::new("area_m2", DataType::Float32, true),
        ],
        "leisure_contract",
        LEISURE_CONTRACT_V2,
    );

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut cgx = Int32Builder::with_capacity(n);
    let mut cgy = Int32Builder::with_capacity(n);
    let mut sport = UInt8Builder::with_capacity(n);
    let mut opening = UInt8Builder::with_capacity(n);
    let mut name = StringBuilder::with_capacity(n, n * 8);
    let mut geom = BinaryBuilder::with_capacity(n, n * 100);
    let mut area_m2 = Float32Builder::with_capacity(n);
    let mut row_bboxes = Vec::with_capacity(n);

    for row in rows {
        // TSV: sq(0) osm_id(1) c_gx(2) c_gy(3) sport(4) opening_hours(5)
        //      name(6) ring(7)
        if row.len() < 7 {
            continue;
        }
        let c_gx = parse_grid_cell(&row[2]);
        let c_gy = parse_grid_cell(&row[3]);
        let ring = decode_tsv_ring(row.get(7).map(|s| s.as_str()).unwrap_or(""));
        row_bboxes.push(polygon_row_bbox(ring.as_deref(), c_gx, c_gy));
        osm_id.append_value(row[1].parse().unwrap_or(0));
        cgx.append_value(c_gx);
        cgy.append_value(c_gy);
        sport.append_value(row[4].parse().unwrap_or(0));
        opening.append_value(row.get(5).and_then(|s| s.parse().ok()).unwrap_or(0));
        name.append_value(row.get(6).unwrap_or(&String::new()));
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
            Arc::new(sport.finish()),
            Arc::new(opening.finish()),
            Arc::new(name.finish()),
            Arc::new(geom.finish()),
            Arc::new(area_m2.finish()),
        ],
        &row_bboxes,
    )
}
