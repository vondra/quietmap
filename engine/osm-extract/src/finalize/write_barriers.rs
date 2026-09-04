//! `barriers.arrow` writer: one row per barrier microsegment (walls/fences/noise
//! barriers) with integer grid geometry + height + material + height tier
//! (0 mapped / 2 defaulted — the structure-table ladder tier the merge carries
//! through). See `finalize` for dispatch.

use anyhow::Result;
use arrow::array::*;
use arrow::datatypes::*;
use std::path::Path;
use std::sync::Arc;

use super::{parse_grid_cell, segment_row_bbox, write_arrow_spatially_batched};

pub(super) fn write_barriers(rows: &[Vec<String>], path: &Path) -> Result<()> {
    let n = rows.len();
    let schema = Schema::new(vec![
        Field::new("osm_id", DataType::Int64, false),
        Field::new("segment_idx", DataType::Int16, false),
        Field::new("start_gx", DataType::Int32, false),
        Field::new("start_gy", DataType::Int32, false),
        Field::new("end_gx", DataType::Int32, false),
        Field::new("end_gy", DataType::Int32, false),
        Field::new("length_m", DataType::Float32, false),
        Field::new("height", DataType::Float32, false),
        Field::new("material", DataType::UInt8, false),
        Field::new("height_tier", DataType::UInt8, false),
    ]);

    let mut osm_id = Int64Builder::with_capacity(n);
    let mut seg_idx = Int16Builder::with_capacity(n);
    let mut sgx = Int32Builder::with_capacity(n);
    let mut sgy = Int32Builder::with_capacity(n);
    let mut egx = Int32Builder::with_capacity(n);
    let mut egy = Int32Builder::with_capacity(n);
    let mut len = Float32Builder::with_capacity(n);
    let mut height = Float32Builder::with_capacity(n);
    let mut material = UInt8Builder::with_capacity(n);
    let mut height_tier = UInt8Builder::with_capacity(n);
    let mut row_bboxes = Vec::with_capacity(n);

    for row in rows {
        if row.len() < 10 {
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
        height.append_value(row[8].parse().unwrap_or(3.0));
        material.append_value(row[9].parse().unwrap_or(0));
        // The tier is spilled with the row; a pre-tier TSV (a finalize rerun
        // over an old spill) marks the defaulted height as tier 2.
        height_tier.append_value(row.get(10).and_then(|v| v.parse().ok()).unwrap_or(2));
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
            Arc::new(height.finish()),
            Arc::new(material.finish()),
            Arc::new(height_tier.finish()),
        ],
        &row_bboxes,
    )
}
